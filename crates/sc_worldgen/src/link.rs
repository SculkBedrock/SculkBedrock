//! 密度函数引用链接：把 [`Node::Ref`] 叶子解析为闭合的类型化树。
//!
//! 链接方式是把被引用文件解析出的节点内联到引用处，同时在文件图上做环检测，
//! 并收集树中采样到的全部 noise 参数。内联会复制共享子图；
//! 编译阶段再把链接后的树重新交接到共享的 [`NodeId`] 上。
//!
//! Reference linking: resolves [`Node::Ref`] leaves into closed typed trees.
//!
//! Linking inlines the referenced file's parsed node at the reference site,
//! detects cycles over the file graph and collects `noise` parameters sampled
//! by the tree. Inlining duplicates shared subgraphs; the plan compiler
//! re-interns the linked tree into shared [`crate::plan::NodeId`]s.

use std::collections::{BTreeMap, BTreeSet};

use crate::data::json::{parse_strict, Json};
use crate::data::node::{parse_node, Node, NodeBudget, ParseError};
use crate::sources::{WorldgenCategory, WorldgenSources};

/// 单个 `noise` 资源经校验后的参数——26.3 数据形状：`base_octave` 必填，
/// `base_amplitude`（默认 1.0）、`octave_count`（默认 1）、
/// `amplitude_modifiers`（存在时长度须与 `octave_count` 一致）、
/// `normalize`（布尔或 `"legacy"`，默认启用）可选。
///
/// Validated parameters of one `noise` resource — the 26.3 data shape:
/// required `base_octave` with optional `base_amplitude` (default 1.0),
/// `octave_count` (default 1),
/// `amplitude_modifiers` (must match `octave_count` when present) and
/// `normalize` (bool | "legacy", default ENABLED).
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseParams {
    pub base_octave: i32,
    pub base_amplitude: f64,
    pub octave_count: Option<i32>,
    pub amplitude_modifiers: Option<Vec<f64>>,
    pub normalize: Option<crate::noise::Normalize26>,
}

/// 链接后的根：闭合的节点树，以及树采样到的全部 noise 参数。
/// noise 键为 `namespace:path` 形式的资源 id。
///
/// A linked root: the closed node tree plus every noise parameter the tree
/// samples. Noise keys are `namespace:path` resource ids.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkedRoot {
    pub root: Node,
    pub noises: BTreeMap<String, NoiseParams>,
}

