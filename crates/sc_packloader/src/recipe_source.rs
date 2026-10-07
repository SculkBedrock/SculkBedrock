//! Behavior-pack recipe sources (`behavior_packs/<pack>/recipes/**/*.json`).
//!
//! The version `.scver` embeds each behavior pack as a nested zip; the
//! outer `__brarchive/recipes.brarchive` is never the sole JSON input.
//! This module reads `recipes/**/*.json` from the nested pack bytes with
//! the existing JSON budget / pathful-error style and preserves pack
//! identity, relative path, `format_version`, identifier hint and the
//! content fingerprint. The recipe *kind* is only derived from the JSON
//! root key at compile time (`sc_recipe`), never from the file name.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use crate::pack::ResourcePack;
use crate::pack_loader::pack::zipped::{
    MAX_BINARY_ENTRY_BYTES, MAX_JSON_DIRECTORY_BYTES, MAX_JSON_ENTRY_BYTES,
};
use crate::version_control::json_budget::BudgetReader;
use sc_ecs::resource::Resource;

use json_comments::StripComments;
use sha2::{Digest, Sha256};

/// Budgets for recipe source loading (all explicit, all bounded).
#[derive(Clone, Copy, Debug)]
pub struct RecipeSourceBudgets {
    /// Max recipe files accepted per behavior pack.
    pub max_files_per_pack: usize,
    /// Max total recipe bytes accepted per behavior pack.
    pub max_dir_bytes: u64,
    /// Max single recipe file bytes.
    pub max_file_bytes: u64,
}

impl Default for RecipeSourceBudgets {
    fn default() -> Self {
        Self {
            max_files_per_pack: 4096,
            max_dir_bytes: 64 * 1024 * 1024,
            // Matches the pack JSON entry budget.
            max_file_bytes: MAX_JSON_ENTRY_BYTES,
        }
    }
}

/// One raw recipe file with provenance.
#[derive(Clone, Debug)]
pub struct RecipeSourceFile {
    /// Behavior pack display name (`manifest.name`).
    pub pack_name: String,
    /// Pack load-order index (lower = loaded earlier = lower priority).
    /// Callers sort low-version → high-version and pass the index through.
    pub pack_order: usize,
    /// Path inside the behavior pack zip (e.g. `recipes/acacia_boat.json`).
    pub relative_path: String,
    pub format_version: String,
    /// `description.identifier` hint when present (the compiler re-derives
    /// the authoritative identifier from the JSON body).
    pub identifier_hint: Option<String>,
    pub raw: serde_json::Value,
    /// Raw file bytes (for directory budgets).
    pub raw_bytes: usize,
    /// sha256 of the raw file bytes.
    pub fingerprint: [u8; 32],
}

impl RecipeSourceFile {
    /// Convert to the compiler input (`sc_recipe` is an optional consumer;
    /// this avoids a hard packloader → recipe dependency).
    pub fn raw_json(&self) -> &serde_json::Value {
        &self.raw
    }
}

/// Pathful source error: pack + path + reason, never silent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecipeSourceError {
    pub pack_name: String,
    pub relative_path: String,
    pub reason: String,
}

impl std::fmt::Display for RecipeSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {}: {}",
            self.pack_name, self.relative_path, self.reason
        )
    }
}

impl std::error::Error for RecipeSourceError {}

