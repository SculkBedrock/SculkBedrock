//! 标量求值器：在单个采样位置递归求值链接后的节点树。
//!
//! 数值合同：每个节点的取值都是 f32（以 f64 承载），每个节点在自己的运算内完成
//! f32 收窄——常量按 f32 解析，二元/一元运算、clamp、lerp、gradient、三线性插值
//! 与 Hermite 样条全部为 f32。逐节点规则：`mul` 在左操作数为 `0.0` 时短路右操作数；
//! `div` 结果非有限时报错；`min`/`max` 采用 `f32::min`/`max` 的 NaN 语义；
//! `clamp` 为 `v < min ? min : v.min(max)`；`lerp` 在 `alpha == 0` 时取 first、
//! `alpha == 1` 时取 second；`gradient` 先把坐标取整并钳制，再取
//! `fromValue + relative * factor`（f32 除法；tiling 仅支持 CLAMP_TO_EDGE）；
//! `interpolated` 为 `floorMod` 单元格 + 8 角点 + f32 三线性 lerp（要求整数块坐标）；
//! `spline` 为 f32 Hermite 样条，区间外按 `linearExtend`（导数非零时线性外推），
//! 区间定位用二分搜索；`find_top_surface` 先取 `floor(upper/cellH)*cellH`
//! （f32 除法），再逐 cell 下探首个 `density > 0.0`；`shift_a`/`shift_b` 为
//! 四分之一尺度采样 × 4（f32）。
//!
//! 固定新世界上下文下的叶子取值（无纯标量合约者显式声明）：`beardifier` = 0.0
//! （仅地形探测用）；`blend_alpha` = 1.0；`blend_offset` = 0.0；`blend_density`
//! 直通；`cache` 直通（性能标记，不改变取值）；`old_blended_noise` 与
//! `end_outer_islands` 由绑定的 `NoiseBank` 提供（bank 内同样为 f32）。
//!
//! Scalar evaluator: recursively evaluates the linked node tree at one sample
//! position.
//!
//! Numeric contract: every node value is an f32 (carried in f64) and each node
//! narrows to f32 inside its own arithmetic — constants parse as f32, and
//! binary/unary ops, clamp, lerp, gradient, trilinear interpolation and the
//! Hermite spline are all f32. Per-node rules: `mul` short-circuits the right
//! operand when the left operand is `0.0`; `div` errors on a non-finite result;
//! `min`/`max` use the NaN semantics of `f32::min`/`max`; `clamp` is
//! `v < min ? min : v.min(max)`; `lerp` returns `first` at `alpha == 0` and
//! `second` at `alpha == 1`; `gradient` truncates and clamps the coordinate to
//! an integer, then applies `fromValue + relative * factor` (f32 division; only
//! CLAMP_TO_EDGE tiling); `interpolated` is a `floorMod` cell, 8 corners and an
//! f32 trilinear lerp (integer block coordinates required); `spline` is an f32
//! Hermite spline using `linearExtend` outside the interval (linear
//! extrapolation when the derivative is non-zero) with binary-search interval
//! location; `find_top_surface` takes `floor(upper/cellH)*cellH` (f32 division)
//! and then scans down cell by cell for the first `density > 0.0`;
//! `shift_a`/`shift_b` are quarter-scale samples multiplied by 4 (f32).
//!
//! Leaf values under the fixed new-world context (those without a pure scalar
//! contract are stated explicitly): `beardifier` = 0.0 (terrain-probe leaf);
//! `blend_alpha` = 1.0; `blend_offset` = 0.0; `blend_density` passes through;
//! `cache` passes through (performance marker only); `old_blended_noise` and
//! `end_outer_islands` are provided by the bound `NoiseBank` (f32 there too).

use crate::data::node::{Axis, Node, Spline};

/// 单个标量采样位置（方块坐标）。
///
/// One scalar sample position (block coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SampleContext {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// `old_blended_noise` 的参数（求值时从节点拷贝而来）。
///
/// Parameters of `old_blended_noise` (copied from the node at eval time).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OldBlendedParams {
    pub xz_scale: f64,
    pub y_scale: f64,
    pub xz_factor: f64,
    pub y_factor: f64,
    pub smear_scale_multiplier: f64,
}

/// 噪声采样后端：求值器通过该接口取得噪声取值，不依赖具体实现。
///
/// Noise sampling backend: the evaluator obtains noise values through this
/// interface and does not depend on a particular implementation.
pub trait NoiseSampler {
    fn sample(&self, noise: &str, x: f64, y: f64, z: f64) -> f64;

    /// `old_blended_noise` 的运行时入口。默认实现返回显式的
    /// `Unsupported` 错误；绑定噪声库的采样器覆盖它。
    ///
    /// `old_blended_noise` runtime. The default implementation returns an
    /// explicit `Unsupported` error; bank-backed samplers override it.
    fn sample_old_blended(
        &self,
        _params: &OldBlendedParams,
        _x: f64,
        _y: f64,
        _z: f64,
    ) -> Result<f64, EvalError> {
        Err(EvalError::Unsupported { node: "old_blended_noise" })
    }

