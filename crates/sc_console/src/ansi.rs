//! Terminal text utilities: `§` codes to ANSI, ANSI stripping, display width, and truncation.
//!
//! Self-contained (no `sc_log` dependency, keeping this crate independent); semantics match
//! `sc_log::color`: unknown/lone `§` sequences are kept verbatim.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SECTION: char = '§';

/// Convert `§` codes to ANSI. Returns a borrow when there are no codes (zero-alloc).
pub fn minecraft_to_ansi(input: &str) -> std::borrow::Cow<'_, str> {
    if !input.contains(SECTION) {
        return std::borrow::Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len() + 16);
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c != SECTION {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(code) => match ansi_for_code(code) {
                Some(ansi) => out.push_str(ansi),
                None => {
                    out.push(SECTION);
                    out.push(code);
                }
            },
            None => out.push(SECTION),
        }
    }
    std::borrow::Cow::Owned(out)
}

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
        'r' => "\x1b[0m",
        _ => return None,
    })
}

/// Strip ANSI CSI escapes (UTF-8 safe).
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Strip `§` format codes (unknown codes kept verbatim).
pub fn strip_minecraft_codes(input: &str) -> String {
    if !input.contains(SECTION) {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c != SECTION {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(code) if code.is_ascii() && ansi_for_code(code).is_some() => {}
            Some(code) => {
                out.push(SECTION);
                out.push(code);
            }
            None => out.push(SECTION),
        }
    }
    out
}

/// Plain text (ANSI first, then `§`): for file logs and width computation.
pub fn plain_text(input: &str) -> String {
    strip_minecraft_codes(&strip_ansi(input))
}

/// Visible display width (after stripping ANSI/`§`, via `unicode-width`; CJK counts as 2).
pub fn display_width(input: &str) -> usize {
    UnicodeWidthStr::width(plain_text(input).as_str())
}

/// Whether the current environment supports ANSI styling (off with `NO_COLOR` / `TERM=dumb`).
///
/// Cached in a `OnceLock` and probed once; TUI background/highlight relies on it, layout preserved when off.
pub fn style_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        match std::env::var("TERM") {
            Ok(term) => !term.eq_ignore_ascii_case("dumb"),
            Err(_) => true,
        }
    })
}

/// Truncate by display width while preserving color styles:
///
/// - Only ANSI SGR styles pass through (zero-width); cursor/clear/OSC terminal controls are dropped;
/// - `§` codes convert to ANSI live (zero-width);
/// - Unknown/lone `§` kept verbatim and counted toward width;
/// - Append `\x1b[0m` at the end (only when a style was emitted) to stop color leaking into borders.
pub fn truncate_styled(input: &str, max_width: usize) -> String {
    let mut out = String::with_capacity(input.len().min(max_width * 4 + 16));
    let mut width = 0usize;
    let mut styled = false;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                let mut params = String::new();
                for c2 in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c2) {
                        if c2 == 'm'
                            && params
                                .chars()
                                .all(|p| p.is_ascii_digit() || p == ';' || p == ':')
                        {
                            out.push_str("\x1b[");
                            out.push_str(&params);
                            out.push('m');
                            styled = true;
                        }
                        break;
                    }
                    params.push(c2);
                }
            } else if matches!(chars.peek(), Some(']' | 'P' | 'X' | '^' | '_')) {
                chars.next();
                while let Some(c2) = chars.next() {
                    if c2 == '\u{7}' {
                        break;
                    }
                    if c2 == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            } else {
                // Two-byte ESC commands (e.g. save/restore cursor) must not cross log boundaries either.
                if chars.peek().is_some_and(|c| c.is_ascii()) {
                    chars.next();
                }
            }
            continue;
        }
        if c.is_control() {
            continue;
        }
        if c == SECTION {
            match chars.next() {
                Some(code) => match ansi_for_code(code) {
                    Some(ansi) => {
                        out.push_str(ansi);
                        styled = true;
                    }
                    None => {
                        let w = 1 + UnicodeWidthChar::width(code).unwrap_or(0);
                        if width + w > max_width {
                            break;
                        }
                        out.push(SECTION);
                        out.push(code);
                        width += w;
                    }
                },
                None => {
                    if width == max_width {
                        break;
                    }
                    out.push(SECTION);
                    width += 1;
                }
            }
            continue;
        }
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if width + w > max_width {
            break;
        }
        out.push(c);
        width += w;
    }
    if styled {
        out.push_str("\x1b[0m");
    }
    out
}

