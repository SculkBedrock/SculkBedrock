//! 把 `(world, pos)` 映射到方块实体 `EntityId` 的空间索引。
//!
//! Spatial index mapping `(world, pos)` to block-entity `EntityId`.

use std::collections::HashMap;

use sc_ecs::entity::EntityId;
use sc_ecs::resource::Resource;
use sc_world::manager::MinecraftWorldId;

/// `(world, pos)` → 加工实体索引。
///
/// `(world, pos)` → process entity index.
///
/// 每个世界上界（默认 MAX_PER_WORLD = 1024）。索引满时拒绝新实体
/// （失败关闭），已有实体不受影响。
/// Bounded per world (default MAX_PER_WORLD = 1024). A full index refuses
/// new entities (fail-closed), leaving existing entities unaffected.
#[derive(Resource, Default)]
pub struct BlockEntityIndex {
    map: HashMap<(MinecraftWorldId, (i32, i32, i32)), EntityId>,
}

impl BlockEntityIndex {
    /// 每个世界允许跟踪的最大方块实体数。
    ///
    /// Maximum tracked block entities allowed per world.
    pub const MAX_PER_WORLD: usize = 1024;

    /// 查找 `(world, pos)` 处的实体。
    ///
    /// Look up the entity at `(world, pos)`.
    ///
    /// - `world_id`: 存档世界 id
    /// - `pos`: 方块坐标
    pub fn get(&self, world_id: &MinecraftWorldId, pos: (i32, i32, i32)) -> Option<EntityId> {
        self.map.get(&(world_id.clone(), pos)).copied()
    }

    /// 显式跟踪一个实体（crate 内部使用：`ensure` 是正常路径；
    /// 手工构造实体的测试用本方法注册）。
    ///
    /// Track an entity explicitly (crate-internal: `ensure` is the normal
    /// path; tests that build entities by hand use this to register them).
    ///
    /// - `world_id`: 存档世界 id
    /// - `pos`: 方块坐标
    /// - `entity`: 实体 id
    pub fn insert(
        &mut self,
        world_id: MinecraftWorldId,
        pos: (i32, i32, i32),
        entity: EntityId,
    ) {
        self.map.insert((world_id, pos), entity);
    }

    /// 从索引中移除一个实体。
    ///
    /// Remove an entity from the index.
    ///
    /// - `world_id`: 存档世界 id
    /// - `pos`: 方块坐标
    pub fn remove(&mut self, world_id: &MinecraftWorldId, pos: (i32, i32, i32)) -> Option<EntityId> {
        self.map.remove(&(world_id.clone(), pos))
    }

    /// 统计一个世界分区中被跟踪的方块实体数。
    ///
    /// Count tracked block entities for a world partition.
    ///
    /// - `world_id`: 存档世界 id
    pub fn count_for(&self, world_id: &MinecraftWorldId) -> usize {
        self.map
            .keys()
            .filter(|(world, _)| world == world_id)
            .count()
    }

    /// 遍历属于指定世界的全部实体。
    ///
    /// Iterate over all entities belonging to a specific world.
    ///
    /// - `world_id`: 存档世界 id
    pub fn iter_for<'a>(
        &'a self,
        world_id: &'a MinecraftWorldId,
    ) -> impl Iterator<Item = ((i32, i32, i32), EntityId)> + 'a {
        self.map.iter().filter_map(move |((w, pos), &entity)| {
            if w == world_id {
                Some((*pos, entity))
            } else {
                None
            }
        })
    }
}
