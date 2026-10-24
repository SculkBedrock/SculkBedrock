//! 从 Java 方块源码中提取方块默认状态，输出 `getDefaultState` 溯源报告；
//! 方块 JSON 格式不携带 `sc:default_state`，输出仅作参考。
//!
//! Extracts block default states from Java block sources, producing a
//! `getDefaultState` provenance report; the block JSON format does not carry
//! `sc:default_state`, so the output is informational only.
//!
//! 每个方块的默认组合必须命中调色板中恰好一个状态，否则报错
//! `combo_not_in_palette`：不猜测，也不回退到首个状态。
//!
//! Every block's default combination must match exactly one state in the
//! palette; otherwise `combo_not_in_palette` is reported. There is no
//! guessing and no first-state fallback.
//!
//! 两趟处理：有 Java 方块类的从类属性列表组合；没有 Java 类的在属性全部
//! 已声明时直接从属性声明组合。每个默认值的来源记录在
//! `ExtractReport.provenance`。
//!
//! Two passes: blocks with Java block classes combine from the class
//! property lists; blocks without Java classes combine directly from the
//! property declarations when all their properties are declared.
//! Each default's origin is recorded in `ExtractReport.provenance`.
//!
//! 属性声明有三种形式：布尔（序列化为 byte）、整数范围、枚举（序列化为
//! 小写常量名）。枚举默认值取首个值或具名常量；方块属性列表引用共享
//! 属性持有者，裸名经静态导入解析。
//!
//! Property declarations come in three forms: booleans (serialized as byte),
//! int ranges, and enums (serialized as the lowercase constant name). Enum
//! defaults are either the first value or a named constant; block property
//! lists reference shared property holders, with bare names resolved through
//! static imports.
//!
//! 输出为 `{identifier: palette_key}`（与
//! [`split_state_key`](sc_packloader::block::split_state_key) 同一键规则）。
//!
//! Output is `{identifier: palette_key}` (same key rule as
//! [`split_state_key`](sc_packloader::block::split_state_key)).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use sc_nbt::compound::CompoundNbt;

use sc_nbt::NbtValue;
use sc_packloader::block::{parse_legacy_palette_entries, split_state_key, LegacyPaletteEntry};

const CB_PROPS: &str = "org.powernukkitx.block.property.CommonBlockProperties";
// Class path of the block-id source file read by the extraction passes.
#[allow(dead_code)]
const BLOCK_ID: &str = "org.powernukkitx.block.BlockID";

/// 属性默认值（内部形式；按 NBT 精确匹配校验）。
///
/// Property default (internal form; checked by exact NBT match).
#[derive(Clone, Debug, PartialEq, Eq)]
enum PropDefault {
    /// 布尔默认值（字节 0/1）。
    ///
    /// Boolean default (byte 0/1).
    Byte(i8),
    /// 整数默认值。
    ///
    /// Integer default.
    Int(i32),
    /// 枚举默认值（小写常量名）。
    ///
    /// Enum default (lowercase constant name).
    Str(String),
}

impl PropDefault {
    /// 按 NBT 值精确匹配（类型与值都需一致）。
    ///
    /// Matches by exact NBT value (type and value must both match).
    fn matches_nbt(&self, v: &NbtValue) -> bool {
        match (self, v) {
            (PropDefault::Byte(a), NbtValue::Byte(b)) => a == b,
            (PropDefault::Int(a), NbtValue::Int(b)) => a == b,
            (PropDefault::Str(a), NbtValue::String(b)) => a == b,
            _ => false,
        }
    }

