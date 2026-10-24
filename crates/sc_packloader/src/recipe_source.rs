//! 行为包配方来源（`behavior_packs/<pack>/recipes/**/*.json`）。
//!
//! Behavior-pack recipe sources (`behavior_packs/<pack>/recipes/**/*.json`).
//!
//! 版本 `.scver` 将每个行为包作为嵌套 zip 内嵌；外层
//! `__brarchive/recipes.brarchive` 从不是唯一的 JSON 输入。
//! 本模块从嵌套包字节读取 `recipes/**/*.json`，沿用既有 JSON 预算 /
//! 带路径错误风格，并保留包身份、相对路径、`format_version`、标识提示
//! 与内容指纹。配方 *kind* 仅在编译期从 JSON 根键派生（`sc_recipe`），
//! 绝不从文件名推断。
//!
//! The version `.scver` embeds each behavior pack as a nested zip; the
//! outer `__brarchive/recipes.brarchive` is never the sole JSON input.
//! This module reads `recipes/**/*.json` from the nested pack bytes with
//! the existing JSON budget / pathful-error style and preserves pack
//! identity, relative path, `format_version`, identifier hint and the
//! content fingerprint. The recipe *kind* is only derived from the JSON
//! root key at compile time (`sc_recipe`), never from the file name.

use std::collections::HashMap;
use std::sync::Arc;

use crate::pack::{PackData, ResourcePack};
use crate::pack_loader::pack::zipped::{
    PackSource, MAX_BINARY_ENTRY_BYTES, MAX_JSON_DIRECTORY_BYTES, MAX_JSON_ENTRY_BYTES,
};
use crate::version_control::json_budget::BudgetReader;
use sc_ecs::resource::Resource;

use json_comments::StripComments;
use sha2::{Digest, Sha256};

/// 配方来源加载预算（全部显式、全部有界）。
///
/// Budgets for recipe source loading (all explicit, all bounded).
#[derive(Clone, Copy, Debug)]
pub struct RecipeSourceBudgets {
    /// 每个行为包最多接受的配方文件数。
    ///
    /// Max recipe files accepted per behavior pack.
    pub max_files_per_pack: usize,
    /// 每个行为包最多接受的配方总字节。
    ///
    /// Max total recipe bytes accepted per behavior pack.
    pub max_dir_bytes: u64,
    /// 单个配方文件最大字节。
    ///
    /// Max single recipe file bytes.
    pub max_file_bytes: u64,
}

impl Default for RecipeSourceBudgets {
    /// 默认预算（文件字节对齐包 JSON 条目预算）。
    ///
    /// Default budgets (file bytes match the pack JSON entry budget).
    fn default() -> Self {
        Self {
            max_files_per_pack: 4096,
            max_dir_bytes: 64 * 1024 * 1024,
            // Matches the pack JSON entry budget.
            max_file_bytes: MAX_JSON_ENTRY_BYTES,
        }
    }
}

/// 一个带来源信息的原始配方文件。
///
/// One raw recipe file with provenance.
#[derive(Clone, Debug)]
pub struct RecipeSourceFile {
    /// 行为包显示名（`manifest.name`）。
    ///
    /// Behavior pack display name (`manifest.name`).
    pub pack_name: String,
    /// 包加载顺序下标（越小越早加载 = 优先级越低）。
    /// 调用方按低版本 → 高版本排序并透传该下标。
    ///
    /// Pack load-order index (lower = loaded earlier = lower priority).
    /// Callers sort low-version → high-version and pass the index through.
    pub pack_order: usize,
    /// 行为包 zip 内路径（如 `recipes/acacia_boat.json`）。
    ///
    /// Path inside the behavior pack zip (e.g. `recipes/acacia_boat.json`).
    pub relative_path: String,
    /// 配方 `format_version`（缺失时为空串）。
    ///
    /// Recipe `format_version` (empty when absent).
    pub format_version: String,
    /// 存在时的 `description.identifier` 提示（编译器会从 JSON 正文
    /// 重新推导权威标识符）。
    ///
    /// `description.identifier` hint when present (the compiler re-derives
    /// the authoritative identifier from the JSON body).
    pub identifier_hint: Option<String>,
    /// 解析后的 JSON 值。
    ///
    /// Parsed JSON value.
    pub raw: serde_json::Value,
    /// 原始文件字节数（用于目录预算）。
    ///
    /// Raw file bytes (for directory budgets).
    pub raw_bytes: usize,
    /// 原始文件字节的 sha256。
    ///
    /// sha256 of the raw file bytes.
    pub fingerprint: [u8; 32],
}

impl RecipeSourceFile {
    /// 取编译器输入 JSON（`sc_recipe` 是可选消费方；
    /// 以此避免 packloader → recipe 的硬依赖）。
    ///
    /// Convert to the compiler input (`sc_recipe` is an optional consumer;
    /// this avoids a hard packloader → recipe dependency).
    pub fn raw_json(&self) -> &serde_json::Value {
        &self.raw
    }
}

