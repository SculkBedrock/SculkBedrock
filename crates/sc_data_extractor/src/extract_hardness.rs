//! Extracts hardness (`split-blocks --hardness` input).
//!
//! Source: `Block.getHardness()` (base default 10; `-1` means unbreakable, matching the
//! `getHardness() != -1` breakable semantics).
//! Classes without an override inherit along the `extends` chain (explicit walk with Java semantics;
//! off-chain superclasses, multiple overrides, and non-literal bodies are loudly skipped, never guessed).
//! Hardness is a per-type static value written to base `components` (`sc:mining` /
//! `sc:unbreakable`, mutually exclusive by schema).
//!
//! Outputs flat JSON (`{identifier: {"hardness": h} | {"unbreakable": true}}`, consumed directly
//! by split) plus a source sidecar (identifier to class file, for auditing).
//!
//! Shares the Java scanning base (paren matching/top-level splitting) in
//! [`crate::extract_defaults`] (same crate, no external deps).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sc_packloader::block::{BlockHardness, LegacyPaletteEntry};

use crate::extract_defaults::{find_matching, split_top};

/// Hardness extraction report (`hardness` holds only statically decidable entries).
#[derive(Clone, Debug, Default)]
pub struct HardnessReport {
    pub hardness: BTreeMap<String, BlockHardness>,
    /// Per-entry source: the class file declaring the hardness (relative to the java root).
    pub provenance: BTreeMap<String, String>,
    pub classes: usize,
    pub with_override: usize,
    pub inherited: usize,
    pub unbreakable: usize,
    /// Non-literal bodies (e.g. state-dependent ternaries): skipped.
    pub non_literal: Vec<String>,
    /// Unresolvable superclass chains: skipped.
    pub unknown_chain: Vec<String>,
    /// Class identifiers outside the palette: skipped.
    pub java_missing: Vec<String>,
    /// Palette types without hardness declarations: keeps components empty.
    pub no_hardness: Vec<String>,
    pub hard_errors: usize,
}

impl HardnessReport {
    pub fn is_shippable(&self) -> bool {
        self.hard_errors == 0
    }
}

