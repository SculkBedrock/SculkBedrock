//! 按种子隔离的世界生成运行时：将不可变的 [`DensityPlan`] 绑定到播种的
//! [`NoiseBank`] 与独占、有界的 [`ScratchPool`]。
//!
//! WorldgenRuntime: per-seed runtime binding the immutable [`DensityPlan`]
//! to a seeded [`NoiseBank`] and an exclusive, bounded [`ScratchPool`].
//!
//! [`Arc<DensityPlan>`] 只读，可在生成设置相同的世界之间共享；[`NoiseBank`]
//! 在世界加载时按种子确定性播种，生成期间只读；可变求值缓冲存放在有界的
//! [`ScratchPool`] 中，由单个工作任务独占借用并在完成后归还。
//!
//! [`Arc<DensityPlan>`] is read-only and shared across worlds with identical
//! generation settings; [`NoiseBank`] is seeded deterministically at world load
//! and read-only during generation; mutable evaluation buffers live in the
//! bounded [`ScratchPool`], borrowed exclusively by one worker job and returned
//! on completion.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::eval::{EvalError, OldBlendedParams, SampleContext};
use crate::exec::{PlanExecutor, ScratchBuffer, ScratchPool};
use crate::link::LinkedNoiseSettings;
use crate::noise::NoiseBank;
use crate::plan::DensityPlan;

thread_local! {
    static THREAD_SCRATCH: RefCell<Option<ScratchBuffer>> = const { RefCell::new(None) };
}

enum EvaluatorSource<'a> {
    Pooled(&'a WorldgenRuntime),
    ThreadLocal,
}

/// 作用域求值器：在多次求值之间持有一个独占暂存缓冲。析构时缓冲自动
/// 归还到有界池或线程本地缓存。
///
/// A scoped evaluator holding an exclusive scratch buffer across multiple
/// evaluations. On drop, the buffer is automatically returned to either the
/// bounded pool or thread-local cache.
pub struct ChunkEvaluator<'a> {
    runtime: &'a WorldgenRuntime,
    source: EvaluatorSource<'a>,
    executor: Option<PlanExecutor<'a>>,
}

impl<'a> ChunkEvaluator<'a> {
    /// 在单个采样位置按名称求值某个根。
    ///
    /// Evaluates a named root at one sample position.
    pub fn sample_root(&mut self, root_name: &str, ctx: &SampleContext) -> Result<f64, EvalError> {
        let executor = self.executor.as_mut().expect("evaluator already consumed");
        executor.evaluate_root(root_name, &*self.runtime.noise_bank, ctx)
    }

    /// 在*同一*采样位置批量求值多个命名根，共享记忆化槽位。
    ///
    /// Evaluates multiple named roots at the SAME sample position, sharing
    /// memoized slots.
    pub fn sample_roots(&mut self, root_names: &[&str], ctx: &SampleContext) -> Result<Vec<f64>, EvalError> {
        let executor = self.executor.as_mut().expect("evaluator already consumed");
        executor.evaluate_roots(root_names, &*self.runtime.noise_bank, ctx)
    }
}

impl<'a> Drop for ChunkEvaluator<'a> {
    fn drop(&mut self) {
        if let Some(executor) = self.executor.take() {
            let buffer = executor.into_buffer();
            match self.source {
                EvaluatorSource::Pooled(rt) => {
                    rt.release_scratch(buffer);
                }
                EvaluatorSource::ThreadLocal => {
                    THREAD_SCRATCH.with(|cell| {
                        *cell.borrow_mut() = Some(buffer);
                    });
                }
            }
        }
    }
}

