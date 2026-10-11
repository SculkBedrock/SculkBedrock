//! Multi-party transaction coordinator (reserved interface).
//!
//! Unified-design §4.4: operations that must touch several owners
//! (boundary pistons, player inventory plus a remote block, multi-block
//! structures) cannot rely on cross-region locks. They go through an
//! explicit state machine instead:
//!
//! ```text
//! Created → Accepted → Prepared(all participants)
//!                     ↘ Abort(reason)
//! Prepared → CommitDecision → Applied(all) → Publish facts/intents → Complete
//! ```
//!
//! Status: types and coordinator skeleton only. Current gameplay routes
//! every commit through a single owner; cross-owner transactions are
//! rejected until participants, reservations and replay journals land
//! (phases 3–4 of the implementation plan).

use std::collections::HashMap;

/// Stable id for one multi-owner operation (idempotent across retries).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OperationId(pub u64);

/// Coordinator-side transaction state (§4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionState {
    Created,
    Accepted,
    Prepared,
    CommitDecision,
    Applied,
    Complete,
    Abort,
}

/// One participant's vote for an operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticipantVote {
    /// Reservation recorded with a TTL; carries owner epoch + target revision.
    Prepared,
    /// Cannot participate (reason is logged with the operation id).
    Rejected,
}

/// Minimal coordinator: tracks states and votes, decides commit/abort.
///
/// No locks are held across `await`; owners apply idempotently on their own
/// tick and report `Applied(operation_id, revision)` back here.
#[derive(Debug, Default)]
pub struct TransactionCoordinator {
    states: HashMap<OperationId, TransactionState>,
}

impl TransactionCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit an operation (reserves queue/bytes/completion slots first;
    /// returns `false` when admission fails and the caller must reject).
    pub fn accept(&mut self, _id: OperationId) -> bool {
        // Reserved: admission budgets land with the RegionCommandQueue work.
        false
    }

    /// Record one participant vote; returns the resulting transaction state.
    pub fn vote(&mut self, _id: OperationId, _vote: ParticipantVote) -> TransactionState {
        // Reserved: full prepare/commit/abort state machine (phase 3).
        TransactionState::Created
    }

    pub fn state(&self, id: &OperationId) -> Option<TransactionState> {
        self.states.get(id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_coordinator_starts_empty() {
        let coordinator = TransactionCoordinator::new();
        assert_eq!(coordinator.state(&OperationId(1)), None);
    }
}
