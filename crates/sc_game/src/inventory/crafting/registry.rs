//! Shared crafting registries: frozen recipe snapshot and item tags.
//!
//! Moved verbatim from `crafting.rs` (P4 split); re-exported from [`super`]
//! so existing paths keep compiling.

use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use sc_ecs::resource::Resource;
use sc_recipe::{MapTagResolver, RecipeRegistryFingerprint, RecipeRegistrySnapshot};

/// Frozen recipe registry shared by all regions (read-only after load).
#[derive(Resource, Clone, Debug)]
pub struct SharedRecipeRegistry {
    snapshot: Arc<RwLock<RecipeRegistrySnapshot>>,
}

impl Default for SharedRecipeRegistry {
    fn default() -> Self {
        Self::new(RecipeRegistrySnapshot::empty())
    }
}

impl SharedRecipeRegistry {
    pub fn new(snapshot: RecipeRegistrySnapshot) -> Self {
        Self {
            snapshot: Arc::new(RwLock::new(snapshot)),
        }
    }
    pub fn snapshot(&self) -> RecipeRegistrySnapshot {
        self.snapshot.read().clone()
    }
    pub fn replace(&self, snapshot: RecipeRegistrySnapshot) {
        *self.snapshot.write() = snapshot;
    }
    pub fn fingerprint(&self) -> RecipeRegistryFingerprint {
        self.snapshot.read().fingerprint()
    }
}

/// Item-tag table for `{"tag": ...}` ingredients (fail-closed when absent).
#[derive(Resource, Clone, Debug, Default)]
pub struct SharedItemTags {
    inner: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl SharedItemTags {
    pub fn new(map: HashMap<String, Vec<String>>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(map)),
        }
    }
    pub fn resolver(&self) -> MapTagResolver {
        MapTagResolver::new(self.inner.read().clone())
    }
    pub fn replace(&self, map: HashMap<String, Vec<String>>) {
        *self.inner.write() = map;
    }
}