fn fingerprint_of(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn is_recipe_json_path(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("__brarchive/") {
        return false;
    }
    if !lower.ends_with(".json") {
        return false;
    }
    // `recipes/**/*.json`: a `recipes/` segment followed by a file name.
    lower.split('/').any(|seg| seg == "recipes")
}

fn relative_recipe_path(name: &str) -> String {
    // Keep the path from the `recipes/` segment on, preserving the
    // original case for diagnostics.
    if let Some(pos) = name.to_ascii_lowercase().find("recipes/") {
        name[pos..].to_string()
    } else {
        name.to_string()
    }
}

/// Read all `recipes/**/*.json` files from one behavior-pack zip.
///
/// `pack_bytes` is the nested behavior-pack zip (`ResourcePack.pack_data`).
/// Returns the files plus per-file errors (a single bad file never aborts
/// the whole pack; directory/file budgets are still enforced).
pub fn read_recipes_from_pack_bytes(
    pack_name: &str,
    pack_order: usize,
    pack_bytes: &[u8],
    budgets: &RecipeSourceBudgets,
) -> (Vec<RecipeSourceFile>, Vec<RecipeSourceError>) {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    let mut archive = match zip::ZipArchive::new(Cursor::new(pack_bytes)) {
        Ok(archive) => archive,
        Err(error) => {
            errors.push(RecipeSourceError {
                pack_name: pack_name.to_string(),
                relative_path: String::new(),
                reason: format!("behavior pack zip could not be opened: {error}"),
            });
            return (files, errors);
        }
    };
    let mut total_bytes: u64 = 0;
    // Collect candidate names first so the borrow of `archive` ends before
    // per-file reads (avoids holding the zip borrow across parsing).
    let mut names: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        let Ok(entry) = archive.by_index(i) else {
            continue;
        };
        if !entry.is_file() {
            continue;
        }
        let name = entry.name().to_string();
        if is_recipe_json_path(&name) {
            names.push(name);
        }
    }
    names.sort();
    for name in names {
        if files.len() >= budgets.max_files_per_pack {
            errors.push(RecipeSourceError {
                pack_name: pack_name.to_string(),
                relative_path: relative_recipe_path(&name),
                reason: format!("too many recipe files (>{})", budgets.max_files_per_pack),
            });
            break;
        }
        let mut entry = match archive.by_name(&name) {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(RecipeSourceError {
                    pack_name: pack_name.to_string(),
                    relative_path: relative_recipe_path(&name),
                    reason: format!("zip entry could not be opened: {error}"),
                });
                continue;
            }
        };
        if entry.size() > budgets.max_file_bytes.min(MAX_BINARY_ENTRY_BYTES) {
            errors.push(RecipeSourceError {
                pack_name: pack_name.to_string(),
                relative_path: relative_recipe_path(&name),
                reason: format!("recipe file exceeds {} bytes", budgets.max_file_bytes),
            });
            continue;
        }
        if total_bytes.saturating_add(entry.size())
            > budgets.max_dir_bytes.min(MAX_JSON_DIRECTORY_BYTES)
        {
            errors.push(RecipeSourceError {
                pack_name: pack_name.to_string(),
                relative_path: relative_recipe_path(&name),
                reason: format!("recipe directory exceeds {} bytes", budgets.max_dir_bytes),
            });
            break;
        }
        let mut bytes = Vec::with_capacity(entry.size().min(usize::MAX as u64) as usize);
        if let Err(error) = entry
            .take(budgets.max_file_bytes + 1)
            .read_to_end(&mut bytes)
        {
            errors.push(RecipeSourceError {
                pack_name: pack_name.to_string(),
                relative_path: relative_recipe_path(&name),
                reason: format!("recipe file could not be read: {error}"),
            });
            continue;
        }
        total_bytes = total_bytes.saturating_add(bytes.len() as u64);
        let fingerprint = fingerprint_of(&bytes);
        // Budgeted JSON parse with comment stripping (packs ship comments).
        let parsed: Result<serde_json::Value, _> =
            serde_json::from_reader(BudgetReader::new(StripComments::new(bytes.as_slice())));
        let raw = match parsed {
            Ok(raw) => raw,
            Err(error) => {
                errors.push(RecipeSourceError {
                    pack_name: pack_name.to_string(),
                    relative_path: relative_recipe_path(&name),
                    reason: format!("invalid JSON: {error}"),
                });
                continue;
            }
        };
        let format_version = raw
            .get("format_version")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // Identifier hint: first body key's description.identifier.
        let mut identifier_hint = None;
        if let Some(obj) = raw.as_object() {
            for (key, body) in obj {
                if key == "format_version" {
                    continue;
                }
                if let Some(id) = body
                    .get("description")
                    .and_then(|d| d.get("identifier"))
                    .and_then(|v| v.as_str())
                {
                    identifier_hint = Some(id.to_string());
                }
                break;
            }
        }
        files.push(RecipeSourceFile {
            pack_name: pack_name.to_string(),
            pack_order,
            relative_path: relative_recipe_path(&name),
            format_version,
            identifier_hint,
            raw_bytes: bytes.len(),
            raw,
            fingerprint,
        });
    }
    (files, errors)
}

