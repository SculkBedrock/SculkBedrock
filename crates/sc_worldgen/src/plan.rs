//! 密度计划：链接后密度树的预构建扁平执行形态。
//!
//! 计划取代运行时的 `Arc<dyn>`/`Box<Node>` 树：所有节点位于同一个后序 arena 中，
//! 结构相同的子图被交接到同一个 [`NodeId`]，常量子树在编译期折叠。
//!
//! 两条不变式使执行器成为扁平循环（见 [`crate::exec`]）：
//! 1. **后序发射**——每个子节点的 NodeId 严格小于其父节点，因此对 arena 的
//!    线性扫描即拓扑求值；
//! 2. **哈希交接**——结构相同的节点（同算子、同子 id、同常量）共享同一槽位。
//!
//! 常量折叠使用与运行时求值相同的 f64 运算顺序与短路规则；运行时可能产生非有限
//! 值的节点（如被采样出的 0 除）不折叠，保留运行时错误路径。
//!
//! DensityPlan: the prebuilt, flat execution form of a linked density tree.
//!
//! The plan replaces the runtime `Arc<dyn>`/`Box<Node>` tree: nodes live in
//! one post-order arena, structurally identical subgraphs are interned to a
//! single [`NodeId`], and constant subtrees are folded at compile time.
//!
//! Two invariants make the executor a flat loop (see [`crate::exec`]):
//! 1. **Post-order emission** — every child's NodeId is strictly smaller than
//!    its parent's, so a linear sweep over the arena is a topological
//!    evaluation;
//! 2. **Hash-consing** — structurally identical nodes (same op, same child
//!    ids, same constants) share one arena slot.
//!
//! Constant folding uses the same f64 operation order and short-circuits as
//! runtime evaluation; nodes that could produce non-finite values at runtime
//! (e.g. `div` by a sampled 0) are left unfolded so the runtime error path
//! stays reachable.

use std::collections::{BTreeMap, HashMap};

use crate::data::node::{Axis, Node, Spline};
use crate::eval::OldBlendedParams;

/// 计划 arena 的紧凑节点 id。
///
/// Compact node id into the plan arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    /// id 在 arena 中的下标。
    ///
    /// The id's index into the arena.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// 二元运算（与 [`Node`] 的算术族一一对应）。
///
/// Binary operations (mirrors [`Node`]'s arithmetic family).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
}

/// 一元运算。
///
/// Unary operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Negate,
    Abs,
    Square,
    Cube,
    HalfNegative,
    QuarterNegative,
    Squeeze,
}

/// 编译后的计划节点。子节点是 arena 下标；在树形态中需要堆装箱的参数
/// （阈值、样条点、函数列表）存放在计划的侧表中，以区间引用。
///
/// A compiled plan node. Children are arena indices; parameters that would
/// need heap boxes in the tree form (thresholds, spline points, function
/// lists) live in the plan's side tables and are referenced by ranges.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanNode {
    Constant(f64),
    Coord(Axis),
    /// `minecraft:noise`：采样器在 `plan.noises` 中的下标 + 可选的 shift
    /// 子节点（先缩放后叠加）。
    ///
    /// `minecraft:noise`: sampler index into `plan.noises` + optional shift
    /// children (applied scale-then-add).
    Noise {
        noise: u32,
        xz_scale: f64,
        y_scale: f64,
        shift_x: Option<NodeId>,
        shift_z: Option<NodeId>,
    },
    ShiftA(u32),
    ShiftB(u32),
    Binary(BinaryOp, NodeId, NodeId),
    Unary(UnaryOp, NodeId),
    Clamp {
        input: NodeId,
        min: f64,
        max: f64,
    },
    RangeChoice {
        input: NodeId,
        min: f64,
        max: f64,
        in_range: NodeId,
        out_of_range: NodeId,
    },
    Gradient {
        axis: Axis,
        from_coordinate: f64,
        from_value: f64,
        to_coordinate: f64,
        to_value: f64,
    },
    Lerp {
        alpha: NodeId,
        first: NodeId,
        second: NodeId,
    },
    Interpolated {
        cell_size_xz: i32,
        cell_size_y: i32,
        input: NodeId,
    },
    Slice {
        axis: Axis,
        coordinate: f64,
        input: NodeId,
    },
    IntervalSelect {
        input: NodeId,
        /// `plan.thresholds` 中的区间。
        ///
        /// Range into `plan.thresholds`.
        thresholds: (u32, u32),
        /// `plan.function_ids` 中的区间。
        ///
        /// Range into `plan.function_ids`.
        functions: (u32, u32),
    },
    DistanceToPoint {
        point: [f64; 3],
    },
    FindTopSurface {
        cell_height: i32,
        density: NodeId,
        lower_bound: i32,
        upper_bound: NodeId,
    },
    /// `old_blended_noise`：`plan.blended_params` 中的下标。
    ///
    /// `old_blended_noise`: index into `plan.blended_params`.
    OldBlended(u32),
    EndOuterIslands,
    /// 新世界上下文叶子（beardifier 结构修饰器）。
    ///
    /// New-world context leaf (beardifier).
    Beardifier,
    BlendAlpha,
    BlendOffset,
    BlendDensity(NodeId),
    Spline {
        coordinate: NodeId,
        /// `plan.spline_points` 中的区间。
        ///
        /// Range into `plan.spline_points`.
        points: (u32, u32),
    },
}


