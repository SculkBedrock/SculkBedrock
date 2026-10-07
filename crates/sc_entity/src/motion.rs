//! Shared entity motion data model.
//!
//! Defines **data** only, no behavior: physics/collision/broadcast are consumed by
//! `sc_movement`-side ECS systems. Position data held by legacy components such as
//! `MinecraftClient` stays as the network presentation layer after migrating to this module's `Position`.

use parking_lot::RwLock;
use std::sync::Arc;
use sc_ecs::component::Component;

/// Entity feet coordinates (world coords, f32; Bedrock network/save semantics use feet y).
#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
pub struct Position {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Position {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// Squared distance (for threshold compares without sqrt).
    pub fn distance_squared(&self, other: &Position) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        dx * dx + dy * dy + dz * dz
    }
}

/// Last-tick position snapshot: basis for broadcast threshold dedup.
#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
pub struct PrevPosition(pub Position);

/// Entity velocity (m/s tick semantics).
#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
pub struct Velocity {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Velocity {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn length_squared(&self) -> f32 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }
}

/// Last-tick velocity snapshot: for SetEntityMotionPacket dedup.
#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
pub struct PrevVelocity(pub Velocity);

/// Facing (yaw/pitch degrees, head_yaw for the player view).
#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
pub struct Rotation {
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
}

impl Rotation {
    pub const fn new(yaw: f32, pitch: f32, head_yaw: f32) -> Self {
        Self {
            yaw,
            pitch,
            head_yaw,
        }
    }
}

/// Grounded flag (maintained by the 0.01-below collision probe).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct OnGround(pub bool);

/// Axis-aligned bounding box (world coords; feet-based, min_y == feet).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Aabb {
    pub min_x: f32,
    pub min_y: f32,
    pub min_z: f32,
    pub max_x: f32,
    pub max_y: f32,
    pub max_z: f32,
}

impl Aabb {
    /// Builds from feet center (x, y, z) with the given half-width and height.
    pub fn from_size(half_width: f32, height: f32) -> Self {
        Self {
            min_x: -half_width,
            min_y: 0.0,
            min_z: -half_width,
            max_x: half_width,
            max_y: height,
            max_z: half_width,
        }
    }

    /// Translates to the given feet coordinates.
    pub fn at(&self, x: f32, y: f32, z: f32) -> Self {
        Self {
            min_x: self.min_x + x,
            min_y: self.min_y + y,
            min_z: self.min_z + z,
            max_x: self.max_x + x,
            max_y: self.max_y + y,
            max_z: self.max_z + z,
        }
    }

    /// Translates by (dx, dy, dz).
    pub fn offset(&self, dx: f32, dy: f32, dz: f32) -> Self {
        Self {
            min_x: self.min_x + dx,
            min_y: self.min_y + dy,
            min_z: self.min_z + dz,
            max_x: self.max_x + dx,
            max_y: self.max_y + dy,
            max_z: self.max_z + dz,
        }
    }

    /// Whether it intersects another AABB (inclusive bounds).
    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min_x < other.max_x
            && self.max_x > other.min_x
            && self.min_y < other.max_y
            && self.max_y > other.min_y
            && self.min_z < other.max_z
            && self.max_z > other.min_z
    }
}

/// Physics parameters (consumed by the sc_movement systems).
#[derive(Component, Clone, Debug)]
pub struct PhysicsBody {
    /// Entity collision box (feet-relative coords).
    pub aabb: Aabb,
    /// Gravity (default 0.08 / tick squared).
    pub gravity: f32,
    /// Terminal velocity (3.92).
    pub terminal_velocity: f32,
    /// Step height (0.6).
    pub step_height: f32,
    /// Horizontal speed cap (6.0, for validation).
    pub max_speed: f32,
    /// No-clip mode (spectator/debug).
    pub no_clip: bool,
    /// Flying mode (player, for the validation branch).
    pub flying: bool,
    /// Eye-height offset (player 1.62; MovePlayerPacket y = feet + base_offset).
    pub base_offset: f32,
    /// Current grounded flag (mirrors the OnGround component, read directly on physics hot paths).
    pub on_ground: bool,
}

impl Default for PhysicsBody {
    fn default() -> Self {
        Self {
            aabb: Aabb::from_size(0.3, 1.8),
            gravity: 0.08,
            terminal_velocity: 3.92,
            step_height: 0.6,
            max_speed: 6.0,
            no_clip: false,
            flying: false,
            base_offset: 1.62,
            on_ground: false,
        }
    }
}

impl PhysicsBody {
    /// Default player physics body (0.6 wide / 1.8 tall).
    pub fn player() -> Self {
        Self::default()
    }
}

/// Entity motion-state component: carries position/velocity/facing/grounded uniformly.
///
/// ECS has no `&mut` component access (`get_component -> Arc<C>`), so mutable state uses an
/// in-component lock (same pattern as `MinecraftClient.data`).
/// The lock holds the last-broadcast snapshot; `movement_broadcast` threshold dedup
/// compares and updates under the same write lock.
#[derive(Component, Clone, Debug)]
pub struct Transform {
    pub inner: Arc<RwLock<TransformData>>,
}

/// Motion-state data (Copy value bundle, all fields read/written under one lock).
#[derive(Clone, Copy, Debug, Default)]
pub struct TransformData {
    pub position: Position,
    pub rotation: Rotation,
    pub velocity: Velocity,
    pub on_ground: bool,
    /// Last-broadcast position snapshot (threshold-dedup basis).
    pub broadcasted_position: Position,
    /// Last-broadcast facing snapshot.
    pub broadcasted_rotation: Rotation,
    /// Last-broadcast velocity snapshot.
    pub broadcasted_velocity: Velocity,
}

impl Transform {
    pub fn new(position: Position, rotation: Rotation) -> Self {
        Self {
            inner: Arc::new(RwLock::new(TransformData {
                position,
                rotation,
                ..TransformData::default()
            })),
        }
    }

    pub fn read(&self) -> parking_lot::RwLockReadGuard<'_, TransformData> {
        self.inner.read()
    }

    pub fn write(&self) -> parking_lot::RwLockWriteGuard<'_, TransformData> {
        self.inner.write()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aabb_intersection_and_offset() {
        let body = Aabb::from_size(0.3, 1.8);
        let a = body.at(0.0, 64.0, 0.0);
        assert!(a.intersects(&body.at(0.4, 64.0, 0.0)));
        assert!(!a.intersects(&body.at(1.0, 64.0, 0.0)));
        let shifted = a.offset(1.0, 0.0, -1.0);
        assert!((shifted.min_x - 0.7).abs() < 1e-6);
        assert!((shifted.max_z + 0.7).abs() < 1e-6);
    }

    #[test]
    fn position_distance_squared() {
        let a = Position::new(0.0, 0.0, 0.0);
        let b = Position::new(3.0, 4.0, 0.0);
        assert_eq!(a.distance_squared(&b), 25.0);
    }

    #[test]
    fn player_physics_defaults_match_reference() {
        let body = PhysicsBody::player();
        assert_eq!(body.max_speed, 6.0);
        assert_eq!(body.base_offset, 1.62);
        assert_eq!(body.aabb.max_y, 1.8);
    }
}