/// Read recipes from an already-parsed [`ResourcePack`] (uses its shared
/// zip bytes). The caller assigns `pack_order` from the low → high version
/// sort done in `load_version`.
pub fn read_recipes_from_resource_pack(
    pack: &ResourcePack,
    pack_order: usize,
    budgets: &RecipeSourceBudgets,
) -> (Vec<RecipeSourceFile>, Vec<RecipeSourceError>) {
    read_recipes_from_pack_bytes(&pack.manifest.name, pack_order, &pack.pack_data, budgets)
}

/// Merge ordered packs low-version → high-version: concatenates per-pack
/// files preserving pack order (override resolution happens at compile
/// time in `sc_recipe`, not here).
pub fn merge_ordered_packs(packs: &[(String, Vec<RecipeSourceFile>)]) -> Vec<OrderedRecipeSource> {
    let mut out = Vec::new();
    for (pack_name, files) in packs {
        for file in files {
            out.push(OrderedRecipeSource {
                pack_name: pack_name.clone(),
                pack_order: file.pack_order,
                file: file.clone(),
            });
        }
    }
    out.sort_by(|a, b| {
        a.pack_order
            .cmp(&b.pack_order)
            .then_with(|| a.file.relative_path.cmp(&b.file.relative_path))
    });
    out
}

/// One ordered source entry (pack order already applied).
#[derive(Clone, Debug)]
pub struct OrderedRecipeSource {
    pub pack_name: String,
    pub pack_order: usize,
    pub file: RecipeSourceFile,
}

/// Raw recipe sources kept from version-pack load (ECS resource).
///
/// `load_version` (SCPreLoad) fills this from the nested behavior-pack
/// zips; bootstrap `load_recipe_registry` (SCLoad) compiles it into the
/// frozen `sc_game::SharedRecipeRegistry`. Keeping the raw files (not just
/// the compiled snapshot) preserves pack identity/path diagnostics.
#[derive(Resource, Clone, Debug, Default)]
pub struct RawRecipeSources {
    pub files: Vec<RecipeSourceFile>,
    pub errors: Vec<RecipeSourceError>,
}

/// Tag tables available to recipe matching (item tags only).
#[derive(Clone, Debug, Default)]
pub struct ItemTagTable {
    inner: HashMap<String, Vec<String>>,
}

impl ItemTagTable {
    pub fn new(inner: HashMap<String, Vec<String>>) -> Self {
        Self { inner }
    }
    pub fn members(&self, tag: &str) -> Vec<String> {
        self.inner.get(tag).cloned().unwrap_or_default()
    }
    pub fn contains(&self, tag: &str) -> bool {
        self.inner.contains_key(tag)
    }
    pub fn len(&self) -> usize {
        self.inner.len()
    }
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;

    fn test_budgets() -> RecipeSourceBudgets {
        RecipeSourceBudgets::default()
    }

    fn make_pack_bytes(files: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in files {
            writer.start_file(*name, options).unwrap();
            use std::io::Write;
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn reads_nested_recipes_and_ignores_brarchive() {
        let bytes = make_pack_bytes(&[
            ("manifest.json", "{}"),
            (
                "recipes/a.json",
                r#"{"format_version":"1.12","minecraft:recipe_shapeless":{"description":{"identifier":"minecraft:a"},"tags":["crafting_table"],"ingredients":[{"item":"minecraft:stone"}],"result":{"item":"minecraft:out"}}}"#,
            ),
            (
                "recipes/nested/b.json",
                r#"{"format_version":"1.12","minecraft:recipe_shaped":{"description":{"identifier":"minecraft:b"},"tags":["crafting_table"],"pattern":["X"],"key":{"X":{"item":"minecraft:stone"}},"result":{"item":"minecraft:out"}}}"#,
            ),
            ("__brarchive/recipes.brarchive", "not json"),
            ("recipes/notes.txt", "ignored"),
        ]);
        let (files, errors) = read_recipes_from_pack_bytes("test", 0, &bytes, &test_budgets());
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.relative_path == "recipes/a.json"));
        assert!(files
            .iter()
            .any(|f| f.relative_path == "recipes/nested/b.json"));
        assert_eq!(files[0].identifier_hint.as_deref(), Some("minecraft:a"));
        assert!(!files[0].fingerprint.iter().all(|b| *b == 0));
    }