// f64 constants are parse-validated finite (and NaN-free), so the manual
// `Eq`/`Hash` impls below are sound: structural equality is bit equality
// and hashing uses the bit pattern.
impl Eq for PlanNode {}

impl std::hash::Hash for PlanNode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            PlanNode::Constant(value) => value.to_bits().hash(state),
            PlanNode::Coord(axis) => (*axis as u8).hash(state),
            PlanNode::Noise { noise, xz_scale, y_scale, shift_x, shift_z } => {
                noise.hash(state);
                xz_scale.to_bits().hash(state);
                y_scale.to_bits().hash(state);
                shift_x.hash(state);
                shift_z.hash(state);
            }
            PlanNode::ShiftA(noise) | PlanNode::ShiftB(noise) => noise.hash(state),
            PlanNode::Binary(op, a, b) => {
                (*op as u8).hash(state);
                a.hash(state);
                b.hash(state);
            }
            PlanNode::Unary(op, input) => {
                (*op as u8).hash(state);
                input.hash(state);
            }
            PlanNode::Clamp { input, min, max } => {
                input.hash(state);
                min.to_bits().hash(state);
                max.to_bits().hash(state);
            }
            PlanNode::RangeChoice { input, min, max, in_range, out_of_range } => {
                input.hash(state);
                min.to_bits().hash(state);
                max.to_bits().hash(state);
                in_range.hash(state);
                out_of_range.hash(state);
            }
            PlanNode::Gradient { axis, from_coordinate, from_value, to_coordinate, to_value } => {
                (*axis as u8).hash(state);
                from_coordinate.to_bits().hash(state);
                from_value.to_bits().hash(state);
                to_coordinate.to_bits().hash(state);
                to_value.to_bits().hash(state);
            }
            PlanNode::Lerp { alpha, first, second } => {
                alpha.hash(state);
                first.hash(state);
                second.hash(state);
            }
            PlanNode::Interpolated { cell_size_xz, cell_size_y, input } => {
                cell_size_xz.hash(state);
                cell_size_y.hash(state);
                input.hash(state);
            }
            PlanNode::Slice { axis, coordinate, input } => {
                (*axis as u8).hash(state);
                coordinate.to_bits().hash(state);
                input.hash(state);
            }
            PlanNode::IntervalSelect { input, thresholds, functions } => {
                input.hash(state);
                thresholds.hash(state);
                functions.hash(state);
            }
            PlanNode::DistanceToPoint { point } => {
                for component in point {
                    component.to_bits().hash(state);
                }
            }
            PlanNode::FindTopSurface { cell_height, density, lower_bound, upper_bound } => {
                cell_height.hash(state);
                density.hash(state);
                lower_bound.hash(state);
                upper_bound.hash(state);
            }
            PlanNode::OldBlended(index) => index.hash(state),
            PlanNode::EndOuterIslands
            | PlanNode::Beardifier
            | PlanNode::BlendAlpha
            | PlanNode::BlendOffset => {}
            PlanNode::BlendDensity(input) => input.hash(state),
            PlanNode::Spline { coordinate, points } => {
                coordinate.hash(state);
                points.hash(state);
            }
        }
    }
}

/// 单个样条点：`(位置, 取值节点, 导数)`。
///
/// One spline point: `(location, value node, derivative)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplinePoint {
    pub location: f64,
    pub value: NodeId,
    pub derivative: f64,
}

/// 单个根树的编译计划。
///
/// The compiled plan for one root tree.
#[derive(Debug, Clone)]
pub struct DensityPlan {
    nodes: Vec<PlanNode>,
    root: NodeId,
    roots: BTreeMap<String, NodeId>,
    /// 采样器名称表，下标即 `PlanNode::Noise`/`ShiftA`/`ShiftB` 使用的 id。
    ///
    /// Sampler names indexed by `PlanNode::Noise`/`ShiftA`/`ShiftB` ids.
    noises: Vec<String>,
    noise_index: HashMap<String, u32>,
    /// `old_blended_noise` 参数形状表。
    ///
    /// `old_blended_noise` parameter shapes.
    blended_params: Vec<OldBlendedParams>,
    /// 侧表（阈值 / 函数 id / 样条点）。
    ///
    /// Side tables (thresholds / function ids / spline points).
    thresholds: Vec<f64>,
    function_ids: Vec<NodeId>,
    spline_points: Vec<SplinePoint>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::node::{Axis, Node};

    #[test]
    fn node_id_is_compact_and_ordered() {
        let a = NodeId(3);
        let b = NodeId(7);
        assert_eq!(a.index(), 3);
        assert!(a < b, "NodeId ordering is the post-order guarantee");
    }

    #[test]
    fn constants_fold_at_compile_time() {
        let tree = Node::Add(
            Box::new(Node::Constant(1.0)),
            Box::new(Node::Sub(
                Box::new(Node::Constant(5.0)),
                Box::new(Node::Constant(2.0)),
            )),
        );
        let plan = DensityPlan::compile(&tree);
        assert_eq!(plan.node_count(), 1, "the whole tree folds to one constant");
        assert_eq!(plan.node(plan.root()), &PlanNode::Constant(4.0));
    }

    #[test]
    fn zero_multiplication_folds_via_the_short_circuit() {
        // mul(0, x) folds to 0 per the evaluator's short-circuit contract.
        let tree = Node::Mul(
            Box::new(Node::Constant(0.0)),
            Box::new(Node::Coord(Axis::Y)),
        );
        let plan = DensityPlan::compile(&tree);
        assert_eq!(plan.node_count(), 1);
        assert_eq!(plan.node(plan.root()), &PlanNode::Constant(0.0));
    }

