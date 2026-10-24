//! `MinecraftWorldId` 与 `MinecraftWorldManager`：世界注册表。
//!
//! 每个世界由 UUID 标识；支持按类型/名称查找、生成器接入与移除。
//! `MinecraftWorldId` 同时是玩家实体上的组件（标记其所在世界）。
//!
//! `MinecraftWorldId` and `MinecraftWorldManager`: the world registry.
//!
//! Each world is identified by UUID; supports lookup by type/name, generator
//! wiring, and removal. `MinecraftWorldId` is also a component on player
//! entities (marks which world they are in).

use crate::storage::WorldGenerator;
use crate::world::MinecraftWorld;
use sc_ecs::component::Component;
use sc_ecs::resource::Resource;
use sc_utils::world::r#type::WorldType;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// 世界 id：UUID 包装，同时用作玩家实体上的所在世界组件。
///
/// World id: a UUID wrapper, also used as the component marking which world
/// a player entity is in.
#[derive(Component, Ord, PartialOrd, Eq, PartialEq, Hash, Debug, Clone)]
pub struct MinecraftWorldId {
    /// UUID 本体。
    ///
    /// The underlying UUID.
    pub world_id: Uuid,
}

impl MinecraftWorldId {
    /// 生成随机世界 id。
    ///
    /// Generates a random world id.
    pub fn random() -> Self {
        Self {
            world_id: Uuid::new_v4(),
        }
    }
}

/// 世界管理器：世界 id → 世界/类型/名称的注册表（ECS 资源）。
///
/// World manager: the registry mapping world id to world/type/name
/// (an ECS resource).
#[derive(Resource, Clone)]
pub struct MinecraftWorldManager {
    /// 世界 id → 世界。
    ///
    /// World id to world.
    worlds: HashMap<MinecraftWorldId, MinecraftWorld>,
    /// 世界 id → 世界类型。
    ///
    /// World id to world type.
    worlds_type: HashMap<MinecraftWorldId, WorldType>,
    /// 世界 id → 世界名。
    ///
    /// World id to world name.
    worlds_name: HashMap<MinecraftWorldId, String>,
}

impl MinecraftWorldManager {
    /// 构造空管理器。
    ///
    /// Builds an empty manager.
    pub fn new() -> Self {
        Self {
            worlds: HashMap::new(),
            worlds_type: HashMap::new(),
            worlds_name: HashMap::new(),
        }
    }

    /// 注册一个世界并返回分配的世界 id（UUID 冲突时重试）。
    ///
    /// Registers a world and returns its assigned world id (retries on UUID
    /// collision).
    ///
    /// - `world_type`: 世界类型（主世界/下界/末地/自定义）
    /// - `world`: 待注册的世界（其 `world_id` 会被改写为分配值）
    pub fn push_world(
        &mut self,
        world_type: WorldType,
        mut world: MinecraftWorld,
    ) -> MinecraftWorldId {
        let mut world_id = MinecraftWorldId::random();
        while self.worlds.contains_key(&world_id) {
            // Retry on UUID collision.
            world_id = MinecraftWorldId::random();
        }
        world.world_id = world_id.clone();
        let world_name = world.world_name.clone();
        self.worlds.insert(world_id.clone(), world);
        self.worlds_type.insert(world_id.clone(), world_type);
        self.worlds_name.insert(world_id.clone(), world_name);
        world_id
    }

    /// 按 id 取世界；不存在返回 `None`。
    ///
    /// Looks up a world by id; returns `None` when absent.
    pub fn get_world(&self, world_id: &MinecraftWorldId) -> Option<&MinecraftWorld> {
        self.worlds.get(world_id)
    }

    /// 全部已加载世界的迭代器（关闭时需要逐个落盘脏区块）。
    ///
    /// All loaded worlds (used when shutdown needs to flush dirty chunks of every world).
    pub fn worlds(&self) -> impl Iterator<Item = &MinecraftWorld> {
        self.worlds.values()
    }

