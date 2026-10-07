//! World loaders: `WorldLoaderTrait` plus directory/zip/world-package implementations.
//!
//! `WorldDirectoryLoader` scans LevelDB worlds from the `worlds/` directory;
//! `ZippedLoader` loads from archives.

use crate::world::MinecraftWorld;

pub mod dir_loader;
pub mod worlds;
pub mod zipped_loader;

pub trait WorldLoaderTrait {
    fn get_worlds(&self) -> Vec<MinecraftWorld>;
}
