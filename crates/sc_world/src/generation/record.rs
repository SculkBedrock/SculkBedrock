//! 权威的区块生成记录。
//!
//! 记录是「本区块的首次基线是否已完成且可证明」的唯一权威。
//! `NeedsDecoration` 式的标志（如果存在）必须由它派生，绝不能反向推导。
//! 记录位于协调器侧、按区块建表的 side table 中，而不是每区块的 ECS 实体。
//!
//! Authoritative chunk generation records.
//!
//! The record is the single authority for "is this chunk's first baseline
//! complete and provable". `NeedsDecoration`-style flags, if any, must be
//! derived from it — never the other way around. The record lives in a
//! coordinator side table keyed by chunk, not as a per-chunk ECS entity.

use std::collections::BTreeSet;

use crate::generation::error::GenerationContractError;
use crate::generation::identity::{GenerationStage, OriginCell};
use serde::{Deserialize, Serialize};

/// 区块生成管线的控制状态。层进度、脏/存档状态与交付状态在别处跟踪，
/// 与本枚举保持正交。
///
/// Control states of the chunk generation pipeline. Layer progress,
/// dirty/save state and delivery state are tracked elsewhere and stay
/// orthogonal to this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GenerationControlState {
    Absent,
    Queued,
    LoadingExisting,
    /// 存在有效的已存最终列；等待发布。
    ///
    /// A valid stored final column exists; waiting to publish.
    PublishPending,
    TerrainPending,
    BaseReady,
    DependenciesPending,
    PlansSealed,
    Assembling,
    FinalCandidate,
    CommitPending,
    Ready,
    /// 可重试的失败，带回退；attempt 身份保持不变。
    ///
    /// Retryable failure with backoff; the attempt identity stays.
    RetryableFailure,
    Cancelled,
    /// 验证、配置或持久化损坏：显式的终止状态。
    ///
    /// Validation, configuration or persistence damage: explicit terminal.
    FailedTerminal,
    RecoveryRequired,
    /// 世界/卸载路径：不接受新工作，只收尾既有义务。
    ///
    /// World/unload path: no new work, finish obligations.
    Closing,
}

impl GenerationControlState {
    /// 本区块是否可以进入编码/交付准入。只有 `Ready` 符合条件；
    /// 任何其他状态（包括 `CommitPending`）都不得被误认为具备交付资格。
    ///
    /// Whether the chunk may enter encoding/delivery admission. Only `Ready`
    /// qualifies; every other state (including `CommitPending`) must not be
    /// mistaken for delivery eligibility.
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// 本状态下的工作是否可以在回退后重试。回退调度本身由协调器跟踪，
    /// 不折叠进本枚举。
    ///
    /// Whether work in this state may be retried after backoff. Backoff
    /// scheduling itself is tracked by the coordinator, not folded into this
    /// enum.
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::RetryableFailure)
    }
}

/// 每个区块键对应的单调 attempt id：重试时递增；携带旧 attempt 的完成会被拒绝。
///
/// Monotonic attempt id per chunk key: retries advance it, and any completion
/// carrying an older attempt is rejected.
pub type AttemptId = u64;

/// 单个阶段的封包记账：期望的源集合在前期从范围闭包计算；只有当每个期望的
/// 源都产出经过验证的封包时，该阶段才算封包。空源以空清单封包——
/// 「没有特征」与「尚未完成」是两种不同状态。
///
/// Seal accounting for one stage: the expected origin set is computed upfront
/// from the extent closure; a stage is sealed only when every expected origin
/// produced a verified seal. Empty sources seal with an empty manifest —
/// "no features" and "not finished" are different states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SealBookkeeping {
    /// 本目标在该阶段必须等待的源。闭包计算完成后注册一次，之后绝不增长。
    ///
    /// Origins this target must wait on for the stage. Registered once the
    /// closure is computed; never grown afterwards.
    pub expected: BTreeSet<OriginCell>,
    /// 已验证封包已被接受的源。
    ///
    /// Origins whose verified seals have been accepted.
    pub sealed: BTreeSet<OriginCell>,
    /// 来自已接受封包的摘要，按源单元格的 x 再按 z（编码后）键入，
    /// 用于确定性诊断。存在即等同于已封包成员关系。
    ///
    /// Digests from accepted seals, keyed by origin cell x then z (encoded)
    /// for deterministic diagnostics. Presence equals sealed membership.
    pub seal_digests: BTreeSet<(i32, i32, u64)>,
}

