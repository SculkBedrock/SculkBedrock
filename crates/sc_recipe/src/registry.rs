//! Immutable `RecipeRegistrySnapshot`: the frozen, shareable view the
//! game, network and workers use. Built once from [`CompileOutput`];
//! never mutated afterwards.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::compile::{
    compile_ordered, registry_fingerprint, CompileBudgets, CompileOutput, CompiledRecipe,
    SourceRecipe,
};
use crate::kind::{RecipeKind, StationKind};

/// Stable recipe index assigned in deterministic network order
/// (priority, then identifier). The network layer uses this index as the
/// recipe network id.
pub type RecipeIndex = u32;

/// Content fingerprint of the whole registry (32-byte sha256).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct RecipeRegistryFingerprint(pub [u8; 32]);

impl RecipeRegistryFingerprint {
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Opaque recipe id (the pack `description.identifier`).
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RecipeId(pub String);

impl RecipeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RecipeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Immutable snapshot shared across regions/connections.
#[derive(Clone, Debug)]
pub struct RecipeRegistrySnapshot {
    inner: Arc<SnapshotInner>,
}

#[derive(Debug)]
struct SnapshotInner {
    recipes: Vec<CompiledRecipe>,
    by_id: HashMap<String, usize>,
    /// Network order index: position in `recipes` (already sorted).
    fingerprint: RecipeRegistryFingerprint,
    disabled_count: usize,
    disabled_reasons: Vec<(String, String)>,
}

impl RecipeRegistrySnapshot {
    /// Build from already-ordered sources. Same-layer duplicate
    /// identifiers are hard errors; cross-layer collisions resolve by
    /// overwrite (see [`compile_ordered`]).
    pub fn compile(
        sources: &[SourceRecipe],
        budgets: &CompileBudgets,
        quarantine_unknown_kind: bool,
    ) -> Result<(Self, CompileOutputDiagnostics), crate::compile::CompileError> {
        let output = compile_ordered(sources, budgets, quarantine_unknown_kind)?;
        Ok(Self::from_output(output))
    }

    pub fn from_output(output: crate::compile::CompileOutput) -> (Self, CompileOutputDiagnostics) {
        let CompileOutput { recipes, disabled } = output;
        let fingerprint = RecipeRegistryFingerprint(registry_fingerprint(&recipes));
        let mut by_id = HashMap::new();
        for (index, recipe) in recipes.iter().enumerate() {
            by_id.insert(recipe.identifier.clone(), index);
        }
        let diagnostics = CompileOutputDiagnostics {
            recipe_count: recipes.len(),
            disabled: disabled.clone(),
            fingerprint,
        };
        let inner = SnapshotInner {
            recipes,
            by_id,
            fingerprint,
            disabled_count: disabled.len(),
            disabled_reasons: disabled
                .iter()
                .map(|d| (d.identifier.clone(), d.reason.clone()))
                .collect(),
        };
        (
            Self {
                inner: Arc::new(inner),
            },
            diagnostics,
        )
    }

    pub fn empty() -> Self {
        let (snapshot, _) = Self::from_output(CompileOutput::default());
        snapshot
    }

    pub fn len(&self) -> usize {
        self.inner.recipes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.recipes.is_empty()
    }

    pub fn fingerprint(&self) -> RecipeRegistryFingerprint {
        self.inner.fingerprint
    }

    pub fn get(&self, id: &str) -> Option<&CompiledRecipe> {
        self.inner.by_id.get(id).map(|i| &self.inner.recipes[*i])
    }

    pub fn get_by_index(&self, index: RecipeIndex) -> Option<&CompiledRecipe> {
        self.inner.recipes.get(index as usize)
    }

    pub fn network_index(&self, id: &str) -> Option<RecipeIndex> {
        self.inner.by_id.get(id).copied().map(|i| i as RecipeIndex)
    }

    /// Deterministic network order (priority, identifier). The slice is
    /// already in that order.
    pub fn in_network_order(&self) -> &[CompiledRecipe] {
        &self.inner.recipes
    }

    pub fn recipes_of_kind(&self, kind: RecipeKind) -> Vec<&CompiledRecipe> {
        self.inner
            .recipes
            .iter()
            .filter(|r| r.kind == kind)
            .collect()
    }

    pub fn recipes_for_station(&self, station: StationKind) -> Vec<&CompiledRecipe> {
        self.inner
            .recipes
            .iter()
            .filter(|r| r.stations.contains(&station))
            .collect()
    }

    pub fn disabled_count(&self) -> usize {
        self.inner.disabled_count
    }

    pub fn disabled_reasons(&self) -> &[(String, String)] {
        &self.inner.disabled_reasons
    }
}

/// Human-readable compile summary (counts + disabled list).
#[derive(Clone, Debug)]
pub struct CompileOutputDiagnostics {
    pub recipe_count: usize,
    pub disabled: Vec<crate::compile::DisabledRecipe>,
    pub fingerprint: RecipeRegistryFingerprint,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{CompileBudgets, SourceRecipe};

    fn src(pack: &str, order: usize, path: &str, raw: serde_json::Value) -> SourceRecipe {
        let bytes = serde_json::to_vec(&raw).unwrap();
        SourceRecipe::new(pack, order, path, "1.12", raw, &bytes)
    }

    #[test]
    fn fingerprint_is_deterministic_and_order_independent_of_input() {
        let budgets = CompileBudgets::default();
        let a = src(
            "p",
            0,
            "a.json",
            serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_shapeless": {
                    "description": {"identifier": "minecraft:b"},
                    "tags": ["crafting_table"],
                    "ingredients": [{"item": "minecraft:stone"}],
                    "result": {"item": "minecraft:out"}
                }
            }),
        );
        let b = src(
            "p",
            0,
            "b.json",
            serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_shapeless": {
                    "description": {"identifier": "minecraft:a"},
                    "tags": ["crafting_table"],
                    "ingredients": [{"item": "minecraft:dirt"}],
                    "result": {"item": "minecraft:out"}
                }
            }),
        );
        let (s1, _) =
            RecipeRegistrySnapshot::compile(&[a.clone(), b.clone()], &budgets, false).unwrap();
        let (s2, _) = RecipeRegistrySnapshot::compile(&[b, a], &budgets, false).unwrap();
        assert_eq!(s1.fingerprint(), s2.fingerprint());
        // Network order is identifier-sorted within equal priority.
        assert_eq!(s1.in_network_order()[0].identifier, "minecraft:a");
    }
}
