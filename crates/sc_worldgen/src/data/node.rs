//! 固定 26.3 数据模型的类型化密度函数节点。
//!
//! 节点词汇表就是 26.3 使用的集合：`cache` 是唯一的缓存节点，`gradient`
//! 是绝对 Y 梯度节点，样条是显式的 `minecraft:spline` 节点，`noise` 带有
//! 逐轴的 `shift_x`/`shift_z` 密度函数，`interpolated` 接受显式单元格尺寸。
//! 未知节点类型、未知字段、缺失字段与非法取值都会返回带 JSON 路径的错误，
//! 没有任何东西会静默默认为 0。
//!
//! Typed density-function nodes for the fixed 26.3 data model.
//!
//! The vocabulary is the 26.3 set: `cache` is the only cache node, `gradient`
//! is the absolute-Y gradient, splines are explicit `minecraft:spline` nodes,
//! `noise` carries per-axis `shift_x`/`shift_z` density functions, and
//! `interpolated` takes explicit cell sizes. Unknown node types, unknown
//! fields, missing fields and bad values all return errors carrying the JSON
//! path; nothing silently defaults to zero.

use crate::data::json::{Json, JsonError};

/// 坐标空间的一个轴。
///
/// One axis of the coordinate space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Node {
    /// 本节点的直接子节点（叶子/参数节点为空）。闭包分析、计划编译与
    /// 资源发现都通过它遍历节点结构。
    ///
    /// Direct sub-nodes of this node (empty for leaves/parameters). Closure
    /// analysis, plan compilation and resource discovery all walk the node
    /// structure through it.
    pub fn children(&self) -> Vec<&Node> {
        match self {
            Node::Constant(_) | Node::Coord(_) | Node::Ref(_) | Node::Gradient { .. }
            | Node::DistanceToPoint { .. } | Node::OldBlendedNoise { .. }
            | Node::EndOuterIslands | Node::Beardifier | Node::BlendAlpha
            | Node::BlendOffset | Node::ShiftA { .. } | Node::ShiftB { .. } => Vec::new(),
            Node::Noise { shift_x, shift_z, .. } => {
                let mut children = Vec::new();
                children.extend(shift_x.iter().map(|node| node.as_ref()));
                children.extend(shift_z.iter().map(|node| node.as_ref()));
                children
            }
            Node::Add(a, b) | Node::Sub(a, b) | Node::Mul(a, b) | Node::Div(a, b)
            | Node::Min(a, b) | Node::Max(a, b) => vec![a.as_ref(), b.as_ref()],
            Node::Negate(a) | Node::Abs(a) | Node::Square(a) | Node::Cube(a)
            | Node::HalfNegative(a) | Node::QuarterNegative(a) | Node::Squeeze(a)
            | Node::Cache(a) | Node::BlendDensity(a) | Node::Interpolated { input: a, .. }
            | Node::Slice { input: a, .. } => vec![a.as_ref()],
            Node::Clamp { input, .. } => vec![input.as_ref()],
            Node::RangeChoice { input, in_range, out_of_range, .. } => {
                vec![input.as_ref(), in_range.as_ref(), out_of_range.as_ref()]
            }
            Node::Lerp { alpha, first, second } => {
                vec![alpha.as_ref(), first.as_ref(), second.as_ref()]
            }
            Node::IntervalSelect { input, functions, .. } => {
                let mut children = vec![input.as_ref()];
                children.extend(functions);
                children
            }
            Node::Spline(spline) => spline.children(),
            Node::FindTopSurface { density, upper_bound, .. } => {
                vec![density.as_ref(), upper_bound.as_ref()]
            }
        }
    }

    /// 先序访问本节点及每个传递子节点，包括嵌套在样条值内部的节点。
    ///
    /// Visits this node and every transitive sub-node (pre-order), including
    /// nodes nested inside spline values.
    pub fn walk(&self, visit: &mut impl FnMut(&Node)) {
        visit(self);
        for child in self.children() {
            child.walk(visit);
        }
        if let Node::Spline(spline) = self {
            spline.walk(&mut |nested| {
                if let Spline::Multi { coordinate, .. } = nested {
                    coordinate.walk(visit);
                }
            });
        }
    }
}

