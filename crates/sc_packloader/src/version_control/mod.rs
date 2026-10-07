use crate::block::{BlockDataManifest, BlockJsonBundle};
use crate::definitions::attribute::EntityAttribute;
use crate::definitions::biome::MinecraftBiomeSpawner;
use crate::pack::ResourcePack;
use crate::pack_loader::pack::zipped::ZippedResourcePack;
use crate::version_control::loader::VersionPackLoaderError;
use crate::version_control::runtime::MinecraftRuntimeJson;
use sc_ecs::resource::Resource;
use sc_utils::game::structs::minecraft_version::MinecraftVersion;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;
use zip::ZipArchive;

pub mod biome_features;
pub mod identifier;
pub(crate) mod json_budget;
pub mod loader;
pub mod manager;
pub mod runtime;
pub mod scdb;

#[derive(Deserialize, Debug, Clone)]
pub struct SCVersionPackManifest {
    pub name: String,
    pub description: String,
    pub authors: Vec<String>,
    pub minecraft_version: String,
    /// Network protocol version string (e.g. "1.26.30") for StartGame vanillaVersion,
    /// MOTD, ResourcePackStack game_version, and other online protocol fields.
    /// Differs from minecraft_version (e.g. "1.26.33"): the latter is the actual game version of
    /// the version pack/client, the former is the online version this protocol build supports.
    #[serde(default)]
    pub minecraft_network_version: Option<String>,
    pub protocol_version: u32,
    /// New block-data declaration (`definitions/blocks/<ns>/<path>.block.json`).
    /// When absent, falls back to the legacy `block_palette.nbt` path; once declared, load failures must not fall back.
    #[serde(default)]
    pub block_data: Option<BlockDataManifest>,
}

impl SCVersionPackManifest {
    /// Return the network version string. Falls back to minecraft_version when the manifest
    /// omits minecraft_network_version, preserving backward compatibility.
    pub fn network_version(&self) -> &str {
        self.minecraft_network_version
            .as_deref()
            .unwrap_or(&self.minecraft_version)
    }
}

/// Version-pack `definitions/worldgen/` data (density_function / noise terrain data).
/// Packloader only does IO and deserialization (no semantic parsing); semantic parsing lives in world generation.
#[derive(Debug, Clone, Default)]
pub struct WorldgenDataBundle {
    /// Relative path (e.g. "overworld/offset") to JSON bytes (under definitions/worldgen/density_function/).
    pub density_functions: HashMap<String, Vec<u8>>,
    /// Relative path (e.g. "continentalness") to JSON bytes (under definitions/worldgen/noise/).
    pub noises: HashMap<String, Vec<u8>>,
    /// Relative path (e.g. "overworld") to JSON bytes (under definitions/worldgen/noise_settings/).
    pub noise_settings: HashMap<String, Vec<u8>>,
}

#[derive(Resource, Debug, Clone)]
pub struct SCVersionPack {
    pub manifest: SCVersionPackManifest,
    behavior_packs: Vec<Vec<u8>>,
    biomes: Vec<MinecraftBiomeSpawner>,
    attributes: HashMap<String, EntityAttribute>,
    plugins: Vec<Vec<u8>>,
    pub runtime_id: Vec<MinecraftRuntimeJson>,
    /// Raw bytes of the vanilla entity_identifiers.dat file
    /// Holds the full vanilla entity identifier set loaded from the resource file
    entity_identifiers: Vec<u8>,
    /// Raw bytes of definitions/block_palette.nbt (optional; falls back to a bootstrapped dictionary when absent).
    /// Semantic parsing lives in sc_block; packloader does not touch block semantics.
    block_palette: Option<Vec<u8>>,
    /// Raw bytes of definitions/creative_items.json (optional Kaooot-format creative-item table).
    /// Parsed by MinecraftRuntimeManager (groups + per-item group_index).
    creative_items: Option<Vec<u8>>,
    biome_ids: HashMap<String, i16>,
    /// definitions/block_tags.json vanilla block tag table (tag to block-identifier list, optional).
    /// Tags are version-pack-level data (not inside block files); identifier-to-state resolution
    /// happens in consumers; packloader only does IO and deserialization.
    block_tags: HashMap<String, Vec<String>>,
    /// definitions/recipe_groups.json crafting ingredient group table
    /// (group name to item-identifier list, optional).
    ///
    /// Merged tag-query table plus Bedrock legacy pseudo-name families
    /// (e.g. `{"item": "minecraft:planks"/"minecraft:wood"/...}` expansion, in legacy data order;
    /// the `data` index semantics depend on it), generated offline and checked in.
    /// Packloader only does IO and deserialization; member-to-registry filtering happens in consumers
    /// (the recipe-registry loader), unknown members are dropped.
    recipe_groups: HashMap<String, Vec<String>>,
    /// definitions/biome_features.json worldgen feature-scheduling data (optional,
    /// extracted from biome_definitions.nbt). Scheduling semantics live in world generation.
    biome_features: biome_features::BiomeFeaturesData,
    /// definitions/worldgen/ terrain data (density_function + noise JSON).
    worldgen: WorldgenDataBundle,
    /// New block bundle (parsed when the manifest declares `block_data`; input to `sc_block` compilation).
    /// None when undeclared (legacy palette path is used).
    block_json_bundle: Option<BlockJsonBundle>,
    /// Whether the manifest declares `block_data` (declared bundles must not mix with legacy palette/tag bytes).
    block_json_declared: bool,
}