    /// 按类型取全部匹配世界。
    ///
    /// Returns all worlds matching the given type.
    ///
    /// - `world_type`: 目标世界类型
    pub fn get_worlds_by_type(&self, world_type: &WorldType) -> Vec<&MinecraftWorld> {
        let mut worlds = Vec::new();
        for (world_id, ty) in self.worlds_type.iter() {
            if ty == world_type {
                if let Some(world) = self.worlds.get(world_id) {
                    worlds.push(world);
                }
            }
        }
        worlds
    }

    /// 按名称取全部匹配世界。
    ///
    /// Returns all worlds matching the given name.
    ///
    /// - `name`: 世界名
    pub fn get_worlds_by_name(&self, name: &String) -> Vec<&MinecraftWorld> {
        let mut worlds = Vec::new();
        for (world_id, nm) in self.worlds_name.iter() {
            if nm == name {
                if let Some(world) = self.worlds.get(world_id) {
                    worlds.push(world);
                }
            }
        }
        worlds
    }

    /// 按 id 取世界类型；不存在返回 `None`。
    ///
    /// Looks up a world type by id; returns `None` when absent.
    pub fn get_world_type(&self, world_id: &MinecraftWorldId) -> Option<&WorldType> {
        self.worlds_type.get(world_id)
    }

    /// 按 id 取可变世界引用；不存在返回 `None`。
    ///
    /// Looks up a mutable world reference by id; returns `None` when absent.
    pub fn get_world_mut(&mut self, world_id: &MinecraftWorldId) -> Option<&mut MinecraftWorld> {
        self.worlds.get_mut(world_id)
    }

    /// 安装插件持有的生成器，不向网络 crate 暴露存储内部。
    ///
    /// Installs a plugin-owned generator without exposing storage internals to
    /// the network crate.
    ///
    /// 世界不存在时返回 `false`。
    ///
    /// Returns `false` when the world does not exist.
    pub fn set_world_generator(
        &mut self,
        world_id: &MinecraftWorldId,
        generator: Arc<dyn WorldGenerator>,
    ) -> bool {
        let Some(world) = self.worlds.get_mut(world_id) else {
            return false;
        };
        world.chunk_provider = world.chunk_provider.clone().with_generator(generator);
        true
    }

    /// 安装带描述符校验的世界生成器。
    ///
    /// Installs a world generator with descriptor verification.
    ///
    /// - `world_id`: 目标世界 id
    /// - `generator`: 生成器 trait 对象
    /// - `descriptor`: 生成器描述符（维度/高度等须匹配）
    /// - `dimension`: 目标维度 id
    ///
    /// 世界不存在返回 `Ok(false)`；描述符校验失败返回存储错误。
    ///
    /// Returns `Ok(false)` when the world does not exist; a storage error when
    /// descriptor verification fails.
    pub fn set_world_generator_with_descriptor(
        &mut self,
        world_id: &MinecraftWorldId,
        generator: Arc<dyn WorldGenerator>,
        descriptor: crate::generation::descriptor::GeneratorDescriptor,
        dimension: i32,
    ) -> Result<bool, crate::storage::WorldStorageError> {
        let Some(world) = self.worlds.get_mut(world_id) else {
            return Ok(false);
        };
        world.chunk_provider = world
            .chunk_provider
            .clone()
            .with_generator_and_descriptor(generator, descriptor, dimension)?;
        Ok(true)
    }

    /// 移除一个世界，返回被移除的世界；不存在返回 `None`。
    ///
    /// Removes a world and returns it; returns `None` when absent.
    pub fn remove_world(&mut self, world_id: &MinecraftWorldId) -> Option<MinecraftWorld> {
        self.worlds_type.remove(world_id)?;
        self.worlds_name.remove(world_id);
        self.worlds.remove(world_id)
    }
}