impl Spline {
    /// 样条的子节点（其坐标函数与嵌套的点值）。
    ///
    /// Sub-nodes of a spline (its coordinate function and nested point
    /// values).
    pub fn children(&self) -> Vec<&Node> {
        match self {
            Spline::Constant(_) => Vec::new(),
            Spline::Multi { coordinate, points } => {
                let children = vec![coordinate.as_ref()];
                for (_, value, _) in points {
                    // Nested splines do not contribute `Node`s themselves;
                    // walk them separately via [`Spline::walk`].
                    let _ = value;
                }
                children
            }
        }
    }

    /// 对样条嵌套结构的先序访问。
    ///
    /// Pre-order visit over the spline's nested structure.
    pub fn walk(&self, visit: &mut impl FnMut(&Spline)) {
        visit(self);
        if let Spline::Multi { points, .. } = self {
            for (_, value, _) in points {
                value.walk(visit);
            }
        }
    }
}

impl Axis {
    fn parse(value: &str, path: &str) -> Result<Self, ParseError> {
        match value {
            "x" => Ok(Self::X),
            "y" => Ok(Self::Y),
            "z" => Ok(Self::Z),
            other => Err(ParseError::bad_value(path, &format!("unknown axis '{other}'"))),
        }
    }
}

/// 类型化密度函数树（链接前：资源引用仍是 [`Node::Ref`]）。
///
/// A typed density function tree (pre-linking: resource references are still
/// [`Node::Ref`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Constant(f64),
    Coord(Axis),
    /// 尚未解析的、指向另一密度函数的 `namespace:path` 引用。
    ///
    /// Unresolved `namespace:path` reference to another density function.
    Ref(String),
    Noise {
        noise: String,
        xz_scale: f64,
        y_scale: f64,
        shift_x: Option<Box<Node>>,
        shift_z: Option<Box<Node>>,
    },
    /// `minecraft:shift_a`：在 `(x/4, 0, z/4)` 处采样噪声，结果（f32）再乘 4。
    ///
    /// `minecraft:shift_a`: samples its noise at `(x/4, 0, z/4)` and
    /// multiplies the f32 result by 4.
    ShiftA { noise: String },
    ShiftB { noise: String },
    Add(Box<Node>, Box<Node>),
    Sub(Box<Node>, Box<Node>),
    Mul(Box<Node>, Box<Node>),
    Div(Box<Node>, Box<Node>),
    Min(Box<Node>, Box<Node>),
    Max(Box<Node>, Box<Node>),
    Negate(Box<Node>),
    Abs(Box<Node>),
    Square(Box<Node>),
    Cube(Box<Node>),
    HalfNegative(Box<Node>),
    QuarterNegative(Box<Node>),
    Squeeze(Box<Node>),
    Clamp { input: Box<Node>, min: f64, max: f64 },
    RangeChoice {
        input: Box<Node>,
        min: f64,
        max: f64,
        in_range: Box<Node>,
        out_of_range: Box<Node>,
    },
    Gradient {
        axis: Axis,
        from_coordinate: f64,
        from_value: f64,
        to_coordinate: f64,
        to_value: f64,
    },
    Lerp {
        alpha: Box<Node>,
        first: Box<Node>,
        second: Box<Node>,
    },
    /// 仅性能标记；标量求值时直通。
    ///
    /// Performance marker only; scalar evaluation passes through.
    Cache(Box<Node>),
    Interpolated {
        cell_size_xz: i64,
        cell_size_y: i64,
        input: Box<Node>,
    },
    /// 将输入的某一个轴固定为 `coordinate` 后求值。
    ///
    /// Evaluates the input with one axis fixed to `coordinate`.
    Slice {
        axis: Axis,
        coordinate: f64,
        input: Box<Node>,
    },
    IntervalSelect {
        input: Box<Node>,
        thresholds: Vec<f64>,
        functions: Vec<Node>,
    },
    Spline(Spline),
    DistanceToPoint {
        /// 仅欧氏度量（26.3 数据中唯一的度量）。
        ///
        /// Euclidean metric only (the only metric in the 26.3 data).
        point: [f64; 3],
    },
    FindTopSurface {
        cell_height: i64,
        density: Box<Node>,
        /// 在 26.3 中 `lower_bound` 是普通整数高度（编解码器 intRange），
        /// 而 `upper_bound` 是密度函数取值。
        ///
        /// `lower_bound` is a plain integer height in 26.3 (codec
        /// intRange), while `upper_bound` is a density-function value.
        lower_bound: i64,
        upper_bound: Box<Node>,
    },
    OldBlendedNoise {
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_scale_multiplier: f64,
    },
    EndOuterIslands,
    /// 结构密度影响叶子：仅地形探测，取值为 0。
    ///
    /// Structure density influence leaf: terrain-only probe yields 0.
    Beardifier,
    /// 具有固定新世界语义的混合标记。
    ///
    /// Blend markers with fixed new-world semantics.
    BlendAlpha,
    BlendOffset,
    BlendDensity(Box<Node>),
}