/// 链接失败。所有失败都是显式错误，不存在静默回退。
///
/// Link failures. All are loud: nothing falls back to defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// 引用（或根本身）无法解析到 density_function 文件。
    ///
    /// A reference (or the root) does not resolve to a density_function file.
    MissingResource { reference: String, from: String },
    /// 引用指向了 density_function 之外的类别（noise_settings / noise 文件
    /// 在此不是链接目标）。
    ///
    /// A reference points outside the density_function category (noise_settings
    /// and noise files are not link targets here).
    WrongCategory { reference: String, category: &'static str },
    /// 文件引用图上存在环；错误携带完整链路用于诊断。
    ///
    /// The file reference graph contains a cycle; the chain is included.
    Cycle { chain: Vec<String> },
    /// 闭包内任一文件的节点/树/schema 解析失败。
    ///
    /// A node/tree/schema failure while parsing any file in the closure.
    Parse(ParseError),
    /// 链接后的树超出了节点预算。
    ///
    /// The linked tree exceeded its node budget.
    Budget { path: String },
    /// noise 参数文件缺失或格式错误。
    ///
    /// A noise parameter file is missing or malformed.
    BadNoise { noise: String, detail: String },
    /// noise_settings 资源未通过结构校验。
    ///
    /// The noise_settings resource failed structural validation.
    Settings(crate::router::SettingsError),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingResource { reference, from } => write!(
                f,
                "unresolved density function reference '{reference}' (from {from})"
            ),
            Self::WrongCategory { reference, category } => write!(
                f,
                "reference '{reference}' resolves to a {category} file, not a density function"
            ),
            Self::Cycle { chain } => {
                write!(f, "density function reference cycle: ")?;
                for (i, key) in chain.iter().enumerate() {
                    if i > 0 {
                        write!(f, " -> ")?;
                    }
                    write!(f, "{key}")?;
                }
                Ok(())
            }
            Self::Parse(error) => write!(f, "{error}"),
            Self::Budget { path } => write!(f, "linked tree node budget exceeded at {path}"),
            Self::BadNoise { noise, detail } => write!(f, "bad noise params for {noise}: {detail}"),
            Self::Settings(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for LinkError {}

impl From<ParseError> for LinkError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

/// 引用链最大深度（文件级嵌套）；超出上限按环报告。
///
/// Maximum reference-chain depth (file-level nesting); exceeding the bound is
/// reported as a cycle.
const MAX_LINK_DEPTH: usize = 128;

/// 链接后树的最大节点总数。
///
/// Maximum total nodes across the linked tree.
const MAX_LINKED_NODES: usize = 262_144;

/// 把一个 density function 资源链接为闭合的树。
///
/// Links a density function resource into a closed tree.
///
/// - `sources`: 世界生成 bundle 源
/// - `resource`: 目标资源 id（`namespace:path`）
///
/// 成功返回链接根（含收集到的 noise 参数）；失败返回 [`LinkError`]。
///
/// - `sources`: the worldgen bundle sources
/// - `resource`: the target resource id (`namespace:path`)
///
/// Returns the linked root with its collected noise parameters, or a [`LinkError`].
pub fn link_root(sources: &WorldgenSources, resource: &str) -> Result<LinkedRoot, LinkError> {
    let mut linker = Linker {
        sources,
        noises: BTreeMap::new(),
        stack: BTreeSet::new(),
        budget: NodeBudget::new(MAX_LINKED_NODES),
    };
    let root = linker.link(resource, resource, 0)?;
    Ok(LinkedRoot { root, noises: linker.noises })
}

/// 完全链接的 `noise_settings`：每个 router 根均已解析，附收集到的 noise
/// 参数与冻结的非密度块。
///
/// A fully linked `noise_settings`: every router root resolved, with the
/// collected noise parameters and the frozen non-density blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkedNoiseSettings {
    /// `noise.min_y`（世界底部）。
    ///
    /// `noise.min_y` (world bottom).
    pub min_y: i64,
    /// `noise.height`（世界高度，单位方块）。
    ///
    /// `noise.height` (world height in blocks).
    pub height: i64,
    /// `legacy_random_source`（26.3 `noise_settings` 标志；nether/end 为 true，
    /// 决定噪声种子派生用 LCG 还是 Xoroshiro 位置工厂）。
    ///
    /// The `legacy_random_source` settings flag (true for nether/end); it
    /// selects the LCG positional factory over the Xoroshiro one.
    pub legacy_random_source: bool,
    /// 链接后的 router 根，键为 router 字段名。
    ///
    /// Linked router roots keyed by router field name.
    pub router: BTreeMap<String, Node>,
    /// 树采样到的 noise 参数，键为 noise 资源 id。
    ///
    /// Noise parameters sampled by the tree, keyed by noise resource id.
    pub noises: BTreeMap<String, NoiseParams>,
    /// 非密度块（material_rule 等）的原始 JSON，按字节冻结供后续阶段使用。
    ///
    /// Raw JSON of non-density blocks (material_rule etc.), frozen as bytes
    /// for later stages.
    pub frozen: BTreeMap<String, crate::data::json::Json>,
}

/// 链接 `noise_settings` 资源的全部 router 根。
///
/// Links every router root of a `noise_settings` resource.
///
/// - `sources`: 世界生成 bundle 源
/// - `resource`: noise_settings 资源 id
///
/// - `sources`: the worldgen bundle sources
/// - `resource`: the noise_settings resource id
pub fn link_noise_settings(
    sources: &WorldgenSources,
    resource: &str,
) -> Result<LinkedNoiseSettings, LinkError> {
    let settings = crate::router::load_noise_settings(sources, resource)
        .map_err(LinkError::Settings)?;
    let mut linker = Linker {
        sources,
        noises: BTreeMap::new(),
        stack: BTreeSet::new(),
        budget: NodeBudget::new(MAX_LINKED_NODES),
    };
    let mut router = BTreeMap::new();
    for (name, node) in settings.router {
        let linked = linker.resolve_node(node, resource, 0)?;
        router.insert(name, linked);
    }
    let legacy_random_source = settings
        .frozen
        .get("legacy_random_source")
        .and_then(|value| match value {
            crate::data::Json::Bool(flag) => Some(*flag),
            _ => None,
        })
        .unwrap_or(false);
    Ok(LinkedNoiseSettings {
        min_y: settings.min_y,
        height: settings.height,
        legacy_random_source,
        router,
        noises: linker.noises,
        frozen: settings.frozen,
    })
}

/// 链接器状态：bundle 源、已收集的 noise 参数、当前引用栈（环检测）与节点预算。
///
/// Linker state: bundle sources, collected noise parameters, the current
/// reference stack (cycle detection) and the node budget.
struct Linker<'a> {
    sources: &'a WorldgenSources,
    noises: BTreeMap<String, NoiseParams>,
    stack: BTreeSet<String>,
    budget: NodeBudget,
}

