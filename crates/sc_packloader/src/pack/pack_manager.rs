use crate::entity::MinecraftEntitySpawner;
use crate::item::MinecraftItemSpawner;
use crate::pack::ResourcePack;
use crate::pack::ResourcePackType;
use sc_ecs::resource::Resource;
use std::collections::HashMap;

#[derive(Resource)]
#[repr(C)]
pub struct ResourcePackManager {
    pub packs: Vec<ResourcePack>,
}

impl ResourcePackManager {
    pub fn new() -> Self {
        Self { packs: vec![] }
    }

    pub fn from_vec(packs: Vec<ResourcePack>) -> Self {
        Self { packs }
    }

    pub fn push(&mut self, pack: ResourcePack) {
        self.packs.push(pack);
    }

    pub fn extends(&mut self, packs: Vec<ResourcePack>) {
        self.packs.extend(packs);
    }

    pub fn get_behavior_packs(&self) -> Vec<ResourcePack> {
        self.packs
            .iter()
            .filter(|p| p.pack_type == ResourcePackType::Behavior)
            .cloned()
            .collect()
    }

    pub fn get_resource_packs(&self) -> Vec<ResourcePack> {
        self.packs
            .iter()
            .filter(|p| p.pack_type == ResourcePackType::Resource)
            .cloned()
            .collect()
    }

    pub fn get_entities(&self) -> HashMap<String, MinecraftEntitySpawner> {
        let mut map = HashMap::new();
        for pack in &self.packs {
            map.extend(pack.entities.clone());
        }
        map
    }

    pub fn get_items(&self) -> HashMap<String, MinecraftItemSpawner> {
        let mut map = HashMap::new();
        for pack in &self.packs {
            map.extend(pack.items.clone());
        }
        map
    }

    /// Clear the parsed entities/items tables of all packs.
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
