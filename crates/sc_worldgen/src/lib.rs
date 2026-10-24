//! 内置世界生成 crate（静态链接进服务端）。
//!
//! Built-in world generation crate (statically linked into the server).
//!
//! 数据流：从版本包摄入 Minecraft Java 世界生成 JSON 数据
//! （`density_function` / `noise` / `noise_settings`）→ 从选定根解析可达资源
//! 闭包（环检测、无效 JSON 拒绝、确定性内容指纹）→ 引用链接 → 编译为不可变
//! 计划 → 按种子求值 → 地形与要素（feature）生成。
//!
//! Pipeline: ingest the Minecraft Java worldgen JSON bundle from the version
//! pack, resolve the reachable resource closure from a selected root (cycle
//! detection, invalid-JSON rejection, deterministic content fingerprint),
//! link references, compile an immutable plan, evaluate per seed, and run
//! terrain and feature generation.
//!
//! 依赖规则：本 crate 不依赖插件系统或网络；只接收自有的字节 bundle，返回自有的
//! 不可变结果。缺失/不支持的资源是显式错误——这里不存在静默回退到零值或空气。
//!
//! Dependency rules: this crate never depends on the plugin system or the
//! network; it receives owned byte bundles and returns owned, immutable
//! results. Missing/unsupported resources are loud errors — nothing here
//! silently falls back to zero values or air.

pub mod compiled;
pub mod data;
pub mod eval;
pub mod exec;
pub mod feature;
pub mod link;
pub mod noise;
pub mod generator;
pub mod plan;
pub mod prepare;
pub mod router;
pub mod runtime;
pub mod sources;
pub mod terrain;

pub use compiled::{CompileError, CompiledWorldgen};
pub use data::{Axis, Json, Node, ParseError, Spline};
pub use eval::{evaluate, EvalError, NoiseSampler, OldBlendedParams, SampleContext};
pub use exec::{PlanExecutor, ScratchBuffer, ScratchPool};
pub use feature::{
    project_feature_plan, CandidateSampler, DesertWellPalette, DesertWellSolver, FeatureCandidate,
    FeatureDecision, FeaturePlan, FeatureRejectReason, PlanManifest, SwampHutPalette, SwampHutSolver,
    TreeBlockPalette, TreeSolver, WorldBlockWrite,
};
pub use generator::{BuiltinWorldGenerator, SurfaceBlockPalette};
pub use link::{link_root, LinkedRoot, LinkError, NoiseParams};
pub use plan::{DensityPlan, NodeId, PlanNode};
pub use prepare::{prepare_worldgen, ClosureNode, PreparedWorldgen, PrepareError};
pub use runtime::{ChunkEvaluator, WorldgenRuntime};
pub use sources::WorldgenSources;
pub use terrain::{BaseBlockPalette, BaseTerrainGenerator};

/// 对字节序列计算 FNV-1a 64 哈希。
///
/// FNV-1a 64 over bytes.
pub(crate) fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a64_matches_known_vectors() {
        // Standard FNV-1a 64 test vectors.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
    }
}
