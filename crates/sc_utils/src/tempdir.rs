use sc_ecs::resource::Resource;
use std::io;
use std::path::Path;
use tempdir::TempDir;

#[derive(Resource)]
pub struct SCTempDir {
    temp_dir: TempDir,
}

impl SCTempDir {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            temp_dir: TempDir::new("ur")?,
        })
    }

    pub fn path(&self) -> &Path {
        self.temp_dir.path()
    }
}
