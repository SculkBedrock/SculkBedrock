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
use sc_ecs::params::resource::ResMut;
use sc_ecs::world::World;
use sc_entity::motion::{Aabb, PhysicsBody, Position, Rotation, Transform};
use sc_utils::game::client::MinecraftClient;
use sc_utils::game::structs::position::MinecraftPosition;
use sc_world::block_dictionary::air_runtime_id;
use sc_world::chunk::ChunkPosition;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use sc_world::storage::ChunkKey;
use std::collections::VecDeque;
use std::sync::Arc;

use crate::net::{NetworkIntent, NetworkOutbox, PlayerInput, PlayerMoveMode};
use crate::net_backpressure::IntentPublisher;
use crate::net_faults::PendingConnectionFaults;

/// Input buffer cap per player: packets beyond it are dropped.
pub const MAX_PENDING_INPUT: usize = 64;

/// Player movement state machine component.
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
            continue;
        }

        let inputs: Vec<PlayerInput> = state.input_buffer.drain(..).collect();
        if inputs.is_empty() {
            continue;
        }

        let mut server_pos = state.last_position.unwrap_or_else(|| {
            let pos = client.data.read().position;
            Position::new(pos.x, pos.y, pos.z)
        });
        let mut rotation = state.last_rotation;

        for input in inputs {
            let client_feet = Position::new(input.feet_x, input.feet_y, input.feet_z);

            // First-packet adoption: trust the client position.
            if state.first_move {
                state.first_move = false;
                server_pos = client_feet;
                rotation = Rotation::new(input.yaw, input.pitch, input.head_yaw);
                apply_position(&client, server_pos);
                state.last_position = Some(server_pos);
                state.last_rotation = rotation;
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

            // Write the motion Transform component.
            if let Some(transform) = world.get_component::<Transform>(&entity) {
                let below = physics
                    .aabb
                    .at(server_pos.x, server_pos.y - 0.02, server_pos.z);
                let on_ground = world_collides(&world_manager, &world_id, &below);
                let mut tf = transform.inner.write();
                tf.position = server_pos;
                tf.rotation = rotation;
                tf.on_ground = on_ground;
            }
            // Chunk membership change.
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
    publisher.publish(NetworkIntent::MovePlayer {
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
pub fn movement_physics(world: World) {
    let Some(world_manager) = world.get_resource::<MinecraftWorldManager>() else {
        return;
    };
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
                publisher.publish(NetworkIntent::MovePlayer {
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
                publisher.publish(NetworkIntent::MoveEntityAbsolute {
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
            publisher.publish(NetworkIntent::SetEntityMotion {
                entity_id: entity_id.0,
                x: vel.x,
                y: vel.y,
                z: vel.z,
            });
        }
    }
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
                    if runtime_id.0 != air {
                        return true;
                    }
                }
            }
        }
    }
    false
}
