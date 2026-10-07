use crate::pack::ResourcePack;
use crate::pack_loader::pack::zipped::ZippedResourcePack;
use crate::pack_loader::PackLoaderTrait;
use std::fs;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zip::ZipArchive;
use sc_log::t_log;

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
        let mut resource_packs = vec![];
        let read_dir = match fs::read_dir(&self.dir_path) {
            Ok(read_dir) => read_dir,
            Err(error) => {
                log::warn!(
                    "{}",
                    t_log!("console.pack.dir_unreadable", dir = self.dir_path.display(), error = error)
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
            if !metadata.is_file() || (extension != "zip" && extension != "mcpack") {
                continue;
            }
            let mut file = match File::open(&path) {
                Ok(file) => file,
                Err(error) => {
                    log::warn!(
                        "{}",
                        t_log!("console.pack.res_open_fail", path = path.display(), error = error)
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
            let shared_bytes = Arc::new(bytes);
            let mut zip = match ZipArchive::new(Cursor::new(shared_bytes.as_slice())) {
                Ok(zip) => zip,
                Err(error) => {
                    log::warn!(
                        "skipping invalid resource pack {}: {}",
                        path.display(),
                        error
                    );
                    continue;
                }
            };
            match ZippedResourcePack::get_resource_pack_shared(&mut zip, Arc::clone(&shared_bytes))
            {
                Some(resource_pack) => resource_packs.push(resource_pack),
                None => log::warn!("{}", t_log!("console.pack.res_invalid", path = path.display())),
            }
        }
        resource_packs
    }
}
