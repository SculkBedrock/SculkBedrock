use crate::pack::ResourcePack;
use crate::pack_loader::pack::zipped::{PackSource, ZippedResourcePack};
use crate::pack_loader::PackLoaderTrait;
use sc_log::t_log;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) fn get_extension(filename: &Path) -> String {
    filename
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

pub struct ResourcePackZippedLoader {
    dir_path: PathBuf,
}

impl ResourcePackZippedLoader {
    pub fn new<P: AsRef<Path>>(dir_path: P) -> Self {
        Self {
            dir_path: dir_path.as_ref().to_path_buf(),
        }
    }
}

impl PackLoaderTrait for ResourcePackZippedLoader {
    fn get_resource_packs(&self) -> Vec<ResourcePack> {
        // Only repo-root `resource_packs/` overlays are client-downloaded;
        // `behavior_packs/` overlays stay silent about missing bytes.
        let client_download = self
            .dir_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("resource_packs"));
        let mut resource_packs = vec![];
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
                return resource_packs;
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
            // dot-dirs are never packs.
            if metadata.is_dir() {
                if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('.'))
                    || !path.join("manifest.json").is_file()
                {
                    continue;
                }
                let mut pack = PackSource::from_dir(path.clone());
                match ZippedResourcePack::get_resource_pack_shared(&mut pack, client_download) {
                    Some(resource_pack) => resource_packs.push(resource_pack),
                    None => {
                        log::warn!(
                            "{}",
                            t_log!("console.pack.res_invalid", path = path.display())
                        )
                    }
                }
                continue;
            }
            if !metadata.is_file() || (extension != "zip" && extension != "mcpack") {
                continue;
            }
            let mut file = match File::open(&path) {
                Ok(file) => file,
                Err(error) => {
                    log::warn!(
                        "{}",
                        t_log!(
                            "console.pack.res_open_fail",
                            path = path.display(),
                            error = error
                        )
                    );
                    continue;
                }
            };
            let mut bytes = Vec::new();
            if let Err(error) = file.read_to_end(&mut bytes) {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.pack.entry_read_fail",
                        path = path.display(),
                        error = error
                    )
                );
                continue;
            }
            // The file cursor is at EOF after read_to_end. Use the already-read
            // bytes as a fresh seekable archive.
            let mut pack = match PackSource::from_bytes(bytes) {
                Ok(pack) => pack,
                Err(error) => {
                    log::warn!(
                        "skipping invalid resource pack {}: {}",
                        path.display(),
                        error
                    );
                    continue;
                }
            };
            match ZippedResourcePack::get_resource_pack_shared(&mut pack, client_download) {
                Some(resource_pack) => resource_packs.push(resource_pack),
                None => log::warn!(
                    "{}",
                    t_log!("console.pack.res_invalid", path = path.display())
                ),
            }
        }
        resource_packs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_scan_loads_pack_dirs_beside_zips() {
        let root =
            std::env::temp_dir().join(format!("sc_overlay_scan_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pack = root.join("overlay1");
        std::fs::create_dir_all(pack.join("items")).unwrap();
        std::fs::write(
            pack.join("manifest.json"),
            r#"{"format_version":2,"header":{"name":"overlay1","uuid":"00000000-0000-0000-0000-000000000002","version":[1,0,0]}}"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("items/stick.json"),
            r#"{"format_version":"1.10","minecraft:item":{"description":{"identifier":"minecraft:stick"},"components":{}}}"#,
        )
        .unwrap();
        let loader = ResourcePackZippedLoader::new(&root);
        let packs = loader.get_resource_packs();
        assert_eq!(packs.len(), 1, "exactly the manifest dir loads");
        assert!(packs[0].items.contains_key("minecraft:stick"));
        // Directory packs carry no download bytes.
        assert!(packs[0].pack_data.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }
}
