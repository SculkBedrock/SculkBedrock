//! Per-kind compilation: independent schemas, budgets, quarantine.
//!
//! Never silently accept an unknown recipe type as an empty recipe.
//! Unknown root keys are hard errors by default; callers that need a
//! compatibility mode must opt into `quarantine_unknown_kind` explicitly
//! and every quarantined entry keeps its path and reason.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::kind::{RecipeKind, StationKind};
use crate::spec::{
    parse_ingredient, parse_output, requires_components, IngredientSpec, OutputSpec,
};

/// Compile budgets. Every queue/cache/worker needs limits; recipe input is
/// untrusted pack data so each dimension has an explicit bound.
#[derive(Clone, Copy, Debug)]
pub struct CompileBudgets {
    /// Max source files accepted in one compile call.
    pub max_files: usize,
    /// Max total source bytes accepted in one compile call.
    pub max_dir_bytes: u64,
    /// Max ingredients / inputs per recipe.
    pub max_ingredients: usize,
    /// Max shaped pattern cells (width * height after trim).
    pub max_pattern_cells: usize,
    /// Max outputs per recipe.
    pub max_outputs: usize,
    /// Max tag expansion entries when resolving one tag.
    pub max_tag_expansion: usize,
}

impl Default for CompileBudgets {
    fn default() -> Self {
        Self {
            max_files: 8192,
            max_dir_bytes: 64 * 1024 * 1024,
            max_ingredients: 64,
            max_pattern_cells: 9,
            max_outputs: 64,
            max_tag_expansion: 1024,
        }
    }
}

/// One raw source file. `sc_packloader` builds this from
/// `behavior_packs/<pack>/recipes/**/*.json`; the compiler never reads the
/// file system and never guesses the kind from the file name.
#[derive(Clone, Debug)]
pub struct SourceRecipe {
    pub pack_name: String,
    /// Pack order index: lower = lower priority (earlier in load order).
    /// Higher layers fully overwrite lower layers on identifier collision.
    pub pack_order: usize,
    pub relative_path: String,
    pub format_version: String,
    pub raw: serde_json::Value,
    pub fingerprint: [u8; 32],
    /// Raw byte length (for directory byte budgets).
    pub raw_bytes: usize,
}

impl SourceRecipe {
    pub fn new(
        pack_name: impl Into<String>,
        pack_order: usize,
        relative_path: impl Into<String>,
        format_version: impl Into<String>,
        raw: serde_json::Value,
        raw_bytes: &[u8],
    ) -> Self {
        let fingerprint = Sha256::digest(raw_bytes);
        let mut fp = [0u8; 32];
        fp.copy_from_slice(&fingerprint);
        Self {
            pack_name: pack_name.into(),
            pack_order,
            relative_path: relative_path.into(),
            format_version: format_version.into(),
            raw,
            fingerprint: fp,
            raw_bytes: raw_bytes.len(),
        }
    }

    /// Build from already-hashed content (e.g. packloader sources that
    /// computed the fingerprint at read time). Skips re-hashing.
    pub fn from_parts(
        pack_name: impl Into<String>,
        pack_order: usize,
        relative_path: impl Into<String>,
        format_version: impl Into<String>,
        raw: serde_json::Value,
        fingerprint: [u8; 32],
        raw_bytes: usize,
    ) -> Self {
        Self {
            pack_name: pack_name.into(),
            pack_order,
            relative_path: relative_path.into(),
            format_version: format_version.into(),
            raw,
            fingerprint,
            raw_bytes,
        }
    }
}

