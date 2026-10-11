//! Test version-pack discovery (`#[test]` only, never production).
//!
//! Tests must not hardcode pack file names: this helper walks up from the
//! test working directory to `version_packs/` and auto-selects the first
//! sorted `.scver`, else `.zip`, else manifest directory. Nothing found =
//! `None`, and callers skip (same pattern as the palette tests).
//! Production keeps its own configured-dir selection (`VersionPackLoader`).

use std::path::{Path, PathBuf};

use crate::pack_loader::pack::zipped::PackSource;

fn version_packs_dir() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..6 {
        let candidate = dir.join("version_packs");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn is_pack_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".scver") || lower.ends_with(".zip")
}

fn is_pack_dir(path: &Path) -> bool {
    !path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
        && path.join("manifest.json").is_file()
}

/// Auto-selected pack opened for parsing. `None` when absent or unreadable.
#[allow(dead_code)]
pub fn open_test_pack() -> Option<PackSource> {
    let dir = version_packs_dir()?;
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in &entries {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_pack = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_pack_file);
        if !is_pack {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(source) = PackSource::from_bytes(bytes) {
                return Some(source);
            }
        }
    }
    for entry in &entries {
        let path = entry.path();
        if path.is_dir() && is_pack_dir(&path) {
            return Some(PackSource::from_dir(path));
        }
    }
    None
}

/// Read one inner file (`/` separators) from the auto-selected pack.
#[allow(dead_code)]
pub fn read_test_pack_file(inner: &str) -> Option<Vec<u8>> {
    open_test_pack().and_then(|mut source| source.read_file(inner))
}