    /// 序列化为默认值的文本形式（用于报告输出）。
    ///
    /// Serializes the default to its text form (for report output).
    fn repr(&self) -> String {
        match self {
            PropDefault::Byte(b) => b.to_string(),
            PropDefault::Int(i) => i.to_string(),
            PropDefault::Str(s) => s.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Java text scan (string/char/comment-aware bracket matching, no dependencies).
// ---------------------------------------------------------------------------

/// 返回 `text[open]` 处括号（`(`/`{`）的匹配下标（跳过注释/字符串）。
///
/// Index of the bracket matching `text[open]` (`(`/`{`), skipping
/// comments/strings.
pub(crate) fn find_matching(text: &str, open: usize) -> Option<usize> {
    let b = text.as_bytes();
    if open >= b.len() {
        return None;
    }
    let (op, cl) = match b[open] {
        b'(' => (b'(', b')'),
        b'{' => (b'{', b'}'),
        _ => return None,
    };
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                    } else if b[i] == b'"' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            b'\'' => {
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                    } else if b[i] == b'\'' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            c if c == op => {
                depth += 1;
                i += 1;
            }
            c if c == cl => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    None
}

/// 按顶层 `,` 切分（感知括号/字符串/注释）。
///
/// Split on top-level `,` (bracket/string/comment aware).
pub(crate) fn split_top(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut depth_paren = 0usize;
    let mut depth_brace = 0usize;
    let mut depth_brack = 0usize;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                    } else if b[i] == b'"' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            b'\'' => {
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                    } else if b[i] == b'\'' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            b'(' => {
                depth_paren += 1;
                i += 1;
            }
            b')' => {
                depth_paren = depth_paren.saturating_sub(1);
                i += 1;
            }
            b'{' => {
                depth_brace += 1;
                i += 1;
            }
            b'}' => {
                depth_brace = depth_brace.saturating_sub(1);
                i += 1;
            }
            b'[' => {
                depth_brack += 1;
                i += 1;
            }
            b']' => {
                depth_brack = depth_brack.saturating_sub(1);
                i += 1;
            }
            b',' if depth_paren == 0 && depth_brace == 0 && depth_brack == 0 => {
                out.push(text[start..i].trim());
                i += 1;
                start = i;
            }
            _ => {
                i += 1;
            }
        }
    }
    out.push(text[start..].trim());
    out
}

/// Java 标识符字符（字母/数字/`_`/`$`）。
///
/// Java identifier character (letter/digit/`_`/`$`).
fn is_ident_char(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'$')
}

/// 取 `text[..end]` 末尾的标识符（字段/常量回溯）。
///
/// Trailing identifier of `text[..end]` (field/constant backtrack).
fn trailing_ident(text: &str, end: usize) -> Option<String> {
    let b = text.as_bytes();
    let mut i = end;
    while i > 0 && (b[i - 1] == b' ' || b[i - 1] == b'\t' || b[i - 1] == b'\n' || b[i - 1] == b'\r')
    {
        i -= 1;
    }
    if i == 0 || b[i - 1] != b'=' {
        return None;
    }
    i -= 1;
    while i > 0 && (b[i - 1] == b' ' || b[i - 1] == b'\t' || b[i - 1] == b'\n' || b[i - 1] == b'\r')
    {
        i -= 1;
    }
    let mut j = i;
    while j > 0 && is_ident_char(b[j - 1]) {
        j -= 1;
    }
    if j == i {
        return None;
    }
    Some(text[j..i].to_string())
}

