//! 扁平计划执行：对计划竞技场进行惰性拓扑求值，配备按调用复用的值记忆槽与有界暂存池。
//!
//! 计划的后序不变量（子节点先于父节点）使每个节点可以通过以 [`NodeId`] 索引的
//! 缓冲区读取子节点的值，热路径不触碰 JSON。
//!
//! 会改变上下文的节点（`interpolated` 的角点、`slice` 替换、`find_top_surface`
//! 探测）会在*不同*上下文中求值其子树，因此这些递归子求值在关闭记忆的情况下运行
//! （某个 NodeId 的记忆槽只在其计算时的上下文中有效），这些子树每次都被完整重算。
//!
//! Flat plan execution: lazy topological evaluation over the plan arena with
//! a per-call value memo, plus a bounded scratch pool.
//!
//! The plan's post-order invariant (children precede parents) lets a node
//! read its children's values from a buffer indexed by NodeId; the hot path
//! never touches JSON.
//!
//! Context-shifting nodes (`interpolated` corners, `slice` substitution,
//! `find_top_surface` probes) evaluate their subtree at a *different*
//! context, so those recursive sub-evaluations run with the memo disabled
//! (a NodeId's memo slot is only valid for the context it was computed at);
//! those subtrees are re-evaluated in full on every call.

use crate::data::node::Axis;
use crate::eval::{
    lerp3, linear_extend, mth_binary_search, mth_clamp, mth_lerp, EvalError, NoiseSampler,
    OldBlendedParams, SampleContext,
};
use crate::plan::{BinaryOp, DensityPlan, NodeId, PlanNode, UnaryOp};

/// 单次求值任务的缓冲区：每个计划节点一个 f64 槽位。槽位只在其计算时的
/// 上下文中有效，这正是改变上下文的子求值绕过记忆的原因。
///
/// A per-job evaluation buffer: one f64 slot per plan node. Slots are
/// only meaningful for the context they were computed at, which is why
/// context-shifting sub-evaluations bypass the memo.
#[derive(Debug)]
pub struct ScratchBuffer {
    values: Vec<Option<f64>>,
}

impl ScratchBuffer {
    pub fn with_capacity(nodes: usize) -> Self {
        let mut values = Vec::with_capacity(nodes);
        values.resize_with(nodes, || None);
        Self { values }
    }

    fn reset(&mut self) {
        for slot in self.values.iter_mut() {
            *slot = None;
        }
    }
}

/// 有界暂存缓冲池：池由单一持有者独占（无内部锁），最多保留 `max_idle`
/// 个空闲缓冲。
///
/// Bounded pool of scratch buffers: one owner holds the pool (no interior
/// locking) and at most `max_idle` idle buffers are retained.
#[derive(Debug)]
pub struct ScratchPool {
    idle: Vec<ScratchBuffer>,
    max_idle: usize,
    outstanding: usize,
}

impl ScratchPool {
    pub fn new(max_idle: usize) -> Self {
        Self { idle: Vec::new(), max_idle: max_idle.max(1), outstanding: 0 }
    }

    /// 获取一个按 `plan` 大小定制的缓冲。
    ///
    /// Acquires a buffer sized for `plan`.
    pub fn acquire(&mut self, plan: &DensityPlan) -> ScratchBuffer {
        self.outstanding += 1;
        let mut buffer = self
            .idle
            .pop()
            .unwrap_or_else(|| ScratchBuffer::with_capacity(0));
        if buffer.values.len() < plan.node_count() {
            buffer.values.resize_with(plan.node_count(), || None);
        }
        buffer
    }

    /// 将缓冲归还池中（超过 `max_idle` 的部分被丢弃）。
    ///
    /// Returns a buffer to the pool (dropped past `max_idle`).
    pub fn release(&mut self, buffer: ScratchBuffer) {
        self.outstanding = self.outstanding.saturating_sub(1);
        if self.idle.len() < self.max_idle {
            self.idle.push(buffer);
        }
    }

    /// 当前已获取、尚未归还的缓冲数量（静止状态下必须为 0）。
    ///
    /// Buffers currently acquired (must be 0 at quiescence).
    pub fn outstanding(&self) -> usize {
        self.outstanding
    }
}

/// 可复用的单计划求值任务。
///
/// A reusable evaluation job over one plan.
pub struct PlanExecutor<'a> {
    plan: &'a DensityPlan,
    buffer: ScratchBuffer,
}