/// Split a styled string into display rows by display width (for word wrap):
/// - Same parsing rules as [`truncate_styled`] (SGR passes through zero-width, OSC/other controls dropped, known `§` to ANSI, unknown `§` counted verbatim, no split inside CJK);
/// - Greedy fill: wrap when a row is full; an over-wide single char (CJK in a narrow window) takes its own overflowing row to guarantee termination;
/// - Re-emit active SGR at each new row start (assumes colors do not leak across rows); no trailing `\x1b[0m` (caller appends it);
/// - Always returns at least one row (empty string yields one empty row).
pub fn split_styled_rows(input: &str, width: usize) -> Vec<String> {
    fn break_row(rows: &mut Vec<String>, col: &mut usize, active: &str) {
        rows.push(active.to_string());
        *col = 0;
    }
    let width = width.max(1);
    let mut rows: Vec<String> = vec![String::new()];
    let mut col = 0usize;
    let mut active = String::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                let mut params = String::new();
                for c2 in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c2) {
                        if c2 == 'm'
                            && params
                                .chars()
                                .all(|p| p.is_ascii_digit() || p == ';' || p == ':')
                        {
                            let seq = format!("\x1b[{params}m");
                            if params.is_empty() || params == "0" {
                                active.clear();
                            } else {
                                active.push_str(&seq);
                            }
                            rows.last_mut().expect("rows").push_str(&seq);
                        }
                        break;
                    }
                    params.push(c2);
                }
            } else if matches!(chars.peek(), Some(']' | 'P' | 'X' | '^' | '_')) {
                chars.next();
                while let Some(c2) = chars.next() {
                    if c2 == '\u{7}' {
                        break;
                    }
                    if c2 == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            } else {
                // Two-byte ESC commands (e.g. save/restore cursor) must not cross log boundaries either.
                if chars.peek().is_some_and(|c| c.is_ascii()) {
                    chars.next();
                }
            }
            continue;
        }
        if c.is_control() {
            continue;
        }
        if c == SECTION {
            match chars.next() {
                Some(code) => match ansi_for_code(code) {
                    Some(ansi) => {
                        let seq = ansi.to_string();
                        if code == 'r' || code == 'R' {
                            active.clear();
                        } else {
                            active.push_str(&seq);
                        }
                        rows.last_mut().expect("rows").push_str(&seq);
                    }
                    None => {
                        // Unknown `§x` kept verbatim: `§` takes 1 column, code takes its real width, wrapping in pairs.
                        let w = 1 + UnicodeWidthChar::width(code).unwrap_or(0);
                        if col > 0 && col + w > width {
                            break_row(&mut rows, &mut col, &active);
                        }
                        let row = rows.last_mut().expect("rows");
                        row.push(SECTION);
                        row.push(code);
                        col += w;
                    }
                },
                None => {
                    if col > 0 && col + 1 > width {
                        break_row(&mut rows, &mut col, &active);
                    }
                    rows.last_mut().expect("rows").push(SECTION);
                    col += 1;
                }
            }
            continue;
        }
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if col > 0 && col + w > width {
            break_row(&mut rows, &mut col, &active);
        }
        rows.last_mut().expect("rows").push(c);
        col += w;
    }
    rows
}

