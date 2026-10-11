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

    /// 被扫描的世界根目录（等待提示显示用）。
    pub fn dir(&self) -> &Path {
        &self.dir_path
    }

    /// Lightweight directory scan: `(dir, display name)` per world directory
    /// without reading `level.dat` or opening any LevelDB.
    ///
    /// The startup wait loop polls this every second; a full
    /// [`WorldLoaderTrait::get_worlds`] (which opens every world's database)
    /// runs exactly once, after the wanted name appears.
    pub fn peek_names(&self) -> Vec<(PathBuf, String)> {
        let mut names = vec![];
        if let Ok(read_dir) = fs::read_dir(&self.dir_path) {
            for entry in read_dir {
                if let Ok(entry) = entry {
                    if entry.metadata().is_ok_and(|m| m.is_dir()) {
                        let dir_path = entry.path().as_path().to_path_buf();
                        names.push((dir_path.clone(), DirectoryWorld::peek_name(&dir_path)));
                    }
                }
            }
        }
        names
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

#[cfg(test)]
mod tests {
    use super::WorldDirectoryLoader;
    use std::fs;
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("sc-world-peek-{name}"));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn peek_names_reads_display_names_without_opening_databases() {
        let root = TempDir::new("names");
        let named = root.0.join("world-a");
        let unnamed = root.0.join("world-b");
        fs::create_dir_all(&named).expect("world dir");
        fs::create_dir_all(&unnamed).expect("world dir");
        fs::write(named.join("levelname.txt"), "我的世界").expect("levelname");
        fs::write(root.0.join("not-a-world.txt"), b"x").expect("plain file");

        let loader = WorldDirectoryLoader::new(&root.0);
        let mut peeked = loader.peek_names();
        peeked.sort();
        assert_eq!(
            peeked,
            vec![
                (named.clone(), "我的世界".to_string()),
                (unnamed.clone(), "SculkBedrock Level".to_string()),
            ]
        );

        // Peeking must not create any LevelDB state (no open attempted).
        for dir in [&named, &unnamed] {
            assert!(!dir.join("LOCK").exists(), "peek must not lock");
            assert!(!dir.join("CURRENT").exists(), "peek must not create");
        }
    }
}