    #[test]
    fn non_finite_folds_stay_unfolded() {
        // div by zero must remain a runtime error, not a compile-time NaN.
        let tree = Node::Div(Box::new(Node::Constant(1.0)), Box::new(Node::Constant(0.0)));
        let plan = DensityPlan::compile(&tree);
        assert!(
            matches!(plan.node(plan.root()), PlanNode::Binary(BinaryOp::Div, _, _)),
            "the runtime error path must stay observable"
        );
    }

    #[test]
    fn shared_subgraphs_are_interned() {
        // (x + y) referenced twice through an inlined ref: hash-consing must
        // emit the shared `y * 2` subtree once.
        let shared = Node::Mul(Box::new(Node::Coord(Axis::Y)), Box::new(Node::Constant(2.0)));
        let tree = Node::Add(
            Box::new(shared.clone()),
            Box::new(Node::Mul(Box::new(shared), Box::new(Node::Constant(3.0)))),
        );
        let plan = DensityPlan::compile(&tree);
        // Expected emission: coord-y, 2, mul, add, 3, mul = 6, not 7.
        assert_eq!(plan.node_count(), 6, "the shared subtree must be one arena slot");
    }

    #[test]
    fn cache_nodes_pass_through() {
        let tree = Node::Cache(Box::new(Node::Constant(1.5)));
        let plan = DensityPlan::compile(&tree);
        assert_eq!(plan.node_count(), 1);
        assert_eq!(plan.node(plan.root()), &PlanNode::Constant(1.5));
    }

    #[test]
    fn noise_names_dedupe_into_one_table() {
        let tree = Node::Add(
            Box::new(Node::Noise {
                noise: "minecraft:temperature".to_string(),
                xz_scale: 1.0,
                y_scale: 1.0,
                shift_x: None,
                shift_z: None,
            }),
            Box::new(Node::Noise {
                noise: "minecraft:temperature".to_string(),
                xz_scale: 1.0,
                y_scale: 1.0,
                shift_x: None,
                shift_z: None,
            }),
        );
        let plan = DensityPlan::compile(&tree);
        assert_eq!(plan.noises().len(), 1, "identical sampler names dedupe");
        assert_eq!(plan.node_count(), 2, "both noise nodes intern to one slot");
    }

    #[test]
    fn multi_root_plan_shares_subgraphs_and_preserves_post_order() {
        let mut roots = BTreeMap::new();
        let shared = Node::Mul(Box::new(Node::Coord(Axis::Y)), Box::new(Node::Constant(2.0)));
        roots.insert(
            "temp".to_string(),
            Node::Add(Box::new(shared.clone()), Box::new(Node::Constant(10.0))),
        );
        roots.insert(
            "veg".to_string(),
            Node::Add(Box::new(shared), Box::new(Node::Constant(20.0))),
        );
        let plan = DensityPlan::compile_multi(&roots);
        assert_eq!(plan.roots().len(), 2);
        let id_temp = plan.root_id("temp").unwrap();
        let id_veg = plan.root_id("veg").unwrap();
        assert_ne!(id_temp, id_veg);

        // Subtree `Y * 2` is shared:
        // Nodes: Coord(Y), 2.0, Mul, 10.0, Add("temp"), 20.0, Add("veg") -> total 7 nodes
        assert_eq!(plan.node_count(), 7);
    }
}
// The compiler: hash-consed emission with constant folding.

impl DensityPlan {
    /// 把链接后的树编译为计划。输入的树必须已链接（无 [`Node::Ref`] 叶子）
    /// ——链接阶段的校验保证这一点。
    ///
    /// Compiles a linked tree into a plan. The tree must already be linked
    /// (no [`Node::Ref`] leaves) — link-time validation guarantees that.
    pub fn compile(root: &Node) -> Self {
        let plan = Self {
            nodes: Vec::new(),
            root: NodeId(0),
            roots: BTreeMap::new(),
            noises: Vec::new(),
            noise_index: HashMap::new(),
            blended_params: Vec::new(),
            thresholds: Vec::new(),
            function_ids: Vec::new(),
            spline_points: Vec::new(),
        };
        let mut compiler = Compiler { plan, memo: HashMap::new() };
        let root_id = compiler.compile(root);
        compiler.plan.root = root_id;
        compiler.plan.roots.insert("root".to_string(), root_id);
        compiler.plan.prune();
        compiler.plan
    }

    /// 把多个具名根编译进同一个计划 arena，跨根哈希交接并统一常量折叠。
    ///
    /// Compiles multiple named roots into one shared plan arena with
    /// inter-root hash-consing and unified constant folding.
    pub fn compile_multi(roots: &BTreeMap<String, Node>) -> Self {
        let plan = Self {
            nodes: Vec::new(),
            root: NodeId(0),
            roots: BTreeMap::new(),
            noises: Vec::new(),
            noise_index: HashMap::new(),
            blended_params: Vec::new(),
            thresholds: Vec::new(),
            function_ids: Vec::new(),
            spline_points: Vec::new(),
        };
        let mut compiler = Compiler { plan, memo: HashMap::new() };
        for (name, node) in roots {
            let root_id = compiler.compile(node);
            compiler.plan.roots.insert(name.clone(), root_id);
        }
        if let Some(&fd_id) = compiler.plan.roots.get("final_density") {
            compiler.plan.root = fd_id;
        } else if let Some(&first_id) = compiler.plan.roots.values().next() {
            compiler.plan.root = first_id;
        }
        compiler.plan.prune();
        compiler.plan
    }

