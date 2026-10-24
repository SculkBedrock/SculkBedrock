//! 资源包管理器：聚合多个资源包并提供按类型查询。
//!
//! Resource pack manager: aggregates multiple packs and offers typed queries.

use crate::entity::MinecraftEntitySpawner;
use crate::item::MinecraftItemSpawner;
use crate::pack::ResourcePack;
use crate::pack::ResourcePackType;
use sc_ecs::resource::Resource;
use std::collections::HashMap;

/// 资源包管理器（ECS 资源）：持有全部已加载包。
///
/// Resource pack manager (ECS resource): holds all loaded packs.
#[derive(Resource)]
#[repr(C)]
pub struct ResourcePackManager {
    /// 已加载包列表。
    ///
    /// List of loaded packs.
    pub packs: Vec<ResourcePack>,
}

impl ResourcePackManager {
    /// 构造空管理器。
    ///
    /// Creates an empty manager.
    pub fn new() -> Self {
        Self { packs: vec![] }
    }

    /// 从包列表构造管理器。
    ///
    /// Creates a manager from a list of packs.
    pub fn from_vec(packs: Vec<ResourcePack>) -> Self {
        Self { packs }
    }

    /// 追加一个包。
    ///
    /// Appends a single pack.
    pub fn push(&mut self, pack: ResourcePack) {
        self.packs.push(pack);
    }

    /// 批量追加包。
    ///
    /// Appends multiple packs.
    pub fn extends(&mut self, packs: Vec<ResourcePack>) {
        self.packs.extend(packs);
    }

    /// 返回全部行为包（克隆）。
    ///
    /// Returns all behavior packs (cloned).
    pub fn get_behavior_packs(&self) -> Vec<ResourcePack> {
        self.packs
            .iter()
            .filter(|p| p.pack_type == ResourcePackType::Behavior)
            .cloned()
            .collect()
    }

    /// 返回全部资源包（克隆）。
    ///
    /// Returns all resource packs (cloned).
    pub fn get_resource_packs(&self) -> Vec<ResourcePack> {
        self.packs
            .iter()
            .filter(|p| p.pack_type == ResourcePackType::Resource)
            .cloned()
            .collect()
    }

    /// 合并所有包的实体定义表（后加载的包覆盖先加载的）。
    ///
    /// Merges entity definitions from all packs (later packs override earlier ones).
    pub fn get_entities(&self) -> HashMap<String, MinecraftEntitySpawner> {
        let mut map = HashMap::new();
        for pack in &self.packs {
            map.extend(pack.entities.clone());
        }
        map
    }

    /// 合并所有包的物品定义表（后加载的包覆盖先加载的）。
    ///
    /// Merges item definitions from all packs (later packs override earlier ones).
    pub fn get_items(&self) -> HashMap<String, MinecraftItemSpawner> {
        let mut map = HashMap::new();
        for pack in &self.packs {
            map.extend(pack.items.clone());
        }
        map
    }

    /// 清空所有包已解析的实体/物品表。
    ///
    /// Clear the parsed entities/items tables of all packs.
    ///
    /// 在 `get_entities`/`get_items` 合并进 `MinecraftRuntimeManager` 之后调用。
    /// 运行时管理器只需要清单（登录协商）与 pack_data zip 字节（客户端下载）；
    /// 解析出的副本只会重复占用内存，故释放之。
    ///
    /// Call after `get_entities`/`get_items` have been merged into
    /// `MinecraftRuntimeManager`. The runtime manager only needs the manifest
    /// (login negotiation) and pack_data zip bytes (client download); the parsed copies
    /// would duplicate memory, so release them.
    pub fn clear_parsed_content(&mut self) {
        for pack in &mut self.packs {
            pack.clear_parsed_content();
        }
    }
}
