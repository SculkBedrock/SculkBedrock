//! 压缩/目录包来源：zip 归档与文件系统目录的统一访问，以及资源包解析。
//!
//! Zipped/directory pack sources: unified access to zip archives and
//! filesystem directories, plus resource-pack parsing.

use crate::entity::MinecraftEntitySpawner;
use crate::item::MinecraftItemSpawner;
use crate::pack::manifest::{ResourcePackManifest, ResourcePackManifestJson};
use crate::pack::ResourcePack;
use crate::version_control::json_budget::BudgetReader;
use crate::{MinecraftJson, MinecraftJsonDeserializer};
use json_comments::StripComments;
use log::info;
use sc_log::t_log;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Error, Read, Seek};
use std::sync::Arc;
use zip::read::ZipFile;
use zip::result::{ZipError, ZipResult};
use zip::ZipArchive;

/// 压缩资源包解析器（仅关联函数，无状态）。
///
/// Zipped resource pack parser (associated functions only, stateless).
pub struct ZippedResourcePack;

/// 带切片视图的共享 zip 字节：满足 `Cursor` 的 `AsRef<[u8]>` 约束
/// 且不复制（裸 `Arc<Vec<u8>>` 不满足该约束）。
///
/// Shared zip bytes with a slice view: satisfies `Cursor`'s
/// `AsRef<[u8]>` bound without copying (plain `Arc<Vec<u8>>` lacks it).
#[derive(Clone)]
pub struct SharedBytes(Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedBytes {
    /// 以字节切片形式借用内部数据。
    ///
    /// Borrows the inner data as a byte slice.
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// 统一的包文件访问：zip 归档或文件系统目录。
/// 所有包解析都经过此帮手，因此两种形式共享布局规则、预算与告警。
/// zip 条目保持归档顺序；目录遍历排序（确定性）。目录读取跳过
/// 点文件、符号链接与非 UTF-8 名称。
///
/// Unified pack-file access: a zip archive or a filesystem directory.
/// Every pack parser goes through this helper, so both forms share layout
/// rules, budgets and warnings. Zip entries keep archive order; directory
/// walks are sorted (deterministic). Dotfiles, symlinks and non-UTF8 names
/// are skipped on directory reads.
pub enum PackSource {
    Zip {
        bytes: Arc<Vec<u8>>,
        archive: ZipArchive<std::io::Cursor<SharedBytes>>,
    },
    Dir(std::path::PathBuf),
}

impl Clone for PackSource {
    /// 克隆来源：zip 变体重建归档（字节已校验过），目录变体克隆路径。
    ///
    /// Clones the source: the zip variant rebuilds the archive (the bytes
    /// were already validated), the dir variant clones the path.
    fn clone(&self) -> Self {
        match self {
            Self::Zip { bytes, .. } => {
                let archive = ZipArchive::new(std::io::Cursor::new(SharedBytes(Arc::clone(bytes))))
                    .expect("cloned pack bytes were already validated");
                Self::Zip {
                    bytes: Arc::clone(bytes),
                    archive,
                }
            }
            Self::Dir(dir) => Self::Dir(dir.clone()),
        }
    }
}

impl std::fmt::Debug for PackSource {
    /// 简要输出（zip 打印字节数，目录打印路径）。
    ///
    /// Brief output (zip prints the byte count, dir prints the path).
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Zip { bytes, .. } => formatter.debug_tuple("Zip").field(&bytes.len()).finish(),
            Self::Dir(dir) => formatter.debug_tuple("Dir").field(dir).finish(),
        }
    }
}

impl PackSource {
    /// 从拥有所有权的字节构造 zip 来源；归档非法时返回错误串。
    ///
    /// Builds a zip source from owned bytes; an error string when the
    /// archive is invalid.
    ///
    /// - `bytes`: zip 文件字节
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, String> {
        Self::from_shared(Arc::new(bytes))
    }

    /// 从共享字节构造 zip 来源；归档非法时返回错误串。
    ///
    /// Builds a zip source from shared bytes; an error string when the
    /// archive is invalid.
    ///
    /// - `bytes`: 共享的 zip 文件字节
    pub fn from_shared(bytes: Arc<Vec<u8>>) -> Result<Self, String> {
        ZipArchive::new(std::io::Cursor::new(SharedBytes(Arc::clone(&bytes))))
            .map(|archive| Self::Zip { bytes, archive })
            .map_err(|error| error.to_string())
    }