    /// `end_outer_islands` 的运行时入口。绑定噪声库
    /// （基于 `LegacyRandomSource(world_seed)` 的 SimplexNoise）；默认实现
    /// 返回显式错误。
    ///
    /// `end_outer_islands` runtime. Bank-bound (SimplexNoise over
    /// `LegacyRandomSource(world_seed)`); the default returns an explicit error.
    fn sample_end_islands(
        &self,
        _block_x: i32,
        _block_z: i32,
    ) -> Result<f64, EvalError> {
        Err(EvalError::Unsupported { node: "end_outer_islands" })
    }
}

/// 求值失败类型。`Unsupported` 会指出节点种类，便于调用方报告是哪个根
/// 触发了哪个缺口。
///
/// Evaluation failures. `Unsupported` names the node kind so the caller can
/// report which root hit which gap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    Unsupported { node: &'static str },
    /// 链接后的树中仍存在资源引用（链接阶段的 bug）。
    ///
    /// A linked tree still contained a resource reference (linking bug).
    UnlinkedRef { reference: String },
    /// 运行时违反了数值合约（例如除以采样得到的 0）。
    ///
    /// A numeric contract was violated at runtime (e.g. div by a sampled 0).
    NonFinite { node: &'static str },
    /// 请求的路由根未被编译进本计划。
    ///
    /// A requested router root was not compiled into this plan.
    UnknownRoot { root: String },
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { node } => {
                write!(f, "reference evaluator does not support node '{node}'")
            }
            Self::UnlinkedRef { reference } => {
                write!(f, "unlinked reference '{reference}' reached evaluation")
            }
            Self::NonFinite { node } => {
                write!(f, "node '{node}' produced a non-finite value")
            }
            Self::UnknownRoot { root } => {
                write!(f, "plan does not contain root '{root}'")
            }
        }
    }
}

impl std::error::Error for EvalError {}

