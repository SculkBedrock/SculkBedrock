//! Survival hunger tick (player-only, difficulty-aware).
//!
//! Hunger state lives in [`EntityAttributes`] (`minecraft:player.hunger` /
//! `.saturation` / `.exhaustion`) so it syncs to the client through the
//! existing `SyncHealth` full-table path; no new attribute intent is needed.
//! Server-only timers live in [`HungerClock`] (never synced).
//!
//! Rules shape (timers, floors, cadences):
//! - exhaustion crosses 4.0 → -1 saturation, else -1 hunger, looping
//!   within the adding call (no spillover from fractional saturation);
//! - only sprinting charges distance (0.1/block); walking is free;
//!   takeoff on the 3rd consecutive airborne tick costs the jump fee
//!   (0.05, 0.2 while sprinting); displacement under 0.05 is ignored;
//!   flight, teleports and peaceful difficulty accrue nothing; food ≤ 6
//!   sprints at walk cost (which is zero);
//! - hunger 0 starves on an 80-tick cadence down to a difficulty floor
//!   (Easy 10 HP, Normal 1 HP, Hard no floor); starvation death sets `Dead`;
//! - hunger ≥ 18 regenerates 1 HP per 80 ticks at +6.0 exhaustion, gated
//!   on `naturalRegeneration`;
//! - Peaceful refills +1 food per 10 ticks and mends +1 HP per 20 ticks;
//!   starvation never fires.
//! Only Survival/Adventure tick; creative/spectator, flying and `Dead`
//! players skip the whole tick. Mining (+0.005/landed survival break)
//! feeds exhaustion through [`add_exhaustion`]; swim, melee and status
//! hooks are extension points (no water/combat/effect signals yet).

use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;
use sc_ecs::params::resource::ResMut;
use sc_ecs::world::World;
use sc_packloader::definitions::attribute::EntityAttributes;
use sc_utils::game::difficulty::Difficulty;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::damage::apply::Dead;
use crate::net::{HurtEntity, IntentPublisher, NetworkOutbox, SyncHealth, SyncHunger};

/// Hunger cap (also the respawn refill target).
pub const HUNGER_MAX: f32 = 20.0;
/// Saturation cap.
pub const SATURATION_MAX: f32 = 20.0;
/// Exhaustion safety ceiling (Bedrock single-moment cap is 20; the
/// cascade below normally keeps the remainder under 4).
pub const EXHAUSTION_MAX: f32 = 20.0;
/// Exhaustion threshold: crossing it drains 1 saturation (else 1 hunger).
pub const EXHAUSTION_THRESHOLD: f32 = 4.0;
/// Minimum per-tick displacement that counts (micro-jitter ignored).
pub const MOVE_THRESHOLD: f32 = 0.05;
/// Exhaustion per block moved while sprinting (walking itself is free).
pub const SPRINT_EXHAUSTION_PER_BLOCK: f32 = 0.1;
/// Takeoff cost on the 3rd consecutive airborne tick (0.2 while sprinting).
pub const JUMP_EXHAUSTION: f32 = 0.05;
pub const SPRINT_JUMP_EXHAUSTION: f32 = 0.2;
/// Mining cost per landed break (survival only).
pub const MINE_EXHAUSTION_PER_BLOCK: f32 = 0.005;
/// Starvation/regen cadence, in ticks.
pub const HUNGER_PHASE_INTERVAL: u8 = 80;
/// Peaceful mend cadence, in ticks.
pub const PEACEFUL_MEND_INTERVAL: u8 = 20;
/// Food level that enables regeneration.
pub const REGEN_FOOD_MIN: f32 = 18.0;
/// Exhaustion cost of one regeneration tick (cascades naturally).
pub const REGEN_EXHAUSTION_COST: f32 = 6.0;
/// Starvation HP floors by difficulty (Hard has none: floor 0 kills).
pub const STARVE_FLOOR_EASY: f32 = 10.0;
pub const STARVE_FLOOR_NORMAL: f32 = 1.0;

/// 仅服务端使用的饥饿阶段与里程状态，不参与同步。
///
/// Server-only hunger phase and odometry state, never synchronized.
#[derive(Component, Clone, Debug, Default)]
pub struct HungerClock {
    pub inner: Arc<RwLock<HungerClockData>>,
}

