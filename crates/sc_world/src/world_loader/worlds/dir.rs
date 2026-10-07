use crate::data_reader::{WorldDataReader, WorldDataReaderError};
use crate::leveldb::LevelDbWorldStorage;
use crate::manager::MinecraftWorldId;
use crate::storage::{EmptyWorldStorage, WorldChunkProvider, WorldStorage};
use crate::world::MinecraftWorld;
use log::{info, warn};
use sc_log::t_log;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// Whether a `db` directory already holds LevelDB state.
///
/// LevelDB's own artifacts are `CURRENT`, `MANIFEST-*` and `*.log`; `LOCK` and
/// `LOG` are created by a failed/empty open attempt and carry no save data.
fn directory_has_database_state(db_path: &std::path::Path) -> bool {
    let Ok(entries) = fs::read_dir(db_path) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.starts_with("CURRENT")
            || name.starts_with("MANIFEST")
            || name.ends_with(".log")
            || name.ends_with(".ldb")
            || name.ends_with(".sst")
    })
}

pub struct DirectoryWorld;

impl DirectoryWorld {
    pub fn get_world(path: PathBuf) -> Result<MinecraftWorld, WorldDataReaderError> {
        //world name
        let mut level_name_path = path.clone();
        level_name_path.push("levelname.txt");
        let world_name = fs::read_to_string(level_name_path)
            .unwrap_or_else(|_| "SculkBedrock Level".to_string());

        //world data
        let mut level_dat_path = path.clone();
        level_dat_path.push("level.dat");
        let world_data = WorldDataReader::new(level_dat_path)?;

        // Storage selection:
        // - no `db` directory at all -> intentionally unsaved world;
        // - `db` present but without LevelDB state -> a freshly prepared world
        //   directory; create the database so the world is actually persisted;
        // - `db` with existing state -> open strictly, never create, so a wrong
        //   world path cannot silently produce an empty save.
        let mut db_path = path.clone();
        db_path.push("db");
        let storage: Arc<dyn WorldStorage> = if db_path.is_dir() {
            let create = !directory_has_database_state(&db_path);
            let opened = if create {
                info!("{}", t_log!("console.world.db_create", name = world_name));
                LevelDbWorldStorage::open_or_create(db_path)
            } else {
                LevelDbWorldStorage::open(db_path)
            };
            match opened {
                Ok(storage) => {
                    info!("{}", t_log!("console.world.db_opened", name = world_name));
                    Arc::new(storage)
                }
                Err(error) => {
                    warn!(
                        "{}",
                        t_log!(
                            "console.world.db_open_fail",
                            name = world_name,
                            error = error
                        )
                    );
                    return Err(WorldDataReaderError::IOError(std::io::Error::other(
                        format!("cannot open world {world_name} database: {error}"),
                    )));
                }
            }
        } else {
            info!("{}", t_log!("console.world.db_no_dir", name = world_name));
            Arc::new(EmptyWorldStorage)
        };
        let chunk_provider = WorldChunkProvider::new(storage);

        Ok(MinecraftWorld {
            world_id: MinecraftWorldId::random(),
            world_name,
            world_path: path,
            world_data,
            chunk_provider,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::directory_has_database_state;
    use std::fs;
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("sc-world-db-{name}"));
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
    fn an_absent_or_empty_directory_has_no_database_state() {
        let directory = TempDir::new("empty");
        assert!(!directory_has_database_state(&directory.0));

        // LOCK/LOG appear after a failed open attempt and carry no save data.
        fs::write(directory.0.join("LOCK"), b"").expect("write LOCK");
        fs::write(directory.0.join("LOG"), b"").expect("write LOG");
        assert!(!directory_has_database_state(&directory.0));

        assert!(!directory_has_database_state(&directory.0.join("missing")));
    }

    #[test]
    fn any_leveldb_artifact_counts_as_existing_state() {
        let directory = TempDir::new("existing");
        for artifact in ["CURRENT", "MANIFEST-000001", "000005.log"] {
            fs::write(directory.0.join(artifact), b"x").expect("write artifact");
            assert!(
                directory_has_database_state(&directory.0),
                "{artifact} must be recognized as database state"
            );
            fs::remove_file(directory.0.join(artifact)).expect("remove artifact");
            assert!(!directory_has_database_state(&directory.0));
        }
    }
}