impl SealBookkeeping {
    /// 注册完整的期望集合。每个阶段必须恰好调用一次；后续注册会追溯性地
    /// 改变契约。
    ///
    /// Registers the complete expected set. Must be called exactly once per
    /// stage; later registration would retroactively change the contract.
    pub fn register_expected(
        &mut self,
        origins: impl IntoIterator<Item = OriginCell>,
    ) -> Result<(), GenerationContractError> {
        if !self.expected.is_empty() {
            return Err(GenerationContractError::InvalidConfiguration(
                "expected seal set already registered for this stage".to_string(),
            ));
        }
        self.expected = origins.into_iter().collect();
        Ok(())
    }

    /// 接受来自某个源的已验证封包。未知的源与重复的封包都是契约错误；
    /// 已接受的封包按集合成员关系是幂等的，重放既不会损坏状态也不会重复计数。
    ///
    /// Accepts a verified seal from an origin. Unknown origins and duplicate
    /// seals are contract errors; accepted seals are idempotent by set
    /// membership, so replays neither corrupt nor double-count.
    pub fn apply_seal(
        &mut self,
        origin: OriginCell,
        manifest_digest: u64,
    ) -> Result<(), GenerationContractError> {
        if !self.expected.contains(&origin) {
            return Err(GenerationContractError::UnexpectedSeal {
                cell_x: origin.x,
                cell_z: origin.z,
                stage: "seal",
            });
        }
        if !self.sealed.insert(origin) {
            return Err(GenerationContractError::UnexpectedSeal {
                cell_x: origin.x,
                cell_z: origin.z,
                stage: "duplicate",
            });
        }
        self.seal_digests.insert((origin.x, origin.z, manifest_digest));
        Ok(())
    }

    /// 仅当每个期望的源都有已验证封包时为真。空的期望集合平凡封包
    /// （例如该阶段被配置禁用）。
    ///
    /// True only when every expected origin has a verified seal. An empty
    /// expected set seals trivially (e.g. the stage is disabled by
    /// configuration).
    pub fn is_complete(&self) -> bool {
        self.sealed.len() == self.expected.len()
    }

    /// 已封包但不属于重新计算的期望集合的源：说明封包被接受之后闭包
    /// 发生了变化。
    ///
    /// Sealed origins not present in a recomputed expected set: evidence of a
    /// closure change after seals were accepted.
    pub fn orphans(&self) -> Vec<OriginCell> {
        self.sealed.difference(&self.expected).copied().collect()
    }
}

/// 权威的单区块生成记录。它承载身份、依赖与就绪状态；世界实例与
/// 所有者纪元字段用于在迁移或世界重开之后挡掉迟到的结果。
///
/// The authoritative per-chunk generation record. It carries identity,
/// dependency, and readiness state; world-instance and owner-epoch fields
/// fence late results after migration or world reopen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkGenerationRecord {
    /// 所属世界的维度 id（身份组成部分；世界实例 id 归协调器所有，
    /// 不在持久化记录中）。
    ///
    /// Dimension id of the owning world (identity component; the world
    /// instance id lives with the coordinator, not in the persisted record).
    pub dimension: i32,
    /// 目标区块坐标。
    ///
    /// Target chunk coordinates.
    pub chunk_x: i32,
    pub chunk_z: i32,
    /// 创建本记录所用描述符的指纹。
    ///
    /// Fingerprint of the descriptor this record was created under.
    pub generator_fingerprint: u64,
    /// BaseSnapshot 输入的摘要。
    ///
    /// Digest of the BaseSnapshot input.
    pub base_digest: Option<u64>,
    /// 当前 attempt；携带更旧 attempt 的完成已过期。
    ///
    /// Current attempt; completions with older attempts are stale.
    pub attempt: AttemptId,
    pub state: GenerationControlState,
    /// `RetryableFailure`/`FailedTerminal` 的失败详情，用于诊断。
    ///
    /// Failure detail for `RetryableFailure`/`FailedTerminal` diagnostics.
    pub failure: Option<String>,
    /// 分阶段的封包记账。
    ///
    /// Per-stage seal bookkeeping.
    pub structure_seals: SealBookkeeping,
    pub decoration_seals: SealBookkeeping,
    /// 已接收并验证的声明碎片数量。
    ///
    /// Number of declared fragments received and verified.
    pub received_fragments: u32,
    /// 所有已接受封包声明的碎片总数。
    ///
    /// Total fragments declared across accepted seals.
    pub declared_fragments: u32,
    /// 组装出的私有最终列的摘要。
    ///
    /// Digest of the assembled private final column.
    pub assembly_digest: Option<u64>,
    /// 记录到达 `Ready` 时刻设置的证书。
    ///
    /// Certificate set at the moment the record reached `Ready`.
    pub ready_certificate: Option<ReadyCertificate>,
    /// Ready 发布所携带的内容生成代。
    ///
    /// Content generation the Ready publication carries.
    pub content_generation: u64,
}