    /// 编译期常量折叠后，从 arena 与侧表中清除不可达节点，仅保留从计划各根
    /// 可达的节点；后序编号保持不变。
    ///
    /// Prunes unreachable nodes from the arena and side tables after
    /// compile-time constant folding, keeping only nodes reachable from
    /// the plan's roots. Post-order numbering is preserved.
    pub fn prune(&mut self) {
        if self.nodes.is_empty() {
            return;
        }

        // 1. Mark reachable nodes from all roots.
        let mut reachable = vec![false; self.nodes.len()];
        let mut stack = Vec::new();
        if self.roots.is_empty() {
            stack.push(self.root);
        } else {
            for &id in self.roots.values() {
                stack.push(id);
            }
        }
        while let Some(curr) = stack.pop() {
            let idx = curr.index();
            if idx >= reachable.len() || reachable[idx] {
                continue;
            }
            reachable[idx] = true;
            match &self.nodes[idx] {
                PlanNode::Constant(_)
                | PlanNode::Coord(_)
                | PlanNode::ShiftA(_)
                | PlanNode::ShiftB(_)
                | PlanNode::Gradient { .. }
                | PlanNode::DistanceToPoint { .. }
                | PlanNode::OldBlended(_)
                | PlanNode::EndOuterIslands
                | PlanNode::Beardifier
                | PlanNode::BlendAlpha
                | PlanNode::BlendOffset => {}
                PlanNode::Noise { shift_x, shift_z, .. } => {
                    if let Some(sx) = shift_x {
                        stack.push(*sx);
                    }
                    if let Some(sz) = shift_z {
                        stack.push(*sz);
                    }
                }
                PlanNode::Binary(_, a, b) => {
                    stack.push(*a);
                    stack.push(*b);
                }
                PlanNode::Unary(_, input)
                | PlanNode::Clamp { input, .. }
                | PlanNode::Interpolated { input, .. }
                | PlanNode::Slice { input, .. }
                | PlanNode::BlendDensity(input) => {
                    stack.push(*input);
                }
                PlanNode::RangeChoice { input, in_range, out_of_range, .. } => {
                    stack.push(*input);
                    stack.push(*in_range);
                    stack.push(*out_of_range);
                }
                PlanNode::Lerp { alpha, first, second } => {
                    stack.push(*alpha);
                    stack.push(*first);
                    stack.push(*second);
                }
                PlanNode::IntervalSelect { input, functions, .. } => {
                    stack.push(*input);
                    let start = functions.0 as usize;
                    let len = functions.1 as usize;
                    for &fn_id in &self.function_ids[start..start + len] {
                        stack.push(fn_id);
                    }
                }
                PlanNode::FindTopSurface { density, upper_bound, .. } => {
                    stack.push(*density);
                    stack.push(*upper_bound);
                }
                PlanNode::Spline { coordinate, points } => {
                    stack.push(*coordinate);
                    let start = points.0 as usize;
                    let len = points.1 as usize;
                    for pt in &self.spline_points[start..start + len] {
                        stack.push(pt.value);
                    }
                }
            }
        }

        // 2. Build mapping for reachable nodes.
        // Scanning in ascending order guarantees that post-order (child < parent)
        // is preserved in the compacted arena.
        let mut mapping = vec![None; self.nodes.len()];
        let mut new_nodes = Vec::new();
        let mut new_thresholds = Vec::new();
        let mut new_function_ids = Vec::new();
        let mut new_spline_points = Vec::new();

        for (old_idx, &is_reachable) in reachable.iter().enumerate() {
            if !is_reachable {
                continue;
            }
            let new_id = NodeId(new_nodes.len() as u32);
            mapping[old_idx] = Some(new_id);
            new_nodes.push(self.nodes[old_idx].clone());
        }

        let remap = |id: NodeId| -> NodeId {
            mapping[id.index()].expect("child node must be marked reachable")
        };

        // 3. Remap children and compact side tables.
        for (old_idx, &is_reachable) in reachable.iter().enumerate() {
            if !is_reachable {
                continue;
            }
            let new_idx = mapping[old_idx].unwrap().index();
            let old_node = &self.nodes[old_idx];
            let remapped_node = match old_node {
                PlanNode::Constant(c) => PlanNode::Constant(*c),
                PlanNode::Coord(axis) => PlanNode::Coord(*axis),
                PlanNode::Noise { noise, xz_scale, y_scale, shift_x, shift_z } => {
                    PlanNode::Noise {
                        noise: *noise,
                        xz_scale: *xz_scale,
                        y_scale: *y_scale,
                        shift_x: shift_x.map(remap),
                        shift_z: shift_z.map(remap),
                    }
                }
                PlanNode::ShiftA(id) => PlanNode::ShiftA(*id),
                PlanNode::ShiftB(id) => PlanNode::ShiftB(*id),
                PlanNode::Binary(op, a, b) => PlanNode::Binary(*op, remap(*a), remap(*b)),
                PlanNode::Unary(op, input) => PlanNode::Unary(*op, remap(*input)),
                PlanNode::Clamp { input, min, max } => {
                    PlanNode::Clamp { input: remap(*input), min: *min, max: *max }
                }
                PlanNode::RangeChoice { input, min, max, in_range, out_of_range } => {
                    PlanNode::RangeChoice {
                        input: remap(*input),
                        min: *min,
                        max: *max,
                        in_range: remap(*in_range),
                        out_of_range: remap(*out_of_range),
                    }
                }
                PlanNode::Gradient { axis, from_coordinate, from_value, to_coordinate, to_value } => {
                    PlanNode::Gradient {
                        axis: *axis,
                        from_coordinate: *from_coordinate,
                        from_value: *from_value,
                        to_coordinate: *to_coordinate,
                        to_value: *to_value,
                    }
                }
                PlanNode::Lerp { alpha, first, second } => {
                    PlanNode::Lerp { alpha: remap(*alpha), first: remap(*first), second: remap(*second) }
                }
                PlanNode::Interpolated { cell_size_xz, cell_size_y, input } => {
                    PlanNode::Interpolated {
                        cell_size_xz: *cell_size_xz,
                        cell_size_y: *cell_size_y,
                        input: remap(*input),
                    }
                }
                PlanNode::Slice { axis, coordinate, input } => {
                    PlanNode::Slice {
                        axis: *axis,
                        coordinate: *coordinate,
                        input: remap(*input),
                    }
                }
                PlanNode::IntervalSelect { input, thresholds, functions } => {
                    let thr_start = new_thresholds.len() as u32;
                    let thr_len = thresholds.1;
                    let old_thr_start = thresholds.0 as usize;
                    let old_thr_len = thresholds.1 as usize;
                    new_thresholds.extend_from_slice(&self.thresholds[old_thr_start..old_thr_start + old_thr_len]);

                    let fn_start = new_function_ids.len() as u32;
                    let fn_len = functions.1;
                    let old_fn_start = functions.0 as usize;
                    let old_fn_len = functions.1 as usize;
                    for &fn_id in &self.function_ids[old_fn_start..old_fn_start + old_fn_len] {
                        new_function_ids.push(remap(fn_id));
                    }

                    PlanNode::IntervalSelect {
                        input: remap(*input),
                        thresholds: (thr_start, thr_len),
                        functions: (fn_start, fn_len),
                    }
                }
                PlanNode::DistanceToPoint { point } => PlanNode::DistanceToPoint { point: *point },
                PlanNode::FindTopSurface { cell_height, density, lower_bound, upper_bound } => {
                    PlanNode::FindTopSurface {
                        cell_height: *cell_height,
                        density: remap(*density),
                        lower_bound: *lower_bound,
                        upper_bound: remap(*upper_bound),
                    }
                }
                PlanNode::OldBlended(id) => PlanNode::OldBlended(*id),
                PlanNode::EndOuterIslands => PlanNode::EndOuterIslands,
                PlanNode::Beardifier => PlanNode::Beardifier,
                PlanNode::BlendAlpha => PlanNode::BlendAlpha,
                PlanNode::BlendOffset => PlanNode::BlendOffset,
                PlanNode::BlendDensity(input) => PlanNode::BlendDensity(remap(*input)),
                PlanNode::Spline { coordinate, points } => {
                    let pt_start = new_spline_points.len() as u32;
                    let pt_len = points.1;
                    let old_pt_start = points.0 as usize;
                    let old_pt_len = points.1 as usize;
                    for pt in &self.spline_points[old_pt_start..old_pt_start + old_pt_len] {
                        new_spline_points.push(SplinePoint {
                            location: pt.location,
                            value: remap(pt.value),
                            derivative: pt.derivative,
                        });
                    }

                    PlanNode::Spline {
                        coordinate: remap(*coordinate),
                        points: (pt_start, pt_len),
                    }
                }
            };
            new_nodes[new_idx] = remapped_node;
        }

        self.root = remap(self.root);
        for id in self.roots.values_mut() {
            *id = remap(*id);
        }
        self.nodes = new_nodes;
        self.thresholds = new_thresholds;
        self.function_ids = new_function_ids;
        self.spline_points = new_spline_points;
    }