/// 饥饿阶段计时器与最近一次权威位置、腾空状态快照。
///
/// Phase timer plus the latest authoritative position and airborne snapshot.
#[derive(Clone, Copy, Debug, Default)]
pub struct HungerClockData {
    pub ticks: u16,
    pub last_x: f32,
    pub last_y: f32,
    pub last_z: f32,
    pub air_ticks: u8,
    pub initialized: bool,
}

impl HungerClock {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Add exhaustion (activity hook: sprinting, jumping, mining, regen
/// costs...). Cascades immediately (4.0 → -1 saturation, else -1 hunger,
/// looping: one call can cross several thresholds), matching the reference
/// setter semantics. Skips `Dead` players, non-survival gamemodes and
/// peaceful worlds. Returns true when values changed; the per-tick hunger
/// pass syncs active players same tick.
pub fn add_exhaustion(world: &World, entity: &EntityId, amount: f32) -> bool {
    if !(amount.is_finite() && amount > 0.0) {
        return false;
    }
    if world.get_component::<Dead>(entity).is_some_and(|d| d.0) {
        return false;
    }
    let Some(attributes) = world.get_component::<EntityAttributes>(entity) else {
        return false;
    };
    let (Some(mut hunger), Some(mut saturation), Some(mut exhaustion)) = (
        attributes.get_mut(EntityAttributes::MAX_HUNGER),
        attributes.get_mut(EntityAttributes::SATURATION),
        attributes.get_mut(EntityAttributes::EXHAUSTION),
    ) else {
        return false;
    };
    if !drain_enabled(world, entity) {
        return false;
    }
    let before = (
        hunger.current_value,
        saturation.current_value,
        exhaustion.current_value,
    );
    let hunger_max = hunger.max_value;
    let saturation_max = saturation.max_value;
    let exhaustion_max = exhaustion.max_value;
    exhaustion.current_value = (exhaustion.current_value + amount).min(exhaustion_max);
    cascade_exhaustion(
        &mut hunger.current_value,
        &mut saturation.current_value,
        &mut exhaustion.current_value,
        hunger_max,
        saturation_max,
    );
    (
        hunger.current_value,
        saturation.current_value,
        exhaustion.current_value,
    ) != before
}

/// Whether exhaustion may accrue here: survival/adventure, non-flying,
/// difficulty above Peaceful. Mirrors the reference enable gates minus
/// systems that do not exist yet (status effects, level hunger flag).
fn drain_enabled(world: &World, entity: &EntityId) -> bool {
    use sc_utils::game::gamemode::Gamemode;
    let Some(client) = world.get_component::<sc_utils::game::client::MinecraftClient>(entity)
    else {
        return false;
    };
    if !matches!(
        client.data.read().gamemode,
        Gamemode::Survival | Gamemode::Adventure
    ) {
        return false;
    }
    if world
        .get_component::<crate::movement::PlayerActionState>(entity)
        .is_some_and(|action| action.inner.read().flying)
    {
        return false;
    }
    let Some(world_id) = world.get_component::<sc_world::manager::MinecraftWorldId>(entity) else {
        return false;
    };
    let Some(manager) = world.get_resource::<sc_world::manager::MinecraftWorldManager>() else {
        return false;
    };
    manager.get_world(&world_id).is_some_and(|minecraft_world| {
        !matches!(
            minecraft_world.world_data.difficulty,
            sc_utils::game::difficulty::Difficulty::Peaceful
        )
    })
}

/// Cascade crossed thresholds immediately: 4.0 → -1 saturation, else
/// -1 hunger (no spillover from fractional saturation). Loops: one call
/// can cross several thresholds on big drains.
fn cascade_exhaustion(
    hunger: &mut f32,
    saturation: &mut f32,
    exhaustion: &mut f32,
    hunger_max: f32,
    saturation_max: f32,
) {
    *hunger = hunger.clamp(0.0, hunger_max);
    *saturation = saturation.clamp(0.0, saturation_max);
    while *exhaustion >= EXHAUSTION_THRESHOLD {
        *exhaustion -= EXHAUSTION_THRESHOLD;
        if *saturation > 0.0 {
            *saturation = (*saturation - 1.0).max(0.0);
        } else if *hunger > 0.0 {
            *hunger = (*hunger - 1.0).max(0.0);
        }
    }
}

fn starve_floor(difficulty: Difficulty) -> Option<f32> {
    match difficulty {
        Difficulty::Peaceful => None,
        Difficulty::Easy => Some(STARVE_FLOOR_EASY),
        Difficulty::Normal => Some(STARVE_FLOOR_NORMAL),
        Difficulty::Hard => Some(0.0),
    }
}

fn difficulty_of(world: &World, entity: &EntityId) -> Option<(Difficulty, bool)> {
    let world_id = world.get_component::<sc_world::manager::MinecraftWorldId>(entity)?;
    let manager = world.get_resource::<sc_world::manager::MinecraftWorldManager>()?;
    let minecraft_world = manager.get_world(&world_id)?;
    let data = &minecraft_world.world_data;
    let regen = data
        .gamerules
        .get_bool(&sc_utils::game::gamerules::GameRule::NaturalRegeneration)
        .unwrap_or(true);
    Some((data.difficulty.clone(), regen))
}

fn survives_tick(world: &World, entity: &EntityId) -> bool {
    use sc_utils::game::gamemode::Gamemode;
    if world.get_component::<Dead>(entity).is_some_and(|d| d.0) {
        return false;
    }
    // Players only (mobs have no client record); Survival/Adventure only;
    // flying players skip the whole food tick.
    let Some(client) = world.get_component::<sc_utils::game::client::MinecraftClient>(entity)
    else {
        return false;
    };
    let gamemode = client.data.read().gamemode;
    if !matches!(gamemode, Gamemode::Survival | Gamemode::Adventure) {
        return false;
    }
    let flying = world
        .get_component::<crate::movement::PlayerActionState>(entity)
        .is_some_and(|action| action.inner.read().flying);
    !flying
}

/// Movement-driven exhaustion for one tick.
///
/// `position`/`on_ground` are authoritative now; `odometry` is last tick's
/// snapshot (updated here); `flags` mirror the latest input; `hunger` gates
/// the low-food sprint lock (≤ 6 food sprints at walk cost, which is zero).
/// Rules: displacement under 0.05 is ignored; only sprinting charges
/// distance (0.1/block, swimming excluded — no water signal yet); takeoff
/// (3rd consecutive airborne tick) costs the jump fee; teleports resync;
/// flight accrues nothing. Pure snapshot math, no World access.
fn action_drain(
    position: (f32, f32, f32),
    on_ground: bool,
    odometry: &mut HungerClockData,
    flags: crate::movement::PlayerActionFlags,
    hunger: f32,
    teleported: bool,
) -> f32 {
    if on_ground {
        odometry.air_ticks = 0;
    } else {
        odometry.air_ticks = odometry.air_ticks.saturating_add(1);
    }
    let mut drain = 0.0;
    if !odometry.initialized || teleported {
        odometry.initialized = true;
    } else if !flags.flying {
        let dx = position.0 - odometry.last_x;
        let dy = position.1 - odometry.last_y;
        let dz = position.2 - odometry.last_z;
        let distance = (dx * dx + dy * dy + dz * dz).sqrt();
        if distance >= MOVE_THRESHOLD {
            let sprinting = flags.sprinting && hunger > 6.0;
            let swimming = flags.swimming;
            let mut jump = 0.0;
            if odometry.air_ticks == 3 {
                jump = if sprinting {
                    SPRINT_JUMP_EXHAUSTION
                } else {
                    JUMP_EXHAUSTION
                };
            }
            drain += if swimming {
                0.01 * distance
            } else if sprinting {
                SPRINT_EXHAUSTION_PER_BLOCK * distance + jump
            } else {
                jump
            };
        }
    }
    odometry.last_x = position.0;
    odometry.last_y = position.1;
    odometry.last_z = position.2;
    drain
}

/// One hunger tick for a player. Returns true when synced state changed
/// (caller publishes `SyncHealth`). Pure attribute math + intents; never
/// touches chunks, blocks, or the network.
pub fn tick_hunger(
    world: &World,
    entity: &EntityId,
    runtime_id: u64,
    publisher: &mut IntentPublisher<'_>,
) -> bool {
    if !survives_tick(world, entity) {
        return false;
    }
    let Some((difficulty, natural_regen)) = difficulty_of(world, entity) else {
        return false;
    };
    let Some(attributes) = world.get_component::<EntityAttributes>(entity) else {
        return false;
    };
    let (Some(mut hunger), Some(mut saturation), Some(mut exhaustion), Some(mut health)) = (
        attributes.get_mut(EntityAttributes::MAX_HUNGER),
        attributes.get_mut(EntityAttributes::SATURATION),
        attributes.get_mut(EntityAttributes::EXHAUSTION),
        attributes.get_mut(EntityAttributes::MAX_HEALTH),
    ) else {
        return false;
    };
    let hunger_max = hunger.max_value;
    let saturation_max = saturation.max_value;
    let exhaustion_max = exhaustion.max_value;
    hunger.current_value = hunger.current_value.clamp(0.0, hunger_max);
    saturation.current_value = saturation.current_value.clamp(0.0, saturation_max);
    exhaustion.current_value = exhaustion.current_value.clamp(0.0, exhaustion_max);
    // Baseline for change detection: snapshot BEFORE any accrual, so
    // drain-only ticks still publish instead of piggybacking on other
    // syncs.
    let before = (
        health.current_value,
        hunger.current_value,
        saturation.current_value,
        exhaustion.current_value,
    );

    // Phase timer + odometry snapshot (ensure-then-bump; the first tick
    // only initializes the snapshot, never accrues).
    let clock = match world.get_component::<HungerClock>(entity) {
        Some(clock) => clock,
        None => {
            world.add_component(entity, HungerClock::new());
            let Some(clock) = world.get_component::<HungerClock>(entity) else {
                return false;
            };
            clock
        }
    };
    let mut odometry = clock.inner.write();
    let tick = odometry.ticks.wrapping_add(1);
    odometry.ticks = tick;
    let phase = |interval: u8| tick % interval as u16 == 0;

    if matches!(difficulty, Difficulty::Peaceful) {
        let mut hunger_changed = false;
        let mut health_changed = false;
        if hunger.current_value < hunger.max_value && tick % 10 == 0 {
            hunger.current_value = (hunger.current_value + 1.0).min(hunger.max_value);
            hunger_changed = true;
        }
        if health.current_value < health.max_value && phase(PEACEFUL_MEND_INTERVAL) {
            health.current_value = (health.current_value + 1.0).min(health.max_value);
            health_changed = true;
        }
        drop((hunger, saturation, exhaustion, health));
        if hunger_changed {
            publisher.publish(SyncHunger {
                entity_id: runtime_id,
            });
        }
        if health_changed {
            publisher.publish(SyncHealth {
                entity_id: runtime_id,
            });
        }
        return hunger_changed || health_changed;
    }

    // Action drain (movement-driven): sprint distance + takeoff fees.
    // Teleport/reset locks resync the snapshot instead of charging
    // cross-map jumps; flight accrues nothing.
    let mut action_cost = 0.0;
    if let Some(transform) = world.get_component::<sc_entity::motion::Transform>(entity) {
        let tf = transform.inner.read();
        let teleported = world
            .get_component::<crate::movement::PlayerMovement>(entity)
            .is_some_and(|movement| {
                let state = movement.state.read();
                state.force_position.is_some() || state.teleport_position.is_some()
            });
        let flags = world
            .get_component::<crate::movement::PlayerActionState>(entity)
            .map(|action| *action.inner.read())
            .unwrap_or_default();
        action_cost = action_drain(
            (tf.position.x, tf.position.y, tf.position.z),
            tf.on_ground,
            &mut odometry,
            flags,
            hunger.current_value,
            teleported,
        );
    }
    exhaustion.current_value = (exhaustion.current_value + action_cost).min(exhaustion_max);
    cascade_exhaustion(
        &mut hunger.current_value,
        &mut saturation.current_value,
        &mut exhaustion.current_value,
        hunger_max,
        saturation_max,
    );

    let mut fatal = false;
    if hunger.current_value <= 0.0 {
        if let Some(floor) = starve_floor(difficulty) {
            if phase(HUNGER_PHASE_INTERVAL) && health.current_value > floor {
                health.current_value = (health.current_value - 1.0).max(health.min_value);
                if health.current_value <= 0.0 {
                    fatal = true;
                }
            }
        }
    } else if hunger.current_value >= REGEN_FOOD_MIN
        && health.current_value < health.max_value
        && natural_regen
        && phase(HUNGER_PHASE_INTERVAL)
    {
        health.current_value = (health.current_value + 1.0).min(health.max_value);
        exhaustion.current_value =
            (exhaustion.current_value + REGEN_EXHAUSTION_COST).min(exhaustion_max);
    }
    if fatal {
        world.remove_component::<Dead>(entity);
        world.add_component(entity, Dead(true));
    }
    let health_changed = health.current_value != before.0 || fatal;
    let hunger_changed = (
        hunger.current_value,
        saturation.current_value,
        exhaustion.current_value,
    ) != (before.1, before.2, before.3);
    drop((hunger, saturation, exhaustion, health));
    if hunger_changed {
        publisher.publish(SyncHunger {
            entity_id: runtime_id,
        });
    }
    if health_changed {
        publisher.publish(SyncHealth {
            entity_id: runtime_id,
        });
    }
    if fatal {
        publisher.publish(HurtEntity {
            entity_id: runtime_id,
            fatal: true,
        });
        // Gamerule opt-in skips the death screen through the same path.
        crate::respawn::maybe_immediate_respawn(world, entity, runtime_id, publisher);
    }
    health_changed || hunger_changed
}

/// Per-tick hunger driver (owner `Update`): survival/adventure players only.
/// Bounded by player count; skips cheaply on missing components/worlds.
pub fn hunger_tick(world: World, mut outbox: ResMut<NetworkOutbox>) {
    let mut publisher = IntentPublisher::new(&mut outbox);
    for entity in world.entities_with_component::<sc_entity::MinecraftEntityId>() {
        let Some(actor) = world.get_component::<sc_entity::MinecraftEntityId>(&entity) else {
            continue;
        };
        // Hunger is a player system; entities without a player inventory
        // (mobs, items, markers) never tick.
        if world
            .get_component::<sc_item::PlayerInventory>(&entity)
            .is_none()
        {
            continue;
        }
        tick_hunger(&world, &entity, actor.0, &mut publisher);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_entity::MinecraftEntityId;
    use sc_item::PlayerInventory;
    use sc_packloader::definitions::attribute::{EntityAttribute, EntityAttributes};
    use sc_utils::game::client::MinecraftClient;
    use sc_utils::world::client_data::MinecraftClientData;

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

    fn survival_world() -> (World, EntityId) {
        let world = World::new();
        let entity = world.spawn(sc_utils::components::DisplayName("hunger-test".to_string()));
        let mut map = std::collections::HashMap::new();
        map.insert(
            EntityAttributes::MAX_HEALTH.to_string(),
            attribute(20.0, 20.0),
        );
        map.insert(
            EntityAttributes::MAX_HUNGER.to_string(),
            attribute(20.0, 20.0),
        );
        map.insert(
            EntityAttributes::SATURATION.to_string(),
            attribute(5.0, 20.0),
        );
        map.insert(
            EntityAttributes::EXHAUSTION.to_string(),
            attribute(0.0, 40.0),
        );
        world.add_component(&entity, EntityAttributes::from_map(map));
        world.add_component(&entity, PlayerInventory::new(36));
        world.add_component(&entity, MinecraftEntityId(7));
        let client = MinecraftClient::new(entity, world.clone(), MinecraftClientData::default());
        world.add_component(&entity, client);
        (world, entity)
    }

    fn world_with_difficulty(difficulty: Difficulty) -> (World, EntityId) {
        let (world, entity) = survival_world();
        let mut manager = sc_world::manager::MinecraftWorldManager::new();
        let mut data = sc_utils::world::data::MinecraftWorldData::default();
        data.difficulty = difficulty;
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

    fn hunger_of(world: &World, entity: &EntityId, key: &str) -> f32 {
        world
            .get_component::<EntityAttributes>(entity)
            .and_then(|attributes| attributes.get(key))
            .map(|attribute| attribute.current_value)
            .unwrap_or(f32::NAN)
    }

    #[test]
    fn idle_players_do_not_drain() {
        let (world, entity) = world_with_difficulty(Difficulty::Normal);
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        // Standing still accrues nothing (no passive drain): 500 ticks
        // leave saturation, hunger and exhaustion untouched.
        for _ in 0..500 {
            tick_hunger(&world, &entity, 7, &mut publisher);
        }
        assert_eq!(
            hunger_of(&world, &entity, EntityAttributes::SATURATION),
            5.0
        );
        assert_eq!(
            hunger_of(&world, &entity, EntityAttributes::MAX_HUNGER),
            20.0
        );
        assert_eq!(
            hunger_of(&world, &entity, EntityAttributes::EXHAUSTION),
            0.0
        );
    }

    #[test]
    fn action_drain_rates_walk_sprint_jump() {
        use crate::movement::PlayerActionFlags;
        let walk = PlayerActionFlags {
            sprinting: false,
            sneaking: false,
            flying: false,
            swimming: false,
        };
        let sprint = PlayerActionFlags {
            sprinting: true,
            ..walk
        };
        let mut clock = HungerClockData {
            initialized: true,
            ..HungerClockData::default()
        };
        // Walking is free (only sprinting charges distance).
        let drain = action_drain((10.0, 64.0, 0.0), true, &mut clock, walk, 20.0, false);
        assert_eq!(drain, 0.0);
        // Sprint 10 blocks: 10 * 0.1.
        let drain = action_drain((20.0, 64.0, 0.0), true, &mut clock, sprint, 20.0, false);
        assert!((drain - 10.0 * SPRINT_EXHAUSTION_PER_BLOCK).abs() < 1e-6);
        // Takeoff on the 3rd consecutive airborne tick costs the jump fee.
        clock.air_ticks = 2;
        let drain = action_drain((20.0, 64.5, 0.0), false, &mut clock, walk, 20.0, false);
        assert!((drain - JUMP_EXHAUSTION).abs() < 1e-6);
        // Higher while sprinting; later airborne ticks charge no jump.
        clock.air_ticks = 2;
        let drain = action_drain((21.0, 65.0, 0.0), false, &mut clock, sprint, 20.0, false);
        let expected = SPRINT_EXHAUSTION_PER_BLOCK * 1.25f32.sqrt() + SPRINT_JUMP_EXHAUSTION;
        assert!((drain - expected).abs() < 1e-4);
        // Low food locks sprinting (walk cost, which is zero).
        let drain = action_drain((30.0, 65.0, 0.0), true, &mut clock, sprint, 5.0, false);
        assert_eq!(drain, 0.0);
        // Micro-jitter under 0.05 is ignored; flying and teleports accrue
        // nothing (teleport resyncs the snapshot).
        let drain = action_drain((30.01, 65.0, 0.0), true, &mut clock, sprint, 20.0, false);
        assert_eq!(drain, 0.0);
        let fly = PlayerActionFlags {
            flying: true,
            ..walk
        };
        let drain = action_drain((100.0, 64.0, 0.0), true, &mut clock, fly, 20.0, false);
        assert_eq!(drain, 0.0);
        let drain = action_drain((500.0, 64.0, 0.0), true, &mut clock, sprint, 20.0, true);
        assert_eq!(drain, 0.0);
        let after = action_drain((500.0, 64.0, 0.0), true, &mut clock, walk, 20.0, false);
        assert_eq!(after, 0.0, "post-teleport snapshot resynced");
    }

    #[test]
    fn hunger_only_change_publishes_sync_hunger_alone() {
        use crate::movement::PlayerActionState;
        use crate::net::NetworkIntent;
        use sc_entity::motion::{Position, Rotation, Transform};
        let (world, entity) = world_with_difficulty(Difficulty::Normal);
        world.add_component(
            &entity,
            Transform::new(Position::new(0.0, 64.0, 0.0), Rotation::default()),
        );
        let action = PlayerActionState::new();
        action.inner.write().sprinting = true;
        world.add_component(&entity, action);
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        tick_hunger(&world, &entity, 7, &mut publisher);
        let transform = world
            .get_component::<Transform>(&entity)
            .expect("transform");
        transform.inner.write().position = Position::new(10.0, 64.0, 0.0);
        tick_hunger(&world, &entity, 7, &mut publisher);
        let intents = outbox.drain();
        assert!(
            matches!(intents.as_slice(), [NetworkIntent::SyncHunger(_)]),
            "hunger-only change must publish exactly one SyncHunger, got {intents:?}"
        );
    }

    #[test]
    fn health_only_change_publishes_sync_health_alone() {
        use crate::net::NetworkIntent;
        let (world, entity) = world_with_difficulty(Difficulty::Normal);
        let attributes = world
            .get_component::<EntityAttributes>(&entity)
            .expect("attributes");
        attributes
            .get_mut(EntityAttributes::MAX_HUNGER)
            .expect("hunger")
            .current_value = 0.0;
        attributes
            .get_mut(EntityAttributes::SATURATION)
            .expect("saturation")
            .current_value = 0.0;
        attributes
            .get_mut(EntityAttributes::MAX_HEALTH)
            .expect("health")
            .current_value = 15.0;
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        // Ticks 1-79 change nothing; the 80th starves exactly once.
        for _ in 0..80 {
            tick_hunger(&world, &entity, 7, &mut publisher);
        }
        let intents = outbox.drain();
        assert!(
            matches!(intents.as_slice(), [NetworkIntent::SyncHealth(_)]),
            "health-only change must publish exactly one SyncHealth, got {intents:?}"
        );
        assert_eq!(
            hunger_of(&world, &entity, EntityAttributes::MAX_HEALTH),
            14.0
        );
    }

    #[test]
    fn regen_tick_publishes_both_syncs() {
        use crate::net::NetworkIntent;
        let (world, entity) = world_with_difficulty(Difficulty::Normal);
        let attributes = world
            .get_component::<EntityAttributes>(&entity)
            .expect("attributes");
        attributes
            .get_mut(EntityAttributes::MAX_HEALTH)
            .expect("health")
            .current_value = 19.0;
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        for _ in 0..80 {
            tick_hunger(&world, &entity, 7, &mut publisher);
        }
        let intents = outbox.drain();
        assert!(
            matches!(
                intents.as_slice(),
                [NetworkIntent::SyncHunger(_), NetworkIntent::SyncHealth(_)]
            ),
            "regen must publish SyncHunger then SyncHealth, got {intents:?}"
        );
    }

    #[test]
    fn sprinting_covers_ground_at_sprint_rate_live() {
        use crate::movement::PlayerActionState;
        use sc_entity::motion::{Position, Rotation, Transform};
        let (world, entity) = world_with_difficulty(Difficulty::Normal);
        world.add_component(
            &entity,
            Transform::new(Position::new(0.0, 64.0, 0.0), Rotation::default()),
        );
        let action = PlayerActionState::new();
        action.inner.write().sprinting = true;
        world.add_component(&entity, action);
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        // First tick initializes the snapshot.
        tick_hunger(&world, &entity, 7, &mut publisher);
        // Sprint 10 blocks: 10 * 0.1 = 1.0 exhaustion on top of metabolic.
        let transform = world
            .get_component::<Transform>(&entity)
            .expect("transform");
        transform.inner.write().position = Position::new(10.0, 64.0, 0.0);
        tick_hunger(&world, &entity, 7, &mut publisher);
        let exhaustion = hunger_of(&world, &entity, EntityAttributes::EXHAUSTION);
        assert!(
            (exhaustion - 1.0).abs() < 0.05,
            "sprint 10 blocks ≈ 1.0 exhaustion, got {exhaustion}"
        );
    }

    #[test]
    fn starvation_floor_follows_difficulty() {
        for (difficulty, floor) in [
            (Difficulty::Easy, 10.0),
            (Difficulty::Normal, 1.0),
            (Difficulty::Hard, 0.0),
        ] {
            let (world, entity) = world_with_difficulty(difficulty.clone());
            let attributes = world
                .get_component::<EntityAttributes>(&entity)
                .expect("attributes");
            attributes
                .get_mut(EntityAttributes::MAX_HUNGER)
                .expect("hunger")
                .current_value = 0.0;
            attributes
                .get_mut(EntityAttributes::SATURATION)
                .expect("saturation")
                .current_value = 0.0;
            let mut outbox = crate::net::NetworkOutbox::default();
            let mut publisher = IntentPublisher::new(&mut outbox);
            // Starve long enough to bottom out (80-tick cadence).
            for _ in 0..2400 {
                tick_hunger(&world, &entity, 7, &mut publisher);
            }
            let health = hunger_of(&world, &entity, EntityAttributes::MAX_HEALTH);
            assert_eq!(health, floor, "difficulty {difficulty:?}");
        }
    }

    #[test]
    fn hard_starvation_death_marks_dead() {
        let (world, entity) = world_with_difficulty(Difficulty::Hard);
        let attributes = world
            .get_component::<EntityAttributes>(&entity)
            .expect("attributes");
        attributes
            .get_mut(EntityAttributes::MAX_HUNGER)
            .expect("hunger")
            .current_value = 0.0;
        attributes
            .get_mut(EntityAttributes::SATURATION)
            .expect("saturation")
            .current_value = 0.0;
        attributes
            .get_mut(EntityAttributes::MAX_HEALTH)
            .expect("health")
            .current_value = 2.0;
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        for _ in 0..400 {
            tick_hunger(&world, &entity, 7, &mut publisher);
        }
        assert!(world.get_component::<Dead>(&entity).is_some_and(|d| d.0));
    }

    #[test]
    fn peaceful_never_starves_and_mends() {
        let (world, entity) = world_with_difficulty(Difficulty::Peaceful);
        let attributes = world
            .get_component::<EntityAttributes>(&entity)
            .expect("attributes");
        attributes
            .get_mut(EntityAttributes::MAX_HUNGER)
            .expect("hunger")
            .current_value = 5.0;
        attributes
            .get_mut(EntityAttributes::MAX_HEALTH)
            .expect("health")
            .current_value = 10.0;
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        for _ in 0..400 {
            tick_hunger(&world, &entity, 7, &mut publisher);
        }
        assert!(hunger_of(&world, &entity, EntityAttributes::MAX_HUNGER) > 5.0);
        assert!(hunger_of(&world, &entity, EntityAttributes::MAX_HEALTH) > 10.0);
        assert!(world.get_component::<Dead>(&entity).is_none());
    }

    #[test]
    fn exhaustion_hook_cascades_and_rejects() {
        let (world, entity) = world_with_difficulty(Difficulty::Normal);
        // Below threshold: stored as-is, no cascade.
        assert!(add_exhaustion(&world, &entity, 3.0));
        assert_eq!(
            hunger_of(&world, &entity, EntityAttributes::EXHAUSTION),
            3.0
        );
        // One call can cross several thresholds: 3 + 9 = 12 -> two
        // cascades (sat 5 -> 3), remainder 4 -> third cascade (sat 2).
        assert!(add_exhaustion(&world, &entity, 9.0));
        assert_eq!(
            hunger_of(&world, &entity, EntityAttributes::EXHAUSTION),
            0.0
        );
        assert_eq!(
            hunger_of(&world, &entity, EntityAttributes::SATURATION),
            2.0
        );
        // Dead players reject.
        world.remove_component::<Dead>(&entity);
        world.add_component(&entity, Dead(true));
        assert!(!add_exhaustion(&world, &entity, 1.0));
    }

    #[test]
    fn exhaustion_hook_rejects_peaceful() {
        let (world, entity) = world_with_difficulty(Difficulty::Peaceful);
        assert!(!add_exhaustion(&world, &entity, 1.0));
        assert_eq!(
            hunger_of(&world, &entity, EntityAttributes::EXHAUSTION),
            0.0
        );
    }

    #[test]
    fn hunger_respects_custom_attribute_caps() {
        let (world, entity) = world_with_difficulty(Difficulty::Normal);
        let attributes = world
            .get_component::<EntityAttributes>(&entity)
            .expect("attributes");
        // Set custom lower caps: hunger max 12, saturation max 10, exhaustion max 5.
        if let Some(mut h) = attributes.get_mut(EntityAttributes::MAX_HUNGER) {
            h.max_value = 12.0;
            h.current_value = 15.0; // above custom cap
        }
        if let Some(mut s) = attributes.get_mut(EntityAttributes::SATURATION) {
            s.max_value = 10.0;
            s.current_value = 12.0; // above custom cap
        }
        if let Some(mut e) = attributes.get_mut(EntityAttributes::EXHAUSTION) {
            e.max_value = 5.0;
            e.current_value = 6.0; // above custom cap
        }
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        tick_hunger(&world, &entity, 7, &mut publisher);

        assert_eq!(hunger_of(&world, &entity, EntityAttributes::MAX_HUNGER), 12.0);
        assert_eq!(hunger_of(&world, &entity, EntityAttributes::SATURATION), 9.0); // 1 drain from 5 - 4 = 1 remainder
        assert_eq!(hunger_of(&world, &entity, EntityAttributes::EXHAUSTION), 1.0);
    }
}