/// Canonical normalized recipe body.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RecipeBody {
    Shaped(ShapedBody),
    Shapeless(ShapelessBody),
    Furnace(FurnaceBody),
    FurnaceMaterial(FurnaceBody),
    BrewingMix(BrewingBody),
    BrewingContainer(BrewingBody),
    SmithingTransform(SmithingTransformBody),
    SmithingTrim(SmithingTrimBody),
    MaterialReducer(MaterialReducerBody),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ShapedBody {
    pub width: u8,
    pub height: u8,
    /// Row-major trimmed grid; `None` = empty slot.
    pub grid: Vec<Option<IngredientSpec>>,
    pub results: Vec<OutputSpec>,
    pub assume_symmetry: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ShapelessBody {
    pub ingredients: Vec<IngredientSpec>,
    pub results: Vec<OutputSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FurnaceBody {
    pub input: IngredientSpec,
    pub output: OutputSpec,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BrewingBody {
    pub input: IngredientSpec,
    pub reagent: IngredientSpec,
    pub output: OutputSpec,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SmithingTransformBody {
    pub template: IngredientSpec,
    pub base: IngredientSpec,
    pub addition: IngredientSpec,
    pub result: OutputSpec,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SmithingTrimBody {
    pub template: IngredientSpec,
    pub base: IngredientSpec,
    pub addition: IngredientSpec,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaterialReducerBody {
    pub input: IngredientSpec,
    pub outputs: Vec<OutputSpec>,
}

/// One compiled recipe: normalized body plus provenance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompiledRecipe {
    pub unlock_context: Option<String>,
    pub identifier: String,
    pub kind: RecipeKind,
    pub stations: Vec<StationKind>,
    /// Raw tag strings (preserved for diagnostics; stations are derived).
    pub tags: Vec<String>,
    pub priority: i32,
    pub pack_name: String,
    pub pack_order: usize,
    pub relative_path: String,
    pub format_version: String,
    pub fingerprint: [u8; 32],
    pub body: RecipeBody,
    /// Unlock requirements (`unlock` array); empty = always available
    /// subject to the world `recipesUnlock` rule.
    pub unlock: Vec<IngredientSpec>,
    /// Unknown top-level / nested fields kept for diagnostics.
    pub unknown_fields: Vec<String>,
}

/// A recipe that compiled far enough to identify but cannot be used.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DisabledRecipe {
    pub identifier: String,
    pub kind: Option<RecipeKind>,
    pub pack_name: String,
    pub relative_path: String,
    pub reason: String,
}

/// Hard failure with pack path (never silent).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileError {
    pub pack_name: String,
    pub relative_path: String,
    pub reason: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {}: {}",
            self.pack_name, self.relative_path, self.reason
        )
    }
}

impl std::error::Error for CompileError {}

/// Output of one compile pass over an ordered source list.
#[derive(Clone, Debug, Default)]
pub struct CompileOutput {
    pub recipes: Vec<CompiledRecipe>,
    pub disabled: Vec<DisabledRecipe>,
}

fn get_str<'a>(map: &'a serde_json::Map<String, serde_json::Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(|v| v.as_str())
}

fn parse_tags(value: Option<&serde_json::Value>) -> (Vec<String>, Vec<StationKind>) {
    let mut tags = Vec::new();
    match value {
        None => {}
        Some(serde_json::Value::String(s)) => tags.push(s.clone()),
        Some(serde_json::Value::Array(arr)) => {
            for entry in arr {
                if let Some(s) = entry.as_str() {
                    tags.push(s.to_string());
                }
            }
        }
        _ => {}
    }
    let stations = tags.iter().map(|t| StationKind::from_tag(t)).collect();
    (tags, stations)
}

fn parse_priority(map: &serde_json::Map<String, serde_json::Value>) -> i32 {
    map.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32
}

