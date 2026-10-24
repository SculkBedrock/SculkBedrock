//! Game-side movement systems (protocol/connection agnostic).
//!
//! - `PlayerMovement`: player input buffer plus validation state;
//! - `movement_input_drain`: authoritative validation (distance, speed,
//!   collision, void fallback); violations emit reset intents plus
//!   `force_position` debouncing;
//! - `movement_physics`: gravity and per-axis collision for non-players;
//! - `movement_broadcast`: threshold-deduped outbound move intents.
//!
//! This module does not depend on sc_network.

use parking_lot::RwLock;
use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;
use sc_ecs::params::resource::ResMut;
use sc_ecs::world::World;
use sc_entity::motion::{Aabb, FallState, PhysicsBody, Position, Rotation, Transform};
use sc_utils::game::client::MinecraftClient;
use sc_utils::game::structs::position::MinecraftPosition;
use sc_world::block_dictionary::air_runtime_id;
use sc_world::chunk::ChunkPosition;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use sc_world::storage::ChunkKey;
use std::collections::VecDeque;
use std::sync::Arc;

use crate::damage::{apply_fall_damage, decide_fall_damage, FallDamageOutcome, LandingMaterial};
use crate::net::{
    IntentPublisher, MoveEntityAbsolute, MovePlayer, NetworkOutbox, PendingConnectionFaults,
    PlayerInput, PlayerMoveMode, SetEntityMotion, SyncPlayerSwimming,
};

/// Input buffer cap per player: packets beyond it are dropped.
pub const MAX_PENDING_INPUT: usize = 64;

/// 当前玩家动作标志，来自最近一次消费的输入。
///
/// Current player action flags mirrored from the latest consumed input.
#[derive(Component, Clone, Debug, Default)]
pub struct PlayerActionState {
    pub inner: Arc<RwLock<PlayerActionFlags>>,
}

/// 单个 tick 的疾跑、潜行、飞行与游泳状态镜像。
///
/// Per-tick mirror of sprint, sneak, flight, and swimming state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlayerActionFlags {
    pub sprinting: bool,
    pub sneaking: bool,
    pub flying: bool,
    pub swimming: bool,
}

impl PlayerActionState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Query whether the given entity is currently sneaking.
///
/// Checks both the tick-level [`PlayerActionState`] mirror and the most recent
/// unprocessed input in [`PlayerMovement`], returning `true` if either indicates sneaking.
pub fn is_player_sneaking(world: &World, entity: &EntityId) -> bool {
    if let Some(action) = world.get_component::<PlayerActionState>(entity) {
        if action.inner.read().sneaking {
            return true;
        }
    }
    if let Some(movement) = world.get_component::<PlayerMovement>(entity) {
        if let Some(last_input) = movement.state.read().input_buffer.back() {
            if last_input.sneaking {
                return true;
            }
        }
    }
    false
}

pub mod swimming;
pub use swimming::{
    detect_immersion, evaluate_swimming_transition, get_or_create_swimming, has_headroom,
    is_water_block_at, is_water_fluid_name, ImmersionDepth, Swimming, SwimmingState,
};


/// 玩家移动状态机组件。
///
/// Player movement state-machine component.
#[derive(Component, Clone, Debug)]
pub struct PlayerMovement {
    pub state: Arc<RwLock<PlayerMovementState>>,
}

impl PlayerMovement {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(PlayerMovementState::default())),
        }
    }
}

impl Default for PlayerMovement {
    fn default() -> Self {
        Self::new()
    }
}

/// 可变的玩家移动状态。
///
/// Mutable player movement state.
#[derive(Clone, Debug)]
pub struct PlayerMovementState {
    /// Pending inputs (pushed by the network domain, consumed by drain).
    pub input_buffer: VecDeque<PlayerInput>,
    /// Authoritative position: reset target; inputs ignored until reached.
    pub force_position: Option<Position>,
    /// Teleport target: movement inputs ignored until done.
    pub teleport_position: Option<Position>,
    pub teleport_ticks: u32,
    /// First-packet adoption (avoids false resets on join).
    pub first_move: bool,
    /// Last rotation (broadcast dedup baseline).
    pub last_rotation: Rotation,
    /// Authoritative server position.
    pub last_position: Option<Position>,
}