impl<'a> PlanExecutor<'a> {
    pub fn new(plan: &'a DensityPlan) -> Self {
        Self { plan, buffer: ScratchBuffer::with_capacity(plan.node_count()) }
    }

    /// 基于池化缓冲构建执行器（缓冲大小必须与计划匹配，不足时会扩容）。
    ///
    /// Builds an executor over a pooled buffer (must match the plan size).
    pub fn with_buffer(plan: &'a DensityPlan, mut buffer: ScratchBuffer) -> Self {
        if buffer.values.len() < plan.node_count() {
            buffer.values.resize_with(plan.node_count(), || None);
        }
        Self { plan, buffer }
    }

    /// 将任务的缓冲归还给池（归还前先清空记忆槽）。
    ///
    /// Releases the job's buffer back to a pool.
    pub fn into_buffer(mut self) -> ScratchBuffer {
        self.buffer.reset();
        self.buffer
    }

    /// 在单个采样位置求值计划根节点。
    ///
    /// Evaluates the plan root at one sample position.
    pub fn evaluate(
        &mut self,
        sampler: &dyn NoiseSampler,
        ctx: &SampleContext,
    ) -> Result<f64, EvalError> {
        self.evaluate_node(self.plan.root(), sampler, ctx)
    }

    /// 在单个采样位置求值指定节点。
    ///
    /// Evaluates a specific node at one sample position.
    pub fn evaluate_node(
        &mut self,
        node: NodeId,
        sampler: &dyn NoiseSampler,
        ctx: &SampleContext,
    ) -> Result<f64, EvalError> {
        self.buffer.reset();
        let nodes = self.plan.node_count();
        if self.buffer.values.len() < nodes {
            self.buffer.values.resize_with(nodes, || None);
        }
        self.eval_id(node, ctx, sampler, true)
    }

    /// 在单个采样位置按名称求值某个根（用于多根计划）；根名不存在时返回
    /// [`EvalError::UnknownRoot`]。
    ///
    /// Evaluates a named root at one sample position (for multi-root plans).
    pub fn evaluate_root(
        &mut self,
        root_name: &str,
        sampler: &dyn NoiseSampler,
        ctx: &SampleContext,
    ) -> Result<f64, EvalError> {
        let node_id = self.plan.root_id(root_name).ok_or_else(|| {
            EvalError::UnknownRoot { root: root_name.to_string() }
        })?;
        self.evaluate_node(node_id, sampler, ctx)
    }

    /// 在*同一*采样位置批量求值多个命名根，跨根复用暂存缓冲的记忆槽以避免重复计算。
    ///
    /// Evaluates multiple named roots at the SAME sample position, reusing
    /// scratch buffer memo slots across roots to avoid redundant calculation.
    pub fn evaluate_roots(
        &mut self,
        root_names: &[&str],
        sampler: &dyn NoiseSampler,
        ctx: &SampleContext,
    ) -> Result<Vec<f64>, EvalError> {
        self.buffer.reset();
        let nodes = self.plan.node_count();
        if self.buffer.values.len() < nodes {
            self.buffer.values.resize_with(nodes, || None);
        }
        let mut results = Vec::with_capacity(root_names.len());
        for &name in root_names {
            let node_id = self.plan.root_id(name).ok_or_else(|| {
                EvalError::UnknownRoot { root: name.to_string() }
            })?;
            let val = self.eval_id(node_id, ctx, sampler, true)?;
            results.push(val);
        }
        Ok(results)
    }

    /// 批量求值：一个缓冲、多个上下文（典型的区块扫描）。
    ///
    /// Batch evaluation: one buffer, many contexts (typical chunk sweeps).
    pub fn evaluate_batch(
        &mut self,
        sampler: &dyn NoiseSampler,
        contexts: &[SampleContext],
        out: &mut [f64],
    ) -> Result<(), EvalError> {
        assert_eq!(contexts.len(), out.len(), "contexts/output length mismatch");
        for (index, ctx) in contexts.iter().enumerate() {
            out[index] = self.evaluate(sampler, ctx)?;
        }
        Ok(())
    }

    fn eval_id(
        &mut self,
        id: NodeId,
        ctx: &SampleContext,
        sampler: &dyn NoiseSampler,
        memo: bool,
    ) -> Result<f64, EvalError> {
        if memo {
            if let Some(value) = self.buffer.values[id.index()] {
                return Ok(value);
            }
        }
        let value = self.eval_node(id, ctx, sampler, memo)?;
        if memo {
            self.buffer.values[id.index()] = Some(value);
        }
        Ok(value)
    }

