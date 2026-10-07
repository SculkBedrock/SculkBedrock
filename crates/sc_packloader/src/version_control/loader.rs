use crate::block::{load_block_json_bundle, BlockBundleBudgets};
use crate::definitions::attribute::EntityAttribute;
use crate::pack_loader::pack::zipped::{
    for_each_dir_file, get_dir_files_bytes, get_file_by_name, MAX_BINARY_ENTRY_BYTES,
    MAX_JSON_ENTRY_BYTES,
};
use crate::pack_loader::zipped_loader::get_extension;
use crate::version_control::json_budget::BudgetReader;
use crate::version_control::runtime::MinecraftRuntimeJson;
use crate::version_control::scdb;
use crate::version_control::WorldgenDataBundle;
use crate::version_control::{SCVersionPack, SCVersionPackManifest};
use crate::{MinecraftJson, MinecraftJsonDeserializer};
use json_comments::StripComments;
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use zip::ZipArchive;

#[cfg(test)]
use sc_binary::ByteReader;
use sc_log::t_log;

fn json_entry_is_allowed(size: u64) -> bool {
    size <= MAX_JSON_ENTRY_BYTES
}

fn binary_entry_is_allowed(size: u64) -> bool {
    size <= MAX_BINARY_ENTRY_BYTES
}

#[derive(Debug)]
pub enum VersionPackLoaderError {
    ZipError(String, zip::result::ZipError),
    JsonError(String, serde_json::Error),
    IoError(String, std::io::Error),
    FileNotFound(&'static str),
    DeserializationError(String),
    //LoadPluginsError(SCPluginLoaderError),
}

impl std::fmt::Display for VersionPackLoaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for VersionPackLoaderError {}

pub struct VersionPackLoader {
    dir_path: PathBuf,
}

impl VersionPackLoader {
    pub fn new<P: AsRef<Path>>(dir_path: P) -> Self {
        Self {
            dir_path: dir_path.as_ref().to_path_buf(),
        }
    }

    pub fn get_versions(&self) -> Vec<SCVersionPack> {
        let mut versions = Vec::new();
        let cache_dir = self.dir_path.join(".scdb");
        scdb::clear_stale_cache(&cache_dir);
        let read_dir = match fs::read_dir(&self.dir_path) {
            Ok(read_dir) => read_dir,
            Err(error) => {
                log::warn!(
                    "{}",
                    t_log!("console.pack.dir_unreadable", dir = self.dir_path.display(), error = error)
                );
                return versions;
            }
        };
        for entry in read_dir {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    log::warn!(
                        "{}",
                        t_log!("console.pack.dir_entry", dir = self.dir_path.display(), error = error)
                    );
                    continue;
                }
            };
            let path = entry.path();
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    log::warn!(
                        "{}",
                        t_log!("console.pack.entry_meta", path = path.display(), error = error)
                    );
                    continue;
                }
            };
            let extension = get_extension(&path);
            if !metadata.is_file() || (extension != "zip" && extension != "scver") {
                continue;
            }
            let compiled_path = match scdb::load_or_compile_path(&path, &cache_dir) {
                Ok(compiled_path) => compiled_path,
                Err(error) => {
                    log::warn!(
                        "{}",
                        t_log!("console.pack.unreadable", path = path.display(), error = error)
                    );
                    continue;
                }
            };
            let file = match File::open(&compiled_path) {
                Ok(file) => file,
                Err(error) => {
                    log::warn!(
                        "{}",
                        t_log!("console.pack.compiled_open_fail", path = compiled_path.display(), error = error)
                    );
                    continue;
                }
            };
            let mut zip = match ZipArchive::new(file) {
                Ok(zip) => zip,
                Err(error) => {
                    log::warn!("{}", t_log!("console.pack.invalid", path = path.display(), error = error));
                    continue;
                }
            };
            match ZippedVersionPack::get_version(&mut zip) {
                Ok(version) => versions.push(version),
                Err(error) => {
                    log::warn!("skipping invalid version pack {}: {error}", path.display())
                }
            }
        }
        versions
    }
}