/// 带路径的来源错误：包 + 路径 + 原因，绝不静默。
///
/// Pathful source error: pack + path + reason, never silent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecipeSourceError {
    /// 包名。
    ///
    /// Pack name.
    pub pack_name: String,
    /// 包内相对路径。
    ///
    /// Relative path inside the pack.
    pub relative_path: String,
    /// 失败原因。
    ///
    /// Failure reason.
    pub reason: String,
}

impl std::fmt::Display for RecipeSourceError {
    /// 以 `[包] 路径: 原因` 格式化。
    ///
    /// Formats as `[pack] path: reason`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {}: {}",
            self.pack_name, self.relative_path, self.reason
        )
    }
}

impl std::error::Error for RecipeSourceError {}

/// 计算字节的内容指纹（SHA-256）。
///
/// Computes the content fingerprint (SHA-256) of the bytes.
///
/// - `bytes`: 输入字节
fn fingerprint_of(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// 判断是否为配方 JSON 路径：排除 `__brarchive/`，须含 `recipes/` 段且以
/// `.json` 结尾。
///
/// Checks whether a path is a recipe JSON path: excludes `__brarchive/`,
/// requires a `recipes/` segment and a `.json` suffix.
///
/// - `name`: 包内路径
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

/// 取从 `recipes/` 段起的相对路径（保留原始大小写便于诊断）。
///
/// Keeps the path from the `recipes/` segment on, preserving the original
/// case for diagnostics.
///
/// - `name`: 包内路径
fn relative_recipe_path(name: &str) -> String {
    // Keep the path from the `recipes/` segment on, preserving the
    // original case for diagnostics.
    if let Some(pos) = name.to_ascii_lowercase().find("recipes/") {
        name[pos..].to_string()
    } else {
        name.to_string()
    }
}

/// 从一个行为包 zip 读取全部 `recipes/**/*.json` 文件。
///
/// Read all `recipes/**/*.json` files from one behavior-pack zip.
///
/// `pack_bytes` 是嵌套行为包 zip（`ResourcePack.pack_data`）。
/// 返回文件列表与逐文件错误（单个坏文件不会中断整包；
/// 目录/文件预算仍然生效）。
///
/// `pack_bytes` is the nested behavior-pack zip (`ResourcePack.pack_data`).
/// Returns the files plus per-file errors (a single bad file never aborts
/// the whole pack; directory/file budgets are still enforced).
///
/// - `pack_name`: 包显示名
/// - `pack_order`: 包加载顺序下标
/// - `pack`: 包来源
/// - `budgets`: 加载预算
pub fn read_recipes_from_pack_source(
    pack_name: &str,
    pack_order: usize,
    pack: &mut PackSource,
    budgets: &RecipeSourceBudgets,
) -> (Vec<RecipeSourceFile>, Vec<RecipeSourceError>) {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    // Candidate names from the unified source (archive order for
    // zips, sorted for directories), then per-file reads.
    let mut names: Vec<String> = Vec::new();
    for name in pack.file_names() {
        if is_recipe_json_path(&name) {
            names.push(name);
        }
    }
    names.sort();
    let mut total_bytes: u64 = 0;
    for name in names {
        if files.len() >= budgets.max_files_per_pack {
            errors.push(RecipeSourceError {
                pack_name: pack_name.to_string(),
                relative_path: relative_recipe_path(&name),
                reason: format!("too many recipe files (>{})", budgets.max_files_per_pack),
            });
            break;
        }
        let Some(size) = pack.file_size(&name) else {
            errors.push(RecipeSourceError {
                pack_name: pack_name.to_string(),
                relative_path: relative_recipe_path(&name),
                reason: "recipe entry could not be opened".to_string(),
            });
            continue;
        };
        if size > budgets.max_file_bytes.min(MAX_BINARY_ENTRY_BYTES) {
            errors.push(RecipeSourceError {
                pack_name: pack_name.to_string(),
                relative_path: relative_recipe_path(&name),
                reason: format!("recipe file exceeds {} bytes", budgets.max_file_bytes),
            });
            continue;
        }
        if total_bytes.saturating_add(size) > budgets.max_dir_bytes.min(MAX_JSON_DIRECTORY_BYTES) {
            errors.push(RecipeSourceError {
                pack_name: pack_name.to_string(),
                relative_path: relative_recipe_path(&name),
                reason: format!("recipe directory exceeds {} bytes", budgets.max_dir_bytes),
            });
            break;
        }
        let Some(bytes) = pack.read_file(&name) else {
            errors.push(RecipeSourceError {
                pack_name: pack_name.to_string(),
                relative_path: relative_recipe_path(&name),
                reason: "recipe file could not be read".to_string(),
            });
            continue;
        };
        if bytes.len() as u64 > budgets.max_file_bytes + 1 {
            errors.push(RecipeSourceError {
                pack_name: pack_name.to_string(),
                relative_path: relative_recipe_path(&name),
                reason: format!("recipe file exceeds {} bytes", budgets.max_file_bytes),
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

/// 从已解析的 [`ResourcePack`] 读取配方（使用其共享 zip 字节）。
/// 调用方按 `load_version` 中的低 → 高版本排序分配 `pack_order`。
///
/// Read recipes from an already-parsed [`ResourcePack`] (uses its shared
/// zip bytes). The caller assigns `pack_order` from the low → high version
/// sort done in `load_version`.
///
/// - `pack`: 已解析资源包
/// - `pack_order`: 包加载顺序下标
/// - `budgets`: 加载预算
pub fn read_recipes_from_resource_pack(
    pack: &ResourcePack,
    pack_order: usize,
    budgets: &RecipeSourceBudgets,
) -> (Vec<RecipeSourceFile>, Vec<RecipeSourceError>) {
    let mut source = match &pack.source {
        PackData::Zip(bytes) => match PackSource::from_shared(Arc::clone(bytes)) {
            Ok(source) => source,
            Err(error) => {
                return (
                    Vec::new(),
                    vec![RecipeSourceError {
                        pack_name: pack.manifest.name.clone(),
                        relative_path: String::new(),
                        reason: format!("behavior pack zip could not be opened: {error}"),
                    }],
                );
            }
        },
        PackData::Dir(dir) => PackSource::from_dir(dir.clone()),
    };
    read_recipes_from_pack_source(&pack.manifest.name, pack_order, &mut source, budgets)
}

/// 按低版本 → 高版本合并有序包：拼接各包文件并保持包顺序
/// （覆盖解析发生在 `sc_recipe` 编译期，不在此处）。
///
/// Merge ordered packs low-version → high-version: concatenates per-pack
/// files preserving pack order (override resolution happens at compile
/// time in `sc_recipe`, not here).
///
/// - `packs`: (包名, 文件列表) 列表
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

/// 一个有序来源条目（包顺序已应用）。
///
/// One ordered source entry (pack order already applied).
#[derive(Clone, Debug)]
pub struct OrderedRecipeSource {
    /// 包名。
    ///
    /// Pack name.
    pub pack_name: String,
    /// 包加载顺序下标。
    ///
    /// Pack load-order index.
    pub pack_order: usize,
    /// 配方文件。
    ///
    /// Recipe file.
    pub file: RecipeSourceFile,
}

/// 版本包加载后保留的原始配方来源（ECS 资源）。
///
/// Raw recipe sources kept from version-pack load (ECS resource).
///
/// `load_version`（SCPreLoad）从嵌套行为包 zip 填充本资源；
/// 引导期 `load_recipe_registry`（SCLoad）将其编译为冻结的
/// `sc_game::SharedRecipeRegistry`。保留原始文件（而非仅编译快照）
/// 以保留包身份/路径诊断信息。
///
/// `load_version` (SCPreLoad) fills this from the nested behavior-pack
/// zips; bootstrap `load_recipe_registry` (SCLoad) compiles it into the
/// frozen `sc_game::SharedRecipeRegistry`. Keeping the raw files (not just
/// the compiled snapshot) preserves pack identity/path diagnostics.
#[derive(Resource, Clone, Debug, Default)]
pub struct RawRecipeSources {
    /// 加载成功的配方文件。
    ///
    /// Successfully loaded recipe files.
    pub files: Vec<RecipeSourceFile>,
    /// 逐文件错误。
    ///
    /// Per-file errors.
    pub errors: Vec<RecipeSourceError>,
}

/// 配方匹配可用的标签表（仅物品标签）。
///
/// Tag tables available to recipe matching (item tags only).
#[derive(Clone, Debug, Default)]
pub struct ItemTagTable {
    /// 标签 → 成员标识列表。
    ///
    /// Tag → member identifier list.
    inner: HashMap<String, Vec<String>>,
}

impl ItemTagTable {
    /// 从映射构造标签表。
    ///
    /// Builds a tag table from a map.
    ///
    /// - `inner`: 标签 → 成员列表映射
    pub fn new(inner: HashMap<String, Vec<String>>) -> Self {
        Self { inner }
    }
    /// 取标签成员列表；不存在时为空表。
    ///
    /// Returns the tag members; empty when absent.
    pub fn members(&self, tag: &str) -> Vec<String> {
        self.inner.get(tag).cloned().unwrap_or_default()
    }
    /// 标签是否存在。
    ///
    /// Whether the tag exists.
    pub fn contains(&self, tag: &str) -> bool {
        self.inner.contains_key(tag)
    }
    /// 标签数量。
    ///
    /// Number of tags.
    pub fn len(&self) -> usize {
        self.inner.len()
    }
    /// 是否为空表。
    ///
    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let (files, errors) = read_recipes_from_pack_source(
            "test",
            0,
            &mut PackSource::from_bytes(bytes.to_vec()).unwrap(),
            &test_budgets(),
        );
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
        let (files, errors) = read_recipes_from_pack_source(
            "test",
            3,
            &mut PackSource::from_bytes(bytes.to_vec()).unwrap(),
            &test_budgets(),
        );
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
        let (files, errors) = read_recipes_from_pack_source(
            "test",
            0,
            &mut PackSource::from_bytes(bytes.to_vec()).unwrap(),
            &budgets,
        );
        assert_eq!(files.len(), 2);
        assert!(!errors.is_empty());
    }

}
