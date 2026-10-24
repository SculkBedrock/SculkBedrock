//! 特性计划、决策与确定性清单。
//!
//! Feature plans, decisions, and deterministic manifests.

use sc_world::generation::identity::FeatureId;

/// 被接受特性发出的单次世界坐标方块写入。
///
/// A single block write in world coordinates emitted by an accepted feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorldBlockWrite {
    pub world_x: i32,
    pub world_y: i32,
    pub world_z: i32,
    pub runtime_id: u32,
    pub layer: u8,
}

/// 候选特性被确定性拒绝的原因。
///
/// Reason why a candidate feature was deterministically rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureRejectReason {
    InvalidFoundation(String),
    Obstructed(String),
    MissingSupport(String),
    Conflict(String),
    OutOfBounds { y: i32 },
}

/// 对候选特性的决策。
///
/// Decision on a feature candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureDecision {
    Accepted,
    Rejected(FeatureRejectReason),
}

/// 已解析、可进行区块投射的特性计划。
///
/// A resolved feature plan ready for chunk projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeaturePlan {
    pub feature_id: FeatureId,
    pub decision: FeatureDecision,
    pub min_x: i32,
    pub min_y: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_y: i32,
    pub max_z: i32,
    pub writes: Vec<WorldBlockWrite>,
}

impl FeaturePlan {
    /// 创建已接受特性计划，并计算包围盒。
    ///
    /// Creates an accepted feature plan with calculated bounding box.
    pub fn accepted(feature_id: FeatureId, mut writes: Vec<WorldBlockWrite>) -> Self {
        writes.sort_unstable();
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut min_z = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        let mut max_z = i32::MIN;

        for w in &writes {
            min_x = min_x.min(w.world_x);
            min_y = min_y.min(w.world_y);
            min_z = min_z.min(w.world_z);
            max_x = max_x.max(w.world_x);
            max_y = max_y.max(w.world_y);
            max_z = max_z.max(w.world_z);
        }

        if writes.is_empty() {
            min_x = 0; min_y = 0; min_z = 0;
            max_x = 0; max_y = 0; max_z = 0;
        }

        Self {
            feature_id,
            decision: FeatureDecision::Accepted,
            min_x,
            min_y,
            min_z,
            max_x,
            max_y,
            max_z,
            writes,
        }
    }

    /// 创建被确定性拒绝、写入数为零的特性计划。
    ///
    /// Creates a deterministically rejected feature plan with zero writes.
    pub fn rejected(feature_id: FeatureId, reason: FeatureRejectReason) -> Self {
        Self {
            feature_id,
            decision: FeatureDecision::Rejected(reason),
            min_x: 0,
            min_y: 0,
            min_z: 0,
            max_x: 0,
            max_y: 0,
            max_z: 0,
            writes: Vec::new(),
        }
    }

    pub fn is_accepted(&self) -> bool {
        matches!(self.decision, FeatureDecision::Accepted)
    }

    /// 计算本特性计划内容的确定性 FNV-1a 64 摘要。
    ///
    /// Computes a deterministic FNV-1a 64 content digest of this feature plan.
    pub fn manifest_digest(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        fn feed(h: &mut u64, bytes: &[u8]) {
            for b in bytes {
                *h ^= u64::from(*b);
                *h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }

        feed(&mut h, &self.feature_id.stable_hash().to_le_bytes());
        match &self.decision {
            FeatureDecision::Accepted => {
                feed(&mut h, &[1]);
                feed(&mut h, &self.min_x.to_le_bytes());
                feed(&mut h, &self.min_y.to_le_bytes());
                feed(&mut h, &self.min_z.to_le_bytes());
                feed(&mut h, &self.max_x.to_le_bytes());
                feed(&mut h, &self.max_y.to_le_bytes());
                feed(&mut h, &self.max_z.to_le_bytes());
                feed(&mut h, &(self.writes.len() as u64).to_le_bytes());
                for w in &self.writes {
                    feed(&mut h, &w.world_x.to_le_bytes());
                    feed(&mut h, &w.world_y.to_le_bytes());
                    feed(&mut h, &w.world_z.to_le_bytes());
                    feed(&mut h, &w.runtime_id.to_le_bytes());
                    feed(&mut h, &[w.layer]);
                }
            }
            FeatureDecision::Rejected(_) => {
                feed(&mut h, &[0]);
            }
        }
        h
    }
}

/// 计划所产出碎片的声明清单。
///
/// Declaration manifest of fragments emitted by a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanManifest {
    pub feature_id: FeatureId,
    pub manifest_digest: u64,
    pub target_chunks: Vec<(i32, i32)>,
    pub fragment_count: u32,
}