pub fn run(args: &[String]) {
    let mut pnx: Option<&str> = None;
    let mut palette: Option<&str> = None;
    let mut out: Option<&str> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--pnx" => {
                i += 1;
                pnx = args.get(i).map(String::as_str);
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
    let (Some(pnx), Some(palette), Some(out)) = (pnx, palette, out) else {
        eprintln!("usage: extract-hardness --pnx <src/main/java> --palette <block_palette.nbt> --out <hardness.json>");
        std::process::exit(2);
    };
    let raw = fs::read(palette).unwrap_or_else(|e| {
        eprintln!("failed to read palette {palette}: {e}");
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
    let entries = sc_packloader::block::parse_legacy_palette_entries(&palette_bytes)
        .unwrap_or_else(|e| {
            eprintln!("legacy palette parse failed: {e}");
            std::process::exit(1);
        });
    let report = extract_hardness_from_entries(Path::new(pnx), &entries);
    eprintln!(
        "[extract-hardness] classes={} self-declared={} inherited={} unbreakable={} non-literal={} unknown-chain={} java-missing-id={} palette-without-hardness={} hard-errors={}",
        report.classes,
        report.with_override,
        report.inherited,
        report.unbreakable,
        report.non_literal.len(),
        report.unknown_chain.len(),
        report.java_missing.len(),
        report.no_hardness.len(),
        report.hard_errors,
    );
    if !report.non_literal.is_empty() {
        eprintln!(
            "[extract-hardness] non-literal (state-dependent, skipped): {:?}",
            report.non_literal
        );
    }
    if !report.unknown_chain.is_empty() {
        eprintln!(
            "[extract-hardness] unknown superclass chain (skipped): {:?}",
            report.unknown_chain
        );
    }
    if !report.is_shippable() {
        eprintln!("[extract-hardness] extraction failed (hard errors, hardness not written)");
        std::process::exit(1);
    }
    let flat: BTreeMap<&String, serde_json::Value> = report
        .hardness
        .iter()
        .map(|(k, h)| {
            let v = match h {
                BlockHardness::Breakable(f) => {
                    let mut m = serde_json::Map::new();
                    m.insert(
                        "hardness".to_string(),
                        serde_json::Number::from_f64(*f as f64)
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null),
                    );
                    serde_json::Value::Object(m)
                }
                BlockHardness::Unbreakable => {
                    let mut m = serde_json::Map::new();
                    m.insert("unbreakable".to_string(), serde_json::Value::Bool(true));
                    serde_json::Value::Object(m)
                }
            };
            (k, v)
        })
        .collect();
    if flat.values().any(|v| v.is_null()) {
        eprintln!("[extract-hardness] hardness not representable as JSON number, refusing to write");
        std::process::exit(1);
    }
    let json = serde_json::to_string_pretty(&flat).unwrap_or_else(|e| {
        eprintln!("hardness JSON serialize failed: {e}");
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
    let prov_path = format!("{out}.provenance.json");
    let prov_json = serde_json::to_string_pretty(&report.provenance).unwrap_or_default();
    fs::write(&prov_path, prov_json.as_bytes()).unwrap_or_else(|e| {
        eprintln!("failed to write provenance {prov_path}: {e}");
        std::process::exit(1);
    });
    eprintln!(
        "hardness written {} ({} entries, provenance in {})",
        out,
        report.hardness.len(),
        prov_path
    );
}

/// Parses a method body as a literal f64 (single `return <num>;` statement; comments tolerated outside).
fn parse_literal_body(body: &str) -> Option<f64> {
    // Strips line comments.
    let mut code = String::new();
    for line in body.lines() {
        let line = match line.find("//") {
            Some(pos) => &line[..pos],
            None => line,
        };
        code.push_str(line);
        code.push(' ');
    }
    let code = code.trim();
    let ret = code.strip_prefix("return")?.trim();
    let num = ret.strip_suffix(';')?.trim();
    // Accepts only decimal literals (with d/D/f/F suffixes, no expressions).
    let num = num.trim_end_matches(['d', 'D', 'f', 'F']);
    if num.is_empty() {
        return None;
    }
    let mut chars = num.chars().peekable();
    if chars.peek() == Some(&'-') {
        chars.next();
    }
    let mut digits = 0usize;
    let mut dots = 0usize;
    for c in chars {
        if c.is_ascii_digit() {
            digits += 1;
        } else if c == '.' {
            dots += 1;
        } else {
            return None;
        }
    }
    if digits == 0 || dots > 1 {
        return None;
    }
    num.parse::<f64>().ok()
}

fn is_ident_char(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'$')
}

/// First top-level `class X [extends Y]` in a file.
fn class_decl(text: &str) -> Option<(String, Option<String>)> {
    let mut search = 0usize;
    while let Some(pos) = text[search..].find("class ") {
        let abs = search + pos;
        // Excludes `xxxclass ` prefixes (conservatively checks the preceding char).
        if abs > 0 && is_ident_char(text.as_bytes()[abs - 1]) {
            search = abs + 1;
            continue;
        }
        let mut i = abs + "class ".len();
        let bytes = text.as_bytes();
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && is_ident_char(bytes[i]) {
            i += 1;
        }
        if start == i {
            search = abs + 1;
            continue;
        }
        let name = text[start..i].to_string();
        let rest = text[i..].trim_start();
        let superclass = rest.strip_prefix("extends").and_then(|r| {
            // `extends` must be followed by whitespace (excludes extendsFoo-like tokens).
            if r.starts_with(|c: char| c == ' ' || c == '\t' || c == '\n' || c == '\r') {
                let r = r.trim_start();
                let mut j = 0usize;
                while j < r.len() && is_ident_char(r.as_bytes()[j]) {
                    j += 1;
                }
                if j > 0 {
                    Some(r[..j].to_string())
                } else {
                    None
                }
            } else {
                None
            }
        });
        return Some((name, superclass));
    }
    None
}

fn push_unique(list: &mut Vec<String>, msg: String) {
    if !list.contains(&msg) {
        list.push(msg);
    }
}

pub fn extract_hardness_from_entries(
    java_root: &Path,
    entries: &[LegacyPaletteEntry],
) -> HardnessReport {
    let mut report = HardnessReport::default();
    let block_dir = java_root.join("org/powernukkitx/block");
    // Class name to file (duplicates are hard errors).
    let mut class_files: HashMap<String, PathBuf> = HashMap::new();
    let mut dup_classes = Vec::new();
    let mut java_files: Vec<PathBuf> = Vec::new();
    let mut seen_paths: std::collections::HashSet<PathBuf> = Default::default();
    let mut dirs = vec![block_dir.clone()];
    while let Some(dir) = dirs.pop() {
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "property") {
                    continue;
                }
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "java") {
                if seen_paths.insert(path.clone()) {
                    java_files.push(path);
                }
            }
        }
    }
    java_files.sort();
    // BlockID map.
    let mut id_map: HashMap<String, String> = HashMap::new();
    if let Ok(id_text) = fs::read_to_string(block_dir.join("BlockID.java")) {
        for line in id_text.lines() {
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
    }
    struct FileInfo {
        path: PathBuf,
        class: String,
        superclass: Option<String>,
        own: Option<f64>,
        own_is_literal: bool,
    }
    let mut files_info: Vec<FileInfo> = Vec::new();
    for path in java_files {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Some((class, superclass)) = class_decl(&text) else {
            continue;
        };
        if let Some(prev) = class_files.insert(class.clone(), path.clone()) {
            dup_classes.push(format!(
                "{class} ({} vs {})",
                prev.display(),
                path.display()
            ));
            continue;
        }
        // Own override (at most one per file; more is loud).
        let mut found = Vec::new();
        let mut search = 0usize;
        while let Some(pos) = text[search..].find("getHardness") {
            let abs = search + pos;
            search = abs + 1;
            // Confirms the `double getHardness()` method definition.
            let head_start = abs.saturating_sub(16);
            if !text[head_start..abs].trim_end().ends_with("double") {
                continue;
            }
            let paren = text[abs..].find('(').map(|p| abs + p);
            let Some(paren) = paren else {
                continue;
            };
            let Some(close_paren) = find_matching(&text, paren) else {
                continue;
            };
            let after = text[close_paren + 1..].trim_start();
            if !after.starts_with('{') {
                continue;
            }
            let brace = close_paren + 1 + (text[close_paren + 1..].len() - after.len());
            let Some(close_brace) = find_matching(&text, brace) else {
                continue;
            };
            found.push(text[brace + 1..close_brace].to_string());
        }
        let (own, own_is_literal) = match found.len() {
            0 => (None, true),
            1 => match parse_literal_body(&found[0]) {
                Some(v) => (Some(v), true),
                None => (None, false),
            },
            _ => {
                report.non_literal.push(format!(
                    "{}: getHardness at {} places in one file",
                    path.display(),
                    found.len()
                ));
                (None, false)
            }
        };
        if found.len() == 1 && !own_is_literal {
            report
                .non_literal
                .push(format!("{}: non-literal body", path.display()));
        }
        files_info.push(FileInfo {
            path,
            class,
            superclass,
            own,
            own_is_literal,
        });
    }
    if !dup_classes.is_empty() {
        report.hard_errors += dup_classes.len();
        for d in dup_classes {
            eprintln!("[extract-hardness] duplicate class name: {d}");
        }
    }
    report.classes = files_info.len();
    // Effective hardness: own literal, otherwise inherited along the chain.
    let class_of: HashMap<&str, usize> = files_info
        .iter()
        .enumerate()
        .map(|(i, f)| (f.class.as_str(), i))
        .collect();
    // File to identifiers (second lightweight scan, avoids borrow tangles).
    let file_idents: Vec<Vec<String>> = files_info
        .iter()
        .map(|f| {
            let Ok(text) = fs::read_to_string(&f.path) else {
                return Vec::new();
            };
            let mut ids = Vec::new();
            let mut psearch = 0usize;
            while let Some(pos) = text[psearch..].find("new BlockProperties(") {
                let abs = psearch + pos;
                let open = abs + "new BlockProperties(".len() - 1;
                psearch = abs + 1;
                let Some(close) = find_matching(&text, open) else {
                    continue;
                };
                let call_args = split_top(&text[open + 1..close]);
                if call_args.is_empty() {
                    continue;
                }
                let id_expr = call_args[0].trim();
                let resolved = match id_expr.split_once('.') {
                    None => id_map.get(id_expr).cloned(),
                    Some(("BlockID", c)) => id_map.get(c).cloned(),
                    Some(_) => None,
                };
                if let Some(id) = resolved {
                    ids.push(id);
                }
            }
            ids
        })
        .collect();
    let mut palette_names: std::collections::BTreeSet<&str> = Default::default();
    for e in entries.iter() {
        palette_names.insert(e.name.as_str());
    }
    // Inherited class set: chain reports cover only identified or inherited files (pure helper classes
    // stay silently skipped; skipping itself still applies, just without noise).
    let mut subclassed: std::collections::HashSet<&str> = Default::default();
    for f in files_info.iter() {
        if let Some(sup) = f.superclass.as_deref() {
            subclassed.insert(sup);
        }
    }
    for (fi, info) in files_info.iter().enumerate() {
        let noisy = file_idents[fi].is_empty() && !subclassed.contains(info.class.as_str());
        // Resolves the effective value.
        let mut current: Option<&FileInfo> = Some(info);
        let mut visited: Vec<&str> = Vec::new();
        let mut effective: Option<f64> = None;
        let mut inherited = false;
        let mut failed = false;
        while let Some(cur) = current {
            if visited.contains(&cur.class.as_str()) {
                if !noisy {
                    push_unique(
                        &mut report.unknown_chain,
                        format!("{}: inheritance cycle", cur.path.display()),
                    );
                }
                failed = true;
                break;
            }
            visited.push(cur.class.as_str());
            if let Some(v) = cur.own {
                effective = Some(v);
                break;
            }
            if !cur.own_is_literal {
                // Own non-literal: this class has no static hardness (state-dependent), so stops climbing;
                // subclasses without their own literal are likewise undecidable (Java semantics would dispatch here).
                // Conservatively treats the whole chain as not statically decidable.
                failed = true;
                break;
            }
            match cur.superclass.as_deref() {
                None => {
                    // No superclass declaration: the chain top takes the base default 10 (only when confirmed as
                    // the Block family root; otherwise unknown).
                    if cur.class == "Block" {
                        effective = Some(10.0);
                    } else {
                        if !noisy {
                            push_unique(
                                &mut report.unknown_chain,
                                format!(
                                    "{}: class {} has no superclass declaration and no own hardness",
                                    cur.path.display(),
                                    cur.class
                                ),
                            );
                        }
                        failed = true;
                    }
                    break;
                }
                Some(sup) => match class_of.get(sup) {
                    Some(&idx) => {
                        inherited = true;
                        current = Some(&files_info[idx]);
                    }
                    None => {
                        if sup == "Block" {
                            // Block.java was not scanned (should not happen): takes the base default.
                            effective = Some(10.0);
                        } else {
                            if !noisy {
                                push_unique(
                                    &mut report.unknown_chain,
                                    format!("{}: superclass {sup} source not found", cur.path.display()),
                                );
                            }
                            failed = true;
                        }
                        break;
                    }
                },
            }
        }
        if failed {
            continue;
        }
        let Some(value) = effective else {
            continue;
        };
        if info.own.is_some() {
            report.with_override += 1;
        } else if inherited {
            report.inherited += 1;
        }
        let hardness = if value == -1.0 {
            report.unbreakable += 1;
            BlockHardness::Unbreakable
        } else {
            BlockHardness::Breakable(value as f32)
        };
        for id in file_idents[fi].iter() {
            if !palette_names.contains(id.as_str()) {
                if !report.java_missing.contains(id) {
                    report.java_missing.push(id.clone());
                }
                continue;
            }
            report.hardness.insert(id.clone(), hardness.clone());
            report.provenance.insert(
                id.clone(),
                info.path
                    .strip_prefix(java_root)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|_| info.path.display().to_string()),
            );
        }
    }
    // Types without hardness declarations in the palette.
    {
        let mut names: std::collections::BTreeSet<&str> = Default::default();
        for e in entries.iter() {
            names.insert(e.name.as_str());
        }
        for name in names {
            if !report.hardness.contains_key(name) {
                report.no_hardness.push(name.to_string());
            }
        }
    }
    report
}