    /// 以目录构造来源。
    ///
    /// Builds a source from a directory.
    ///
    /// - `dir`: 包目录路径
    pub fn from_dir(dir: std::path::PathBuf) -> Self {
        Self::Dir(dir)
    }

    /// 按精确相对路径（`/` 分隔）查询文件声明大小；用于读取前的预算预检。
    ///
    /// Declared size of one file by exact relative path (`/` separators).
    /// Used for budget pre-checks before reading.
    ///
    /// - `path`: 包内相对路径
    pub fn file_size(&mut self, path: &str) -> Option<u64> {
        match self {
            Self::Zip { archive, .. } => {
                let name = archive.file_names().find(|name| *name == path)?.to_string();
                archive.by_name(&name).ok().map(|file| file.size())
            }
            Self::Dir(dir) => dir
                .join(path)
                .metadata()
                .ok()
                .filter(|meta| meta.is_file())
                .map(|meta| meta.len()),
        }
    }

    /// 按精确相对路径读取一个文件；不存在时为 None。
    ///
    /// Read one file by exact relative path. None when absent.
    ///
    /// - `path`: 包内相对路径
    pub fn read_file(&mut self, path: &str) -> Option<Vec<u8>> {
        match self {
            Self::Zip { archive, .. } => {
                let name = archive.file_names().find(|name| *name == path)?.to_string();
                archive.by_name(&name).ok().and_then(|mut file| {
                    let mut bytes = Vec::new();
                    file.read_to_end(&mut bytes).ok().map(|_| bytes)
                })
            }
            Self::Dir(dir) => {
                let full = dir.join(path);
                full.is_file().then(|| std::fs::read(&full).ok()).flatten()
            }
        }
    }

    /// 全部文件路径（包内相对，`/` 分隔）：zip 为归档顺序，目录为排序后。
    ///
    /// All file paths (stored-relative, `/` separators). Archive order for
    /// zips, sorted for directories.
    pub fn file_names(&mut self) -> Vec<String> {
        match self {
            Self::Zip { archive, .. } => archive.file_names().map(str::to_string).collect(),
            Self::Dir(dir) => {
                let mut out = Vec::new();
                Self::collect_files(dir, dir, &mut out);
                out.sort();
                out
            }
        }
    }

