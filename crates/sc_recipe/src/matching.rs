//! Matching: shaped grids, shapeless multisets, process recipes.
//!
//! Rules:
//! - Shaped patterns are trimmed at compile time; matching never adds
//!   mirroring unless `assume_symmetry` is set (Microsoft docs default
//!   `true`). Mirroring here means horizontal flip of the trimmed grid.
//! - Shapeless uses deterministic bounded bipartite matching over a
//!   multiset, never a naive greedy that fails on overlapping tags.
//! - The match key is (identifier, data-wildcard, components fingerprint).
//!   Stage-1 has no components, so any recipe that needs them is disabled
//!   at compile time and never reaches the matcher.
//! - Station, size and per-slot counts are always verified.

use std::collections::HashMap;

use crate::compile::{CompiledRecipe, RecipeBody};
use crate::kind::StationKind;
use crate::spec::{IngredientChoice, IngredientSpec, DATA_WILDCARD};

/// One input stack presented to the matcher.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct MatchInput {
    pub identifier: String,
    pub data: i32,
    /// Components/NBT fingerprint. `None` = no components present.
    /// Stage-1 callers pass `None`; recipes requiring components are
    /// disabled before matching, so `None` never falsely matches them.
    pub components: Option<[u8; 32]>,
    pub count: u16,
}

impl MatchInput {
    pub fn new(identifier: impl Into<String>, data: i32, count: u16) -> Self {
        Self {
            identifier: identifier.into(),
            data,
            components: None,
            count,
        }
    }
}

/// Resolves an item tag to member identifiers. The game layer provides
/// this from its tag tables; tests use a static map.
pub trait TagResolver {
    fn members(&self, tag: &str) -> Vec<String>;
}

pub struct MapTagResolver {
    map: HashMap<String, Vec<String>>,
}

impl MapTagResolver {
    pub fn new(map: HashMap<String, Vec<String>>) -> Self {
        Self { map }
    }
    pub fn empty() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
}

impl TagResolver for MapTagResolver {
    fn members(&self, tag: &str) -> Vec<String> {
        self.map.get(tag).cloned().unwrap_or_default()
    }
}

fn data_matches(required: Option<i32>, actual: i32) -> bool {
    match required {
        None => true,
        Some(v) if v == DATA_WILDCARD || v == 32767 => true,
        Some(v) => v == actual,
    }
}

fn choice_matches(
    choice: &IngredientChoice,
    input: &MatchInput,
    tags: &dyn TagResolver,
    max_expansion: usize,
) -> bool {
    match choice {
        IngredientChoice::Item { identifier, data } => {
            // Components: stage-1 recipes never require them (disabled at
            // compile); if the *input* carries components but the recipe
            // does not constrain them, treat as match (vanilla behavior for
            // plain ingredients). If a recipe required components it would
            // have been quarantined, so reaching here with a plain item
            // ingredient is correct.
            if &input.identifier == identifier && data_matches(*data, input.data) {
                return true;
            }
            item_family_matches(identifier, *data, input, tags, max_expansion)
        }
        IngredientChoice::Tag { tag } => {
            let members = tags.members(tag);
            if members.len() > max_expansion {
                return false;
            }
            members.iter().any(|m| m == &input.identifier)
        }
    }
}

/// Bedrock legacy family fallback for `Item` ingredients.
///
/// Vanilla recipe files use pseudo names (`minecraft:planks`, `minecraft:log2`,
/// `minecraft:reeds`, ...) that denote a whole material family. The game layer
/// registers these families in the tag table (ordered by legacy data value);
/// `data` then selects the legacy variant index (`None`/wildcard = any member).
/// Exact identifiers with no family entry behave exactly as before.
pub fn item_family_matches(
    identifier: &str,
    data: Option<i32>,
    input: &MatchInput,
    tags: &dyn TagResolver,
    max_expansion: usize,
) -> bool {
    let members = tags.members(identifier);
    if members.is_empty() || members.len() > max_expansion {
        return false;
    }
    let Some(position) = members.iter().position(|m| m == &input.identifier) else {
        return false;
    };
    // Flattened inputs carry no variant data (damage 0); the legacy index
    // comes from the member position, mirroring `data_matches`.
    match data {
        None => true,
        Some(v) if v == DATA_WILDCARD || v == 32767 => true,
        Some(v) => v as usize == position,
    }
}

