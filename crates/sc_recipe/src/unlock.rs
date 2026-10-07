//! Recipe unlock: world `recipesUnlock` rule, recipe-book state and
//! fingerprint migration.
//!
//! If the world rule disables recipe unlocking, no locked recipe may be
//! force-executed by a direct client request. The minimal persistent model
//! is [`PlayerRecipeBook`] (unlocked identifiers + registry fingerprint).
//! Save scope: the book lives next to player data and is versioned by the
//! registry fingerprint; callers persist it through their existing player
//! save path (no silent bypass).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::registry::RecipeRegistryFingerprint;

/// World-level unlock policy derived from the `recipesUnlock` game rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnlockPolicy {
    /// Value of the `recipesUnlock` game rule.
    pub recipes_unlock_rule: bool,
}

impl UnlockPolicy {
    /// When the rule is on, every compiled recipe is usable without an
    /// explicit unlock entry (vanilla default for most worlds).
    pub fn all_available(&self) -> bool {
        self.recipes_unlock_rule
    }

    pub fn may_craft(
        &self,
        book: &PlayerRecipeBook,
        recipe_id: &str,
        recipe_has_unlock: bool,
    ) -> bool {
        if self.all_available() {
            return true;
        }
        // Rule off: only recipes without an `unlock` requirement, or
        // explicitly unlocked entries, may execute.
        if !recipe_has_unlock {
            return true;
        }
        book.is_unlocked(recipe_id)
    }
}

/// Minimal player recipe-book state.
///
/// `registry` records the fingerprint the book was last reconciled with;
/// a mismatch triggers [`PlayerRecipeBook::migrate`] which keeps unlocked
/// entries that still exist and drops removed ones (reported to the caller
/// so the game layer can resync the client).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlayerRecipeBook {
    pub unlocked: HashSet<String>,
    pub registry: Option<RecipeRegistryFingerprint>,
}

impl PlayerRecipeBook {
    pub fn is_unlocked(&self, recipe_id: &str) -> bool {
        self.unlocked.contains(recipe_id)
    }

    pub fn unlock(&mut self, recipe_id: impl Into<String>) {
        self.unlocked.insert(recipe_id.into());
    }

    /// Reconcile after a registry fingerprint change. Returns the list of
    /// dropped (removed-recipe) identifiers.
    pub fn migrate(
        &mut self,
        new_fingerprint: RecipeRegistryFingerprint,
        live_identifiers: &HashSet<String>,
    ) -> Vec<String> {
        if self.registry == Some(new_fingerprint) {
            return Vec::new();
        }
        let mut dropped = Vec::new();
        self.unlocked.retain(|id| {
            if live_identifiers.contains(id) {
                true
            } else {
                dropped.push(id.clone());
                false
            }
        });
        dropped.sort();
        self.registry = Some(new_fingerprint);
        dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_on_allows_everything_and_migration_drops_removed() {
        let policy = UnlockPolicy {
            recipes_unlock_rule: true,
        };
        let book = PlayerRecipeBook::default();
        assert!(policy.may_craft(&book, "minecraft:any", true));

        let policy = UnlockPolicy {
            recipes_unlock_rule: false,
        };
        let mut book = PlayerRecipeBook::default();
        assert!(!policy.may_craft(&book, "minecraft:locked", true));
        assert!(policy.may_craft(&book, "minecraft:free", false));
        book.unlock("minecraft:locked");
        assert!(policy.may_craft(&book, "minecraft:locked", true));

        let live: HashSet<String> = ["minecraft:locked".to_string()].into_iter().collect();
        let dropped = book.migrate(RecipeRegistryFingerprint([1; 32]), &live);
        assert!(dropped.is_empty());
        let live: HashSet<String> = HashSet::new();
        let dropped = book.migrate(RecipeRegistryFingerprint([2; 32]), &live);
        assert_eq!(dropped, vec!["minecraft:locked".to_string()]);
    }
}