    #[test]
    fn bad_json_is_a_pathful_error_not_a_pack_abort() {
        let bytes = make_pack_bytes(&[
            (
                "recipes/good.json",
                r#"{"format_version":"1.12","minecraft:recipe_shapeless":{"description":{"identifier":"minecraft:g"},"tags":["crafting_table"],"ingredients":[{"item":"minecraft:stone"}],"result":{"item":"minecraft:out"}}}"#,
            ),
            ("recipes/bad.json", "{oops"),
        ]);
        let (files, errors) = read_recipes_from_pack_bytes("test", 3, &bytes, &test_budgets());
        assert_eq!(files.len(), 1);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].relative_path, "recipes/bad.json");
        assert_eq!(errors[0].pack_name, "test");
        assert_eq!(files[0].pack_order, 3);
    }

    #[test]
    fn file_count_budget_is_enforced() {
        let files: Vec<(String, String)> = (0..5)
            .map(|i| {
                (
                    format!("recipes/{i}.json"),
                    r#"{"format_version":"1.12","minecraft:recipe_shapeless":{"description":{"identifier":"minecraft:x"},"tags":["crafting_table"],"ingredients":[{"item":"minecraft:stone"}],"result":{"item":"minecraft:out"}}}"#.to_string(),
                )
            })
            .collect();
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let bytes = make_pack_bytes(&refs);
        let budgets = RecipeSourceBudgets {
            max_files_per_pack: 2,
            ..test_budgets()
        };
        let (files, errors) = read_recipes_from_pack_bytes("test", 0, &bytes, &budgets);
        assert_eq!(files.len(), 2);
        assert!(!errors.is_empty());
    }

    #[test]
    fn real_pack_stonecutter_recipes_all_load() {
        let pack_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../version_packs/[SC原版版本包] Vanilla-1.26.40.scver"
        );
        let bytes = match fs::read(pack_path) {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        let mut outer = match zip::ZipArchive::new(Cursor::new(bytes)) {
            Ok(z) => z,
            Err(_) => return,
        };
        // Find the 1.18 stonecutter pack (24 recipes) inside the version pack.
        let mut target: Option<Vec<u8>> = None;
        for i in 0..outer.len() {
            let mut entry = outer.by_index(i).unwrap();
            if !entry.is_file() {
                continue;
            }
            let name = entry.name().to_string();
            if name.ends_with("vanilla_1.18.0.zip") {
                let mut data = Vec::new();
                use std::io::Read;
                entry.read_to_end(&mut data).unwrap();
                target = Some(data);
                break;
            }
        }
        let Some(nested) = target else { return };
        let (files, errors) =
            read_recipes_from_pack_bytes("vanilla_1.18.0", 1, &nested, &test_budgets());
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(files.len(), 24, "expected 24 stonecutter recipes");
        for file in &files {
            assert!(file.relative_path.starts_with("recipes/"));
            assert!(!file.format_version.is_empty());
            assert!(file
                .identifier_hint
                .as_deref()
                .unwrap_or("")
                .starts_with("minecraft:stonecutter"));
        }
    }

    #[test]
    fn real_pack_vanilla_recipes_load_without_brarchive() {
        let pack_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../version_packs/[SC原版版本包] Vanilla-1.26.40.scver"
        );
        let bytes = match fs::read(pack_path) {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        let mut outer = match zip::ZipArchive::new(Cursor::new(bytes)) {
            Ok(z) => z,
            Err(_) => return,
        };
        let mut nested: Option<Vec<u8>> = None;
        for i in 0..outer.len() {
            let mut entry = outer.by_index(i).unwrap();
            if !entry.is_file() {
                continue;
            }
            let name = entry.name().to_string();
            if name.ends_with("behavior_packs/vanilla.zip") {
                let mut data = Vec::new();
                use std::io::Read;
                entry.read_to_end(&mut data).unwrap();
                nested = Some(data);
                break;
            }
        }
        let Some(nested) = nested else { return };
        let (files, errors) = read_recipes_from_pack_bytes("vanilla", 0, &nested, &test_budgets());
        // Chemistry packs have malformed JSON; vanilla must be clean.
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert!(
            files.len() >= 800,
            "vanilla should have 800+ recipes, got {}",
            files.len()
        );
        assert!(!files
            .iter()
            .any(|f| f.relative_path.contains("__brarchive")));
    }
}
