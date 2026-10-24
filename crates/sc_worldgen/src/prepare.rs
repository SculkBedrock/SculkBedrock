//! 世界生成数据的闭包解析与准备阶段。
//!
//! Closure resolution and preparation for worldgen bundles.
//!
//! 从选定的根资源（`noise_settings`）出发解析可达资源闭包，在可解析的引用上
//! 检测环，对无效 JSON 报出包含 bundle 键的错误，并冻结确定性摘要。引用扫描把
//! 每个能解析到 bundle 文件的具名字符串视为一条边；无法解析的具名字符串记录为
//! 诊断信息，交由 schema 校验区分节点类型字符串与真正缺失的资源（节点 *类型*
//! 字符串如 `minecraft:add` 永不匹配文件键，在此忽略）。
//!
//! Resolves the reachable resource closure from a selected root
//! (`noise_settings`), detects cycles over resolvable references, rejects
//! invalid JSON with the offending bundle key, and freezes deterministic
//! digests. The reference scan treats every namespaced string that resolves
//! to an existing bundle file as an edge and records unresolved namespaced
//! strings as diagnostics; schema validation later classifies them as node
//! types (ignored) or genuinely missing resources (errors). Node *type*
//! strings like `minecraft:add` never match file keys, so they are ignored
//! here.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::sources::{WorldgenCategory, WorldgenSources};
use serde::{Deserialize, Serialize};

/// 准备好的世界生成数据集：已解析闭包、确定性摘要与未解析引用诊断。
/// 返回后不可变。
///
/// A prepared worldgen dataset: the resolved closure, deterministic
/// digests and unresolved-reference diagnostics. Immutable once returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedWorldgen {
    /// 选定的根资源，如 `minecraft:overworld`。
    ///
    /// Selected root resource, e.g. `minecraft:overworld`.
    pub root: String,
    /// 从根可达的全部资源（含根本身）。
    ///
    /// All resources reachable from the root (including the root itself).
    pub closure: BTreeSet<ClosureNode>,
    /// 闭包内已解析的引用边（from → to）。
    ///
    /// Resolved reference edges (from → to) inside the closure.
    pub edges: BTreeSet<(ClosureNode, ClosureNode)>,
    /// 未解析到任何 bundle 文件的具名字符串及其出现次数；schema 校验负责区分
    /// 节点类型字符串（忽略）与真正缺失的资源（错误）。
    ///
    /// Namespaced strings that did not resolve to any bundle file, with
    /// occurrence counts; schema validation distinguishes node type strings
    /// from genuinely missing resources.
    pub unresolved_references: BTreeMap<String, usize>,
    /// 整个源 bundle 的 FNV-1a 64；等于提取来源清单的 `closure_fingerprint`。
    ///
    /// FNV-1a 64 over the full source bundle; equals the extraction
    /// provenance manifest's `closure_fingerprint`.
    pub content_fingerprint: u64,
    /// 仅可达闭包的 FNV-1a 64（排序后的节点行，含各文件内容摘要）；
    /// 作为生成器描述符的闭包指纹输入。
    ///
    /// FNV-1a 64 over the reachable closure only (sorted node lines with
    /// per-file content digests); feeds the generator descriptor's closure
    /// fingerprint.
    pub closure_fingerprint: u64,
}

/// 一个已解析的世界生成资源。
///
/// One resolved worldgen resource.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ClosureNode {
    /// 资源类别。
    ///
    /// The resource category.
    pub category: WorldgenCategory,
    /// bundle 键：`<namespace>/<path>`。
    ///
    /// Bundle key: `<namespace>/<path>`.
    pub id: String,
}

impl ClosureNode {
    /// 闭包指纹使用的节点行：`category|key|fnv64(content)`。
    ///
    /// Node line used by the closure fingerprint:
    /// `category|key|fnv64(content)`.
    fn fingerprint_line(&self, content: &[u8]) -> Vec<u8> {
        format!(
            "{}|{}|{:016x}",
            self.category.name(),
            self.id,
            crate::fnv1a64(content)
        )
        .into_bytes()
    }
}