/// 在给定采样位置求值单个节点；噪声采样委托给 `sampler`。
/// 返回该节点的 f32 取值（以 f64 承载，见模块文档的数值合同）；
/// 节点违反数值合同时返回 `EvalError`。
///
/// Evaluates one node at a sample position. The returned value is an f32
/// (carried in f64) — see the module docs for the numeric contract.
pub fn evaluate(
    node: &Node,
    ctx: &SampleContext,
    sampler: &dyn NoiseSampler,
) -> Result<f64, EvalError> {
    let kind_of = |n: &Node| node_kind(n);
    match node {
        // Constants are parsed as f32.
        Node::Constant(value) => Ok(f64::from(*value as f32)),
        Node::Coord(Axis::X) => Ok(ctx.x),
        Node::Coord(Axis::Y) => Ok(ctx.y),
        Node::Coord(Axis::Z) => Ok(ctx.z),
        Node::Ref(reference) => Err(EvalError::UnlinkedRef {
            reference: reference.clone(),
        }),
        Node::Noise { noise, xz_scale, y_scale, shift_x, shift_z } => {
            // Shifted coordinates are computed in f64; the noise stack
            // narrows to f32 internally.
            let shifted_x = ctx.x * xz_scale
                + match shift_x {
                    Some(shift) => evaluate(shift, ctx, sampler)?,
                    None => 0.0,
                };
            let shifted_y = ctx.y * y_scale;
            let shifted_z = ctx.z * xz_scale
                + match shift_z {
                    Some(shift) => evaluate(shift, ctx, sampler)?,
                    None => 0.0,
                };
            Ok(sampler.sample(noise, shifted_x, shifted_y, shifted_z))
        }
        // shift_a: sample at (x * 0.25, 0.0, z * 0.25) and multiply by 4.0 in f32.
        Node::ShiftA { noise } => {
            let sampled = sampler.sample(noise, ctx.x * 0.25, 0.0, ctx.z * 0.25) as f32;
            Ok(f64::from(sampled * 4.0))
        }
        // shift_b: sample at (z * 0.25, x * 0.25, 0.0) and multiply by 4.0 in f32.
        Node::ShiftB { noise } => {
            let sampled = sampler.sample(noise, ctx.z * 0.25, ctx.x * 0.25, 0.0) as f32;
            Ok(f64::from(sampled * 4.0))
        }
        Node::Add(a, b) => {
            let left = evaluate(a, ctx, sampler)? as f32;
            let right = evaluate(b, ctx, sampler)? as f32;
            Ok(f64::from(left + right))
        }
        Node::Sub(a, b) => {
            let left = evaluate(a, ctx, sampler)? as f32;
            let right = evaluate(b, ctx, sampler)? as f32;
            Ok(f64::from(left - right))
        }
        Node::Mul(a, b) => {
            // mul short-circuits the second operand when the first is 0.0.
            let left = evaluate(a, ctx, sampler)? as f32;
            if left == 0.0 {
                Ok(0.0)
            } else {
                let right = evaluate(b, ctx, sampler)? as f32;
                Ok(f64::from(left * right))
            }
        }
        Node::Div(a, b) => {
            let left = evaluate(a, ctx, sampler)? as f32;
            let right = evaluate(b, ctx, sampler)? as f32;
            let value = left / right;
            if value.is_finite() {
                Ok(f64::from(value))
            } else {
                Err(EvalError::NonFinite { node: kind_of(node) })
            }
        }
        Node::Min(a, b) => {
            let left = evaluate(a, ctx, sampler)? as f32;
            let right = evaluate(b, ctx, sampler)? as f32;
            Ok(f64::from(left.min(right)))
        }
        Node::Max(a, b) => {
            let left = evaluate(a, ctx, sampler)? as f32;
            let right = evaluate(b, ctx, sampler)? as f32;
            Ok(f64::from(left.max(right)))
        }
        Node::Negate(a) => {
            let value = evaluate(a, ctx, sampler)? as f32;
            Ok(f64::from(-value))
        }
        Node::Abs(a) => {
            let value = evaluate(a, ctx, sampler)? as f32;
            Ok(f64::from(value.abs()))
        }
        Node::Square(a) => {
            let value = evaluate(a, ctx, sampler)? as f32;
            Ok(f64::from(value * value))
        }
        Node::Cube(a) => {
            let value = evaluate(a, ctx, sampler)? as f32;
            Ok(f64::from(value * value * value))
        }
        Node::HalfNegative(a) => {
            let value = evaluate(a, ctx, sampler)? as f32;
            Ok(f64::from(if value > 0.0 { value } else { value * 0.5 }))
        }
        Node::QuarterNegative(a) => {
            let value = evaluate(a, ctx, sampler)? as f32;
            Ok(f64::from(if value > 0.0 { value } else { value * 0.25 }))
        }
        Node::Squeeze(a) => {
            let value = evaluate(a, ctx, sampler)? as f32;
            // squeeze: clamp(v, -1, 1) / 2 - cube(clamped) / 24, all f32.
            let clamped = mth_clamp(value, -1.0, 1.0);
            Ok(f64::from(clamped / 2.0 - clamped * clamped * clamped / 24.0))
        }
        Node::Clamp { input, min, max } => {
            let value = evaluate(input, ctx, sampler)? as f32;
            // clamp: value < min ? min : min(value, max).
            Ok(f64::from(mth_clamp(value, *min as f32, *max as f32)))
        }
        Node::RangeChoice { input, min, max, in_range, out_of_range } => {
            let value = evaluate(input, ctx, sampler)? as f32;
            if value >= *min as f32 && value < *max as f32 {
                evaluate(in_range, ctx, sampler)
            } else {
                evaluate(out_of_range, ctx, sampler)
            }
        }
        Node::Gradient { axis, from_coordinate, from_value, to_coordinate, to_value } => {
            // gradient: integer coordinate clamp, then
            // fromValue + relative * factor (f32 division).
            let coordinate = axis_value(*axis, ctx) as i64;
            let from = *from_coordinate;
            let to = *to_coordinate;
            let min = from.min(to);
            let max = from.max(to);
            let relative = ((coordinate as f64).clamp(min, max) - from) as f32;
            let factor = (*to_value as f32 - *from_value as f32) / (to - from) as f32;
            Ok(f64::from(*from_value as f32 + relative * factor))
        }
        Node::Lerp { alpha, first, second } => {
            let alpha = evaluate(alpha, ctx, sampler)? as f32;
            let first = evaluate(first, ctx, sampler)? as f32;
            let second = evaluate(second, ctx, sampler)? as f32;
            // lerp short-circuits at alpha 0.0 / 1.0 (1-ulp observable).
            let value = if alpha == 0.0 {
                first
            } else if alpha == 1.0 {
                second
            } else {
                mth_lerp(alpha, first, second)
            };
            Ok(f64::from(value))
        }
        Node::Cache(input) => evaluate(input, ctx, sampler),
        Node::Interpolated { cell_size_xz, cell_size_y, input } => {
            evaluate_interpolated(*cell_size_xz, *cell_size_y, input, ctx, sampler)
        }
        Node::Slice { axis, coordinate, input } => {
            let sliced = SampleContext {
                x: if *axis == Axis::X { *coordinate } else { ctx.x },
                y: if *axis == Axis::Y { *coordinate } else { ctx.y },
                z: if *axis == Axis::Z { *coordinate } else { ctx.z },
            };
            evaluate(input, &sliced, sampler)
        }
        Node::IntervalSelect { input, thresholds, functions } => {
            let value = evaluate(input, ctx, sampler)? as f32;
            for (index, threshold) in thresholds.iter().enumerate() {
                if value < *threshold as f32 {
                    return evaluate(&functions[index], ctx, sampler);
                }
            }
            evaluate(functions.last().expect("validated non-empty"), ctx, sampler)
        }
        Node::Spline(spline) => evaluate_spline(spline, ctx, sampler),
        Node::DistanceToPoint { point } => {
            // euclidean distance: int deltas, f32 lengthSquared,
            // f64 sqrt, narrowed to f32.
            let dx = (point[0] as i64 - ctx.x as i64) as f32;
            let dy = (point[1] as i64 - ctx.y as i64) as f32;
            let dz = (point[2] as i64 - ctx.z as i64) as f32;
            let squared = dx * dx + dy * dy + dz * dz;
            Ok(f64::from((f64::from(squared).sqrt()) as f32))
        }
        Node::FindTopSurface { cell_height, density, lower_bound, upper_bound } => {
            // find_top_surface: topY = floor(upper/cellHeight)*cellHeight
            // (f32 division); scan downward in cell steps for the first
            // density > 0.0, otherwise fall through to lowerBound.
            let upper = evaluate(upper_bound, ctx, sampler)? as f32;
            let cell = *cell_height;
            let top_y = (upper / cell as f32).floor() as i64 * cell;
            let lower = *lower_bound;
            if top_y <= lower {
                Ok(lower as f64)
            } else {
                let mut probe_y = top_y;
                while probe_y >= lower {
                    let probe_ctx = SampleContext {
                        x: ctx.x,
                        y: probe_y as f64,
                        z: ctx.z,
                    };
                    if evaluate(density, &probe_ctx, sampler)? > 0.0 {
                        return Ok(probe_y as f64);
                    }
                    probe_y -= cell;
                }
                Ok(lower as f64)
            }
        }
        Node::OldBlendedNoise { xz_scale, y_scale, xz_factor, y_factor, smear_scale_multiplier } => {
            sampler.sample_old_blended(
                &OldBlendedParams {
                    xz_scale: *xz_scale,
                    y_scale: *y_scale,
                    xz_factor: *xz_factor,
                    y_factor: *y_factor,
                    smear_scale_multiplier: *smear_scale_multiplier,
                },
                ctx.x,
                ctx.y,
                ctx.z,
            )
        }
        Node::EndOuterIslands => {
            sampler.sample_end_islands(ctx.x as i32, ctx.z as i32)
        }
        Node::Beardifier => Ok(0.0),
        Node::BlendAlpha => Ok(1.0),
        Node::BlendOffset => Ok(0.0),
        Node::BlendDensity(input) => evaluate(input, ctx, sampler),
    }
}