impl<'a> Linker<'a> {
    /// 解析并内联单个资源。`from` 为引用方文件名，仅用于诊断信息
    /// （根调用时传空串）；`depth` 为当前引用链深度。
    ///
    /// Parses and inlines one resource. `from` names the referencing file
    /// for diagnostics ("" for the root call); `depth` is the current
    /// reference-chain depth.
    fn link(&mut self, resource: &str, from: &str, depth: usize) -> Result<Node, LinkError> {
        if depth > MAX_LINK_DEPTH {
            return Err(LinkError::Cycle { chain: self.stack.iter().cloned().collect() });
        }
        let key = resource.replace(':', "/");
        if self.stack.contains(&key) {
            let mut chain: Vec<String> = self.stack.iter().cloned().collect();
            chain.push(key);
            return Err(LinkError::Cycle { chain });
        }
        let bytes = self
            .sources
            .category(WorldgenCategory::DensityFunction)
            .get(&key)
            .ok_or_else(|| {
                // Distinguish a wrong-category hit from a missing file.
                for category in [WorldgenCategory::Noise, WorldgenCategory::NoiseSettings] {
                    if self
                        .sources
                        .resolve(category, resource)
                        .is_some()
                    {
                        return LinkError::WrongCategory {
                            reference: resource.to_string(),
                            category: category.name(),
                        };
                    }
                }
                LinkError::MissingResource {
                    reference: resource.to_string(),
                    from: from.to_string(),
                }
            })?
            .clone();

        self.stack.insert(key.clone());
        let json: Json =
            parse_strict(&bytes).map_err(ParseError::from).map_err(LinkError::Parse)?;
        let node = parse_node(&json, &format!("$[{key}]"), &mut self.budget)?;
        let linked = self.resolve_node(node, resource, depth)?;
        self.stack.remove(&key);
        Ok(linked)
    }