impl SCVersionPack {
    pub fn take_biomes(&mut self) -> Vec<MinecraftBiomeSpawner> {
        std::mem::take(&mut self.biomes)
    }

    /// Consume the entity_identifiers bytes to avoid keeping a duplicate copy in memory
    pub fn take_entity_identifiers(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.entity_identifiers)
    }
    /// Borrow the raw block_palette bytes (None when absent).
    ///
    /// Read by dlopen rust-kind plugins: each plugin DLL statically links its own
    /// sc_world copy with its own `BlockStateDictionary::global()`, so it needs
    /// the bytes to register into its own dictionary copy.
    pub fn block_palette_bytes(&self) -> Option<&[u8]> {
        self.block_palette.as_deref()
    }
    /// Take the raw block_palette bytes (None when absent).
    pub fn take_block_palette(&mut self) -> Option<Vec<u8>> {
        self.block_palette.take()
    }
    /// Take the raw creative_items.json bytes (None when absent).
    pub fn take_creative_items(&mut self) -> Option<Vec<u8>> {
        self.creative_items.take()
    }
    pub fn take_biome_ids(&mut self) -> HashMap<String, i16> {
        std::mem::take(&mut self.biome_ids)
    }
    /// Take the block tag table (tag to block-identifier list; definitions/block_tags.json, optional).
    pub fn take_block_tags(&mut self) -> HashMap<String, Vec<String>> {
        std::mem::take(&mut self.block_tags)
    }
    /// Whether block_tags is present (non-empty).
    pub fn has_block_tags(&self) -> bool {
        !self.block_tags.is_empty()
    }
    /// Take the crafting ingredient group table (group name to item-identifier list;
    /// definitions/recipe_groups.json, optional).
    pub fn take_recipe_groups(&mut self) -> HashMap<String, Vec<String>> {
        std::mem::take(&mut self.recipe_groups)
    }
    /// Whether recipe_groups is present (non-empty).
    pub fn has_recipe_groups(&self) -> bool {
        !self.recipe_groups.is_empty()
    }
    /// Take the feature-scheduling data (definitions/biome_features.json, optional).
    pub fn take_biome_features(&mut self) -> biome_features::BiomeFeaturesData {
        std::mem::take(&mut self.biome_features)
    }
    /// Whether biome_features is present (non-empty).
    pub fn has_biome_features(&self) -> bool {
        !self.biome_features.biomes.is_empty()
    }
    /// Take the block bundle (for `sc_block` compilation; the declared flag is kept after taking).
    pub fn take_block_json_bundle(&mut self) -> Option<BlockJsonBundle> {
        self.block_json_bundle.take()
    }

    /// Whether new block data is declared (declared packs must not fall back to or mix the legacy palette).
    pub fn has_declared_block_json(&self) -> bool {
        self.block_json_declared
    }

    /// Write derived palette bytes (snapshot-derived, consumed by dynamic plugin dictionary copies).
    pub fn set_derived_block_palette(&mut self, bytes: Vec<u8>) {
        self.block_palette = Some(bytes);
    }

    /// Take the worldgen data (density_function + noise JSON bundle).
    pub fn take_worldgen(&mut self) -> WorldgenDataBundle {
        std::mem::take(&mut self.worldgen)
    }
    /// Borrow the worldgen data (non-consuming).
    pub fn worldgen(&self) -> &WorldgenDataBundle {
        &self.worldgen
    }
    /// Whether worldgen data is present.
    pub fn has_worldgen(&self) -> bool {
        !self.worldgen.density_functions.is_empty() || !self.worldgen.noises.is_empty()
    }
    /// Whether block_palette is present.
    pub fn has_block_palette(&self) -> bool {
        self.block_palette.is_some()
    }
    /// In-pack plugin bytes (zip list; one entry per plugin package).
    /// Actual loading is done by the version-pack plugin loader:
    /// `kind:"rust"` resolves via the builtin factory, `kind:"cabi"` goes through HostPluginV1.
    pub fn plugins_bytes(&self) -> &[Vec<u8>] {
        &self.plugins
    }

    /// Consume embedded plugin archives after bootstrap takes ownership.
    pub fn take_plugins_bytes(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.plugins)
    }

    pub fn plugins_len(&self) -> usize {
        self.plugins.len()
    }
    pub fn take_behavior_packs(&mut self) -> Vec<(MinecraftVersion, ResourcePack)> {
        let mut behavior_packs: Vec<(MinecraftVersion, ResourcePack)> = Vec::new();
        for bytes in std::mem::take(&mut self.behavior_packs) {
            let shared_bytes = Arc::new(bytes);
            if let Ok(mut zip) = ZipArchive::new(Cursor::new(shared_bytes.as_slice())) {
                if let Some(pack) = ZippedResourcePack::get_resource_pack_shared(
                    &mut zip,
                    Arc::clone(&shared_bytes),
                ) {
                    behavior_packs.push((pack.manifest.version, pack));
                }
            }
        }
        behavior_packs
    }

    pub fn get_biomes(&self) -> Result<Vec<MinecraftBiomeSpawner>, VersionPackLoaderError> {
        Ok(self.biomes.clone())
    }

    pub fn take_attributes(&mut self) -> HashMap<String, EntityAttribute> {
        std::mem::take(&mut self.attributes)
    }

    pub fn take_runtime_id(&mut self) -> Vec<MinecraftRuntimeJson> {
        std::mem::take(&mut self.runtime_id)
    }
}