    /// 递归收集目录下全部文件（跳过点文件/符号链接/非 UTF-8 名称）。
    ///
    /// Recursively collects all files under a directory (skipping dotfiles,
    /// symlinks, and non-UTF8 names).
    ///
    /// - `root`: 根目录（用于计算相对路径）
    /// - `current`: 当前遍历目录
    /// - `out`: 输出相对路径列表
    fn collect_files(root: &std::path::Path, current: &std::path::Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(|entry| entry.ok()).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                Self::collect_files(root, &path, out);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let mut name = String::new();
            for (index, part) in relative.components().enumerate() {
                if index > 0 {
                    name.push('/');
                }
                let Some(text) = part.as_os_str().to_str() else {
                    name.clear();
                    break;
                };
                name.push_str(text);
            }
            if !name.is_empty() {
                out.push(name);
            }
        }
    }

    /// 判断文件名是否位于 `dir_path/` 之下（整段前缀匹配）。
    ///
    /// Checks whether a file name is under `dir_path/` (whole-segment prefix).
    ///
    /// - `file_name`: 文件名
    /// - `dir_path`: 目录前缀
    fn matches_dir(file_name: &str, dir_path: &str) -> bool {
        file_name.starts_with(dir_path) && file_name[dir_path.len()..].starts_with('/')
    }

    /// 将嵌套条目作为独立来源打开：`.zip`/`.mcpack` 文件成为 zip 来源，
    /// 清单目录成为目录来源。用于版本包内嵌套的行为包。
    ///
    /// Open a nested entry as its own source: a `.zip`/`.mcpack` file
    /// becomes a zip source, a manifest directory becomes a dir source.
    /// Used for behavior packs nested in version packs.
    ///
    /// - `path`: 嵌套条目相对路径
    pub fn sub_source(&mut self, path: &str) -> Option<PackSource> {
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".zip") || lower.ends_with(".mcpack") {
            return self
                .read_file(path)
                .and_then(|bytes| PackSource::from_bytes(bytes).ok());
        }
        match self {
            Self::Zip { .. } => None,
            Self::Dir(root) => {
                let full = root.join(path);
                full.join("manifest.json")
                    .is_file()
                    .then(|| PackSource::from_dir(full))
            }
        }
    }

    /// `dir/` 下含 `marker` 文件的直接子目录。目录包以此嵌套行为/资源包；
    /// zip 归档以文件形式携带包，故对 zip 恒为空。
    ///
    /// Immediate child directories of `dir/` containing `marker` file.
    /// Directory packs nest behavior/resource packs this way; zip
    /// archives carry packs as files, so this is always empty for them.
    ///
    /// - `dir`: 父目录相对路径
    /// - `marker`: 标记文件名
    pub fn subdirs_with(&mut self, dir: &str, marker: &str) -> Vec<String> {
        match self {
            Self::Zip { .. } => Vec::new(),
            Self::Dir(root) => {
                let base = root.join(dir);
                let Ok(entries) = std::fs::read_dir(&base) else {
                    return Vec::new();
                };
                let mut out = Vec::new();
                for entry in entries.filter_map(|entry| entry.ok()) {
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    if path.join(marker).is_file() {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            out.push(format!("{dir}/{name}"));
                        }
                    }
                }
                out.sort();
                out
            }
        }
    }

    /// 遍历 `dir/` 下的 JSON 文件（递归）：与 zip 路径相同的前缀规则、
    /// 预算与告警。
    ///
    /// Iterate JSON files under `dir/` (recursive): same prefix rule,
    /// budgets and warnings as the zip path.
    ///
    /// - `dir`: 目录相对路径
    /// - `callback`: 对每个 (文件名, 内容) 的回调
    pub fn for_each_json_file(&mut self, dir: &str, callback: &mut dyn FnMut(String, String)) {
        let dir_path = dir.to_ascii_lowercase();
        let mut total_bytes = 0u64;
        // Collect names first so zip borrows end before reads.
        let names: Vec<String> = self
            .file_names()
            .into_iter()
            .filter(|name| Self::matches_dir(&name.to_ascii_lowercase(), &dir_path))
            .collect();
        for name in names {
            let Some(size) = self.file_size(&name) else {
                continue;
            };
            if size > MAX_JSON_ENTRY_BYTES {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.pack.json_too_big",
                        file = name,
                        max = MAX_JSON_ENTRY_BYTES
                    )
                );
                continue;
            }
            if total_bytes.saturating_add(size) > MAX_JSON_DIRECTORY_BYTES {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.pack.json_dir_too_big",
                        dir = dir_path,
                        max = MAX_JSON_DIRECTORY_BYTES
                    )
                );
                continue;
            }
            total_bytes = total_bytes.saturating_add(size);
            let Some(bytes) = self.read_file(&name) else {
                continue;
            };
            if bytes.len() as u64 > MAX_JSON_ENTRY_BYTES + 1 {
                continue;
            }
            match String::from_utf8(bytes) {
                Ok(content) => callback(name, content),
                Err(error) => log::warn!(
                    "{}",
                    t_log!("console.pack.entry_utf8", file = name, error = error)
                ),
            }
        }
    }

    /// 读取 `dir/` 下全部文件（递归，套用二进制预算）。
    ///
    /// Read all files under `dir/` (recursive) with binary budgets.
    ///
    /// - `dir`: 目录相对路径
    pub fn dir_files_bytes(&mut self, dir: &str) -> Vec<(String, Vec<u8>)> {
        let dir_path = dir.to_ascii_lowercase();
        let mut vec = Vec::new();
        let mut total_bytes = 0u64;
        let names: Vec<String> = self
            .file_names()
            .into_iter()
            .filter(|name| Self::matches_dir(&name.to_ascii_lowercase(), &dir_path))
            .collect();
        for name in names {
            let Some(size) = self.file_size(&name) else {
                continue;
            };
            if size > MAX_BINARY_ENTRY_BYTES {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.pack.entry_too_big",
                        file = name,
                        max = MAX_BINARY_ENTRY_BYTES
                    )
                );
                continue;
            }
            if total_bytes.saturating_add(size) > MAX_BINARY_DIRECTORY_BYTES {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.pack.json_dir_too_big",
                        dir = dir_path,
                        max = MAX_BINARY_DIRECTORY_BYTES
                    )
                );
                continue;
            }
            total_bytes = total_bytes.saturating_add(size);
            if let Some(bytes) = self.read_file(&name) {
                vec.push((name, bytes));
            }
        }
        vec
    }
}