    /// 主根节点。
    ///
    /// The primary root node.
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// 按名称查询根节点 id（多根编译时存在）。
    ///
    /// Looks up a named root node id if compiled with multi-root support.
    pub fn root_id(&self, name: &str) -> Option<NodeId> {
        self.roots.get(name).copied()
    }

    /// 计划内全部具名根。
    ///
    /// All named roots in this plan.
    pub fn roots(&self) -> &BTreeMap<String, NodeId> {
        &self.roots
    }

    /// arena 长度（等于去重/折叠后的节点数）。
    ///
    /// Arena length (equals emitted node count after dedup/folding).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// 噪声采样器名称表（下标即计划内的 `u32` 噪声 id）。
    ///
    /// Noise sampler names (index = the plan's `u32` noise ids).
    pub fn noises(&self) -> &[String] {
        &self.noises
    }

    /// `old_blended_noise` 参数形状表。
    ///
    /// `old_blended_noise` parameter shapes.
    pub fn blended_params(&self) -> &[OldBlendedParams] {
        &self.blended_params
    }

    /// 按 id 取节点。
    ///
    /// A node by id.
    pub fn node(&self, id: NodeId) -> &PlanNode {
        &self.nodes[id.index()]
    }

    /// arena 切片（子节点总在父节点之前）。
    ///
    /// The arena slice (children always precede parents).
    pub fn nodes(&self) -> &[PlanNode] {
        &self.nodes
    }

    /// 样条点表访问（interval-select / spline 区间）。
    ///
    /// Spline point table access (interval-select / spline ranges).
    pub fn spline_points(&self, range: (u32, u32)) -> &[SplinePoint] {
        &self.spline_points[range.0 as usize..(range.0 + range.1) as usize]
    }

