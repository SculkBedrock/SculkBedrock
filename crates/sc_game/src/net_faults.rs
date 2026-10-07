//! Unified connection-level fault handling for outbound/inbound traffic.
//!
//! The game domain isolates players as terminal states (reliable facts with
//! no authoritative resync path under hard budgets). Such states register in
//! [`PendingConnectionFaults`] by runtime id for the network domain to
//! disconnect.
//!
//! Boundaries:
//! - the game domain registers semantic isolation requests (runtime id plus
//!   stable reason) and never touches protocol packets;
//! - the set is bounded and deduplicated by runtime id;
//! - overflow is counted with rate-limited errors, never silent;
//! - requests for gone entities only count, never panic.

use std::collections::{BTreeSet, VecDeque};

use sc_ecs::resource::Resource;
use sc_log::t_log;

/// Bounded connection isolation request set (resource).
///
/// Registration order (FIFO) with dedup: repeat requests from one player
/// collapse into the first registration.
#[derive(Resource, Clone, Debug)]
pub struct PendingConnectionFaults {
    order: VecDeque<ConnectionFault>,
    pending: BTreeSet<u64>,
    max_pending: usize,
    rejected: u64,
}

/// One connection isolation request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionFault {
    /// Target player entity runtime id.
    pub runtime_id: u64,
    /// Stable machine-readable reason for logs and metrics.
    pub reason: &'static str,
}

impl PendingConnectionFaults {
    pub const DEFAULT_MAX_PENDING: usize = 1024;

    pub fn with_limits(max_pending: usize) -> Self {
        Self {
            order: VecDeque::new(),
            pending: BTreeSet::new(),
            max_pending: max_pending.max(1),
            rejected: 0,
        }
    }

    /// Register one isolation request; duplicates count as accepted.
    pub fn request(&mut self, runtime_id: u64, reason: &'static str) -> bool {
        if self.pending.contains(&runtime_id) {
            return true;
        }
        if self.order.len() >= self.max_pending {
            self.rejected = self.rejected.saturating_add(1);
            log::error!(
                "{}",
                t_log!(
                    "console.game.fault_limit",
                    max = self.max_pending,
                    entity = runtime_id,
                    reason = reason,
                    total = self.rejected
                )
            );
            return false;
        }
        self.pending.insert(runtime_id);
        self.order.push_back(ConnectionFault { runtime_id, reason });
        true
    }

    pub fn pending(&self) -> usize {
        self.order.len()
    }

    pub fn rejected(&self) -> u64 {
        self.rejected
    }

    /// Drain all requests (the caller performs isolation).
    pub fn take_all(&mut self) -> Vec<ConnectionFault> {
        self.pending.clear();
        self.order.drain(..).collect()
    }

    /// Requeue requests (when consumption fails).
    pub fn retain(&mut self, fault: ConnectionFault) {
        if self.pending.insert(fault.runtime_id) {
            self.order.push_back(fault);
        }
    }

    /// Return all requests to the queue.
    pub fn requeue_all(&mut self, faults: impl IntoIterator<Item = ConnectionFault>) {
        for fault in faults {
            self.retain(fault);
        }
    }
}

impl Default for PendingConnectionFaults {
    fn default() -> Self {
        Self::with_limits(Self::DEFAULT_MAX_PENDING)
    }
}

/// Escalate reliable facts without resync paths to connection isolation.
/// Call only when no resync path exists.
pub fn escalate_fact_to_connection_fault(
    faults: Option<&mut PendingConnectionFaults>,
    runtime_id: u64,
    reason: &'static str,
) -> bool {
    match faults {
        Some(faults) => faults.request(runtime_id, reason),
        None => {
            log::error!(
                "{}",
                t_log!(
                    "console.game.fault_no_registry",
                    entity = runtime_id,
                    reason = reason
                )
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faults_are_deduplicated_and_ordered() {
        let mut faults = PendingConnectionFaults::with_limits(8);
        assert!(faults.request(7, "first"));
        assert!(faults.request(8, "second"));
        // A duplicate must not create a second isolation.
        assert!(faults.request(7, "first"));
        assert_eq!(faults.pending(), 2);

        let taken = faults.take_all();
        assert_eq!(taken.len(), 2);
        assert_eq!(taken[0], faults_from(7, "first"));
        assert_eq!(taken[1], faults_from(8, "second"));
        assert_eq!(faults.pending(), 0);
    }

    fn faults_from(runtime_id: u64, reason: &'static str) -> ConnectionFault {
        ConnectionFault { runtime_id, reason }
    }

    #[test]
    fn the_fault_queue_is_bounded_and_reports_overflow() {
        let mut faults = PendingConnectionFaults::with_limits(2);
        assert!(faults.request(1, "a"));
        assert!(faults.request(2, "b"));
        assert!(!faults.request(3, "c"));
        assert!(!faults.request(4, "d"));
        assert_eq!(faults.rejected(), 2);
        assert_eq!(faults.pending(), 2);
    }

    #[test]
    fn requeued_faults_are_isolated_again_later() {
        let mut faults = PendingConnectionFaults::with_limits(4);
        faults.request(9, "lifecycle");
        let taken = faults.take_all();
        assert_eq!(faults.pending(), 0);
        faults.requeue_all(taken);
        assert_eq!(faults.pending(), 1);
        assert_eq!(faults.take_all()[0].runtime_id, 9);
    }

    #[test]
    fn a_missing_registry_is_reported_instead_of_ignored() {
        // No registry available: the caller learns the request was not recorded.
        assert!(!escalate_fact_to_connection_fault(None, 5, "test"));
        let mut faults = PendingConnectionFaults::default();
        assert!(escalate_fact_to_connection_fault(
            Some(&mut faults),
            5,
            "test"
        ));
        assert_eq!(faults.pending(), 1);
    }
}