/// 单个 JSON 条目字节上限。
///
/// Maximum bytes for a single JSON entry.
pub(crate) const MAX_JSON_ENTRY_BYTES: u64 = 16 * 1024 * 1024;
/// 单个二进制条目字节上限。
///
/// Maximum bytes for a single binary entry.
pub(crate) const MAX_BINARY_ENTRY_BYTES: u64 = 128 * 1024 * 1024;
/// JSON 目录总字节上限。
///
/// Maximum total bytes for a JSON directory.
pub(crate) const MAX_JSON_DIRECTORY_BYTES: u64 = 256 * 1024 * 1024;
/// 二进制目录总字节上限。
///
/// Maximum total bytes for a binary directory.
pub(crate) const MAX_BINARY_DIRECTORY_BYTES: u64 = 512 * 1024 * 1024;

/// 计算数据的 SHA-256 摘要。
///
/// Computes the SHA-256 digest of the data.
///
/// - `data`: 输入字节
fn sha256_hash(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

/// 按名称在 zip 归档中查找并打开条目。
///
/// Looks up and opens a zip archive entry by name.
///
/// - `zip`: zip 归档
/// - `path`: 条目名
///
/// # Errors
/// 条目不存在时返回 zip IO 错误。
pub(crate) fn get_file_by_name<'a, T: Read + Seek>(
    zip: &'a mut ZipArchive<T>,
    path: &str,
) -> ZipResult<ZipFile<'a, T>> {
    // Look up the zip entry by name and own the file name
    let file_name = {
        zip.file_names()
            .find(|&name| name == path)
            .ok_or(ZipError::Io(Error::other("cannot get zip file")))?
            .to_string() // Own the name as a String
    };
    zip.by_name(&file_name)
}

impl ZippedResourcePack {
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    /// 从 zip 归档读取并解析 manifest.json。
    ///
    /// Reads and parses manifest.json from a zip archive.
    ///
    /// - `zip`: zip 归档
    fn get_manifest<T: Read + Seek>(zip: &mut ZipArchive<T>) -> Option<ResourcePackManifest> {
        let mut manifest = get_file_by_name(zip, "manifest.json").ok()?;
        if manifest.is_file() {
            let mut str = String::new();
            manifest.read_to_string(&mut str).ok()?;
            let manifest_json: ResourcePackManifestJson =
                serde_json::from_reader(BudgetReader::new(StripComments::new(str.as_bytes())))
                    .ok()?;
            let result = manifest_json.to_manifest().ok();
            result
        } else {
            None
        }
    }

    /// 解析包内实体定义（`entities/` 与 `entity/` 两个目录，按标识名索引）。
    ///
    /// Parses in-pack entity definitions (both `entities/` and `entity/`,
    /// keyed by identifier).
    ///
    /// - `pack`: 包来源
    fn get_entities(pack: &mut PackSource) -> HashMap<String, MinecraftEntitySpawner> {
        let mut map = HashMap::new();
        let mut parse_file = |name: String, json: String| {
            let result = serde_json::from_reader::<_, MinecraftJsonDeserializer>(
                BudgetReader::new(StripComments::new(json.as_bytes())),
            );
            if let Ok(types) = result {
                let types = types.get();
                match types {
                    MinecraftJson::MinecraftEntitySpawner(content) => {
                        let description = content.clone().description.map(|x| x.identifier.clone());
                        if let Some(description) = description {
                            map.insert(description, content);
                        }
                    }
                    _ => {}
                }
            } else if let Err(error) = result {
                let reason = error.to_string();
                info!(
                    "{}",
                    t_log!(
                        "console.resource_pack.entities.error",
                        name = name,
                        reason = reason
                    )
                )
            }
        };
        pack.for_each_json_file("entities", &mut parse_file);
        pack.for_each_json_file("entity", &mut parse_file);
        map
    }

