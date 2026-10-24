//! 已编译世界生成资产：单个世界维度的、完全解析且不可变的预编译世界生成资产。
//!
//! CompiledWorldgen: fully resolved, immutable, pre-compiled worldgen assets
//! for one world dimension.
//!
//! 封装内容：
//!
//! Encapsulates:
//! - 解析后的闭包与内容指纹（[`PreparedWorldgen`]）；
//! - 链接后的噪声设置与参数定义（[`LinkedNoiseSettings`]）；
//! - 不可变的、经剪枝的类字节码密度计划（[`DensityPlan`]）；
//! - 用于产出显式 [`GeneratorDescriptor`] 与按种子隔离的 [`WorldgenRuntime`]
//!   （配备有界暂存池）的工厂方法。
//!
//! - The resolved closure and content fingerprint ([`PreparedWorldgen`]);
//! - The linked noise settings and parameter definitions ([`LinkedNoiseSettings`]);
//! - The immutable, pruned bytecode-like density plan ([`DensityPlan`]);
//! - Factory methods to produce explicit [`GeneratorDescriptor`]s and
//!   per-seed [`WorldgenRuntime`]s with bounded scratch pools.

use std::fmt;
use std::sync::Arc;

use sc_world::generation::descriptor::{CellSettings, GeneratorDescriptor};

use crate::link::{link_noise_settings, LinkError, LinkedNoiseSettings};
use crate::plan::DensityPlan;
use crate::prepare::{prepare_worldgen, PrepareError, PreparedWorldgen};
use crate::runtime::WorldgenRuntime;
use crate::sources::WorldgenSources;

/// 世界生成编译过程中产生的错误。
///
/// Errors arising during worldgen compilation.
#[derive(Debug)]
pub enum CompileError {
    Prepare(PrepareError),
    Link(LinkError),
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Prepare(err) => write!(f, "worldgen prepare failed: {err}"),
            Self::Link(err) => write!(f, "worldgen link failed: {err}"),
        }
    }
}

impl std::error::Error for CompileError {}

impl From<PrepareError> for CompileError {
    fn from(err: PrepareError) -> Self {
        Self::Prepare(err)
    }
}

impl From<LinkError> for CompileError {
    fn from(err: LinkError) -> Self {
        Self::Link(err)
    }
}

/// 单个维度根的、完整编译且不可变的世界生成资产。
///
/// Fully compiled, immutable world generation asset for a dimension root.
#[derive(Debug, Clone)]
pub struct CompiledWorldgen {
    root: String,
    prepared: PreparedWorldgen,
    settings: LinkedNoiseSettings,
    plan: Arc<DensityPlan>,
}

impl CompiledWorldgen {
    /// 从原始源为指定的 noise_settings 根（如 `"minecraft:overworld"`）
    /// 编译世界生成资产。
    ///
    /// 执行闭包解析、引用校验、路由链接，并将所有路由根编译进一棵共享的、
    /// 经剪枝的 [`DensityPlan`]。
    ///
    /// Compiles worldgen assets from raw sources for the given noise_settings root
    /// (e.g. `"minecraft:overworld"`).
    ///
    /// Performs closure resolution, reference validation, router linking, and
    /// compiling all router roots into a shared, pruned [`DensityPlan`].
    pub fn compile(sources: &WorldgenSources, root: &str) -> Result<Self, CompileError> {
        let prepared = prepare_worldgen(sources, root)?;
        let settings = link_noise_settings(sources, root)?;
        let plan = Arc::new(DensityPlan::compile_multi(&settings.router));

        Ok(Self {
            root: root.to_string(),
            prepared,
            settings,
            plan,
        })
    }

    /// 选中的根资源标识符（如 `"minecraft:overworld"`）。
    ///
    /// Selected root resource identifier (e.g. `"minecraft:overworld"`).
    pub fn root(&self) -> &str {
        &self.root
    }

    /// 解析后的闭包与指纹元数据。
    ///
    /// Resolved closure and fingerprint metadata.
    pub fn prepared(&self) -> &PreparedWorldgen {
        &self.prepared
    }

    /// 链接后的噪声设置，含垂直边界与单元格参数。
    ///
    /// Linked noise settings including vertical bounds and cell parameters.
    pub fn settings(&self) -> &LinkedNoiseSettings {
        &self.settings
    }

    /// 共享的不可变密度计划。
    ///
    /// Shared immutable density plan.
    pub fn plan(&self) -> &Arc<DensityPlan> {
        &self.plan
    }

    /// 为给定种子与注册表快照指纹的世界派生规范的 [`GeneratorDescriptor`]。
    ///
    /// Derives the canonical [`GeneratorDescriptor`] for a world with the given
    /// seed and registry snapshot fingerprint.
    pub fn descriptor(&self, seed: i64, registry_fingerprint: u64) -> GeneratorDescriptor {
        let cell = CellSettings {
            cell_size_xz: 4,
            cell_height: 8,
            min_y: self.settings.min_y as i32,
            world_height: self.settings.height as u32,
        };
        GeneratorDescriptor::new_builtin(
            seed,
            &self.root,
            self.prepared.closure_fingerprint,
            registry_fingerprint,
            cell,
        )
    }

    /// 为给定世界种子实例化线程安全的 [`WorldgenRuntime`]，绑定共享计划
    /// 并初始化播种的噪声表。
    ///
    /// Instantiates a thread-safe [`WorldgenRuntime`] for the given world seed,
    /// binding the shared plan and initializing the seeded noise tables.
    pub fn instantiate_runtime(&self, seed: i64) -> WorldgenRuntime {
        self.instantiate_runtime_with_pool_size(seed, WorldgenRuntime::default_scratch_pool_capacity())
    }

    /// 以自定义的有界暂存池大小实例化 [`WorldgenRuntime`]。
    ///
    /// Instantiates a [`WorldgenRuntime`] with a custom bounded scratch pool size.
    pub fn instantiate_runtime_with_pool_size(
        &self,
        seed: i64,
        max_idle_scratch: usize,
    ) -> WorldgenRuntime {
        WorldgenRuntime::from_shared_plan(
            seed,
            Arc::clone(&self.plan),
            &self.settings,
            max_idle_scratch,
        )
    }
}
