//! Authoritative fall-damage application (owner-tick only).
//!
//! Health lives in `EntityAttributes.minecraft:health.current_value` (default
//! 20). This module mutates it under the owner tick and publishes reliable
//! facts; the network domain owns wire encoding. Blood-loss is never rolled
//! back on outbox pressure: rejection escalates to connection isolation via
//! `IntentPublisher` (same path as other lifecycle facts without resync).

use sc_ecs::component::Component;
use sc_ecs::world::World;
use sc_entity::MinecraftEntityId;

use crate::damage::{calc_fall_damage, FallDamageInput, FallDamageOutcome, LandingMaterial};
use crate::net::{HurtEntity, IntentPublisher, SyncHealth};

/// Dead marker: stops repeat fall settlement after fatal damage.
/// Respawn / revive removes it (P4 scope; MVP only sets it).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Dead(pub bool);

/// Result of one authoritative fall settlement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FallSettlement {
    pub damage: f32,
    pub health: f32,
    pub fatal: bool,
}

/// Classify landing + run the pure formula. Pure except for the `sneaking`
/// flag passed in by the caller (no World access here for testability).
pub fn decide_fall_damage(
    fall_distance: f32,
    landing: LandingMaterial,
    sneaking: bool,
    immune_gamemode: bool,
    can_fly: bool,
    gamerule_enabled: bool,
    is_player: bool,
) -> FallDamageOutcome {
    calc_fall_damage(FallDamageInput {
        fall_distance,
        landing,
        sneaking,
        immune_gamemode,
        can_fly,
        gamerule_enabled,
        is_player,
        has_slow_falling: false,
        jump_boost: 0.0,
        ride_reduction: 0.0,
    })
}

/// Apply settled fall damage to authoritative health and publish facts.
/// Returns `None` when there is no damage (gates/offset), `Some` on damage.
///
/// Steps (all synchronous, no await, no Chunk/Network locks held):
/// 1. read `EntityAttributes.minecraft:health`, subtract, clamp to `[min,max]`;
/// 2. set `Dead` on `health <= 0`;
/// 3. publish `SyncHealth` + `HurtEntity{fatal}` via `IntentPublisher`
///    (backpressure escalates to connection fault; health is not rolled back).
pub fn apply_fall_damage(
    world: &World,
    entity: &sc_ecs::entity::EntityId,
    runtime_id: u64,
    damage: f32,
    publisher: &mut IntentPublisher<'_>,
) -> Option<FallSettlement> {
    if !(damage.is_finite() && damage > 0.0) {
        return None;
    }
    if world.get_component::<Dead>(entity).is_some_and(|d| d.0) {
        return None;
    }
    let Some(attributes) =
        world.get_component::<sc_packloader::definitions::attribute::EntityAttributes>(entity)
    else {
        return None;
    };
    let health_key = sc_packloader::definitions::attribute::EntityAttributes::MAX_HEALTH;
    let Some(mut attr) = attributes.get_mut(health_key) else {
        return None;
    };
    let next = (attr.current_value - damage).clamp(attr.min_value, attr.max_value);
    attr.current_value = next;
    drop(attr);
    let fatal = next <= 0.0;
    if fatal {
        world.remove_component::<Dead>(entity);
        world.add_component(entity, Dead(true));
    }
    publisher.publish(SyncHealth {
        entity_id: runtime_id,
    });
    publisher.publish(HurtEntity {
        entity_id: runtime_id,
        fatal,
    });
    if fatal {
        // Gamerule opt-in skips the death screen through the same path.
        crate::respawn::maybe_immediate_respawn(world, entity, runtime_id, publisher);
    }
    // Keep the runtime-id index usable for the network mapping layer.
    let _ = world.get_component::<MinecraftEntityId>(entity);
    Some(FallSettlement {
        damage,
        health: next,
        fatal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_ecs::world::World;
    use sc_entity::MinecraftEntityId;
    use sc_packloader::definitions::attribute::{EntityAttribute, EntityAttributes};

    fn world_with_health(health: f32) -> (World, sc_ecs::entity::EntityId) {
        let world = World::new();
        let entity = world.spawn(sc_utils::components::DisplayName("fall-test".to_string()));
        let mut map = std::collections::HashMap::new();
        map.insert(
            EntityAttributes::MAX_HEALTH.to_string(),
            EntityAttribute {
                min_value: 0.0,
                max_value: 20.0,
                default_min_value: 0.0,
                default_max_value: 20.0,
                default_value: 20.0,
                current_value: health,
                should_send: true,
            },
        );
        world.add_component(&entity, EntityAttributes::from_map(map));
        world.add_component(&entity, MinecraftEntityId(42));
        (world, entity)
    }

    #[test]
    fn non_positive_damage_applies_nothing() {
        let (world, entity) = world_with_health(20.0);
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        assert!(apply_fall_damage(&world, &entity, 42, 0.0, &mut publisher).is_none());
        assert!(outbox.is_empty());
    }

    #[test]
    fn damage_reduces_health_and_publishes_two_facts() {
        let (world, entity) = world_with_health(20.0);
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        let settlement =
            apply_fall_damage(&world, &entity, 42, 5.0, &mut publisher).expect("damage applies");
        assert_eq!(settlement.health, 15.0);
        assert!(!settlement.fatal);
        assert_eq!(outbox.pending(), 2);
    }

    #[test]
    fn lethal_damage_marks_dead_and_reports_fatal() {
        let (world, entity) = world_with_health(5.0);
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        let settlement =
            apply_fall_damage(&world, &entity, 42, 10.0, &mut publisher).expect("lethal applies");
        assert!(settlement.fatal);
        assert_eq!(settlement.health, 0.0);
        // Second settlement is suppressed while dead.
        let mut outbox2 = crate::net::NetworkOutbox::default();
        let mut publisher2 = IntentPublisher::new(&mut outbox2);
        assert!(apply_fall_damage(&world, &entity, 42, 5.0, &mut publisher2).is_none());
    }
}