    /// 解析包内物品定义（`items/` 与 `item/` 两个目录，按标识名索引）。
    ///
    /// Parses in-pack item definitions (both `items/` and `item/`, keyed by
    /// identifier).
    ///
    /// - `pack`: 包来源
    fn get_items(pack: &mut PackSource) -> HashMap<String, MinecraftItemSpawner> {
        let mut map = HashMap::new();
        let mut parse_file = |name: String, json: String| {
            let result = serde_json::from_reader::<_, MinecraftJsonDeserializer>(
                BudgetReader::new(StripComments::new(json.as_bytes())),
            );
            if let Ok(types) = result {
                let types = types.get();
                match types {
                    MinecraftJson::MinecraftItemSpawner(content) => {
                        let description = content.clone().description.map(|x| x.identifier.clone());
                        if let Some(description) = description {
                            map.insert(description, content);
                        }
                    }
                    _ => {}
                }
            } else if let Err(error) = result {
                let reason = error.to_string();
                info!(
                    "{}",
                    t_log!(
                        "console.resource_pack.items.error",
                        name = name,
                        reason = reason
                    )
                )
            }
        };
        pack.for_each_json_file("items", &mut parse_file);
        pack.for_each_json_file("item", &mut parse_file);
        map
    }

    /// 解析一个包（非客户端下载场景）。
    ///
    /// Parses one pack (not client-downloaded).
    ///
    /// - `pack`: 包来源
    pub fn get_resource_pack(pack: &mut PackSource) -> Option<ResourcePack> {
        Self::get_resource_pack_shared(pack, false)
    }

    /// 解析一个包。`client_download` 标记客户端从本服获取的包
    /// （仓库根 `resource_packs/` 覆盖层）：仅这些包在目录无下载字节时告警。
    /// 行为包（内嵌或覆盖层）永不被下载，故保持静默。
    ///
    /// Parse one pack. `client_download` marks packs the client fetches
    /// from this server (repo-root `resource_packs/` overlays): only those
    /// warn when a directory has no download bytes. Behavior packs
    /// (embedded or overlay) are never downloaded, so they stay silent.
    ///
    /// - `pack`: 包来源
    /// - `client_download`: 是否为客户端下载包
    pub fn get_resource_pack_shared(
        pack: &mut PackSource,
        client_download: bool,
    ) -> Option<ResourcePack> {
        use crate::pack::PackData;
        //manifest
        let encryption_key = String::new();
        let (pack_type, mut manifest) = {
            let Some(bytes) = pack.read_file("manifest.json") else {
                return None;
            };
            let Ok(text) = String::from_utf8(bytes) else {
                return None;
            };
            let manifest_json: ResourcePackManifestJson =
                serde_json::from_reader(BudgetReader::new(StripComments::new(text.as_bytes())))
                    .ok()?;
            let pack_type = manifest_json.get_pack_type();
            let manifest = manifest_json.to_manifest().ok()?;
            (pack_type, manifest)
        };
        manifest.information.encryption_key = encryption_key;
        // Directory packs carry no download bytes (author locally,
        // distribute as zip). Only client-downloaded packs warn.
        let (pack_data, source) = match pack {
            PackSource::Zip { bytes, .. } => {
                manifest.information.pack_len = bytes.len();
                manifest.information.sha256 = sha256_hash(bytes);
                (Arc::clone(bytes), PackData::Zip(Arc::clone(bytes)))
            }
            PackSource::Dir(dir) => {
                if client_download {
                    log::warn!(
                        "directory pack has no download bytes {} (distribute as zip for clients)",
                        dir.display()
                    );
                }
                manifest.information.pack_len = 0;
                manifest.information.sha256 = vec![];
                (Arc::new(Vec::new()), PackData::Dir(dir.clone()))
            }
        };
        //entities
        let entities = Self::get_entities(pack);
        //items
        let items = Self::get_items(pack);
        let mut resource =
            ResourcePack::new_shared(manifest, pack_type, pack_data, entities, items);
        resource.source = source;
        Some(resource)
    }
}