/// 三次样条：常量，或坐标函数上的多点样条。
///
/// Cubic spline: a constant, or a multi-point spline over a coordinate
/// function.
#[derive(Debug, Clone, PartialEq)]
pub enum Spline {
    Constant(f64),
    Multi {
        coordinate: Box<Node>,
        /// 按文件顺序排列的 `(location, value, derivative)` 三元组。
        ///
        /// `(location, value, derivative)` triples in file order.
        points: Vec<(f64, Spline, f64)>,
    },
}

/// 节点模式解析失败。
///
/// Node-schema parse failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub path: String,
    pub detail: String,
}

impl ParseError {
    pub(crate) fn bad_value(path: &str, detail: impl Into<String>) -> Self {
        Self { path: path.to_string(), detail: detail.into() }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.detail)
    }
}

impl std::error::Error for ParseError {}

impl From<JsonError> for ParseError {
    fn from(error: JsonError) -> Self {
        Self { path: error.path, detail: error.detail }
    }
}

/// 单文件的解析预算（一切都有界）。
///
/// Parse budgets for one file (everything is bounded).
#[derive(Debug, Clone, Copy)]
pub struct ParseBudget {
    pub max_nodes: usize,
}

impl Default for ParseBudget {
    fn default() -> Self {
        Self { max_nodes: 8192 }
    }
}

/// 统计已构建的节点数；超出预算则解析失败。
///
/// Counts nodes built so far; exceeding the budget fails the parse.
#[derive(Debug, Clone, Copy)]
pub struct NodeBudget {
    used: usize,
    max: usize,
}

impl NodeBudget {
    pub fn new(max: usize) -> Self {
        Self { used: 0, max }
    }

    pub(crate) fn charge(&mut self, path: &str) -> Result<(), ParseError> {
        self.used += 1;
        if self.used > self.max {
            return Err(ParseError::bad_value(
                path,
                format!("node budget exceeded ({} nodes)", self.max),
            ));
        }
        Ok(())
    }
}

/// 解析单个密度函数取值（数字 | 引用字符串 | 节点对象）。既用于整份文件，
/// 也用于嵌套子字段。
///
/// Parses one density-function value (number | reference string | node
/// object). Used both for whole files and for nested sub-fields.
pub fn parse_node(json: &Json, path: &str, budget: &mut NodeBudget) -> Result<Node, ParseError> {
    budget.charge(path)?;
    match json {
        Json::Num(n) => {
            if !n.is_finite() {
                return Err(ParseError::bad_value(path, "non-finite constant"));
            }
            Ok(Node::Constant(*n))
        }
        Json::Str(reference) => parse_reference(reference, path),
        Json::Obj(_) => parse_typed_node(json, path, budget),
        other => Err(ParseError::bad_value(
            path,
            format!("expected number, reference or object, got {other:?}"),
        )),
    }
}

/// 引用字符串：识别内置坐标叶子；其余一律保留为 [`Node::Ref`] 交给链接器。
///
/// Reference strings: built-in coordinate leaves are recognized; everything
/// else stays a [`Node::Ref`] for the linker.
fn parse_reference(reference: &str, path: &str) -> Result<Node, ParseError> {
    match reference {
        "minecraft:x" => Ok(Node::Coord(Axis::X)),
        "minecraft:y" => Ok(Node::Coord(Axis::Y)),
        "minecraft:z" => Ok(Node::Coord(Axis::Z)),
        "minecraft:zero" => Ok(Node::Constant(0.0)),
        other => {
            if is_namespaced(other) {
                Ok(Node::Ref(other.to_string()))
            } else {
                Err(ParseError::bad_value(
                    path,
                    format!("reference '{other}' is not namespaced"),
                ))
            }
        }
    }
}