    /// 阈值表访问。
    ///
    /// Threshold table access.
    pub fn thresholds(&self, range: (u32, u32)) -> &[f64] {
        &self.thresholds[range.0 as usize..(range.0 + range.1) as usize]
    }

    /// 函数 id 表访问。
    ///
    /// Function-id table access.
    pub fn function_ids(&self, range: (u32, u32)) -> &[NodeId] {
        &self.function_ids[range.0 as usize..(range.0 + range.1) as usize]
    }
}

/// 计划编译器：哈希交接发射 + 常量折叠。`memo` 记录结构到 id 的映射。
///
/// Plan compiler: hash-consed emission with constant folding. `memo` maps
/// structure to id.
struct Compiler {
    plan: DensityPlan,
    memo: HashMap<PlanNode, NodeId>,
}

impl Compiler {
    /// 取得（或登记）噪声名对应的采样器 id，同名去重。
    ///
    /// Returns (registering if new) the sampler id for a noise name;
    /// identical names dedupe.
    fn noise_id(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.plan.noise_index.get(name) {
            return id;
        }
        let id = self.plan.noises.len() as u32;
        self.plan.noises.push(name.to_string());
        self.plan.noise_index.insert(name.to_string(), id);
        id
    }

    /// 取得（或登记）`old_blended_noise` 参数形状对应的下标，相同形状去重。
    ///
    /// Returns (registering if new) the index for an `old_blended_noise`
    /// parameter shape; identical shapes dedupe.
    fn blended_id(&mut self, params: OldBlendedParams) -> u32 {
        if let Some(existing) = self
            .plan
            .blended_params
            .iter()
            .position(|p| *p == params)
        {
            return existing as u32;
        }
        let id = self.plan.blended_params.len() as u32;
        self.plan.blended_params.push(params);
        id
    }

    /// 哈希交接插入（后序：子节点已先发射）。
    ///
    /// Hash-consed insertion (post-order: children already emitted).
    fn intern(&mut self, node: PlanNode) -> NodeId {
        if let Some(&id) = self.memo.get(&node) {
            return id;
        }
        let id = NodeId(self.plan.nodes.len() as u32);
        self.plan.nodes.push(node.clone());
        self.memo.insert(node, id);
        id
    }

    /// 与 [`Self::intern`] 相同，但节点重复时回滚刚刚追加的侧表内容
    /// （区间指向的数据不保留）。
    ///
    /// Like [`Self::intern`], but rolls back the side tables when the node
    /// is a duplicate (ranges reference content that was just appended).
    fn intern_with_tables(
        &mut self,
        node: PlanNode,
        thresholds_start: usize,
        functions_start: usize,
        spline_points_start: usize,
    ) -> NodeId {
        if let Some(&id) = self.memo.get(&node) {
            self.plan.thresholds.truncate(thresholds_start);
            self.plan.function_ids.truncate(functions_start);
            self.plan.spline_points.truncate(spline_points_start);
            return id;
        }
        let id = NodeId(self.plan.nodes.len() as u32);
        self.plan.nodes.push(node.clone());
        self.memo.insert(node, id);
        id
    }

    /// 交接一个常量节点。
    ///
    /// Interns a constant node.
    fn constant(&mut self, value: f64) -> NodeId {
        self.intern(PlanNode::Constant(value))
    }