/// 准备阶段失败。所有失败都是显式的，不回退默认值。
///
/// Preparation failures. All are loud: nothing falls back to defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareError {
    /// bundle 中没有任何文件。
    ///
    /// The bundle contains no files at all.
    EmptyBundle,
    /// 选定的根资源不存在。
    ///
    /// The selected root resource is absent.
    RootMissing { root: String },
    /// 某个 JSON 文件解析失败。
    ///
    /// A JSON file failed to parse.
    InvalidJson { key: String, detail: String },
    /// 可达引用图上存在环；链路首尾为同一节点，携带用于诊断。
    ///
    /// The reachable reference graph contains a cycle; the chain starts and
    /// ends at the same node and is included for diagnosis.
    ReferenceCycle { chain: Vec<ClosureNode> },
}

impl std::fmt::Display for PrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyBundle => write!(f, "worldgen bundle is empty"),
            Self::RootMissing { root } => {
                write!(f, "selected root resource is missing from the bundle: {root}")
            }
            Self::InvalidJson { key, detail } => {
                write!(f, "invalid worldgen JSON at {key}: {detail}")
            }
            Self::ReferenceCycle { chain } => {
                write!(f, "worldgen reference cycle: ")?;
                for (i, node) in chain.iter().enumerate() {
                    if i > 0 {
                        write!(f, " -> ")?;
                    }
                    write!(f, "{}:{}", node.category.name(), node.id)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for PrepareError {}

/// 解析 `root`（`minecraft:overworld` 这类 `noise_settings` 资源）的闭包
/// 并冻结摘要。对可解析引用做 BFS，遍历确定性（仅用 BTreeMap/BTreeSet）。
///
/// Resolves the closure for `root` (a `noise_settings` resource like
/// `minecraft:overworld`) and freezes digests. BFS over resolvable
/// references keeps the traversal deterministic (BTreeMaps/sets only).
///
/// - `sources`: 世界生成 bundle 源
/// - `root`: 根资源 id
pub fn prepare_worldgen(
    sources: &WorldgenSources,
    root: &str,
) -> Result<PreparedWorldgen, PrepareError> {
    if sources.is_empty() {
        return Err(PrepareError::EmptyBundle);
    }
    let root_bytes = sources
        .resolve(WorldgenCategory::NoiseSettings, root)
        .ok_or_else(|| PrepareError::RootMissing { root: root.to_string() })?
        .clone();
    let root_node = ClosureNode {
        category: WorldgenCategory::NoiseSettings,
        id: key_of(root)?,
    };

    let mut closure: BTreeSet<ClosureNode> = BTreeSet::new();
    let mut edges: BTreeSet<(ClosureNode, ClosureNode)> = BTreeSet::new();
    let mut unresolved: BTreeMap<String, usize> = BTreeMap::new();
    let mut node_contents: BTreeMap<ClosureNode, Vec<u8>> = BTreeMap::new();

    let mut queue: VecDeque<ClosureNode> = VecDeque::new();
    queue.push_back(root_node.clone());
    node_contents.insert(root_node, root_bytes);

    while let Some(node) = queue.pop_front() {
        if !closure.insert(node.clone()) {
            continue;
        }
        let content = node_contents.get(&node).expect("queued nodes have content");
        let value: serde_json::Value = serde_json::from_slice(content).map_err(|e| {
            PrepareError::InvalidJson { key: node.id.clone(), detail: e.to_string() }
        })?;
        for reference in collect_namespaced_strings(&value) {
            let resolved = resolve_reference(node.category, sources, &reference);
            match resolved {
                Some(target_node) => {
                    let target = target_node;
                    if edges.insert((node.clone(), target.clone())) && !closure.contains(&target) {
                        // First visit: fetch content for BFS.
                        let bytes = sources
                            .category(target.category)
                            .get(&target.id)
                            .expect("resolved keys exist")
                            .clone();
                        node_contents.insert(target.clone(), bytes);
                        queue.push_back(target);
                    }
                }
                None => {
                    *unresolved.entry(reference).or_insert(0) += 1;
                }
            }
        }
    }

    detect_cycle(&edges)?;

    let mut sorted_lines: Vec<Vec<u8>> = closure
        .iter()
        .map(|node| {
            node.fingerprint_line(
                node_contents.get(node).expect("closure nodes have content"),
            )
        })
        .collect();
    sorted_lines.sort();
    let closure_fingerprint = crate::fnv1a64(&sorted_lines.join(&b'\n'));

    Ok(PreparedWorldgen {
        root: root.to_string(),
        closure,
        edges,
        unresolved_references: unresolved,
        content_fingerprint: sources.content_fingerprint(),
        closure_fingerprint,
    })
}

/// Converts a `namespace:path` reference into a bundle key.
fn key_of(resource: &str) -> Result<String, PrepareError> {
    resource
        .split_once(':')
        .map(|(ns, path)| format!("{ns}/{path}"))
        .ok_or_else(|| PrepareError::InvalidJson {
            key: resource.to_string(),
            detail: "reference without namespace".to_string(),
        })
}

/// Allowed target categories per source category, matching the 26.3 data
/// model: density functions reference density functions and noises; noise
/// settings reference density functions and noises (the router); noises
/// reference nothing. Plain data strings inside a file (block names, biome
/// names, the dimension id itself) never create edges — without this
/// restriction the real 26.3 bundle produces false cycles through strings
/// like `minecraft:overworld` inside `noise_settings`.
fn allowed_reference_targets(from: WorldgenCategory) -> &'static [WorldgenCategory] {
    match from {
        WorldgenCategory::NoiseSettings => &[
            WorldgenCategory::DensityFunction,
            WorldgenCategory::Noise,
        ],
        WorldgenCategory::DensityFunction => &[
            WorldgenCategory::DensityFunction,
            WorldgenCategory::Noise,
        ],
        WorldgenCategory::Noise => &[],
    }
}

/// Finds the category holding a reference (first match in stable category
/// order among the targets allowed for `from`; a resource id may exist in
/// several categories only if the pack is ambiguous, which is reported by
/// keeping the first in category order).
fn resolve_reference(
    from: WorldgenCategory,
    sources: &WorldgenSources,
    reference: &str,
) -> Option<ClosureNode> {
    for category in allowed_reference_targets(from) {
        if sources.resolve(*category, reference).is_some() {
            return Some(ClosureNode {
                category: *category,
                id: key_of(reference).ok()?,
            });
        }
    }
    None
}

/// Collects every namespaced string in the JSON tree (`ns:path` shape).
/// Strings without `:` are plain scalars; node type names resolve to no file
/// and land in `unresolved_references`.
fn collect_namespaced_strings(value: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::String(s) => {
                if looks_namespaced(s) {
                    out.push(s.clone());
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, out);
                }
            }
            serde_json::Value::Object(map) => {
                for (_, v) in map {
                    walk(v, out);
                }
            }
            _ => {}
        }
    }
    walk(value, &mut out);
    out
}