impl Default for PlayerMovementState {
    fn default() -> Self {
        Self {
            input_buffer: VecDeque::new(),
            force_position: None,
            teleport_position: None,
            teleport_ticks: 0,
            first_move: true,
            last_rotation: Rotation::default(),
            last_position: None,
        }
    }
}

impl PlayerMovementState {
    /// Enqueue one input (drops newest beyond the cap).
    pub fn push_input(&mut self, packet: PlayerInput, cap: usize) {
        if self.input_buffer.len() < cap {
            self.input_buffer.push_back(packet);
        }
    }

    /// Take one input.
    pub fn pop_input(&mut self) -> Option<PlayerInput> {
        self.input_buffer.pop_front()
    }
}

// Authoritative server validation.

/// Consume the input buffer each tick, validating and landing or
/// rolling back (outbound intents to the network domain).
pub fn movement_input_drain(world: World, mut outbox: ResMut<NetworkOutbox>) {
    let mut faults = world.get_resource_mut::<PendingConnectionFaults>();
    let mut publisher = IntentPublisher::new(&mut outbox)
        .with_world(&world)
        .maybe_with_faults(faults.as_deref_mut());
    for entity in world.entities_with_component::<PlayerMovement>() {
        let Some(movement) = world.get_component::<PlayerMovement>(&entity) else {
            continue;
        };
        let Some(client) = world.get_component::<MinecraftClient>(&entity) else {
            continue;
        };
        let Some(entity_id) = world.get_component::<sc_entity::MinecraftEntityId>(&entity) else {
            continue;
        };
        let Some(world_id) = world.get_component::<MinecraftWorldId>(&entity) else {
            continue;
        };
        let Some(world_manager) = world.get_resource::<MinecraftWorldManager>() else {
            continue;
        };
        let physics = world
            .get_component::<PhysicsBody>(&entity)
            .map(|p| p.as_ref().clone())
            .unwrap_or_else(PhysicsBody::player);

        let mut state = movement.state.write();
        // Teleport lock: ignore movement inputs until done.
        if state.teleport_position.is_some() {
            state.input_buffer.clear();
            // Teleport breaks the fall chain (resets fall distance).
            if let Some(fall) = world.get_component::<FallState>(&entity) {
                let pos = state.last_position.unwrap_or_else(|| {
                    let p = client.data.read().position;
                    Position::new(p.x, p.y, p.z)
                });
                fall.reset(pos.y, true);
            }
            continue;
        }

        let inputs: Vec<PlayerInput> = state.input_buffer.drain(..).collect();
        if inputs.is_empty() {
            continue;
        }
        // Mirror the latest input flags for action consumers (hunger drain
        // reads them; movement is the sole writer). Taken before the loop
        // consumes the vector below.
        let action_flags = inputs
            .last()
            .map(|input| (input.sprinting, input.sneaking, input.flying));

        let mut server_pos = state.last_position.unwrap_or_else(|| {
            let pos = client.data.read().position;
            Position::new(pos.x, pos.y, pos.z)
        });
        let mut rotation = state.last_rotation;

        for input in &inputs {
            let client_feet = Position::new(input.feet_x, input.feet_y, input.feet_z);

            // First-packet adoption: trust the client position.
            if state.first_move {
                state.first_move = false;
                server_pos = client_feet;
                rotation = Rotation::new(input.yaw, input.pitch, input.head_yaw);
                apply_position(&client, server_pos);
                state.last_position = Some(server_pos);
                state.last_rotation = rotation;
                ensure_fall_state(&world, &entity, server_pos.y, false);
                continue;
            }

            // force_position debounce: keep resetting until back.
            if let Some(force) = state.force_position {
                if client_feet.distance_squared(&force) > 0.1 {
                    push_reset(
                        &mut publisher,
                        &mut state,
                        entity_id.0,
                        &force,
                        &rotation,
                        &physics,
                    );
                    continue;
                }
                state.force_position = None;
            }

            // Distance check (entry 100, normal 9, falling relaxed to 49).
            let dy = client_feet.y - server_pos.y;
            let d2 = client_feet.distance_squared(&server_pos);
            if d2 > 100.0 {
                push_reset(
                    &mut publisher,
                    &mut state,
                    entity_id.0,
                    &server_pos,
                    &rotation,
                    &physics,
                );
                continue;
            }
            let max_dist = if dy < 2.0 { 49.0 } else { 9.0 };
            if d2 > max_dist {
                push_reset(
                    &mut publisher,
                    &mut state,
                    entity_id.0,
                    &server_pos,
                    &rotation,
                    &physics,
                );
                continue;
            }

            // Horizontal speed check (maximum 6.0).
            let dx = client_feet.x - server_pos.x;
            let dz = client_feet.z - server_pos.z;
            if dx * dx + dz * dz > physics.max_speed * physics.max_speed {
                push_reset(
                    &mut publisher,
                    &mut state,
                    entity_id.0,
                    &server_pos,
                    &rotation,
                    &physics,
                );
                continue;
            }

            // Collision: target AABB against loaded solid blocks.
            let target = physics.aabb.at(client_feet.x, client_feet.y, client_feet.z);
            if world_collides(&world_manager, &world_id, &target) {
                continue;
            }

            // Void fallback: freeze at the last valid position.
            if let Some(minecraft_world) = world_manager.get_world(&world_id) {
                let (min_y, _) = minecraft_world.vertical_bounds();
                if client_feet.y < (min_y as f32) - 4.0 {
                    push_reset(
                        &mut publisher,
                        &mut state,
                        entity_id.0,
                        &server_pos,
                        &rotation,
                        &physics,
                    );
                    continue;
                }
            }

            // Land position and rotation.
            server_pos = client_feet;
            rotation = Rotation::new(input.yaw, input.pitch, input.head_yaw);
            state.last_position = Some(server_pos);
            state.last_rotation = rotation;
            apply_position(&client, server_pos);

            // Server-authoritative ground probe (never trust client onGround).
            let on_ground = {
                let below = physics
                    .aabb
                    .at(server_pos.x, server_pos.y - 0.02, server_pos.z);
                world_collides(&world_manager, &world_id, &below)
            };
            // Write the motion Transform component.
            if let Some(transform) = world.get_component::<Transform>(&entity) {
                let mut tf = transform.inner.write();
                tf.position = server_pos;
                tf.rotation = rotation;
                tf.on_ground = on_ground;
            }
            // Fall settlement: air-to-ground transition settles exactly once.
            settle_player_fall(
                &world,
                &world_manager,
                &world_id,
                &entity,
                entity_id.0,
                &client,
                server_pos,
                on_ground,
                input.sneaking,
                &physics,
                &mut publisher,
            );
            // Chunk membership change.
        }
        // Swimming state machine and component resolution:
        let current_swimming = get_or_create_swimming(&world, &entity).unwrap_or_default();
        let immersion = detect_immersion(&world_manager, &world_id, &server_pos, physics.aabb.max_y);
        let req_start = inputs.iter().any(|i| i.start_swimming);
        let req_stop = inputs.iter().any(|i| i.stop_swimming);
        let can_stand = has_headroom(&world_manager, &world_id, &server_pos, world_collides);

        let next_swimming = evaluate_swimming_transition(
            current_swimming,
            immersion,
            req_start,
            req_stop,
            can_stand,
        );

        if next_swimming.is_swimming != current_swimming.is_swimming {
            world.add_component(&entity, next_swimming);
            let mut new_physics = physics.clone();
            if next_swimming.is_swimming {
                new_physics.aabb = Aabb::from_size(0.3, 0.6);
            } else {
                new_physics.aabb = Aabb::from_size(0.3, 1.8);
            }
            // Bedrock protocol player wire coordinates (MovePlayer / PlayerAuthInput)
            // are always anchored at feet + 1.62. Modifying base_offset causes client-server
            // reset loops and position desync.
            new_physics.base_offset = 1.62;
            world.add_component(&entity, new_physics);

            publisher.publish(SyncPlayerSwimming {
                entity_id: entity_id.0,
                is_swimming: next_swimming.is_swimming,
            });
        } else if next_swimming != current_swimming {
            world.add_component(&entity, next_swimming);
        }

        if let Some((sprinting, sneaking, flying)) = action_flags {
            let action = match world.get_component::<PlayerActionState>(&entity) {
                Some(action) => action,
                None => {
                    world.add_component(&entity, PlayerActionState::new());
                    let Some(action) = world.get_component::<PlayerActionState>(&entity) else {
                        continue;
                    };
                    action
                }
            };
            let mut flags = action.inner.write();
            flags.sprinting = sprinting;
            flags.sneaking = sneaking;
            flags.flying = flying;
            flags.swimming = next_swimming.is_swimming;
        }
    }
}