pub struct ZippedVersionPack;

impl ZippedVersionPack {
    fn get_manifest<T: Read + Seek>(
        zip: &mut ZipArchive<T>,
    ) -> Result<SCVersionPackManifest, VersionPackLoaderError> {
        let mut version = get_file_by_name(zip, "manifest.json")
            .map_err(|e| VersionPackLoaderError::ZipError("manifest.json".to_string(), e))?;
        if version.is_file() {
            if !json_entry_is_allowed(version.size()) {
                return Err(VersionPackLoaderError::DeserializationError(
                    "manifest.json exceeds JSON budget".to_string(),
                ));
            }
            serde_json::from_reader(BudgetReader::new(&mut version))
                .map_err(|e| VersionPackLoaderError::JsonError("manifest.json".to_string(), e))
        } else {
            Err(VersionPackLoaderError::FileNotFound("manifest.json"))
        }
    }

    fn get_runtime_id<T: Read + Seek>(
        zip: &mut ZipArchive<T>,
    ) -> Result<Vec<MinecraftRuntimeJson>, VersionPackLoaderError> {
        let mut runtime = get_file_by_name(zip, "definitions/runtime.json").map_err(|e| {
            VersionPackLoaderError::ZipError("definitions/runtime.json".to_string(), e)
        })?;
        if runtime.is_file() {
            if !json_entry_is_allowed(runtime.size()) {
                return Err(VersionPackLoaderError::DeserializationError(
                    "definitions/runtime.json exceeds JSON budget".to_string(),
                ));
            }
            serde_json::from_reader(BudgetReader::new(&mut runtime)).map_err(|e| {
                VersionPackLoaderError::JsonError("definitions/runtime.json".to_string(), e)
            })
        } else {
            Err(VersionPackLoaderError::FileNotFound(
                "definitions/attributes.json",
            ))
        }
    }

    fn get_behavior_packs<T: Read + Seek>(zip: &mut ZipArchive<T>) -> Vec<Vec<u8>> {
        let files = get_dir_files_bytes(zip, "behavior_packs");
        files.into_iter().map(|(_, bytes)| bytes).collect()
    }

    fn get_biomes<T: Read + Seek>(
        zip: &mut ZipArchive<T>,
    ) -> Result<Vec<crate::definitions::biome::MinecraftBiomeSpawner>, VersionPackLoaderError> {
        let mut biomes = Vec::new();
        let mut first_error = None;
        for_each_dir_file(zip, "definitions/biomes", |name, json| {
            if first_error.is_some() {
                return;
            }
            let parsed = serde_json::from_reader::<_, MinecraftJsonDeserializer>(
                StripComments::new(json.as_bytes()),
            );
            match parsed.map(|value| value.get()) {
                Ok(MinecraftJson::MinecraftBiomeSpawner(biome)) => biomes.push(biome),
                Ok(_) => first_error = Some(VersionPackLoaderError::DeserializationError(name)),
                Err(error) => first_error = Some(VersionPackLoaderError::JsonError(name, error)),
            }
        });
        match first_error {
            Some(error) => Err(error),
            None => Ok(biomes),
        }
    }

    fn get_biome_ids<T: Read + Seek>(zip: &mut ZipArchive<T>) -> HashMap<String, i16> {
        let Ok(mut file) = get_file_by_name(zip, "definitions/biome_ids.json") else {
            return HashMap::new();
        };
        if !file.is_file() {
            return HashMap::new();
        }
        if !json_entry_is_allowed(file.size()) {
            return HashMap::new();
        }
        serde_json::from_reader(BudgetReader::new(&mut file)).unwrap_or_default()
    }

    /// Read definitions/block_tags.json (optional block tag table).
    /// Missing, over-budget, or unparsable files fall back to an empty table.
    fn get_block_tags<T: Read + Seek>(zip: &mut ZipArchive<T>) -> HashMap<String, Vec<String>> {
        let Ok(mut file) = get_file_by_name(zip, "definitions/block_tags.json") else {
            return HashMap::new();
        };
        if !file.is_file() {
            return HashMap::new();
        }
        if !json_entry_is_allowed(file.size()) {
            return HashMap::new();
        }
        serde_json::from_reader(BudgetReader::new(&mut file)).unwrap_or_default()
    }

