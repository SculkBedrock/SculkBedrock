//! `World` 的人机工程扩展 trait，提供方块实体语法糖。
//!
//! Ergonomic extension trait for `World` providing block entity syntactic
//! sugar.

use super::dirty::BlockEntityDirty;
use super::identity::BlockEntityRef;
use super::index::BlockEntityIndex;
use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;
use sc_ecs::world::World;
use sc_world::manager::MinecraftWorldId;
use std::sync::Arc;

/// 在 `World` 上直接查询与修改方块实体的流式扩展方法。
///
/// Fluent extension methods for querying and mutating block entities
/// directly on `World`.
pub trait BlockEntityWorldExt {
    /// 获取给定世界坐标处方块实体的 `EntityId`。
    ///
    /// Retrieve the `EntityId` of a block entity at the given world
    /// coordinates.
    ///
    /// - `world_id`: 存档世界 id
    /// - `pos`: 方块坐标
    fn block_entity_at(&self, world_id: &MinecraftWorldId, pos: (i32, i32, i32)) -> Option<EntityId>;

    /// 获取方块实体的标识组件。
    ///
    /// Retrieve the identity component of a block entity.
    ///
    /// - `entity`: 实体 id
    fn block_entity_ref(&self, entity: &EntityId) -> Option<Arc<BlockEntityRef>>;

    /// 修改方块实体上的组件并自动更新脏标记。
    ///
    /// Mutate a component on a block entity and automatically update dirty
    /// change flags.
    ///
    /// 读取现有组件、克隆、应用闭包 `f`、替换世界中的组件，并挂上
    /// `BlockEntityDirty::ALL`。
    /// Reads the existing component, clones it, applies closure `f`, replaces
    /// the component in the world, and attaches `BlockEntityDirty::ALL`.
    ///
    /// - `world_id`: 存档世界 id
    /// - `pos`: 方块坐标
    /// - `f`: 修改闭包
    fn mutate_block_entity<C: Component + Clone, R>(
        &self,
        world_id: &MinecraftWorldId,
        pos: (i32, i32, i32),
        f: impl FnOnce(&mut C) -> R,
    ) -> Option<R>;

    /// 把方块实体标记为需要持久化和/或网络同步。
    ///
    /// Mark a block entity as dirty for persistence and/or network sync.
    ///
    /// - `entity`: 实体 id
    /// - `save`: 是否需要写存
    /// - `network`: 是否需要同步
    fn mark_block_entity_dirty(&self, entity: &EntityId, save: bool, network: bool);
}

impl BlockEntityWorldExt for World {
    fn block_entity_at(&self, world_id: &MinecraftWorldId, pos: (i32, i32, i32)) -> Option<EntityId> {
        let index = self.get_resource::<BlockEntityIndex>()?;
        index.get(world_id, pos)
    }

    fn block_entity_ref(&self, entity: &EntityId) -> Option<Arc<BlockEntityRef>> {
        self.get_component::<BlockEntityRef>(entity)
    }

    fn mutate_block_entity<C: Component + Clone, R>(
        &self,
        world_id: &MinecraftWorldId,
        pos: (i32, i32, i32),
        f: impl FnOnce(&mut C) -> R,
    ) -> Option<R> {
        let entity = self.block_entity_at(world_id, pos)?;
        let component = self.get_component::<C>(&entity)?;
        let mut cloned = (*component).clone();
        let ret = f(&mut cloned);

        self.add_component(&entity, (cloned, BlockEntityDirty::ALL));
        Some(ret)
    }

    fn mark_block_entity_dirty(&self, entity: &EntityId, save: bool, network: bool) {
        if let Some(existing) = self.get_component::<BlockEntityDirty>(entity) {
            let mut updated = *existing;
            if save {
                updated.save_dirty = true;
            }
            if network {
                updated.network_dirty = true;
            }
            self.add_component(entity, updated);
        } else {
            self.add_component(
                entity,
                BlockEntityDirty {
                    save_dirty: save,
                    network_dirty: network,
                },
            );
        }
    }
}