fn is_namespaced(value: &str) -> bool {
    let Some((ns, path)) = value.split_once(':') else {
        return false;
    };
    let valid =
        |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '/' | '-'));
    valid(ns) && valid(path)
}

fn require_field<'a>(json: &'a Json, key: &str, path: &str) -> Result<&'a Json, ParseError> {
    json.get(key)
        .ok_or_else(|| ParseError::bad_value(path, &format!("missing required field '{key}'")))
}

fn number_field(json: &Json, key: &str, path: &str) -> Result<f64, ParseError> {
    let field_path = format!("{path}.{key}");
    let value = require_field(json, key, path)?;
    value.as_f64().ok_or_else(|| {
        ParseError::bad_value(&field_path, format!("expected a number, got {value:?}"))
    })
}

fn int_field(json: &Json, key: &str, path: &str) -> Result<i64, ParseError> {
    let field_path = format!("{path}.{key}");
    let value = require_field(json, key, path)?;
    value.as_i64().ok_or_else(|| {
        ParseError::bad_value(&field_path, format!("expected an integer, got {value:?}"))
    })
}

fn optional_node_field(
    json: &Json,
    key: &str,
    path: &str,
    budget: &mut NodeBudget,
) -> Result<Option<Box<Node>>, ParseError> {
    match json.get(key) {
        None => Ok(None),
        Some(value) => Ok(Some(Box::new(parse_node(
            value,
            &format!("{path}.{key}"),
            budget,
        )?))),
    }
}

fn node_field(json: &Json, key: &str, path: &str, budget: &mut NodeBudget) -> Result<Box<Node>, ParseError> {
    let field_path = format!("{path}.{key}");
    let value = require_field(json, key, path)?;
    parse_node(value, &field_path, budget).map(Box::new)
}

fn string_field(json: &Json, key: &str, path: &str) -> Result<String, ParseError> {
    let field_path = format!("{path}.{key}");
    let value = require_field(json, key, path)?;
    value.as_str().ok_or_else(|| {
        ParseError::bad_value(&field_path, format!("expected a string, got {value:?}"))
    }).map(str::to_string)
}

/// 拒绝节点类型精确模式之外的字段。
///
/// Rejects fields outside the version's exact schema for the node type.
fn reject_unknown_fields(
    json: &Json,
    allowed: &[&str],
    path: &str,
) -> Result<(), ParseError> {
    if let Json::Obj(fields) = json {
        for (key, _) in fields {
            if key != "type" && !allowed.contains(&key.as_str()) {
                return Err(ParseError::bad_value(
                    &format!("{path}.{key}"),
                    format!("unknown field (allowed: {allowed:?})"),
                ));
            }
        }
    }
    Ok(())
}

