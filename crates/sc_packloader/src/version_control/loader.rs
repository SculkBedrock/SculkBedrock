//! 版本包加载器：扫描版本包目录并解析为 [`SCVersionPack`]。
//!
//! Version pack loader: scans the version pack directory and parses it into
//! an [`SCVersionPack`].

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

/// 判断 JSON 条目大小是否在预算之内。
///
/// Whether the size of a JSON entry is within budget.
///
/// - `size`: 条目字节数
fn json_entry_is_allowed(size: u64) -> bool {
    size <= MAX_JSON_ENTRY_BYTES
}

/// 判断二进制条目大小是否在预算之内。
///
/// Whether the size of a binary entry is within budget.
///
/// - `size`: 条目字节数
fn binary_entry_is_allowed(size: u64) -> bool {
    size <= MAX_BINARY_ENTRY_BYTES
}

/// 版本包加载错误类型。
///
/// Version pack loader error type.
#[derive(Debug, ScError)]
pub enum VersionPackLoaderError {
    /// 压缩包条目读取失败。
    ///
    /// Zip entry failure.
    #[sc_error(code = "packloader.version.zip")]
    ZipError {
        /// 条目路径。
        ///
        /// Entry path.
        path: String,
        /// 底层压缩包错误。
        ///
        /// Underlying zip error.
        source: zip::result::ZipError,
        /// 说明错误原因的英文详情。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 错误构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    /// JSON 解析失败。
    ///
    /// JSON parse failure.
    #[sc_error(code = "packloader.version.json")]
    JsonError {
        /// 条目路径。
        ///
        /// Entry path.
        path: String,
        /// 底层 JSON 错误。
        ///
        /// Underlying JSON error.
        source: serde_json::Error,
        /// 说明错误原因的英文详情。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 错误构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    /// 文件读写失败。
    ///
    /// File I/O failure.
    #[sc_error(code = "packloader.version.io")]
    IoError {
        /// 条目路径。
        ///
        /// Entry path.
        path: String,
        /// 底层 I/O 错误。
        ///
        /// Underlying I/O error.
        source: std::io::Error,
        /// 说明错误原因的英文详情。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 错误构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    /// 必需文件缺失。
    ///
    /// Required file is missing.
    #[sc_error(code = "packloader.version.not_found")]
    FileNotFound {
        /// 缺失的条目路径。
        ///
        /// Missing entry path.
        path: &'static str,
        /// 说明错误原因的英文详情。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 错误构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    /// 条目内容未通过校验。
    ///
    /// Entry contents fail validation.
    #[sc_error(code = "packloader.version.deserialization")]
    DeserializationError {
        /// 说明错误原因的英文详情。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 错误构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    //LoadPluginsError(SCPluginLoaderError),
}

impl VersionPackLoaderError {
    /// 构造压缩包错误，并捕获调用位置。
    ///
    /// Builds a zip error, capturing the call site.
    ///
    /// - `path`: 条目路径
    /// - `source`: 底层压缩包错误
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

    /// 构造 JSON 错误，并捕获调用位置。
    ///
    /// Builds a JSON error, capturing the call site.
    ///
    /// - `path`: 条目路径
    /// - `source`: 底层 JSON 错误
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

    /// 构造 I/O 错误，并捕获调用位置。
    ///
    /// Builds an I/O error, capturing the call site.
    ///
    /// - `path`: 条目路径
    /// - `source`: 底层 I/O 错误
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

    /// 构造文件缺失错误，并捕获调用位置。
    ///
    /// Builds a missing-file error, capturing the call site.
    ///
    /// - `path`: 缺失的条目路径
    #[track_caller]
    pub fn file_not_found(path: &'static str) -> Self {
        Self::FileNotFound {
            path,
            detail: format!("required file '{path}' not found"),
            site: capture_site(),
        }
    }

    /// 构造校验错误，并捕获调用位置。
    ///
    /// Builds a validation error, capturing the call site.
    ///
    /// - `detail`: 说明错误原因的英文详情
    #[track_caller]
    pub fn deserialization_error(detail: String) -> Self {
        Self::DeserializationError {
            detail,
            site: capture_site(),
        }
    }
}

impl std::fmt::Display for VersionPackLoaderError {
    /// 以 `[错误码] 详情 (at 位置)` 的格式格式化错误。
    ///
    /// Formats the error as `[code] detail (at site)`.
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
    /// 返回底层错误源（若有）。
    ///
    /// Returns the underlying error source, if any.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ZipError { source, .. } => Some(source),
            Self::JsonError { source, .. } => Some(source),
            Self::IoError { source, .. } => Some(source),
            _ => None,
        }
    }
}


/// 版本包加载器：扫描版本包目录。
///
/// Version pack loader: scans the version pack directory.
pub struct VersionPackLoader {
    /// 版本包所在目录。
    ///
    /// Directory holding the version packs.
    dir_path: PathBuf,
}

impl VersionPackLoader {
    /// 以指定目录构造加载器。
    ///
    /// Builds a loader for the given directory.
    ///
    /// - `dir_path`: 版本包所在目录
    pub fn new<P: AsRef<Path>>(dir_path: P) -> Self {
        Self {
            dir_path: dir_path.as_ref().to_path_buf(),
        }
    }

    /// 扫描目录中的全部版本包并解析。
    ///
    /// Scans the directory for all version packs and parses them.
    ///
    /// 无效或损坏的版本包会被跳过并记录告警，不会中断整体扫描。
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

/// 压缩版本包解析器：从包源读取各段数据。
///
/// Zipped version pack parser: reads every section from a pack source.
pub struct ZippedVersionPack;

impl ZippedVersionPack {
    /// 读取并解析 `manifest.json`。
    ///
    /// Reads and parses `manifest.json`.
    ///
    /// - `pack`: 包源
    ///
    /// # Errors
    /// 文件缺失、超出 JSON 预算或解析失败时返回错误。
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

    /// 读取并解析 `definitions/runtime.json`（运行时 id 映射表）。
    ///
    /// Reads and parses `definitions/runtime.json` (the runtime id mapping
    /// table).
    ///
    /// - `pack`: 包源
    ///
    /// # Errors
    /// 文件缺失、超出 JSON 预算或解析失败时返回错误。
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

    /// 将每个内嵌行为包打开为独立来源（先目录包，再按归档顺序的压缩包；
    /// 对同一版本包是确定的）。公开以便外部消费者（如 `sc_block` 的测试）
    /// 把同样的来源交给方块数据包加载器。
    ///
    /// Opens every nested behavior pack as its own source (directory packs
    /// first, then zip packs in archive order; deterministic for a given
    /// version pack). Public so external consumers (e.g. `sc_block` tests)
    /// can feed the same sources to the block bundle loader.
    ///
    /// - `pack`: 版本包来源
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

    /// 读取并解析 `definitions/biomes/` 下的全部生物群系定义。
    ///
    /// Reads and parses every biome definition under `definitions/biomes/`.
    ///
    /// - `pack`: 包源
    ///
    /// # Errors
    /// 首个解析失败即返回错误。
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

    /// 读取 `definitions/biome_ids.json`（生物群系标识名 → 网络 id，可选）。
    ///
    /// Reads `definitions/biome_ids.json` (biome identifier → network id,
    /// optional).
    ///
    /// - `pack`: 包源
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

    /// 读取 `definitions/block_tags.json`（可选的方块标签表）。
    /// 文件缺失、超出预算或无法解析时回退为空表。
    ///
    /// Reads `definitions/block_tags.json` (optional block tag table).
    /// Missing, over-budget, or unparsable files fall back to an empty table.
    ///
    /// - `pack`: 包源
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

    /// 读取 `definitions/recipe_groups.json`（可选的合成原料组表）。
    ///
    /// Reads `definitions/recipe_groups.json` (optional ingredient group table).
    ///
    /// 文件缺失、超出预算或无法解析时回退为空表（调用方为失败即闭合策略：
    /// 精确匹配不受影响，标签/伪名查询没有成员）。复用方块标签的预算与风格。
    ///
    /// Missing, over-budget, or unparsable files fall back to an empty table
    /// (callers are fail-closed: exact matches are unaffected, tag/pseudo-name
    /// queries have no members). Reuses the block_tags budget and style.
    ///
    /// - `pack`: 包源
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

    /// 读取 `definitions/biome_features.json`（可选的世界生成特性调度数据）。
    /// 文件缺失时返回默认值；调度语义位于世界生成。
    ///
    /// Reads `definitions/biome_features.json` (optional worldgen
    /// feature-scheduling data). Returns default when absent; scheduling
    /// semantics live in world generation.
    ///
    /// - `pack`: 包源
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

    /// 读取并解析 `definitions/attributes.json`（实体属性默认值表）。
    ///
    /// Reads and parses `definitions/attributes.json` (the default entity
    /// attribute table).
    ///
    /// - `pack`: 包源
    ///
    /// # Errors
    /// 文件缺失、超出 JSON 预算或解析失败时返回错误。
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

    /// 读取 `plugins/` 下的全部插件归档字节。
    ///
    /// Reads the bytes of every plugin archive under `plugins/`.
    ///
    /// - `pack`: 包源
    fn get_plugins(pack: &mut PackSource) -> Vec<Vec<u8>> {
        pack.dir_files_bytes("plugins")
            .into_iter()
            .map(|(_, bytes)| bytes)
            .collect()
    }

    /// 读取 `definitions/entity_identifiers.nbt`（必需的原版实体标识符 NBT）。
    /// 文件缺失或无法读取时返回 `FileNotFound`。
    ///
    /// Reads `definitions/entity_identifiers.nbt` (required vanilla
    /// entity-identifier NBT). Returns `FileNotFound` when absent or
    /// unreadable.
    ///
    /// - `pack`: 包源
    ///
    /// # Errors
    /// 文件缺失、无法读取或超出二进制预算时返回错误。
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

    /// 读取 `definitions/block_palette.nbt`（可选的方块调色板字节）。
    /// 文件缺失或超出预算时返回 `None`。
    ///
    /// Reads `definitions/block_palette.nbt` (optional block palette bytes).
    /// Returns `None` when absent or over budget.
    ///
    /// - `pack`: 包源
    fn get_block_palette(pack: &mut PackSource) -> Option<Vec<u8>> {
        let size = pack.file_size("definitions/block_palette.nbt")?;
        if !binary_entry_is_allowed(size) {
            return None;
        }
        pack.read_file("definitions/block_palette.nbt")
    }

    /// 读取 `definitions/creative_items.json`（可选的创造模式物品表）。
    /// 文件缺失或超出预算时返回 `None`。
    ///
    /// Reads `definitions/creative_items.json` (optional creative-item table).
    /// Returns `None` when absent or over budget.
    ///
    /// - `pack`: 包源
    fn get_creative_items(pack: &mut PackSource) -> Option<Vec<u8>> {
        let size = pack.file_size("definitions/creative_items.json")?;
        if !json_entry_is_allowed(size) {
            return None;
        }
        pack.read_file("definitions/creative_items.json")
    }

    /// 将 `definitions/worldgen/` 下的 density_function 与 noise JSON 读入数据包。
    ///
    /// Reads the density_function and noise JSON under `definitions/worldgen/`
    /// into a bundle.
    ///
    /// - `pack`: 包源
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

    /// 解析完整版本包：读取清单与各段数据，组装为 [`SCVersionPack`]。
    ///
    /// Parses a complete version pack: reads the manifest and every section,
    /// assembling them into an [`SCVersionPack`].
    ///
    /// - `pack`: 包源
    ///
    /// # Errors
    /// 必需段缺失、解析失败或清单声明的 `block_data` 加载失败时返回错误。
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

}
