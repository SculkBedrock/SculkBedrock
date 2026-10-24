//! 实体运动数据模型。
//!
//! 仅定义数据（位置、速度、朝向、碰撞盒等）；物理、碰撞与广播行为由
//! `sc_movement` 侧的 ECS 系统消费这些组件。
//!
//! Shared entity motion data model.
//!
//! Defines data only (position, velocity, rotation, collision boxes); physics,
//! collision and broadcast behavior live in the `sc_movement` ECS systems.

use parking_lot::RwLock;
use sc_ecs::component::Component;
use std::sync::Arc;

/// 实体脚部坐标（世界坐标，f32；Bedrock 网络与存档语义使用脚部 y 值）。
///
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

    /// 距离平方（用于阈值比较，避免 sqrt）。
    ///
    /// Squared distance (for threshold compares without sqrt).
    pub fn distance_squared(&self, other: &Position) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        dx * dx + dy * dy + dz * dz
    }
}

/// 上一 tick 的位置快照：广播阈值去重的比较基准。
///
/// Last-tick position snapshot: basis for broadcast threshold dedup.
#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
pub struct PrevPosition(pub Position);

/// 实体速度（m/s，tick 语义）。
///
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

/// 上一 tick 的速度快照：用于 SetEntityMotionPacket 去重。
///
/// Last-tick velocity snapshot: for SetEntityMotionPacket dedup.
#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
pub struct PrevVelocity(pub Velocity);

/// 朝向（yaw/pitch 角度；玩家的 head_yaw）。
///
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

/// 着地标志（由下方 0.01 的碰撞探测维护）。
///
/// Grounded flag (maintained by the 0.01-below collision probe).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct OnGround(pub bool);

/// 坠落追踪状态：服务端权威地记录坠落高度峰值与空气-地面转换。
/// `on_ground` 始终来自服务端碰撞探测（`world_collides`），不采信客户端输入。
/// 内部加锁：ECS 组件访问不提供 `&mut`，变更通过内部锁进行。
///
/// Fall tracking state: server-authoritative fall distance/peak tracking.
/// `on_ground` always comes from the server collision probe (`world_collides`);
/// client input is not trusted. Locked internally because ECS component access
/// provides no `&mut`.
#[derive(Component, Clone, Debug, Default)]
pub struct FallState {
    pub inner: Arc<RwLock<FallStateData>>,
}

/// 保存在 `FallState` 锁内的可复制坠落数据。
///
/// Copyable fall data held under the `FallState` lock.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct FallStateData {
    pub highest_y: f32,
    pub was_on_ground: bool,
}

impl FallState {
    pub fn new(feet_y: f32, on_ground: bool) -> Self {
        Self {
            inner: Arc::new(RwLock::new(FallStateData {
                highest_y: feet_y,
                was_on_ground: on_ground,
            })),
        }
    }

    /// 落地、传送或模式切换后重置。
    ///
    /// Reset after landing, teleport, or mode change.
    pub fn reset(&self, feet_y: f32, on_ground: bool) {
        let mut data = self.inner.write();
        data.highest_y = feet_y;
        data.was_on_ground = on_ground;
    }

    /// 观测一个权威位置。返回收敛后的下落距离
    ///
    /// Observe one authoritative position. Returns the settled fall distance
    /// (highest - current) exactly once on the air-to-ground transition.
    pub fn observe(&self, feet_y: f32, on_ground: bool) -> Option<f32> {
        let mut data = self.inner.write();
        if feet_y.is_finite() && feet_y > data.highest_y {
            data.highest_y = feet_y;
        }
        let was_air = !data.was_on_ground;
        data.was_on_ground = on_ground;
        if on_ground && was_air {
            let distance = data.highest_y - feet_y;
            if distance.is_finite() && distance > 0.0 {
                return Some(distance);
            }
        }
        // While continuously grounded, keep highest pinned to avoid drift.
        if on_ground {
            data.highest_y = feet_y;
        }
        None
    }
}

/// 轴对齐包围盒（世界坐标；以脚部为基准，min_y == 脚部 y）。
///
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
    /// 由脚底中心 (x, y, z) 及给定半宽与高度构建。
    ///
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

    /// 平移到给定脚底坐标。
    ///
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

    /// 按 (dx, dy, dz) 平移。
    ///
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

    /// 是否与另一个 AABB 相交（边界含端点）。
    ///
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

/// 物理参数（由 sc_movement 系统消费）。
///
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
    /// 默认玩家物理体（0.6 宽 / 1.8 高）。
    ///
    /// Default player physics body (0.6 wide / 1.8 tall).
    pub fn player() -> Self {
        Self::default()
    }
}

/// 实体运动状态组件：统一携带位置/速度/朝向/着地。
/// ECS 组件访问不提供 `&mut`（`get_component -> Arc<C>`），可变状态使用组件内部锁；
/// 锁内保存最近一次广播快照。
///
/// Entity motion-state component: carries position/velocity/facing/grounded uniformly.
/// ECS has no `&mut` component access (`get_component -> Arc<C>`), so mutable state
/// uses an in-component lock; the lock holds the last-broadcast snapshot.
#[derive(Component, Clone, Debug)]
pub struct Transform {
    pub inner: Arc<RwLock<TransformData>>,
}

/// 运动状态数据（Copy 值组合，全部字段在同一把锁下读写）。
///
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
    fn player_physics_body_uses_default_speed_and_eye_height() {
        let body = PhysicsBody::player();
        assert_eq!(body.max_speed, 6.0);
        assert_eq!(body.base_offset, 1.62);
        assert_eq!(body.aabb.max_y, 1.8);
    }

    #[test]
    fn fall_state_settles_exactly_once_on_landing() {
        let fall = FallState::new(70.0, false);
        // Rising updates the peak without settling.
        assert!(fall.observe(74.0, false).is_none());
        assert!(fall.observe(72.0, false).is_none());
        // Air-to-ground transition settles highest - current exactly once.
        assert_eq!(fall.observe(64.0, true), Some(10.0));
        // Continuously grounded pins highest; no repeat settlement.
        assert!(fall.observe(64.0, true).is_none());
        assert!(fall.observe(64.0, true).is_none());
    }

    #[test]
    fn fall_state_reset_breaks_the_chain() {
        let fall = FallState::new(70.0, false);
        fall.reset(64.0, true);
        // Post-reset landing at the same height settles nothing.
        assert!(fall.observe(64.0, true).is_none());
    }

    #[test]
    fn fall_state_ignores_non_finite_peaks() {
        let fall = FallState::new(70.0, false);
        assert!(fall.observe(f32::NAN, false).is_none());
        assert_eq!(fall.observe(64.0, true), Some(6.0));
    }
}
