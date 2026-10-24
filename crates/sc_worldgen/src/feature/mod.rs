//! 特性规划、候选采样与区块投射。
//!
//! Feature planning, candidate sampling, and chunk projection.

pub mod candidate;
pub mod plan;
pub mod project;
pub mod structure;
pub mod tree;

pub use candidate::{CandidateSampler, FeatureCandidate};
pub use plan::{FeatureDecision, FeaturePlan, FeatureRejectReason, PlanManifest, WorldBlockWrite};
pub use project::project_feature_plan;
pub use structure::{DesertWellPalette, DesertWellSolver, SwampHutPalette, SwampHutSolver};
pub use tree::{TreeBlockPalette, TreeSolver};