/// Take plain text for columns `[x0, x1)` by display width (CJK-boundary safe: a straddled wide char belongs to the right side).
pub fn slice_by_width(input: &str, x0: usize, x1: usize) -> String {
    if x0 >= x1 {
        return String::new();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for c in input.chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if width >= x1 {
            break;
        }
        if width >= x0 {
            out.push(c);
        }
        width += w;
    }
    out
}

/// Split a styled string into `(before, mid, after)` segments by display width:
///
/// - `mid` holds display columns `[x0, x1)` (styles preserved; a straddled wide char belongs to the right segment);
/// - `mid`/`after` re-emit the SGR active at the cut point (assumes colors do not leak across segments; used for selection highlight);
/// - Only ANSI SGR passes through (zero-width); other control sequences are dropped as in `truncate_styled`.
pub fn split_styled_range(input: &str, x0: usize, x1: usize) -> (String, String, String) {
    let mut before = String::new();
    let mut mid = String::new();
    let mut after = String::new();
    if x0 >= x1 {
        return (input.to_string(), String::new(), String::new());
    }
    // SGR active at the cut point (`[0m` clears; re-emitted at the start of `mid`/`after`).
    let mut active = String::new();
    let mut mid_prefixed = false;
    let mut after_prefixed = false;
    let mut width = 0usize;
    // Route one visible char (ownership by start column `pos`; zero-width SGR passes empty text plus current position).
    // Pass state explicitly without capturing mutables to avoid closure borrow conflicts.
    let route = |c: char,
                 pos: usize,
                 before: &mut String,
                 mid: &mut String,
                 after: &mut String,
                 active: &str,
                 mid_prefixed: &mut bool,
                 after_prefixed: &mut bool| {
        if pos < x0 {
            before.push(c);
        } else if pos < x1 {
            if !*mid_prefixed {
                mid.push_str(active);
                *mid_prefixed = true;
            }
            mid.push(c);
        } else {
            if !*after_prefixed {
                after.push_str(active);
                *after_prefixed = true;
            }
            after.push(c);
        }
    };
    // Route one zero-width SGR run (into the segment owning the given column).
    let route_sgr = |seq: &str,
                     pos: usize,
                     before: &mut String,
                     mid: &mut String,
                     after: &mut String,
                     active: &str,
                     mid_prefixed: &mut bool,
                     after_prefixed: &mut bool| {
        if pos < x0 {
            before.push_str(seq);
        } else if pos < x1 {
            if !*mid_prefixed {
                mid.push_str(active);
                *mid_prefixed = true;
            }
            mid.push_str(seq);
        } else {
            if !*after_prefixed {
                after.push_str(active);
                *after_prefixed = true;
            }
            after.push_str(seq);
        }
    };
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                let mut params = String::new();
                for c2 in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c2) {
                        if c2 == 'm'
                            && params
                                .chars()
                                .all(|p| p.is_ascii_digit() || p == ';' || p == ':')
                        {
                            let seq = format!("\x1b[{params}m");
                            if params.is_empty() || params == "0" {
                                active.clear();
                            } else {
                                active.push_str(&seq);
                            }
                            route_sgr(
                                &seq,
                                width,
                                &mut before,
                                &mut mid,
                                &mut after,
                                &active,
                                &mut mid_prefixed,
                                &mut after_prefixed,
                            );
                        }
                        break;
                    }
                    params.push(c2);
                }
            } else if matches!(chars.peek(), Some(']' | 'P' | 'X' | '^' | '_')) {
                chars.next();
                while let Some(c2) = chars.next() {
                    if c2 == '\u{7}' {
                        break;
                    }
                    if c2 == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            } else if chars.peek().is_some_and(|c| c.is_ascii()) {
                chars.next();
            }
            continue;
        }
        if c.is_control() {
            continue;
        }
        if c == SECTION {
            match chars.next() {
                Some(code) => match ansi_for_code(code) {
                    Some(ansi) => {
                        let seq = ansi.to_string();
                        if code == 'r' || code == 'R' {
                            active.clear();
                        } else {
                            active.push_str(&seq);
                        }
                        route_sgr(
                            &seq,
                            width,
                            &mut before,
                            &mut mid,
                            &mut after,
                            &active,
                            &mut mid_prefixed,
                            &mut after_prefixed,
                        );
                    }
                    None => {
                        // Unknown `§x` kept verbatim: `§` takes 1 column, code takes its real width.
                        let w = UnicodeWidthChar::width(code).unwrap_or(0);
                        route(
                            '§',
                            width,
                            &mut before,
                            &mut mid,
                            &mut after,
                            &active,
                            &mut mid_prefixed,
                            &mut after_prefixed,
                        );
                        route(
                            code,
                            width + 1,
                            &mut before,
                            &mut mid,
                            &mut after,
                            &active,
                            &mut mid_prefixed,
                            &mut after_prefixed,
                        );
                        width += 1 + w;
                        continue;
                    }
                },
                None => {
                    route(
                        '§',
                        width,
                        &mut before,
                        &mut mid,
                        &mut after,
                        &active,
                        &mut mid_prefixed,
                        &mut after_prefixed,
                    );
                    width += 1;
                }
            }
            continue;
        }
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        route(
            c,
            width,
            &mut before,
            &mut mid,
            &mut after,
            &active,
            &mut mid_prefixed,
            &mut after_prefixed,
        );
        width += w;
    }
    (before, mid, after)
}