/// 从链接后的路由根中收集所有 `old_blended_noise` 参数形态（按位去重）。
///
/// Collects every `old_blended_noise` parameter shape from linked router roots.
fn collect_blended_params_from_settings(settings: &LinkedNoiseSettings) -> Vec<OldBlendedParams> {
    let mut keys = BTreeSet::new();
    for node in settings.router.values() {
        node.walk(&mut |visited| {
            if let crate::data::Node::OldBlendedNoise {
                xz_scale,
                y_scale,
                xz_factor,
                y_factor,
                smear_scale_multiplier,
            } = visited
            {
                keys.insert([
                    xz_scale.to_bits(),
                    y_scale.to_bits(),
                    xz_factor.to_bits(),
                    y_factor.to_bits(),
                    smear_scale_multiplier.to_bits(),
                ]);
            }
        });
    }
    keys.into_iter()
        .map(|[a, b, c, d, e]| OldBlendedParams {
            xz_scale: f64::from_bits(a),
            y_scale: f64::from_bits(b),
            xz_factor: f64::from_bits(c),
            y_factor: f64::from_bits(d),
            smear_scale_multiplier: f64::from_bits(e),
        })
        .collect()
}

/// 单个世界/维度生成实例的运行时状态。
///
/// The runtime state for one world/dimension generation instance.
pub struct WorldgenRuntime {
    seed: i64,
    plan: Arc<DensityPlan>,
    noise_bank: Arc<NoiseBank>,
    scratch_pool: Mutex<ScratchPool>,
    min_y: i64,
    height: i64,
}

impl fmt::Debug for WorldgenRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorldgenRuntime")
            .field("seed", &self.seed)
            .field("min_y", &self.min_y)
            .field("height", &self.height)
            .field("plan_nodes", &self.plan.node_count())
            .finish()
    }
}

impl WorldgenRuntime {
    /// 基于 CPU 核心数的默认暂存池容量：max_workers + 4。
    ///
    /// Default scratch pool capacity based on CPU cores: max_workers + 4.
    pub fn default_scratch_pool_capacity() -> usize {
        let max_workers = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .saturating_sub(1)
            .max(2);
        max_workers + 4
    }

    /// 直接从链接后的噪声设置与世界种子构建运行时；将所有路由根编译进
    /// 共享计划并绑定噪声库。
    ///
    /// Builds a runtime directly from linked noise settings and world seed.
    /// Compiles all router roots into a shared plan and binds the noise bank.
    pub fn new(seed: i64, settings: &LinkedNoiseSettings) -> Self {
        let plan = Arc::new(DensityPlan::compile_multi(&settings.router));
        let blended = collect_blended_params_from_settings(settings);
        let noise_bank = Arc::new(NoiseBank::new(seed, &settings.noises, &blended));
        let scratch_pool = Mutex::new(ScratchPool::new(Self::default_scratch_pool_capacity()));

        Self {
            seed,
            plan,
            noise_bank,
            scratch_pool,
            min_y: settings.min_y,
            height: settings.height,
        }
    }

    /// 复用已编译计划构建运行时（例如多个世界或测试共享相同维度/设置时）。
    ///
    /// Builds a runtime reusing an already-compiled plan (e.g. for multiple worlds
    /// or tests sharing the same dimensions/settings).
    pub fn from_shared_plan(
        seed: i64,
        plan: Arc<DensityPlan>,
        settings: &LinkedNoiseSettings,
        max_idle_scratch: usize,
    ) -> Self {
        let blended = collect_blended_params_from_settings(settings);
        let noise_bank = Arc::new(NoiseBank::new(seed, &settings.noises, &blended));
        let scratch_pool = Mutex::new(ScratchPool::new(max_idle_scratch));

        Self {
            seed,
            plan,
            noise_bank,
            scratch_pool,
            min_y: settings.min_y,
            height: settings.height,
        }
    }

    /// 世界种子。
    ///
    /// World seed.
    pub fn seed(&self) -> i64 {
        self.seed
    }

    /// 维度最小 Y 坐标。
    ///
    /// Dimension minimum Y coordinate.
    pub fn min_y(&self) -> i64 {
        self.min_y
    }

    /// 维度高度。
    ///
    /// Dimension height.
    pub fn height(&self) -> i64 {
        self.height
    }

    /// 共享的不可变密度计划。
    ///
    /// Shared immutable density plan.
    pub fn plan(&self) -> &Arc<DensityPlan> {
        &self.plan
    }

    /// 共享的不可变噪声库。
    ///
    /// Shared immutable noise bank.
    pub fn noise_bank(&self) -> &Arc<NoiseBank> {
        &self.noise_bank
    }

