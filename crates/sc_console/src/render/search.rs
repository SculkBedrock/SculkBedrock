//! TUI rendering: search section (split from `render.rs`; re-exported at `crate::render`).

use super::*;

// ================= Log search =================

/// One search hit (display column range `[x0, x1)`; wide chars count 2, same columns as [`log_row_plain`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SearchMatch {
    /// Log global sequence (`log_evicted` plus deque index; naturally expires after ring eviction).
    pub seq: u64,
    pub x0: usize,
    pub x1: usize,
}

/// Search render context (assembled by the driver from the query and matches; rendering only reads it).
#[derive(Clone, Copy, Debug)]
pub(crate) struct SearchCtx<'a> {
    /// All matches (bounded; see the driver cap; rendering filters by row).
    pub matches: &'a [SearchMatch],
    /// Current match index (0 with no highlight when empty; clamped by the render layer).
    pub current: usize,
    /// Query string (shown verbatim in the status line).
    pub query: &'a str,
    /// Query cursor (char index; positions the terminal cursor in the status line).
    pub cursor: usize,
    /// Current selection animation age in ms (`Some` means fade/shine is playing; `None` means steady).
    pub current_age_ms: Option<u64>,
}

/// Case-insensitive search for all non-overlapping hits in plain text; returns display column ranges.
/// Columns use [`crate::ansi::display_width`] (same as render truncation).
pub(crate) fn find_matches(plain: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle = query.to_lowercase();
    let hay = plain.to_lowercase();
    let mut out = Vec::new();
    let mut start = 0usize;
    while start <= hay.len() {
        let Some(rel) = hay[start..].find(&needle) else {
            break;
        };
        let b = start + rel;
        let x0 = crate::ansi::display_width(&hay[..b]);
        let w = crate::ansi::display_width(&hay[b..b + needle.len()]);
        if w > 0 {
            out.push((x0, x0 + w));
        }
        start = b + needle.len().max(1);
    }
    out
}

/// Search highlight: wraps each column range in background colors (plain pale blue, current green plus gold).
///
/// - `ranges`: `(x0, x1, is_current)` in any order (sorted by x0, then split right to left
///   in the same columns as [`crate::ansi::split_styled_range`]);
/// - `current_age_ms`: current selection animation age (`Some` fades dark-green to green plus text shine,
///   `None`/expired means steady green plus gold);
/// - after an in-row reset, re-assert the background so highlights never break (same as selection highlight);
/// - non-styled environments (`NO_COLOR`) return the input unchanged.
pub(crate) fn highlight_search_ranges(
    input: &str,
    ranges: &[(usize, usize, bool)],
    styled: bool,
    current_age_ms: Option<u64>,
) -> String {
    if !styled || ranges.is_empty() {
        return input.to_string();
    }
    // Selection animation: background fades dark-green to green within FADE_MS; text shines within SHINE_MS, then gold.
    // Past expiry uses steady state (matches the driver cutoff).
    let shining = current_age_ms.filter(|age| *age < SEARCH_SHINE_MS);
    let fade_op = match shining {
        Some(age) => smoothstep((age as f32 / SEARCH_FADE_MS as f32).clamp(0.0, 1.0)),
        None => 1.0,
    };
    let current_bg = mix_rgb(SEARCH_CURRENT_FADE_FROM, SEARCH_CURRENT_BG, fade_op);
    let shine_t = shining.map(|age| (age as f32 / SEARCH_SHINE_MS as f32).clamp(0.0, 1.0));
    let mut sorted: Vec<(usize, usize, bool)> = ranges.to_vec();
    sorted.sort_by_key(|r| (r.0, r.1));
    let mut out = input.to_string();
    for (x0, x1, is_current) in sorted.into_iter().rev() {
        if x0 >= x1 {
            continue;
        }
        let (before, mid, after) = crate::ansi::split_styled_range(&out, x0, x1);
        if mid.is_empty() {
            continue;
        }
        if is_current {
            let painted = paint_current_match(&mid, current_bg, shine_t);
            let mut open = String::new();
            push_bg_rgb(&mut open, current_bg, styled);
            out = format!("{before}{open}{painted}{SEARCH_BG_RESET}{after}");
        } else {
            let mid = mid.replace(
                "\x1b[0m",
                &format!("\x1b[0m\x1b[{code}m", code = SEARCH_BG_CODE),
            );
            out = format!("{before}{SEARCH_BG}{mid}{SEARCH_BG_RESET}{after}");
        }
    }
    out
}

/// Visible char count (skips ESC sequences; section signs already became ANSI upstream, nothing to handle).
pub(super) fn count_visible_chars(s: &str) -> usize {
    let mut n = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c2 in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c2) {
                        break;
                    }
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
        n += 1;
    }
    n
}

