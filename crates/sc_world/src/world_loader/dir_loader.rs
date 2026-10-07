use crate::world::MinecraftWorld;
use crate::world_loader::worlds::dir::DirectoryWorld;
use crate::world_loader::WorldLoaderTrait;
use std::fs;
use std::path::{Path, PathBuf};

pub struct WorldDirectoryLoader {
    dir_path: PathBuf,
}

impl WorldDirectoryLoader {
    pub fn new<P: AsRef<Path>>(dir_path: P) -> Self {
        Self {
            dir_path: dir_path.as_ref().to_path_buf(),
        }
    }
}

impl WorldLoaderTrait for WorldDirectoryLoader {
    fn get_worlds(&self) -> Vec<MinecraftWorld> {
        let mut worlds = vec![];
        if let Ok(read_dir) = fs::read_dir(&self.dir_path) {
            for entry in read_dir {
                if let Ok(entry) = entry {
                    if let Ok(metadata) = entry.metadata() {
                        if metadata.is_dir() {
                            let dir_path = entry.path().as_path().to_path_buf();
                            if let Ok(world) = DirectoryWorld::get_world(dir_path) {
                                worlds.push(world);
                            }
                        }
                    }
                }
            }
        }
        worlds
    }
}
