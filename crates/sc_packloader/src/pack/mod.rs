//! 资源包数据模型：清单、子包与已解析内容。
//!
//! Resource pack data model: manifest, sub-packs, and parsed content.

use crate::entity::MinecraftEntitySpawner;
use crate::item::MinecraftItemSpawner;
use crate::pack::manifest::ResourcePackManifest;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub mod manifest;
pub mod pack_manager;
pub mod sub_pack;

/// 资源包类型：行为包或资源包。
///
/// Resource pack type: behavior pack or resource pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourcePackType {
    Behavior,
    Resource,
}

/// 包的解析来源：压缩来源保留共享字节（配方、客户端下载），目录来源直接从文件系统读取。
///
/// Where a pack is parsed from. Zip sources keep shared bytes (recipes,
/// client download); directory sources read natively from the filesystem.
#[derive(Clone, Debug)]
pub enum PackData {
    Zip(Arc<Vec<u8>>),
    Dir(PathBuf),
}

/// 资源包/行为包。
///
/// Resource/behavior pack.
///
/// `pack_data`（整包 zip 字节，可达数 MB）通过 [`Arc`] 共享：在
/// `get_behavior_packs()`/`get_resource_packs()` 的登录路径上克隆时，
/// 由深拷贝降为引用计数。`entities`/`items` 在推入
/// [`crate::version_control::runtime::MinecraftRuntimeManager`] 后可通过
/// [`ResourcePack::clear_parsed_content`] 清空（注册表持有唯一副本）。
///
/// Resource/behavior pack.
///
/// `pack_data` (whole-pack zip bytes, up to several MB) is shared via [`Arc`]: cloning on the login path
/// in `get_behavior_packs()`/`get_resource_packs()` drops from a deep copy to
/// a refcount. `entities`/`items` can be cleared via
/// [`ResourcePack::clear_parsed_content`] once pushed into
/// [`crate::version_control::runtime::MinecraftRuntimeManager`] (the registry holds the only copy).
#[derive(Clone, Debug)]
pub struct ResourcePack {
    /// 包清单。
    ///
    /// Pack manifest.
    pub manifest: ResourcePackManifest,
    /// 包类型（行为包/资源包）。
    ///
    /// Pack type (behavior/resource).
    pub pack_type: ResourcePackType,
    /// 客户端下载用字节（目录包为空：本地调试用，分发时打为 zip）。
    ///
    /// Client-download bytes (empty for directory packs: author locally,
    /// distribute as zip).
    pub pack_data: Arc<Vec<u8>>,
    /// 解析来源。
    ///
    /// Where this pack was parsed from.
    pub source: PackData,
    /// 包内实体定义表。
    ///
    /// Entity definitions inside the pack.
    pub entities: HashMap<String, MinecraftEntitySpawner>,
    /// 包内物品定义表。
    ///
    /// Item definitions inside the pack.
    pub items: HashMap<String, MinecraftItemSpawner>,
}

impl ResourcePack {
    /// 从拥有所有权的字节构造资源包（内部转为 [`Arc`] 共享）。
    ///
    /// Constructs a resource pack from owned bytes (shared internally via [`Arc`]).
    pub fn new(
        manifest: ResourcePackManifest,
        pack_type: ResourcePackType,
        pack_data: Vec<u8>,
        entities: HashMap<String, MinecraftEntitySpawner>,
        items: HashMap<String, MinecraftItemSpawner>,
    ) -> Self {
        Self::new_shared(manifest, pack_type, Arc::new(pack_data), entities, items)
    }

    /// 以共享字节构造资源包。
    ///
    /// Constructs a resource pack with shared bytes.
    pub fn new_shared(
        manifest: ResourcePackManifest,
        pack_type: ResourcePackType,
        pack_data: Arc<Vec<u8>>,
        entities: HashMap<String, MinecraftEntitySpawner>,
        items: HashMap<String, MinecraftItemSpawner>,
    ) -> Self {
        let source = PackData::Zip(Arc::clone(&pack_data));
        Self {
            manifest,
            pack_type,
            pack_data,
            source,
            entities,
            items,
        }
    }

    /// 读取包数据的一个分片（客户端分块下载用）；越界时返回空。
    ///
    /// Reads a chunk of the pack bytes (for chunked client download);
    /// returns empty when out of bounds.
    ///
    /// - `offset`: 起始偏移
    /// - `length`: 分片长度
    pub fn get_pack_chunk(&self, offset: usize, length: usize) -> Vec<u8> {
        let end = (offset + length).min(self.pack_data.len());
        if offset < self.pack_data.len() {
            self.pack_data[offset..end].to_vec()
        } else {
            vec![]
        }
    }

    /// 清空已解析的实体/物品表（内容已合并进运行时注册表；
    /// 管理器仅保留清单 + zip 字节用于客户端下载）。
    ///
    /// Clear the parsed entities/items tables (contents already merged into the runtime registry;
    /// the manager keeps only the manifest + zip bytes for client download).
    pub fn clear_parsed_content(&mut self) {
        self.entities.clear();
        self.items.clear();
    }
}
