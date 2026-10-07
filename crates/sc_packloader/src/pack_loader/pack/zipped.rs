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

pub struct ZippedResourcePack;

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

pub(crate) fn for_each_dir_file<T: Read + Seek, F: FnMut(String, String)>(
    zip: &mut ZipArchive<T>,
    dir_path: &str,
    mut callback: F,
) {
    let dir_path = dir_path.to_ascii_lowercase();
    let mut total_bytes = 0u64;
    for i in 0..zip.len() {
        if let Ok(mut file) = zip.by_index(i) {
            if !file.is_file() {
                continue;
            }
            let file_name = file.name().to_ascii_lowercase();
            if !file_name.starts_with(&dir_path) || !file_name[dir_path.len()..].starts_with('/') {
                continue;
            }
            if file.size() > MAX_JSON_ENTRY_BYTES {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.pack.json_too_big",
                        file = file.name(),
                        max = MAX_JSON_ENTRY_BYTES
                    )
                );
                continue;
            }
            if total_bytes.saturating_add(file.size()) > MAX_JSON_DIRECTORY_BYTES {
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
            total_bytes = total_bytes.saturating_add(file.size());
            let file_name = file.name().to_string();
            let mut content = String::new();
            match file
                .take(MAX_JSON_ENTRY_BYTES + 1)
                .read_to_string(&mut content)
            {
                Ok(_) => callback(file_name, content),
                Err(error) => log::warn!(
                    "{}",
                    t_log!(
                        "console.pack.entry_utf8",
                        file = file_name,
                        error = error
                    )
                ),
            }
        }
    }
}

pub(crate) fn get_dir_files_bytes<T: Read + Seek>(
    zip: &mut ZipArchive<T>,
    dir_path: &str,
) -> Vec<(String, Vec<u8>)> {
    let mut vec = Vec::new();
    let mut total_bytes = 0u64;
    let dir_path = dir_path.to_ascii_lowercase();
    for i in 0..zip.len() {
        if let Ok(mut file) = zip.by_index(i) {
            if file.is_file() {
                let file_name = file.name().to_ascii_lowercase();
                if file_name.starts_with(&dir_path) && file_name[dir_path.len()..].starts_with('/')
                {
                    if file.size() > MAX_BINARY_ENTRY_BYTES {
                        log::warn!(
                            "{}",
                            t_log!(
                                "console.pack.entry_too_big",
                                file = file.name(),
                                max = MAX_BINARY_ENTRY_BYTES
                            )
                        );
                        continue;
                    }
                    if total_bytes.saturating_add(file.size()) > MAX_BINARY_DIRECTORY_BYTES {
                        log::warn!(
                            "{}",
                            t_log!(
                                "console.pack.bin_dir_too_big",
                                dir = dir_path,
                                max = MAX_BINARY_DIRECTORY_BYTES
                            )
                        );
                        continue;
                    }
                    total_bytes = total_bytes.saturating_add(file.size());
                    let file_name = file.name().to_string();
                    let mut content =
                        Vec::with_capacity(file.size().min(usize::MAX as u64) as usize);
                    match file
                        .take(MAX_BINARY_ENTRY_BYTES + 1)
                        .read_to_end(&mut content)
                    {
                        Ok(_) => vec.push((file_name, content)),
                        Err(error) => {
                            log::warn!(
                                "{}",
                                t_log!(
                                    "console.pack.entry_read_fail",
                                    file = file_name,
                                    error = error
                                )
                            );
                        }
                    }
                }
            }
        }
    }
    vec
}

impl ZippedResourcePack {
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

    fn get_entities<T: Read + Seek>(
        zip: &mut ZipArchive<T>,
    ) -> HashMap<String, MinecraftEntitySpawner> {
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
        for_each_dir_file(zip, "entities", &mut parse_file);
        for_each_dir_file(zip, "entity", &mut parse_file);
        map
    }

    fn get_items<T: Read + Seek>(zip: &mut ZipArchive<T>) -> HashMap<String, MinecraftItemSpawner> {
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
        for_each_dir_file(zip, "items", &mut parse_file);
        for_each_dir_file(zip, "item", &mut parse_file);
        map
    }

    pub fn get_resource_pack<T: Read + Seek>(
        zip: &mut ZipArchive<T>,
        file_bytes: Vec<u8>,
    ) -> Option<ResourcePack> {
        Self::get_resource_pack_shared(zip, Arc::new(file_bytes))
    }

    pub fn get_resource_pack_shared<T: Read + Seek>(
        zip: &mut ZipArchive<T>,
        file_bytes: Arc<Vec<u8>>,
    ) -> Option<ResourcePack> {
        //manifest
        let encryption_key = String::new();
        let (pack_type, mut manifest) = {
            let mut manifest_file = get_file_by_name(zip, "manifest.json").ok()?;
            if !manifest_file.is_file() {
                return None;
            }
            let mut str = String::new();
            manifest_file.read_to_string(&mut str).ok()?;
            let manifest_json: ResourcePackManifestJson =
                serde_json::from_reader(BudgetReader::new(StripComments::new(str.as_bytes())))
                    .ok()?;
            let pack_type = manifest_json.get_pack_type();
            let manifest = manifest_json.to_manifest().ok()?;
            (pack_type, manifest)
        };
        manifest.information.encryption_key = encryption_key;
        manifest.information.pack_len = file_bytes.len();
        manifest.information.sha256 = sha256_hash(&file_bytes);
        //entities
        let entities = Self::get_entities(zip);
        //items
        let items = Self::get_items(zip);
        Some(ResourcePack::new_shared(
            manifest, pack_type, file_bytes, entities, items,
        ))
    }
}
