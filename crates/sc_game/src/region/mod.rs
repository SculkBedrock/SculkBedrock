//! 游戏域的 region 扩展（框架 region 类型位于 `sc_ecs::region`）。
//!
//! Game-domain region extension (framework region types live in
//! sc_ecs::region).
//!
//! 本模块只保留 Minecraft 专属部分：
//! This module keeps only the Minecraft-specific parts:
//! - [`RegionId`]：世界加网格坐标的 region 标识；
//!   [`RegionId`]: world plus grid-coordinate region identity;
//! - [`RegionGrid`]：区块到 region 网格的数学换算；
//!   [`RegionGrid`]: chunk to region grid math;
//! - [`region_tick`]：注入 `RegionDriver` 的示例 region tick。
//!   [`region_tick`]: sample region tick injected into `RegionDriver`.
//!
//! 共享的框架类型在此重新导出，供游戏域使用。
//! Shared framework types re-export here for game-domain use.

pub mod transaction;

pub use sc_ecs::region::{
    BoundedRegionScheduler, EntityRegion, Region, RegionBus, RegionDriver, RegionEvent, RegionHub,
    RegionStatus,
};
pub use transaction::{OperationId, ParticipantVote, TransactionCoordinator, TransactionState};

use parking_lot::RwLock;
use sc_ecs::resource::Resource;
use sc_ecs::world::World;
use sc_entity::motion::{PhysicsBody, Transform};
use sc_utils::world::r#type::WorldType;
use sc_world::manager::MinecraftWorldManager;
use std::sync::Arc;
use std::time::Duration;

use crate::world::bus::{RegionPing, RegionPingBus};
use crate::world::GameWorldId;

/// Region 标识（世界内网格坐标）。
///
/// Region identity (in-world grid coordinates).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RegionId {
    /// 所属游戏世界 id。
    ///
    /// Owning game-world id.
    pub world: GameWorldId,
    /// region 网格 X 坐标。
    ///
    /// Region grid X coordinate.
    pub x: i32,
    /// region 网格 Z 坐标。
    ///
    /// Region grid Z coordinate.
    pub z: i32,
}

impl RegionId {
    /// 构造 region 标识。
    ///
    /// Constructs a region identity.
    ///
    /// - `world`: 游戏世界 id
    /// - `x`: region 网格 X 坐标
    /// - `z`: region 网格 Z 坐标
    pub const fn new(world: GameWorldId, x: i32, z: i32) -> Self {
        Self { world, x, z }
    }
}

/// Region 网格：`region_size_chunks`（每 region NxN 个区块）决定粒度。
///
/// Region grid: `region_size_chunks` (NxN chunks per region) sets granularity.
///
/// region 越小并行越好，但跨 region 流量更多。
/// Smaller regions parallelize better with more cross-region traffic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionGrid {
    /// 所属游戏世界 id。
    ///
    /// Owning game-world id.
    pub world: GameWorldId,
    /// 每 region 边长的区块数（至少 1）。
    ///
    /// Edge length per region in chunks (at least 1).
    pub region_size_chunks: u32,
}

impl RegionGrid {
    /// 构造 region 网格。
    ///
    /// Constructs a region grid.
    ///
    /// - `world`: 游戏世界 id
    /// - `region_size_chunks`: 每 region 边长的区块数（小于 1 按 1 处理）
    pub fn new(world: GameWorldId, region_size_chunks: u32) -> Self {
        Self {
            world,
            region_size_chunks: region_size_chunks.max(1),
        }
    }

    /// 区块坐标所属的 region。
    ///
    /// Owning region of chunk coordinates.
    ///
    /// - `chunk_x`: 区块 X 坐标
    /// - `chunk_z`: 区块 Z 坐标
    #[inline]
    pub fn region_of(&self, chunk_x: i32, chunk_z: i32) -> RegionId {
        let n = self.region_size_chunks as i32;
        RegionId::new(self.world, chunk_x.div_euclid(n), chunk_z.div_euclid(n))
    }

    /// region 的原点区块坐标。
    ///
    /// Origin chunk of a region.
    ///
    /// - `region`: region 标识
    #[inline]
    pub fn region_origin(&self, region: RegionId) -> (i32, i32) {
        let n = self.region_size_chunks as i32;
        (region.x * n, region.z * n)
    }