fn parse_typed_node(json: &Json, path: &str, budget: &mut NodeBudget) -> Result<Node, ParseError> {
    let type_name = string_field(json, "type", path)?;
    let type_name = type_name
        .strip_prefix("minecraft:")
        .ok_or_else(|| ParseError::bad_value(&format!("{path}.type"), format!("node type '{type_name}' must be namespaced")))?;
    match type_name {
        "constant" => {
            reject_unknown_fields(json, &["argument"], path)?;
            let value = number_field(json, "argument", path)?;
            if !value.is_finite() {
                return Err(ParseError::bad_value(&format!("{path}.argument"), "non-finite constant"));
            }
            Ok(Node::Constant(value))
        }
        "noise" => {
            reject_unknown_fields(json, &["noise", "xz_scale", "y_scale", "shift_x", "shift_z"], path)?;
            let noise = string_field(json, "noise", path)?;
            let node = Node::Noise {
                noise,
                xz_scale: number_field(json, "xz_scale", path)?,
                y_scale: number_field(json, "y_scale", path)?,
                shift_x: optional_node_field(json, "shift_x", path, budget)?,
                shift_z: optional_node_field(json, "shift_z", path, budget)?,
            };
            Ok(node)
        }
        "shift_a" | "shift_b" => {
            reject_unknown_fields(json, &["noise"], path)?;
            let noise = string_field(json, "noise", path)?;
            Ok(if type_name == "shift_a" {
                Node::ShiftA { noise }
            } else {
                Node::ShiftB { noise }
            })
        }
        "add" | "sub" | "mul" | "div" | "min" | "max" => {
            reject_unknown_fields(json, &["left", "right"], path)?;
            let left = node_field(json, "left", path, budget)?;
            let right = node_field(json, "right", path, budget)?;
            Ok(match type_name {
                "add" => Node::Add(left, right),
                "sub" => Node::Sub(left, right),
                "mul" => Node::Mul(left, right),
                "div" => Node::Div(left, right),
                "min" => Node::Min(left, right),
                _ => Node::Max(left, right),
            })
        }
        "negate" => {
            reject_unknown_fields(json, &["input"], path)?;
            Ok(Node::Negate(node_field(json, "input", path, budget)?))
        }
        "abs" | "square" | "cube" | "half_negative" | "quarter_negative" | "squeeze" => {
            reject_unknown_fields(json, &["input"], path)?;
            let input = node_field(json, "input", path, budget)?;
            Ok(match type_name {
                "abs" => Node::Abs(input),
                "square" => Node::Square(input),
                "cube" => Node::Cube(input),
                "half_negative" => Node::HalfNegative(input),
                "quarter_negative" => Node::QuarterNegative(input),
                _ => Node::Squeeze(input),
            })
        }
        "clamp" => {
            reject_unknown_fields(json, &["input", "min", "max"], path)?;
            let min = number_field(json, "min", path)?;
            let max = number_field(json, "max", path)?;
            if min > max {
                return Err(ParseError::bad_value(
                    &format!("{path}.min"),
                    format!("clamp min {min} exceeds max {max}"),
                ));
            }
            Ok(Node::Clamp {
                input: node_field(json, "input", path, budget)?,
                min,
                max,
            })
        }
        "range_choice" => {
            reject_unknown_fields(
                json,
                &["input", "min_inclusive", "max_exclusive", "when_in_range", "when_out_of_range"],
                path,
            )?;
            let min = number_field(json, "min_inclusive", path)?;
            let max = number_field(json, "max_exclusive", path)?;
            if min >= max {
                return Err(ParseError::bad_value(
                    &format!("{path}.min_inclusive"),
                    format!("range min_inclusive {min} must be below max_exclusive {max}"),
                ));
            }
            Ok(Node::RangeChoice {
                input: node_field(json, "input", path, budget)?,
                min,
                max,
                in_range: node_field(json, "when_in_range", path, budget)?,
                out_of_range: node_field(json, "when_out_of_range", path, budget)?,
            })
        }
        "gradient" => {
            reject_unknown_fields(
                json,
                &["axis", "from_coordinate", "from_value", "to_coordinate", "to_value"],
                path,
            )?;
            let axis = Axis::parse(string_field(json, "axis", path)?.as_str(), &format!("{path}.axis"))?;
            let from_coordinate = number_field(json, "from_coordinate", path)?;
            let to_coordinate = number_field(json, "to_coordinate", path)?;
            if from_coordinate == to_coordinate {
                return Err(ParseError::bad_value(
                    &format!("{path}.from_coordinate"),
                    "gradient from_coordinate equals to_coordinate",
                ));
            }
            Ok(Node::Gradient {
                axis,
                from_coordinate,
                from_value: number_field(json, "from_value", path)?,
                to_coordinate,
                to_value: number_field(json, "to_value", path)?,
            })
        }
        "lerp" => {
            reject_unknown_fields(json, &["alpha", "first", "second"], path)?;
            Ok(Node::Lerp {
                alpha: node_field(json, "alpha", path, budget)?,
                first: node_field(json, "first", path, budget)?,
                second: node_field(json, "second", path, budget)?,
            })
        }
        "cache" => {
            reject_unknown_fields(json, &["input"], path)?;
            Ok(Node::Cache(node_field(json, "input", path, budget)?))
        }
        "interpolated" => {
            reject_unknown_fields(json, &["input", "cell_size_xz", "cell_size_y"], path)?;
            let cell_size_xz = int_field(json, "cell_size_xz", path)?;
            let cell_size_y = int_field(json, "cell_size_y", path)?;
            if cell_size_xz <= 0 || cell_size_y <= 0 {
                return Err(ParseError::bad_value(
                    &format!("{path}.cell_size_xz"),
                    "cell sizes must be positive",
                ));
            }
            Ok(Node::Interpolated {
                cell_size_xz,
                cell_size_y,
                input: node_field(json, "input", path, budget)?,
            })
        }
        "slice" => {
            reject_unknown_fields(json, &["axis", "coordinate", "input"], path)?;
            let axis = Axis::parse(string_field(json, "axis", path)?.as_str(), &format!("{path}.axis"))?;
            Ok(Node::Slice {
                axis,
                coordinate: number_field(json, "coordinate", path)?,
                input: node_field(json, "input", path, budget)?,
            })
        }
        "interval_select" => {
            reject_unknown_fields(json, &["input", "thresholds", "functions"], path)?;
            let input = node_field(json, "input", path, budget)?;
            let field_path = format!("{path}.thresholds");
            let thresholds_json = require_field(json, "thresholds", path)?;
            let Json::Arr(threshold_items) = thresholds_json else {
                return Err(ParseError::bad_value(&field_path, "expected an array"));
            };
            let mut thresholds = Vec::with_capacity(threshold_items.len());
            for (i, item) in threshold_items.iter().enumerate() {
                let value = item.as_f64().ok_or_else(|| {
                    ParseError::bad_value(&format!("{field_path}[{i}]"), "expected a number")
                })?;
                thresholds.push(value);
            }
            for pair in thresholds.windows(2) {
                if pair[0] >= pair[1] {
                    return Err(ParseError::bad_value(
                        &field_path,
                        "thresholds must be strictly ascending",
                    ));
                }
            }
            let functions_field = format!("{path}.functions");
            let functions_json = require_field(json, "functions", path)?;
            let Json::Arr(function_items) = functions_json else {
                return Err(ParseError::bad_value(&functions_field, "expected an array"));
            };
            if function_items.len() != thresholds.len() + 1 {
                return Err(ParseError::bad_value(
                    &functions_field,
                    format!(
                        "expected {} functions (thresholds+1), got {}",
                        thresholds.len() + 1,
                        function_items.len()
                    ),
                ));
            }
            let mut functions = Vec::with_capacity(function_items.len());
            for (i, item) in function_items.iter().enumerate() {
                functions.push(parse_node(item, &format!("{functions_field}[{i}]"), budget)?);
            }
            Ok(Node::IntervalSelect { input, thresholds, functions })
        }
        "spline" => {
            reject_unknown_fields(json, &["spline"], path)?;
            let spline = parse_spline(require_field(json, "spline", path)?, &format!("{path}.spline"), budget)?;
            Ok(Node::Spline(spline))
        }
        "distance_to_point" => {
            reject_unknown_fields(json, &["metric", "point"], path)?;
            let metric = string_field(json, "metric", path)?;
            if metric != "euclidean" {
                return Err(ParseError::bad_value(
                    &format!("{path}.metric"),
                    format!("unsupported metric '{metric}' (only euclidean)"),
                ));
            }
            let point_path = format!("{path}.point");
            let point_json = require_field(json, "point", path)?;
            let Json::Arr(items) = point_json else {
                return Err(ParseError::bad_value(&point_path, "expected [x, y, z]"));
            };
            if items.len() != 3 {
                return Err(ParseError::bad_value(&point_path, "expected exactly 3 coordinates"));
            }
            let mut point = [0.0f64; 3];
            for (i, item) in items.iter().enumerate() {
                point[i] = item.as_f64().ok_or_else(|| {
                    ParseError::bad_value(&format!("{point_path}[{i}]"), "expected a number")
                })?;
            }
            Ok(Node::DistanceToPoint { point })
        }
        "find_top_surface" => {
            reject_unknown_fields(json, &["cell_height", "density", "lower_bound", "upper_bound"], path)?;
            let cell_height = int_field(json, "cell_height", path)?;
            if cell_height <= 0 {
                return Err(ParseError::bad_value(
                    &format!("{path}.cell_height"),
                    "cell_height must be positive",
                ));
            }
            Ok(Node::FindTopSurface {
                cell_height,
                density: node_field(json, "density", path, budget)?,
                lower_bound: int_field(json, "lower_bound", path)?,
                upper_bound: node_field(json, "upper_bound", path, budget)?,
            })
        }
        "old_blended_noise" => {
            reject_unknown_fields(
                json,
                &["xz_scale", "y_scale", "xz_factor", "y_factor", "smear_scale_multiplier"],
                path,
            )?;
            Ok(Node::OldBlendedNoise {
                xz_scale: number_field(json, "xz_scale", path)?,
                y_scale: number_field(json, "y_scale", path)?,
                xz_factor: number_field(json, "xz_factor", path)?,
                y_factor: number_field(json, "y_factor", path)?,
                smear_scale_multiplier: number_field(json, "smear_scale_multiplier", path)?,
            })
        }
        "end_outer_islands" => {
            reject_unknown_fields(json, &[], path)?;
            Ok(Node::EndOuterIslands)
        }
        "beardifier" => {
            reject_unknown_fields(json, &[], path)?;
            Ok(Node::Beardifier)
        }
        "blend_alpha" => {
            reject_unknown_fields(json, &[], path)?;
            Ok(Node::BlendAlpha)
        }
        "blend_offset" => {
            reject_unknown_fields(json, &[], path)?;
            Ok(Node::BlendOffset)
        }
        "blend_density" => {
            reject_unknown_fields(json, &["input"], path)?;
            Ok(Node::BlendDensity(node_field(json, "input", path, budget)?))
        }
        other => Err(ParseError::bad_value(
            &format!("{path}.type"),
            format!("unsupported density function type 'minecraft:{other}' for data version 26.3"),
        )),
    }
}

