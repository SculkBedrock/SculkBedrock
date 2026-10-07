//! `colorize_literal!`: compile-time section-code to ANSI conversion.
//!
//! Zero-dependency proc-macro: takes one string literal, emits the converted
//! string literal (ANSI escapes as `\u{1b}`). Zero runtime cost;
//! conversion happens entirely at compile time.
//!
//! ```ignore
//! use sc_log::colorize_literal;
//! let s: &'static str = colorize_literal!("§aHello world!");
//! assert_eq!(s, "\u{1b}[92mHello world!");
//! ```

use proc_macro::{TokenStream, TokenTree};

/// Converts section codes in a literal to ANSI escapes (at compile time).
#[proc_macro]
pub fn colorize_literal(input: TokenStream) -> TokenStream {
    let literal = match input.into_iter().next() {
        Some(TokenTree::Literal(literal)) => literal,
        other => panic!(
            "colorize_literal! takes a single string literal, got: {:?}",
            other.map(|t| t.to_string())
        ),
    };
    let source = literal.to_string();
    let value = unescape_string_literal(&source)
        .unwrap_or_else(|e| panic!("colorize_literal! cannot parse string literal {source:?}: {e}"));
    let colored = convert(&value);
    let escaped = escape_literal(&colored);
    format!("\"{escaped}\"")
        .parse()
        .expect("generated string literal is invalid")
}

// ---- Unit-testable pure functions below (kept in sync with the sc_log::color runtime table) ----

/// Section code to ANSI (code char lowercased before lookup; matches sc_log::color::COLOR_TABLE).
#[inline]
fn ansi_for_code(code: u8) -> Option<&'static str> {
    Some(match code.to_ascii_lowercase() {
        b'0' => "\x1b[30m",
        b'1' => "\x1b[34m",
        b'2' => "\x1b[32m",
        b'3' => "\x1b[36m",
        b'4' => "\x1b[31m",
        b'5' => "\x1b[35m",
        b'6' => "\x1b[33m",
        b'7' => "\x1b[37m",
        b'8' => "\x1b[90m",
        b'9' => "\x1b[94m",
        b'a' => "\x1b[92m",
        b'b' => "\x1b[96m",
        b'c' => "\x1b[91m",
        b'd' => "\x1b[95m",
        b'e' => "\x1b[93m",
        b'f' => "\x1b[97m",
        b'l' => "\x1b[1m",
        b'o' => "\x1b[3m",
        b'n' => "\x1b[4m",
        b'm' => "\x1b[9m",
        b'k' => "\x1b[5m",
        b'r' => "\x1b[0m",
        _ => return None,
    })
}

/// Converts section codes to ANSI; unknown/lone codes stay verbatim.
fn convert(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c != '§' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(code) => match ansi_for_code(code as u8) {
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

/// Restores actual string content from Rust literal source (quotes and escapes included).
fn unescape_string_literal(source: &str) -> Result<String, String> {
    if source.starts_with('r') {
        return Err("raw string literals are not supported, use a plain string".into());
    }
    let body = source
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(|| "expected a double-quoted string literal".to_string())?;
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let esc = chars.next().ok_or_else(|| "trailing backslash".to_string())?;
        match esc {
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '0' => out.push('\0'),
            'x' => {
                let hi = chars.next().and_then(|c| c.to_digit(16));
                let lo = chars.next().and_then(|c| c.to_digit(16));
                let (Some(hi), Some(lo)) = (hi, lo) else {
                    return Err("\\x escape needs two hex digits".into());
                };
                out.push(((hi << 4) | lo) as u8 as char);
            }
            'u' => {
                if chars.next() != Some('{') {
                    return Err("\\u escape needs {..}".into());
                }
                let mut digits = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    digits.push(c);
                }
                let value = u32::from_str_radix(&digits, 16)
                    .map_err(|_| "\\u escape has illegal hex".to_string())?;
                let c =
                    char::from_u32(value).ok_or_else(|| "\\u escape out of Unicode range".to_string())?;
                out.push(c);
            }
            other => return Err(format!("unsupported escape: \\{other}")),
        }
    }
    Ok(out)
}

/// Escapes a string as Rust literal body (ESC as `\u{1b}`, control chars as `\u{..}`).
fn escape_literal(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    for c in input.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => {
                out.push_str(&format!("\\u{{{:x}}}", c as u32));
            }
            // Section marks and other non-ASCII stay verbatim (the literal itself is valid UTF-8).
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_basic() {
        assert_eq!(convert("§aHello"), "\u{1b}[92mHello");
        assert_eq!(convert("§cRed§r"), "\u{1b}[91mRed\u{1b}[0m");
        assert_eq!(convert("plain"), "plain");
    }

    #[test]
    fn convert_unknown_and_lone() {
        assert_eq!(convert("§x"), "§x");
        assert_eq!(convert("end§"), "end§");
    }

    #[test]
    fn unescape_roundtrip() {
        assert_eq!(
            unescape_string_literal("\"a\\n\\t\\u{1b}[92mb\"").unwrap(),
            "a\n\t\u{1b}[92mb"
        );
        assert_eq!(unescape_string_literal("\"\\\\\\\"\"").unwrap(), "\\\"");
        assert!(unescape_string_literal("not a literal").is_err());
    }

    #[test]
    fn escape_roundtrip() {
        assert_eq!(escape_literal("\u{1b}[92m"), "\\u{1b}[92m");
        assert_eq!(escape_literal("a\"b\\c"), "a\\\"b\\\\c");
    }

    #[test]
    fn full_pipeline_matches_runtime_table() {
        let value = unescape_string_literal("\"§a绿§r白\"").unwrap();
        let escaped = escape_literal(&convert(&value));
        assert_eq!(escaped, "\\u{1b}[92m绿\\u{1b}[0m白");
    }
}