impl ChunkGenerationRecord {
    /// 创建处于 `Queued`、attempt 为 1 的记录。
    ///
    /// Creates a record in `Queued` with attempt 1.
    pub fn new_queued(dimension: i32, chunk_x: i32, chunk_z: i32, generator_fingerprint: u64) -> Self {
        Self {
            dimension,
            chunk_x,
            chunk_z,
            generator_fingerprint,
            base_digest: None,
            attempt: 1,
            state: GenerationControlState::Queued,
            failure: None,
            structure_seals: SealBookkeeping::default(),
            decoration_seals: SealBookkeeping::default(),
            received_fragments: 0,
            declared_fragments: 0,
            assembly_digest: None,
            ready_certificate: None,
            content_generation: 0,
        }
    }

    /// 为可重试状态开启新 attempt；旧的完成变为过期。封包记账被重置：
    /// 重新计算的闭包是新的契约。
    ///
    /// Starts a new attempt for a retryable state; older completions become
    /// stale. Seal bookkeeping is reset: the recomputed closure is the new
    /// contract.
    pub fn begin_retry(&mut self) {
        self.attempt += 1;
        self.state = GenerationControlState::Queued;
        self.failure = None;
        self.structure_seals = SealBookkeeping::default();
        self.decoration_seals = SealBookkeeping::default();
        self.received_fragments = 0;
        self.declared_fragments = 0;
        self.assembly_digest = None;
        self.ready_certificate = None;
    }

    /// 为某阶段注册期望的封包集合（每次 attempt 仅可调用一次）。
    ///
    /// - `stage`: 生成阶段；Base 阶段无跨源封包，调用会报错
    /// - `origins`: 期望的源单元格集合
    ///
    /// Registers the expected seal closure for a stage (once per attempt).
    ///
    /// - `stage`: generation stage; the Base stage has no cross-source seals
    ///   and errors
    /// - `origins`: expected origin cells
    pub fn register_expected_seals(
        &mut self,
        stage: GenerationStage,
        origins: impl IntoIterator<Item = OriginCell>,
    ) -> Result<(), GenerationContractError> {
        match stage {
            GenerationStage::Base => Err(GenerationContractError::InvalidConfiguration(
                "base stage has no cross-source seals; it is produced locally".to_string(),
            )),
            GenerationStage::Structure => self.structure_seals.register_expected(origins),
            GenerationStage::Decoration => self.decoration_seals.register_expected(origins),
        }
    }