fn spec_matches(
    spec: &IngredientSpec,
    input: &MatchInput,
    tags: &dyn TagResolver,
    max_expansion: usize,
) -> bool {
    if input.count < spec.count {
        return false;
    }
    spec.choices
        .iter()
        .any(|c| choice_matches(c, input, tags, max_expansion))
}

/// Shaped match over a `width x height` crafting grid.
///
/// `grid` is row-major `width*height` with `None` for empty slots.
/// The recipe grid is placed at every offset that fits; every recipe cell
/// must match and every non-empty grid cell must be covered.
pub fn match_shaped(
    recipe: &CompiledRecipe,
    grid: &[Option<MatchInput>],
    grid_width: u8,
    grid_height: u8,
    station: StationKind,
    tags: &dyn TagResolver,
    max_expansion: usize,
) -> bool {
    let RecipeBody::Shaped(body) = &recipe.body else {
        return false;
    };
    if !recipe.stations.is_empty() && !recipe.stations.contains(&station) {
        return false;
    }
    if grid.len() != grid_width as usize * grid_height as usize {
        return false;
    }
    let candidates: Vec<Vec<Option<IngredientSpec>>> = if body.assume_symmetry {
        vec![
            body.grid.clone(),
            mirror_grid(&body.grid, body.width as usize),
        ]
    } else {
        vec![body.grid.clone()]
    };
    for candidate in &candidates {
        if match_shaped_grid(
            candidate,
            body.width,
            body.height,
            grid,
            grid_width,
            grid_height,
            tags,
            max_expansion,
        ) {
            return true;
        }
    }
    false
}