fn parse_spline(json: &Json, path: &str, budget: &mut NodeBudget) -> Result<Spline, ParseError> {
    budget.charge(path)?;
    match json {
        Json::Num(value) => Ok(Spline::Constant(*value)),
        Json::Obj(_) => {
            reject_unknown_fields(json, &["coordinate", "points"], path)?;
            let coordinate = node_field(json, "coordinate", path, budget)?;
            let points_path = format!("{path}.points");
            let points_json = require_field(json, "points", path)?;
            let Json::Arr(items) = points_json else {
                return Err(ParseError::bad_value(&points_path, "expected an array"));
            };
            if items.len() < 2 {
                return Err(ParseError::bad_value(&points_path, "spline needs at least 2 points"));
            }
            let mut points = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let point_path = format!("{points_path}[{i}]");
                reject_unknown_fields(item, &["location", "value", "derivative"], &point_path)?;
                let location = number_field(item, "location", &point_path)?;
                let derivative = number_field(item, "derivative", &point_path)?;
                let value = parse_spline(
                    require_field(item, "value", &point_path)?,
                    &format!("{point_path}.value"),
                    budget,
                )?;
                points.push((location, value, derivative));
            }
            for pair in points.windows(2) {
                if pair[0].0 >= pair[1].0 {
                    return Err(ParseError::bad_value(
                        &points_path,
                        "spline locations must be strictly ascending",
                    ));
                }
            }
            Ok(Spline::Multi { coordinate, points })
        }
        other => Err(ParseError::bad_value(
            path,
            format!("expected a number or spline object, got {other:?}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::json::parse_strict;

    fn parse(source: &str) -> Result<Node, ParseError> {
        let json = parse_strict(source.as_bytes())?;
        parse_node(&json, "$", &mut NodeBudget::new(1024))
    }

    #[test]
    fn value_triple_form_numbers_references_and_objects() {
        assert_eq!(parse("0.25").unwrap(), Node::Constant(0.25));
        assert_eq!(parse("\"minecraft:y\"").unwrap(), Node::Coord(Axis::Y));
        assert_eq!(
            parse("\"minecraft:overworld/depth\"").unwrap(),
            Node::Ref("minecraft:overworld/depth".to_string())
        );
    }

    #[test]
    fn binary_and_unary_nodes_parse_with_exact_fields() {
        let node = parse(
            r#"{"type":"minecraft:sub","left":100.0,"right":{"type":"minecraft:abs","input":-3}}"#,
        )
        .unwrap();
        assert_eq!(
            node,
            Node::Sub(
                Box::new(Node::Constant(100.0)),
                Box::new(Node::Abs(Box::new(Node::Constant(-3.0))))
            )
        );
    }

    #[test]
    fn unknown_type_and_unknown_field_are_loud() {
        let error = parse(r#"{"type":"minecraft:weird_scaled_sampler"}"#).unwrap_err();
        assert!(error.detail.contains("unsupported"), "got: {error}");
        let error = parse(
            r#"{"type":"minecraft:abs","input":1,"surprise":2}"#,
        )
        .unwrap_err();
        assert!(error.path.ends_with(".surprise"), "got: {error}");
        let error = parse(r#"{"type":"minecraft:abs"}"#).unwrap_err();
        assert!(error.detail.contains("missing required field 'input'"), "got: {error}");
    }

    #[test]
    fn noise_node_carries_optional_shifts() {
        let node = parse(
            r#"{"type":"minecraft:noise","noise":"minecraft:continentalness","shift_x":"minecraft:shift_x","shift_z":"minecraft:shift_z","xz_scale":0.25,"y_scale":0.0}"#,
        )
        .unwrap();
        match node {
            Node::Noise { noise, shift_x, shift_z, xz_scale, y_scale } => {
                assert_eq!(noise, "minecraft:continentalness");
                assert_eq!(xz_scale, 0.25);
                assert_eq!(y_scale, 0.0);
                assert_eq!(*shift_x.unwrap(), Node::Ref("minecraft:shift_x".to_string()));
                assert!(shift_z.is_some());
            }
            other => panic!("expected noise node, got {other:?}"),
        }
    }

    #[test]
    fn validation_rejects_broken_ranges_and_metrics() {
        assert!(parse(
            r#"{"type":"minecraft:clamp","input":1,"min":5,"max":1}"#
        )
        .unwrap_err()
        .detail
        .contains("exceeds max"));
        assert!(parse(
            r#"{"type":"minecraft:distance_to_point","metric":"manhattan","point":[0,0,0]}"#
        )
        .unwrap_err()
        .detail
        .contains("metric"));
        assert!(parse(
            r#"{"type":"minecraft:range_choice","input":0,"min_inclusive":5,"max_exclusive":5,"when_in_range":1,"when_out_of_range":2}"#
        )
        .unwrap_err()
        .detail
        .contains("below max_exclusive"));
    }

    #[test]
    fn spline_parses_nested_values_and_validates_ordering() {
        let node = parse(
            r#"{"type":"minecraft:spline","spline":{"coordinate":"minecraft:overworld/continents","points":[
                {"location":-1.0,"value":0.0,"derivative":0.0},
                {"location":0.0,"value":{"coordinate":"minecraft:overworld/erosion","points":[
                    {"location":-1.0,"value":1.0,"derivative":0.0},
                    {"location":1.0,"value":2.0,"derivative":0.0}]},
                 "derivative":0.0},
                {"location":1.0,"value":3.0,"derivative":0.0}
            ]}}"#,
        )
        .unwrap();
        match node {
            Node::Spline(Spline::Multi { points, .. }) => assert_eq!(points.len(), 3),
            other => panic!("expected spline, got {other:?}"),
        }
        // Descending locations are rejected.
        assert!(parse(
            r#"{"type":"minecraft:spline","spline":{"coordinate":0,"points":[
                {"location":1.0,"value":0.0,"derivative":0.0},
                {"location":-1.0,"value":0.0,"derivative":0.0}]}}"#
        )
        .is_err());
    }

    #[test]
    fn interval_select_requires_functions_eq_thresholds_plus_one() {
        assert!(parse(
            r#"{"type":"minecraft:interval_select","input":0,"thresholds":[0.0],"functions":[1,2,3]}"#
        )
        .is_err());
        let node = parse(
            r#"{"type":"minecraft:interval_select","input":0,"thresholds":[-0.1,0.1],"functions":[1,2,3]}"#,
        )
        .unwrap();
        match node {
            Node::IntervalSelect { thresholds, functions, .. } => {
                assert_eq!(thresholds, vec![-0.1, 0.1]);
                assert_eq!(functions.len(), 3);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn node_budget_bounds_the_tree() {
        let json = parse_strict(br#"{"type":"minecraft:add","left":1,"right":2}"#).unwrap();
        let mut budget = NodeBudget::new(1);
        let error = parse_node(&json, "$", &mut budget).unwrap_err();
        assert!(error.detail.contains("budget"), "got: {error}");
    }
}