    /// 递归解析单个节点：引用叶子内联为目标树，容器节点递归子节点，
    /// noise/shift 叶子注册其参数。`from`/`depth` 含义同 [`Self::link`]。
    ///
    /// Recursively resolves one node: reference leaves inline the target
    /// tree, container nodes recurse into children, and noise/shift leaves
    /// register their parameters. `from`/`depth` as in [`Self::link`].
    fn resolve_node(
        &mut self,
        node: Node,
        from: &str,
        depth: usize,
    ) -> Result<Node, LinkError> {
        if let Err(error) = self.budget_charge() {
            return Err(error);
        }
        Ok(match node {
            Node::Ref(reference) => match reference.as_str() {
                "minecraft:x" => Node::Coord(crate::data::node::Axis::X),
                "minecraft:y" => Node::Coord(crate::data::node::Axis::Y),
                "minecraft:z" => Node::Coord(crate::data::node::Axis::Z),
                "minecraft:zero" => Node::Constant(0.0),
                _ => self.link(&reference, from, depth + 1)?,
            },
            Node::Noise { noise, xz_scale, y_scale, shift_x, shift_z } => {
                self.register_noise(&noise)?;
                Node::Noise {
                    noise,
                    xz_scale,
                    y_scale,
                    shift_x: shift_x.map(|n| self.resolve_box(*n, from, depth)).transpose()?,
                    shift_z: shift_z.map(|n| self.resolve_box(*n, from, depth)).transpose()?,
                }
            }
            Node::ShiftA { noise } => {
                self.register_noise(&noise)?;
                Node::ShiftA { noise }
            }
            Node::ShiftB { noise } => {
                self.register_noise(&noise)?;
                Node::ShiftB { noise }
            }
            Node::Add(a, b) => Node::Add(self.resolve_box(*a, from, depth)?, self.resolve_box(*b, from, depth)?),
            Node::Sub(a, b) => Node::Sub(self.resolve_box(*a, from, depth)?, self.resolve_box(*b, from, depth)?),
            Node::Mul(a, b) => Node::Mul(self.resolve_box(*a, from, depth)?, self.resolve_box(*b, from, depth)?),
            Node::Div(a, b) => Node::Div(self.resolve_box(*a, from, depth)?, self.resolve_box(*b, from, depth)?),
            Node::Min(a, b) => Node::Min(self.resolve_box(*a, from, depth)?, self.resolve_box(*b, from, depth)?),
            Node::Max(a, b) => Node::Max(self.resolve_box(*a, from, depth)?, self.resolve_box(*b, from, depth)?),
            Node::Negate(a) => Node::Negate(self.resolve_box(*a, from, depth)?),
            Node::Abs(a) => Node::Abs(self.resolve_box(*a, from, depth)?),
            Node::Square(a) => Node::Square(self.resolve_box(*a, from, depth)?),
            Node::Cube(a) => Node::Cube(self.resolve_box(*a, from, depth)?),
            Node::HalfNegative(a) => Node::HalfNegative(self.resolve_box(*a, from, depth)?),
            Node::QuarterNegative(a) => Node::QuarterNegative(self.resolve_box(*a, from, depth)?),
            Node::Squeeze(a) => Node::Squeeze(self.resolve_box(*a, from, depth)?),
            Node::Clamp { input, min, max } => Node::Clamp {
                input: self.resolve_box(*input, from, depth)?,
                min,
                max,
            },
            Node::RangeChoice { input, min, max, in_range, out_of_range } => Node::RangeChoice {
                input: self.resolve_box(*input, from, depth)?,
                min,
                max,
                in_range: self.resolve_box(*in_range, from, depth)?,
                out_of_range: self.resolve_box(*out_of_range, from, depth)?,
            },
            Node::Gradient { .. } | Node::DistanceToPoint { .. } => node,
            Node::Lerp { alpha, first, second } => Node::Lerp {
                alpha: self.resolve_box(*alpha, from, depth)?,
                first: self.resolve_box(*first, from, depth)?,
                second: self.resolve_box(*second, from, depth)?,
            },
            Node::Cache(input) => Node::Cache(self.resolve_box(*input, from, depth)?),
            Node::Interpolated { cell_size_xz, cell_size_y, input } => Node::Interpolated {
                cell_size_xz,
                cell_size_y,
                input: self.resolve_box(*input, from, depth)?,
            },
            Node::Slice { axis, coordinate, input } => Node::Slice {
                axis,
                coordinate,
                input: self.resolve_box(*input, from, depth)?,
            },
            Node::IntervalSelect { input, thresholds, functions } => {
                let mut linked_functions = Vec::with_capacity(functions.len());
                for function in functions {
                    linked_functions.push(self.resolve_node(function, from, depth)?);
                }
                Node::IntervalSelect {
                    input: self.resolve_box(*input, from, depth)?,
                    thresholds,
                    functions: linked_functions,
                }
            }
            Node::Spline(spline) => Node::Spline(self.resolve_spline(spline, from, depth)?),
            Node::FindTopSurface { cell_height, density, lower_bound, upper_bound } => {
                Node::FindTopSurface {
                    cell_height,
                    density: self.resolve_box(*density, from, depth)?,
                    lower_bound,
                    upper_bound: self.resolve_box(*upper_bound, from, depth)?,
                }
            }
            Node::BlendDensity(input) => {
                Node::BlendDensity(self.resolve_box(*input, from, depth)?)
            }
            Node::Constant(_) | Node::Coord(_) | Node::OldBlendedNoise { .. }
            | Node::EndOuterIslands | Node::Beardifier | Node::BlendAlpha
            | Node::BlendOffset => node,
        })
    }