fn mirror_grid(grid: &[Option<IngredientSpec>], width: usize) -> Vec<Option<IngredientSpec>> {
    let height = grid.len() / width.max(1);
    let mut out = Vec::with_capacity(grid.len());
    for row in 0..height {
        for col in 0..width {
            out.push(grid[row * width + (width - 1 - col)].clone());
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn match_shaped_grid(
    pattern: &[Option<IngredientSpec>],
    pw: u8,
    ph: u8,
    grid: &[Option<MatchInput>],
    gw: u8,
    gh: u8,
    tags: &dyn TagResolver,
    max_expansion: usize,
) -> bool {
    let (pw, ph, gw, gh) = (pw as usize, ph as usize, gw as usize, gh as usize);
    if pw > gw || ph > gh {
        return false;
    }
    for oy in 0..=(gh - ph) {
        for ox in 0..=(gw - pw) {
            if offset_matches(pattern, pw, ph, grid, gw, gh, ox, oy, tags, max_expansion) {
                return true;
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn offset_matches(
    pattern: &[Option<IngredientSpec>],
    pw: usize,
    ph: usize,
    grid: &[Option<MatchInput>],
    gw: usize,
    gh: usize,
    ox: usize,
    oy: usize,
    tags: &dyn TagResolver,
    max_expansion: usize,
) -> bool {
    for y in 0..gh {
        for x in 0..gw {
            let cell = &grid[y * gw + x];
            let in_pattern = x >= ox && x < ox + pw && y >= oy && y < oy + ph;
            if !in_pattern {
                // Outside the placed pattern every slot must be empty.
                if cell.is_some() {
                    return false;
                }
                continue;
            }
            let spec = &pattern[(y - oy) * pw + (x - ox)];
            match (spec, cell) {
                (None, None) => {}
                (None, Some(_)) => return false,
                (Some(_), None) => return false,
                (Some(spec), Some(input)) => {
                    if !spec_matches(spec, input, tags, max_expansion) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// Shapeless match: deterministic bounded bipartite matching.
///
/// Each ingredient must be assigned a distinct input stack with enough
/// count. Tag overlap is handled by exhaustive search with memoization
/// over (ingredient_index, used_mask) — deterministic and never greedy.
/// Bounds: ingredients ≤ 64, inputs ≤ 64; the search is exponential in
/// the worst case but vanilla recipes are tiny (≤ 9 ingredients) and the
/// mask uses u64 with early pruning by candidate precomputation.
pub fn match_shapeless(
    recipe: &CompiledRecipe,
    inputs: &[MatchInput],
    station: StationKind,
    tags: &dyn TagResolver,
    max_expansion: usize,
) -> bool {
    let RecipeBody::Shapeless(body) = &recipe.body else {
        return false;
    };
    if !recipe.stations.is_empty() && !recipe.stations.contains(&station) {
        return false;
    }
    // Non-empty inputs only; empty slots are removed by the caller.
    let inputs: Vec<&MatchInput> = inputs.iter().filter(|i| i.count > 0).collect();
    if inputs.len() > 64 || body.ingredients.len() > 64 {
        return false;
    }
    // Candidate matrix.
    let mut candidates: Vec<Vec<usize>> = Vec::with_capacity(body.ingredients.len());
    for spec in &body.ingredients {
        let mut list = Vec::new();
        for (idx, input) in inputs.iter().enumerate() {
            if spec_matches(spec, input, tags, max_expansion) {
                list.push(idx);
            }
        }
        if list.is_empty() {
            return false;
        }
        // Deterministic order: sort candidate input indices.
        list.sort_unstable();
        candidates.push(list);
    }
    // Order ingredients by fewest candidates first for pruning, but keep
    // the assignment deterministic.
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    order.sort_by_key(|i| (candidates[*i].len(), *i));
    let ordered: Vec<Vec<usize>> = order.iter().map(|i| candidates[*i].clone()).collect();
    // Depth-first search with memo on (pos, used_mask) when inputs ≤ 64.
    // For > 64 inputs this path already returned false above.
    fn dfs(
        pos: usize,
        used: u64,
        ordered: &[Vec<usize>],
        memo: &mut std::collections::HashSet<(usize, u64)>,
    ) -> bool {
        if pos == ordered.len() {
            return true;
        }
        if !memo.insert((pos, used)) {
            // Already visited this state via another path that failed?
            // Note: insert returns false if present; but we need failure
            // memo only. Simplify: keep a failure set.
        }
        for idx in &ordered[pos] {
            // Inputs longer than 64 are rejected earlier; idx < 64 here.
            if *idx >= 64 {
                continue;
            }
            let bit = 1u64 << *idx;
            if used & bit != 0 {
                continue;
            }
            if dfs(pos + 1, used | bit, ordered, memo) {
                return true;
            }
        }
        false
    }
    // Fast path for > 64 inputs is unreachable; for ≤ 64 use bitmask.
    // When inputs.len() > 64 we already bailed. When ingredients assign
    // the same stack twice with count ≥ 2, Bedrock requires distinct
    // slots per ingredient entry, so one-bit-per-input is correct.
    if inputs.len() <= 64 {
        let mut memo = std::collections::HashSet::new();
        // Correct memo: only remember failures.
        fn dfs_fail(
            pos: usize,
            used: u64,
            ordered: &[Vec<usize>],
            fail: &mut std::collections::HashSet<(usize, u64)>,
        ) -> bool {
            if pos == ordered.len() {
                return true;
            }
            if fail.contains(&(pos, used)) {
                return false;
            }
            for idx in &ordered[pos] {
                if *idx >= 64 {
                    continue;
                }
                let bit = 1u64 << *idx;
                if used & bit != 0 {
                    continue;
                }
                if dfs_fail(pos + 1, used | bit, ordered, fail) {
                    return true;
                }
            }
            fail.insert((pos, used));
            false
        }
        return dfs_fail(0, 0, &ordered, &mut memo);
    }
    // Fallback greedy for oversized (unreachable due to bound above).
    let _ = dfs;
    false
}

/// Furnace / process single-input match.
pub fn match_process_input(
    spec: &IngredientSpec,
    input: &MatchInput,
    tags: &dyn TagResolver,
    max_expansion: usize,
) -> bool {
    spec_matches(spec, input, tags, max_expansion)
}

/// Find the best instant (shaped/shapeless) recipe for the given grid.
///
/// Ordering: priority ascending, then identifier ascending. Only recipes
/// whose station matches and whose body matches are considered.
pub fn find_best_instant<'a>(
    recipes: &'a [CompiledRecipe],
    grid: &[Option<MatchInput>],
    grid_width: u8,
    grid_height: u8,
    station: StationKind,
    tags: &dyn TagResolver,
    max_expansion: usize,
) -> Option<&'a CompiledRecipe> {
    let mut ordered: Vec<&CompiledRecipe> = recipes
        .iter()
        .filter(|r| matches!(r.body, RecipeBody::Shaped(_) | RecipeBody::Shapeless(_)))
        .collect();
    ordered.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.identifier.cmp(&b.identifier))
    });
    // For shapeless, flatten the grid to a multiset.
    let flat: Vec<MatchInput> = grid.iter().filter_map(|c| c.clone()).collect();
    for recipe in ordered {
        let matched = match &recipe.body {
            RecipeBody::Shaped(_) => match_shaped(
                recipe,
                grid,
                grid_width,
                grid_height,
                station,
                tags,
                max_expansion,
            ),
            RecipeBody::Shapeless(_) => {
                match_shapeless(recipe, &flat, station, tags, max_expansion)
            }
            _ => false,
        };
        if matched {
            return Some(recipe);
        }
    }
    None
}

/// Deterministic tie-break helper used by transaction selection.
pub fn sort_candidates<'a>(mut candidates: Vec<&'a CompiledRecipe>) -> Vec<&'a CompiledRecipe> {
    candidates.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.identifier.cmp(&b.identifier))
    });
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{CompileBudgets, SourceRecipe};
    use std::collections::HashMap;

    fn compile_one(raw: serde_json::Value) -> CompiledRecipe {
        let bytes = serde_json::to_vec(&raw).unwrap();
        let src = SourceRecipe::new("p", 0, "r.json", "1.12", raw, &bytes);
        let mut disabled = Vec::new();
        crate::compile::compile_source(&src, &CompileBudgets::default(), false, &mut disabled)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn legacy_family_pseudo_names_match_with_data_index() {
        // Bedrock pseudo-name: `minecraft:planks` (no data) accepts any planks;
        // `minecraft:log2` data 1 accepts only dark_oak (legacy order).
        let recipe = compile_one(serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shaped": {
                "description": {"identifier": "minecraft:test"},
                "tags": ["crafting_table"],
                "pattern": ["XX", "XX"],
                "key": {"X": {"item": "minecraft:planks"}},
                "result": {"item": "minecraft:out"}
            }
        }));
        let mut map = HashMap::new();
        map.insert(
            "minecraft:planks".to_string(),
            vec![
                "minecraft:oak_planks".to_string(),
                "minecraft:dark_oak_planks".to_string(),
            ],
        );
        map.insert(
            "minecraft:log2".to_string(),
            vec![
                "minecraft:acacia_log".to_string(),
                "minecraft:dark_oak_log".to_string(),
            ],
        );
        let tags = MapTagResolver::new(map);
        let grid = vec![
            Some(MatchInput::new("minecraft:dark_oak_planks", 0, 1)),
            Some(MatchInput::new("minecraft:dark_oak_planks", 0, 1)),
            Some(MatchInput::new("minecraft:dark_oak_planks", 0, 1)),
            Some(MatchInput::new("minecraft:dark_oak_planks", 0, 1)),
        ];
        assert!(match_shaped(
            &recipe,
            &grid,
            2,
            2,
            StationKind::CraftingTable,
            &tags,
            1024
        ));
        // Unregistered families still match exactly: stone accepts nothing beyond labelling.
        let stone_grid = vec![
            Some(MatchInput::new("minecraft:granite", 0, 1)),
            Some(MatchInput::new("minecraft:granite", 0, 1)),
            Some(MatchInput::new("minecraft:granite", 0, 1)),
            Some(MatchInput::new("minecraft:granite", 0, 1)),
        ];
        assert!(!match_shaped(
            &recipe,
            &stone_grid,
            2,
            2,
            StationKind::CraftingTable,
            &tags,
            1024
        ));
        // data selects the legacy index.
        assert!(item_family_matches(
            "minecraft:log2",
            Some(1),
            &MatchInput::new("minecraft:dark_oak_log", 0, 1),
            &tags,
            1024
        ));
        assert!(!item_family_matches(
            "minecraft:log2",
            Some(1),
            &MatchInput::new("minecraft:acacia_log", 0, 1),
            &tags,
            1024
        ));
        assert!(item_family_matches(
            "minecraft:log2",
            None,
            &MatchInput::new("minecraft:acacia_log", 0, 1),
            &tags,
            1024
        ));
    }

    #[test]
    fn shaped_matches_with_offset_and_symmetry() {        let recipe = compile_one(serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shaped": {
                "description": {"identifier": "minecraft:test"},
                "tags": ["crafting_table"],
                "pattern": ["XX", "X "],
                "key": {
                    "X": {"item": "minecraft:stone"}
                },
                "result": {"item": "minecraft:out"}
            }
        }));
        let tags = MapTagResolver::empty();
        let stone = || Some(MatchInput::new("minecraft:stone", 0, 1));
        // Exact placement at top-left of a 3x3 grid.
        let mut grid: Vec<Option<MatchInput>> = vec![None; 9];
        grid[0] = stone();
        grid[1] = stone();
        grid[3] = stone();
        assert!(match_shaped(
            &recipe,
            &grid,
            3,
            3,
            StationKind::CraftingTable,
            &tags,
            1024
        ));
        // Mirrored placement also matches (assume_symmetry defaults true).
        // Original L: (0,0),(1,0),(0,1). Mirror: (0,0),(1,0),(1,1).
        let mut mirrored: Vec<Option<MatchInput>> = vec![None; 9];
        mirrored[0] = stone();
        mirrored[1] = stone();
        mirrored[4] = stone();
        assert!(match_shaped(
            &recipe,
            &mirrored,
            3,
            3,
            StationKind::CraftingTable,
            &tags,
            1024
        ));
        // Wrong station rejects.
        assert!(!match_shaped(
            &recipe,
            &grid,
            3,
            3,
            StationKind::Stonecutter,
            &tags,
            1024
        ));
    }

    #[test]
    fn shaped_without_symmetry_rejects_mirror() {
        let recipe = compile_one(serde_json::json!({
            "format_version": "1.19",
            "minecraft:recipe_shaped": {
                "description": {"identifier": "minecraft:zig"},
                "tags": ["crafting_table"],
                "assume_symmetry": false,
                "pattern": ["##", " ##"],
                "key": {"#": {"item": "minecraft:planks"}},
                "result": {"item": "minecraft:zig"}
            }
        }));
        let tags = MapTagResolver::empty();
        let plank = || Some(MatchInput::new("minecraft:planks", 0, 1));
        // Padded pattern is 3 wide: rows "## " / " ##".
        // Zig at offset (0,0) of a 3x3 grid: (0,0),(1,0),(1,1),(2,1).
        let mut grid: Vec<Option<MatchInput>> = vec![None; 9];
        grid[0] = plank();
        grid[1] = plank();
        grid[4] = plank();
        grid[5] = plank();
        assert!(match_shaped(
            &recipe,
            &grid,
            3,
            3,
            StationKind::CraftingTable,
            &tags,
            1024
        ));
        // Horizontal flip is (1,0),(2,0),(0,1),(1,1); with
        // assume_symmetry=false it must not match.
        let mut mirrored: Vec<Option<MatchInput>> = vec![None; 9];
        mirrored[1] = plank();
        mirrored[2] = plank();
        mirrored[3] = plank();
        mirrored[4] = plank();
        assert!(!match_shaped(
            &recipe,
            &mirrored,
            3,
            3,
            StationKind::CraftingTable,
            &tags,
            1024
        ));
    }

    #[test]
    fn shapeless_overlapping_tags_use_bounded_matching() {
        // Recipe needs one planks-tag and one stone: inputs are
        // [oak_planks-as-planks, stone]. A greedy matcher that assigns the
        // tag to the wrong stack first would fail; ours must succeed.
        let recipe = compile_one(serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shapeless": {
                "description": {"identifier": "minecraft:overlap"},
                "tags": ["crafting_table"],
                "ingredients": [
                    {"tag": "minecraft:planks"},
                    {"item": "minecraft:oak_planks"}
                ],
                "result": {"item": "minecraft:out"}
            }
        }));
        let mut map = HashMap::new();
        map.insert(
            "minecraft:planks".to_string(),
            vec![
                "minecraft:oak_planks".to_string(),
                "minecraft:birch_planks".to_string(),
            ],
        );
        let tags = MapTagResolver::new(map);
        let inputs = vec![
            MatchInput::new("minecraft:birch_planks", 0, 1),
            MatchInput::new("minecraft:oak_planks", 0, 1),
        ];
        assert!(match_shapeless(
            &recipe,
            &inputs,
            StationKind::CraftingTable,
            &tags,
            1024
        ));
        // Reversed order must also match (deterministic, order-free).
        let inputs2 = vec![
            MatchInput::new("minecraft:oak_planks", 0, 1),
            MatchInput::new("minecraft:birch_planks", 0, 1),
        ];
        assert!(match_shapeless(
            &recipe,
            &inputs2,
            StationKind::CraftingTable,
            &tags,
            1024
        ));
    }

    #[test]
    fn data_wildcard_matches_any() {
        let recipe = compile_one(serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shapeless": {
                "description": {"identifier": "minecraft:wild"},
                "tags": ["crafting_table"],
                "ingredients": [{"item": "minecraft:planks"}],
                "result": {"item": "minecraft:out"}
            }
        }));
        let tags = MapTagResolver::empty();
        assert!(match_shapeless(
            &recipe,
            &[MatchInput::new("minecraft:planks", 4, 1)],
            StationKind::CraftingTable,
            &tags,
            1024
        ));
        let recipe2 = compile_one(serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shapeless": {
                "description": {"identifier": "minecraft:exact"},
                "tags": ["crafting_table"],
                "ingredients": [{"item": "minecraft:planks", "data": 4}],
                "result": {"item": "minecraft:out"}
            }
        }));
        assert!(match_shapeless(
            &recipe2,
            &[MatchInput::new("minecraft:planks", 4, 1)],
            StationKind::CraftingTable,
            &tags,
            1024
        ));
        assert!(!match_shapeless(
            &recipe2,
            &[MatchInput::new("minecraft:planks", 5, 1)],
            StationKind::CraftingTable,
            &tags,
            1024
        ));
    }

    #[test]
    fn priority_and_identifier_break_ties() {
        let mk = |id: &str, priority: i64| {
            compile_one(serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_shapeless": {
                    "description": {"identifier": id},
                    "tags": ["crafting_table"],
                    "priority": priority,
                    "ingredients": [{"item": "minecraft:stone"}],
                    "result": {"item": "minecraft:out"}
                }
            }))
        };
        let a = mk("minecraft:b", 0);
        let b = mk("minecraft:a", 0);
        let c = mk("minecraft:c", -1);
        let sorted = sort_candidates(vec![&a, &b, &c]);
        assert_eq!(sorted[0].identifier, "minecraft:c");
        assert_eq!(sorted[1].identifier, "minecraft:a");
        assert_eq!(sorted[2].identifier, "minecraft:b");
    }
}