/// 钳制单个值：`value < min` 时返回 `min`，否则返回 `value.min(max)`（f32）。
///
/// Clamps one value: returns `min` when `value < min`, else `value.min(max)`
/// (f32).
#[inline]
pub(crate) fn mth_clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min {
        min
    } else {
        value.min(max)
    }
}

/// 线性插值：`p0 + alpha * (p1 - p0)`（f32）。
///
/// Linear interpolation: `p0 + alpha * (p1 - p0)` (f32).
#[inline]
pub(crate) fn mth_lerp(alpha: f32, p0: f32, p1: f32) -> f32 {
    p0 + alpha * (p1 - p0)
}

fn axis_value(axis: Axis, ctx: &SampleContext) -> f64 {
    match axis {
        Axis::X => ctx.x,
        Axis::Y => ctx.y,
        Axis::Z => ctx.z,
    }
}

/// 单元格三线性插值：`floorMod` 定位单元格、采样 8 个角点、f32 三线性
/// lerp。要求整数块坐标。
///
/// Trilinear interpolation over a cell: `floorMod` cell, 8 corners, f32
/// trilinear lerp. Requires integer block coordinates.
fn evaluate_interpolated(
    cell_size_xz: i64,
    cell_size_y: i64,
    input: &Node,
    ctx: &SampleContext,
    sampler: &dyn NoiseSampler,
) -> Result<f64, EvalError> {
    let block_x = ctx.x as i64;
    let block_y = ctx.y as i64;
    let block_z = ctx.z as i64;
    let x_in_cell = block_x.rem_euclid(cell_size_xz);
    let y_in_cell = block_y.rem_euclid(cell_size_y);
    let z_in_cell = block_z.rem_euclid(cell_size_xz);
    if x_in_cell == 0 && y_in_cell == 0 && z_in_cell == 0 {
        return evaluate(input, ctx, sampler);
    }
    let x0 = block_x - x_in_cell;
    let y0 = block_y - y_in_cell;
    let z0 = block_z - z_in_cell;
    let corner = |cx: i64, cy: i64, cz: i64| {
        evaluate(
            input,
            &SampleContext {
                x: cx as f64,
                y: cy as f64,
                z: cz as f64,
            },
            sampler,
        )
    };
    let c000 = corner(x0, y0, z0)? as f32;
    let c100 = corner(x0 + cell_size_xz, y0, z0)? as f32;
    let c010 = corner(x0, y0 + cell_size_y, z0)? as f32;
    let c110 = corner(x0 + cell_size_xz, y0 + cell_size_y, z0)? as f32;
    let c001 = corner(x0, y0, z0 + cell_size_xz)? as f32;
    let c101 = corner(x0 + cell_size_xz, y0, z0 + cell_size_xz)? as f32;
    let c011 = corner(x0, y0 + cell_size_y, z0 + cell_size_xz)? as f32;
    let c111 = corner(x0 + cell_size_xz, y0 + cell_size_y, z0 + cell_size_xz)? as f32;
    let tx = x_in_cell as f32 / cell_size_xz as f32;
    let ty = y_in_cell as f32 / cell_size_y as f32;
    let tz = z_in_cell as f32 / cell_size_xz as f32;
    Ok(f64::from(lerp3(tx, ty, tz, c000, c100, c010, c110, c001, c101, c011, c111)))
}