fn parse_unlock(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<IngredientSpec>, String> {
    let Some(unlock) = map.get("unlock") else {
        return Ok(Vec::new());
    };
    if let Some(context) = unlock.get("context").and_then(|value| value.as_str()) {
        if matches!(
            context,
            "None" | "AlwaysUnlocked" | "PlayerInWater" | "PlayerHasManyItems"
        ) {
            return Ok(Vec::new());
        }
        return Err(format!("unknown recipe unlock context '{context}'"));
    }
    let arr = unlock
        .as_array()
        .ok_or_else(|| "'unlock' must be an array".to_string())?;
    let mut out = Vec::new();
    for (i, entry) in arr.iter().enumerate() {
        let (spec, _) = parse_ingredient(entry, &format!("unlock[{i}]"))?;
        out.push(spec);
    }
    Ok(out)
}

fn parse_results(
    map: &serde_json::Map<String, serde_json::Value>,
    budgets: &CompileBudgets,
) -> Result<(Vec<OutputSpec>, Vec<String>), String> {
    // Accept both "result" and (defensively) "result" with stray whitespace.
    let key = map.keys().find(|k| k.trim() == "result").cloned();
    let Some(key) = key else {
        return Err("missing 'result'".to_string());
    };
    let value = &map[&key];
    let mut unknown = Vec::new();
    let mut out = Vec::new();
    match value {
        serde_json::Value::Array(arr) => {
            if arr.len() > budgets.max_outputs {
                return Err(format!(
                    "too many outputs {} > {}",
                    arr.len(),
                    budgets.max_outputs
                ));
            }
            for (i, entry) in arr.iter().enumerate() {
                let (spec, u) = parse_output(entry, &format!("result[{i}]"))?;
                unknown.extend(u);
                if let Some(field) = requires_components(entry) {
                    return Err(format!(
                        "result[{i}] requires unsupported components field '{field}'"
                    ));
                }
                out.push(spec);
            }
        }
        _ => {
            let (spec, u) = parse_output(value, "result")?;
            unknown.extend(u);
            if let Some(field) = requires_components(value) {
                return Err(format!(
                    "result requires unsupported components field '{field}'"
                ));
            }
            out.push(spec);
        }
    }
    if out.is_empty() {
        return Err("empty 'result'".to_string());
    }
    if out.len() > budgets.max_outputs {
        return Err(format!(
            "too many outputs {} > {}",
            out.len(),
            budgets.max_outputs
        ));
    }
    Ok((out, unknown))
}

fn unknown_top_fields(
    map: &serde_json::Map<String, serde_json::Value>,
    known: &[&str],
) -> Vec<String> {
    map.keys()
        .filter(|k| !known.contains(&k.as_str()))
        .cloned()
        .collect()
}

fn compile_shaped(
    map: &serde_json::Map<String, serde_json::Value>,
    budgets: &CompileBudgets,
) -> Result<(RecipeBody, Vec<IngredientSpec>, Vec<String>), String> {
    let pattern = map
        .get("pattern")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing 'pattern' array".to_string())?;
    if pattern.is_empty() || pattern.len() > 3 {
        return Err(format!("bad pattern row count {}", pattern.len()));
    }
    let mut rows: Vec<String> = Vec::new();
    for row in pattern {
        let s = row
            .as_str()
            .ok_or_else(|| "pattern rows must be strings".to_string())?;
        if s.is_empty() || s.chars().count() > 3 {
            return Err(format!("bad pattern row width {s:?}"));
        }
        rows.push(s.to_string());
    }
    let width = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
    // Vanilla packs contain ragged patterns (e.g. ["##", " ##"]); pad
    // shorter rows with empty slots instead of rejecting the file.
    for row in rows.iter_mut() {
        let missing = width.saturating_sub(row.chars().count());
        for _ in 0..missing {
            row.push(' ');
        }
    }
    let key_obj = map
        .get("key")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "missing 'key' object".to_string())?;
    // Key validation: single character, not space, no multi-char keys.
    let mut key_map: BTreeMap<char, IngredientSpec> = BTreeMap::new();
    for (k, v) in key_obj {
        let mut chars = k.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return Err(format!("bad key {k:?}: must be one character"));
        };
        if c == ' ' {
            return Err("bad key ' ': space is reserved for empty slots".to_string());
        }
        if key_map.contains_key(&c) {
            return Err(format!("duplicate key {k:?}"));
        }
        if let Some(field) = requires_components(v) {
            return Err(format!(
                "key '{c}' requires unsupported components field '{field}'"
            ));
        }
        let (spec, _) = parse_ingredient(v, &format!("key['{c}']"))?;
        if spec.count > 1 {
            // Shaped keys consume exactly one item per slot; a count > 1
            // would be ambiguous, so reject loudly instead of ignoring it.
            return Err(format!("key '{c}' must not set count"));
        }
        key_map.insert(c, spec);
    }
    // Every non-space pattern char must have a key; empty rows rejected.
    let mut has_content = false;
    for row in &rows {
        for c in row.chars() {
            if c == ' ' {
                continue;
            }
            has_content = true;
            if !key_map.contains_key(&c) {
                return Err(format!("pattern uses unknown key '{c}'"));
            }
        }
    }
    if !has_content {
        return Err("pattern is empty".to_string());
    }
    // Build full grid then trim outer empty rows/columns.
    let height = rows.len();
    let mut grid: Vec<Vec<Option<IngredientSpec>>> = Vec::new();
    for row in &rows {
        let mut out_row = Vec::new();
        for c in row.chars() {
            if c == ' ' {
                out_row.push(None);
            } else {
                out_row.push(key_map.get(&c).cloned());
            }
        }
        grid.push(out_row);
    }
    // Trim empty outer rows.
    while grid
        .first()
        .map(|r| r.iter().all(|c| c.is_none()))
        .unwrap_or(false)
    {
        grid.remove(0);
    }
    while grid
        .last()
        .map(|r| r.iter().all(|c| c.is_none()))
        .unwrap_or(false)
    {
        grid.pop();
    }
    // Trim empty outer columns.
    let mut w = width;
    while w > 0 && grid.iter().all(|r| matches!(r.first(), Some(None))) {
        for r in grid.iter_mut() {
            r.remove(0);
        }
        w -= 1;
    }
    while w > 0 && grid.iter().all(|r| matches!(r.last(), Some(None))) {
        for r in grid.iter_mut() {
            r.pop();
        }
        w -= 1;
    }
    let h = grid.len();
    let cells = w * h;
    if cells == 0 {
        return Err("pattern is empty after trim".to_string());
    }
    if cells > budgets.max_pattern_cells {
        return Err(format!(
            "pattern area {cells} > {}",
            budgets.max_pattern_cells
        ));
    }
    let flat: Vec<Option<IngredientSpec>> = grid.into_iter().flatten().collect();
    let (results, mut unknown) = parse_results(map, budgets)?;
    let assume_symmetry = map
        .get("assume_symmetry")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let unlock = parse_unlock(map)?;
    unknown.extend(unknown_top_fields(
        map,
        &[
            "description",
            "tags",
            "pattern",
            "key",
            "result",
            "priority",
            "assume_symmetry",
            "unlock",
        ],
    ));
    Ok((
        RecipeBody::Shaped(ShapedBody {
            width: w as u8,
            height: h as u8,
            grid: flat,
            results,
            assume_symmetry,
        }),
        unlock,
        unknown,
    ))
}