    /// 从有界池获取一个独占暂存缓冲。
    ///
    /// Acquires an exclusive scratch buffer from the bounded pool.
    pub fn acquire_scratch(&self) -> ScratchBuffer {
        let mut pool = self.scratch_pool.lock().expect("scratch pool mutex poisoned");
        pool.acquire(&self.plan)
    }

    /// 将独占暂存缓冲归还到有界池。
    ///
    /// Releases an exclusive scratch buffer back to the bounded pool.
    pub fn release_scratch(&self, buffer: ScratchBuffer) {
        let mut pool = self.scratch_pool.lock().expect("scratch pool mutex poisoned");
        pool.release(buffer);
    }

    /// 当前已借出（未归还）的暂存缓冲数量。静止状态与世界关闭时必须为 0。
    ///
    /// Current number of outstanding (unreturned) scratch buffers.
    /// Must be 0 at quiescence and world shutdown.
    pub fn outstanding_scratch(&self) -> usize {
        let pool = self.scratch_pool.lock().expect("scratch pool mutex poisoned");
        pool.outstanding()
    }

    /// 线程安全的命名路由根（如 "final_density"）单点求值：从池获取暂存
    /// 缓冲，求值后归还。
    ///
    /// Thread-safe point evaluation of a named router root (e.g. "final_density").
    /// Acquires a scratch buffer from the pool, evaluates, and returns the buffer.
    pub fn sample_root(&self, root_name: &str, ctx: &SampleContext) -> Result<f64, EvalError> {
        let buffer = self.acquire_scratch();
        let mut executor = PlanExecutor::with_buffer(&self.plan, buffer);
        let result = executor.evaluate_root(root_name, &*self.noise_bank, ctx);
        let returned_buffer = executor.into_buffer();
        self.release_scratch(returned_buffer);
        result
    }

    /// 线程安全的多个命名路由根单点求值（同一上下文），跨根共享中间记忆化
    /// 节点槽位。
    ///
    /// Thread-safe point evaluation of multiple named router roots at the SAME
    /// context, sharing intermediate memoized node slots across roots.
    pub fn sample_roots(
        &self,
        root_names: &[&str],
        ctx: &SampleContext,
    ) -> Result<Vec<f64>, EvalError> {
        let buffer = self.acquire_scratch();
        let mut executor = PlanExecutor::with_buffer(&self.plan, buffer);
        let result = executor.evaluate_roots(root_names, &*self.noise_bank, ctx);
        let returned_buffer = executor.into_buffer();
        self.release_scratch(returned_buffer);
        result
    }