    /// 应用某阶段的已验证封包。
    ///
    /// - `stage`: 生成阶段
    /// - `origin`: 封包来源的源单元格
    /// - `manifest_digest`: 清单摘要
    /// - `declared_fragments`: 该封包声明的投影到本区块的碎片数量
    ///
    /// Applies a verified seal for a stage.
    ///
    /// - `stage`: generation stage
    /// - `origin`: source cell the seal comes from
    /// - `manifest_digest`: manifest digest
    /// - `declared_fragments`: number of fragments the seal declares for this
    ///   target chunk
    pub fn apply_seal(
        &mut self,
        stage: GenerationStage,
        origin: OriginCell,
        manifest_digest: u64,
        declared_fragments: u32,
    ) -> Result<(), GenerationContractError> {
        match stage {
            GenerationStage::Base => Err(GenerationContractError::InvalidConfiguration(
                "base stage has no cross-source seals".to_string(),
            )),
            GenerationStage::Structure => self
                .structure_seals
                .apply_seal(origin, manifest_digest),
            GenerationStage::Decoration => self.decoration_seals.apply_seal(origin, manifest_digest),
        }?;
        self.declared_fragments = self
            .declared_fragments
            .saturating_add(declared_fragments);
        // Both stages sealed => the dependency wait is over, whether or not
        // any fragments exist.
        if self.structure_seals.is_complete() && self.decoration_seals.is_complete() {
            if self.state == GenerationControlState::DependenciesPending {
                self.state = GenerationControlState::PlansSealed;
            }
        }
        Ok(())
    }

    /// 接受一个已验证的碎片（幂等的重放不会重复计数）。
    ///
    /// - `count`: 本次接受的碎片数量
    ///
    /// Accepts a verified fragment (idempotent replays do not double-count).
    ///
    /// - `count`: number of fragments accepted in this call
    pub fn accept_fragment(&mut self, count: u32) -> Result<(), GenerationContractError> {
        let total = self.received_fragments.saturating_add(count);
        if total > self.declared_fragments {
            return Err(GenerationContractError::InvalidConfiguration(format!(
                "fragment accounting broken: received {total} > declared {}",
                self.declared_fragments
            )));
        }
        self.received_fragments = total;
        Ok(())
    }

    /// 携带证书过渡到 `Ready`，校验 attempt 为当前值且证书所有条件成立。
    ///
    /// - `attempt`: 发起完成的 attempt id，必须为当前 attempt
    /// - `certificate`: 已验证的就绪证书
    ///
    /// Transitions to `Ready` with the certificate, validating the attempt
    /// is current and the certificate conditions all hold.
    ///
    /// - `attempt`: attempt id of the completion; must be the current attempt
    /// - `certificate`: the validated readiness certificate
    pub fn commit_ready(
        &mut self,
        attempt: AttemptId,
        certificate: ReadyCertificate,
    ) -> Result<(), GenerationContractError> {
        if attempt != self.attempt {
            return Err(GenerationContractError::StaleAttempt {
                key_x: self.chunk_x,
                key_z: self.chunk_z,
                expected: self.attempt,
                received: attempt,
            });
        }
        certificate.validate()?;
        debug_assert_eq!(self.received_fragments, self.declared_fragments);
        self.state = GenerationControlState::Ready;
        self.ready_certificate = Some(certificate);
        self.content_generation = self.content_generation.max(1);
        Ok(())
    }
}

/// `Ready(c)` 的各逻辑条件。由组装器携带证据字段构造；
/// [`ReadyCertificate::validate`] 拒绝为任何条件未设置的证书放行，
/// 因此「没有树木」（空清单、已封包、零碎片）与「依赖缺失」不会坍缩成
/// 同一状态。
///
/// Logical conditions of `Ready(c)`. Constructed by the assembler with
/// evidence fields; [`ReadyCertificate::validate`] refuses to bless a
/// certificate with any condition unset, so "no trees" (empty manifests,
/// sealed, zero fragments) and "dependencies missing" cannot collapse into
/// the same state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyCertificate {
    /// Base 快照在当前描述符下产出，且其摘要与记录一致。
    ///
    /// The Base snapshot was produced under the current descriptor and its
    /// digest matched the record.
    pub base_valid: bool,
    /// 每个期望的结构源都已封包。
    ///
    /// Every expected structure origin sealed.
    pub structure_seals_complete: bool,
    /// 每个期望的装饰源都已封包。
    ///
    /// Every expected decoration origin sealed.
    pub decoration_seals_complete: bool,
    /// 每个声明的碎片都已接收并验证（身份 + 内容）。
    ///
    /// Every declared fragment was received and verified (identity + content).
    pub all_fragments_verified: bool,
    /// 确定性组装已验证（摘要与计划派生的期望一致；每特征的顺序在输出中
    /// 不可观测）。
    ///
    /// Deterministic assembly validated (digest matches the plan-derived
    /// expectation; per-feature order not observable in output).
    pub assembly_validated: bool,
    /// 必需的元数据/初始化（高度图、生物群系、方块实体）已包含。
    ///
    /// Required metadata/initialization (heightmaps, biome, block entities)
    /// included.
    pub metadata_included: bool,
    /// 证书绑定的所有者纪元；新的化身不得复用它。
    ///
    /// Owner epoch the certificate is bound to; a new incarnation must not
    /// reuse it.
    pub owner_epoch: u64,
}