    /// Read definitions/recipe_groups.json (optional ingredient group table).
    ///
    /// Missing, over-budget, or unparsable files fall back to an empty table (callers are fail-closed: exact
    /// matches are unaffected, tag/pseudo-name queries have no members). Reuses the block_tags budget and style.
    fn get_recipe_groups<T: Read + Seek>(zip: &mut ZipArchive<T>) -> HashMap<String, Vec<String>> {
        let Ok(mut file) = get_file_by_name(zip, "definitions/recipe_groups.json") else {
            return HashMap::new();
        };
        if !file.is_file() {
            return HashMap::new();
        }
        if !json_entry_is_allowed(file.size()) {
            return HashMap::new();
        }
        serde_json::from_reader(BudgetReader::new(&mut file)).unwrap_or_default()
    }

    /// Read definitions/biome_features.json (optional worldgen feature-scheduling data).
    /// Returns default when absent; scheduling semantics live in world generation.
    fn get_biome_features<T: Read + Seek>(
        zip: &mut ZipArchive<T>,
    ) -> crate::version_control::biome_features::BiomeFeaturesData {
        let Ok(mut file) = get_file_by_name(zip, "definitions/biome_features.json") else {
            return Default::default();
        };
        if !file.is_file() {
            return Default::default();
        }
        if !json_entry_is_allowed(file.size()) {
            return Default::default();
        }
        serde_json::from_reader(BudgetReader::new(&mut file)).unwrap_or_default()
    }

    fn get_attributes<T: Read + Seek>(
        zip: &mut ZipArchive<T>,
    ) -> Result<HashMap<String, EntityAttribute>, VersionPackLoaderError> {
        let mut attributes = get_file_by_name(zip, "definitions/attributes.json").map_err(|e| {
            VersionPackLoaderError::ZipError("definitions/attributes.json".to_string(), e)
        })?;
        if attributes.is_file() {
            if !json_entry_is_allowed(attributes.size()) {
                return Err(VersionPackLoaderError::DeserializationError(
                    "definitions/attributes.json exceeds JSON budget".to_string(),
                ));
            }
            serde_json::from_reader(BudgetReader::new(&mut attributes)).map_err(|e| {
                VersionPackLoaderError::JsonError("definitions/attributes.json".to_string(), e)
            })
        } else {
            Err(VersionPackLoaderError::FileNotFound(
                "definitions/runtime.json",
            ))
        }
    }

    fn get_plugins<T: Read + Seek>(zip: &mut ZipArchive<T>) -> Vec<Vec<u8>> {
        get_dir_files_bytes(zip, "plugins")
            .into_iter()
            .map(|(_, bytes)| bytes)
            .collect()
    }

