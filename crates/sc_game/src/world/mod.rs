//! 多世界模型：`GameWorldId` / `GameWorld` / `WorldHub`。
//!
//! Multi-world model: `GameWorldId` / `GameWorld` / `WorldHub`.
//!
//! 每个游戏世界是共享同一套游戏命令代码的独立 ECS `World` 实例。
//! `WorldHub` 位于根世界上；`hub_tick` 每 tick 对各 Running 实例执行
//! 世界内命令。
//! Each game world is an independent ECS `World` instance sharing the same
//! game command code. `WorldHub` lives on the root world; `hub_tick` runs
//! in-world commands over Running instances each tick.

pub mod bus;
pub mod ids;

pub use bus::{
    AnnouncementBus, CrossWorldBus, CrossWorldEvent, CrossWorldPing, PingBus, PingMailbox,
    RegionPing, RegionPingBus, TeleportBus, TeleportRequest, WorldAnnouncement, WorldMailbox,
};
pub use ids::{RegionEpoch, RegionShardId, RegionTick, WorldInstanceId};

use sc_ecs::component::Component;
use sc_ecs::resource::Resource;
use sc_ecs::world::World;
use sc_world::manager::MinecraftWorldId;
use std::collections::HashMap;

/// 世界实例 id（0=主世界，1=下界，2=末地，更大值为自定义）。
///
/// World instance id (0=overworld, 1=nether, 2=end, higher=custom).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct GameWorldId(pub u32);

/// 玩家所属的 GameWorld（显式的游戏域所有权，区别于 `MinecraftWorldId`
/// 存档 id）。由位置维护，跨世界传送后更新；游戏系统按此组件路由。
///
/// Owning GameWorld of a player (explicit game-domain ownership, distinct
/// from the `MinecraftWorldId` save id). Maintained by position and updated
/// after cross-world teleport; game systems route by this component.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerGameWorld(pub GameWorldId);

/// 世界实例运行状态。
///
/// World instance run state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldStatus {
    /// 每 tick 执行游戏命令。
    ///
    /// Runs game commands every tick.
    Running,
    /// 暂停（传送/保存中），跳过 tick。
    ///
    /// Paused (teleporting/saving), skips ticks.
    Paused,
    /// 卸载中（迁移/刷盘后从枢纽移除）。
    ///
    /// Unloading (removed from the hub after migration/flush).
    Unloading,
}

/// 一个运行中的游戏世界实例。
///
/// One running game world instance.
#[derive(Clone)]
pub struct GameWorld {
    /// 世界实例 id。
    ///
    /// World instance id.
    pub id: GameWorldId,
    /// 独立的 ECS World 实例。
    ///
    /// Independent ECS World instance.
    pub world: World,
    /// 运行状态。
    ///
    /// Run state.
    pub status: WorldStatus,
}

/// 多世界枢纽（根世界上的资源）。
///
/// Multi-world hub (a root-world resource).
///
/// `spawn_world` 为新实例挂上默认资源；游戏系统通过 `running()` /
/// `running_mut()` 遍历实例。
/// `spawn_world` attaches default resources to new instances; game systems
/// iterate instances via `running()`/`running_mut()`.
#[derive(Resource, Clone, Default)]
pub struct WorldHub {
    worlds: HashMap<GameWorldId, GameWorld>,
}

impl WorldHub {
    /// 生成一个 Running 状态的世界实例并挂上默认资源。
    ///
    /// Spawns a Running world instance with default resources.
    ///
    /// - `id`: 世界实例 id
    pub fn spawn_world(&mut self, id: GameWorldId) -> GameWorldId {
        let world = World::new();
        world.insert_resource(TickCounter::default());
        world.insert_resource(bus::PingMailbox::default());
        self.worlds.insert(
            id,
            GameWorld {
                id,
                world,
                status: WorldStatus::Running,
            },
        );
        id
    }

    /// 按 id 只读获取实例。
    ///
    /// Gets an instance by id (read-only).
    ///
    /// - `id`: 世界实例 id
    pub fn get(&self, id: &GameWorldId) -> Option<&GameWorld> {
        self.worlds.get(id)
    }

    /// 按 id 可变获取实例。
    ///
    /// Gets an instance by id (mutable).
    ///
    /// - `id`: 世界实例 id
    pub fn get_mut(&mut self, id: &GameWorldId) -> Option<&mut GameWorld> {
        self.worlds.get_mut(id)
    }