fn compile_shapeless(
    map: &serde_json::Map<String, serde_json::Value>,
    budgets: &CompileBudgets,
) -> Result<(RecipeBody, Vec<IngredientSpec>, Vec<String>), String> {
    let ingredients = map
        .get("ingredients")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing 'ingredients' array".to_string())?;
    if ingredients.is_empty() {
        return Err("empty 'ingredients'".to_string());
    }
    if ingredients.len() > budgets.max_ingredients {
        return Err(format!(
            "too many ingredients {} > {}",
            ingredients.len(),
            budgets.max_ingredients
        ));
    }
    let mut specs = Vec::new();
    let mut unknown = Vec::new();
    for (i, entry) in ingredients.iter().enumerate() {
        if let Some(field) = requires_components(entry) {
            return Err(format!(
                "ingredients[{i}] requires unsupported components field '{field}'"
            ));
        }
        let (spec, u) = parse_ingredient(entry, &format!("ingredients[{i}]"))?;
        unknown.extend(u);
        specs.push(spec);
    }
    // Expand counts into per-unit slots? No: keep (spec,count) and let the
    // matcher consume counts; but enforce a total unit bound.
    let total_units: usize = specs.iter().map(|s| s.count as usize).sum();
    if total_units > budgets.max_ingredients {
        return Err(format!(
            "too many ingredient units {total_units} > {}",
            budgets.max_ingredients
        ));
    }
    let (results, mut ru) = parse_results(map, budgets)?;
    unknown.append(&mut ru);
    let unlock = parse_unlock(map)?;
    unknown.extend(unknown_top_fields(
        map,
        &[
            "description",
            "tags",
            "ingredients",
            "result",
            "priority",
            "unlock",
        ],
    ));
    Ok((
        RecipeBody::Shapeless(ShapelessBody {
            ingredients: specs,
            results,
        }),
        unlock,
        unknown,
    ))
}

fn parse_single_input(
    map: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    ctx: &str,
) -> Result<IngredientSpec, String> {
    // Accept stray-whitespace variants ("output " appears in MS docs).
    let key = map.keys().find(|k| k.trim() == field).cloned();
    let Some(key) = key else {
        return Err(format!("missing '{field}'"));
    };
    let value = &map[&key];
    if let Some(field) = requires_components(value) {
        return Err(format!(
            "{ctx} requires unsupported components field '{field}'"
        ));
    }
    let (spec, _) = parse_ingredient(value, ctx)?;
    Ok(spec)
}

