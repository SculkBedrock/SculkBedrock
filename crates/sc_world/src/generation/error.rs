//! 显式的生成契约错误。
//!
//! 缺失依赖是与「没有特征」不同的状态；配置与范围违规会显式失败，
//! 而不是被静默裁剪。
//!
//! Explicit generation contract errors.
//!
//! A missing dependency is a distinct state from "no features"; configuration
//! and extent violations fail loudly instead of being silently clipped.

use std::fmt;

/// 生成身份/状态契约抛出的错误。
///
/// Errors raised by the generation identity/state contracts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerationContractError {
    /// 某次读取依赖不在已准备的快照集合中：邻域数据是缺失的，不是空气。
    ///
    /// A read dependency was not part of the prepared snapshot set:
    /// neighborhood data is absent, not air.
    MissingDependency {
        dimension: i32,
        chunk_x: i32,
        chunk_z: i32,
        stage: &'static str,
    },
    /// 展开的依赖闭包超出了配置的上界。闭包绝不会为了标记区块 Ready
    /// 而被截断。
    ///
    /// The expanded dependency closure exceeded the configured bound. The
    /// closure is never truncated to mark a chunk Ready.
    DependencyBudgetExceeded {
        target_x: i32,
        target_z: i32,
        required_cells: usize,
        budget: usize,
    },
    /// 某次特征结果超出了其已注册的覆盖范围上界。该次尝试被拒绝；
    /// 写入绝不会被静默裁剪。
    ///
    /// A feature result exceeded its registered footprint bound. The attempt
    /// is rejected; writes are never silently clipped.
    ExtentExceeded {
        feature: String,
        origin_x: i32,
        origin_z: i32,
        observed: &'static str,
        bound: i64,
    },
    /// 某个完成/attempt 结果来自已不再是当前值的 attempt；坐标相同并不会
    /// 使它有效。
    ///
    /// A completion/attempt result arrived for an attempt that is no longer
    /// current; identical coordinates do not make it valid.
    StaleAttempt {
        key_x: i32,
        key_z: i32,
        expected: u64,
        received: u64,
    },
    /// 封包来自不属于期望依赖集合的源，或在该集合已封包之后到达。
    ///
    /// A seal arrived from an origin that is not part of the expected
    /// dependency set, or after the set was already sealed.
    UnexpectedSeal { cell_x: i32, cell_z: i32, stage: &'static str },
    /// 已存世界的描述符缺失或与当前内置生成器不匹配。世界不会为生成而
    /// 打开，其数据会被保留。
    ///
    /// A stored world's descriptor is missing or does not match the current
    /// built-in generator. The world is not opened for generation and its
    /// data is preserved.
    DescriptorMismatch { detail: String },
    /// 契约所需的某个配置值缺失或无效。
    ///
    /// A configuration value required by the contracts is missing or invalid.
    InvalidConfiguration(String),
    /// 某个完成携带的所有者纪元早于当前区域纪元。
    ///
    /// A completion arrived with an older owner epoch than the current region
    /// epoch.
    StaleOwnerEpoch {
        region_x: i32,
        region_z: i32,
        expected: u64,
        received: u64,
    },
    /// 提交队列或邮箱已饱和。
    ///
    /// A submission queue or mailbox is saturated.
    MailboxSaturated {
        region_x: i32,
        region_z: i32,
        capacity: usize,
    },
    /// 提交的目标区块不属于接收方的所有者区域。
    ///
    /// A submission target chunk does not belong to the recipient owner
    /// region.
    OwnerMismatch {
        chunk_x: i32,
        chunk_z: i32,
        expected_region_x: i32,
        expected_region_z: i32,
        actual_region_x: i32,
        actual_region_z: i32,
    },
}

impl fmt::Display for GenerationContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDependency { dimension, chunk_x, chunk_z, stage } => write!(
                f,
                "missing dependency snapshot for stage {stage} at chunk ({chunk_x},{chunk_z}) in dimension {dimension}"
            ),
            Self::DependencyBudgetExceeded { target_x, target_z, required_cells, budget } => write!(
                f,
                "dependency closure for chunk ({target_x},{target_z}) needs {required_cells} cells, budget is {budget}"
            ),
            Self::ExtentExceeded { feature, origin_x, origin_z, observed, bound } => write!(
                f,
                "feature {feature} at cell ({origin_x},{origin_z}) exceeded its {observed} bound {bound}"
            ),
            Self::StaleAttempt { key_x, key_z, expected, received } => write!(
                f,
                "stale generation attempt at chunk ({key_x},{key_z}): expected {expected}, received {received}"
            ),
            Self::UnexpectedSeal { cell_x, cell_z, stage } => write!(
                f,
                "unexpected {stage} seal from cell ({cell_x},{cell_z}): not part of the expected dependency set"
            ),
            Self::DescriptorMismatch { detail } => {
                write!(f, "stored world generation descriptor mismatch: {detail}")
            }
            Self::InvalidConfiguration(detail) => {
                write!(f, "invalid generation configuration: {detail}")
            }
            Self::StaleOwnerEpoch { region_x, region_z, expected, received } => write!(
                f,
                "stale generation owner epoch at region ({region_x},{region_z}): expected {expected}, received {received}"
            ),
            Self::MailboxSaturated { region_x, region_z, capacity } => write!(
                f,
                "generation mailbox saturated at region ({region_x},{region_z}): capacity {capacity} reached"
            ),
            Self::OwnerMismatch {
                chunk_x,
                chunk_z,
                expected_region_x,
                expected_region_z,
                actual_region_x,
                actual_region_z,
            } => write!(
                f,
                "chunk ({chunk_x},{chunk_z}) owner mismatch: expected region ({expected_region_x},{expected_region_z}), delivered to ({actual_region_x},{actual_region_z})"
            ),
        }
    }
}

impl std::error::Error for GenerationContractError {}