/// 取枚举体中顶层 `;` 之前的常量名（剥离注解）。
///
/// Constant names before the top-level `;` of an enum body (annotations
/// stripped).
fn enum_constant_name(segment: &str) -> Option<String> {
    let mut s = segment.trim();
    while let Some(rest) = s.strip_prefix('@') {
        // Skip annotation names and optional parameter lists.
        let mut i = 0usize;
        while i < rest.len() && is_ident_char(rest.as_bytes()[i]) {
            i += 1;
        }
        s = rest[i..].trim_start();
        if let Some(tail) = s.strip_prefix('(') {
            let _ = tail;
            // Annotation parameters are rare; abandon the constant on sight (caller reports loudly).
            return None;
        }
    }
    let mut i = 0usize;
    while i < s.len() && is_ident_char(s.as_bytes()[i]) {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    Some(s[..i].to_string())
}

// ---------------------------------------------------------------------------
// Main flow.
// ---------------------------------------------------------------------------

/// 属性声明：属性名加默认值。
///
/// Property declaration: property name plus default value.
struct PropDecl {
    prop_name: String,
    default: PropDefault,
}

/// CLI 入口：解析参数（`--java-src`/`--palette`/`--out`）并写出结果。
///
/// CLI entry point: parses arguments (`--java-src`/`--palette`/`--out`) and
/// writes the results.
pub fn run(args: &[String]) {
    let mut java_src: Option<&str> = None;
    let mut palette: Option<&str> = None;
    let mut out: Option<&str> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--java-src" => {
                i += 1;
                java_src = args.get(i).map(String::as_str);
            }
            "--palette" => {
                i += 1;
                palette = args.get(i).map(String::as_str);
            }
            "--out" => {
                i += 1;
                out = args.get(i).map(String::as_str);
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let (Some(java_src), Some(palette), Some(out)) = (java_src, palette, out) else {
        eprintln!("usage: extract-defaults --java-src <src/main/java> --palette <block_palette.nbt> --out <defaults.json>");
        std::process::exit(2);
    };
    match extract(Path::new(java_src), Path::new(palette)) {
        Ok(report) => {
            let json = serde_json::to_string_pretty(&report.defaults).unwrap_or_else(|e| {
                eprintln!("defaults JSON serialize failed: {e}");
                std::process::exit(1);
            });
            if let Some(parent) = Path::new(out).parent() {
                fs::create_dir_all(parent).unwrap_or_else(|e| {
                    eprintln!("failed to create dir: {e}");
                    std::process::exit(1);
                });
            }
            fs::write(out, json.as_bytes()).unwrap_or_else(|e| {
                eprintln!("failed to write {out}: {e}");
                std::process::exit(1);
            });
            // Provenance sidecar lives next to the flat file (audit only).
            let prov_path = format!("{out}.provenance.json");
            let prov_json = serde_json::to_string_pretty(&report.provenance).unwrap_or_default();
            fs::write(&prov_path, prov_json.as_bytes()).unwrap_or_else(|e| {
                eprintln!("failed to write provenance {prov_path}: {e}");
                std::process::exit(1);
            });
            eprintln!(
                "defaults written {} ({} entries, provenance in {})",
                out,
                report.defaults.len(),
                prov_path
            );
        }
        Err(failures) => {
            eprintln!("extraction failed ({failures} hard class errors, defaults not written):");
            std::process::exit(1);
        }
    }
}

/// 读取整个文件为字符串（失败即退出进程）。
///
/// Reads a whole file as a string (exits the process on failure).
fn read_file(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("failed to read {}: {e}", path.display());
        std::process::exit(1);
    })
}