    /// 编译单个树节点：先递归子节点，再做与运行时求值一致的常量折叠，
    /// 最后交接。遇到未链接的引用会 panic（编译前必须先链接）。
    ///
    /// Compiles one tree node: recurses into children first, applies
    /// constant folding per the runtime semantics, then
    /// interns. Panics on an unlinked reference (link before compiling).
    fn compile(&mut self, node: &Node) -> NodeId {
        match node {
            // Constants are parsed as f32.
            Node::Constant(value) => self.constant(f64::from(*value as f32)),
            Node::Coord(axis) => self.intern(PlanNode::Coord(*axis)),
            Node::Ref(_) => {
                panic!("plan compilation reached an unlinked reference; link first")
            }
            Node::Noise { noise, xz_scale, y_scale, shift_x, shift_z } => {
                let shift_x = self.compile_opt(shift_x);
                let shift_z = self.compile_opt(shift_z);
                let noise_id = self.noise_id(noise);
                self.intern(PlanNode::Noise {
                    noise: noise_id,
                    xz_scale: *xz_scale,
                    y_scale: *y_scale,
                    shift_x,
                    shift_z,
                })
            }
            Node::ShiftA { noise } => {
                let id = self.noise_id(noise);
                self.intern(PlanNode::ShiftA(id))
            }
            Node::ShiftB { noise } => {
                let id = self.noise_id(noise);
                self.intern(PlanNode::ShiftB(id))
            }
            Node::Add(a, b) => self.compile_binary(BinaryOp::Add, a, b),
            Node::Sub(a, b) => self.compile_binary(BinaryOp::Sub, a, b),
            Node::Mul(a, b) => self.compile_binary(BinaryOp::Mul, a, b),
            Node::Div(a, b) => self.compile_binary(BinaryOp::Div, a, b),
            Node::Min(a, b) => self.compile_binary(BinaryOp::Min, a, b),
            Node::Max(a, b) => self.compile_binary(BinaryOp::Max, a, b),
            Node::Negate(a) => self.compile_unary(UnaryOp::Negate, a),
            Node::Abs(a) => self.compile_unary(UnaryOp::Abs, a),
            Node::Square(a) => self.compile_unary(UnaryOp::Square, a),
            Node::Cube(a) => self.compile_unary(UnaryOp::Cube, a),
            Node::HalfNegative(a) => self.compile_unary(UnaryOp::HalfNegative, a),
            Node::QuarterNegative(a) => self.compile_unary(UnaryOp::QuarterNegative, a),
            Node::Squeeze(a) => self.compile_unary(UnaryOp::Squeeze, a),
            Node::Cache(input) => {
                // Performance marker only: pass through to the input.
                self.compile(input)
            }
            Node::BlendDensity(input) => {
                let input_id = self.compile(input);
                if let Some(value) = self.plan_constant(input_id) {
                    self.constant(value)
                } else {
                    self.intern(PlanNode::BlendDensity(input_id))
                }
            }
            Node::Clamp { input, min, max } => {
                let input_id = self.compile(input);
                if let Some(value) = self.plan_constant(input_id) {
                    if let Some(folded) = fold_clamp(value, *min, *max) {
                        return self.constant(folded);
                    }
                }
                self.intern(PlanNode::Clamp { input: input_id, min: *min, max: *max })
            }
            Node::RangeChoice { input, min, max, in_range, out_of_range } => {
                let input_id = self.compile(input);
                let in_id = self.compile(in_range);
                let out_id = self.compile(out_of_range);
                if let Some(value) = self.plan_constant(input_id) {
                    let chosen = if value >= *min && value < *max {
                        self.plan_constant(in_id)
                    } else {
                        self.plan_constant(out_id)
                    };
                    if let Some(folded) = chosen {
                        return self.constant(folded);
                    }
                }
                self.intern(PlanNode::RangeChoice {
                    input: input_id,
                    min: *min,
                    max: *max,
                    in_range: in_id,
                    out_of_range: out_id,
                })
            }
            Node::Gradient { axis, from_coordinate, from_value, to_coordinate, to_value } => {
                self.intern(PlanNode::Gradient {
                    axis: *axis,
                    from_coordinate: *from_coordinate,
                    from_value: *from_value,
                    to_coordinate: *to_coordinate,
                    to_value: *to_value,
                })
            }
            Node::Lerp { alpha, first, second } => {
                let alpha_id = self.compile(alpha);
                let first_id = self.compile(first);
                let second_id = self.compile(second);
                if let (Some(alpha), Some(first), Some(second)) = (
                    self.plan_constant(alpha_id),
                    self.plan_constant(first_id),
                    self.plan_constant(second_id),
                ) {
                    if let Some(folded) = fold_lerp(alpha, first, second) {
                        return self.constant(folded);
                    }
                }
                self.intern(PlanNode::Lerp { alpha: alpha_id, first: first_id, second: second_id })
            }
            Node::Interpolated { cell_size_xz, cell_size_y, input } => {
                let input_id = self.compile(input);
                if let Some(value) = self.plan_constant(input_id) {
                    // A constant input interpolates to itself at every cell
                    // corner, so the trilinear blend is exact.
                    return self.constant(value);
                }
                self.intern(PlanNode::Interpolated {
                    cell_size_xz: *cell_size_xz as i32,
                    cell_size_y: *cell_size_y as i32,
                    input: input_id,
                })
            }
            Node::Slice { axis, coordinate, input } => {
                let input_id = self.compile(input);
                self.intern(PlanNode::Slice {
                    axis: *axis,
                    coordinate: *coordinate,
                    input: input_id,
                })
            }
            Node::IntervalSelect { input, thresholds, functions } => {
                let input_id = self.compile(input);
                // Compile children first (they may append to the side
                // tables themselves), then claim this node's ranges.
                let mut function_ids = Vec::with_capacity(functions.len());
                for function in functions {
                    function_ids.push(self.compile(function));
                }
                let thr_start = self.plan.thresholds.len();
                self.plan.thresholds.extend(thresholds.iter().copied());
                let threshold_start = thr_start as u32;
                let threshold_len = thresholds.len() as u32;
                let fn_start = self.plan.function_ids.len();
                self.plan.function_ids.extend(function_ids.iter().copied());
                let function_start = fn_start as u32;
                let function_len = function_ids.len() as u32;
                let node = PlanNode::IntervalSelect {
                    input: input_id,
                    thresholds: (threshold_start, threshold_len),
                    functions: (function_start, function_len),
                };
                self.intern_with_tables(node, thr_start, fn_start, self.plan.spline_points.len())
            }
            Node::DistanceToPoint { point } => {
                self.intern(PlanNode::DistanceToPoint { point: *point })
            }
            Node::Spline(spline) => self.compile_spline(spline),
            Node::FindTopSurface { cell_height, density, lower_bound, upper_bound } => {
                let density_id = self.compile(density);
                let upper_id = self.compile(upper_bound);
                self.intern(PlanNode::FindTopSurface {
                    cell_height: *cell_height as i32,
                    density: density_id,
                    lower_bound: *lower_bound as i32,
                    upper_bound: upper_id,
                })
            }
            Node::OldBlendedNoise { xz_scale, y_scale, xz_factor, y_factor, smear_scale_multiplier } => {
                let id = self.blended_id(OldBlendedParams {
                    xz_scale: *xz_scale,
                    y_scale: *y_scale,
                    xz_factor: *xz_factor,
                    y_factor: *y_factor,
                    smear_scale_multiplier: *smear_scale_multiplier,
                });
                self.intern(PlanNode::OldBlended(id))
            }
            Node::EndOuterIslands => self.intern(PlanNode::EndOuterIslands),
            Node::Beardifier => self.intern(PlanNode::Beardifier),
            Node::BlendAlpha => self.intern(PlanNode::BlendAlpha),
            Node::BlendOffset => self.intern(PlanNode::BlendOffset),
        }
    }

