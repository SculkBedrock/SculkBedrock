use crate::block::load_block_json_bundle;
use crate::definitions::attribute::EntityAttribute;
use crate::pack_loader::pack::zipped::{PackSource, MAX_BINARY_ENTRY_BYTES, MAX_JSON_ENTRY_BYTES};
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
use std::path::{Path, PathBuf};

use sc_error::{ScError, ScSite, capture_site};
use sc_log::t_log;

fn json_entry_is_allowed(size: u64) -> bool {
    size <= MAX_JSON_ENTRY_BYTES
}

fn binary_entry_is_allowed(size: u64) -> bool {
    size <= MAX_BINARY_ENTRY_BYTES
}

#[derive(Debug, ScError)]
pub enum VersionPackLoaderError {
    /// Zip entry failure.
    #[sc_error(code = "packloader.version.zip")]
    ZipError {
        /// Entry path.
        path: String,
        /// Underlying zip error.
        source: zip::result::ZipError,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// JSON parse failure.
    #[sc_error(code = "packloader.version.json")]
    JsonError {
        /// Entry path.
        path: String,
        /// Underlying JSON error.
        source: serde_json::Error,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// File I/O failure.
    #[sc_error(code = "packloader.version.io")]
    IoError {
        /// Entry path.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Required file is missing.
    #[sc_error(code = "packloader.version.not_found")]
    FileNotFound {
        /// Missing entry path.
        path: &'static str,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Entry contents fail validation.
    #[sc_error(code = "packloader.version.deserialization")]
    DeserializationError {
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    //LoadPluginsError(SCPluginLoaderError),
}

impl VersionPackLoaderError {
    /// Build a zip error, capturing the call site.
    #[track_caller]
    pub fn zip_error(path: String, source: zip::result::ZipError) -> Self {
        let detail = format!("failed to read '{path}': {source}");
        Self::ZipError {
            path,
            source,
            detail,
            site: capture_site(),
        }
    }

    /// Build a JSON error, capturing the call site.
    #[track_caller]
    pub fn json_error(path: String, source: serde_json::Error) -> Self {
        let detail = format!("failed to parse '{path}': {source}");
        Self::JsonError {
            path,
            source,
            detail,
            site: capture_site(),
        }
    }

    /// Build an I/O error, capturing the call site.
    #[track_caller]
    pub fn io_error(path: String, source: std::io::Error) -> Self {
        let detail = format!("failed to read '{path}': {source}");
        Self::IoError {
            path,
            source,
            detail,
            site: capture_site(),
        }
    }

    /// Build a missing-file error, capturing the call site.
    #[track_caller]
    pub fn file_not_found(path: &'static str) -> Self {
        Self::FileNotFound {
            path,
            detail: format!("required file '{path}' not found"),
            site: capture_site(),
        }
    }

    /// Build a validation error, capturing the call site.
    #[track_caller]
    pub fn deserialization_error(detail: String) -> Self {
        Self::DeserializationError {
            detail,
            site: capture_site(),
        }
    }
}

impl std::fmt::Display for VersionPackLoaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZipError { detail, site, .. } => {
                write!(f, "[packloader.version.zip] {detail} (at {site})")
            }
            Self::JsonError { detail, site, .. } => {
                write!(f, "[packloader.version.json] {detail} (at {site})")
            }
            Self::IoError { detail, site, .. } => {
                write!(f, "[packloader.version.io] {detail} (at {site})")
            }
            Self::FileNotFound { detail, site, .. } => {
                write!(f, "[packloader.version.not_found] {detail} (at {site})")
            }
            Self::DeserializationError { detail, site } => {
                write!(f, "[packloader.version.deserialization] {detail} (at {site})")
            }
        }
    }
}

impl Error for VersionPackLoaderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ZipError { source, .. } => Some(source),
            Self::JsonError { source, .. } => Some(source),
            Self::IoError { source, .. } => Some(source),
            _ => None,
        }
    }
}


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
                    t_log!(
                        "console.pack.dir_unreadable",
                        dir = self.dir_path.display(),
                        error = error
                    )
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
                        t_log!(
                            "console.pack.dir_entry",
                            dir = self.dir_path.display(),
                            error = error
                        )
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
                        t_log!(
                            "console.pack.entry_meta",
                            path = path.display(),
                            error = error
                        )
                    );
                    continue;
                }
            };
            let extension = get_extension(&path);
            // Directory packs (extracted layout, must hold manifest.json);
            // dot-dirs (`.scdb` cache) are never packs.
            if metadata.is_dir() {
                if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('.'))
                {
                    continue;
                }
                if !path.join("manifest.json").is_file() {
                    continue;
                }
                let mut pack = PackSource::from_dir(path.clone());
                match ZippedVersionPack::get_version(&mut pack) {
                    Ok(version) => versions.push(version),
                    Err(error) => {
                        log::warn!("skipping invalid version pack {}: {error}", path.display())
                    }
                }
                continue;
            }
            if !metadata.is_file() || (extension != "zip" && extension != "scver") {
                continue;
            }
            let compiled_path = match scdb::load_or_compile_path(&path, &cache_dir) {
                Ok(compiled_path) => compiled_path,
                Err(error) => {
                    log::warn!(
                        "{}",
                        t_log!(
                            "console.pack.unreadable",
                            path = path.display(),
                            error = error
                        )
                    );
                    continue;
                }
            };
            let bytes = match fs::read(&compiled_path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    log::warn!(
                        "{}",
                        t_log!(
                            "console.pack.compiled_open_fail",
                            path = compiled_path.display(),
                            error = error
                        )
                    );
                    continue;
                }
            };
            let mut pack = match PackSource::from_bytes(bytes) {
                Ok(pack) => pack,
                Err(error) => {
                    log::warn!(
                        "{}",
                        t_log!("console.pack.invalid", path = path.display(), error = error)
                    );
                    continue;
                }
            };
            match ZippedVersionPack::get_version(&mut pack) {
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
    fn get_manifest(
        pack: &mut PackSource,
    ) -> Result<SCVersionPackManifest, VersionPackLoaderError> {
        let Some(size) = pack.file_size("manifest.json") else {
            return Err(VersionPackLoaderError::file_not_found("manifest.json"));
        };
        if !json_entry_is_allowed(size) {
            return Err(VersionPackLoaderError::deserialization_error(
                "manifest.json exceeds JSON budget".to_string(),
            ));
        }
        let Some(bytes) = pack.read_file("manifest.json") else {
            return Err(VersionPackLoaderError::file_not_found("manifest.json"));
        };
        serde_json::from_reader(BudgetReader::new(bytes.as_slice()))
            .map_err(|e| VersionPackLoaderError::json_error("manifest.json".to_string(), e))
    }

    fn get_runtime_id(
        pack: &mut PackSource,
    ) -> Result<Vec<MinecraftRuntimeJson>, VersionPackLoaderError> {
        let Some(size) = pack.file_size("definitions/runtime.json") else {
            return Err(VersionPackLoaderError::file_not_found(
                "definitions/attributes.json",
            ));
        };
        if !json_entry_is_allowed(size) {
            return Err(VersionPackLoaderError::deserialization_error(
                "definitions/runtime.json exceeds JSON budget".to_string(),
            ));
        }
        let Some(bytes) = pack.read_file("definitions/runtime.json") else {
            return Err(VersionPackLoaderError::file_not_found(
                "definitions/attributes.json",
            ));
        };
        serde_json::from_reader(BudgetReader::new(bytes.as_slice())).map_err(|e| {
            VersionPackLoaderError::json_error("definitions/runtime.json".to_string(), e)
        })
    }

    /// Open every nested behavior pack as its own source (directory packs
    /// first, then zip packs in archive order; deterministic for a given
    /// version pack). Public so external consumers (e.g. `sc_block` tests)
    /// can feed the same sources to the block bundle loader.
    pub fn get_behavior_packs(pack: &mut PackSource) -> Vec<PackSource> {
        // Directory wins on a file/dir name collision (active edit).
        let mut out = Vec::new();
        let mut dir_stems = std::collections::HashSet::new();
        if let PackSource::Dir(root) = pack {
            for sub in
                PackSource::from_dir(root.clone()).subdirs_with("behavior_packs", "manifest.json")
            {
                if let Some(stem) = sub.rsplit('/').next() {
                    dir_stems.insert(stem.to_ascii_lowercase());
                }
                out.push(PackSource::from_dir(root.join(&sub)));
            }
        }
        for (name, bytes) in pack.dir_files_bytes("behavior_packs") {
            let lower = name.to_ascii_lowercase();
            if !(lower.ends_with(".zip") || lower.ends_with(".mcpack")) {
                continue;
            }
            let stem = name
                .rsplit('/')
                .next()
                .unwrap_or(&name)
                .rsplit_once('.')
                .map(|(stem, _)| stem.to_ascii_lowercase())
                .unwrap_or_default();
            if dir_stems.contains(&stem) {
                log::debug!("version pack dir wins over file for {name}");
                continue;
            }
            match PackSource::from_bytes(bytes) {
                Ok(source) => out.push(source),
                Err(error) => log::warn!("behavior pack 打包无法打开 {name}: {error}"),
            }
        }
        out
    }

    fn get_biomes(
        pack: &mut PackSource,
    ) -> Result<Vec<crate::definitions::biome::MinecraftBiomeSpawner>, VersionPackLoaderError> {
        let mut biomes = Vec::new();
        let mut first_error = None;
        pack.for_each_json_file("definitions/biomes", &mut |name, json| {
            if first_error.is_some() {
                return;
            }
            let parsed = serde_json::from_reader::<_, MinecraftJsonDeserializer>(
                StripComments::new(json.as_bytes()),
            );
            match parsed.map(|value| value.get()) {
                Ok(MinecraftJson::MinecraftBiomeSpawner(biome)) => biomes.push(biome),
                Ok(_) => first_error = Some(VersionPackLoaderError::deserialization_error(name)),
                Err(error) => first_error = Some(VersionPackLoaderError::json_error(name, error)),
            }
        });
        match first_error {
            Some(error) => Err(error),
            None => Ok(biomes),
        }
    }

    fn get_biome_ids(pack: &mut PackSource) -> HashMap<String, i16> {
        let Some(size) = pack.file_size("definitions/biome_ids.json") else {
            return HashMap::new();
        };
        if !json_entry_is_allowed(size) {
            return HashMap::new();
        }
        let Some(bytes) = pack.read_file("definitions/biome_ids.json") else {
            return HashMap::new();
        };
        serde_json::from_reader(BudgetReader::new(bytes.as_slice())).unwrap_or_default()
    }

    /// Read definitions/block_tags.json (optional block tag table).
    /// Missing, over-budget, or unparsable files fall back to an empty table.
    fn get_block_tags(pack: &mut PackSource) -> HashMap<String, Vec<String>> {
        let Some(size) = pack.file_size("definitions/block_tags.json") else {
            return HashMap::new();
        };
        if !json_entry_is_allowed(size) {
            return HashMap::new();
        }
        let Some(bytes) = pack.read_file("definitions/block_tags.json") else {
            return HashMap::new();
        };
        serde_json::from_reader(BudgetReader::new(bytes.as_slice())).unwrap_or_default()
    }

    /// Read definitions/recipe_groups.json (optional ingredient group table).
    ///
    /// Missing, over-budget, or unparsable files fall back to an empty table (callers are fail-closed: exact
    /// matches are unaffected, tag/pseudo-name queries have no members). Reuses the block_tags budget and style.
    fn get_recipe_groups(pack: &mut PackSource) -> HashMap<String, Vec<String>> {
        let Some(size) = pack.file_size("definitions/recipe_groups.json") else {
            return HashMap::new();
        };
        if !json_entry_is_allowed(size) {
            return HashMap::new();
        }
        let Some(bytes) = pack.read_file("definitions/recipe_groups.json") else {
            return HashMap::new();
        };
        serde_json::from_reader(BudgetReader::new(bytes.as_slice())).unwrap_or_default()
    }

    /// Read definitions/biome_features.json (optional worldgen feature-scheduling data).
    /// Returns default when absent; scheduling semantics live in world generation.
    fn get_biome_features(
        pack: &mut PackSource,
    ) -> crate::version_control::biome_features::BiomeFeaturesData {
        let Some(size) = pack.file_size("definitions/biome_features.json") else {
            return Default::default();
        };
        if !json_entry_is_allowed(size) {
            return Default::default();
        }
        let Some(bytes) = pack.read_file("definitions/biome_features.json") else {
            return Default::default();
        };
        serde_json::from_reader(BudgetReader::new(bytes.as_slice())).unwrap_or_default()
    }

    fn get_attributes(
        pack: &mut PackSource,
    ) -> Result<HashMap<String, EntityAttribute>, VersionPackLoaderError> {
        let Some(size) = pack.file_size("definitions/attributes.json") else {
            return Err(VersionPackLoaderError::file_not_found(
                "definitions/runtime.json",
            ));
        };
        if !json_entry_is_allowed(size) {
            return Err(VersionPackLoaderError::deserialization_error(
                "definitions/attributes.json exceeds JSON budget".to_string(),
            ));
        }
        let Some(bytes) = pack.read_file("definitions/attributes.json") else {
            return Err(VersionPackLoaderError::file_not_found(
                "definitions/runtime.json",
            ));
        };
        serde_json::from_reader(BudgetReader::new(bytes.as_slice())).map_err(|e| {
            VersionPackLoaderError::json_error("definitions/attributes.json".to_string(), e)
        })
    }

    fn get_plugins(pack: &mut PackSource) -> Vec<Vec<u8>> {
        pack.dir_files_bytes("plugins")
            .into_iter()
            .map(|(_, bytes)| bytes)
            .collect()
    }

    /// Read definitions/entity_identifiers.nbt (required vanilla entity-identifier NBT).
    /// Returns FileNotFound when absent or unreadable.
    fn get_entity_identifiers(pack: &mut PackSource) -> Result<Vec<u8>, VersionPackLoaderError> {
        let Some(size) = pack.file_size("definitions/entity_identifiers.nbt") else {
            return Err(VersionPackLoaderError::file_not_found(
                "definitions/entity_identifiers.nbt",
            ));
        };
        if !binary_entry_is_allowed(size) {
            return Err(VersionPackLoaderError::file_not_found(
                "definitions/entity_identifiers.nbt",
            ));
        }
        pack.read_file("definitions/entity_identifiers.nbt").ok_or(
            VersionPackLoaderError::file_not_found("definitions/entity_identifiers.nbt"),
        )
    }

    /// Read definitions/block_palette.nbt (optional block palette bytes).
    /// Returns None when absent or over budget.
    fn get_block_palette(pack: &mut PackSource) -> Option<Vec<u8>> {
        let size = pack.file_size("definitions/block_palette.nbt")?;
        if !binary_entry_is_allowed(size) {
            return None;
        }
        pack.read_file("definitions/block_palette.nbt")
    }

    /// Read definitions/creative_items.json (optional creative-item table).
    /// Returns None when absent or over budget.
    fn get_creative_items(pack: &mut PackSource) -> Option<Vec<u8>> {
        let size = pack.file_size("definitions/creative_items.json")?;
        if !json_entry_is_allowed(size) {
            return None;
        }
        pack.read_file("definitions/creative_items.json")
    }

    /// Read definitions/worldgen/ density_function + noise JSON into a bundle.
    fn get_worldgen(pack: &mut PackSource) -> WorldgenDataBundle {
        let mut bundle = WorldgenDataBundle::default();
        for (path, bytes) in pack.dir_files_bytes("definitions/worldgen/density_function") {
            let rel = path
                .strip_prefix("definitions/worldgen/density_function/")
                .unwrap_or(&path);
            let key = rel.trim_end_matches(".json").to_string();
            bundle.density_functions.insert(key, bytes);
        }
        for (path, bytes) in pack.dir_files_bytes("definitions/worldgen/noise") {
            let rel = path
                .strip_prefix("definitions/worldgen/noise/")
                .unwrap_or(&path);
            let key = rel.trim_end_matches(".json").to_string();
            bundle.noises.insert(key, bytes);
        }
        for (path, bytes) in pack.dir_files_bytes("definitions/worldgen/noise_settings") {
            let rel = path
                .strip_prefix("definitions/worldgen/noise_settings/")
                .unwrap_or(&path);
            let key = rel.trim_end_matches(".json").to_string();
            bundle.noise_settings.insert(key, bytes);
        }
        bundle
    }

    pub fn get_version(pack: &mut PackSource) -> Result<SCVersionPack, VersionPackLoaderError> {
        let manifest = Self::get_manifest(pack)?;
        let mut behavior_packs = Self::get_behavior_packs(pack);
        let runtime_id = Self::get_runtime_id(pack)?;
        let biomes = Self::get_biomes(pack)?;
        let biome_ids = Self::get_biome_ids(pack);
        let block_tags = Self::get_block_tags(pack);
        let recipe_groups = Self::get_recipe_groups(pack);
        let biome_features = Self::get_biome_features(pack);
        let attributes = Self::get_attributes(pack)?;
        let plugins = Self::get_plugins(pack);
        let entity_identifiers = Self::get_entity_identifiers(pack)?;
        let block_palette = Self::get_block_palette(pack);
        let creative_items = Self::get_creative_items(pack);
        let worldgen = Self::get_worldgen(pack);
        // New block bundle: only loaded when the manifest declares `block_data`; any failure after declaration
        // returns Err directly (the whole pack is skipped; no fallback to the legacy format).
        let pack_id = manifest.name.clone();
        let (block_json_bundle, block_json_declared) = match manifest.block_data.as_ref() {
            None => (None, false),
            Some(decl) => {
                let bundle =
                    load_block_json_bundle(&mut behavior_packs, decl, &pack_id).map_err(|e| {
                        VersionPackLoaderError::deserialization_error(format!("block_data: {e}"))
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

    /// Build a minimal synthetic version pack directory (same layout as
    /// the zip form). Returns the dir path; callers remove it when done.
    fn make_mini_pack_dir(tag: &str, with_inner_pack: bool) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sc_pack_dir_test_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let manifest = serde_json::json!({
            "name": "MiniDirTest",
            "description": "test",
            "authors": [],
            "minecraft_version": "1.26.40",
            "protocol_version": 2168
        });
        std::fs::create_dir_all(dir.join("definitions")).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("definitions/runtime.json"), b"[]").unwrap();
        std::fs::write(dir.join("definitions/attributes.json"), b"{}").unwrap();
        std::fs::write(dir.join("definitions/entity_identifiers.nbt"), b"\x00").unwrap();
        if with_inner_pack {
            let inner = dir.join("behavior_packs/bp1");
            std::fs::create_dir_all(inner.join("recipes")).unwrap();
            std::fs::create_dir_all(inner.join("items")).unwrap();
            std::fs::write(
                inner.join("manifest.json"),
                r#"{"format_version":2,"header":{"name":"bp1","uuid":"00000000-0000-0000-0000-000000000001","version":[1,0,0]}}"#,
            )
            .unwrap();
            std::fs::write(
                inner.join("recipes/r.json"),
                r#"{"format_version":"1.12","minecraft:recipe_shapeless":{"description":{"identifier":"minecraft:dir_test"},"tags":["crafting_table"],"ingredients":[{"item":"minecraft:stone"}],"result":{"item":"minecraft:out"}}}"#,
            )
            .unwrap();
            std::fs::write(
                inner.join("items/coal.json"),
                r#"{"format_version":"1.10","minecraft:item":{"description":{"identifier":"minecraft:coal"},"components":{}}}"#,
            )
            .unwrap();
        }
        dir
    }

    #[test]
    fn dir_pack_loads_like_zip_with_inner_pack_dir() {
        let dir = make_mini_pack_dir("basic", true);
        let mut source = PackSource::from_dir(dir.clone());
        // Unified helper sees the same layout as the zip form.
        assert!(source.file_size("manifest.json").unwrap_or(0) > 0);
        assert!(source.read_file("missing.json").is_none());
        let mut pack = ZippedVersionPack::get_version(&mut source).expect("目录包应可加载");
        assert_eq!(pack.manifest.name, "MiniDirTest");
        let mut behavior = pack.take_behavior_packs();
        assert_eq!(behavior.len(), 1, "inner pack dir loads");
        let (_, resource) = behavior.pop().unwrap();
        assert!(resource.items.contains_key("minecraft:coal"));
        let budgets = crate::recipe_source::RecipeSourceBudgets::default();
        let (files, errors) =
            crate::recipe_source::read_recipes_from_resource_pack(&resource, 0, &budgets);
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, "recipes/r.json");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dir_pack_without_manifest_is_skipped() {
        let dir = std::env::temp_dir().join(format!(
            "sc_pack_dir_test_nomanifest_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut source = PackSource::from_dir(dir.clone());
        assert!(ZippedVersionPack::get_version(&mut source).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn version_scan_discovers_pack_dirs_beside_zips() {
        let root = std::env::temp_dir().join(format!("sc_pack_scan_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // A directory pack (moved into place), a junk dir, and a dot-dir.
        let pack_dir = make_mini_pack_dir("scan", false);
        std::fs::rename(&pack_dir, root.join("mypack")).unwrap();
        std::fs::create_dir_all(root.join("junk")).unwrap();
        std::fs::create_dir_all(root.join(".scdb")).unwrap();
        let loader = VersionPackLoader::new(&root);
        let versions = loader.get_versions();
        assert_eq!(versions.len(), 1, "exactly the manifest dir loads");
        assert_eq!(versions[0].manifest.name, "MiniDirTest");
        std::fs::remove_dir_all(&root).ok();
    }

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
                "directory": "blocks",
                "network_id_mode": "hashed"
            });
        }
        // Block files live inside a nested behavior pack (`blocks/**/*.json`).
        let inner_entries: Vec<(String, Vec<u8>)> = block_files
            .iter()
            .map(|(n, c)| (n.to_string(), c.as_bytes().to_vec()))
            .collect();
        let behavior_pack_bytes = {
            let mut inner = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            for (name, bytes) in inner_entries {
                inner.start_file(name, options).unwrap();
                inner.write_all(&bytes).unwrap();
            }
            inner.finish().unwrap().into_inner()
        };
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
        .chain(std::iter::once((
            "behavior_packs/inner.zip".to_string(),
            behavior_pack_bytes,
        )))
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
        let mut source = PackSource::from_bytes(bytes).expect("pack source 打开失败");
        let mut pack = ZippedVersionPack::get_version(&mut source).expect("旧包应可加载");
        assert!(!pack.has_declared_block_json());
        assert!(pack.take_block_json_bundle().is_none());
    }

    /// Declared packs load the bundle from nested behavior packs; zero or broken files fail the whole pack (no fallback).
    #[test]
    fn declared_pack_loads_bundle_and_fails_loud_without_fallback() {
        let bytes = make_mini_pack(&[("blocks/minecraft/air.json", MINI_AIR)], true);
        let mut source = PackSource::from_bytes(bytes).expect("pack source 打开失败");
        let mut pack = ZippedVersionPack::get_version(&mut source).expect("声明包应可加载");
        assert!(pack.has_declared_block_json());
        let bundle = pack.take_block_json_bundle().expect("bundle 应存在");
        assert_eq!(bundle.files.len(), 1);
        assert_eq!(bundle.files[0].identifier, "minecraft:air");
        assert_eq!(bundle.network_id_mode, "hashed");

        // Declared but zero files: fail the whole pack.
        let bytes = make_mini_pack(&[], true);
        let mut source = PackSource::from_bytes(bytes).expect("pack source 打开失败");
        ZippedVersionPack::get_version(&mut source).expect_err("零文件必须整包失败");

        // Declared but broken file: fail the whole pack (path-attributed error, no legacy fallback).
        let bytes = make_mini_pack(&[("blocks/minecraft/air.json", "{oops")], true);
        let mut source = PackSource::from_bytes(bytes).expect("pack source 打开失败");
        let e = ZippedVersionPack::get_version(&mut source).expect_err("坏文件必须整包失败");
        assert!(e.to_string().contains("air.json"), "实际：{e}");
    }

    /// Real version-pack integration test: definitions/block_tags.json must load once injected.
    /// Tag table from upstream gamedata (tag to block-identifier list, 39 tags).
    #[test]
    fn load_block_tags_from_real_version_pack() {
        let Some(mut source) = crate::test_pack::open_test_pack() else {
            eprintln!("[skip] 版本包缺失，跳过测试");
            return;
        };
        let mut pack = ZippedVersionPack::get_version(&mut source).expect("版本包解析失败");

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
        let Some(mut source) = crate::test_pack::open_test_pack() else {
            eprintln!("[skip] 版本包缺失，跳过测试");
            return;
        };
        let mut pack = ZippedVersionPack::get_version(&mut source).expect("版本包解析失败");

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
        let Some(mut source) = crate::test_pack::open_test_pack() else {
            eprintln!("[skip] 版本包缺失，跳过测试");
            return;
        };

        // Biome json count is 88 (full official 1.26.40.05 set).
        let mut biome_count = 0;
        for name in source.file_names() {
            if name.starts_with("definitions/biomes/") && name.ends_with(".biome.json") {
                biome_count += 1;
            }
        }
        assert_eq!(biome_count, 88, "官方 1.26.40.05 biome json 应为 88 个");

        let mut pack = ZippedVersionPack::get_version(&mut source).expect("版本包解析失败");
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
        let Some(mut source) = crate::test_pack::open_test_pack() else {
            eprintln!("[skip] 版本包缺失，跳过测试");
            return;
        };
        let mut pack = ZippedVersionPack::get_version(&mut source).expect("版本包解析失败");

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