/// Selection highlight: reverse-video columns `[x0, x1)` (`7`/`27`); re-assert reverse after an in-segment `0m` reset to keep the whole range.
/// Returned unchanged in non-styled environments.
pub fn highlight_range(input: &str, x0: usize, x1: usize, styled: bool) -> String {
    if !styled || x0 >= x1 {
        return input.to_string();
    }
    let (before, mid, after) = split_styled_range(input, x0, x1);
    if mid.is_empty() {
        return input.to_string();
    }
    let mid = mid.replace("\x1b[0m", "\x1b[0m\x1b[7m");
    format!("{before}\x1b[7m{mid}\x1b[27m{after}")
}

/// Truncate by display width (char-boundary safe, never splits CJK; no ellipsis, caller appends one as needed).
pub fn truncate_to_width(input: &str, max_width: usize) -> String {
    let plain = plain_text(input);
    if UnicodeWidthStr::width(plain.as_str()) <= max_width {
        return plain;
    }
    let mut out = String::new();
    let mut width = 0;
    for c in plain.chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if width + w > max_width {
            break;
        }
        out.push(c);
        width += w;
    }
    out
}

/// Exact RGB colors (truecolor output for foreground and background).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Map RGB to the nearest 256-color index (fallback when truecolor is unavailable; plain Euclidean distance with grays competing).
pub fn nearest_256(rgb: Rgb) -> u8 {
    const LEVELS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    fn snap(v: i32) -> (i32, i32) {
        let mut best = 0;
        let mut best_err = i32::MAX;
        for (i, &level) in LEVELS.iter().enumerate() {
            let err = (v - level) * (v - level);
            if err < best_err {
                best_err = err;
                best = i as i32;
            }
        }
        (best, LEVELS[best as usize])
    }
    let (ri, rv) = snap(rgb.0 as i32);
    let (gi, gv) = snap(rgb.1 as i32);
    let (bi, bv) = snap(rgb.2 as i32);
    let cube = 16 + 36 * ri + 6 * gi + bi;
    let cube_err =
        (rgb.0 as i32 - rv).pow(2) + (rgb.1 as i32 - gv).pow(2) + (rgb.2 as i32 - bv).pow(2);
    let avg = (rgb.0 as i32 + rgb.1 as i32 + rgb.2 as i32) / 3;
    let gray_idx = ((avg - 8) / 10).clamp(0, 23);
    let gray_val = 8 + gray_idx * 10;
    let gray_err = (rgb.0 as i32 - gray_val).pow(2)
        + (rgb.1 as i32 - gray_val).pow(2)
        + (rgb.2 as i32 - gray_val).pow(2);
    if gray_err < cube_err {
        (232 + gray_idx) as u8
    } else {
        cube as u8
    }
}

