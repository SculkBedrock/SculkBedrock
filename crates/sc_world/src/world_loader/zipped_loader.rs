use crate::world::MinecraftWorld;
use crate::world_loader::dir_loader::WorldDirectoryLoader;
use crate::world_loader::WorldLoaderTrait;
use sc_log::t_log;
use std::fs;
use std::path::{Path, PathBuf};

fn get_extension(filename: &Path) -> Option<String> {
    filename
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
}

fn get_name(filename: &Path) -> Option<String> {
    filename
        .file_stem()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

pub struct WorldZippedLoader {
    dir_path: PathBuf,
}

impl WorldZippedLoader {
    pub fn new<P: AsRef<Path>>(dir_path: P) -> Self {
        Self {
            dir_path: dir_path.as_ref().to_path_buf(),
        }
    }
}

impl WorldLoaderTrait for WorldZippedLoader {
    fn get_worlds(&self) -> Vec<MinecraftWorld> {
        if let Ok(read_dir) = fs::read_dir(&self.dir_path) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                if !metadata.is_file() {
                    continue;
                }

                let Some(extension) = get_extension(&path) else {
                    log::warn!(
                        "{}",
                        t_log!("console.world.archive_ext", path = format!("{path:?}"))
                    );
                    continue;
                };
                if extension != "zip" && extension != "mcworld" {
                    continue;
                }
                let Some(file_name) = get_name(&path) else {
                    log::warn!(
                        "{}",
                        t_log!("console.world.archive_name", path = format!("{path:?}"))
                    );
                    continue;
                };

                let Ok(mut file) = fs::File::open(&path) else {
                    log::warn!(
                        "{}",
                        t_log!("console.world.archive_open", path = format!("{path:?}"))
                    );
                    continue;
                };
                let Ok(mut zip) = zip::ZipArchive::new(&mut file) else {
                    log::warn!(
                        "{}",
                        t_log!("console.world.archive_invalid", path = format!("{path:?}"))
                    );
                    continue;
                };

                let mut output_path = self.dir_path.clone();
                output_path.push(file_name);
                if output_path.exists() {
                    continue;
                }
                if let Err(error) = zip.extract(&output_path) {
                    log::warn!(
                        "{}",
                        t_log!(
                            "console.world.archive_extract",
                            path = format!("{path:?}"),
                            error = error
                        )
                    );
                }
            }

            let dir_loader = WorldDirectoryLoader::new(self.dir_path.clone());
            dir_loader.get_worlds()
        } else {
            vec![]
        }
    }
}