/// Record one reset: force_position debounce plus outbound reset intent.
fn push_reset(
    publisher: &mut IntentPublisher<'_>,
    state: &mut PlayerMovementState,
    entity_id: u64,
    target: &Position,
    rotation: &Rotation,
    physics: &PhysicsBody,
) {
    state.force_position = Some(*target);
    // Correction packets are lossless facts: on budget shortfall record
    // the terminal state. `force_position` stays armed, so persistent
    // drift resets again next tick without a retry queue here.
    publisher.publish(MovePlayer {
        entity_id,
        x: target.x,
        y: target.y + physics.base_offset,
        z: target.z,
        yaw: rotation.yaw,
        pitch: rotation.pitch,
        head_yaw: rotation.head_yaw,
        on_ground: false,
        mode: PlayerMoveMode::Reset,
    });
}

/// Land: write the authoritative client position.
fn apply_position(client: &MinecraftClient, pos: Position) {
    client.data.write().position = MinecraftPosition::new(pos.x, pos.y, pos.z);
}

// Non-player entity physics.

/// Non-player entity physics: gravity plus per-axis collision.
///
/// Mob fall damage settles here (owner tick, `is_player=false` so the
/// `fallDamage` gamerule is ignored for mobs). Entities without
/// `MinecraftEntityId` or `EntityAttributes.health` track fall state but
/// still skip damage (e.g. demo/region entities before spawners run).
pub fn movement_physics(world: World, mut outbox: ResMut<NetworkOutbox>) {
    let Some(world_manager) = world.get_resource::<MinecraftWorldManager>() else {
        return;
    };
    let mut faults = world.get_resource_mut::<PendingConnectionFaults>();
    let mut publisher = IntentPublisher::new(&mut outbox)
        .with_world(&world)
        .maybe_with_faults(faults.as_deref_mut());
    for entity in world.entities_with_component::<Transform>() {
        if world.get_component::<PlayerMovement>(&entity).is_some() {
            continue;
        }
        let Some(physics) = world.get_component::<PhysicsBody>(&entity) else {
            continue;
        };
        let Some(world_id) = world.get_component::<MinecraftWorldId>(&entity) else {
            continue;
        };
        let Some(transform) = world.get_component::<Transform>(&entity) else {
            continue;
        };

        let mut tf = transform.inner.write();
        let mut vel = tf.velocity;
        if !physics.no_clip && !physics.flying {
            vel.y -= physics.gravity;
            if vel.y < -physics.terminal_velocity {
                vel.y = -physics.terminal_velocity;
            }
        }

        let mut pos = tf.position;
        let mut on_ground = tf.on_ground;

        let target = physics.aabb.at(pos.x + vel.x, pos.y, pos.z);
        if !world_collides(&world_manager, &world_id, &target) {
            pos.x += vel.x;
        } else {
            vel.x = 0.0;
        }

        let target = physics.aabb.at(pos.x, pos.y, pos.z + vel.z);
        if !world_collides(&world_manager, &world_id, &target) {
            pos.z += vel.z;
        } else {
            vel.z = 0.0;
        }

        let target = physics.aabb.at(pos.x, pos.y + vel.y, pos.z);
        if !world_collides(&world_manager, &world_id, &target) {
            pos.y += vel.y;
            on_ground = false;
        } else {
            if vel.y < 0.0 {
                on_ground = true;
            }
            vel.y = 0.0;
        }

        if on_ground {
            vel.x *= 0.5;
            vel.z *= 0.5;
        }

        tf.position = pos;
        tf.velocity = vel;
        tf.on_ground = on_ground;
        drop(tf);
        // Mob fall settlement: same air-to-ground state machine as players,
        // `is_player=false` (gamerule ignored), no sneaking, flight from
        // the physics flag only (mobs have no AdventureSettings).
        if world.get_component::<FallState>(&entity).is_none() {
            world.add_component(&entity, FallState::new(pos.y, on_ground));
            continue;
        }
        let Some(fall) = world.get_component::<FallState>(&entity) else {
            continue;
        };
        let Some(distance) = fall.observe(pos.y, on_ground) else {
            continue;
        };
        fall.reset(pos.y, on_ground);
        let landing = landing_material(&world_manager, &world_id, pos);
        match decide_fall_damage(distance, landing, false, false, physics.flying, true, false) {
            FallDamageOutcome::NoDamage => {}
            FallDamageOutcome::Damage(damage) => {
                let Some(runtime) = world.get_component::<sc_entity::MinecraftEntityId>(&entity)
                else {
                    continue;
                };
                log::debug!(
                    "mob fall damage: entity={} distance={:.2} damage={:.2} landing={:?}",
                    runtime.0,
                    distance,
                    damage,
                    landing
                );
                apply_fall_damage(&world, &entity, runtime.0, damage, &mut publisher);
            }
        }
    }
}