fn parse_single_output(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<OutputSpec, String> {
    let key = map.keys().find(|k| k.trim() == "output").cloned();
    let Some(key) = key else {
        return Err("missing 'output'".to_string());
    };
    let value = &map[&key];
    if let Some(field) = requires_components(value) {
        return Err(format!(
            "output requires unsupported components field '{field}'"
        ));
    }
    let (spec, _) = parse_output(value, "output")?;
    Ok(spec)
}

/// Smithing transform names its output "result".
fn parse_single_result(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<OutputSpec, String> {
    let key = map.keys().find(|k| k.trim() == "result").cloned();
    let Some(key) = key else {
        return Err("missing 'result'".to_string());
    };
    let value = &map[&key];
    if let Some(field) = requires_components(value) {
        return Err(format!(
            "result requires unsupported components field '{field}'"
        ));
    }
    let (spec, _) = parse_output(value, "result")?;
    Ok(spec)
}

fn compile_furnace(
    map: &serde_json::Map<String, serde_json::Value>,
    kind: RecipeKind,
    budgets: &CompileBudgets,
) -> Result<(RecipeBody, Vec<IngredientSpec>, Vec<String>), String> {
    let _ = budgets;
    let input = parse_single_input(map, "input", "input")?;
    let output = parse_single_output(map)?;
    let unlock = parse_unlock(map)?;
    let unknown = unknown_top_fields(
        map,
        &[
            "description",
            "tags",
            "input",
            "output",
            "output ",
            "priority",
            "unlock",
        ],
    );
    let body = FurnaceBody { input, output };
    Ok((
        match kind {
            RecipeKind::FurnaceMaterial => RecipeBody::FurnaceMaterial(body),
            _ => RecipeBody::Furnace(body),
        },
        unlock,
        unknown,
    ))
}

fn compile_brewing(
    map: &serde_json::Map<String, serde_json::Value>,
    kind: RecipeKind,
) -> Result<(RecipeBody, Vec<IngredientSpec>, Vec<String>), String> {
    let input = parse_single_input(map, "input", "input")?;
    let reagent = parse_single_input(map, "reagent", "reagent")?;
    let output = parse_single_output(map)?;
    let unlock = parse_unlock(map)?;
    let unknown = unknown_top_fields(
        map,
        &[
            "description",
            "tags",
            "input",
            "reagent",
            "output",
            "priority",
            "unlock",
        ],
    );
    let body = BrewingBody {
        input,
        reagent,
        output,
    };
    Ok((
        match kind {
            RecipeKind::BrewingContainer => RecipeBody::BrewingContainer(body),
            _ => RecipeBody::BrewingMix(body),
        },
        unlock,
        unknown,
    ))
}

fn compile_smithing_transform(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<(RecipeBody, Vec<IngredientSpec>, Vec<String>), String> {
    let template = parse_single_input(map, "template", "template")?;
    let base = parse_single_input(map, "base", "base")?;
    let addition = parse_single_input(map, "addition", "addition")?;
    // Smithing transform names its output "result" (not "output").
    let result = parse_single_result(map)?;
    let unlock = parse_unlock(map)?;
    let unknown = unknown_top_fields(
        map,
        &[
            "description",
            "tags",
            "template",
            "base",
            "addition",
            "result",
            "priority",
            "unlock",
        ],
    );
    Ok((
        RecipeBody::SmithingTransform(SmithingTransformBody {
            template,
            base,
            addition,
            result,
        }),
        unlock,
        unknown,
    ))
}

fn compile_smithing_trim(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<(RecipeBody, Vec<IngredientSpec>, Vec<String>), String> {
    let template = parse_single_input(map, "template", "template")?;
    let base = parse_single_input(map, "base", "base")?;
    let addition = parse_single_input(map, "addition", "addition")?;
    let unlock = parse_unlock(map)?;
    let unknown = unknown_top_fields(
        map,
        &[
            "description",
            "tags",
            "template",
            "base",
            "addition",
            "priority",
            "unlock",
        ],
    );
    Ok((
        RecipeBody::SmithingTrim(SmithingTrimBody {
            template,
            base,
            addition,
        }),
        unlock,
        unknown,
    ))
}

fn compile_material_reducer(
    map: &serde_json::Map<String, serde_json::Value>,
    budgets: &CompileBudgets,
) -> Result<(RecipeBody, Vec<IngredientSpec>, Vec<String>), String> {
    let input = parse_single_input(map, "input", "input")?;
    let key = map.keys().find(|k| k.trim() == "output").cloned();
    let Some(key) = key else {
        return Err("missing 'output'".to_string());
    };
    let value = &map[&key];
    let arr = value
        .as_array()
        .ok_or_else(|| "'output' must be an array for material reducer".to_string())?;
    if arr.is_empty() || arr.len() > budgets.max_outputs {
        return Err(format!("bad material reducer output count {}", arr.len()));
    }
    let mut outputs = Vec::new();
    for (i, entry) in arr.iter().enumerate() {
        if let Some(field) = requires_components(entry) {
            return Err(format!(
                "output[{i}] requires unsupported components field '{field}'"
            ));
        }
        let (spec, _) = parse_output(entry, &format!("output[{i}]"))?;
        outputs.push(spec);
    }
    let unlock = parse_unlock(map)?;
    let unknown = unknown_top_fields(
        map,
        &[
            "description",
            "tags",
            "input",
            "output",
            "priority",
            "unlock",
        ],
    );
    Ok((
        RecipeBody::MaterialReducer(MaterialReducerBody { input, outputs }),
        unlock,
        unknown,
    ))
}

/// Compile one source file. Hard errors become `Err(CompileError)`;
/// recipes that need unavailable data become `Ok(None)` plus a disabled
/// entry handled by the caller.
pub fn compile_source(
    source: &SourceRecipe,
    budgets: &CompileBudgets,
    quarantine_unknown_kind: bool,
    disabled_out: &mut Vec<DisabledRecipe>,
) -> Result<Option<CompiledRecipe>, CompileError> {
    let fail = |reason: String| CompileError {
        pack_name: source.pack_name.clone(),
        relative_path: source.relative_path.clone(),
        reason,
    };
    let root = source
        .raw
        .as_object()
        .ok_or_else(|| fail("recipe root must be a JSON object".to_string()))?;
    // The kind comes only from the JSON root key, never the file name.
    let kind_keys: Vec<&String> = root.keys().filter(|k| *k != "format_version").collect();
    if kind_keys.is_empty() {
        return Err(fail(
            "missing recipe body (only format_version)".to_string(),
        ));
    }
    if kind_keys.len() != 1 {
        return Err(fail(format!(
            "recipe must have exactly one body key, found {}",
            kind_keys.len()
        )));
    }
    let root_key = kind_keys[0].clone();
    let Some(kind) = RecipeKind::from_root_key(&root_key) else {
        if quarantine_unknown_kind {
            disabled_out.push(DisabledRecipe {
                identifier: format!("{}:{}", source.pack_name, source.relative_path),
                kind: None,
                pack_name: source.pack_name.clone(),
                relative_path: source.relative_path.clone(),
                reason: format!("unknown recipe type '{root_key}' (quarantined)"),
            });
            return Ok(None);
        }
        return Err(fail(format!("unknown recipe type '{root_key}'")));
    };
    let body_value = &root[&root_key];
    let body_obj = body_value
        .as_object()
        .ok_or_else(|| fail(format!("'{root_key}' body must be an object")))?;
    let description = body_obj.get("description").and_then(|v| v.as_object());
    let identifier = description
        .and_then(|d| d.get("identifier"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| fail("missing 'description.identifier'".to_string()))?
        .to_string();
    if identifier.is_empty() {
        return Err(fail("empty 'description.identifier'".to_string()));
    }
    let (tags, stations) = parse_tags(body_obj.get("tags"));
    let priority = body_obj
        .get("priority")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    // Strip "description" before per-kind compile (it is handled here).
    let compile_result = match kind {
        RecipeKind::Shaped => compile_shaped(body_obj, budgets),
        RecipeKind::Shapeless => compile_shapeless(body_obj, budgets),
        RecipeKind::Furnace | RecipeKind::FurnaceMaterial => {
            compile_furnace(body_obj, kind, budgets)
        }
        RecipeKind::BrewingMix | RecipeKind::BrewingContainer => compile_brewing(body_obj, kind),
        RecipeKind::SmithingTransform => compile_smithing_transform(body_obj),
        RecipeKind::SmithingTrim => compile_smithing_trim(body_obj),
        RecipeKind::MaterialReducer => compile_material_reducer(body_obj, budgets),
    };
    let (body, unlock, mut unknown) = match compile_result {
        Ok(v) => v,
        Err(reason) => {
            // Missing fields / wrong types / component requirements are
            // disabled with a reason when they stem from data the stage-1
            // model cannot evaluate; structural errors stay hard errors.
            // Distinguish by marker: component requirements quarantine,
            // everything else is a hard compile error.
            if reason.contains("unsupported components") {
                disabled_out.push(DisabledRecipe {
                    identifier: identifier.clone(),
                    kind: Some(kind),
                    pack_name: source.pack_name.clone(),
                    relative_path: source.relative_path.clone(),
                    reason: reason.clone(),
                });
                return Ok(None);
            }
            return Err(fail(reason));
        }
    };
    // Preserve description-level unknown fields too.
    if let Some(desc) = description {
        for k in desc.keys() {
            if k != "identifier" {
                unknown.push(format!("description.{k}"));
            }
        }
    }
    let _ = parse_priority(body_obj);
    Ok(Some(CompiledRecipe {
        unlock_context: body_obj
            .get("unlock")
            .and_then(|value| value.get("context"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        identifier,
        kind,
        stations,
        tags,
        priority,
        pack_name: source.pack_name.clone(),
        pack_order: source.pack_order,
        relative_path: source.relative_path.clone(),
        format_version: source.format_version.clone(),
        fingerprint: source.fingerprint,
        body,
        unlock,
        unknown_fields: unknown,
    }))
}

/// Compile an ordered source list with budgets and pack-override rules.
///
/// - Sources must already be ordered low-version → high-version.
/// - The same identifier in the same layer (same `pack_order`) is a hard
///   error; across layers the higher layer fully replaces the lower one.
/// - Unknown kinds are rejected unless `quarantine_unknown_kind` is set.
pub fn compile_ordered(
    sources: &[SourceRecipe],
    budgets: &CompileBudgets,
    quarantine_unknown_kind: bool,
) -> Result<CompileOutput, CompileError> {
    if sources.len() > budgets.max_files {
        return Err(CompileError {
            pack_name: String::new(),
            relative_path: String::new(),
            reason: format!(
                "too many recipe files {} > {}",
                sources.len(),
                budgets.max_files
            ),
        });
    }
    let total_bytes: usize = sources.iter().map(|s| s.raw_bytes).sum();
    if total_bytes as u64 > budgets.max_dir_bytes {
        return Err(CompileError {
            pack_name: String::new(),
            relative_path: String::new(),
            reason: format!(
                "recipe directory bytes {total_bytes} > {}",
                budgets.max_dir_bytes
            ),
        });
    }
    let mut by_id: BTreeMap<String, &SourceRecipe> = BTreeMap::new();
    let mut seen_layers = std::collections::HashSet::new();
    let mut disabled = Vec::new();
    for source in sources {
        let identifier = source.raw.as_object().and_then(|root| {
            root.iter()
                .find(|(key, _)| key.as_str() != "format_version")
                .and_then(|(_, body)| body.get("description")?.get("identifier")?.as_str())
        });
        let Some(identifier) = identifier else {
            compile_source(source, budgets, quarantine_unknown_kind, &mut disabled)?;
            continue;
        };
        if !seen_layers.insert((source.pack_order, identifier.to_string())) {
            return Err(CompileError {
                pack_name: source.pack_name.clone(),
                relative_path: source.relative_path.clone(),
                reason: format!("duplicate identifier '{identifier}' in the same layer"),
            });
        }
        if by_id
            .get(identifier)
            .is_none_or(|previous| previous.pack_order < source.pack_order)
        {
            by_id.insert(identifier.to_string(), source);
        }
    }
    // Superseded historical schemas must not invalidate the active pack stack.
    let mut recipes = Vec::new();
    for source in by_id.into_values() {
        if let Some(recipe) =
            compile_source(source, budgets, quarantine_unknown_kind, &mut disabled)?
        {
            recipes.push(recipe);
        }
    }
    // Deterministic network order: (priority, identifier).
    recipes.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.identifier.cmp(&b.identifier))
    });
    Ok(CompileOutput { recipes, disabled })
}

/// Deterministic registry fingerprint: sha256 over
/// `identifier | kind | priority | canonical-body` in identifier order.
pub fn registry_fingerprint(recipes: &[CompiledRecipe]) -> [u8; 32] {
    let mut sorted: Vec<&CompiledRecipe> = recipes.iter().collect();
    sorted.sort_by(|a, b| a.identifier.cmp(&b.identifier));
    let mut hasher = Sha256::new();
    for recipe in sorted {
        hasher.update(recipe.identifier.as_bytes());
        hasher.update([0]);
        hasher.update(recipe.kind.root_key().as_bytes());
        hasher.update([0]);
        hasher.update(recipe.priority.to_le_bytes());
        hasher.update([0]);
        let body = serde_json::to_vec(&recipe.body).unwrap_or_default();
        hasher.update(&body);
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(pack: &str, order: usize, path: &str, raw: serde_json::Value) -> SourceRecipe {
        let bytes = serde_json::to_vec(&raw).unwrap();
        SourceRecipe::new(pack, order, path, "1.12", raw, &bytes)
    }

    fn shaped_json(id: &str) -> serde_json::Value {
        serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shaped": {
                "description": {"identifier": id},
                "tags": ["crafting_table"],
                "pattern": ["XX", "X "],
                "key": {
                    "X": {"item": "minecraft:iron_ingot"}
                },
                "result": {"item": "minecraft:iron_pickaxe"}
            }
        })
    }

    #[test]
    fn shaped_trims_outer_empty() {
        let budgets = CompileBudgets::default();
        let mut disabled = Vec::new();
        let source = src(
            "p",
            0,
            "r.json",
            serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_shaped": {
                    "description": {"identifier": "minecraft:test"},
                    "tags": ["crafting_table"],
                    "pattern": [" X ", " X "],
                    "key": {"X": {"item": "minecraft:stone"}},
                    "result": {"item": "minecraft:out"}
                }
            }),
        );
        let recipe = compile_source(&source, &budgets, false, &mut disabled)
            .unwrap()
            .unwrap();
        match recipe.body {
            RecipeBody::Shaped(b) => {
                assert_eq!((b.width, b.height), (1, 2));
            }
            _ => panic!("wrong body"),
        }
    }

    #[test]
    fn unknown_root_key_is_rejected_by_default() {
        let budgets = CompileBudgets::default();
        let mut disabled = Vec::new();
        let source = src(
            "p",
            0,
            "r.json",
            serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_unknown": {"description": {"identifier": "x"}}
            }),
        );
        assert!(compile_source(&source, &budgets, false, &mut disabled).is_err());
        let mut disabled2 = Vec::new();
        assert!(compile_source(&source, &budgets, true, &mut disabled2)
            .unwrap()
            .is_none());
        assert_eq!(disabled2.len(), 1);
    }

    #[test]
    fn same_layer_duplicate_is_an_error_and_higher_layer_overwrites() {
        let budgets = CompileBudgets::default();
        let a = src("low", 0, "a.json", shaped_json("minecraft:dup"));
        let b = src("low2", 0, "b.json", shaped_json("minecraft:dup"));
        assert!(compile_ordered(&[a, b], &budgets, false).is_err());
        let a = src("low", 0, "a.json", shaped_json("minecraft:dup"));
        let b = src("high", 1, "b.json", shaped_json("minecraft:dup"));
        let out = compile_ordered(&[a, b], &budgets, false).unwrap();
        assert_eq!(out.recipes.len(), 1);
        assert_eq!(out.recipes[0].pack_name, "high");
    }

    #[test]
    fn missing_field_and_bad_pattern_are_hard_errors() {
        let budgets = CompileBudgets::default();
        let mut disabled = Vec::new();
        let bad = src(
            "p",
            0,
            "r.json",
            serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_shaped": {
                    "description": {"identifier": "minecraft:x"},
                    "tags": ["crafting_table"],
                    "pattern": ["ZZ"],
                    "key": {"X": {"item": "minecraft:stone"}},
                    "result": {"item": "minecraft:out"}
                }
            }),
        );
        assert!(compile_source(&bad, &budgets, false, &mut disabled).is_err());
    }
}