    /// 移除实例（先暂停、刷盘，再移除）。
    ///
    /// Removes an instance (pause, flush, then remove).
    ///
    /// - `id`: 世界实例 id
    pub fn remove(&mut self, id: &GameWorldId) -> Option<GameWorld> {
        self.worlds.remove(id)
    }

    /// 实例数量。
    ///
    /// Number of instances.
    pub fn count(&self) -> usize {
        self.worlds.len()
    }

    /// 是否包含指定 id 的实例。
    ///
    /// Whether an instance with the given id exists.
    ///
    /// - `id`: 世界实例 id
    pub fn contains(&self, id: &GameWorldId) -> bool {
        self.worlds.contains_key(id)
    }

    /// 遍历 Running 实例（只读）。
    ///
    /// Iterate Running instances (read-only).
    pub fn running(&self) -> impl Iterator<Item = &GameWorld> + '_ {
        self.worlds
            .values()
            .filter(|w| w.status == WorldStatus::Running)
    }

    /// 遍历 Running 实例（可变）。
    ///
    /// Iterate Running instances (mutable).
    pub fn running_mut(&mut self) -> impl Iterator<Item = &mut GameWorld> + '_ {
        self.worlds
            .values_mut()
            .filter(|w| w.status == WorldStatus::Running)
    }
}

/// 每实例 tick 计数器（由 `spawn_world` 挂载）。
///
/// Per-instance tick counter (attached by `spawn_world`).
#[derive(Resource, Clone, Debug, Default)]
pub struct TickCounter {
    /// 已累计的 tick 数。
    ///
    /// Accumulated tick count.
    pub ticks: u64,
}

/// GameWorldId 到 Minecraft 存档世界的映射。
///
/// GameWorldId to Minecraft save-world map.
///
/// 每个维度（主世界/下界/末地）对应一个 `GameWorld` 实例，在此链接到
/// `MinecraftWorldManager` 中的存档数据，以及用于构建 ChunkKey 的维度 id。
/// Each dimension (Overworld/Nether/End) gets one `GameWorld` instance,
/// linked here to save data in `MinecraftWorldManager` plus the dimension
/// id used to build ChunkKeys.
#[derive(Resource, Clone, Debug, Default)]
pub struct GameWorldMap {
    entries: HashMap<GameWorldId, GameWorldEntry>,
}

/// 一个 GameWorld 的存档信息。
///
/// Save info for one GameWorld.
#[derive(Clone, Debug)]
pub struct GameWorldEntry {
    /// 维度 id。
    ///
    /// Dimension id.
    pub dimension: i32,
    /// 关联的 Minecraft 存档世界 id。
    ///
    /// Linked Minecraft save-world id.
    pub minecraft_world_id: MinecraftWorldId,
}

impl GameWorldMap {
    /// 插入一条映射。
    ///
    /// Inserts a mapping.
    ///
    /// - `id`: GameWorldId
    /// - `entry`: 存档信息
    pub fn insert(&mut self, id: GameWorldId, entry: GameWorldEntry) {
        self.entries.insert(id, entry);
    }

    /// 按 id 获取存档信息。
    ///
    /// Gets save info by id.
    ///
    /// - `id`: GameWorldId
    pub fn get(&self, id: &GameWorldId) -> Option<&GameWorldEntry> {
        self.entries.get(id)
    }

    /// 按 Minecraft 存档世界反查 GameWorldId。
    ///
    /// Finds a GameWorldId by Minecraft save world.
    ///
    /// - `minecraft_world_id`: 存档世界 id
    pub fn find_by_world(&self, minecraft_world_id: &MinecraftWorldId) -> Option<GameWorldId> {
        self.entries
            .iter()
            .find(|(_, entry)| entry.minecraft_world_id == *minecraft_world_id)
            .map(|(id, _)| *id)
    }

    /// 移除一条映射。
    ///
    /// Removes a mapping.
    ///
    /// - `id`: GameWorldId
    pub fn remove(&mut self, id: &GameWorldId) -> Option<GameWorldEntry> {
        self.entries.remove(id)
    }

    /// 映射条目数量。
    ///
    /// Number of mapping entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否没有映射条目。
    ///
    /// Whether the map has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