    /// 中心 region 指定半径内的全部 region（含中心）。
    ///
    /// Regions within radius of a center region, center included.
    ///
    /// - `center`: 中心 region
    /// - `radius`: 半径（region 数）
    pub fn regions_in_radius(&self, center: RegionId, radius: u32) -> Vec<RegionId> {
        let radius = radius as i32;
        let mut out = Vec::with_capacity(((radius * 2 + 1) as usize).pow(2));
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                out.push(RegionId::new(self.world, center.x + dx, center.z + dz));
            }
        }
        out
    }
}

/// Region 命令：注册进 [`RegionCommands`]，在 region tick 期间执行。
///
/// Region command: registered into [`RegionCommands`], run during the region
/// tick.
pub trait RegionCommand: Send + Sync {
    /// 执行命令。
    ///
    /// Runs the command.
    ///
    /// - `world`: ECS 世界
    /// - `region`: 目标 region
    fn run(&self, world: &World, region: RegionId);
}

/// Region 命令集（分区世界上的资源）。
///
/// Region command set (resource on the partitioned world).
#[derive(Resource, Default)]
pub struct RegionCommands {
    commands: RwLock<Vec<Arc<dyn RegionCommand>>>,
}

impl RegionCommands {
    /// 注册一个命令。
    ///
    /// Registers a command.
    ///
    /// - `command`: 要注册的命令
    pub fn register(&self, command: Arc<dyn RegionCommand>) {
        self.commands.write().push(command);
    }

    /// 清空全部命令。
    ///
    /// Clears all commands.
    pub fn clear(&self) {
        self.commands.write().clear();
    }

    /// 基于快照执行命令，执行期间不持锁。
    ///
    /// Run snapshotted commands without holding locks across calls.
    ///
    /// - `world`: ECS 世界
    /// - `region`: 目标 region
    pub fn run_all(&self, world: &World, region: RegionId) {
        let commands = self.commands.read().clone();
        for command in commands {
            command.run(world, region);
        }
    }
}

/// 包装 [`region_physics`] 的 region 物理命令。
///
/// Region physics command wrapping [`region_physics`].
pub struct RegionPhysicsCommand;

impl RegionCommand for RegionPhysicsCommand {
    fn run(&self, world: &World, region: RegionId) {
        region_physics(world, region);
    }
}