    /// [`Self::resolve_node`] 的装箱版本，供需要 `Box<Node>` 的字段使用。
    ///
    /// Boxed variant of [`Self::resolve_node`] for fields requiring `Box<Node>`.
    fn resolve_box(
        &mut self,
        node: Node,
        from: &str,
        depth: usize,
    ) -> Result<Box<Node>, LinkError> {
        self.resolve_node(node, from, depth).map(Box::new)
    }

    /// 递归解析样条：多点样条的坐标与各点取值同样是密度节点。
    ///
    /// Recursively resolves a spline: a multi-point spline's coordinate and
    /// per-point values are density nodes too.
    fn resolve_spline(&mut self, spline: crate::data::node::Spline, from: &str, depth: usize) -> Result<crate::data::node::Spline, LinkError> {
        use crate::data::node::Spline;
        Ok(match spline {
            Spline::Constant(value) => Spline::Constant(value),
            Spline::Multi { coordinate, points } => {
                let mut linked = Vec::with_capacity(points.len());
                for (location, value, derivative) in points {
                    linked.push((
                        location,
                        self.resolve_spline(value, from, depth)?,
                        derivative,
                    ));
                }
                Spline::Multi {
                    coordinate: self.resolve_box(*coordinate, from, depth)?,
                    points: linked,
                }
            }
        })
    }