// Threshold-deduped broadcast (outbound intents).

/// Threshold-deduped movement broadcast.
///
/// Each mover pushes exactly one intent; receiver selection lives in the
/// network mapping layer.
pub fn movement_broadcast(world: World, mut outbox: ResMut<NetworkOutbox>) {
    let mut faults = world.get_resource_mut::<PendingConnectionFaults>();
    let mut publisher = IntentPublisher::new(&mut outbox)
        .with_world(&world)
        .maybe_with_faults(faults.as_deref_mut());
    for entity in world.entities_with_component::<Transform>() {
        // Dead entities stop broadcasting: the client already received
        // HURT(/DEATH); further Move/Motion would resurrect the actor
        // visually. Health resync still flows via SyncHealth facts.
        if world
            .get_component::<crate::damage::Dead>(&entity)
            .is_some_and(|d| d.0)
        {
            continue;
        }
        let Some(entity_id) = world.get_component::<sc_entity::MinecraftEntityId>(&entity) else {
            continue;
        };
        let Some(transform) = world.get_component::<Transform>(&entity) else {
            continue;
        };
        let is_player = world.get_component::<PlayerMovement>(&entity).is_some();

        let (moved, velocity_changed, pos, rot, vel, on_ground) = {
            let mut tf = transform.inner.write();
            let pos = tf.position;
            let rot = tf.rotation;
            let vel = tf.velocity;
            let dp = pos.distance_squared(&tf.broadcasted_position);
            let dr = (rot.yaw - tf.broadcasted_rotation.yaw).powi(2)
                + (rot.pitch - tf.broadcasted_rotation.pitch).powi(2);
            let dm = (vel.x - tf.broadcasted_velocity.x).powi(2)
                + (vel.y - tf.broadcasted_velocity.y).powi(2)
                + (vel.z - tf.broadcasted_velocity.z).powi(2);
            let moved = dp > 0.0001 || dr > 1.0;
            let velocity_changed = dm > 0.0025;
            if moved {
                tf.broadcasted_position = pos;
                tf.broadcasted_rotation = rot;
            }
            if velocity_changed {
                tf.broadcasted_velocity = vel;
            }
            (moved, velocity_changed, pos, rot, vel, tf.on_ground)
        };

        if moved {
            if is_player {
                let base_offset = world
                    .get_component::<PhysicsBody>(&entity)
                    .map(|physics| physics.base_offset)
                    .unwrap_or(1.62);
                publisher.publish(MovePlayer {
                    entity_id: entity_id.0,
                    x: pos.x,
                    y: pos.y + base_offset,
                    z: pos.z,
                    yaw: rot.yaw,
                    pitch: rot.pitch,
                    head_yaw: rot.head_yaw,
                    on_ground,
                    mode: PlayerMoveMode::Normal,
                });
            } else {
                publisher.publish(MoveEntityAbsolute {
                    entity_id: entity_id.0,
                    x: pos.x,
                    y: pos.y,
                    z: pos.z,
                    yaw: rot.yaw,
                    pitch: rot.pitch,
                    head_yaw: rot.head_yaw,
                    on_ground,
                });
            }
        }

        if velocity_changed {
            publisher.publish(SetEntityMotion {
                entity_id: entity_id.0,
                x: vel.x,
                y: vel.y,
                z: vel.z,
            });
        }
    }
}

