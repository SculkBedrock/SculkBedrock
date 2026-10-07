use std::collections::{HashMap, HashSet};

use rusty_leveldb::{LdbIterator, WriteBatch, DB};

use super::format::{
    decode_spillover_journal_value, encode_spillover_journal_value, parse_spillover_journal_key,
    spillover_journal_key, spillover_journal_target_prefix, SPILLOVER_JOURNAL_NAMESPACE,
};
use crate::chunk::{dimension_bounds, ChunkPosition};
use crate::storage::{
    merge_spillover_entries, ChunkKey, SpilloverJournalEntry, SpilloverOperationId,
    WorldStorageError, MAX_GENERATED_SPILLOVER_WRITES, MAX_STORED_SPILLOVER_WRITES,
};

/// Logical record limits, reconstructed at open and changed only after a
/// successful DB batch. Values/keys are fixed-size; this is not a DB RSS limit.
#[derive(Default)]
pub(super) struct SpilloverJournal {
    total: usize,
    targets: HashMap<ChunkKey, usize>,
}

fn backend(status: impl std::fmt::Display) -> WorldStorageError {
    WorldStorageError::Backend(format!("spillover journal: {status}"))
}

fn decode(key: &[u8], value: &[u8]) -> Result<SpilloverJournalEntry, WorldStorageError> {
    let (target, operation_id) = parse_spillover_journal_key(key).ok_or_else(|| {
        WorldStorageError::Unsupported("invalid or unsupported spillover journal key".into())
    })?;
    let write = decode_spillover_journal_value(target, value)
        .ok_or_else(|| WorldStorageError::Corrupt("invalid spillover journal value".into()))?;
    let (min_y, max_y) = dimension_bounds(target.dimension);
    if operation_id.source.dimension != target.dimension
        || ChunkPosition::from_world(write.x, write.z) != target.position
        || write.layer > 1
        || !(min_y..=max_y).contains(&write.y)
    {
        return Err(WorldStorageError::Corrupt(format!(
            "invalid spillover coordinates for operation {operation_id:?}"
        )));
    }
    Ok(SpilloverJournalEntry {
        operation_id,
        write,
    })
}

pub(super) fn read_target(
    db: &mut DB,
    target: ChunkKey,
) -> Result<Vec<SpilloverJournalEntry>, WorldStorageError> {
    let prefix = spillover_journal_target_prefix(target);
    let mut iterator = db.new_iter().map_err(backend)?;
    iterator.seek(&prefix);
    let mut entries = Vec::new();
    let mut key = Vec::new();
    let mut value = Vec::new();
    while iterator.current(&mut key, &mut value) && key.starts_with(&prefix) {
        if entries.len() >= MAX_GENERATED_SPILLOVER_WRITES {
            return Err(WorldStorageError::Capacity(
                "target journal replay limit exceeded".into(),
            ));
        }
        entries.push(decode(&key, &value)?);
        if !iterator.advance() {
            break;
        }
    }
    entries.sort_unstable_by_key(|entry| entry.operation_id);
    Ok(entries)
}

impl SpilloverJournal {
    pub(super) fn open(db: &mut DB) -> Result<Self, WorldStorageError> {
        let mut journal = Self::default();
        let mut iterator = db.new_iter().map_err(backend)?;
        iterator.seek(SPILLOVER_JOURNAL_NAMESPACE);
        let mut key = Vec::new();
        let mut value = Vec::new();
        while iterator.current(&mut key, &mut value) && key.starts_with(SPILLOVER_JOURNAL_NAMESPACE)
        {
            let entry = decode(&key, &value)?;
            let target_count = journal.targets.entry(entry.write.key).or_default();
            if journal.total >= MAX_STORED_SPILLOVER_WRITES
                || *target_count >= MAX_GENERATED_SPILLOVER_WRITES
            {
                return Err(WorldStorageError::Capacity(
                    "stored spillover journal exceeds its limits".into(),
                ));
            }
            *target_count += 1;
            journal.total += 1;
            if !iterator.advance() {
                break;
            }
        }
        Ok(journal)
    }

    pub(super) fn prepare_append(
        &self,
        db: &mut DB,
        batch: &mut WriteBatch,
        entries: &[SpilloverJournalEntry],
    ) -> Result<HashMap<ChunkKey, usize>, WorldStorageError> {
        if entries.len() > MAX_GENERATED_SPILLOVER_WRITES {
            return Err(WorldStorageError::Capacity(
                "spillover journal batch limit exceeded".into(),
            ));
        }
        let entries = merge_spillover_entries(entries.iter().copied())?;
        let snapshot = db.get_snapshot();
        let mut additions: HashMap<ChunkKey, usize> = HashMap::new();
        for entry in &entries {
            let key = spillover_journal_key(entry.write.key, entry.operation_id);
            let value = encode_spillover_journal_value(entry.write);
            decode(&key, &value)?;
            if let Some(existing) = db.get_at(&snapshot, &key).map_err(backend)? {
                if existing != value {
                    return Err(WorldStorageError::Corrupt(format!(
                        "spillover operation id reused with different payload: {:?}",
                        entry.operation_id
                    )));
                }
            } else {
                *additions.entry(entry.write.key).or_default() += 1;
                batch.put(&key, &value);
            }
        }
        let added = additions.values().sum::<usize>();
        if self.total.saturating_add(added) > MAX_STORED_SPILLOVER_WRITES
            || additions.iter().any(|(target, count)| {
                self.targets
                    .get(target)
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(*count)
                    > MAX_GENERATED_SPILLOVER_WRITES
            })
        {
            return Err(WorldStorageError::Capacity(
                "spillover journal is full; generation deferred".into(),
            ));
        }
        Ok(additions)
    }

    pub(super) fn prepare_ack(
        &self,
        db: &mut DB,
        batch: &mut WriteBatch,
        target: ChunkKey,
        acknowledgements: &[SpilloverOperationId],
    ) -> Result<usize, WorldStorageError> {
        if acknowledgements.len() > MAX_GENERATED_SPILLOVER_WRITES {
            return Err(WorldStorageError::Capacity(
                "spillover acknowledgement limit exceeded".into(),
            ));
        }
        let snapshot = db.get_snapshot();
        let mut seen = HashSet::new();
        let mut removed = 0;
        for &operation_id in acknowledgements {
            if !seen.insert(operation_id) {
                continue;
            }
            let key = spillover_journal_key(target, operation_id);
            if db.get_at(&snapshot, &key).map_err(backend)?.is_some() {
                batch.delete(&key);
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub(super) fn committed(
        &mut self,
        additions: HashMap<ChunkKey, usize>,
        target: ChunkKey,
        removed: usize,
    ) {
        self.total = self.total.saturating_sub(removed);
        if let Some(count) = self.targets.get_mut(&target) {
            *count = count.saturating_sub(removed);
            if *count == 0 {
                self.targets.remove(&target);
            }
        }
        for (target, count) in additions {
            self.total += count;
            *self.targets.entry(target).or_default() += count;
        }
    }
}
