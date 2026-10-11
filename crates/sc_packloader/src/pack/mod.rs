use crate::entity::MinecraftEntitySpawner;
use crate::item::MinecraftItemSpawner;
use crate::pack::manifest::ResourcePackManifest;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub mod manifest;
pub mod pack_manager;
pub mod sub_pack;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourcePackType {
    Behavior,
    Resource,
}

/// Resource/behavior pack.
///
/// `pack_data` (whole-pack zip bytes, up to several MB) is shared via [`Arc`]: cloning on the login path
/// in `get_behavior_packs()`/`get_resource_packs()` drops from a deep copy to
/// a refcount. `entities`/`items` can be cleared via
/// [`ResourcePack::clear_parsed_content`] once pushed into
/// [`crate::version_control::runtime::MinecraftRuntimeManager`] (the registry holds the only copy).
/// Where a pack is parsed from. Zip sources keep shared bytes (recipes,
/// client download); directory sources read natively from the filesystem.
#[derive(Clone, Debug)]
pub enum PackData {
    Zip(Arc<Vec<u8>>),
    Dir(PathBuf),
}

#[derive(Clone, Debug)]
pub struct ResourcePack {
    pub manifest: ResourcePackManifest,
    pub pack_type: ResourcePackType,
    /// Client-download bytes (empty for directory packs: author locally,
    /// distribute as zip).
    pub pack_data: Arc<Vec<u8>>,
    pub source: PackData,
    pub entities: HashMap<String, MinecraftEntitySpawner>,
    pub items: HashMap<String, MinecraftItemSpawner>,
}

impl ResourcePack {
    pub fn new(
        manifest: ResourcePackManifest,
        pack_type: ResourcePackType,
        pack_data: Vec<u8>,
        entities: HashMap<String, MinecraftEntitySpawner>,
        items: HashMap<String, MinecraftItemSpawner>,
    ) -> Self {
        Self::new_shared(manifest, pack_type, Arc::new(pack_data), entities, items)
    }

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

    pub fn get_pack_chunk(&self, offset: usize, length: usize) -> Vec<u8> {
        let end = (offset + length).min(self.pack_data.len());
        if offset < self.pack_data.len() {
            self.pack_data[offset..end].to_vec()
        } else {
            vec![]
        }
    }

    /// Clear the parsed entities/items tables (contents already merged into the runtime registry;
    /// the manager keeps only the manifest + zip bytes for client download).
    pub fn clear_parsed_content(&mut self) {
        self.entities.clear();
        self.items.clear();
    }
}