    fn eval_node(
        &mut self,
        id: NodeId,
        ctx: &SampleContext,
        sampler: &dyn NoiseSampler,
        memo: bool,
    ) -> Result<f64, EvalError> {
        let node = self.plan.node(id).clone();
        Ok(match node {
            // Constants are parsed as f32.
            PlanNode::Constant(value) => f64::from(value as f32),
            PlanNode::Coord(axis) => match axis {
                Axis::X => ctx.x,
                Axis::Y => ctx.y,
                Axis::Z => ctx.z,
            },
            PlanNode::Noise { noise, xz_scale, y_scale, shift_x, shift_z } => {
                let shifted_x = ctx.x * xz_scale
                    + match shift_x {
                        Some(shift) => self.eval_id(shift, ctx, sampler, memo)?,
                        None => 0.0,
                    };
                let shifted_y = ctx.y * y_scale;
                let shifted_z = ctx.z * xz_scale
                    + match shift_z {
                        Some(shift) => self.eval_id(shift, ctx, sampler, memo)?,
                        None => 0.0,
                    };
                sampler.sample(
                    &self.plan.noises()[noise as usize],
                    shifted_x,
                    shifted_y,
                    shifted_z,
                )
            }
            PlanNode::ShiftA(noise) => {
                let sampled = sampler.sample(
                    &self.plan.noises()[noise as usize],
                    ctx.x * 0.25,
                    0.0,
                    ctx.z * 0.25,
                ) as f32;
                f64::from(sampled * 4.0)
            }
            PlanNode::ShiftB(noise) => {
                let sampled = sampler.sample(
                    &self.plan.noises()[noise as usize],
                    ctx.z * 0.25,
                    ctx.x * 0.25,
                    0.0,
                ) as f32;
                f64::from(sampled * 4.0)
            }
            PlanNode::Binary(op, a, b) => {
                let a = self.eval_id(a, ctx, sampler, memo)? as f32;
                let b = self.eval_id(b, ctx, sampler, memo)? as f32;
                let value = match op {
                    BinaryOp::Add => a + b,
                    BinaryOp::Sub => a - b,
                    BinaryOp::Mul => {
                        if a == 0.0 {
                            0.0
                        } else {
                            a * b
                        }
                    }
                    BinaryOp::Div => {
                        let value = a / b;
                        if !value.is_finite() {
                            return Err(EvalError::NonFinite { node: "div" });
                        }
                        value
                    }
                    BinaryOp::Min => a.min(b),
                    BinaryOp::Max => a.max(b),
                };
                f64::from(value)
            }
            PlanNode::Unary(op, input) => {
                let value = self.eval_id(input, ctx, sampler, memo)? as f32;
                let folded = match op {
                    UnaryOp::Negate => -value,
                    UnaryOp::Abs => value.abs(),
                    UnaryOp::Square => value * value,
                    UnaryOp::Cube => value * value * value,
                    UnaryOp::HalfNegative => {
                        if value > 0.0 {
                            value
                        } else {
                            value * 0.5
                        }
                    }
                    UnaryOp::QuarterNegative => {
                        if value > 0.0 {
                            value
                        } else {
                            value * 0.25
                        }
                    }
                    UnaryOp::Squeeze => {
                        let clamped = mth_clamp(value, -1.0, 1.0);
                        clamped / 2.0 - clamped * clamped * clamped / 24.0
                    }
                };
                f64::from(folded)
            }
            PlanNode::Clamp { input, min, max } => {
                let value = self.eval_id(input, ctx, sampler, memo)? as f32;
                f64::from(mth_clamp(value, min as f32, max as f32))
            }
            PlanNode::RangeChoice { input, min, max, in_range, out_of_range } => {
                let value = self.eval_id(input, ctx, sampler, memo)? as f32;
                let chosen = if value >= min as f32 && value < max as f32 {
                    in_range
                } else {
                    out_of_range
                };
                self.eval_id(chosen, ctx, sampler, memo)?
            }
            PlanNode::Gradient { axis, from_coordinate, from_value, to_coordinate, to_value } => {
                let coordinate = match axis {
                    Axis::X => ctx.x,
                    Axis::Y => ctx.y,
                    Axis::Z => ctx.z,
                } as i64;
                let from = from_coordinate as i64;
                let to = to_coordinate as i64;
                let min = from.min(to);
                let max = from.max(to);
                let relative = (coordinate.clamp(min, max) - from) as f32;
                let factor = (to_value as f32 - from_value as f32) / (to - from) as f32;
                f64::from(from_value as f32 + relative * factor)
            }
            PlanNode::Lerp { alpha, first, second } => {
                let alpha = self.eval_id(alpha, ctx, sampler, memo)? as f32;
                let first = self.eval_id(first, ctx, sampler, memo)? as f32;
                let second = self.eval_id(second, ctx, sampler, memo)? as f32;
                let value = if alpha == 0.0 {
                    first
                } else if alpha == 1.0 {
                    second
                } else {
                    mth_lerp(alpha, first, second)
                };
                f64::from(value)
            }
            PlanNode::Interpolated { cell_size_xz, cell_size_y, input } => {
                // Interpolated sampling: floorMod cell, 8 corners, trilinear
                // lerp3 (all f32, integer block coordinates).
                let block_x = ctx.x as i64;
                let block_y = ctx.y as i64;
                let block_z = ctx.z as i64;
                let x_in_cell = block_x.rem_euclid(cell_size_xz as i64);
                let y_in_cell = block_y.rem_euclid(cell_size_y as i64);
                let z_in_cell = block_z.rem_euclid(cell_size_xz as i64);
                if x_in_cell == 0 && y_in_cell == 0 && z_in_cell == 0 {
                    self.eval_id(input, ctx, sampler, memo)?
                } else {
                    let x0 = block_x - x_in_cell;
                    let y0 = block_y - y_in_cell;
                    let z0 = block_z - z_in_cell;
                    let sx = cell_size_xz as i64;
                    let sy = cell_size_y as i64;
                    // Corners are a different context: the memo is disabled.
                    let corner = |executor: &mut Self, cx: i64, cy: i64, cz: i64| {
                        executor.eval_id(
                            input,
                            &SampleContext {
                                x: cx as f64,
                                y: cy as f64,
                                z: cz as f64,
                            },
                            sampler,
                            false,
                        )
                    };
                    let c000 = corner(self, x0, y0, z0)? as f32;
                    let c100 = corner(self, x0 + sx, y0, z0)? as f32;
                    let c010 = corner(self, x0, y0 + sy, z0)? as f32;
                    let c110 = corner(self, x0 + sx, y0 + sy, z0)? as f32;
                    let c001 = corner(self, x0, y0, z0 + sx)? as f32;
                    let c101 = corner(self, x0 + sx, y0, z0 + sx)? as f32;
                    let c011 = corner(self, x0, y0 + sy, z0 + sx)? as f32;
                    let c111 = corner(self, x0 + sx, y0 + sy, z0 + sx)? as f32;
                    let tx = x_in_cell as f32 / cell_size_xz as f32;
                    let ty = y_in_cell as f32 / cell_size_y as f32;
                    let tz = z_in_cell as f32 / cell_size_xz as f32;
                    f64::from(lerp3(tx, ty, tz, c000, c100, c010, c110, c001, c101, c011, c111))
                }
            }
            PlanNode::Slice { axis, coordinate, input } => {
                let sliced = SampleContext {
                    x: if axis == Axis::X { coordinate } else { ctx.x },
                    y: if axis == Axis::Y { coordinate } else { ctx.y },
                    z: if axis == Axis::Z { coordinate } else { ctx.z },
                };
                self.eval_id(input, &sliced, sampler, false)?
            }
            PlanNode::IntervalSelect { input, thresholds, functions } => {
                let value = self.eval_id(input, ctx, sampler, memo)? as f32;
                let thresholds = self.plan.thresholds(thresholds);
                let functions = self.plan.function_ids(functions);
                for (index, threshold) in thresholds.iter().enumerate() {
                    if value < *threshold as f32 {
                        return self.eval_id(functions[index], ctx, sampler, memo);
                    }
                }
                let last = functions.len() - 1;
                self.eval_id(functions[last], ctx, sampler, memo)?
            }
            PlanNode::DistanceToPoint { point } => {
                // Euclidean distance: int deltas, f32 lengthSquared,
                // f64 sqrt narrowed to f32.
                let dx = (point[0] as i64 - ctx.x as i64) as f32;
                let dy = (point[1] as i64 - ctx.y as i64) as f32;
                let dz = (point[2] as i64 - ctx.z as i64) as f32;
                let squared = dx * dx + dy * dy + dz * dz;
                f64::from((f64::from(squared).sqrt()) as f32)
            }
            PlanNode::Spline { coordinate, points } => {
                // Multi-point spline: f32 Hermite with
                // linearExtend boundaries and binary-search interval location.
                let x = self.eval_id(coordinate, ctx, sampler, memo)? as f32;
                let points = self.plan.spline_points(points);
                let locations: Vec<f32> = points.iter().map(|p| p.location as f32).collect();
                let start = mth_binary_search(&locations, x) as i32 - 1;
                let last = (points.len() - 1) as i32;
                if start < 0 {
                    let value = self.eval_id(points[0].value, ctx, sampler, memo)? as f32;
                    return Ok(f64::from(linear_extend(
                        x,
                        locations[0],
                        value,
                        points[0].derivative as f32,
                    )));
                }
                if start == last {
                    let value =
                        self.eval_id(points[last as usize].value, ctx, sampler, memo)? as f32;
                    return Ok(f64::from(linear_extend(
                        x,
                        locations[last as usize],
                        value,
                        points[last as usize].derivative as f32,
                    )));
                }
                let index = start as usize;
                let x1 = locations[index];
                let x2 = locations[index + 1];
                let t = (x - x1) / (x2 - x1);
                let y1 = self.eval_id(points[index].value, ctx, sampler, memo)? as f32;
                let y2 = self.eval_id(points[index + 1].value, ctx, sampler, memo)? as f32;
                let d1 = points[index].derivative as f32;
                let d2 = points[index + 1].derivative as f32;
                let a = d1 * (x2 - x1) - (y2 - y1);
                let b = -d2 * (x2 - x1) + (y2 - y1);
                f64::from(mth_lerp(t, y1, y2) + t * (1.0 - t) * mth_lerp(t, a, b))
            }
            PlanNode::FindTopSurface { cell_height, density, lower_bound, upper_bound } => {
                // find_top_surface: topY = floor(upper/cellH)*cellH (f32
                // division), then probe down by cell until density > 0.
                let upper = self.eval_id(upper_bound, ctx, sampler, memo)? as f32;
                let cell = cell_height as i64;
                let top_y = (upper / cell_height as f32).floor() as i64 * cell;
                let lower = lower_bound as i64;
                if top_y <= lower {
                    lower as f64
                } else {
                    let mut probe_y = top_y;
                    loop {
                        let probe_ctx = SampleContext {
                            x: ctx.x,
                            y: probe_y as f64,
                            z: ctx.z,
                        };
                        let probe = self.eval_id(density, &probe_ctx, sampler, false)?;
                        if probe > 0.0 || probe_y <= lower {
                            break if probe > 0.0 { probe_y as f64 } else { lower as f64 };
                        }
                        probe_y -= cell;
                    }
                }
            }
            PlanNode::OldBlended(index) => {
                let params: &OldBlendedParams = &self.plan.blended_params()[index as usize];
                sampler.sample_old_blended(params, ctx.x, ctx.y, ctx.z)?
            }
            PlanNode::EndOuterIslands => sampler.sample_end_islands(ctx.x as i32, ctx.z as i32)?,
            PlanNode::Beardifier => 0.0,
            PlanNode::BlendAlpha => 1.0,
            PlanNode::BlendOffset => 0.0,
            PlanNode::BlendDensity(input) => self.eval_id(input, ctx, sampler, memo)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::node::Node;
    use crate::eval::NoiseSampler;

    struct StubSampler;

    impl NoiseSampler for StubSampler {
        fn sample(&self, noise: &str, x: f64, y: f64, z: f64) -> f64 {
            let seed: i64 = noise.bytes().map(i64::from).sum();
            let k = (x as i64)
                .wrapping_mul(-7046029254386353131i64)
                ^ (y as i64).wrapping_mul(-4658895280553007687i64)
                ^ (z as i64).wrapping_mul(-7723592293110705685i64)
                ^ seed;
            ((k >> 33) % 1000) as f64 / 1000.0
        }
    }

    #[test]
    fn plan_executor_agrees_with_the_tree_evaluator() {
        let tree = Node::Add(
            Box::new(Node::Constant(1.5)),
            Box::new(Node::Mul(
                Box::new(Node::Coord(Axis::Y)),
                Box::new(Node::Constant(2.0)),
            )),
        );
        let plan = DensityPlan::compile(&tree);
        let mut executor = PlanExecutor::new(&plan);
        let ctx = SampleContext { x: 3.0, y: 4.0, z: 5.0 };
        assert_eq!(
            executor.evaluate(&StubSampler, &ctx).unwrap(),
            crate::eval::evaluate(&tree, &ctx, &StubSampler).unwrap()
        );
    }

    #[test]
    fn scratch_pool_reuses_buffers_and_tracks_outstanding() {
        let tree = Node::Coord(Axis::X);
        let plan = DensityPlan::compile(&tree);
        let mut pool = ScratchPool::new(2);

        assert_eq!(pool.outstanding(), 0);
        let b1 = pool.acquire(&plan);
        let b2 = pool.acquire(&plan);
        assert_eq!(pool.outstanding(), 2);

        pool.release(b1);
        assert_eq!(pool.outstanding(), 1);

        let _b3 = pool.acquire(&plan);
        assert_eq!(pool.outstanding(), 2);

        pool.release(b2);
        pool.release(_b3);
        assert_eq!(pool.outstanding(), 0);
    }

    #[test]
    fn scratch_pool_enforces_max_idle() {
        let tree = Node::Coord(Axis::Y);
        let plan = DensityPlan::compile(&tree);
        let mut pool = ScratchPool::new(2);

        let b1 = pool.acquire(&plan);
        let b2 = pool.acquire(&plan);
        let b3 = pool.acquire(&plan);
        let b4 = pool.acquire(&plan);
        assert_eq!(pool.outstanding(), 4);

        pool.release(b1);
        pool.release(b2);
        pool.release(b3);
        pool.release(b4);
        assert_eq!(pool.outstanding(), 0);

        // Max idle is 2, so popping twice empties the pool, a third pop allocates fresh.
        let _p1 = pool.acquire(&plan);
        let _p2 = pool.acquire(&plan);
        assert_eq!(pool.idle.len(), 0);
        let _p3 = pool.acquire(&plan);
        assert_eq!(pool.outstanding(), 3);
    }

    #[test]
    fn plan_executor_with_pooled_buffer_and_into_buffer() {
        let tree = Node::Mul(
            Box::new(Node::Coord(Axis::Z)),
            Box::new(Node::Constant(3.0)),
        );
        let plan = DensityPlan::compile(&tree);
        let mut pool = ScratchPool::new(1);

        let buf = pool.acquire(&plan);
        assert_eq!(pool.outstanding(), 1);

        let mut executor = PlanExecutor::with_buffer(&plan, buf);
        let ctx = SampleContext { x: 1.0, y: 2.0, z: 4.0 };
        assert_eq!(executor.evaluate(&StubSampler, &ctx).unwrap(), 12.0);

        let retrieved = executor.into_buffer();
        pool.release(retrieved);
        assert_eq!(pool.outstanding(), 0);
    }

    #[test]
    fn multi_root_plan_executor_shares_scratch_and_evaluates_consistently() {
        use std::collections::BTreeMap;

        let mut roots = BTreeMap::new();
        let shared = Node::Mul(Box::new(Node::Coord(Axis::Y)), Box::new(Node::Constant(5.0)));
        roots.insert(
            "temperature".to_string(),
            Node::Add(Box::new(shared.clone()), Box::new(Node::Constant(1.0))),
        );
        roots.insert(
            "vegetation".to_string(),
            Node::Add(Box::new(shared), Box::new(Node::Constant(2.0))),
        );

        let plan = DensityPlan::compile_multi(&roots);
        let mut executor = PlanExecutor::new(&plan);
        let ctx = SampleContext { x: 0.0, y: 10.0, z: 0.0 };

        // Single root evaluations:
        assert_eq!(executor.evaluate_root("temperature", &StubSampler, &ctx).unwrap(), 51.0);
        assert_eq!(executor.evaluate_root("vegetation", &StubSampler, &ctx).unwrap(), 52.0);

        // Batched roots at same context (reusing memo):
        let results = executor
            .evaluate_roots(&["temperature", "vegetation"], &StubSampler, &ctx)
            .unwrap();
        assert_eq!(results, vec![51.0, 52.0]);
    }
}