/// 类名到候选源文件（显式 import 优先）。
///
/// Class name to candidate source files (explicit imports first).
fn resolve_class(
    java_root: &Path,
    imports: &[String],
    own_pkg_dir: &Path,
    class: &str,
) -> Option<PathBuf> {
    for import in imports {
        if import.ends_with(&format!(".{class}")) {
            let rel = import.replace('.', "/") + ".java";
            let p = java_root.join(rel);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let same = own_pkg_dir.join(format!("{class}.java"));
    if same.is_file() {
        return Some(same);
    }
    for import in imports {
        if let Some(pkg) = import.strip_suffix(".*") {
            let p = java_root
                .join(pkg.replace('.', "/"))
                .join(format!("{class}.java"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// 收集文件中的 import（静态导入加 `static ` 前缀）。
///
/// Collects a file's imports (static imports get a `static ` prefix).
fn file_imports(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("import static ") {
            out.push(format!("static {}", rest.trim_end_matches(';').trim()));
        } else if let Some(rest) = line.strip_prefix("import ") {
            out.push(rest.trim_end_matches(';').trim().to_string());
        }
    }
    out
}

/// 枚举常量（声明序）到序列化用小写名。
///
/// Enum constants (declaration order) to serialized lowercase names.
fn parse_enum_constants(
    java_root: &Path,
    imports: &[String],
    own_pkg_dir: &Path,
    class: &str,
) -> Result<Vec<String>, String> {
    // Nested classes resolve through the outer class file.
    let (outer, inner) = match class.split_once('.') {
        Some((o, i)) => (o, Some(i)),
        None => (class, None),
    };
    let path = resolve_class(java_root, imports, own_pkg_dir, outer)
        .ok_or_else(|| format!("enum class {class} source not found"))?;
    let text = read_file(&path);
    let mut body_start = None;
    if let Some(inner) = inner {
        // Locate `enum Inner`.
        let mut search = 0usize;
        while let Some(pos) = text[search..].find(&format!("enum {inner}")) {
            let abs = search + pos;
            // Must be a standalone word.
            let before = text[..abs].chars().last();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                search = abs + 1;
                continue;
            }
            let brace = text[abs..]
                .find('{')
                .map(|p| abs + p)
                .ok_or_else(|| format!("inner class {inner} enum body not found ({})", path.display()))?;
            body_start = Some(brace);
            break;
        }
    } else {
        let mut search = 0usize;
        while let Some(pos) = text[search..].find(&format!("enum {class}")) {
            let abs = search + pos;
            let before = text[..abs].chars().last();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                search = abs + 1;
                continue;
            }
            let brace = text[abs..]
                .find('{')
                .map(|p| abs + p)
                .ok_or_else(|| format!("enum {class} body not found ({})", path.display()))?;
            body_start = Some(brace);
            break;
        }
    }
    let brace =
        body_start.ok_or_else(|| format!("enum {class} declaration not found ({})", path.display()))?;
    let close = find_matching(&text, brace).ok_or_else(|| format!("enum {class} brace mismatch"))?;
    let body = &text[brace + 1..close];
    // The constant zone ends at the top-level `;`.
    let mut depth = 0usize;
    let mut end = body.len();
    let mut idx = 0usize;
    let bb = body.as_bytes();
    while idx < bb.len() {
        match bb[idx] {
            b'(' | b'{' | b'[' => {
                depth += 1;
                idx += 1;
            }
            b')' | b'}' | b']' => {
                depth = depth.saturating_sub(1);
                idx += 1;
            }
            b';' if depth == 0 => {
                end = idx;
                break;
            }
            b'"' => {
                idx += 1;
                while idx < bb.len() {
                    if bb[idx] == b'\\' {
                        idx += 2;
                    } else if bb[idx] == b'"' {
                        idx += 1;
                        break;
                    } else {
                        idx += 1;
                    }
                }
            }
            _ => {
                idx += 1;
            }
        }
    }
    let mut constants = Vec::new();
    for seg in split_top(&body[..end]) {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        match enum_constant_name(seg) {
            Some(name) => constants.push(name.to_lowercase()),
            None => return Err(format!("enum {class} constant parse failed: {seg:?}")),
        }
    }
    if constants.is_empty() {
        return Err(format!("enum {class} has no constants"));
    }
    Ok(constants)
}

/// 在状态组中精确命中默认组合，返回其下标。
///
/// Exact-hit the default combination in a state group; return its index.
fn find_exact_match(
    states: &[&LegacyPaletteEntry],
    defaults: &[(String, PropDefault)],
    empty_states: &CompoundNbt,
) -> Vec<usize> {
    let mut matched = Vec::new();
    for (idx, entry) in states.iter().enumerate() {
        let map = entry.states.as_ref().unwrap_or(empty_states);
        let mut keys: Vec<&String> = map.iter().map(|(k, _)| k).collect();
        keys.sort();
        let mut want: Vec<&String> = defaults.iter().map(|(k, _)| k).collect();
        want.sort();
        if keys != want {
            continue;
        }
        if defaults
            .iter()
            .all(|(k, d)| map.get(k).is_some_and(|v| d.matches_nbt(v)))
        {
            matched.push(idx);
        }
    }
    matched
}

/// 状态键遵循 split 规则。
///
/// State keys follow the split rule.
fn split_key_for(
    states: &[&LegacyPaletteEntry],
    matched_idx: usize,
    empty_states: &CompoundNbt,
) -> String {
    let mut union: BTreeSet<&str> = BTreeSet::new();
    for entry in states.iter() {
        let map = entry.states.as_ref().unwrap_or(empty_states);
        for (k, _) in map.iter() {
            union.insert(k.as_str());
        }
    }
    let matched_entry = states[matched_idx];
    let single: Option<(String, String)> = if union.len() == 1 {
        let map = matched_entry.states.as_ref().unwrap_or(empty_states);
        map.iter().next().map(|(k, v)| {
            let repr = match v {
                NbtValue::Byte(b) => b.to_string(),
                NbtValue::Int(i) => i.to_string(),
                NbtValue::String(s) => s.clone(),
                other => format!("TAG{}", other.tag()),
            };
            (k.clone(), repr)
        })
    } else {
        None
    };
    split_state_key(union.len(), single, matched_idx)
}

/// 提取报告（CLI 与回归测试共用）。
///
/// Extraction report (CLI and regression tests share it).
#[derive(Clone, Debug, Default)]
pub struct ExtractReport {
    pub defaults: BTreeMap<String, String>,
    /// 默认值来源：类声明或按声明组合（无 Java 类的方块按声明组合）。
    ///
    /// Default origin: class declaration or per-declaration composition
    /// (blocks without Java classes compose from declarations).
    pub provenance: BTreeMap<String, String>,
    pub declarations: usize,
    pub ok: usize,
    pub combo_miss: Vec<String>,
    pub dup_canon: Vec<String>,
    pub java_missing: Vec<String>,
    pub no_java: Vec<String>,
    pub hard_errors: usize,
}

impl ExtractReport {
    /// 可交付（无硬错误、无组合缺失、无重复规格）。
    ///
    /// Shippable (no hard errors, no misses, no duplicate specs).
    pub fn is_shippable(&self) -> bool {
        self.hard_errors == 0 && self.combo_miss.is_empty() && self.dup_canon.is_empty()
    }
}

/// 读取调色板（支持 gzip）并执行提取；报告不可交付时以错误码退出。
///
/// Reads the palette (gzip supported) and runs extraction; exits with an
/// error status when the report is not shippable.
fn extract(java_root: &Path, palette_path: &Path) -> Result<ExtractReport, usize> {
    let raw = fs::read(palette_path).unwrap_or_else(|e| {
        eprintln!("failed to read palette {}: {e}", palette_path.display());
        std::process::exit(1);
    });
    let palette_bytes = if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        let mut dec = Vec::new();
        flate2::read::GzDecoder::new(&raw[..])
            .read_to_end(&mut dec)
            .unwrap_or_else(|e| {
                eprintln!("gzip decompress failed: {e}");
                std::process::exit(1);
            });
        dec
    } else {
        raw
    };
    let entries = parse_legacy_palette_entries(&palette_bytes).unwrap_or_else(|e| {
        eprintln!("legacy palette parse failed: {e}");
        std::process::exit(1);
    });
    let report = extract_from_entries(java_root, &entries);
    eprintln!(
        "[extract-defaults] ok={} combo-miss={} dup-spec={} java-missing-id={} palette-without-java={} hard-errors={}",
        report.ok,
        report.combo_miss.len(),
        report.dup_canon.len(),
        report.java_missing.len(),
        report.no_java.len(),
        report.hard_errors,
    );
    if !report.is_shippable() {
        Err(1)
    } else {
        Ok(report)
    }
}

/// 从已解析的调色板条目提取默认状态（核心流程）。
///
/// Extracts default states from parsed palette entries (core flow).
pub fn extract_from_entries(java_root: &Path, entries: &[LegacyPaletteEntry]) -> ExtractReport {
    let mut report = ExtractReport::default();
    let mut hard_errors = 0usize;
    // ---- 1. Property declarations ----
    let props_path = java_root.join("org/powernukkitx/block/property/CommonBlockProperties.java");
    let props_text = read_file(&props_path);
    let props_pkg_dir = java_root.join("org/powernukkitx/block/property");
    let props_imports = file_imports(&props_text);
    let mut decls: HashMap<String, PropDecl> = HashMap::new();
    // Property name to default (conflicting duplicates are hard errors).
    let mut prop_defaults: HashMap<String, PropDefault> = HashMap::new();
    for (factory, kind) in [
        ("BooleanPropertyType.of(", "bool"),
        ("IntPropertyType.of(", "int"),
        ("EnumPropertyType.of(", "enum"),
    ] {
        let mut search = 0usize;
        while let Some(pos) = props_text[search..].find(factory) {
            let abs = search + pos;
            let field = trailing_ident(&props_text, abs);
            let open = abs + factory.len() - 1;
            let close = find_matching(&props_text, open);
            search = abs + 1;
            let (Some(field), Some(close)) = (field, close) else {
                eprintln!("[extract-defaults] property declaration parse failed ({factory} @ {abs})");
                hard_errors += 1;
                continue;
            };
            let call_args = split_top(&props_text[open + 1..close]);
            let name = call_args
                .first()
                .map(|s| s.trim().trim_matches('"').to_string())
                .unwrap_or_default();
            let parsed: Option<(PropDefault, Option<String>)> = match kind {
                "bool" => match call_args.get(1).map(|s| s.trim()) {
                    Some("false") => Some((PropDefault::Byte(0), None)),
                    Some("true") => Some((PropDefault::Byte(1), None)),
                    other => {
                        eprintln!("[extract-defaults] non-literal bool default: {name} = {other:?}");
                        hard_errors += 1;
                        None
                    }
                },
                "int" => match call_args.get(3).map(|s| s.trim().parse::<i32>()) {
                    Some(Ok(n)) => Some((PropDefault::Int(n), None)),
                    _ => {
                        eprintln!("[extract-defaults] illegal int default: {name}");
                        hard_errors += 1;
                        None
                    }
                },
                _ => {
                    let cls = call_args
                        .get(1)
                        .map(|s| s.trim().trim_end_matches(".class").trim().to_string())
                        .unwrap_or_default();
                    let cls_simple = cls.rsplit('.').next().unwrap_or(&cls).to_string();
                    let def_expr = call_args
                        .get(2)
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default();
                    // Forms Cls.values()[N] or Cls.CONST.
                    let constant: Option<String> = if let Some(idx) = def_expr.find(".values()[") {
                        let expr_cls = def_expr[..idx].rsplit('.').next().unwrap_or("").to_string();
                        let num = def_expr[idx + ".values()[".len()..].trim_end_matches(']');
                        if expr_cls != cls_simple {
                            eprintln!("[extract-defaults] enum default class mismatch: {name} ({expr_cls} vs {cls_simple})");
                            hard_errors += 1;
                            None
                        } else {
                            match num.parse::<usize>() {
                                Ok(n) => {
                                    match parse_enum_constants(java_root, &props_imports, &props_pkg_dir, &cls) {
                                        Ok(list) => list.get(n).cloned().or_else(|| {
                                            eprintln!("[extract-defaults] enum index out of range: {name} [{n}] ({} total)", list.len());
                                            hard_errors += 1;
                                            None
                                        }),
                                        Err(m) => {
                                            eprintln!("[extract-defaults] {m}");
                                            hard_errors += 1;
                                            None
                                        }
                                    }
                                }
                                Err(_) => {
                                    eprintln!(
                                        "[extract-defaults] illegal enum index: {name} = {def_expr:?}"
                                    );
                                    hard_errors += 1;
                                    None
                                }
                            }
                        }
                    } else if let Some(dot) = def_expr.rfind('.') {
                        let expr_cls = def_expr[..dot].rsplit('.').next().unwrap_or("");
                        let constant = def_expr[dot + 1..].to_string();
                        if expr_cls != cls_simple
                            || !constant
                                .chars()
                                .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
                        {
                            eprintln!("[extract-defaults] illegal enum constant form: {name} = {def_expr:?}");
                            hard_errors += 1;
                            None
                        } else {
                            // The constant must exist.
                            match parse_enum_constants(
                                java_root,
                                &props_imports,
                                &props_pkg_dir,
                                &cls,
                            ) {
                                Ok(list) => {
                                    let lowered = constant.to_lowercase();
                                    if list.contains(&lowered) {
                                        Some(lowered)
                                    } else {
                                        eprintln!("[extract-defaults] enum constant missing: {name} = {constant}");
                                        hard_errors += 1;
                                        None
                                    }
                                }
                                Err(m) => {
                                    eprintln!("[extract-defaults] {m}");
                                    hard_errors += 1;
                                    None
                                }
                            }
                        }
                    } else {
                        eprintln!("[extract-defaults] unknown enum default form: {name} = {def_expr:?}");
                        hard_errors += 1;
                        None
                    };
                    constant.map(|c| (PropDefault::Str(c), Some(cls)))
                }
            };
            if let Some((default, _)) = parsed {
                if decls.contains_key(&field) {
                    eprintln!("[extract-defaults] duplicate property constant: {field}");
                    hard_errors += 1;
                } else {
                    match prop_defaults.entry(name.clone()) {
                        std::collections::hash_map::Entry::Occupied(e) => {
                            if *e.get() != default {
                                eprintln!("[extract-defaults] same-name property default conflict: {name}");
                                hard_errors += 1;
                            }
                        }
                        std::collections::hash_map::Entry::Vacant(e) => {
                            e.insert(default.clone());
                        }
                    }
                    decls.insert(
                        field,
                        PropDecl {
                            prop_name: name,
                            default,
                        },
                    );
                }
            }
        }
    }
    eprintln!("[extract-defaults] {} property declarations", decls.len());

    // ---- 2. BlockID ----
    let block_id_text = read_file(&java_root.join("org/powernukkitx/block/BlockID.java"));
    let mut id_map: HashMap<String, String> = HashMap::new();
    for line in block_id_text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("String ") else {
            continue;
        };
        let Some(eq) = rest.find('=') else {
            continue;
        };
        let constant = rest[..eq].trim();
        let value = rest[eq + 1..]
            .trim()
            .trim_end_matches(';')
            .trim()
            .trim_matches('"');
        if !constant.is_empty() && value.starts_with("minecraft:") {
            id_map.insert(constant.to_string(), value.to_string());
        }
    }

    // ---- 3. Per-block PROPERTIES ----
    let mut block_defaults: BTreeMap<String, Vec<(String, PropDefault)>> = BTreeMap::new();
    let mut dup_blocks = 0usize;
    let block_dir = java_root.join("org/powernukkitx/block");
    let mut block_files = Vec::new();
    let mut dirs = vec![block_dir.clone()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Property-declaration subtrees are not block declarations.
                if path.file_name().is_some_and(|n| n == "property") {
                    continue;
                }
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "java") {
                block_files.push(path);
            }
        }
    }
    block_files.sort();
    for path in block_files {
        let text = read_file(&path);
        let imports = file_imports(&text);
        let uses_cb_wildcard = imports.iter().any(|i| i == &format!("static {CB_PROPS}.*"));
        let mut search = 0usize;
        while let Some(pos) = text[search..].find("new BlockProperties(") {
            let abs = search + pos;
            let open = abs + "new BlockProperties(".len() - 1;
            search = abs + 1;
            let Some(close) = find_matching(&text, open) else {
                eprintln!(
                    "[extract-defaults] BlockProperties brace mismatch ({})",
                    path.display()
                );
                hard_errors += 1;
                continue;
            };
            let call_args = split_top(&text[open + 1..close]);
            if call_args.is_empty() {
                continue;
            }
            // First parameter: identifier constant.
            let id_expr = call_args[0].trim();
            let (id_qual, id_const) = match id_expr.split_once('.') {
                Some((q, c)) => (Some(q), c),
                None => (None, id_expr),
            };
            // First parameter: identifier constant. Block classes implement
            // the BlockID interface; bare constants resolve globally.
            let identifier: Option<String> = match id_qual {
                None => id_map.get(id_const).cloned(),
                Some("BlockID") => id_map.get(id_const).cloned(),
                Some(_) => None,
            };
            let Some(identifier) = identifier else {
                eprintln!(
                    "[extract-defaults] identifier constant unresolvable: {id_expr} ({})",
                    path.display()
                );
                hard_errors += 1;
                continue;
            };
            // Remaining parameters: property references. `Set.of(...)` tag
            // sets contribute no properties and are skipped.
            let mut props = Vec::new();
            let mut bad = false;
            for arg in call_args.iter().skip(1) {
                let arg = arg.trim();
                if arg.starts_with("Set.of(") {
                    continue;
                }
                let prop_const: Option<&str> = match arg.split_once('.') {
                    Some((q, c)) if q == "CommonBlockProperties" => Some(c),
                    Some(_) => None,
                    None => {
                        if uses_cb_wildcard
                            || imports
                                .iter()
                                .any(|i| i == &format!("static {CB_PROPS}.{arg}"))
                        {
                            Some(arg)
                        } else {
                            None
                        }
                    }
                };
                match prop_const.and_then(|c| decls.get(c)) {
                    Some(decl) => props.push((decl.prop_name.clone(), decl.default.clone())),
                    None => {
                        eprintln!(
                            "[extract-defaults] property reference unresolvable: {arg} ({identifier}, {})",
                            path.display()
                        );
                        hard_errors += 1;
                        bad = true;
                        break;
                    }
                }
            }
            if bad {
                continue;
            }
            if block_defaults.insert(identifier.clone(), props).is_some() {
                eprintln!("[extract-defaults] duplicate identifier declaration: {identifier}");
                dup_blocks += 1;
            }
        }
    }
    eprintln!(
        "[extract-defaults] {} block default declarations ({dup_blocks} duplicates)",
        block_defaults.len()
    );
    report.declarations = block_defaults.len();

    // ---- 4. Palette check ----
    let mut grouped: BTreeMap<&str, Vec<&LegacyPaletteEntry>> = BTreeMap::new();
    for e in entries.iter() {
        grouped.entry(e.name.as_str()).or_default().push(e);
    }
    let empty_states = sc_nbt::compound::CompoundNbt::new(None);
    for (identifier, defaults) in block_defaults.iter() {
        let Some(states) = grouped.get(identifier.as_str()) else {
            report.java_missing.push(identifier.clone());
            continue;
        };
        let matched = find_exact_match(states, defaults, &empty_states);
        if matched.len() != 1 {
            if matched.is_empty() {
                report.combo_miss.push(identifier.clone());
                let combo: Vec<String> = defaults
                    .iter()
                    .map(|(k, d)| format!("{k}={}", d.repr()))
                    .collect();
                eprintln!("[extract-defaults] default combo missing from palette: {identifier} (expected {combo:?}, palette has {} states)", states.len());
            } else {
                report.dup_canon.push(identifier.clone());
                eprintln!(
                    "[extract-defaults] palette has duplicate-spec states: {identifier} ({} hits)",
                    matched.len()
                );
            }
            continue;
        }
        report.defaults.insert(
            identifier.clone(),
            split_key_for(states, matched[0], &empty_states),
        );
        report
            .provenance
            .insert(identifier.clone(), "class".to_string());
        report.ok += 1;
    }
    // ---- 5. Declaration composition for blocks without Java classes ----
    //
    // Defaults compose per declaration; for blocks without Java classes,
    // combine directly when all properties are declared.
    // Undeclared properties are never guessed.
    let mut composed = 0usize;
    for (identifier, states) in grouped.iter() {
        if report.defaults.contains_key(*identifier) {
            continue;
        }
        if states.len() <= 1 {
            continue;
        }
        let mut union: BTreeSet<&str> = BTreeSet::new();
        for entry in states.iter() {
            let map = entry.states.as_ref().unwrap_or(&empty_states);
            for (k, _) in map.iter() {
                union.insert(k.as_str());
            }
        }
        let mut combo: Vec<(String, PropDefault)> = Vec::new();
        let mut unknown_prop = false;
        for prop in union.iter() {
            match prop_defaults.get(*prop) {
                Some(d) => combo.push((prop.to_string(), d.clone())),
                None => {
                    unknown_prop = true;
                    break;
                }
            }
        }
        if unknown_prop {
            continue;
        }
        let matched = find_exact_match(states, &combo, &empty_states);
        if matched.len() != 1 {
            if matched.is_empty() {
                report.combo_miss.push(identifier.to_string());
                let combo_str: Vec<String> = combo
                    .iter()
                    .map(|(k, d)| format!("{k}={}", d.repr()))
                    .collect();
                eprintln!("[extract-defaults] composed default missing from palette: {identifier} (expected {combo_str:?})");
            } else {
                report.dup_canon.push(identifier.to_string());
                eprintln!(
                    "[extract-defaults] palette has duplicate-spec states: {identifier} ({} hits)",
                    matched.len()
                );
            }
            continue;
        }
        report.defaults.insert(
            identifier.to_string(),
            split_key_for(states, matched[0], &empty_states),
        );
        report
            .provenance
            .insert(identifier.to_string(), "composed".to_string());
        report.ok += 1;
        composed += 1;
    }
    eprintln!("[extract-defaults] composed {composed} from declarations");
    report.no_java = grouped
        .keys()
        .filter(|k| !block_defaults.contains_key(**k))
        .map(|s| s.to_string())
        .collect();
    report.hard_errors = hard_errors;
    if !report.no_java.is_empty() {
        let mut sample: Vec<String> = report.no_java.iter().take(10).cloned().collect();
        sample.sort();
        eprintln!("[extract-defaults] palette types without java declarations: {sample:?} (awaiting authoritative data)");
    }
    report
}