    /// 构建区块作用域求值器，从有界池借用一个独占暂存缓冲；析构时归还池。
    ///
    /// Builds a chunk-scoped evaluator borrowing an exclusive scratch buffer
    /// from the bounded pool. The buffer is returned to the pool on drop.
    pub fn chunk_evaluator(&self) -> ChunkEvaluator<'_> {
        let buffer = self.acquire_scratch();
        ChunkEvaluator {
            runtime: self,
            source: EvaluatorSource::Pooled(self),
            executor: Some(PlanExecutor::with_buffer(&self.plan, buffer)),
        }
    }

    /// 构建工作线程独占的求值器，复用线程本地暂存缓冲，完全绕过池的互斥锁。
    ///
    /// Builds a worker-exclusive evaluator reusing a thread-local scratch buffer,
    /// bypassing all pool mutex locks completely.
    pub fn thread_local_evaluator(&self) -> ChunkEvaluator<'_> {
        let buffer = THREAD_SCRATCH.with(|cell| {
            cell.borrow_mut().take().unwrap_or_else(|| {
                ScratchBuffer::with_capacity(self.plan.node_count())
            })
        });
        ChunkEvaluator {
            runtime: self,
            source: EvaluatorSource::ThreadLocal,
            executor: Some(PlanExecutor::with_buffer(&self.plan, buffer)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use crate::data::Node;

    fn make_test_settings() -> LinkedNoiseSettings {
        let mut router = BTreeMap::new();
        router.insert(
            "final_density".to_string(),
            Node::Add(
                Box::new(Node::Coord(crate::data::Axis::Y)),
                Box::new(Node::Constant(10.0)),
            ),
        );
        router.insert(
            "temperature".to_string(),
            Node::Mul(
                Box::new(Node::Coord(crate::data::Axis::X)),
                Box::new(Node::Constant(0.5)),
            ),
        );

        LinkedNoiseSettings {
            legacy_random_source: false,
            min_y: -64,
            height: 384,
            router,
            noises: BTreeMap::new(),
            frozen: BTreeMap::new(),
        }
    }

    #[test]
    fn runtime_evaluates_samples_and_cleans_up_scratch() {
        let settings = make_test_settings();
        let runtime = WorldgenRuntime::new(12345, &settings);

        assert_eq!(runtime.seed(), 12345);
        assert_eq!(runtime.min_y(), -64);
        assert_eq!(runtime.height(), 384);
        assert_eq!(runtime.outstanding_scratch(), 0);

        let ctx = SampleContext { x: 20.0, y: 5.0, z: 0.0 };
        let fd = runtime.sample_root("final_density", &ctx).unwrap();
        assert_eq!(fd, 15.0);
        assert_eq!(runtime.outstanding_scratch(), 0, "buffer returned to pool");

        let temp = runtime.sample_root("temperature", &ctx).unwrap();
        assert_eq!(temp, 10.0);
        assert_eq!(runtime.outstanding_scratch(), 0);

        let batched = runtime
            .sample_roots(&["final_density", "temperature"], &ctx)
            .unwrap();
        assert_eq!(batched, vec![15.0, 10.0]);
        assert_eq!(runtime.outstanding_scratch(), 0);
    }

    #[test]
    fn runtime_concurrent_evaluations_from_multiple_threads() {
        let settings = make_test_settings();
        let runtime = Arc::new(WorldgenRuntime::new(42, &settings));

        let mut handles = Vec::new();
        for i in 0..8 {
            let rt = Arc::clone(&runtime);
            let handle = std::thread::spawn(move || {
                for step in 0..100 {
                    let ctx = SampleContext {
                        x: (i * 10 + step) as f64,
                        y: step as f64,
                        z: 0.0,
                    };
                    let val = rt.sample_root("final_density", &ctx).unwrap();
                    assert_eq!(val, (step + 10) as f64);
                }
            });
            handles.push(handle);
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(runtime.outstanding_scratch(), 0, "all scratch returned after threads complete");
    }

    #[test]
    fn chunk_evaluator_reuses_buffer_across_samples_and_cleans_up() {
        let settings = make_test_settings();
        let runtime = WorldgenRuntime::new(54321, &settings);
        assert_eq!(runtime.outstanding_scratch(), 0);

        {
            let mut evaluator = runtime.chunk_evaluator();
            assert_eq!(runtime.outstanding_scratch(), 1, "1 buffer borrowed for chunk");

            for y in 0..100 {
                let ctx = SampleContext { x: 0.0, y: y as f64, z: 0.0 };
                let val = evaluator.sample_root("final_density", &ctx).unwrap();
                assert_eq!(val, (y + 10) as f64);
            }
            assert_eq!(runtime.outstanding_scratch(), 1, "still 1 buffer held during chunk");
        }

        assert_eq!(runtime.outstanding_scratch(), 0, "buffer returned to pool on drop");
    }

    #[test]
    fn thread_local_evaluator_bypasses_pool_locks() {
        let settings = make_test_settings();
        let runtime = WorldgenRuntime::new(99999, &settings);
        assert_eq!(runtime.outstanding_scratch(), 0);

        {
            let mut evaluator = runtime.thread_local_evaluator();
            for y in 0..50 {
                let ctx = SampleContext { x: 10.0, y: y as f64, z: 0.0 };
                let val = evaluator.sample_root("temperature", &ctx).unwrap();
                assert_eq!(val, 5.0);
            }
            // Pool was never touched
            assert_eq!(runtime.outstanding_scratch(), 0);
        }

        assert_eq!(runtime.outstanding_scratch(), 0);
    }

    #[test]
    fn capacity_strategy_is_max_workers_plus_four() {
        let cap = WorldgenRuntime::default_scratch_pool_capacity();
        let parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let expected_workers = parallelism.saturating_sub(1).max(2);
        assert_eq!(cap, expected_workers + 4);
    }
}