    /// 读取并校验一个 noise 参数文件并登记（已登记过则直接返回）。
    ///
    /// Reads, validates and registers one noise parameter file (no-op when
    /// already registered).
    fn register_noise(&mut self, noise: &str) -> Result<(), LinkError> {
        if self.noises.contains_key(noise) {
            return Ok(());
        }
        let bad = |detail: &str| LinkError::BadNoise {
            noise: noise.to_string(),
            detail: detail.to_string(),
        };
        let bytes = self
            .sources
            .resolve(WorldgenCategory::Noise, noise)
            .ok_or_else(|| bad("noise resource missing from the bundle"))?
            .clone();
        let json: Json =
            parse_strict(&bytes).map_err(|error| bad(&error.to_string()))?;
        let allowed = [
            "base_octave",
            "base_amplitude",
            "octave_count",
            "amplitude_modifiers",
            "normalize",
        ];
        if let Json::Obj(fields) = &json {
            for (key, _) in fields {
                if !allowed.contains(&key.as_str()) {
                    return Err(bad(&format!("unknown noise field '{key}'")));
                }
            }
        }
        let base_octave = json
            .get("base_octave")
            .and_then(Json::as_i64)
            .ok_or_else(|| bad("missing or non-integer base_octave"))?;
        let base_amplitude = json
            .get("base_amplitude")
            .and_then(Json::as_f64)
            .unwrap_or(1.0);
        if !base_amplitude.is_finite() {
            return Err(bad("non-finite base_amplitude"));
        }
        let octave_count = match json.get("octave_count") {
            None => None,
            Some(value) => Some(value.as_i64().ok_or_else(|| {
                bad("non-integer octave_count")
            })? as i32),
        };
        let amplitude_modifiers = match json.get("amplitude_modifiers") {
            None => None,
            Some(Json::Arr(items)) => {
                let mut modifiers = Vec::with_capacity(items.len());
                for item in items {
                    let value = item
                        .as_f64()
                        .ok_or_else(|| bad("non-numeric amplitude modifier"))?;
                    if !value.is_finite() {
                        return Err(bad("non-finite amplitude modifier"));
                    }
                    modifiers.push(value);
                }
                if modifiers.is_empty() {
                    return Err(bad("amplitude_modifiers must not be empty"));
                }
                Some(modifiers)
            }
            Some(_) => return Err(bad("amplitude_modifiers must be an array")),
        };
        // Validation: non-empty modifiers must match octave_count.
        if let (Some(modifiers), Some(count)) = (&amplitude_modifiers, octave_count) {
            if modifiers.len() as i32 != count {
                return Err(bad(&format!(
                    "amplitude_modifiers has size {} but octave_count is {count}",
                    modifiers.len()
                )));
            }
        }
        let normalize = match json.get("normalize") {
            None => None,
            Some(Json::Bool(true)) => Some(crate::noise::Normalize26::Enabled),
            Some(Json::Bool(false)) => Some(crate::noise::Normalize26::Disabled),
            Some(Json::Str(value)) if value == "legacy" => Some(crate::noise::Normalize26::Legacy),
            Some(other) => {
                return Err(bad(&format!("invalid normalize value: {other:?}")))
            }
        };
        self.noises.insert(noise.to_string(), NoiseParams {
            base_octave: base_octave as i32,
            base_amplitude,
            octave_count,
            amplitude_modifiers,
            normalize,
        });
        Ok(())
    }

