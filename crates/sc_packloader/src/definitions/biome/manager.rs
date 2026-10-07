use crate::definitions::biome::MinecraftBiomeSpawner;
use log::debug;
use sc_ecs::resource::Resource;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use std::collections::HashMap;

#[derive(Clone, Debug, Resource)]
pub struct MinecraftBiomeManager {
    map: HashMap<String, MinecraftBiomeSpawner>,
    ids: HashMap<String, i16>,
    nbt: Option<NbtValue>,
}

impl MinecraftBiomeManager {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            ids: HashMap::new(),
            nbt: None,
        }
    }

    pub fn from_vec(biomes: Vec<MinecraftBiomeSpawner>) -> Self {
        let mut manager = Self::new();
        for b in biomes {
            manager.push_biome(b);
        }
        manager
    }

    pub fn push_biome(&mut self, biome: MinecraftBiomeSpawner) {
        self.map.insert(biome.description.identifier.clone(), biome);
    }

    pub fn get_biome(&self, identifier: &str) -> Option<&MinecraftBiomeSpawner> {
        self.map.get(identifier)
    }

    pub fn set_biome_ids(&mut self, ids: HashMap<String, i16>) {
        self.ids = ids;
    }

    pub fn biome_id(&self, identifier: &str) -> Option<i16> {
        self.ids.get(identifier).copied().or_else(|| {
            identifier
                .strip_prefix("minecraft:")
                .and_then(|short| self.ids.get(short).copied())
        })
    }

    /// Iterate all loaded biomes (used to build BiomeDefinitionList, in biome registry order).
    pub fn iter(&self) -> impl Iterator<Item = &MinecraftBiomeSpawner> {
        self.map.values()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn build_nbt(&mut self) {
        debug!("BiomeManager: building biome list NBT...");
        let mut compound = CompoundNbt::new(Some("".to_string()));
        for (identifier, biome) in self.map.iter() {
            if let Some(components) = biome.components.as_ref() {
                if let Ok(nbt) = components.to_nbt() {
                    compound.insert(identifier, nbt);
                }
            }
        }
        self.nbt = Some(NbtValue::Compound(compound));
    }

    pub fn get_nbt(&self) -> Option<&NbtValue> {
        self.nbt.as_ref()
    }

    /// Export the biome id to official tag-set (`minecraft:tags` component) mapping.
    ///
    /// Consumer: the tree feature `canSpawnHere` check in world generation
    /// (same query as `Registries.BIOME.containsTag(BiomeTags.X, biomeId)`).
    /// Tag strings match the official biome json `minecraft:tags.tags` (e.g. "forest"/"jungle").
    pub fn biome_tags_by_id(&self) -> HashMap<i32, std::collections::HashSet<String>> {
        use crate::definitions::biome::component::Tags;
        let mut result = HashMap::new();
        for (identifier, biome) in self.map.iter() {
            let Some(id) = self.biome_id(identifier) else {
                continue;
            };
            let Some(components) = biome.components.as_ref() else {
                continue;
            };
            if let Some(tags) = components.get::<Tags>() {
                result.insert(id as i32, tags.tags.iter().cloned().collect());
            }
        }
        result
    }
}
