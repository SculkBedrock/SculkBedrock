//! Player respawn (death-screen button flow).
//!
//! No respawn-point component existed on the player entity, so this module
//! owns one: [`RespawnPoint`] (block coords, defaults to world spawn; beds
//! update it later). Flow:
//!
//! 1. network translates `PlayerAction(RESPAWN)` into [`RespawnRequest`]
//!    (protocol-independent, per the game/network boundary);
//! 2. [`respawn_tick`] validates `Dead`, restores health/hunger/saturation,
//!    teleports to the respawn point (authoritative `Transform` write plus
//!    `force_position` arming so movement does not rubber-band), and
//!    publishes `MovePlayer{Teleport}` + `SyncHealth` (full attribute table,
//!    covers the hunger bar) + `RespawnPlayer` (dismisses the death screen).
//!
//! Requests for live players are ignored (fail-closed); missing attributes
//! or worlds reject without state changes.

use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;
use sc_ecs::event::Event;
use sc_ecs::params::event::EventReader;
use sc_ecs::params::resource::ResMut;
use sc_ecs::world::World;
use sc_entity::motion::{Position, Transform, Velocity};
use sc_packloader::definitions::attribute::EntityAttributes;

use crate::damage::apply::Dead;
use crate::net::{
    IntentPublisher, MovePlayer, NetworkOutbox, PlayerMoveMode, RespawnPlayer, SyncAirSupply,
    SyncHealth,
};

/// 权威重生点，使用方块坐标保存。
///
/// Authoritative respawn point stored in block coordinates.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RespawnPoint {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// 网络域提交的重生按钮请求。
///
/// Respawn button request submitted by the network domain.
#[derive(Event, Clone, Debug)]
pub struct RespawnRequest {
    pub entity: EntityId,
}

/// Respawn refill targets: full food and saturation.
pub const RESPAWN_SATURATION: f32 = 20.0;