/// 三线性插值（f32）：先在 x/y 上做一次 `lerp2`，再在 z 上混合两个结果。
///
/// Trilinear interpolation (f32): two `lerp2` results blended along z.
#[inline]
pub(crate) fn lerp3(
    a1: f32,
    a2: f32,
    a3: f32,
    x000: f32,
    x100: f32,
    x010: f32,
    x110: f32,
    x001: f32,
    x101: f32,
    x011: f32,
    x111: f32,
) -> f32 {
    mth_lerp(
        a3,
        lerp2(a1, a2, x000, x100, x010, x110),
        lerp2(a1, a2, x001, x101, x011, x111),
    )
}

/// 双线性插值（f32）：在 x 方向插值两次，再在 y 方向混合。
///
/// Bilinear interpolation (f32): two x-direction lerps blended along y.
#[inline]
pub(crate) fn lerp2(a1: f32, a2: f32, x00: f32, x10: f32, x01: f32, x11: f32) -> f32 {
    mth_lerp(a2, mth_lerp(a1, x00, x10), mth_lerp(a1, x01, x11))
}

/// 求值多点样条：区间内用 f32 Hermite 混合，区间外用 `linearExtend`
/// 边界处理，区间定位用二分搜索语义。
///
/// Evaluates a multipoint spline: f32 Hermite blending inside the interval,
/// `linearExtend` outside it, and binary-search interval location.
fn evaluate_spline(
    spline: &Spline,
    ctx: &SampleContext,
    sampler: &dyn NoiseSampler,
) -> Result<f64, EvalError> {
    match spline {
        Spline::Constant(value) => Ok(f64::from(*value as f32)),
        Spline::Multi { coordinate, points } => {
            let x = evaluate(coordinate, ctx, sampler)? as f32;
            let locations: Vec<f32> = points.iter().map(|(location, _, _)| *location as f32).collect();
            let start = mth_binary_search(&locations, x) as i32 - 1;
            let last = (points.len() - 1) as i32;
            if start < 0 {
                let value = evaluate_spline(&points[0].1, ctx, sampler)? as f32;
                return Ok(f64::from(linear_extend(x, locations[0], value, points[0].2 as f32)));
            }
            if start == last {
                let value = evaluate_spline(&points[last as usize].1, ctx, sampler)? as f32;
                return Ok(f64::from(linear_extend(
                    x,
                    locations[last as usize],
                    value,
                    points[last as usize].2 as f32,
                )));
            }
            let index = start as usize;
            let x1 = locations[index];
            let x2 = locations[index + 1];
            let t = (x - x1) / (x2 - x1);
            let y1 = evaluate_spline(&points[index].1, ctx, sampler)? as f32;
            let y2 = evaluate_spline(&points[index + 1].1, ctx, sampler)? as f32;
            let d1 = points[index].2 as f32;
            let d2 = points[index + 1].2 as f32;
            let a = d1 * (x2 - x1) - (y2 - y1);
            let b = -d2 * (x2 - x1) + (y2 - y1);
            let blended = mth_lerp(t, y1, y2) + t * (1.0 - t) * mth_lerp(t, a, b);
            Ok(f64::from(blended))
        }
    }
}

/// 样条区间外扩展（f32）：导数为 0 时返回常量 `value`，否则沿导数线性外推。
///
/// Out-of-range spline extension (f32): the constant `value` when the
/// derivative is 0, otherwise a linear extrapolation along it.
#[inline]
pub(crate) fn linear_extend(input: f32, location: f32, value: f32, derivative: f32) -> f32 {
    if derivative == 0.0 {
        value
    } else {
        value + derivative * (input - location)
    }
}