/// 单次 region tick（游戏侧示例实现）。
///
/// Single region tick (sample game-side implementation).
///
/// 统计 tick、处理入站迁移、交换邻居 ping，并执行已注册的 region 命令
/// （如物理）。
/// Counts ticks, processes inbound migration, exchanges neighbor pings,
/// and runs registered region commands such as physics.
///
/// - `world`: ECS 世界
/// - `region`: 目标 region
pub fn region_tick(world: &World, region: RegionId) {
    // Demo switches (production mode disables pings and migration demos).
    let demo = world
        .get_resource::<crate::plugin::DemoSystems>()
        .map(|d| d.0)
        .unwrap_or(false);

    // Process entities migrated into this region.
    if let Some(mut hub) = world.get_resource_mut::<RegionHub<RegionId>>() {
        hub.recover_expired_migrations(world, Duration::from_secs(5));
        hub.process_inbound(world, &region);
    }

    // Receive this region's events.
    if let Some(mut bus) = world.get_resource_mut::<RegionPingBus>() {
        let events = bus.take_for(&region);
        if !events.is_empty() {
            log::debug!(
                "region ({},{}) received {} region events",
                region.x,
                region.z,
                events.len()
            );
        }
    }

    // Advance this region's tick.
    let (tick, should_migrate, should_ping) = {
        let Some(mut hub) = world.get_resource_mut::<RegionHub<RegionId>>() else {
            return;
        };
        let Some(region_state) = hub.get_mut(&region) else {
            return;
        };
        region_state.tick += 1;
        (
            region_state.tick,
            demo && region_state.tick % 200 == 0,
            demo && region_state.tick % 100 == 0,
        )
    };

    // Demo: ping neighbor regions every 100 ticks.
    if should_ping {
        if let Some(mut bus) = world.get_resource_mut::<RegionPingBus>() {
            let neighbor = RegionId::new(region.world, (region.x + 1) % 2, region.z);
            bus.send(region, neighbor, RegionPing { from_tick: tick });
        }
    }

    // Demo: migrate this region's first entity to the right neighbor.
    if should_migrate {
        let entity = world
            .entities_with_component::<EntityRegion<RegionId>>()
            .into_iter()
            .find(|entity| {
                world
                    .get_component::<EntityRegion<RegionId>>(entity)
                    .map(|er| er.0 == region)
                    .unwrap_or(false)
            });
        if let Some(entity) = entity {
            let neighbor = RegionId::new(region.world, (region.x + 1) % 2, region.z);
            let Some(mut hub) = world.get_resource_mut::<RegionHub<RegionId>>() else {
                return;
            };
            if !hub.begin_migration(world, entity, region, neighbor) {
                log::debug!("migration failed: entity missing or without EntityRegion");
            }
        }
    }

    // Region command set when registered, direct physics otherwise.
    if let Some(commands) = world.get_resource::<RegionCommands>() {
        commands.run_all(world, region);
    } else {
        region_physics(world, region);
    }

    // Periodic marker proving independent region progress.
    if tick % 100 == 0 {
        if let Some(hub) = world.get_resource::<RegionHub<RegionId>>() {
            let Some(region_state) = hub.get(&region) else {
                return;
            };
            let entity_count = world
                .entities_with_component::<EntityRegion<RegionId>>()
                .into_iter()
                .filter(|entity| {
                    world
                        .get_component::<EntityRegion<RegionId>>(entity)
                        .map(|er| er.0 == region)
                        .unwrap_or(false)
                })
                .count();
            log::debug!(
                "region ({},{}) tick={} entity count={}",
                region.x,
                region.z,
                region_state.tick,
                entity_count
            );
        }
    }
}

/// region 内的游戏命令（物理）：本 region 实体的重力与 Y 碰撞，
/// 只读共享世界数据，不做 IO。
///
/// In-region game commands (physics): gravity and Y collision for this
/// region's entities, reading shared world data without IO.
///
/// - `world`: ECS 世界
/// - `region`: 目标 region
fn region_physics(world: &World, region: RegionId) {
    let Some(manager) = world.get_resource::<MinecraftWorldManager>() else {
        return;
    };
    let Some(world_id) = manager
        .get_worlds_by_type(&WorldType::Overworld)
        .first()
        .map(|w| w.world_id.clone())
    else {
        return;
    };
    for entity in world.entities_with_component::<EntityRegion<RegionId>>() {
        let Some(er) = world.get_component::<EntityRegion<RegionId>>(&entity) else {
            continue;
        };
        if er.0 != region {
            continue;
        }
        let Some(transform) = world.get_component::<Transform>(&entity) else {
            continue;
        };
        let Some(physics) = world.get_component::<PhysicsBody>(&entity) else {
            continue;
        };
        let mut tf = transform.inner.write();
        if !physics.no_clip && !physics.flying {
            tf.velocity.y -= physics.gravity;
            if tf.velocity.y < -physics.terminal_velocity {
                tf.velocity.y = -physics.terminal_velocity;
            }
        }
        let target = physics
            .aabb
            .at(tf.position.x, tf.position.y + tf.velocity.y, tf.position.z);
        if !crate::movement::world_collides(&manager, &world_id, &target) {
            tf.position.y += tf.velocity.y;
            tf.on_ground = false;
        } else {
            if tf.velocity.y < 0.0 {
                tf.on_ground = true;
            }
            tf.velocity.y = 0.0;
        }
        let feet_y = tf.position.y;
        let on_ground = tf.on_ground;
        drop(tf);
        // Same tracking-only contract as `movement_physics`: settle exactly
        // once per landing, pin highest; damage is handled by the damage
        // module, not applied here.
        use sc_entity::motion::FallState;
        if world.get_component::<FallState>(&entity).is_none() {
            world.add_component(&entity, FallState::new(feet_y, on_ground));
        } else if let Some(fall) = world.get_component::<FallState>(&entity) {
            if fall.observe(feet_y, on_ground).is_some() {
                fall.reset(feet_y, on_ground);
            }
        }
    }
}