/// Whether this player's world opts into instant respawn
/// (`doImmediateRespawn`; vanilla default false: missing worlds fail
/// closed to the death-screen button flow).
pub fn immediate_respawn_enabled(world: &World, entity: &EntityId) -> bool {
    let Some(world_id) = world.get_component::<sc_world::manager::MinecraftWorldId>(entity) else {
        return false;
    };
    let Some(manager) = world.get_resource::<sc_world::manager::MinecraftWorldManager>() else {
        return false;
    };
    manager
        .get_world(&world_id)
        .map(|minecraft_world| {
            minecraft_world
                .world_data
                .gamerules
                .get_bool(&sc_utils::game::gamerules::GameRule::DoImmediateRespawn)
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// Run [`execute_respawn`] immediately when the world opts in; returns
/// whether a respawn happened. Death sites call this right after marking
/// `Dead` so button and instant flows share one execution path.
pub fn maybe_immediate_respawn(
    world: &World,
    entity: &EntityId,
    runtime_id: u64,
    publisher: &mut IntentPublisher<'_>,
) -> bool {
    if !immediate_respawn_enabled(world, entity) {
        return false;
    }
    execute_respawn(world, entity, runtime_id, publisher)
}

/// Ensure a respawn point, returning it. Existing components are reused
/// verbatim; otherwise world spawn is stored and returned.
pub fn ensure_respawn_point(
    world: &World,
    entity: &EntityId,
    world_id: &sc_world::manager::MinecraftWorldId,
) -> Option<(i32, i32, i32)> {
    if let Some(point) = world.get_component::<RespawnPoint>(entity) {
        return Some((point.x, point.y, point.z));
    }
    let manager = world.get_resource::<sc_world::manager::MinecraftWorldManager>()?;
    let minecraft_world = manager.get_world(world_id)?;
    let data = &minecraft_world.world_data;
    let point = RespawnPoint {
        x: data.spawn_x,
        y: data.spawn_y,
        z: data.spawn_z,
    };
    world.add_component(entity, point);
    Some((point.x, point.y, point.z))
}

/// Execute one respawn. Returns false (no state touched, no intents) unless
/// the player is `Dead` with health attributes, a transform, and a
/// resolvable spawn.
pub fn execute_respawn(
    world: &World,
    entity: &EntityId,
    runtime_id: u64,
    publisher: &mut IntentPublisher<'_>,
) -> bool {
    if !world.get_component::<Dead>(entity).is_some_and(|d| d.0) {
        return false;
    }
    let Some(world_id) = world.get_component::<sc_world::manager::MinecraftWorldId>(entity) else {
        return false;
    };
    let Some((x, y, z)) = ensure_respawn_point(world, entity, &world_id) else {
        return false;
    };
    let (Some(attributes), Some(transform)) = (
        world.get_component::<EntityAttributes>(entity),
        world.get_component::<Transform>(entity),
    ) else {
        return false;
    };
    let (Some(mut health), Some(mut hunger), Some(mut saturation), Some(mut exhaustion)) = (
        attributes.get_mut(EntityAttributes::MAX_HEALTH),
        attributes.get_mut(EntityAttributes::MAX_HUNGER),
        attributes.get_mut(EntityAttributes::SATURATION),
        attributes.get_mut(EntityAttributes::EXHAUSTION),
    ) else {
        return false;
    };
    // Restore vitals first (fail-closed ordering: no teleport on half state).
    health.current_value = health.max_value;
    hunger.current_value = hunger.max_value;
    saturation.current_value = RESPAWN_SATURATION.min(saturation.max_value);
    exhaustion.current_value = exhaustion.min_value;
    drop((health, hunger, saturation, exhaustion));
    if let Some(mut air) = attributes.get_mut(EntityAttributes::AIR) {
        air.current_value = air.max_value;
        publisher.publish(SyncAirSupply {
            entity_id: runtime_id,
            air_supply: air.max_value as i16,
            max_air: air.max_value as i16,
            is_breathing: true,
        });
    }
    if let Some(clock) = world.get_component::<crate::oxygen::OxygenClock>(entity) {
        let mut data = clock.inner.write();
        data.drowning_ticks = 0;
        data.is_breathing = true;
        if let Some(air) = attributes.get(EntityAttributes::AIR) {
            data.last_synced_air = air.max_value as i16;
        }
    }
    world.remove_component::<Dead>(entity);

    // Authoritative teleport: Transform write plus movement reset arming.
    let feet = Position::new(x as f32, y as f32, z as f32);
    {
        let mut tf = transform.inner.write();
        tf.position = feet;
        tf.velocity = Velocity::new(0.0, 0.0, 0.0);
        tf.on_ground = true;
        tf.broadcasted_position = feet;
        tf.broadcasted_velocity = Velocity::new(0.0, 0.0, 0.0);
    }
    if let Some(movement) = world.get_component::<crate::movement::PlayerMovement>(entity) {
        movement.state.write().force_position = Some(feet);
    }

    publisher.publish(MovePlayer {
        entity_id: runtime_id,
        x: feet.x,
        y: feet.y,
        z: feet.z,
        yaw: 0.0,
        pitch: 0.0,
        head_yaw: 0.0,
        on_ground: true,
        mode: PlayerMoveMode::Teleport,
    });
    publisher.publish(SyncHealth {
        entity_id: runtime_id,
    });
    publisher.publish(RespawnPlayer {
        entity_id: runtime_id,
        x: feet.x,
        y: feet.y,
        z: feet.z,
    });
    // Keep the runtime-id index usable for the network mapping layer.
    let _ = world.get_component::<sc_entity::MinecraftEntityId>(entity);
    true
}

/// Death-screen button driver (owner `Update`): drains [`RespawnRequest`].
/// Bounded by request count; each request validates independently.
pub fn respawn_tick(
    world: World,
    mut reader: EventReader<RespawnRequest>,
    mut outbox: ResMut<NetworkOutbox>,
) {
    let mut publisher = IntentPublisher::new(&mut outbox);
    for request in reader.read() {
        let entity = request.entity;
        let Some(actor) = world.get_component::<sc_entity::MinecraftEntityId>(&entity) else {
            continue;
        };
        execute_respawn(&world, &entity, actor.0, &mut publisher);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_entity::MinecraftEntityId;
    use sc_item::PlayerInventory;
    use sc_packloader::definitions::attribute::{EntityAttribute, EntityAttributes};
    use sc_utils::game::difficulty::Difficulty;

    fn attribute(value: f32, max: f32) -> EntityAttribute {
        EntityAttribute {
            min_value: 0.0,
            max_value: max,
            default_min_value: 0.0,
            default_max_value: max,
            default_value: value,
            current_value: value,
            should_send: true,
        }
    }

    fn dead_player() -> (World, EntityId) {
        let world = World::new();
        let entity = world.spawn(sc_utils::components::DisplayName(
            "respawn-test".to_string(),
        ));
        let mut map = std::collections::HashMap::new();
        map.insert(
            EntityAttributes::MAX_HEALTH.to_string(),
            attribute(0.0, 20.0),
        );
        map.insert(
            EntityAttributes::MAX_HUNGER.to_string(),
            attribute(3.0, 20.0),
        );
        map.insert(
            EntityAttributes::SATURATION.to_string(),
            attribute(0.0, 20.0),
        );
        map.insert(
            EntityAttributes::EXHAUSTION.to_string(),
            attribute(2.0, 40.0),
        );
        world.add_component(&entity, EntityAttributes::from_map(map));
        world.add_component(&entity, PlayerInventory::new(36));
        world.add_component(&entity, MinecraftEntityId(9));
        world.add_component(
            &entity,
            Transform::new(
                Position::new(100.0, 50.0, 100.0),
                sc_entity::motion::Rotation::default(),
            ),
        );
        world.add_component(&entity, Dead(true));
        let mut manager = sc_world::manager::MinecraftWorldManager::new();
        let mut data = sc_utils::world::data::MinecraftWorldData::default();
        data.difficulty = Difficulty::Normal;
        data.spawn_x = 0;
        data.spawn_y = 64;
        data.spawn_z = 0;
        let provider = sc_world::storage::WorldChunkProvider::new(std::sync::Arc::new(
            sc_world::storage::EmptyWorldStorage,
        ));
        // NOTE: `push_world` assigns its own id (ignores the constructed
        // one); the entity must bind the RETURNED id or lookups miss.
        let world_id = manager.push_world(
            sc_utils::world::r#type::WorldType::Overworld,
            sc_world::world::MinecraftWorld::new(
                sc_world::manager::MinecraftWorldId::random(),
                "test".into(),
                std::path::PathBuf::new(),
                data,
                provider,
            ),
        );
        world.add_component(&entity, world_id);
        world.insert_resource(manager);
        (world, entity)
    }

    #[test]
    fn respawn_restores_teleports_and_syncs() {
        let (world, entity) = dead_player();
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        assert!(execute_respawn(&world, &entity, 9, &mut publisher));
        assert!(world.get_component::<Dead>(&entity).is_none());
        let attributes = world
            .get_component::<EntityAttributes>(&entity)
            .expect("attributes");
        assert_eq!(
            attributes
                .get(EntityAttributes::MAX_HEALTH)
                .expect("health")
                .current_value,
            20.0
        );
        assert_eq!(
            attributes
                .get(EntityAttributes::MAX_HUNGER)
                .expect("hunger")
                .current_value,
            20.0
        );
        assert_eq!(
            attributes
                .get(EntityAttributes::SATURATION)
                .expect("saturation")
                .current_value,
            20.0,
            "respawn refills food and saturation"
        );
        // Teleported to world spawn (respawn point ensured).
        let transform = world
            .get_component::<Transform>(&entity)
            .expect("transform");
        let position = transform.inner.read().position;
        assert_eq!((position.x, position.y, position.z), (0.0, 64.0, 0.0));
        let point = world
            .get_component::<RespawnPoint>(&entity)
            .expect("respawn point ensured");
        assert_eq!((point.x, point.y, point.z), (0, 64, 0));
        // Three facts published (teleport + health + death-screen dismiss).
        assert_eq!(outbox.pending(), 3);
    }

    #[test]
    fn immediate_respawn_follows_gamerule() {
        use sc_utils::game::gamerules::{GameRule, GameRuleType, GameRuleValue};
        // Default (false): stays dead for the button flow.
        let (world, entity) = dead_player();
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        assert!(!maybe_immediate_respawn(&world, &entity, 9, &mut publisher));
        assert!(world.get_component::<Dead>(&entity).is_some_and(|d| d.0));
        // Opt-in: same tick, fully respawned (teleport + vitals + 3 facts).
        let (world, entity) = dead_player();
        let world_id = world
            .get_component::<sc_world::manager::MinecraftWorldId>(&entity)
            .expect("world binding");
        world
            .get_resource_mut::<sc_world::manager::MinecraftWorldManager>()
            .expect("manager")
            .get_world_mut(&world_id)
            .expect("world")
            .world_data
            .gamerules
            .change(
                GameRule::DoImmediateRespawn,
                GameRuleValue::new(GameRuleType::Bool(true)),
            );
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        assert!(maybe_immediate_respawn(&world, &entity, 9, &mut publisher));
        assert!(world.get_component::<Dead>(&entity).is_none());
        let transform = world
            .get_component::<Transform>(&entity)
            .expect("transform");
        let position = transform.inner.read().position;
        assert_eq!((position.x, position.y, position.z), (0.0, 64.0, 0.0));
        assert_eq!(outbox.pending(), 3);
    }

    #[test]
    fn respawn_reuses_existing_point_and_ignores_living() {
        let (world, entity) = dead_player();
        world.add_component(
            &entity,
            RespawnPoint {
                x: 10,
                y: 65,
                z: -5,
            },
        );
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        assert!(execute_respawn(&world, &entity, 9, &mut publisher));
        let transform = world
            .get_component::<Transform>(&entity)
            .expect("transform");
        let position = transform.inner.read().position;
        assert_eq!((position.x, position.y, position.z), (10.0, 65.0, -5.0));
        // Living players ignore the button.
        world.remove_component::<Dead>(&entity);
        assert!(!execute_respawn(&world, &entity, 9, &mut publisher));
    }
}