/// Plan which inventory slots satisfy a list of unit requirements.
///
/// Each requirement is `(spec, units_to_take)`; each slot has a live
/// `MatchInput` with remaining count. Returns per-requirement
/// `(slot_index, take)` assignments, or `None` when unsatisfiable.
/// Deterministic (candidate order + fewest-first + failure memo), never
/// greedy — tag overlap is searched exhaustively within the 64-slot bound.
///
/// Stage-1 note: shaped recipes plan over pattern cells as a multiset
/// (each non-empty cell takes 1). Strict grid placement stays in
/// [`match_shaped`] for callers that present a real grid; the live
/// inventory path consumes the exact multiset instead of tracking a
/// client-side crafting container.
pub fn plan_consumption(
    requirements: &[(IngredientSpec, u16)],
    slots: &[(usize, MatchInput)],
    tags: &dyn TagResolver,
    max_expansion: usize,
) -> Option<Vec<(usize, u16)>> {
    if requirements.is_empty() || requirements.len() > 64 || slots.len() > 64 {
        return None;
    }
    // Candidate slots per requirement (deterministic order).
    let mut candidates: Vec<Vec<usize>> = Vec::with_capacity(requirements.len());
    for (spec, take) in requirements {
        let mut list = Vec::new();
        for (pos, (_, input)) in slots.iter().enumerate() {
            if input.count >= *take
                && spec
                    .choices
                    .iter()
                    .any(|c| choice_matches(c, input, tags, max_expansion))
            {
                list.push(pos);
            }
        }
        if list.is_empty() {
            return None;
        }
        list.sort_unstable();
        candidates.push(list);
    }
    let mut order: Vec<usize> = (0..requirements.len()).collect();
    order.sort_by_key(|i| (candidates[*i].len(), *i));
    // DFS over ordered requirements with remaining-count tracking.
    let mut remaining: Vec<u16> = slots.iter().map(|(_, input)| input.count).collect();
    let mut assignment: Vec<Option<usize>> = vec![None; requirements.len()];
    let mut fail: std::collections::HashSet<(usize, Vec<u16>)> = std::collections::HashSet::new();

    fn dfs(
        pos: usize,
        order: &[usize],
        requirements: &[(IngredientSpec, u16)],
        candidates: &[Vec<usize>],
        remaining: &mut [u16],
        assignment: &mut [Option<usize>],
        fail: &mut std::collections::HashSet<(usize, Vec<u16>)>,
    ) -> bool {
        if pos == order.len() {
            return true;
        }
        let key = (pos, remaining.to_vec());
        if fail.contains(&key) {
            return false;
        }
        let req = order[pos];
        let take = requirements[req].1;
        for slot_pos in &candidates[req] {
            if remaining[*slot_pos] < take {
                continue;
            }
            remaining[*slot_pos] -= take;
            assignment[req] = Some(*slot_pos);
            if dfs(
                pos + 1,
                order,
                requirements,
                candidates,
                remaining,
                assignment,
                fail,
            ) {
                return true;
            }
            remaining[*slot_pos] += take;
            assignment[req] = None;
        }
        fail.insert(key);
        false
    }

    if !dfs(
        0,
        &order,
        requirements,
        &candidates,
        &mut remaining,
        &mut assignment,
        &mut fail,
    ) {
        return None;
    }
    // Map back to original requirement order with live slot indices.
    let mut out = Vec::with_capacity(requirements.len());
    for (req, take) in requirements.iter().enumerate() {
        let slot_pos = assignment[req].expect("planned requirement has a slot");
        out.push((slots[slot_pos].0, take.1));
    }
    Some(out)
}

