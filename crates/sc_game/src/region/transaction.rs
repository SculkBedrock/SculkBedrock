//! 多方事务协调器（预留接口）。
//!
//! Multi-party transaction coordinator (reserved interface).
//!
//! 必须接触多个所有者的操作（边界活塞、玩家背包加远程方块、多方块结构）
//! 不能依赖跨 region 锁，而是经过一个显式状态机：
//! Operations that must touch several owners (boundary pistons, player
//! inventory plus a remote block, multi-block structures) cannot rely on
//! cross-region locks. They go through an explicit state machine instead:
//!
//! ```text
//! Created → Accepted → Prepared(all participants)
//!                     ↘ Abort(reason)
//! Prepared → CommitDecision → Applied(all) → Publish facts/intents → Complete
//! ```
//!
//! 当前只有类型与协调器骨架。现有 gameplay 的每次提交都经由单一所有者；
//! 跨所有者事务在参与者、预留与重放日志落地前一律拒绝。
//! Types and coordinator skeleton only for now. Current gameplay routes
//! every commit through a single owner; cross-owner transactions are
//! rejected until participants, reservations and replay journals land.

use std::collections::HashMap;

/// 一个多所有者操作的稳定 id（重试时幂等）。
///
/// Stable id for one multi-owner operation (idempotent across retries).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OperationId(pub u64);

/// 协调器侧的事务状态。
///
/// Coordinator-side transaction state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionState {
    /// 已创建。
    ///
    /// Created.
    Created,
    /// 已受理。
    ///
    /// Accepted.
    Accepted,
    /// 全部参与者已准备。
    ///
    /// All participants prepared.
    Prepared,
    /// 提交决策已作出。
    ///
    /// Commit decision made.
    CommitDecision,
    /// 全部参与者已应用。
    ///
    /// All participants applied.
    Applied,
    /// 已完成。
    ///
    /// Complete.
    Complete,
    /// 已中止。
    ///
    /// Aborted.
    Abort,
}

/// 一个参与者对操作的投票。
///
/// One participant's vote for an operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticipantVote {
    /// 预留已登记并带 TTL；携带所有者纪元与目标修订号。
    ///
    /// Reservation recorded with a TTL; carries owner epoch + target
    /// revision.
    Prepared,
    /// 无法参与（原因随操作 id 一起记录日志）。
    ///
    /// Cannot participate (reason is logged with the operation id).
    Rejected,
}

/// 最小协调器：跟踪状态与投票，决定提交/中止。
///
/// Minimal coordinator: tracks states and votes, decides commit/abort.
///
/// `await` 期间不持锁；所有者在自己的 tick 上幂等应用，并向这里回报
/// `Applied(operation_id, revision)`。
/// No locks are held across `await`; owners apply idempotently on their own
/// tick and report `Applied(operation_id, revision)` back here.
#[derive(Debug, Default)]
pub struct TransactionCoordinator {
    states: HashMap<OperationId, TransactionState>,
}

impl TransactionCoordinator {
    /// 构造空协调器。
    ///
    /// Constructs an empty coordinator.
    pub fn new() -> Self {
        Self::default()
    }

    /// 受理一个操作（先预留队列/字节/完成位；准入失败时返回 `false`，
    /// 调用方必须拒绝）。
    ///
    /// Admit an operation (reserves queue/bytes/completion slots first;
    /// returns `false` when admission fails and the caller must reject).
    ///
    /// - `id`: 操作 id
    ///
    /// 当前尚未接入准入预算，一律返回 `false`。
    /// Admission budgets are not wired up yet, so this always returns
    /// `false`.
    pub fn accept(&mut self, _id: OperationId) -> bool {
        false
    }

    /// 记录一个参与者投票；返回结果事务状态。
    ///
    /// Record one participant vote; returns the resulting transaction state.
    ///
    /// - `id`: 操作 id
    /// - `vote`: 参与者投票
    ///
    /// 完整的状态机尚未接入，当前保持 `Created`。
    /// The full state machine is not wired up yet, so this stays `Created`.
    pub fn vote(&mut self, _id: OperationId, _vote: ParticipantVote) -> TransactionState {
        TransactionState::Created
    }

    /// 查询操作状态。
    ///
    /// Looks up an operation's state.
    ///
    /// - `id`: 操作 id
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
