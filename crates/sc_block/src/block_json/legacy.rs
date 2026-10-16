//! Legacy palette support: published registry, preexisting lookup and equivalence.
//!
//! Moved verbatim from `block_json.rs` (P4 split); re-exported from [`super`]
//! so existing paths keep compiling.

use super::*;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use sc_ecs::resource::Resource;
use sc_packloader::block::LegacyPaletteEntry;

// ---------------------------------------------------------------------------
// Published snapshot resource (shared read-only across regions/worldgen, no deep copy).
// ---------------------------------------------------------------------------

/// Published block snapshot (`Arc` read-only sharing; JSON source released after build).
#[derive(Resource, Default)]
pub struct BlockJsonRegistry {
    snapshot: RwLock<Option<Arc<BlockJsonSnapshot>>>,
}

impl Clone for BlockJsonRegistry {
    fn clone(&self) -> Self {
        Self {
            snapshot: RwLock::new(self.get()),
        }
    }
}

impl BlockJsonRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publishes the snapshot (call only after all checks pass; never on failure paths).
    pub fn publish(&self, snapshot: Arc<BlockJsonSnapshot>) {
        if let Ok(mut guard) = self.snapshot.write() {
            *guard = Some(snapshot);
        }
    }

    pub fn get(&self) -> Option<Arc<BlockJsonSnapshot>> {
        self.snapshot.read().ok()?.clone()
    }

    pub fn is_present(&self) -> bool {
        self.snapshot.read().ok().is_some_and(|g| g.is_some())
    }
}

// ---------------------------------------------------------------------------
// Global dictionary coordination (production probing plus post-success registration).
// ---------------------------------------------------------------------------

/// Production existing-state probe: the global bootstrap dictionary (first-seen wins, never written here).
pub fn global_preexisting_lookup(hash: u32) -> Option<(Box<str>, Box<str>)> {
    use sc_world::block_dictionary::BlockStateDictionary;
    let entry = BlockStateDictionary::global().get(hash)?;
    let key = typed_key_for_nbt(&entry.name, entry.states.as_ref());
    Some((Box::from(entry.name.as_str()), key))
}

/// Registers snapshot states into the global dictionary after successful publish (cross-check plus superset
/// fallback). Call if and only if the whole pack succeeded; never on failure paths (no partial publish).
pub fn publish_legacy_support(snapshot: &BlockJsonSnapshot) {
    use sc_world::block_dictionary::{BlockStateDictionary, BlockStateEntry};
    let dictionary = BlockStateDictionary::global();
    for state in snapshot.states.iter() {
        let Some(type_view) = snapshot.block_type(state.type_id) else {
            continue;
        };
        dictionary.record_with_network_id(state.hash, state.network_id, || BlockStateEntry {
            name: type_view.identifier.to_string(),
            states: Some(states_nbt_of(&state.props)),
        });
    }
}

// ---------------------------------------------------------------------------
// Legacy/new data equivalence checks (migration acceptance for a pinned version).
// ---------------------------------------------------------------------------

/// Legacy/new equivalence report (compares type, hash, and sequential id per state).
#[derive(Clone, Debug, Default)]
pub struct EquivalenceReport {
    pub snapshot_states: usize,
    pub legacy_states: usize,
    pub missing_in_snapshot: Vec<String>,
    pub extra_in_snapshot: Vec<String>,
    pub hash_mismatches: Vec<String>,
    pub network_id_mismatches: Vec<String>,
}

impl EquivalenceReport {
    pub fn is_equal(&self) -> bool {
        self.missing_in_snapshot.is_empty()
            && self.extra_in_snapshot.is_empty()
            && self.hash_mismatches.is_empty()
            && self.network_id_mismatches.is_empty()
    }
}

/// Compares the snapshot against the legacy palette (per state: identifier plus typed key, hash, protocol id).
pub fn compare_legacy_snapshot(
    snapshot: &BlockJsonSnapshot,
    legacy_entries: &[LegacyPaletteEntry],
) -> EquivalenceReport {
    let mut legacy_map: HashMap<(String, Box<str>), Vec<usize>> = HashMap::new();
    for (i, entry) in legacy_entries.iter().enumerate() {
        if entry.name.is_empty() {
            continue;
        }
        let key = typed_key_for_nbt(&entry.name, entry.states.as_ref());
        legacy_map
            .entry((entry.name.clone(), key))
            .or_default()
            .push(i);
    }
    let mut report = EquivalenceReport {
        snapshot_states: snapshot.states.len(),
        legacy_states: legacy_entries.len(),
        ..Default::default()
    };
    let mut matched_legacy = vec![false; legacy_entries.len()];
    for state in snapshot.states.iter() {
        let type_view = match snapshot.block_type(state.type_id) {
            Some(t) => t,
            None => continue,
        };
        let key = typed_state_key(&type_view.identifier, &state.props);
        match legacy_map.get(&(type_view.identifier.to_string(), key)) {
            None => report
                .extra_in_snapshot
                .push(format!("{}#{:?}", type_view.identifier, state.key)),
            Some(indices) => {
                for i in indices {
                    matched_legacy[*i] = true;
                    let legacy = &legacy_entries[*i];
                    if legacy.network_id != state.network_id {
                        report.network_id_mismatches.push(format!(
                            "{}#{:?}: 快照 {} vs 旧 {}",
                            type_view.identifier, state.key, state.network_id, legacy.network_id
                        ));
                    }
                    let legacy_hash = sc_world::leveldb::block_hash::block_state_hash(
                        &legacy.name,
                        legacy.states.as_ref(),
                    );
                    if state.hash != legacy_hash {
                        report
                            .hash_mismatches
                            .push(format!("{}#{:?}", type_view.identifier, state.key));
                    }
                }
            }
        }
    }
    for (i, entry) in legacy_entries.iter().enumerate() {
        if matched_legacy[i] {
            continue;
        }
        report.missing_in_snapshot.push(format!(
            "{}#{}",
            entry.name,
            typed_key_for_nbt(&entry.name, entry.states.as_ref())
        ));
    }
    report
}