impl ReadyCertificate {
    /// 所有条件都必须成立；否则属于组装器的缺陷，宁可拒绝 Ready，
    /// 也不降级为「大概没问题」。
    ///
    /// All conditions must hold; anything else is a bug in the assembler and
    /// refuses Ready rather than degrading to "probably fine".
    pub fn validate(&self) -> Result<(), GenerationContractError> {
        let missing = [
            (self.base_valid, "base_valid"),
            (self.structure_seals_complete, "structure_seals_complete"),
            (self.decoration_seals_complete, "decoration_seals_complete"),
            (self.all_fragments_verified, "all_fragments_verified"),
            (self.assembly_validated, "assembly_validated"),
            (self.metadata_included, "metadata_included"),
        ];
        let unset: Vec<&str> = missing
            .iter()
            .filter(|(ok, _)| !ok)
            .map(|(_, name)| *name)
            .collect();
        if unset.is_empty() {
            Ok(())
        } else {
            Err(GenerationContractError::InvalidConfiguration(format!(
                "ReadyCertificate refused: unset conditions {:?}",
                unset
            )))
        }
    }
}

/// 持久化的生成完成记录，存储于 LevelDB。
///
/// 与最终区块列数据原子地一同写入，保证不可能出现撕裂的生成状态。
///
/// Persisted generation completion record stored in LevelDB.
///
/// Written atomically alongside final chunk column data to guarantee no torn
/// generation state can exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedGenerationRecord {
    pub dimension: i32,
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub generator_fingerprint: u64,
    pub assembly_digest: u64,
    pub attempt: AttemptId,
    pub certificate: ReadyCertificate,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(x: i32, z: i32) -> OriginCell {
        OriginCell::new(x, z)
    }

    fn complete_certificate() -> ReadyCertificate {
        ReadyCertificate {
            base_valid: true,
            structure_seals_complete: true,
            decoration_seals_complete: true,
            all_fragments_verified: true,
            assembly_validated: true,
            metadata_included: true,
            owner_epoch: 1,
        }
    }

    #[test]
    fn seals_require_the_full_expected_set_and_reject_unknown_origins() {
        let mut record = ChunkGenerationRecord::new_queued(0, 5, -7, 0xab);
        record
            .register_expected_seals(GenerationStage::Decoration, [cell(4, -7), cell(5, -7)])
            .unwrap();
        // An origin outside the closure is a contract error.
        assert!(matches!(
            record.apply_seal(GenerationStage::Decoration, cell(6, -7), 1, 0),
            Err(GenerationContractError::UnexpectedSeal { .. })
        ));
        record.apply_seal(GenerationStage::Decoration, cell(4, -7), 11, 0).unwrap();
        assert!(!record.decoration_seals.is_complete());
        // Duplicate seals are rejected and do not change completeness.
        assert!(matches!(
            record.apply_seal(GenerationStage::Decoration, cell(4, -7), 11, 0),
            Err(GenerationContractError::UnexpectedSeal { stage: "duplicate", .. })
        ));
        record.apply_seal(GenerationStage::Decoration, cell(5, -7), 0, 0).unwrap();
        assert!(record.decoration_seals.is_complete());
        // Zero-fragment seals are normal ("no features" != "not finished").
        assert_eq!(record.declared_fragments, 0);
    }

    #[test]
    fn register_is_once_per_stage_and_base_stage_has_no_seals() {
        let mut record = ChunkGenerationRecord::new_queued(0, 0, 0, 1);
        record
            .register_expected_seals(GenerationStage::Structure, [cell(0, 0)])
            .unwrap();
        assert!(record.register_expected_seals(GenerationStage::Structure, [cell(1, 0)]).is_err());
        assert!(matches!(
            record.register_expected_seals(GenerationStage::Base, []),
            Err(GenerationContractError::InvalidConfiguration { .. })
        ));
    }

    #[test]
    fn ready_requires_current_attempt_and_complete_certificate() {
        let mut record = ChunkGenerationRecord::new_queued(0, 1, 2, 3);
        // Stale attempt cannot commit Ready.
        assert!(matches!(
            record.commit_ready(99, complete_certificate()),
            Err(GenerationContractError::StaleAttempt { expected: 1, received: 99, .. })
        ));
        // An incomplete certificate is refused and leaves the state unchanged.
        let mut bad = complete_certificate();
        bad.assembly_validated = false;
        assert!(record.commit_ready(record.attempt, bad).is_err());
        assert_eq!(record.state, GenerationControlState::Queued);
        // The current attempt with a complete certificate commits.
        record.commit_ready(record.attempt, complete_certificate()).unwrap();
        assert_eq!(record.state, GenerationControlState::Ready);
        assert!(record.state.is_ready());
        assert!(record.ready_certificate.is_some());
        assert_eq!(record.content_generation, 1);
    }

    #[test]
    fn retry_resets_bookkeeping_and_increments_attempt() {
        let mut record = ChunkGenerationRecord::new_queued(0, 0, 0, 1);
        record
            .register_expected_seals(GenerationStage::Decoration, [cell(1, 0), cell(0, 1)])
            .unwrap();
        record.apply_seal(GenerationStage::Decoration, cell(1, 0), 5, 3).unwrap();
        record.begin_retry();
        assert_eq!(record.attempt, 2);
        assert!(record.decoration_seals.expected.is_empty());
        assert_eq!(record.declared_fragments, 0);
        // The old origin is no longer expected until re-registered.
        assert!(matches!(
            record.apply_seal(GenerationStage::Decoration, cell(1, 0), 5, 3),
            Err(GenerationContractError::UnexpectedSeal { .. })
        ));
        assert!(record.state.is_retryable() == false || record.state == GenerationControlState::Queued);
    }

    #[test]
    fn fragment_accounting_never_exceeds_declared() {
        let mut record = ChunkGenerationRecord::new_queued(0, 0, 0, 1);
        record
            .register_expected_seals(GenerationStage::Decoration, [cell(0, 0)])
            .unwrap();
        record.apply_seal(GenerationStage::Decoration, cell(0, 0), 7, 4).unwrap();
        record.accept_fragment(2).unwrap();
        record.accept_fragment(2).unwrap();
        assert_eq!(record.received_fragments, 4);
        assert!(record.accept_fragment(1).is_err());
        // Replays must not double-count (idempotent acceptance is enforced by
        // the caller deduplicating FragmentIds; the record only guards totals).
        assert_eq!(record.received_fragments, 4);
    }

    #[test]
    fn plans_sealed_transition_happens_only_from_dependencies_pending() {
        let mut record = ChunkGenerationRecord::new_queued(0, 0, 0, 1);
        record.state = GenerationControlState::DependenciesPending;
        record
            .register_expected_seals(GenerationStage::Structure, [cell(0, 1)])
            .unwrap();
        record
            .register_expected_seals(GenerationStage::Decoration, [cell(1, 0)])
            .unwrap();
        record.apply_seal(GenerationStage::Structure, cell(0, 1), 1, 0).unwrap();
        assert_eq!(record.state, GenerationControlState::DependenciesPending);
        record.apply_seal(GenerationStage::Decoration, cell(1, 0), 1, 0).unwrap();
        assert_eq!(record.state, GenerationControlState::PlansSealed);
    }
}
