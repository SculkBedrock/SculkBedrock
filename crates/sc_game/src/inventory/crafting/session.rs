//! Crafting session state: per-player revisions, queues and recipe books.
//!
//! Moved verbatim from `crafting.rs` (P4 split); re-exported from [`super`]
//! so existing paths keep compiling.

use std::collections::HashSet;
use std::sync::Arc;
use parking_lot::RwLock;
use sc_ecs::component::Component;
use sc_recipe::{
    CraftIntent, CraftQueue, CraftReject, PlayerRecipeBook, RecipeRegistryFingerprint,
    SeenOperations,
};

// ===================== components / resources =====================

/// Per-player crafting revisions. Bumped on every accepted inventory or
/// container change; client requests carry the observed values and stale
/// ones are rejected.
#[derive(Component, Clone, Debug, Default)]
pub struct CraftingRevision {
    inner: Arc<RwLock<RevisionData>>,
}

#[derive(Clone, Copy, Debug, Default)]
struct RevisionData {
    inventory: u64,
    container: u64,
    station: u64,
}

impl CraftingRevision {
    pub fn inventory(&self) -> u64 {
        self.inner.read().inventory
    }
    pub fn container(&self) -> u64 {
        self.inner.read().container
    }
    pub fn station(&self) -> u64 {
        self.inner.read().station
    }
    pub fn bump_inventory(&self) -> u64 {
        let mut data = self.inner.write();
        data.inventory = data.inventory.wrapping_add(1);
        data.inventory
    }
    pub fn bump_container(&self) -> u64 {
        let mut data = self.inner.write();
        data.container = data.container.wrapping_add(1);
        data.container
    }
    pub fn set_station(&self, revision: u64) {
        self.inner.write().station = revision;
    }
}

/// Per-player idempotency log + bounded craft queue.
#[derive(Component, Clone, Debug)]
pub struct CraftingSession {
    pub(super) seen: Arc<RwLock<SeenOperations>>,
    queue: Arc<RwLock<CraftQueue>>,
}

impl Default for CraftingSession {
    fn default() -> Self {
        Self {
            seen: Arc::new(RwLock::new(SeenOperations::new(256))),
            queue: Arc::new(RwLock::new(CraftQueue::new(32))),
        }
    }
}

impl CraftingSession {
    pub fn try_submit(&self, intent: CraftIntent) -> Result<(), CraftReject> {
        self.queue.write().try_submit(intent)
    }
    pub fn pop(&self) -> Option<CraftIntent> {
        self.queue.write().pop()
    }
    pub fn queue_len(&self) -> usize {
        self.queue.read().len()
    }
}

/// Minimal player recipe-book state (persisted next to player data).
#[derive(Component, Clone, Debug, Default)]
pub struct RecipeBookState {
    pub(super) inner: Arc<RwLock<PlayerRecipeBook>>,
}

impl RecipeBookState {
    pub fn with_book(&self, f: impl FnOnce(&PlayerRecipeBook)) {
        f(&self.inner.read());
    }
    pub fn unlock(&self, id: impl Into<String>) {
        self.inner.write().unlock(id);
    }
    pub fn migrate(
        &self,
        fingerprint: RecipeRegistryFingerprint,
        live: &HashSet<String>,
    ) -> Vec<String> {
        self.inner.write().migrate(fingerprint, live)
    }
}