/// Conservative namespaced-string shape: `<ns>:<path>` with lowercase
/// resource characters. Rejects empty parts and path escapes so stray prose
/// is not mistaken for a reference.
fn looks_namespaced(s: &str) -> bool {
    let Some((ns, path)) = s.split_once(':') else {
        return false;
    };
    let valid = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '/' | '-'))
    };
    valid(ns) && valid(path) && !path.contains("//") && !path.starts_with('/')
}

/// Detects a cycle in the resolved reference graph and reports one chain.
fn detect_cycle(edges: &BTreeSet<(ClosureNode, ClosureNode)>) -> Result<(), PrepareError> {
    let mut adj: BTreeMap<&ClosureNode, Vec<&ClosureNode>> = BTreeMap::new();
    for (from, to) in edges {
        adj.entry(from).or_default().push(to);
    }
    // Deterministic DFS with colors; iterative to keep the stack bounded.
    const WHITE: u8 = 0;
    const GRAY: u8 = 1;
    const BLACK: u8 = 2;
    let mut color: BTreeMap<&ClosureNode, u8> = BTreeMap::new();
    for node in adj.keys() {
        color.insert(node, WHITE);
    }
    for start in adj.keys().copied().collect::<Vec<_>>() {
        if color.get(start).copied().unwrap_or(WHITE) != WHITE {
            continue;
        }
        // Stack of (node, next-child-index).
        let mut stack: Vec<(&ClosureNode, usize)> = vec![(start, 0)];
        let mut path: Vec<ClosureNode> = vec![start.clone()];
        color.insert(start, GRAY);
        while let Some((node, child_index)) = stack.pop() {
            let children = adj.get(node).cloned().unwrap_or_default();
            if child_index < children.len() {
                let child = children[child_index];
                stack.push((node, child_index + 1));
                match color.get(child).copied().unwrap_or(WHITE) {
                    GRAY => {
                        // Found a cycle: trim the path to the gray node.
                        let pos = path
                            .iter()
                            .position(|n| n == child)
                            .unwrap_or(0);
                        let mut chain: Vec<ClosureNode> = path[pos..].to_vec();
                        chain.push(child.clone());
                        return Err(PrepareError::ReferenceCycle { chain });
                    }
                    WHITE => {
                        color.insert(child, GRAY);
                        path.push(child.clone());
                        stack.push((child, 0));
                    }
                    _ => {}
                }
            } else {
                color.insert(node, BLACK);
                path.pop();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(name: &str, body: &str) -> (String, Vec<u8>) {
        (name.to_string(), body.as_bytes().to_vec())
    }

    #[test]
    fn resolves_full_closure_from_noise_settings_root() {
        let sources = WorldgenSources {
            density_functions: BTreeMap::from([
                noise(
                    "minecraft/overworld/continents",
                    r#"{"type":"minecraft:shifted_noise","noise":"minecraft:overworld/temperature","shift_x":"minecraft:overworld/shift_x","shift_z":"minecraft:overworld/shift_z"}"#,
                ),
                noise(
                    "minecraft/overworld/shift_x",
                    r#"{"type":"minecraft:noise","noise":"minecraft:overworld/temperature"}"#,
                ),
                // Unreachable from the root: must stay out of the closure.
                noise("minecraft/unused", r#"{"type":"minecraft:constant","argument":1}"#),
            ]),
            noises: BTreeMap::from([noise(
                "minecraft/overworld/temperature",
                r#"{"firstOctave":-9,"amplitudes":[1.0]}"#,
            )]),
            noise_settings: BTreeMap::from([noise(
                "minecraft/overworld",
                r#"{"sea_level":63,"noise_router":{"final_density":"minecraft:overworld/continents"}}"#,
            )]),
        };

        let prepared = prepare_worldgen(&sources, "minecraft:overworld").unwrap();
        let ids: Vec<&str> = prepared
            .closure
            .iter()
            .map(|n| n.id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec![
                // BTreeSet orders by (category name, id).
                "minecraft/overworld/continents",
                "minecraft/overworld/shift_x",
                "minecraft/overworld/temperature",
                "minecraft/overworld",
            ]
        );
        assert_eq!(prepared.edges.len(), 4, "root->df, df->noise, df->shift_x, shift_x->noise");
        // Node type strings of reachable files are recorded as unresolved
        // references; the unreachable file's content is never parsed, so its
        // `minecraft:constant` type does not appear.
        assert!(prepared.unresolved_references.contains_key("minecraft:shifted_noise"));
        assert!(prepared.unresolved_references.contains_key("minecraft:noise"));
        assert!(!prepared.unresolved_references.contains_key("minecraft:constant"));

        // Determinism: re-preparing yields identical digests.
        let again = prepare_worldgen(&sources, "minecraft:overworld").unwrap();
        assert_eq!(prepared, again);
        // Content fingerprint covers the whole bundle (including unused).
        assert_eq!(prepared.content_fingerprint, sources.content_fingerprint());
        // Closure fingerprint is content-sensitive.
        let mut mutated = sources.clone();
        mutated.noises.insert(
            "minecraft/overworld/temperature".into(),
            br#"{"firstOctave":-8,"amplitudes":[1.0]}"#.to_vec(),
        );
        let other = prepare_worldgen(&mutated, "minecraft:overworld").unwrap();
        assert_ne!(prepared.closure_fingerprint, other.closure_fingerprint);
    }

    #[test]
    fn missing_root_is_a_loud_error() {
        let sources = WorldgenSources {
            noise_settings: BTreeMap::from([noise("minecraft/overworld", "{}")]),
            ..Default::default()
        };
        match prepare_worldgen(&sources, "minecraft:nether") {
            Err(PrepareError::RootMissing { root }) => assert_eq!(root, "minecraft:nether"),
            other => panic!("expected RootMissing, got {other:?}"),
        }
        assert!(matches!(
            prepare_worldgen(&WorldgenSources::default(), "minecraft:overworld"),
            Err(PrepareError::EmptyBundle)
        ));
    }

    #[test]
    fn invalid_json_reports_the_bundle_key() {
        let sources = WorldgenSources {
            noise_settings: BTreeMap::from([noise(
                "minecraft/overworld",
                "{\"sea_level\": ",
            )]),
            ..Default::default()
        };
        match prepare_worldgen(&sources, "minecraft:overworld") {
            Err(PrepareError::InvalidJson { key, .. }) => {
                assert_eq!(key, "minecraft/overworld")
            }
            other => panic!("expected InvalidJson, got {other:?}"),
        }
    }

    #[test]
    fn reference_cycles_are_detected_with_a_chain() {
        let sources = WorldgenSources {
            density_functions: BTreeMap::from([
                noise(
                    "minecraft/a",
                    r#"{"type":"minecraft:add","argument1":"minecraft:b","argument2":0}"#,
                ),
                noise(
                    "minecraft/b",
                    r#"{"type":"minecraft:add","argument1":"minecraft:a","argument2":0}"#,
                ),
            ]),
            noise_settings: BTreeMap::from([noise(
                "minecraft/overworld",
                r#"{"noise_router":{"final_density":"minecraft:a"}}"#,
            )]),
            noises: BTreeMap::new(),
        };
        match prepare_worldgen(&sources, "minecraft:overworld") {
            Err(PrepareError::ReferenceCycle { chain }) => {
                assert!(chain.len() >= 2);
                assert_eq!(chain.first(), chain.last());
            }
            other => panic!("expected ReferenceCycle, got {other:?}"),
        }
    }

    #[test]
    fn unresolved_shape_rejects_prose_and_path_escapes() {
        assert!(looks_namespaced("minecraft:overworld/caves/entrances"));
        assert!(looks_namespaced("sc:custom_thing.v2"));
        assert!(!looks_namespaced("not a reference"));
        assert!(!looks_namespaced("minecraft:"));
        assert!(!looks_namespaced(":path"));
        assert!(!looks_namespaced("minecraft:/escaped"));
        assert!(!looks_namespaced("minecraft:a//b"));
        assert!(!looks_namespaced("Minecraft:Upper"));
    }

    #[test]
    fn prepared_worldgen_round_trips_through_json() {
        let sources = WorldgenSources {
            noise_settings: BTreeMap::from([noise("minecraft/overworld", "{}")]),
            ..Default::default()
        };
        let prepared = prepare_worldgen(&sources, "minecraft:overworld").unwrap();
        let bytes = serde_json::to_vec(&prepared).unwrap();
        let parsed: PreparedWorldgen = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed, prepared);
    }
}