/// Write the foreground color: 24-bit truecolor, preserving animation gradient midpoints.
pub fn push_fg_rgb(buf: &mut String, rgb: Rgb, styled: bool) {
    if !styled {
        return;
    }
    buf.push_str(&format!("\x1b[38;2;{};{};{}m", rgb.0, rgb.1, rgb.2));
}

/// Write the background color: 24-bit truecolor, preserving continuous gradient midpoints.
pub fn push_bg_rgb(buf: &mut String, rgb: Rgb, styled: bool) {
    if !styled {
        return;
    }
    buf.push_str(&format!("\x1b[48;2;{};{};{}m", rgb.0, rgb.1, rgb.2));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_and_preserves_unknown() {
        assert_eq!(minecraft_to_ansi("§a绿§r白"), "\x1b[92m绿\x1b[0m白");
        assert_eq!(minecraft_to_ansi("§x"), "§x");
        assert_eq!(minecraft_to_ansi("100§"), "100§");
    }

    #[test]
    fn strips_both_kinds() {
        assert_eq!(plain_text("\x1b[92mHi\x1b[0m §aOK"), "Hi OK");
    }

    #[test]
    fn cjk_width_counts_two() {
        assert_eq!(display_width("你好ab"), 6);
        assert_eq!(truncate_to_width("你好世界ab", 5), "你好");
        assert_eq!(truncate_to_width("abc", 10), "abc");
    }

    #[test]
    fn nearest_256_matches_hand_computed_values() {
        // User-supplied 14-color gradient maps to expected 256-color indices (fallback path matches hand computation).
        let cases: [((u8, u8, u8), u8); 14] = [
            ((0x4C, 0x9C, 0xB5), 73),
            ((0x4A, 0x95, 0xAD), 67),
            ((0x49, 0x8A, 0xA1), 67),
            ((0x42, 0x74, 0x86), 66),
            ((0x43, 0x76, 0x86), 66),
            ((0x40, 0x6A, 0x79), 60),
            ((0x3C, 0x61, 0x6C), 240),
            ((0x3A, 0x56, 0x60), 239),
            ((0x36, 0x4B, 0x54), 238),
            ((0x31, 0x43, 0x48), 237),
            ((0x2D, 0x39, 0x3D), 236),
            ((0x2A, 0x31, 0x31), 235),
            ((0x27, 0x28, 0x2A), 235),
            ((0x27, 0x26, 0x26), 235),
        ];
        for ((r, g, b), expected) in cases {
            assert_eq!(nearest_256(Rgb(r, g, b)), expected, "rgb({r},{g},{b})");
        }
    }

    #[test]
    fn nearest_256_exact_colors_roundtrip() {
        assert_eq!(nearest_256(Rgb(38, 38, 38)), 235);
        assert_eq!(nearest_256(Rgb(135, 255, 135)), 120);
        assert_eq!(nearest_256(Rgb(0, 0, 0)), 16);
    }

    #[test]
    fn background_preserves_truecolor_and_no_color_mode() {
        let mut buf = String::new();
        push_bg_rgb(&mut buf, Rgb(47, 123, 142), true);
        assert_eq!(buf, "\x1b[48;2;47;123;142m");
        push_bg_rgb(&mut buf, Rgb(1, 2, 3), false);
        assert_eq!(buf, "\x1b[48;2;47;123;142m");
    }

    #[test]
    fn foreground_preserves_truecolor_and_no_color_mode() {
        let mut buf = String::new();
        push_fg_rgb(&mut buf, Rgb(47, 123, 142), true);
        assert_eq!(buf, "\x1b[38;2;47;123;142m");
        push_fg_rgb(&mut buf, Rgb(1, 2, 3), false);
        assert_eq!(buf, "\x1b[38;2;47;123;142m");
    }

    #[test]
    fn styled_truncation_rejects_terminal_controls() {
        let out = truncate_styled(
            "\x1b[31mred\x1b[0m\x1b[2J\x1b[H\x1b]8;;https://example.com\x07link\x1b]8;;\x1b\\\x08\r\n",
            20,
        );
        assert_eq!(plain_text(&out), "redlink");
        assert!(out.contains("\x1b[31m"));
        assert!(!out.contains("\x1b[2J"));
        assert!(!out.contains("\x1b[H"));
        assert!(!out.contains("https://"));
        assert_eq!(truncate_styled("a§x", 2), "a");
        assert_eq!(truncate_styled("a§", 1), "a");
    }

    #[test]
    fn styled_truncation_keeps_colors_and_width() {
        // CJK width 2: budget 6 fits three wide chars (2+2+2) but not the trailing "a".
        let out = truncate_styled("§a你好世界ab", 6);
        assert!(out.starts_with("\x1b[92m"));
        assert!(out.ends_with("\x1b[0m"));
        assert_eq!(display_width(&out), 6);
        // Budget 5 only fits two wide chars (width 4); never split a wide char.
        assert_eq!(display_width(&truncate_styled("§a你好世界ab", 5)), 4);
    }

    #[test]
    fn styled_row_splitting_wraps_and_replays_styles() {
        // Plain text wraps greedily by column; never splits CJK.
        assert_eq!(split_styled_rows("abcdef", 4), vec!["abcd", "ef"]);
        assert_eq!(split_styled_rows("你好世界", 5), vec!["你好", "世界"]);
        assert_eq!(split_styled_rows("", 10), vec![""]);
        // An over-wide single char takes its own overflowing row (no infinite loop or dropped char in narrow windows).
        assert_eq!(split_styled_rows("好", 1), vec!["好"]);
        // SGR is zero-width and re-emitted on wrap: the second row keeps its color and plain concatenation is lossless.
        let rows = split_styled_rows("\x1b[92m你好世界农\x1b[0m", 5);
        assert_eq!(rows.len(), 3);
        assert!(rows[1].starts_with("\x1b[92m"), "换行重放颜色：{rows:?}");
        assert_eq!(
            rows.iter()
                .map(|r| plain_text(r))
                .collect::<Vec<_>>()
                .join(""),
            "你好世界农"
        );
        // Every row stays within the visible column budget.
        for row in &rows {
            assert!(display_width(row) <= 5, "{row:?}");
        }
    }

    #[test]
    fn styled_row_splitting_matches_truncation() {
        // Same parsing rules: first chunk plus style trailer equals truncate_styled (corpus covers ANSI/`§`/CJK/controls).
        let corpus = [
            "plain text here",
            "\x1b[31mred\x1b[0m and \x1b[1mbold",
            "§a你好世界ab§r尾",
            "a§x未知码b",
            "100§",
            "\x1b[2J\x1b[H\x08\r\nclean",
            "混合\x1b[93mmix§b色\x1b[0m彩",
        ];
        for input in corpus {
            // With width=1 and a leading wide char, wrap and truncate intentionally differ: wrapping must show the char
            // (on its own overflowing row) while truncation must cut it; only compare widths where behavior agrees.
            for width in [2, 3, 5, 8, 20] {
                let rows = split_styled_rows(input, width);
                assert!(!rows.is_empty(), "{input:?} 恒至少一行");
                let first = &rows[0];
                let mut expected = first.clone();
                if first.contains('\x1b') {
                    expected.push_str("\x1b[0m");
                }
                assert_eq!(
                    truncate_styled(input, width),
                    expected,
                    "{input:?} w={width}"
                );
                for row in &rows {
                    assert!(display_width(row) <= width, "{input:?} w={width} {row:?}");
                }
            }
        }
    }
}