    /// [`Self::compile`] 的可选版本：`None` 保持为 `None`。
    ///
    /// Optional variant of [`Self::compile`]: `None` stays `None`.
    fn compile_opt(&mut self, node: &Option<Box<Node>>) -> Option<NodeId> {
        node.as_ref().map(|inner| self.compile(inner))
    }

    /// 编译二元运算；左操作数为常量 0 时按 mul 契约短路为 0，
    /// 不求值右操作数。双侧皆常量时折叠。
    ///
    /// Compiles a binary op; a constant-0 left operand short-circuits to 0
    /// per the mul contract (right operand not evaluated). Folds when
    /// both sides are constant.
    fn compile_binary(&mut self, op: BinaryOp, left: &Node, right: &Node) -> NodeId {
        let left_id = self.compile(left);
        // Mul contract: a zero left operand short-circuits without evaluating right.
        if op == BinaryOp::Mul && self.plan_constant(left_id) == Some(0.0) {
            return self.constant(0.0);
        }
        let right_id = self.compile(right);
        if let (Some(a), Some(b)) = (self.plan_constant(left_id), self.plan_constant(right_id)) {
            if let Some(folded) = fold_binary(op, a, b) {
                return self.constant(folded);
            }
        }
        self.intern(PlanNode::Binary(op, left_id, right_id))
    }

    /// 编译一元运算；输入为常量时折叠。
    ///
    /// Compiles a unary op; folds when the input is constant.
    fn compile_unary(&mut self, op: UnaryOp, input: &Node) -> NodeId {
        let input_id = self.compile(input);
        if let Some(value) = self.plan_constant(input_id) {
            if let Some(folded) = fold_unary(op, value) {
                return self.constant(folded);
            }
        }
        self.intern(PlanNode::Unary(op, input_id))
    }

    /// 编译样条：常量样条折叠为常量；多点样条先编译坐标与各点取值，
    /// 再整体领取该样条的点区间。
    ///
    /// Compiles a spline: a constant spline folds to a constant; a
    /// multi-point spline compiles the coordinate and point values first,
    /// then claims its point range as a whole.
    fn compile_spline(&mut self, spline: &Spline) -> NodeId {
        match spline {
            Spline::Constant(value) => self.constant(*value),
            Spline::Multi { coordinate, points } => {
                let coordinate_id = self.compile(coordinate);
                // Point values may nest interval/spline tables: compile
                // everything first, then claim this spline's point range.
                let mut table = Vec::with_capacity(points.len());
                for (location, value, derivative) in points {
                    table.push(SplinePoint {
                        location: *location,
                        value: self.compile_spline(value),
                        derivative: *derivative,
                    });
                }
                let pts_start = self.plan.spline_points.len();
                self.plan.spline_points.extend(table.iter().copied());
                let start = pts_start as u32;
                let len = table.len() as u32;
                let node = PlanNode::Spline {
                    coordinate: coordinate_id,
                    points: (start, len),
                };
                self.intern_with_tables(node, self.plan.thresholds.len(), self.plan.function_ids.len(), pts_start)
            }
        }
    }

    /// 若 id 指向常量节点则返回其值，否则 `None`。
    ///
    /// The value when the id points at a constant node, else `None`.
    fn plan_constant(&self, id: NodeId) -> Option<f64> {
        match self.plan.node(id) {
            PlanNode::Constant(value) => Some(*value),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Folding arithmetic: same operation order and short-circuits as evaluation.
// ---------------------------------------------------------------------------

/// 折叠二元运算；结果为非有限值时返回 `None`，该节点保持不折叠。
///
/// Folds a binary op; returns `None` for non-finite results, leaving the node
/// unfolded.
fn fold_binary(op: BinaryOp, a: f64, b: f64) -> Option<f64> {
    let a = a as f32;
    let b = b as f32;
    let value = match op {
        BinaryOp::Add => a + b,
        BinaryOp::Sub => a - b,
        // Mul contract: mul short-circuits a zero left operand.
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
                return None;
            }
            value
        }
        BinaryOp::Min => a.min(b),
        BinaryOp::Max => a.max(b),
    };
    value.is_finite().then(|| f64::from(value))
}

fn fold_unary(op: UnaryOp, value: f64) -> Option<f64> {
    let value = value as f32;
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
            let clamped = crate::eval::mth_clamp(value, -1.0, 1.0);
            clamped / 2.0 - clamped * clamped * clamped / 24.0
        }
    };
    folded.is_finite().then(|| f64::from(folded))
}

fn fold_clamp(value: f64, min: f64, max: f64) -> Option<f64> {
    let folded = crate::eval::mth_clamp(value as f32, min as f32, max as f32);
    folded.is_finite().then(|| f64::from(folded))
}

fn fold_lerp(alpha: f64, first: f64, second: f64) -> Option<f64> {
    let alpha = alpha as f32;
    let first = first as f32;
    let second = second as f32;
    // Lerp short-circuits at alpha 0.0 / 1.0 (1-ulp observable).
    let folded = if alpha == 0.0 {
        first
    } else if alpha == 1.0 {
        second
    } else {
        crate::eval::mth_lerp(alpha, first, second)
    };
    folded.is_finite().then(|| f64::from(folded))
}