    /// 从共享预算中扣除一个节点；超限时映射为 [`LinkError::Budget`]。
    ///
    /// Charges one node against the shared budget, mapping an overrun to
    /// [`LinkError::Budget`].
    fn budget_charge(&mut self) -> Result<(), LinkError> {
        // Charges a copy of the shared budget so an overrun can be reported
        // as LinkError::Budget instead of a parse error.
        let mut probe = self.budget;
        let path = "$[linked]";
        match probe.charge(path) {
            Ok(()) => {
                self.budget = probe;
                Ok(())
            }
            Err(_) => Err(LinkError::Budget { path: path.to_string() }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::node::Axis;

    fn sources_with(density: &[(&str, &str)], noises: &[(&str, &str)]) -> WorldgenSources {
        WorldgenSources {
            density_functions: density
                .iter()
                .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
                .collect(),
            noises: noises
                .iter()
                .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
                .collect(),
            noise_settings: BTreeMap::new(),
        }
    }

    #[test]
    fn links_references_and_collects_noise_params() {
        let sources = sources_with(
            &[
                ("minecraft/root", r#"{"type":"minecraft:add","left":"minecraft:leaf","right":{"type":"minecraft:noise","noise":"minecraft:temperature","xz_scale":1.0,"y_scale":1.0}}"#),
                ("minecraft/leaf", r#"{"type":"minecraft:square","input":2}"#),
            ],
            &[("minecraft/temperature", r#"{"base_octave":-4,"base_amplitude":0.875,"octave_count":3,"amplitude_modifiers":[1.0,0.5,2.0]}"#)],
        );
        let linked = link_root(&sources, "minecraft:root").unwrap();
        assert_eq!(
            linked.root,
            Node::Add(
                Box::new(Node::Square(Box::new(Node::Constant(2.0)))),
                Box::new(Node::Noise {
                    noise: "minecraft:temperature".to_string(),
                    xz_scale: 1.0,
                    y_scale: 1.0,
                    shift_x: None,
                    shift_z: None,
                })
            )
        );
        let params = linked.noises.get("minecraft:temperature").unwrap();
        assert_eq!(params.base_octave, -4);
        assert_eq!(params.base_amplitude, 0.875);
        assert_eq!(params.octave_count, Some(3));
        assert_eq!(params.amplitude_modifiers.as_deref(), Some(&[1.0, 0.5, 2.0][..]));
    }

    #[test]
    fn builtin_coordinate_refs_become_leaves() {
        let sources = sources_with(
            &[("minecraft/root", r#"{"type":"minecraft:abs","input":"minecraft:y"}"#)],
            &[],
        );
        let linked = link_root(&sources, "minecraft:root").unwrap();
        assert_eq!(linked.root, Node::Abs(Box::new(Node::Coord(Axis::Y))));
    }

    #[test]
    fn missing_and_wrong_category_references_are_loud() {
        let sources = sources_with(
            &[("minecraft/root", r#"{"type":"minecraft:abs","input":"minecraft:ghost"}"#)],
            &[],
        );
        match link_root(&sources, "minecraft:root") {
            Err(LinkError::MissingResource { reference, .. }) => {
                assert_eq!(reference, "minecraft:ghost");
            }
            other => panic!("expected MissingResource, got {other:?}"),
        }
        let sources = sources_with(
            &[("minecraft/root", r#"{"type":"minecraft:abs","input":"minecraft:overworld"}"#)],
            &[],
        )
        .with_noise_settings_root();
        match link_root(&sources, "minecraft:root") {
            Err(LinkError::WrongCategory { category, .. }) => {
                assert_eq!(category, "noise_settings");
            }
            other => panic!("expected WrongCategory, got {other:?}"),
        }
    }

    #[test]
    fn reference_cycles_are_detected() {
        let sources = sources_with(
            &[
                ("minecraft/a", r#"{"type":"minecraft:abs","input":"minecraft:b"}"#),
                ("minecraft/b", r#"{"type":"minecraft:abs","input":"minecraft:a"}"#),
            ],
            &[],
        );
        match link_root(&sources, "minecraft:a") {
            Err(LinkError::Cycle { chain }) => {
                assert!(chain.len() >= 2);
                assert_eq!(chain.first(), chain.last());
            }
            other => panic!("expected Cycle, got {other:?}"),
        }
    }

    #[test]
    fn bad_noise_params_fail_the_link() {
        // Unknown noise fields are loud (26.3 schema is closed).
        let sources = sources_with(
            &[("minecraft/root", r#"{"type":"minecraft:noise","noise":"minecraft:t","xz_scale":1.0,"y_scale":1.0}"#)],
            &[("minecraft/t", r#"{"base_octave":0,"base_amplitude":1.0,"legacy_amplitudes":[1.0]}"#)],
        );
        match link_root(&sources, "minecraft:root") {
            Err(LinkError::BadNoise { noise, detail }) => {
                assert_eq!(noise, "minecraft:t");
                assert!(detail.contains("unknown noise field"), "got: {detail}");
            }
            other => panic!("expected BadNoise, got {other:?}"),
        }
        // Non-finite base_amplitude is loud.
        let sources = sources_with(
            &[("minecraft/root", r#"{"type":"minecraft:noise","noise":"minecraft:t","xz_scale":1.0,"y_scale":1.0}"#)],
            &[("minecraft/t", r#"{"base_octave":0,"base_amplitude":1e400}"#)],
        );
        assert!(matches!(
            link_root(&sources, "minecraft:root"),
            Err(LinkError::BadNoise { .. })
        ));
    }

    impl WorldgenSources {
        fn with_noise_settings_root(mut self) -> Self {
            self.noise_settings
                .insert("minecraft/overworld".to_string(), b"{}".to_vec());
            self
        }
    }
}
