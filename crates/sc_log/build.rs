//! build.rs: pre-render i18n section-sign color codes at build time.
//!
//! Reads `locales/*.yml`, converting each message's section codes to ANSI
//! (colored table) or stripping them (plain table), and emits the static
//! `$OUT_DIR/colored_locales.rs` table into the binary. Runtime `t!()`
//! lookups need zero conversion and zero allocation.
//!
//! Note: this section-code table is the third copy alongside sc_log::color
//! and sc_log_macros (build.rs cannot reference the crate itself); each
//! copy has alignment tests.

use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let locales_dir = Path::new(&manifest).join("locales");
    println!("cargo:rerun-if-changed=locales");

    let mut locales: Vec<(String, Vec<(String, String)>)> = Vec::new();
    if let Ok(read_dir) = fs::read_dir(&locales_dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yml") {
                continue;
            }
            let locale = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let content = fs::read_to_string(&path).unwrap_or_default();
            let entries = parse_yaml(&content);
            locales.push((locale, entries));
        }
    }
    if locales.is_empty() {
        println!(
            "cargo:warning=sc_log build.rs: locales directory is empty, pre-colored table is empty"
        );
    }

    let out_dir = env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir).join("colored_locales.rs");
    fs::write(&out_path, generate(&locales)).unwrap();
}

// ---------------- Mini YAML parser (flat format used by this repo) ----------------
// Format: `key: "value"` (double quotes) or `key: value`.

fn parse_yaml(content: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(idx) = line.find(':') else { continue };
        let key = line[..idx].trim();
        if key.is_empty() || key == "_version" {
            continue;
        }
        let value_raw = line[idx + 1..].trim();
        let value = parse_scalar(value_raw);
        entries.push((key.to_string(), value));
    }
    entries
}

fn parse_scalar(raw: &str) -> String {
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        unescape_double_quoted(&raw[1..raw.len() - 1])
    } else {
        raw.to_string()
    }
}

fn unescape_double_quoted(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('u') => {
                let mut digits = String::new();
                if chars.next() == Some('{') {
                    for c in chars.by_ref() {
                        if c == '}' {
                            break;
                        }
                        digits.push(c);
                    }
                }
                if let Ok(value) = u32::from_str_radix(&digits, 16) {
                    if let Some(c) = char::from_u32(value) {
                        out.push(c);
                        continue;
                    }
                }
                // Parse failure: keep the raw `\u{..}`
                out.push_str("\\u{");
                out.push_str(&digits);
                out.push('}');
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

// ---------------- Section-code conversion (matches sc_log::color / sc_log_macros) ----------------

fn ansi_for_code(code: char) -> Option<&'static str> {
    Some(match code.to_ascii_lowercase() {
        '0' => "\x1b[30m",
        '1' => "\x1b[34m",
        '2' => "\x1b[32m",
        '3' => "\x1b[36m",
        '4' => "\x1b[31m",
        '5' => "\x1b[35m",
        '6' => "\x1b[33m",
        '7' => "\x1b[37m",
        '8' => "\x1b[90m",
        '9' => "\x1b[94m",
        'a' => "\x1b[92m",
        'b' => "\x1b[96m",
        'c' => "\x1b[91m",
        'd' => "\x1b[95m",
        'e' => "\x1b[93m",
        'f' => "\x1b[97m",
        'l' => "\x1b[1m",
        'o' => "\x1b[3m",
        'n' => "\x1b[4m",
        'm' => "\x1b[9m",
        'k' => "\x1b[5m",
        'r' => "\x1b[0m",
        _ => return None,
    })
}

/// Section code to ANSI (unknown/lone section marks kept verbatim).
fn to_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c != '§' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(code) => match ansi_for_code(code) {
                Some(ansi) => out.push_str(ansi),
                None => {
                    out.push('§');
                    out.push(code);
                }
            },
            None => out.push('§'),
        }
    }
    out
}

/// Strip all section codes.
fn strip_codes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

// ---------------- Code generation ----------------

fn generate(locales: &[(String, Vec<(String, String)>)]) -> String {
    let mut code = String::with_capacity(4096);
    code.push_str("// Generated by sc_log/build.rs at build time: section codes\n");
    code.push_str("// from locales/*.yml are ANSI (COLORED_LOCALES) or stripped\n");
    code.push_str("// (PLAIN_LOCALES). Do not edit by hand.\n\n");

    code.push_str("pub static COLORED_LOCALES: &[(&str, &[(&str, &str)])] = &[\n");
    for (locale, entries) in locales {
        code.push_str(&format!("    ({:?}, &[\n", locale));
        for (key, value) in entries {
            code.push_str(&format!("        ({:?}, {:?}),\n", key, to_ansi(value)));
        }
        code.push_str("    ]),\n");
    }
    code.push_str("];\n\n");

    code.push_str("pub static PLAIN_LOCALES: &[(&str, &[(&str, &str)])] = &[\n");
    for (locale, entries) in locales {
        code.push_str(&format!("    ({:?}, &[\n", locale));
        for (key, value) in entries {
            code.push_str(&format!("        ({:?}, {:?}),\n", key, strip_codes(value)));
        }
        code.push_str("    ]),\n");
    }
    code.push_str("];\n");
    code
}