#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::spec::IngredientSpec;

    #[test]
    fn plans_shapeless_units_deterministically() {
        let requirements = vec![
            (IngredientSpec::single_tag("minecraft:planks", 1), 1),
            (
                IngredientSpec::single_item("minecraft:oak_planks", None, 1),
                1,
            ),
        ];
        let slots = vec![
            (0, MatchInput::new("minecraft:birch_planks", 0, 1)),
            (3, MatchInput::new("minecraft:oak_planks", 0, 2)),
        ];
        let mut map = HashMap::new();
        map.insert(
            "minecraft:planks".to_string(),
            vec![
                "minecraft:oak_planks".to_string(),
                "minecraft:birch_planks".to_string(),
            ],
        );
        let tags = MapTagResolver::new(map);
        // The tag must take birch (slot 0) so oak (slot 3) stays for the
        // exact requirement — a greedy tag-first matcher would fail here
        // if it consumed oak first without backtracking.
        let plan = plan_consumption(&requirements, &slots, &tags, 1024).unwrap();
        assert_eq!(plan.len(), 2);
        let tag_take = plan[0];
        let exact_take = plan[1];
        assert_eq!(tag_take, (0, 1));
        assert_eq!(exact_take, (3, 1));
    }

    #[test]
    fn plan_fails_closed_when_unsatisfiable() {
        let requirements = vec![(IngredientSpec::single_item("minecraft:diamond", None, 2), 2)];
        let slots = vec![(0, MatchInput::new("minecraft:diamond", 0, 1))];
        let tags = MapTagResolver::empty();
        assert!(plan_consumption(&requirements, &slots, &tags, 1024).is_none());
    }
}