    /// Read definitions/entity_identifiers.nbt (required vanilla entity-identifier NBT).
    /// Returns FileNotFound when absent or unreadable.
    fn get_entity_identifiers<T: Read + Seek>(
        zip: &mut ZipArchive<T>,
    ) -> Result<Vec<u8>, VersionPackLoaderError> {
        let mut file =
            get_file_by_name(zip, "definitions/entity_identifiers.nbt").map_err(|e| {
                VersionPackLoaderError::ZipError(
                    "definitions/entity_identifiers.nbt".to_string(),
                    e,
                )
            })?;
        if file.is_file() && binary_entry_is_allowed(file.size()) {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|e| {
                VersionPackLoaderError::IoError("definitions/entity_identifiers.nbt".to_string(), e)
            })?;
            Ok(bytes)
        } else {
            Err(VersionPackLoaderError::FileNotFound(
                "definitions/entity_identifiers.nbt",
            ))
        }
    }

    /// Read definitions/block_palette.nbt (optional block palette bytes).
    /// Returns None when absent or over budget.
    fn get_block_palette<T: Read + Seek>(zip: &mut ZipArchive<T>) -> Option<Vec<u8>> {
        let mut file = get_file_by_name(zip, "definitions/block_palette.nbt").ok()?;
        if !file.is_file() {
            return None;
        }
        if !binary_entry_is_allowed(file.size()) {
            return None;
        }
        let mut bytes = Vec::with_capacity(file.size().min(usize::MAX as u64) as usize);
        file.read_to_end(&mut bytes).ok()?;
        Some(bytes)
    }

    /// Read definitions/creative_items.json (optional creative-item table).
    /// Returns None when absent or over budget.
    fn get_creative_items<T: Read + Seek>(zip: &mut ZipArchive<T>) -> Option<Vec<u8>> {
        let mut file = get_file_by_name(zip, "definitions/creative_items.json").ok()?;
        if !file.is_file() {
            return None;
        }
        if !json_entry_is_allowed(file.size()) {
            return None;
        }
        let mut bytes = Vec::with_capacity(file.size().min(usize::MAX as u64) as usize);
        file.read_to_end(&mut bytes).ok()?;
        Some(bytes)
    }

    /// Read definitions/worldgen/ density_function + noise JSON into a bundle.
    fn get_worldgen<T: Read + Seek>(zip: &mut ZipArchive<T>) -> WorldgenDataBundle {
        let mut bundle = WorldgenDataBundle::default();
        for (path, bytes) in get_dir_files_bytes(zip, "definitions/worldgen/density_function") {
            let rel = path
                .strip_prefix("definitions/worldgen/density_function/")
                .unwrap_or(&path);
            let key = rel.trim_end_matches(".json").to_string();
            bundle.density_functions.insert(key, bytes);
        }
        for (path, bytes) in get_dir_files_bytes(zip, "definitions/worldgen/noise") {
            let rel = path
                .strip_prefix("definitions/worldgen/noise/")
                .unwrap_or(&path);
            let key = rel.trim_end_matches(".json").to_string();
            bundle.noises.insert(key, bytes);
        }
        for (path, bytes) in get_dir_files_bytes(zip, "definitions/worldgen/noise_settings") {
            let rel = path
                .strip_prefix("definitions/worldgen/noise_settings/")
                .unwrap_or(&path);
            let key = rel.trim_end_matches(".json").to_string();
            bundle.noise_settings.insert(key, bytes);
        }
        bundle
    }

    pub fn get_version<T: Read + Seek>(
        zip: &mut ZipArchive<T>,
    ) -> Result<SCVersionPack, VersionPackLoaderError> {
        let manifest = Self::get_manifest(zip)?;
        let behavior_packs = Self::get_behavior_packs(zip);
        let runtime_id = Self::get_runtime_id(zip)?;
        let biomes = Self::get_biomes(zip)?;
        let biome_ids = Self::get_biome_ids(zip);
        let block_tags = Self::get_block_tags(zip);
        let recipe_groups = Self::get_recipe_groups(zip);
        let biome_features = Self::get_biome_features(zip);
        let attributes = Self::get_attributes(zip)?;
        let plugins = Self::get_plugins(zip);
        let entity_identifiers = Self::get_entity_identifiers(zip)?;
        let block_palette = Self::get_block_palette(zip);
        let creative_items = Self::get_creative_items(zip);
        let worldgen = Self::get_worldgen(zip);
        // New block bundle: only loaded when the manifest declares `block_data`; any failure after declaration
        // returns Err directly (the whole pack is skipped; no fallback to the legacy format).
        let pack_id = manifest.name.clone();
        let (block_json_bundle, block_json_declared) = match manifest.block_data.as_ref() {
            None => (None, false),
            Some(decl) => {
                let bundle =
                    load_block_json_bundle(zip, decl, &pack_id, &BlockBundleBudgets::default())
                        .map_err(|e| {
                            VersionPackLoaderError::DeserializationError(format!("block_data: {e}"))
                        })?;
                (bundle, true)
            }
        };
        Ok(SCVersionPack {
            manifest,
            behavior_packs,
            biomes,
            attributes,
            plugins,
            runtime_id,
            entity_identifiers,
            block_palette,
            creative_items,
            biome_ids,
            block_tags,
            recipe_groups,
            biome_features,
            worldgen,
            block_json_bundle,
            block_json_declared,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal synthetic version pack (manifest + required definitions + optional blocks dir).
    fn make_mini_pack(block_files: &[(&str, &str)], declare: bool) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        let mut manifest = serde_json::json!({
            "name": "MiniTest",
            "description": "test",
            "authors": [],
            "minecraft_version": "1.26.40",
            "protocol_version": 2168
        });
        if declare {
            manifest["block_data"] = serde_json::json!({
                "schema_version": 1,
                "directory": "definitions/blocks",
                "network_id_mode": "hashed"
            });
        }
        let entries: Vec<(String, Vec<u8>)> = std::iter::once((
            "manifest.json".to_string(),
            serde_json::to_vec(&manifest).unwrap(),
        ))
        .chain([
            ("definitions/runtime.json".to_string(), b"[]".to_vec()),
            ("definitions/attributes.json".to_string(), b"{}".to_vec()),
            (
                "definitions/entity_identifiers.nbt".to_string(),
                b"\x00".to_vec(),
            ),
        ])
        .chain(
            block_files
                .iter()
                .map(|(n, c)| (n.to_string(), c.as_bytes().to_vec())),
        )
        .collect();
        for (name, bytes) in entries {
            writer.start_file(name, options).unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    const MINI_AIR: &str = r#"{
        "format_version": "1.10.0",
        "minecraft:block": {
            "description": {"identifier": "minecraft:air", "states": {}},
            "components": {},
            "sc:default_state": {},
            "sc:protocol_runtime_ids": [0]
        }
    }"#;

    /// Without a block_data declaration, use the legacy path (bundle is None, undeclared).
    #[test]
    fn legacy_pack_without_declaration_has_no_bundle() {
        let bytes = make_mini_pack(&[], false);
        let mut zip = ZipArchive::new(ByteReader::from(bytes)).expect("zip 打开失败");
        let mut pack = ZippedVersionPack::get_version(&mut zip).expect("旧包应可加载");
        assert!(!pack.has_declared_block_json());
        assert!(pack.take_block_json_bundle().is_none());
    }

    /// Declared packs load the bundle normally; zero or broken files fail the whole pack (no fallback).
    #[test]
    fn declared_pack_loads_bundle_and_fails_loud_without_fallback() {
        let bytes = make_mini_pack(
            &[("definitions/blocks/minecraft/air.block.json", MINI_AIR)],
            true,
        );
        let mut zip = ZipArchive::new(ByteReader::from(bytes)).expect("zip 打开失败");
        let mut pack = ZippedVersionPack::get_version(&mut zip).expect("声明包应可加载");
        assert!(pack.has_declared_block_json());
        let bundle = pack.take_block_json_bundle().expect("bundle 应存在");
        assert_eq!(bundle.files.len(), 1);
        assert_eq!(bundle.files[0].identifier, "minecraft:air");
        assert_eq!(bundle.network_id_mode, "hashed");

        // Declared but zero files: fail the whole pack.
        let bytes = make_mini_pack(&[], true);
        let mut zip = ZipArchive::new(ByteReader::from(bytes)).expect("zip 打开失败");
        ZippedVersionPack::get_version(&mut zip).expect_err("零文件必须整包失败");

        // Declared but broken file: fail the whole pack (path-attributed error, no legacy fallback).
        let bytes = make_mini_pack(
            &[("definitions/blocks/minecraft/air.block.json", "{oops")],
            true,
        );
        let mut zip = ZipArchive::new(ByteReader::from(bytes)).expect("zip 打开失败");
        let e = ZippedVersionPack::get_version(&mut zip).expect_err("坏文件必须整包失败");
        assert!(e.to_string().contains("air.block.json"), "实际：{e}");
    }

    /// Real version-pack integration test: definitions/block_tags.json must load once injected.
    /// Tag table from upstream gamedata (tag to block-identifier list, 39 tags).
    #[test]
    fn load_block_tags_from_real_version_pack() {
        let pack_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../version_packs/[SC原版版本包] Vanilla-1.26.40.scver"
        );
        let bytes = match fs::read(pack_path) {
            Ok(bytes) => bytes,
            Err(e) => panic!("failed to read version pack {pack_path}: {e}"),
        };
        let mut zip = ZipArchive::new(ByteReader::from(bytes)).expect("版本包 zip 打开失败");
        let mut pack = ZippedVersionPack::get_version(&mut zip).expect("版本包解析失败");

        assert!(pack.has_block_tags(), "block_tags 应存在且非空");
        let tags = pack.take_block_tags();
        assert_eq!(tags.len(), 39, "上游 1.26.40 gamedata 应含 39 个 tag");

        // Check minecraft:dirt members (source for the isSupportDirt check).
        let dirt = tags.get("minecraft:dirt").expect("应有 minecraft:dirt tag");
        for member in [
            "minecraft:dirt",
            "minecraft:grass_block",
            "minecraft:podzol",
            "minecraft:mycelium",
            "minecraft:coarse_dirt",
            "minecraft:farmland",
            "minecraft:moss_block",
            "minecraft:mud",
            "minecraft:dirt_with_roots",
            "minecraft:muddy_mangrove_roots",
            "minecraft:pale_moss_block",
        ] {
            assert!(
                dirt.contains(&member.to_string()),
                "minecraft:dirt 应包含 {member}"
            );
        }

        // Spot-check minecraft:grass / minecraft:log (support/trunk tags for tree generation).
        assert!(tags.contains_key("minecraft:grass"));
        assert!(tags.contains_key("minecraft:log"));
    }

    /// Real version-pack integration test: definitions/recipe_groups.json must load.
    /// Merged tag table plus legacy pseudo-name families (planks/wood/dye/...,
    /// in legacy data order), generated offline and checked in.
    #[test]
    fn load_recipe_groups_from_real_version_pack() {
        let pack_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../version_packs/[SC原版版本包] Vanilla-1.26.40.scver"
        );
        let bytes = match fs::read(pack_path) {
            Ok(bytes) => bytes,
            Err(e) => panic!("failed to read version pack {pack_path}: {e}"),
        };
        let mut zip = ZipArchive::new(ByteReader::from(bytes)).expect("版本包 zip 打开失败");
        let mut pack = ZippedVersionPack::get_version(&mut zip).expect("版本包解析失败");

        assert!(pack.has_recipe_groups(), "recipe_groups 应存在且非空");
        let groups = pack.take_recipe_groups();
        // Spot-check tag-side keys.
        let planks = groups.get("minecraft:planks").expect("应有 planks 组");
        for member in ["minecraft:oak_planks", "minecraft:birch_planks"] {
            assert!(planks.contains(&member.to_string()), "planks 应包含 {member}");
        }
        // Spot-check legacy pseudo-name keys.
        let wood = groups.get("minecraft:wood").expect("应有 wood 伪名组");
        assert!(wood.contains(&"minecraft:oak_wood".to_string()));
        let dye = groups.get("minecraft:dye").expect("应有 dye 伪名组");
        assert!(dye.contains(&"minecraft:ink_sac".to_string()));
        // Legacy data order: planks[0] must be oak (data index semantics depend on it).
        assert_eq!(planks[0], "minecraft:oak_planks");
        // Should be empty after consumption.
        assert!(!pack.has_recipe_groups());
    }

    /// Real version-pack integration test: official 1.26.40.05 biome data must load:
    /// 88 biome json files (including new biomes such as deep_dark/cherry_grove/pale_garden,
    /// 17 new biomes whose ids worldgen writes and clients need) + 88
    /// biome_ids.json mappings (exported from the worldgen biome_id constant table).
    #[test]
    fn load_official_biomes_and_ids_from_real_version_pack() {
        let pack_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../version_packs/[SC原版版本包] Vanilla-1.26.40.scver"
        );
        let bytes = match fs::read(pack_path) {
            Ok(bytes) => bytes,
            Err(e) => panic!("failed to read version pack {pack_path}: {e}"),
        };
        let mut zip = ZipArchive::new(ByteReader::from(bytes)).expect("版本包 zip 打开失败");

        // Biome json count is 88 (full official 1.26.40.05 set).
        let mut biome_count = 0;
        for name in zip.file_names() {
            if name.starts_with("definitions/biomes/") && name.ends_with(".biome.json") {
                biome_count += 1;
            }
        }
        assert_eq!(biome_count, 88, "官方 1.26.40.05 biome json 应为 88 个");

        let mut pack = ZippedVersionPack::get_version(&mut zip).expect("版本包解析失败");
        let biome_ids = pack.take_biome_ids();
        assert_eq!(biome_ids.len(), 88, "biome_ids.json 应含 88 条映射");

        // Spot-check new biome ids (match the worldgen biome_id constant table).
        assert_eq!(biome_ids.get("minecraft:plains"), Some(&1));
        assert_eq!(biome_ids.get("minecraft:deep_dark"), Some(&190));
        assert_eq!(biome_ids.get("minecraft:pale_garden"), Some(&193));

        // Full deserialization: all 88 biome json files (including new official components
        // such as minecraft:village_type; BiomeComponents keeps unknown components in an untagged map).
        let biomes = pack.get_biomes().expect("biome json 全量反序列化失败");
        assert_eq!(biomes.len(), 88, "应解析出 88 个 MinecraftBiomeSpawner");
        let identifiers: Vec<&str> = biomes
            .iter()
            .map(|b| b.description.identifier.as_str())
            .collect();
        for expected in [
            "minecraft:deep_dark",
            "minecraft:cherry_grove",
            "minecraft:pale_garden",
            "minecraft:crimson_forest",
            "minecraft:soulsand_valley",
        ] {
            assert!(
                identifiers.contains(&expected),
                "官方新群系 {expected} 应可解析"
            );
        }
    }

    /// Real version-pack integration test: definitions/biome_features.json (feature-scheduling data
    /// extracted from biome_definitions.nbt) must load.
    /// Covers 88 biomes / 4273 features; spot-check the key tree feature
    /// (forest_surface_trees_feature matches ForestTreeFeature.NAME).
    #[test]
    fn load_biome_features_from_real_version_pack() {
        let pack_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../version_packs/[SC原版版本包] Vanilla-1.26.40.scver"
        );
        let bytes = match fs::read(pack_path) {
            Ok(bytes) => bytes,
            Err(e) => panic!("failed to read version pack {pack_path}: {e}"),
        };
        let mut zip = ZipArchive::new(ByteReader::from(bytes)).expect("版本包 zip 打开失败");
        let mut pack = ZippedVersionPack::get_version(&mut zip).expect("版本包解析失败");

        assert!(pack.has_biome_features(), "biome_features 应存在且非空");
        let data = pack.take_biome_features();
        assert_eq!(data.biomes.len(), 88, "features 调度应覆盖 88 个群系");

        let total: usize = data.biomes.values().map(|b| b.features.len()).sum();
        assert_eq!(total, 4273, "feature 总数应为 4273");

        // Forest tree feature: identifier matches ForestTreeFeature.NAME;
        // eval_order/iterations match the NBT-extracted values.
        let forest = data
            .biomes
            .get("minecraft:forest")
            .expect("minecraft:forest 应有 features");
        let tree = forest
            .features
            .iter()
            .find(|f| f.identifier == "minecraft:forest_surface_trees_feature")
            .expect("forest 应含 forest_surface_trees_feature");
        assert_eq!(tree.feature, "minecraft:legacy:forest_tree_feature");
        assert_eq!(tree.pass, "surface_pass");
        assert_eq!(tree.scatter.eval_order, "ZXY");
        assert_eq!(tree.scatter.iterations, 10);
        assert_eq!(tree.scatter.coordinates.len(), 3, "x/y/z 三轴坐标");

        // Consumers (take-then-get) should see empty data.
        assert!(!pack.has_biome_features());
    }
}
