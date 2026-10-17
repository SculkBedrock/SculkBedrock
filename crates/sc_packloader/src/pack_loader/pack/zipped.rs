use crate::entity::{EntityComponents, MinecraftEntitySpawner};
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

pub struct ZippedResourcePack;

/// Shared zip bytes with a slice view: satisfies `Cursor`'s
/// `AsRef<[u8]>` bound without copying (plain `Arc<Vec<u8>>` lacks it).
#[derive(Clone)]
pub struct SharedBytes(Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

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
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Zip { bytes, .. } => formatter.debug_tuple("Zip").field(&bytes.len()).finish(),
            Self::Dir(dir) => formatter.debug_tuple("Dir").field(dir).finish(),
        }
    }
}

impl PackSource {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, String> {
        Self::from_shared(Arc::new(bytes))
    }

    pub fn from_shared(bytes: Arc<Vec<u8>>) -> Result<Self, String> {
        ZipArchive::new(std::io::Cursor::new(SharedBytes(Arc::clone(&bytes))))
            .map(|archive| Self::Zip { bytes, archive })
            .map_err(|error| error.to_string())
    }

    pub fn from_dir(dir: std::path::PathBuf) -> Self {
        Self::Dir(dir)
    }

    /// Declared size of one file by exact relative path (`/` separators).
    /// Used for budget pre-checks before reading.
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

    /// Read one file by exact relative path. None when absent.
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

    fn matches_dir(file_name: &str, dir_path: &str) -> bool {
        file_name.starts_with(dir_path) && file_name[dir_path.len()..].starts_with('/')
    }

    /// Open a nested entry as its own source: a `.zip`/`.mcpack` file
    /// becomes a zip source, a manifest directory becomes a dir source.
    /// Used for behavior packs nested in version packs.
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

    /// Immediate child directories of `dir/` containing `marker` file.
    /// Directory packs nest behavior/resource packs this way; zip
    /// archives carry packs as files, so this is always empty for them.
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

    /// Iterate JSON files under `dir/` (recursive): same prefix rule,
    /// budgets and warnings as the zip path.
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

    /// Read all files under `dir/` (recursive) with binary budgets.
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

pub(crate) const MAX_JSON_ENTRY_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_BINARY_ENTRY_BYTES: u64 = 128 * 1024 * 1024;
pub(crate) const MAX_JSON_DIRECTORY_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_BINARY_DIRECTORY_BYTES: u64 = 512 * 1024 * 1024;

fn sha256_hash(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

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
    // 数据包 schema/数据模型字段，为兼容性保留
    #[allow(dead_code)]
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
        // Attach parsed loot tables: an entity `minecraft:loot` path reference
        // resolves against this behavior pack and becomes a parsed
        // `sc:loot_tables` component next to the raw reference. Unresolvable
        // references are left untouched (loading never fails on them).
        for spawner in map.values_mut() {
            let Some(EntityComponents::Map(component_map)) = spawner.components.as_mut() else {
                continue;
            };
            crate::loot_tables::attach_entity_loot_tables(
                component_map,
                &mut |path: &str| pack.read_file(path),
            );
        }
        map
    }

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

    pub fn get_resource_pack(pack: &mut PackSource) -> Option<ResourcePack> {
        Self::get_resource_pack_shared(pack, false)
    }

    /// Parse one pack. `client_download` marks packs the client fetches
    /// from this server (repo-root `resource_packs/` overlays): only those
    /// warn when a directory has no download bytes. Behavior packs
    /// (embedded or overlay) are never downloaded, so they stay silent.
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