/// Whether a block runtime ID represents a solid collision obstacle.
/// Air, water, flowing water, lava, and bubble columns are non-solid fluids/passable.
pub(crate) fn is_solid_block(runtime_id: u32, air: u32) -> bool {
    if runtime_id == air {
        return false;
    }
    let dict = sc_world::block_dictionary::BlockStateDictionary::global();
    if let Some(entry) = dict.get(runtime_id) {
        if matches!(
            entry.name.as_str(),
            "minecraft:water"
                | "minecraft:flowing_water"
                | "minecraft:lava"
                | "minecraft:flowing_lava"
                | "minecraft:bubble_column"
        ) {
            return false;
        }
    }
    true
}

/// Whether the entity is currently submerged in water for swimming purposes.
#[inline]
pub fn is_in_water(
    world_manager: &MinecraftWorldManager,
    world_id: &MinecraftWorldId,
    pos: &Position,
    height: f32,
) -> bool {
    detect_immersion(world_manager, world_id, pos, height).can_swim()
}

/// Whether a target AABB collides with loaded non-air blocks (cache only).
///
/// `pub(crate)`: shared with region physics.
pub(crate) fn world_collides(
    world_manager: &MinecraftWorldManager,
    world_id: &MinecraftWorldId,
    aabb: &Aabb,
) -> bool {
    let Some(minecraft_world) = world_manager.get_world(world_id) else {
        return false;
    };
    let dimension = minecraft_world.world_data.get_dimension();
    let air = air_runtime_id();

    let min_x = aabb.min_x.floor() as i32;
    let max_x = (aabb.max_x - 0.0001).floor() as i32;
    let min_y = aabb.min_y.floor() as i32;
    let max_y = (aabb.max_y - 0.0001).floor() as i32;
    let min_z = aabb.min_z.floor() as i32;
    let max_z = (aabb.max_z - 0.0001).floor() as i32;

    for bx in min_x..=max_x {
        for bz in min_z..=max_z {
            let key = ChunkKey::new(dimension, ChunkPosition::from_world(bx, bz));
            let Some(column) = minecraft_world.chunk_provider.cached_chunk(key) else {
                continue;
            };
            let chunk = column.read();
            let lx = bx.rem_euclid(16) as u8;
            let lz = bz.rem_euclid(16) as u8;
            for by in min_y..=max_y {
                if let Some(runtime_id) = chunk.block_at(lx, by, lz) {
                    if is_solid_block(runtime_id.0, air) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Ensure a `FallState` exists (login fallback + first-move adoption).
fn ensure_fall_state(
    world: &World,
    entity: &sc_ecs::entity::EntityId,
    feet_y: f32,
    on_ground: bool,
) {
    if world.get_component::<FallState>(entity).is_none() {
        world.add_component(entity, FallState::new(feet_y, on_ground));
    }
}

/// Settle one authoritative player landing through the fall pipeline.
#[allow(clippy::too_many_arguments)]
fn settle_player_fall(
    world: &World,
    world_manager: &MinecraftWorldManager,
    world_id: &MinecraftWorldId,
    entity: &sc_ecs::entity::EntityId,
    runtime_id: u64,
    client: &MinecraftClient,
    feet: Position,
    on_ground: bool,
    sneaking: bool,
    physics: &PhysicsBody,
    publisher: &mut IntentPublisher<'_>,
) {
    if world
        .get_component::<crate::damage::Dead>(entity)
        .is_some_and(|d| d.0)
    {
        return;
    }
    ensure_fall_state(world, entity, feet.y, on_ground);
    let Some(fall) = world.get_component::<FallState>(entity) else {
        return;
    };
    let Some(distance) = fall.observe(feet.y, on_ground) else {
        return;
    };
    // Landing happens; pin highest to current regardless of damage outcome.
    fall.reset(feet.y, on_ground);

    let landing = landing_material(world_manager, world_id, feet);
    let data = client.data.read();
    let immune_gamemode = data.gamemode.is_creative() || data.gamemode.is_spectator();
    drop(data);
    // Flight immunity: physics flag or AllowFlight ability.
    let mut can_fly = physics.flying;
    if let Some(settings) =
        world.get_component::<sc_entity::player::adventure_settings::AdventureSettings>(entity)
    {
        use sc_entity::player::adventure_settings::AdventureSettingsType;
        if settings.get(&AdventureSettingsType::AllowFlight)
            || settings.get(&AdventureSettingsType::Flying)
        {
            can_fly = true;
        }
    }
    let gamerule_enabled = world_manager
        .get_world(world_id)
        .map(|w| w.world_data.gamerules.fall_damage_enabled())
        .unwrap_or(true);
    match decide_fall_damage(
        distance,
        landing,
        sneaking,
        immune_gamemode,
        can_fly,
        gamerule_enabled,
        true,
    ) {
        FallDamageOutcome::NoDamage => {}
        FallDamageOutcome::Damage(damage) => {
            log::debug!(
                "fall damage: entity={} distance={:.2} damage={:.2} landing={:?}",
                runtime_id,
                distance,
                damage,
                landing
            );
            apply_fall_damage(world, entity, runtime_id, damage, publisher);
        }
    }
}

/// Classify the landing blocks (feet + below, both layers) into water/slime/normal.
///
/// Cache-only (`cached_chunk`): unloaded columns return `Normal` and let the
/// collision probe own the ground truth; never trigger IO here.
fn landing_material(
    world_manager: &MinecraftWorldManager,
    world_id: &MinecraftWorldId,
    feet: Position,
) -> LandingMaterial {
    let Some(minecraft_world) = world_manager.get_world(world_id) else {
        return LandingMaterial::Normal;
    };
    let dimension = minecraft_world.world_data.get_dimension();
    let bx = feet.x.floor() as i32;
    let bz = feet.z.floor() as i32;
    let by = feet.y.floor() as i32;
    let key = ChunkKey::new(dimension, ChunkPosition::from_world(bx, bz));
    let Some(column) = minecraft_world.chunk_provider.cached_chunk(key) else {
        return LandingMaterial::Normal;
    };
    let chunk = column.read();
    let lx = bx.rem_euclid(16) as u8;
    let lz = bz.rem_euclid(16) as u8;
    let dict = sc_world::block_dictionary::BlockStateDictionary::global();
    let is_water = |id: Option<sc_world::chunk::BlockRuntimeId>| {
        id.is_some_and(|id| {
            dict.get(id.0)
                .is_some_and(|entry| entry.name == "minecraft:water")
        })
    };
    let is_slime = |id: Option<sc_world::chunk::BlockRuntimeId>| {
        id.is_some_and(|id| {
            dict.get(id.0)
                .is_some_and(|entry| entry.name == "minecraft:slime")
        })
    };
    // Feet block (either layer) water cushions; below-block slime cushions.
    let feet0 = chunk.block_at_layer(0, lx, by, lz);
    let feet1 = chunk.block_at_layer(1, lx, by, lz);
    if is_water(feet0) || is_water(feet1) {
        return LandingMaterial::Water;
    }
    let below0 = chunk.block_at_layer(0, lx, by - 1, lz);
    let below1 = chunk.block_at_layer(1, lx, by - 1, lz);
    if is_slime(below0) || is_slime(below1) || is_slime(feet0) || is_slime(feet1) {
        return LandingMaterial::Slime;
    }
    LandingMaterial::Normal
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_ecs::world::World;
    use sc_entity::MinecraftEntityId;

    #[test]
    fn get_or_create_swimming_creates_when_missing_and_reuses_when_present() {
        let world = World::new();
        let entity = world.spawn(MinecraftEntityId(1));

        // 1. Initial state: entity does not have Swimming component
        assert!(world.get_component::<Swimming>(&entity).is_none());

        // 2. get_or_create_swimming should create it
        let swimming1 =
            get_or_create_swimming(&world, &entity).expect("should create swimming component");
        assert!(!swimming1.is_swimming);

        // 3. Mutate the state
        world.add_component(&entity, Swimming::new(true));

        // 4. get_or_create_swimming again: should reuse existing component
        let swimming2 =
            get_or_create_swimming(&world, &entity).expect("should reuse swimming component");
        assert!(swimming2.is_swimming);
    }

    #[test]
    fn solid_block_treats_air_as_passable() {
        let air = 0u32;
        assert!(!is_solid_block(air, air));
    }

    #[test]
    fn physics_body_bounding_box_and_offset_for_swimming() {
        let mut body = PhysicsBody::player();
        assert_eq!(body.aabb.max_y - body.aabb.min_y, 1.8);
        assert_eq!(body.base_offset, 1.62);

        // Switch to swimming dimensions: collision box height changes to 0.6m,
        // but base_offset remains 1.62 because Bedrock player wire coordinates
        // (MovePlayerPacket / PlayerAuthInputPacket) are always anchored at feet + 1.62.
        body.aabb = Aabb::from_size(0.3, 0.6);
        body.base_offset = 1.62;
        assert_eq!(body.aabb.max_y - body.aabb.min_y, 0.6);
        assert_eq!(body.base_offset, 1.62);

        // Reset to standing dimensions (1.8m height, 1.62m eye height)
        body.aabb = Aabb::from_size(0.3, 1.8);
        body.base_offset = 1.62;
        assert_eq!(body.aabb.max_y - body.aabb.min_y, 1.8);
        assert_eq!(body.base_offset, 1.62);
    }

    #[test]
    fn swimming_transition_logic() {
        // 1. In water, start_swimming input -> transitions to swimming
        let in_water = true;
        let inputs = vec![PlayerInput {
            start_swimming: true,
            ..PlayerInput::default()
        }];
        let req_start = inputs.iter().any(|i| i.start_swimming);
        let req_stop = inputs.iter().any(|i| i.stop_swimming);
        let current_swimming = false;
        let next_swimming = if current_swimming {
            if !in_water || req_stop { false } else { true }
        } else if req_start && in_water {
            true
        } else {
            false
        };
        assert!(next_swimming, "explicit start_swimming in water must enter swimming");

        // 2. Sprinting in water without start_swimming flag -> does NOT synthesize start
        let inputs_sprint_only = vec![PlayerInput {
            sprinting: true,
            move_z: 1.0,
            ..PlayerInput::default()
        }];
        let req_start_sprint = inputs_sprint_only.iter().any(|i| i.start_swimming);
        assert!(!req_start_sprint, "sprinting must not synthesize swimming start without client flag");

        // 3. Swimming, leaves water (!in_water) -> exits swimming immediately
        let in_water_surfaced = false;
        let inputs_normal = vec![PlayerInput {
            sprinting: true,
            ..PlayerInput::default()
        }];
        let req_start = inputs_normal.iter().any(|i| i.start_swimming);
        let req_stop = inputs_normal.iter().any(|i| i.stop_swimming);
        let current_swimming = true;
        let next_swimming = if current_swimming {
            if !in_water_surfaced || req_stop { false } else { true }
        } else if req_start && in_water_surfaced {
            true
        } else {
            false
        };
        assert!(!next_swimming, "swimming must terminate when entity surfaces/leaves water");
    }

    #[test]
    fn swimming_immersion_probes_torso_and_head() {
        // When swimming prone with height 0.6:
        // If bottom of prone box reaches y = 63.8 (where block 63 is water and block 64 is air):
        let pos_y = 63.8f32;
        let height = 0.6f32;
        let feet_y = pos_y.floor() as i32;
        let waist_y = (pos_y + 0.4 * height).floor() as i32;
        let head_y = (pos_y + 0.8 * height).floor() as i32;

        assert_eq!(feet_y, 63, "feet are still in block 63");
        assert_eq!(waist_y, 64, "waist has emerged into air at block 64");
        assert_eq!(head_y, 64, "head has emerged into air at block 64");
        // Because is_in_water checks waist_y and head_y instead of feet_y,
        // this prone player is correctly recognized as having breached the surface into air.
    }
}