/// Current selected match text: green background plus gold text, brightening per char toward white during shine.
///
/// Source SGR passes through (`0m`/reverse/bold keep their meaning); every visible char first overwrites background plus
/// foreground (later wins: source foreground never leaks through, the whole selection stays gold;
/// re-asserted per char after `0m` so it never breaks; non-SGR CSI/OSC is dropped per the splitter).
/// `shine_t`: shine progress 0..1 (`None` means steady gold); the wave sweeps left to right and leaves,
/// steady gold at both ends (no jump; same waveform and window as the title shine).
pub(super) fn paint_current_match(mid: &str, bg: Rgb, shine_t: Option<f32>) -> String {
    let n = count_visible_chars(mid);
    let front = shine_t.map(|t| t.clamp(0.0, 1.0) * (n as f32 + 6.0) - 3.0);
    let mut out = String::with_capacity(mid.len() + n * 24);
    let mut idx = 0usize;
    let mut last_fg: Option<Rgb> = None;
    let mut chars = mid.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                let mut params = String::new();
                let mut seq = String::from("\x1b[");
                let mut is_sgr = false;
                for c2 in chars.by_ref() {
                    seq.push(c2);
                    if ('\u{40}'..='\u{7e}').contains(&c2) {
                        is_sgr = c2 == 'm'
                            && params
                                .chars()
                                .all(|p| p.is_ascii_digit() || p == ';' || p == ':');
                        break;
                    }
                    params.push(c2);
                }
                if is_sgr {
                    out.push_str(&seq);
                    // SGR may have changed colors, so force the foreground again on the next char.
                    last_fg = None;
                }
                // Non-SGR CSI is dropped directly (same scope as the splitter).
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
        let glow = match front {
            Some(f) => {
                let d = idx as f32 - f;
                (-(d * d) / (2.0 * 1.5 * 1.5)).exp()
            }
            None => 0.0,
        };
        let fg = mix_rgb(SEARCH_CURRENT_FG, SEARCH_SHINE_PEAK, glow * 0.9);
        // Re-assert the background per char (never breaks after `0m`); emit foreground only on change (saves bytes).
        // (Only called in styled environments; `styled` is always true, see the early caller return.)
        push_bg_rgb(&mut out, bg, true);
        if last_fg != Some(fg) {
            push_fg_rgb(&mut out, fg, true);
            last_fg = Some(fg);
        }
        out.push(c);
        idx += 1;
    }
    out
}

/// Status-line search box: `query <cur/total>` on the left (returns used columns plus the query cursor column).
///
/// Overlong queries show only the tail (the cursor is usually at the tail; the cursor column shifts and clamps).
pub(super) fn render_search_bar(
    buf: &mut String,
    search: &SearchCtx,
    width: usize,
    right_w: usize,
    styled: bool,
    lang: &str,
) -> (usize, usize) {
    let total = search.matches.len();
    let count_text = if search.query.is_empty() {
        rust_i18n::t!("tui.search_placeholder", locale = lang).to_string()
    } else if total == 0 {
        rust_i18n::t!("tui.search_no_match", locale = lang).to_string()
    } else {
        format!(
            "{}/{}",
            search.current.min(total.saturating_sub(1)) + 1,
            total
        )
    };
    let count_w = display_width(&count_text);
    let left_budget = width.saturating_sub(right_w + 1);
    let query_budget = left_budget.saturating_sub(SEARCH_PREFIX_W + 1 + count_w);
    let query_w = display_width(search.query);
    let hidden_w = query_w.saturating_sub(query_budget);
    let query_shown = if hidden_w == 0 {
        search.query.to_string()
    } else {
        crate::ansi::slice_by_width(search.query, hidden_w, query_w)
    };
    let shown_w = display_width(&query_shown);
    push_fg(buf, Color::Cyan, styled);
    buf.push_str(&rust_i18n::t!("tui.search_prefix", locale = lang));
    if styled {
        buf.push_str("\x1b[39m");
    }
    buf.push_str(&query_shown);
    buf.push(' ');
    push_fg(buf, Color::DarkGrey, styled);
    buf.push_str(&count_text);
    push_reset(buf);
    let used = SEARCH_PREFIX_W + shown_w + 1 + count_w;
    // Query cursor column (char index to display column; shifts together when showing only the tail).
    let query_chars = search.query.chars().count();
    let cursor_w: usize = search
        .query
        .chars()
        .take(search.cursor.min(query_chars))
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum();
    let cursor_x = SEARCH_PREFIX_W + cursor_w.saturating_sub(hidden_w).min(shown_w);
    (used, cursor_x)
}

/// One log plain-text row (visible text, same as the `build_frame` log section; reused for selection extraction).
///
/// ANSI escapes and section-sign color codes are stripped so clipboard copies carry plain text;
/// column mapping uses the stripped visible columns (styles are zero width).
pub(crate) fn log_row_plain(line: &LogLine) -> String {
    let mut row = String::new();
    row.push_str(&line.ts);
    row.push(' ');
    row.push_str(level_label(line.level));
    row.push(' ');
    let first = line.message.replace('\t', "  ").replace('\r', "");
    let first = first.split('\n').next().unwrap_or("");
    row.push_str(&crate::ansi::plain_text(first));
    row
}