/// 返回第一个满足 `input < locations[i]` 的下标；全不满足时返回
/// `locations.len()`。
///
/// Returns the first index where `input < locations[i]` holds
/// (`locations.len()` when none does).
pub(crate) fn mth_binary_search(locations: &[f32], input: f32) -> usize {
    let mut from = 0usize;
    let mut len = locations.len();
    while len > 0 {
        let half = len / 2;
        let middle = from + half;
        if input < locations[middle] {
            len = half;
        } else {
            from = middle + 1;
            len -= half + 1;
        }
    }
    from
}

fn node_kind(node: &Node) -> &'static str {
    match node {
        Node::Constant(_) => "constant",
        Node::Coord(_) => "coord",
        Node::Ref(_) => "ref",
        Node::Noise { .. } => "noise",
        Node::ShiftA { .. } => "shift_a",
        Node::ShiftB { .. } => "shift_b",
        Node::Add(_, _) => "add",
        Node::Sub(_, _) => "sub",
        Node::Mul(_, _) => "mul",
        Node::Div(_, _) => "div",
        Node::Min(_, _) => "min",
        Node::Max(_, _) => "max",
        Node::Negate(_) => "negate",
        Node::Abs(_) => "abs",
        Node::Square(_) => "square",
        Node::Cube(_) => "cube",
        Node::HalfNegative(_) => "half_negative",
        Node::QuarterNegative(_) => "quarter_negative",
        Node::Squeeze(_) => "squeeze",
        Node::Clamp { .. } => "clamp",
        Node::RangeChoice { .. } => "range_choice",
        Node::Gradient { .. } => "gradient",
        Node::Lerp { .. } => "lerp",
        Node::Cache(_) => "cache",
        Node::Interpolated { .. } => "interpolated",
        Node::Slice { .. } => "slice",
        Node::IntervalSelect { .. } => "interval_select",
        Node::Spline(_) => "spline",
        Node::DistanceToPoint { .. } => "distance_to_point",
        Node::FindTopSurface { .. } => "find_top_surface",
        Node::OldBlendedNoise { .. } => "old_blended_noise",
        Node::EndOuterIslands => "end_outer_islands",
        Node::Beardifier => "beardifier",
        Node::BlendAlpha => "blend_alpha",
        Node::BlendOffset => "blend_offset",
        Node::BlendDensity(_) => "blend_density",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 确定性桩采样器：取值只依赖噪声名与位置的整数格点，
    /// 因此期望值精确可复现。
    ///
    /// Deterministic stub sampler: value depends only on the noise name and
    /// the integer lattice of the position, so expectations are exact.
    struct StubSampler;

    impl NoiseSampler for StubSampler {
        fn sample(&self, noise: &str, x: f64, y: f64, z: f64) -> f64 {
            let lattice = x as i64 ^ (y as i64).wrapping_mul(3) ^ (z as i64).wrapping_mul(7);
            let name: i64 = noise.bytes().map(|b| i64::from(b)).sum::<i64>();
            f64::from((lattice.wrapping_add(name) % 100) as i32) / 100.0
        }
    }

    const SAMPLE: SampleContext = SampleContext { x: 12.0, y: -32.0, z: 5.0 };
    const SAMPLER: StubSampler = StubSampler;

    fn eval(node: &Node) -> f64 {
        evaluate(node, &SAMPLE, &SAMPLER).unwrap()
    }

    fn constant(value: f64) -> Box<Node> {
        Box::new(Node::Constant(value))
    }

    #[test]
    fn arithmetic_matches_java_contracts() {
        assert_eq!(eval(&Node::Add(constant(1.0), constant(2.0))), 3.0);
        assert_eq!(eval(&Node::Sub(constant(1.0), constant(2.0))), -1.0);
        assert_eq!(eval(&Node::Min(constant(1.0), constant(2.0))), 1.0);
        assert_eq!(eval(&Node::Max(constant(1.0), constant(2.0))), 2.0);
        assert_eq!(eval(&Node::Div(constant(1.0), constant(4.0))), 0.25);
        // Division by a sampled zero is a loud runtime error, not NaN.
        assert!(matches!(
            evaluate(&Node::Div(constant(1.0), constant(0.0)), &SAMPLE, &SAMPLER),
            Err(EvalError::NonFinite { node: "div" })
        ));
        assert_eq!(eval(&Node::Negate(constant(3.0))), -3.0);
        assert_eq!(eval(&Node::Abs(constant(-3.0))), 3.0);
        assert_eq!(eval(&Node::Square(constant(3.0))), 9.0);
        assert_eq!(eval(&Node::Cube(constant(2.0))), 8.0);
        // Half/quarter negative: strictly-positive passthrough.
        assert_eq!(eval(&Node::HalfNegative(constant(4.0))), 4.0);
        assert_eq!(eval(&Node::HalfNegative(constant(-4.0))), -2.0);
        assert_eq!(eval(&Node::QuarterNegative(constant(-4.0))), -1.0);
        // Squeeze: clamp to (-1, 1) then c/2 - c^3/24, all f32.
        assert_eq!(
            eval(&Node::Squeeze(constant(5.0))),
            f64::from(1.0f32 / 2.0 - 1.0f32 / 24.0)
        );
        assert_eq!(
            eval(&Node::Squeeze(constant(0.5))),
            f64::from(0.5f32 / 2.0 - (0.5f32 * 0.5 * 0.5) / 24.0)
        );
    }

    #[test]
    fn clamp_range_choice_gradient_and_lerp() {
        assert_eq!(
            eval(&Node::Clamp { input: constant(7.0), min: -1.0, max: 1.0 }),
            1.0
        );
        // range_choice: [min, max) — the max boundary goes out of range.
        let choice = |input: f64| {
            eval(&Node::RangeChoice {
                input: constant(input),
                min: 0.0,
                max: 1.0,
                in_range: constant(10.0),
                out_of_range: constant(20.0),
            })
        };
        assert_eq!(choice(0.0), 10.0);
        assert_eq!(choice(0.999), 10.0);
        assert_eq!(choice(1.0), 20.0);
        // gradient: fraction clamps outside [from, to].
        let gradient = Node::Gradient {
            axis: Axis::Y,
            from_coordinate: -64.0,
            from_value: 0.0,
            to_coordinate: -40.0,
            to_value: 1.0,
        };
        assert_eq!(
            evaluate(&gradient, &SampleContext { x: 0.0, y: -70.0, z: 0.0 }, &SAMPLER).unwrap(),
            0.0
        ); // y=-70: below from_coordinate
        assert_eq!(
            evaluate(&gradient, &SampleContext { x: 0.0, y: -52.0, z: 0.0 }, &SAMPLER).unwrap(),
            0.5
        );
        assert_eq!(
            evaluate(&gradient, &SampleContext { x: 0.0, y: 0.0, z: 0.0 }, &SAMPLER).unwrap(),
            1.0
        );
        assert_eq!(
            eval(&Node::Lerp { alpha: constant(0.25), first: constant(2.0), second: constant(4.0) }),
            2.5
        );
    }

    #[test]
    fn new_world_context_leaves_have_fixed_values() {
        assert_eq!(eval(&Node::Beardifier), 0.0);
        assert_eq!(eval(&Node::BlendAlpha), 1.0);
        assert_eq!(eval(&Node::BlendOffset), 0.0);
        assert_eq!(eval(&Node::BlendDensity(constant(3.5))), 3.5);
        assert_eq!(eval(&Node::Cache(constant(3.5))), 3.5);
    }

    #[test]
    fn slice_fixes_one_axis() {
        // y fixed to -32 regardless of the context y.
        let node = Node::Slice {
            axis: Axis::Y,
            coordinate: -32.0,
            input: Box::new(Node::Coord(Axis::Y)),
        };
        assert_eq!(eval(&node), -32.0);
    }

    #[test]
    fn distance_to_point_is_euclidean() {
        // Euclidean distance: int deltas, f32 lengthSquared,
        // f64 sqrt narrowed to f32.
        let node = Node::DistanceToPoint { point: [0.0, 0.0, 0.0] };
        let squared = 144.0f32 + 1024.0 + 25.0;
        assert_eq!(eval(&node), f64::from((f64::from(squared).sqrt()) as f32));
    }

    #[test]
    fn noise_applies_scale_then_shift() {
        // shift evaluated at the ORIGINAL context; coordinates scaled first.
        let node = Node::Noise {
            noise: "minecraft:temperature".to_string(),
            xz_scale: 0.25,
            y_scale: 0.0,
            shift_x: Some(Box::new(Node::Coord(Axis::X))),
            shift_z: None,
        };
        let expected_sample_x = 12.0 * 0.25 + 12.0; // 0.25*x + shift_x(x,y,z)
        let stub = StubSampler;
        assert_eq!(
            evaluate(&node, &SAMPLE, &stub).unwrap(),
            stub.sample("minecraft:temperature", expected_sample_x, -32.0 * 0.0, 5.0 * 0.25)
        );
        // shift_a/shift_b: quarter-scale sample times 4 (f32), axes as above.
        assert_eq!(
            eval(&Node::ShiftA { noise: "minecraft:t".to_string() }),
            f64::from(stub.sample("minecraft:t", 12.0 * 0.25, 0.0, 5.0 * 0.25) as f32 * 4.0)
        );
        assert_eq!(
            eval(&Node::ShiftB { noise: "minecraft:t".to_string() }),
            f64::from(stub.sample("minecraft:t", 5.0 * 0.25, 12.0 * 0.25, 0.0) as f32 * 4.0)
        );
    }

    #[test]
    fn interval_select_picks_by_thresholds() {
        let node = Node::IntervalSelect {
            input: constant(0.05),
            thresholds: vec![-0.1, 0.1],
            functions: vec![Node::Constant(1.0), Node::Constant(2.0), Node::Constant(3.0)],
        };
        assert_eq!(eval(&node), 2.0); // 0.05 >= -0.1 and < 0.1
    }

    #[test]
    fn spline_clamps_outside_and_interpolates_inside() {
        // Constant-valued spline over a linear coordinate: exact cubic math.
        let spline = Spline::Multi {
            coordinate: Box::new(Node::Coord(Axis::Y)),
            points: vec![
                (-64.0, Spline::Constant(0.0), 0.0),
                (-40.0, Spline::Constant(4.0), 0.0),
                (320.0, Spline::Constant(4.0), 0.0),
            ],
        };
        // Below the first point: clamped.
        assert_eq!(
            evaluate(&Node::Spline(spline.clone()), &SampleContext { x: 0.0, y: -100.0, z: 0.0 }, &SAMPLER)
                .unwrap(),
            0.0
        );
        // Between -64 and -40 with zero derivatives: linear halfway = 2.
        assert_eq!(
            evaluate(&Node::Spline(spline.clone()), &SampleContext { x: 0.0, y: -52.0, z: 0.0 }, &SAMPLER)
                .unwrap(),
            2.0
        );
        // Above the last point: clamped.
        assert_eq!(
            evaluate(&Node::Spline(spline), &SampleContext { x: 0.0, y: 400.0, z: 0.0 }, &SAMPLER)
                .unwrap(),
            4.0
        );
    }

    #[test]
    fn interpolated_trilerps_cell_corners() {
        // Input = X coordinate; cell 4: x0=12, x1=16, t=0 (on-corner) -> 12.
        let node = Node::Interpolated {
            cell_size_xz: 4,
            cell_size_y: 8,
            input: Box::new(Node::Coord(Axis::X)),
        };
        assert_eq!(eval(&node), 12.0);
        // Midpoint between 12 and 16 -> 14.
        assert_eq!(
            evaluate(&node, &SampleContext { x: 14.0, y: -32.0, z: 5.0 }, &SAMPLER).unwrap(),
            14.0
        );
        // Y interpolation with cell 8: y0=-32 (on corner) exact.
        let y_node = Node::Interpolated {
            cell_size_xz: 4,
            cell_size_y: 8,
            input: Box::new(Node::Coord(Axis::Y)),
        };
        assert_eq!(eval(&y_node), -32.0);
    }

    #[test]
    fn unsupported_leaves_are_explicit() {
        // OldBlendedNoise needs the bank runtime; end islands need the bank
        // island simplex. Both stay explicit under a stub sampler.
        for node in [
            Node::OldBlendedNoise {
                xz_scale: 0.25,
                y_scale: 0.25,
                xz_factor: 80.0,
                y_factor: 160.0,
                smear_scale_multiplier: 4.0,
            },
            Node::EndOuterIslands,
        ] {
            let error = evaluate(&node, &SAMPLE, &SAMPLER).unwrap_err();
            assert!(matches!(error, EvalError::Unsupported { .. }), "got {error:?}");
        }
    }

    #[test]
    fn find_top_surface_returns_top_y_for_constant_density() {
        // Constant density/upper_bound samplers make the expected values
        // seed-independent.
        let fts = |density: f64, upper: f64, lower: i64| Node::FindTopSurface {
            cell_height: 8,
            density: constant(density),
            lower_bound: lower,
            upper_bound: constant(upper),
        };
        // density 5 everywhere: topY = floor(100/8)*8 = 96, first positive probe.
        assert_eq!(eval(&fts(5.0, 100.0, -64)), 96.0);
        // density 0: no probe > 0 → lowerBound.
        assert_eq!(eval(&fts(0.0, 20.0, -64)), -64.0);
        // lowerBound 50 > topY 16 → lowerBound.
        assert_eq!(eval(&fts(5.0, 20.0, 50)), 50.0);
        // negative density everywhere → lowerBound.
        assert_eq!(eval(&fts(-5.0, 100.0, -64)), -64.0);
    }

    #[test]
    fn find_top_surface_stops_at_first_positive_density_cell() {
        // Density is positive only *below* y = 40 (cell step 8), so the
        // downward scan from topY=96 must stop at the first crossing: 32.
        let density = Node::RangeChoice {
            input: Box::new(Node::Coord(Axis::Y)),
            min: -1000.0,
            max: 40.0,
            in_range: Box::new(Node::Constant(5.0)),
            out_of_range: Box::new(Node::Constant(-5.0)),
        };
        let node = Node::FindTopSurface {
            cell_height: 8,
            density: Box::new(density),
            lower_bound: -64,
            upper_bound: Box::new(Node::Constant(100.0)),
        };
        // Probes 96, 88, … 40 (all negative), then 32 (positive) → 32.
        assert_eq!(eval(&node), 32.0);
    }
}
