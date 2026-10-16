//! TUI rendering: flat minimal style (no borders).
//!
//! Top to bottom: two title rows (version plus CPU/MEM), log stream, completion list,
//! five input rows (wrapped editing plus pack version), status line.
//!
//! Diffs frame snapshots per row, rewriting only changed rows and clearing tails; cursor and paint flush together.
//! Width math goes through [`crate::ansi`] (ANSI/section-sign/wide-char safe).
//!
//! Boot effects (the driver advances `phase` from server state):
//!
//! - `Booting` (`Frame::boot`, configurable): clears the screen, then stretches the title and input bars
//!   from the left to the edges with easing, followed by first-row title typing plus CPU/MEM fade-in,
//!   input content fading in together without waiting for the title, and visible logs typing out top to bottom
//!   (steady boot state after ~1.1s; later logs render as full rows)
//! - `Starting`: input locked, blue ripple flows across the input box, title breathes (blue tones)
//! - `Sweeping`: green sweep moves left to right (~900ms), title fades from blue to green
//! - `Running`: normal input, title stays soft green

use std::collections::VecDeque;
use std::io::Write;

use crossterm::style::Color;
use unicode_width::UnicodeWidthChar;

use crate::ansi::{
    display_width, highlight_range, push_bg_rgb, push_fg_rgb, style_enabled, truncate_styled,
    truncate_to_width, Rgb,
};
use crate::editor::EditorState;
use crate::model::{ConsoleHeader, ConsoleStats, LogLevel, LogLine, ServerPhase};

pub mod boot;
pub mod frame;
pub mod input;
pub mod logview;
pub mod search;
use self::boot::*;
use self::frame::*;
use self::input::*;
use self::logview::*;
use self::search::*;
pub(crate) use self::boot::{BootFrame, boot_frame, boot_log_budgets};
pub(crate) use self::input::{POPUP_APPEAR_MS, animated_popup_count, popup_row_anim};
pub(crate) use self::logview::{Hit, ScrollView, Selection, hit_test, input_click_cursor, jump_target_view_wrapped, log_layout_wrapped, log_wrap_height, scroll_view_wrapped, unseen_count, wrap_lines};
#[cfg(test)] pub(crate) use self::logview::{log_layout, scroll_view};
pub(crate) use self::search::{SearchCtx, SearchMatch, find_matches, highlight_search_ranges, log_row_plain};
/// Layout constants (row counts).
pub const HEADER_ROWS: usize = 2;
pub const INPUT_ROWS: usize = 5;
/// Input box top padding rows (content sits lower for a roomier feel).
pub const INPUT_PAD_TOP: usize = 1;
/// Input text area rows (padding plus content share them; content uses the last 3).
pub const INPUT_TEXT_ROWS: usize = 4;
pub const STATUS_ROWS: usize = 1;

/// Render snapshot (assembled per frame by the driver thread; rendering only reads it).
pub struct Frame<'a> {
    pub logs: &'a VecDeque<LogLine>,
    pub editor: &'a EditorState,
    /// Log viewport position (follow-bottom or frozen anchor, see [`ScrollView`]).
    pub view: ScrollView,
    /// History rows evicted from the ring buffer (offset from global sequence to deque index).
    pub log_evicted: u64,
    /// Total enqueued logs (global sequence watermark; with `log_evicted` converts rows below the
    /// viewport into a "new arrivals" count).
    pub log_seq: u64,
    /// Frozen watermark (`Some` means reviewing): the `log_seq` at freeze time;
    /// only later arrivals count as new.
    pub frozen_base_seq: Option<u64>,
    /// Mouse drag selection (`Some` means drag-highlight; release-to-copy is done by the driver).
    pub selection: Option<Selection>,
    /// Online player name snapshot (read-only side panel; pushed by the host each tick).
    pub players: &'a [String],
    /// Log search (`Some` means searching: match highlight plus status-line search box plus cursor).
    pub search: Option<SearchCtx<'a>>,
    /// Copy notice (input box first-row top right): text plus opacity 0..1 (fades in/out).
    pub copy_note: Option<(String, f32)>,
    /// Exit double-confirm prompt opacity 0..1 (status line left, pale-red background with golden text;
    /// `None`/0 shows the normal key hints).
    pub exit_prompt: Option<f32>,
    pub dropped_logs: u64,
    pub provider_name: &'a str,
    /// Top info bar (server version plus pack label).
    pub header: &'a ConsoleHeader,
    /// Title second-row display string (typewriter intermediate state, advanced by the driver per quote version).
    pub subtitle: &'a str,
    /// Quote shine progress 0..1 (`Some` means left-to-right shine after typing finishes).
    pub subtitle_shine: Option<f32>,
    /// Session console language (canonical code; threads every chrome lookup).
    pub lang: &'a str,
    /// Refresh every row after a size change (no pre-clear, avoids flicker).
    pub full_clear: bool,
    pub stats: &'a ConsoleStats,
    pub phase: ServerPhase,
    /// Sweep progress 0..1 (valid in `Sweeping`, 1 in `Running`).
    pub sweep_progress: f32,
    /// Boot animation frame (`Some` means stretch/type/fade is playing, ~1.1s; `None` means steady).
    /// Only appears during `Starting`; the driver returns to the normal boot ripple when done.
    pub boot: Option<BootFrame>,
    /// Completion popup appear age in ms (`Some` means popping upward, ~240ms; `None` means steady).
    pub popup_age_ms: Option<u64>,
    /// Celebration progress 0..1 (`Some` plays `>STARTUP<` after the sweep without blocking input).
    pub celebrate: Option<f32>,
    /// Afterglow strength 1 to 0 (faint green input background after the sweep, ~800ms; 0 means none).
    pub afterglow: f32,
    /// Animation clock in ms (accumulated by the driver since start; tests pass fixed values).
    pub anim_ms: u64,
    pub width: u16,
    pub height: u16,
}

fn level_color(level: LogLevel) -> Color {
    match level {
        LogLevel::Error => Color::Red,
        LogLevel::Warn => Color::Yellow,
        LogLevel::Info => Color::Green,
        LogLevel::Debug => Color::Grey,
        LogLevel::Trace => Color::DarkGrey,
    }
}

fn level_label(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Error => "ERROR",
        LogLevel::Warn => "WARN ",
        LogLevel::Info => "INFO ",
        LogLevel::Debug => "DEBUG",
        LogLevel::Trace => "TRACE",
    }
}

/// Input content left margin (prompt, text, and footnote share the indent, never hug the edge).
pub const INPUT_MARGIN: usize = 3;
/// Prompt text (includes spacing so it never touches the text).
pub const PROMPT_TEXT: &str = "❯  ";
/// Prompt display width (marker takes 1 cell plus 2 spaces).
pub const PROMPT_W: usize = 3;
/// First-row text start column: margin plus prompt width.
pub const TEXT_COL: usize = INPUT_MARGIN + PROMPT_W; // 6
/// Right online-player panel width (includes the left separator column).
pub const SIDE_PANEL_WIDTH: usize = 22;
/// Panel background: one shade darker than the title/input bars, distinct but unobtrusive.
const SIDE_PANEL_BG: Rgb = Rgb(22, 22, 22);
/// Minimum terminal width for the panel (narrow windows hide it; logs take the full width).
pub(crate) const SIDE_PANEL_MIN_WIDTH: usize = 70;

/// Whether the panel is visible (same condition as [`log_content_width`]; shared by driver and render).
pub(crate) fn side_panel_visible(term_width: usize) -> bool {
    term_width >= SIDE_PANEL_MIN_WIDTH
}

/// Log area content width (yields to the panel on the right when visible; title/input/status stay full width).
pub(crate) fn log_content_width(term_width: usize) -> usize {
    if side_panel_visible(term_width) {
        term_width.saturating_sub(SIDE_PANEL_WIDTH).max(1)
    } else {
        term_width.max(1)
    }
}
// ================= Effect colors =================

/// Steady bar background RGB (dark gray #262626, truecolor output; used for title bars).
const BAR_RGB: Rgb = Rgb(38, 38, 38);
const SWEEP_PEAK: Rgb = Rgb(135, 255, 135);
const SWEEP_HALF_WIDTH: f64 = 6.0;
const RIPPLE_PEAK: Rgb = Rgb(0x10, 0xEF, 0xEF);
const RIPPLE_HALF_WIDTH: f64 = 8.0;
const RIPPLE_SPEED: f64 = 0.022; // 列/毫秒
const PACK_COLOR: Rgb = Rgb(95, 215, 255);
const TITLE_BLUE: Rgb = Rgb(135, 215, 255);
const TITLE_DIM: Rgb = Rgb(35, 100, 140);
const SWEEP_TRAIL: Rgb = Rgb(48, 65, 50);
/// Quote steady base color (matches fixed color 117 = (135,215,255), same as the non-shine path).
const SUB_BASE: Rgb = Rgb(135, 215, 255);
/// Quote shine peak (near white, bright but not harsh; same color as the title shine).
const SUB_SHINE_PEAK: Rgb = Rgb(235, 245, 255);
/// Unified TUI pale blue RGB (near color 81 = (95,215,255)): steady marker, slide end, copy-notice steady state.
const LIGHT_BLUE: Rgb = Rgb(95, 215, 255);
/// Slide-in marker fade start (dark end near color 32 = (0,135,215), interpolates toward [`LIGHT_BLUE`]).
const SLIDE_BLUE_DIM: Rgb = Rgb(0, 135, 215);
/// Copy notice gray-blue gradient (steady slate-400 near (148,163,178), low-key;
/// dark end converges toward the background).
const COPY_DIM: Rgb = Rgb(55, 65, 80);
const COPY_BRIGHT: Rgb = Rgb(148, 163, 178);
/// Copy notice lifetime in ms: 250ms fade in, hold, then 400ms fade out.
pub(crate) const COPY_FLASH_MS: u64 = 2500;
/// Search-box label width in columns (both `tui.search_prefix` translations
/// are 6 columns; locked by `config::tests`).
pub(crate) const SEARCH_PREFIX_W: usize = 6;
/// Key hint steady color (near gray 245) and fade-out dark end.
const HINT_FG: Rgb = Rgb(135, 135, 135);
const HINT_FG_DIM: Rgb = Rgb(24, 24, 24);
/// Exit confirm: pale-red background (full bright and fade dark end).
const EXIT_BG: Rgb = Rgb(122, 40, 40);
const EXIT_BG_DIM: Rgb = Rgb(22, 14, 14);
/// Exit confirm: golden text (full bright and fade dark end).
const EXIT_FG: Rgb = Rgb(255, 205, 90);
const EXIT_FG_DIM: Rgb = Rgb(64, 56, 44);
const COPY_FADE_IN_MS: u64 = 250;
const COPY_FADE_OUT_MS: u64 = 400;
/// Search hit highlight: plain hits use pale-blue background (104); the current one uses green plus gold text.
const SEARCH_BG: &str = "\x1b[104m";
const SEARCH_BG_CODE: &str = "104";
/// Current selection background (steady endpoint; animates from dark green with truecolor so handoff is seamless).
const SEARCH_CURRENT_BG: Rgb = Rgb(56, 142, 60);
/// Current selection background fade start (dark green).
const SEARCH_CURRENT_FADE_FROM: Rgb = Rgb(18, 52, 20);
/// Current selection text (gold; brightens toward the white peak during shine).
const SEARCH_CURRENT_FG: Rgb = Rgb(255, 213, 79);
/// Text shine peak (near white).
const SEARCH_SHINE_PEAK: Rgb = Rgb(255, 255, 255);
/// Current selection background fade duration (ms) and text shine duration (ms, total animation length).
pub(crate) const SEARCH_FADE_MS: u64 = 250;
pub(crate) const SEARCH_SHINE_MS: u64 = 500;
/// Search hit background reset (log rows have no background of their own; back to the default).
const SEARCH_BG_RESET: &str = "\x1b[49m";

/// Copy notice opacity 0..1 (the driver calls it by notice age; the render layer maps it onto a blue gradient).
pub(crate) fn copy_opacity(elapsed_ms: u64) -> f32 {
    if elapsed_ms < COPY_FADE_IN_MS {
        elapsed_ms as f32 / COPY_FADE_IN_MS as f32
    } else if elapsed_ms + COPY_FADE_OUT_MS < COPY_FLASH_MS {
        1.0
    } else if elapsed_ms < COPY_FLASH_MS {
        (COPY_FLASH_MS - elapsed_ms) as f32 / COPY_FADE_OUT_MS as f32
    } else {
        0.0
    }
}
/// Running-state placeholder: a gray slightly lighter than the background.
const PLACEHOLDER_GRAY: u8 = 248;

/// Sweep highlight position: the leading edge travels across the whole input box (including entry/exit).
fn sweep_front(width: usize, progress: f32) -> f32 {
    let margin = SWEEP_HALF_WIDTH as f32;
    let travel = width as f32 + margin * 2.0;
    progress.clamp(0.0, 1.0) * travel - margin
}

/// Smooth falloff on both sides with zero slope at the peak and the gray base, avoiding band edges.
fn band_rgb(distance: f64, half_width: f64, peak: Rgb) -> Rgb {
    let t = (1.0 - distance.abs() / half_width).clamp(0.0, 1.0);
    mix_rgb(BAR_RGB, peak, (t * t * (3.0 - 2.0 * t)) as f32)
}

/// Sweep row background: keeps sub-cell positions, continuously mixing bright green with the gray base.
fn sweep_bar(col: usize, width: usize, progress: f32) -> Rgb {
    if progress >= 1.0 {
        return BAR_RGB;
    }
    let distance = col as f64 - sweep_front(width, progress) as f64;
    let trail = smoothstep((-distance / SWEEP_HALF_WIDTH) as f32);
    let base = mix_rgb(BAR_RGB, SWEEP_TRAIL, trail);
    let glow = smoothstep((1.0 - distance.abs() / SWEEP_HALF_WIDTH) as f32);
    mix_rgb(base, SWEEP_PEAK, glow)
}

/// Two rings spread from the center; the radius covers the farthest corner and loops only after the tail leaves.
fn ripple_bg(col: usize, row: usize, width: usize, anim_ms: u64) -> Rgb {
    let center_col = width / 2;
    // Scale distances by terminal cell aspect (row height is about 2x column width) so ripples look round.
    let dx = col as f64 - center_col as f64;
    let dy = (row as f64 - 2.0) * 2.0;
    let d = dx.hypot(dy);
    let edge = center_col.max(width.saturating_sub(1).saturating_sub(center_col)) as f64;
    let corner = edge.hypot((INPUT_ROWS - 1) as f64);
    // Ring spacing is at least one full color band, leaving room to fade to gray even in narrow windows.
    let travel = (corner + RIPPLE_HALF_WIDTH * 2.0).max(RIPPLE_HALF_WIDTH * 4.0);
    let mut distance = f64::INFINITY;
    for k in 0..2 {
        // Enter from a negative radius so the center is gray on wrap and no new ring flashes in.
        let r = (anim_ms as f64 * RIPPLE_SPEED + RIPPLE_HALF_WIDTH + k as f64 * travel / 2.0)
            % travel
            - RIPPLE_HALF_WIDTH;
        distance = distance.min((d - r).abs());
    }
    let brightness = ripple_brightness(anim_ms);
    mix_rgb(
        BAR_RGB,
        band_rgb(distance, RIPPLE_HALF_WIDTH, RIPPLE_PEAK),
        brightness,
    )
}

fn ripple_brightness(anim_ms: u64) -> f32 {
    0.15 + 0.85 * smoothstep((anim_ms as f32 - 500.0) / 1000.0)
}

/// RGB linear mix (used for shine highlights; also reused by the welcome screen).
pub(crate) fn mix_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let mix = |ca: u8, cb: u8| (ca as f32 + (cb as f32 - ca as f32) * t).round() as u8;
    Rgb(mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

pub(crate) fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Ease-out (fast start, slow end; used for marker slide-in).
pub(crate) fn ease_out_cubic(p: f32) -> f32 {
    1.0 - (1.0 - p.clamp(0.0, 1.0)).powi(3)
}

/// Sine ease-out (soft start, zero landing velocity; used for blob gathering).
pub(crate) fn ease_out_sine(p: f32) -> f32 {
    (p.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2).sin()
}

/// Title breathing color (period about 1.6s).
fn breath_fg(anim_ms: u64) -> Rgb {
    let p = ((anim_ms as f32 / 1600.0 * std::f32::consts::TAU).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
    mix_rgb(TITLE_DIM, TITLE_BLUE, p)
}

/// Sweep gradient color (progress 0..1).
fn sweep_fg(progress: f32) -> Rgb {
    let p = progress.clamp(0.0, 1.0);
    if p < 0.5 {
        mix_rgb(TITLE_BLUE, SWEEP_PEAK, smoothstep(p * 2.0))
    } else {
        mix_rgb(SWEEP_PEAK, TITLE_BLUE, smoothstep((p - 0.5) * 2.0))
    }
}
fn fg256(buf: &mut String, code: u8, styled: bool) {
    if styled {
        buf.push_str(&format!("\x1b[38;5;{code}m"));
    }
}

fn ansi_fg(c: Color) -> &'static str {
    match c {
        Color::Black => "\x1b[30m",
        Color::DarkRed => "\x1b[31m",
        Color::DarkGreen => "\x1b[32m",
        Color::DarkYellow => "\x1b[33m",
        Color::DarkBlue => "\x1b[34m",
        Color::DarkMagenta => "\x1b[35m",
        Color::DarkCyan => "\x1b[36m",
        Color::Grey => "\x1b[38;5;247m", // 定死浅灰：不用 37，主题可重映射
        Color::DarkGrey => "\x1b[38;5;245m", // 定死中灰：不用 90，某些主题下 90 偏紫
        Color::Red => "\x1b[91m",
        Color::Green => "\x1b[92m",
        Color::Yellow => "\x1b[93m",
        Color::Blue => "\x1b[94m",
        Color::Magenta => "\x1b[95m",
        Color::Cyan => "\x1b[96m",
        Color::White => "\x1b[97m",
        _ => "\x1b[39m",
    }
}

fn push_fg(buf: &mut String, c: Color, styled: bool) {
    if styled {
        buf.push_str(ansi_fg(c));
    }
}

fn push_reset(buf: &mut String) {
    buf.push_str("\x1b[0m");
}

// ================= Shaded row emission =================

/// One span foreground: fixed color index or 24-bit RGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fg {
    Code(u8),
    Rgb(Rgb),
}

/// One span plus optional foreground (`None` means default).
type Segment<'a> = (&'a str, Option<Fg>);

/// Emit one shaded text row: lays the background per display cell; wide-char safe.
///
/// - `segments`: text spans (each may carry its own foreground);
/// - `bg_at`: (column, row) to background RGB (24-bit truecolor);
/// - `row`: this row number in effect coordinates;
/// - `start_col`: row start column (usually 0);
/// - rows pad with spaces to `width` (keeps the background bar complete).
fn emit_shaded_row(
    buf: &mut String,
    segments: &[Segment],
    bg_at: &dyn Fn(usize, usize) -> Rgb,
    row: usize,
    start_col: usize,
    width: usize,
    styled: bool,
) -> usize {
    let mut col = start_col;
    let mut cur_bg: Option<Rgb> = None;
    let mut cur_fg: Option<Fg> = None;
    // Lay the starting background first so the first cell is never bare.
    if styled {
        let bg = bg_at(col.min(width.saturating_sub(1)), row);
        push_bg_rgb(buf, bg, styled);
        cur_bg = Some(bg);
    }
    for (text, fg) in segments {
        for c in text.chars() {
            let cw = UnicodeWidthChar::width(c).unwrap_or(0);
            // Never split a wide char: truncate the whole char when it does not fit (pad spaces extend the background).
            if col + cw > width {
                break;
            }
            let bg = bg_at(col, row);
            if cur_bg != Some(bg) {
                push_bg_rgb(buf, bg, styled);
                cur_bg = Some(bg);
            }
            if cur_fg != *fg {
                match fg {
                    Some(Fg::Code(code)) => fg256(buf, *code, styled),
                    Some(Fg::Rgb(rgb)) => push_fg_rgb(buf, *rgb, styled),
                    None => {
                        if styled {
                            buf.push_str("\x1b[39m");
                        }
                    }
                }
                cur_fg = *fg;
            }
            buf.push(c);
            col += UnicodeWidthChar::width(c).unwrap_or(0);
        }
    }
    // Pad the row tail with spaces (background continues so the color bar stays complete).
    while col < width {
        let bg = bg_at(col, row);
        if cur_bg != Some(bg) {
            push_bg_rgb(buf, bg, styled);
            cur_bg = Some(bg);
        }
        if cur_fg.is_some() {
            if styled {
                buf.push_str("\x1b[39m");
            }
            cur_fg = None;
        }
        buf.push(' ');
        col += 1;
    }
    push_reset(buf);
    col
}
// ================= Main render =================

#[derive(Default)]
pub(crate) struct Renderer {
    rows: Vec<String>,
    size: Option<(u16, u16)>,
}

impl Renderer {
    pub fn render<W: Write>(&mut self, w: &mut W, f: &Frame) -> std::io::Result<()> {
        let (rows, (cx, cy)) = build_frame(f);
        let repaint = f.full_clear || self.size != Some((f.width, f.height));
        let mut buf = String::with_capacity(12 * 1024);
        // Without sync output support (e.g. Apple Terminal), per-row updates still avoid unrelated flicker.
        // Disable autowrap so full-width rows and the bottom-right corner never scroll; position each row absolutely.
        buf.push_str("\x1b[?2026h\x1b[?25l\x1b[?7l");
        for (i, row) in rows.iter().enumerate() {
            if repaint || self.rows.get(i) != Some(row) {
                buf.push_str(&format!("\x1b[{};1H\x1b[0m", i + 1));
                buf.push_str(row);
                // Erase the tail only after writing new content, so short logs and empty rows leave no residue.
                buf.push_str("\x1b[0m");
                if display_width(row) < f.width.max(1) as usize {
                    buf.push_str("\x1b[K");
                }
            }
        }
        buf.push_str(&format!("\x1b[{};{}H\x1b[?7h", cy + 1, cx + 1));
        // The search-state cursor moves into the search box (also allowed while locked; read-only).
        if f.phase == ServerPhase::Running || f.search.is_some() {
            buf.push_str("\x1b[?25h");
        }
        buf.push_str("\x1b[?2026l");
        w.write_all(buf.as_bytes())?;
        w.flush()?;
        self.rows = rows;
        self.size = Some((f.width, f.height));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::completion::CompletionItem;
    use crate::editor::CompletionState;
    use crate::model::{ConsoleHeader, ConsoleStats};
    use std::collections::VecDeque;

    /// Chinese chrome lookups (existing tests pin zh-CN rendering; English has
    /// its own tests below).
    fn zh_hint() -> String {
        rust_i18n::t!("tui.hint_back_to_bottom", locale = "zh-CN").to_string()
    }

    fn zh_exit() -> String {
        rust_i18n::t!("tui.exit_confirm", locale = "zh-CN").to_string()
    }

    fn en_frame<'a>(
        logs: &'a VecDeque<LogLine>,
        editor: &'a EditorState,
        header: &'a ConsoleHeader,
        subtitle: &'a str,
        stats: &'a ConsoleStats,
        phase: ServerPhase,
        width: u16,
        height: u16,
    ) -> Frame<'a> {
        let mut frame = sample_frame(logs, editor, header, subtitle, stats, phase, 0, width, height);
        frame.lang = "en-US";
        frame
    }

    fn frame_plain(frame: &Frame) -> String {
        build_frame(frame)
            .0
            .iter()
            .map(|r| crate::ansi::plain_text(r))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn english_chrome_renders_without_chinese() {
        let mut logs = VecDeque::new();
        for i in 0..30 {
            logs.push_back(LogLine::new(LogLevel::Info, "server", &format!("line {i:02}")));
        }
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut frame = en_frame(
            &logs, &editor, &header, "", &stats, ServerPhase::Running, 100, 30,
        );
        // Freeze off-bottom so the review hint row shows too.
        frame.view = ScrollView::Frozen { top_seq: 5 };
        frame.log_seq = 30;
        frame.frozen_base_seq = Some(30);
        let plain = frame_plain(&frame);
        // Input placeholder, side panel, status bar: all English.
        assert!(plain.contains("Type command"), "input placeholder");
        assert!(plain.contains("No players online"), "side panel empty");
        assert!(plain.contains("Online (0)"), "side panel title");
        assert!(plain.contains("Completion:static"), "status provider");
        assert!(plain.contains("Press Esc for bottom"), "review hint");
        for zh in ["在线", "查找", "补全", "锁定", "输入", "启动中", "玩家"] {
            assert!(!plain.contains(zh), "no Chinese leaks: {zh}");
        }
    }

    #[test]
    fn chinese_chrome_still_default_in_tests() {
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let frame = sample_frame(
            &logs, &editor, &header, "", &stats, ServerPhase::Running, 0, 100, 30,
        );
        let plain = frame_plain(&frame);
        assert!(plain.contains("在线玩家(0)"));
        assert!(plain.contains("补全:static"));
    }

    fn sample_frame<'a>(
        logs: &'a VecDeque<LogLine>,
        editor: &'a EditorState,
        header: &'a ConsoleHeader,
        subtitle: &'a str,
        stats: &'a ConsoleStats,
        phase: ServerPhase,
        anim_ms: u64,
        width: u16,
        height: u16,
    ) -> Frame<'a> {
        Frame {
            logs,
            editor,
            view: ScrollView::FollowBottom,
            log_evicted: 0,
            log_seq: 0,
            frozen_base_seq: None,
            selection: None,
            players: &[],
            copy_note: None,
            exit_prompt: None,
            dropped_logs: 0,
            provider_name: "static",
            header,
            subtitle,
            subtitle_shine: None,
            lang: crate::config::LANG_ZH_CN,
            stats,
            phase,
            sweep_progress: if phase == ServerPhase::Running {
                1.0
            } else {
                0.0
            },
            celebrate: None,
            afterglow: 0.0,
            boot: None,
            search: None,
            popup_age_ms: None,
            full_clear: false,
            anim_ms,
            width,
            height,
        }
    }

    fn test_header() -> ConsoleHeader {
        ConsoleHeader::default()
    }

    fn test_stats() -> ConsoleStats {
        ConsoleStats {
            cpu_pct: 12.5,
            mem_mb: 312.5,
            has_data: true,
        }
    }

    fn render_text(frame: &Frame) -> String {
        let mut buf = Vec::new();
        render(&mut buf, frame).expect("render");
        String::from_utf8(buf).expect("utf8")
    }

    #[test]
    fn forced_repaint_does_not_clear_screen() {
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        let mut renderer = Renderer::default();
        renderer.render(&mut Vec::new(), &frame).expect("first");
        frame.full_clear = true;
        let mut output = Vec::new();
        renderer.render(&mut output, &frame).expect("repaint");
        let output = String::from_utf8(output).expect("utf8");
        assert!(!output.contains("\x1b[2J"), "尺寸变化也不先清屏");
        assert!(output.contains("\x1b[1;1H"));
        assert!(output.contains("\x1b[24;1H"));
    }

    #[test]
    fn frame_starts_at_home_without_clear() {
        let logs = VecDeque::from([LogLine::new(LogLevel::Info, "server", "hi")]);
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        let mut buf = Vec::new();
        Renderer::default()
            .render(&mut buf, &frame)
            .expect("render");
        let output = String::from_utf8(buf).expect("utf8");
        assert!(output.contains("\x1b[1;1H"));
        assert!(!output.contains("\x1b[2J"));
        assert!(!output.contains("\r\n"), "绝对定位避免滚屏");
        let s = render_text(&frame);
        // Total row count always equals height.
        assert_eq!(s.split("\r\n").count(), 24);
    }

    #[test]
    fn header_has_two_rows_with_cpu_and_mem() {
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = ConsoleHeader::new("1.0.0ALPHA(fjord)", "Vanilla 1.26.40 · protocol 2168");
        let subtitle = "「只想去往山野和清风来邂逅」";
        let stats = test_stats();
        let frame = sample_frame(
            &logs,
            &editor,
            &header,
            subtitle,
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        let s = render_text(&frame);
        let rows: Vec<&str> = s.split("\r\n").collect();
        assert!(rows[0].contains("1.0.0ALPHA(fjord)"), "首行核心版本");
        assert!(rows[0].contains("CPU"), "首行右侧 CPU");
        assert!(rows[0].contains("12.5%"), "CPU 值");
        assert!(rows[1].contains("只想去往山野和清风来邂逅"), "次行一言");
        assert!(rows[1].contains("MEM"), "次行右侧 MEM");
        assert!(rows[1].contains("312.5 MB"), "MEM 值");
    }

    #[test]
    fn input_box_has_five_rows_with_pack_footer() {
        let logs = VecDeque::new();
        let mut editor = EditorState::new();
        editor.insert_str("st");
        let header = ConsoleHeader::new("1.0.0ALPHA(fjord)", "Vanilla 1.26.40 · protocol 2168");
        let stats = test_stats();
        let frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        let mut buf = Vec::new();
        let (cx, cy) = render(&mut buf, &frame).expect("render");
        let s = String::from_utf8(buf).expect("utf8");
        let rows: Vec<&str> = s.split("\r\n").collect();
        assert_eq!(rows.len(), 24);
        // Five input rows: height-6..height-1; row 0 is top padding, the last row is the version footnote.
        let input_rows = &rows[24 - 6..24 - 1];
        assert_eq!(input_rows.len(), 5);
        assert!(!input_rows[0].contains('❯'), "首行留白");
        assert!(input_rows[1].contains('❯'), "内容首行为编辑行");
        assert!(
            input_rows[4].contains("protocol 2168"),
            "末行左下角版本包版本"
        );
        // Cursor sits on the edit row (height-6+1); column is indent 3 plus prompt 3 plus 2 chars.
        assert_eq!(cy, 19);
        assert_eq!(cx, 8);
    }

    #[test]
    fn long_input_wraps_instead_of_scrolling() {
        let logs = VecDeque::new();
        let mut editor = EditorState::new();
        editor.insert_str(&"ab".repeat(60)); // 120 字符，80 宽下折 2 行
        let header = test_header();
        let stats = test_stats();
        let frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            40,
            24,
        );
        let mut buf = Vec::new();
        let (cx, cy) = render(&mut buf, &frame).expect("render");
        // width=40: first-row budget 38 takes 38 chars; 82 remain for row two (40); 42 remain for row three (40);
        // 2 remain for row four. Cursor at the end lands on row four (display index 3) with 2 in-row chars.
        assert_eq!(cy, 24 - 6 + 3);
        // Row four is a continuation (indent 3); cursor column is 3 plus 12 in-row char columns.
        assert_eq!(cx, 15);
    }

    #[test]
    fn starting_phase_locks_input_with_placeholder() {
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Starting,
            500,
            80,
            24,
        );
        let s = render_text(&frame);
        // The ripple inserts background escapes per column; content assertions target the visible text.
        let visible = crate::ansi::plain_text(&s);
        assert!(visible.contains("服务端正在启动中..."), "锁定占位居中");
        assert!(visible.contains("锁定"), "状态行标记");
    }

    #[test]
    fn animation_is_deterministic_for_same_clock() {
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let a = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Starting,
            1234,
            80,
            24,
        );
        let b = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Starting,
            1234,
            80,
            24,
        );
        let c = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Starting,
            9999,
            80,
            24,
        );
        assert_eq!(render_text(&a), render_text(&b), "同时钟输出一致");
        assert_ne!(render_text(&a), render_text(&c), "水波纹随时间流动");
    }

    fn fg_code_count(row_raw: &str) -> usize {
        // Distinct in-row foreground colors after dedup (both 38;5 and 38;2 forms collected).
        let mut set = std::collections::HashSet::new();
        let mut rest = row_raw;
        while let Some(i) = rest.find("38;") {
            let tail = &rest[i + 4..];
            let end = tail.find('m').unwrap_or(tail.len());
            set.insert(tail[..end].to_string());
            rest = &tail[end..];
            if rest.len() < 6 {
                break;
            }
        }
        set.len()
    }

    #[test]
    fn title_shine_sweeps_after_green() {
        // Inside the shine window the row shows several foreground brightnesses; outside it stays single-color.
        let (_, raw_on) = sweep_frame(0.65, 80);
        let row_on = raw_on.split("\r\n").next().unwrap_or("");
        let (_, raw_off) = sweep_frame(0.2, 80);
        let row_off = raw_off.split("\r\n").next().unwrap_or("");
        assert!(
            fg_code_count(row_on) > fg_code_count(row_off),
            "扫光应产生更多前景色"
        );
    }

    /// SGR sequence in effect for char `ch` in `raw_row` (the nearest escape before that char).
    fn fg_before(raw_row: &str, ch: char) -> String {
        let pos = raw_row
            .find(ch)
            .unwrap_or_else(|| panic!("找不到字符 {ch}"));
        let head = &raw_row[..pos];
        head.rfind('\x1b')
            .map(|i| {
                let tail = &head[i..];
                let end = tail.find('m').map(|e| e + 1).unwrap_or(0);
                tail[..end].to_string()
            })
            .unwrap_or_default()
    }

    /// Raw frame of the quote row (second row) at a given shine progress.
    fn subtitle_frame(sub: &str, shine: Option<f32>) -> (Vec<String>, String) {
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            sub,
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        frame.subtitle_shine = shine;
        let mut buf = Vec::new();
        render(&mut buf, &frame).expect("render");
        let s = String::from_utf8(buf).expect("utf8");
        let rows: Vec<String> = s
            .split("\r\n")
            .map(|r| crate::ansi::plain_text(r))
            .collect();
        (rows, s)
    }

    #[test]
    fn subtitle_shine_sweeps_left_to_right() {
        // Mid-shine: the second row shows several foreground brightnesses (body brightens per char).
        let (rows, raw) = subtitle_frame("「山野清风」", Some(0.5));
        assert!(
            rows.get(1).is_some_and(|r| r.starts_with("「山野清风」")),
            "第二行展示完整一言"
        );
        let row1 = raw.split("\r\n").nth(1).unwrap_or("");
        assert!(fg_code_count(row1) >= 3, "扫光应产生多种亮度");
        // Without shine the whole row is single-color (fixed color 117).
        let (_, raw_off) = subtitle_frame("「山野清风」", None);
        let row_off = raw_off.split("\r\n").nth(1).unwrap_or("");
        assert_eq!(
            fg_before(row_off, '\u{5c71}'),
            "\x1b[38;5;117m",
            "无扫光时正文为固定色号"
        );
    }

    #[test]
    fn subtitle_closer_stays_pinned_during_shine() {
        let pin = format!("\x1b[38;2;{};{};{}m", SUB_BASE.0, SUB_BASE.1, SUB_BASE.2);
        // The pinned right bracket always keeps the base color and never joins the shine.
        for t in [0.05_f32, 0.3, 0.5, 0.8, 0.98] {
            let (_, raw) =
                subtitle_frame("\u{300c}\u{5c71}\u{91ce}\u{6e05}\u{98ce}\u{300d}", Some(t));
            let row1 = raw.split("\r\n").nth(1).unwrap_or("");
            assert_eq!(
                fg_before(row1, '\u{300d}'),
                pin,
                "扫光进度 {t} 时右括号仍为基色"
            );
        }
        // The body instead varies with shine progress (never single-color).
        let (_, a) = subtitle_frame(
            "\u{300c}\u{5c71}\u{91ce}\u{6e05}\u{98ce}\u{300d}",
            Some(0.1),
        );
        let (_, b) = subtitle_frame(
            "\u{300c}\u{5c71}\u{91ce}\u{6e05}\u{98ce}\u{300d}",
            Some(0.9),
        );
        let ra = a.split("\r\n").nth(1).unwrap_or("");
        let rb = b.split("\r\n").nth(1).unwrap_or("");
        assert_ne!(
            fg_before(ra, '\u{5c71}'),
            fg_before(rb, '\u{5c71}'),
            "正文随扫光进度变化"
        );
    }

    #[test]
    fn split_pinned_closer_only_strips_closing_bracket() {
        assert_eq!(split_pinned_closer("「abc」"), ("「abc", "」"));
        assert_eq!(split_pinned_closer("no bracket"), ("no bracket", ""));
        assert_eq!(split_pinned_closer("」"), ("", "」"));
    }

    #[test]
    fn overscroll_clamps_to_oldest_page() {
        // Scrolling past the top pins the topmost page: the earliest logs stay visible
        // instead of leaving the whole log area blank.
        let logs = VecDeque::from([
            LogLine::new(LogLevel::Info, "test", "第一条"),
            LogLine::new(LogLevel::Info, "test", "第二条"),
            LogLine::new(LogLevel::Info, "test", "第三条"),
        ]);
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        frame.view = ScrollView::Frozen { top_seq: 0 };
        let (rows, _) = build_frame(&frame);
        let shown = rows.join("\n");
        assert!(
            crate::ansi::plain_text(&shown).contains("第一条"),
            "滚过头仍显示最早日志"
        );
    }

    #[test]
    fn copy_note_shows_at_input_top_right() {
        // The copy notice appears at the input first row (top right); the driver clears it after ~2.5s.
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        frame.copy_note = Some(("已复制12个字符到剪贴板".to_string(), 1.0));
        let (rows, _) = build_frame(&frame);
        // Input block start row is height minus input and status rows; the first row is padding.
        let top = &rows[24 - INPUT_ROWS - STATUS_ROWS];
        let plain = crate::ansi::plain_text(top);
        assert!(
            plain.trim_end().ends_with("已复制12个字符到剪贴板"),
            "提示右对齐"
        );
    }

    #[test]
    fn copy_note_uses_gray_blue_with_fade() {
        // Steady (opacity 1) is slate-400 gray-blue (low-key); the first frame is dark gray-blue.
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        frame.copy_note = Some(("已复制1个字符到剪贴板".to_string(), 1.0));
        let mut buf = Vec::new();
        render(&mut buf, &frame).expect("render");
        let raw = String::from_utf8(buf).expect("utf8");
        let top = raw
            .split("\r\n")
            .nth(24 - INPUT_ROWS - STATUS_ROWS)
            .unwrap_or("");
        assert!(top.contains("38;2;148;163;178"), "稳态为灰蓝");
        frame.copy_note = Some(("已复制1个字符到剪贴板".to_string(), 0.0));
        let mut buf = Vec::new();
        render(&mut buf, &frame).expect("render");
        let raw = String::from_utf8(buf).expect("utf8");
        let top = raw
            .split("\r\n")
            .nth(24 - INPUT_ROWS - STATUS_ROWS)
            .unwrap_or("");
        assert!(top.contains("38;2;55;65;80"), "首帧为暗灰蓝（渐显起点）");
        assert!(!top.contains("38;2;148;163;178"), "首帧未直接全亮");
        // Midway is a smooth intermediate color (never discrete jumps).
        frame.copy_note = Some(("已复制1个字符到剪贴板".to_string(), 0.5));
        let mut buf = Vec::new();
        render(&mut buf, &frame).expect("render");
        let raw = String::from_utf8(buf).expect("utf8");
        let top = raw
            .split("\r\n")
            .nth(24 - INPUT_ROWS - STATUS_ROWS)
            .unwrap_or("");
        assert!(top.contains("38;2;102;114;129"), "中途为插值中间色");
    }

    #[test]
    fn copy_opacity_fades_in_and_out() {
        // 250ms fade in, hold, then 400ms fade out (COPY_FLASH_MS=2500).
        assert_eq!(copy_opacity(0), 0.0);
        assert_eq!(copy_opacity(125), 0.5);
        assert_eq!(copy_opacity(250), 1.0);
        assert_eq!(copy_opacity(1000), 1.0);
        assert_eq!(copy_opacity(2100), 1.0);
        assert_eq!(copy_opacity(2300), 0.5);
        assert_eq!(copy_opacity(2500), 0.0);
        assert_eq!(copy_opacity(9999), 0.0);
    }

    #[test]
    fn copy_note_absent_keeps_blank_top_row() {
        // With no notice the first row stays padding (keeps the roomy feel).
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        let (rows, _) = build_frame(&frame);
        let top = &rows[24 - INPUT_ROWS - STATUS_ROWS];
        assert!(crate::ansi::plain_text(top).trim().is_empty());
    }

    #[test]
    fn selection_highlights_log_range() {
        // Log rows inside the drag range show reverse video; rows outside show none.
        let logs = VecDeque::from([
            LogLine::new(LogLevel::Info, "test", "第一条日志"),
            LogLine::new(LogLevel::Info, "test", "第二条日志"),
        ]);
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        let mut sel = Selection::point(1, 0);
        sel.extend(1, 5);
        frame.selection = Some(sel);
        let mut buf = Vec::new();
        render(&mut buf, &frame).expect("render");
        let raw = String::from_utf8(buf).expect("utf8");
        let row1 = raw.split("\r\n").nth(HEADER_ROWS + 1).unwrap_or("");
        let row0 = raw.split("\r\n").nth(HEADER_ROWS).unwrap_or("");
        assert!(row1.contains("\x1b[7m"), "选中行反白");
        assert!(!row0.contains("\x1b[7m"), "未选中行不高亮");
    }

    #[test]
    fn hit_test_maps_rows_to_regions() {
        // height=24 with no popup: log area [2,16), input box [18,23), status row 23.
        let layout = log_layout(24, 0, 100, 0, ScrollView::FollowBottom);
        let ones = |_: usize| 1usize;
        assert!(layout.valid);
        assert_eq!(layout.log_top, HEADER_ROWS);
        assert_eq!(layout.log_rows, 24 - 8);
        assert_eq!(layout.input_base, 24 - INPUT_ROWS - STATUS_ROWS);
        // Screen row 2 is the earliest visible row (100-16=84).
        assert!(matches!(
            hit_test(&layout, &ones, 80, 5, 2),
            Hit::Log { idx: 84, x: 5 }
        ));
        assert!(matches!(
            hit_test(&layout, &ones, 80, 10, 20),
            Hit::Input { row: 2, x: 10 }
        ));
        assert_eq!(hit_test(&layout, &ones, 80, 0, 0), Hit::None);
        assert_eq!(hit_test(&layout, &ones, 80, 0, 23), Hit::None);
        // Out-of-range indexes (empty logs) never hit.
        let empty = log_layout(24, 0, 0, 0, ScrollView::FollowBottom);
        assert_eq!(hit_test(&empty, &ones, 80, 0, 2), Hit::None);
        // Too-short terminals are invalid.
        assert!(!log_layout(8, 0, 10, 0, ScrollView::FollowBottom).valid);
    }

    #[test]
    fn input_click_maps_column_to_cursor() {
        // Single-line text: click content row 1 (in-box row 1); columns map to the cursor.
        // First-row base TEXT_COL=6: "stop" covers columns 6..10.
        assert_eq!(input_click_cursor("stop", 4, 80, 1, 6), 0);
        assert_eq!(input_click_cursor("stop", 4, 80, 1, 7), 1);
        assert_eq!(input_click_cursor("stop", 4, 80, 1, 100), 4);
        // Padding row (in-box row 0) jumps to the line end.
        assert_eq!(input_click_cursor("stop", 0, 80, 0, 0), 4);
        // Empty text means 0.
        assert_eq!(input_click_cursor("", 0, 80, 1, 10), 0);
    }

    #[test]
    fn log_row_plain_strips_ansi_and_section_codes() {
        // Copies use visible text: ANSI escapes and section-sign codes must not reach the clipboard.
        let line = LogLine::new(LogLevel::Info, "test", "§a你好\x1b[31m世界\x1b[0m！");
        assert_eq!(
            log_row_plain(&line),
            format!("{} INFO  你好世界！", line.ts)
        );
        // Column mapping also uses visible columns: escapes take no columns.
        let plain = log_row_plain(&line);
        assert_eq!(
            crate::ansi::slice_by_width(&plain, 0, 100),
            plain,
            "无零宽残留影响列宽"
        );
    }

    #[test]
    fn hint_row_takes_one_log_row_only_when_scrolled() {
        // height=24 with no popup: 16 base log rows; reviewing leaves 15 rows plus 1 hint row.
        let scrolled = log_layout(24, 0, 100, 0, ScrollView::Frozen { top_seq: 60 });
        assert!(scrolled.valid);
        assert_eq!(scrolled.hint_rows, 1);
        assert_eq!(scrolled.log_rows, 15);
        // Not reviewing means no hint and a full 16 log rows.
        let bottom = log_layout(24, 0, 100, 0, ScrollView::FollowBottom);
        assert_eq!(bottom.hint_rows, 0);
        assert_eq!(bottom.log_rows, 16);
        // With a popup the hint yields (Esc closes the popup first, never competes for the row).
        let popup = log_layout(24, 3, 100, 0, ScrollView::Frozen { top_seq: 60 });
        assert_eq!(popup.hint_rows, 0);
        // With only 1 row left there is no hint (never squeezes out the last log).
        let tiny = log_layout(9, 0, 100, 0, ScrollView::Frozen { top_seq: 50 });
        assert_eq!(tiny.log_rows, 1);
        assert_eq!(tiny.hint_rows, 0);
    }

    #[test]
    fn hint_row_geometry_is_stable() {
        // Recomputing the same viewport (the driver does it every frame) must agree: no flicker.
        for top in [0usize, 1, 40, 83, 84] {
            let view = ScrollView::Frozen {
                top_seq: top as u64,
            };
            let a = log_layout(24, 0, 100, 0, view);
            let b = log_layout(24, 0, 100, 0, view);
            assert_eq!(
                (a.start, a.end, a.up, a.log_rows, a.hint_rows),
                (b.start, b.end, b.up, b.log_rows, b.hint_rows)
            );
            // Anchored: start is the anchored top (clamped to [0, max_start]).
            assert!(a.start <= top.min(100usize.saturating_sub(a.log_rows)));
        }
        // top=84 (=max_start) already hugs the bottom, so no hint shows.
        let at_bottom = log_layout(24, 0, 100, 0, ScrollView::Frozen { top_seq: 84 });
        assert_eq!(at_bottom.hint_rows, 0);
        assert_eq!(at_bottom.up, 0);
    }

    #[test]
    fn hit_test_jump_to_latest_row() {
        // While reviewing: log area [2,17), hint row 17, input box [18,23).
        let layout = log_layout(24, 0, 100, 0, ScrollView::Frozen { top_seq: 60 });
        let ones = |_: usize| 1usize;
        assert_eq!(layout.log_rows, 15);
        assert_eq!(layout.input_base, 18);
        assert_eq!(hit_test(&layout, &ones, 80, 0, 17), Hit::JumpToLatest);
        assert_eq!(hit_test(&layout, &ones, 80, 40, 17), Hit::JumpToLatest);
        // At follow-bottom there is no hint row, so that row is a normal log row (no shortcut jump).
        let bottom = log_layout(24, 0, 100, 0, ScrollView::FollowBottom);
        assert_eq!(bottom.hint_rows, 0);
        assert_eq!(bottom.log_rows, 16);
        assert!(matches!(
            hit_test(&bottom, &ones, 80, 0, 17),
            Hit::Log { .. }
        ));
    }

    #[test]
    fn hint_row_shows_hint_and_only_nonzero_badge() {
        fn make_logs(range: std::ops::Range<usize>) -> VecDeque<LogLine> {
            range
                .map(|i| LogLine::new(LogLevel::Info, "test", &format!("log{i:02}")))
                .collect()
        }
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let hint_row = |f: &Frame| {
            let (rows, _) = build_frame(f);
            let plain = crate::ansi::plain_text(&rows[24 - INPUT_ROWS - STATUS_ROWS - 1]);
            // Excludes the right player panel (wide windows); only asserts the hint area (content width 58).
            plain.split('│').next().unwrap_or("").to_string()
        };
        let text_w = crate::ansi::display_width(zh_hint().as_str());
        let cw = log_content_width(80);

        // Case one: 30 old logs under review with no new arrivals means only the centered hint, no badge.
        let before = make_logs(0..30);
        let mut frame = sample_frame(
            &before,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        frame.view = ScrollView::Frozen { top_seq: 5 };
        frame.log_seq = 30;
        frame.frozen_base_seq = Some(30);
        let plain = hint_row(&frame);
        assert!(plain.contains(zh_hint().as_str()), "提示文案");
        assert!(!plain.contains("新增"), "无新增不显示徽标：{plain:?}");
        let leading = plain.chars().take_while(|c| *c == ' ').count();
        assert_eq!(leading, (cw - text_w) / 2, "提示居中于内容区");

        // Case two: 7 new logs arrive while reviewing (seq 30..36).
        let after = make_logs(0..37);
        let mut frame = sample_frame(
            &after,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        frame.view = ScrollView::Frozen { top_seq: 5 };
        frame.log_seq = 37;
        frame.frozen_base_seq = Some(30);
        // Every new arrival sits below the viewport, so the badge shows 7.
        let plain = hint_row(&frame);
        assert!(
            plain.trim_end().ends_with("▼新增7条日志"),
            "徽标右对齐且只算新增：{plain:?}"
        );
        assert!(plain.contains(zh_hint().as_str()), "提示仍在");
        let badge_w = crate::ansi::display_width("▼新增7条日志");
        let leading = plain.chars().take_while(|c| *c == ' ').count();
        assert_eq!(
            leading,
            (cw - badge_w - 1 - text_w) / 2,
            "提示在徽标左侧居中"
        );

        // Scrolling down: 3 new arrivals entered the viewport, so the badge drops to 4.
        frame.view = ScrollView::Frozen { top_seq: 18 };
        let plain = hint_row(&frame);
        assert!(plain.trim_end().ends_with("▼新增4条日志"), "{plain:?}");

        // Scrolled to the bottom: every new arrival is in view, so badge and hint row disappear together.
        frame.view = ScrollView::Frozen { top_seq: 22 };
        let plain = hint_row(&frame);
        assert!(!plain.contains("新增"), "到底无新增徽标：{plain:?}");
        assert!(
            !plain.contains(zh_hint().as_str()),
            "到底无提示行：{plain:?}"
        );

        // Follow-bottom means no hint row (even with a leftover freeze watermark).
        let mut bottom = sample_frame(
            &after,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        bottom.log_seq = 37;
        bottom.frozen_base_seq = Some(30);
        let (rows2, _) = build_frame(&bottom);
        let joined = rows2.join("\n");
        assert!(!joined.contains("回到底部"));
        assert!(!joined.contains("新增"));
    }

    #[test]
    fn unseen_count_subtracts_new_logs_inside_viewport() {
        // Freeze watermark 100 (seq 100 and up are new), 110 arrived total, evicted=0.
        let base = Some(100);
        // Viewport [0,16): every new arrival sits below, so 10 rows.
        assert_eq!(unseen_count(110, 0, 16, base), 10);
        // Viewport scrolled to [85,101): holds 1 new arrival (seq 100), so minus 1 leaves 9.
        assert_eq!(unseen_count(110, 0, 101, base), 9);
        // Viewport [90,106): holds 6 new arrivals, so 4 remain.
        assert_eq!(unseen_count(110, 0, 106, base), 4);
        // Scrolled to the bottom [94,110): every new arrival is in view, so 0 (badge gone).
        assert_eq!(unseen_count(110, 0, 110, base), 0);
        // No freeze watermark (not reviewing) means 0.
        assert_eq!(unseen_count(110, 0, 16, None), 0);
        // Scrolling up changes nothing (new arrivals all sit below; the watermark is the freeze level).
        assert_eq!(unseen_count(110, 0, 5, base), 10);
        // New logs keep arriving, so the count only grows.
        assert_eq!(unseen_count(115, 0, 16, base), 15);
        // Ring eviction: the count stays right after watermark conversion
        // (evicted=5 with last viewport index 105 means below-viewport starts at seq 110, leaving 5).
        assert_eq!(unseen_count(115, 5, 105, base), 5);
    }

    #[test]
    fn exit_confirm_pill_cross_fades_with_hints() {
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        let status = |f: &Frame| {
            let (rows, _) = build_frame(f);
            rows.last().cloned().unwrap_or_default()
        };
        let hint_fg_code = format!("38;2;{};{};{}m", HINT_FG.0, HINT_FG.1, HINT_FG.2);
        let pill_bg_code = format!("48;2;{};{};{}m", EXIT_BG.0, EXIT_BG.1, EXIT_BG.2);
        let pill_fg_code = format!("38;2;{};{};{}m", EXIT_FG.0, EXIT_FG.1, EXIT_FG.2);

        // Steady: only the normal key hints (full-bright gray), no pill.
        let raw = status(&frame);
        assert!(crate::ansi::plain_text(&raw).contains("TAB"), "常规提示");
        assert!(raw.contains(&hint_fg_code), "提示满亮");
        assert!(!raw.contains(zh_exit().as_str()));
        assert!(!raw.contains(&pill_bg_code));

        // Confirm full bright: the pill (pale-red plus golden text) covers the hints, which fade to 0.
        frame.exit_prompt = Some(1.0);
        let raw = status(&frame);
        let plain = crate::ansi::plain_text(&raw);
        assert!(plain.contains(zh_exit().as_str()), "确认文案：{plain:?}");
        assert!(!plain.contains("TAB"), "按键提示被替换");
        assert!(raw.contains(&pill_bg_code), "淡红底");
        assert!(raw.contains(&pill_fg_code), "金黄字");
        assert!(!raw.contains(&hint_fg_code), "提示已淡出");

        // Mid-transition: the confirm hint keeps its place while colors interpolate as RGB.
        frame.exit_prompt = Some(0.5);
        let raw = status(&frame);
        let plain = crate::ansi::plain_text(&raw);
        assert!(plain.contains(zh_exit().as_str()));
        assert!(!plain.contains("TAB"), "确认提示占用同一左侧区域");
        assert!(!raw.contains(&pill_bg_code), "药丸非满亮");
        assert!(!raw.contains(&hint_fg_code), "提示非满亮");
        assert!(raw.contains("48;2;72;27;27"), "半透明底色");
        assert!(raw.contains("38;2;160;131;67"), "半透明字色");
        assert_eq!(display_width(&plain), 80, "渐变期间不溢出状态行");

        // Pill gone (None / 0): hints fade back to full bright with no empty rows or stale blocks.
        frame.exit_prompt = None;
        let raw = status(&frame);
        assert!(crate::ansi::plain_text(&raw).contains("TAB"));
        assert!(raw.contains(&hint_fg_code), "提示渐显到满亮");
        assert!(!raw.contains("48;2;72;27;27"), "无残色块");
        assert!(!raw.contains(zh_exit().as_str()));
    }

    #[test]
    fn exit_confirm_status_stays_within_terminal_width() {
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        for width in [20, 32, 40, 80, 160] {
            let mut frame = sample_frame(
                &logs,
                &editor,
                &header,
                "",
                &stats,
                ServerPhase::Running,
                0,
                width,
                24,
            );
            for opacity in [0.0, 0.01, 0.5, 1.0] {
                frame.exit_prompt = Some(opacity);
                let (rows, _) = build_frame(&frame);
                let plain = crate::ansi::plain_text(rows.last().unwrap());
                assert_eq!(display_width(&plain), width as usize, "{plain:?}");
                let right = format!("补全:{}", frame.provider_name);
                assert!(plain.ends_with(&right), "右侧状态保持右对齐：{plain:?}");
                if opacity > 0.0 {
                    assert!(
                        zh_exit().starts_with(plain.trim_end_matches(&right).trim_end())
                    );
                }
            }
        }
    }

    #[test]
    fn selection_range_semantics() {
        // A tap has no range.
        let p = Selection::point(3, 5);
        assert!(p.is_point());
        assert_eq!(p.normalize(), (3, 5, 3, 5));
        assert_eq!(p.x_range_for(3), None);
        // Same-row drags (either direction normalizes the same way).
        let mut s = Selection::point(2, 9);
        s.extend(2, 4);
        assert!(!s.is_point());
        assert_eq!(s.normalize(), (2, 4, 2, 9));
        assert_eq!(s.x_range_for(2), Some((4, 9)));
        assert_eq!(s.x_range_for(1), None);
        // Across rows: first row runs to the end, last row starts at the beginning, middle rows are whole.
        let mut m = Selection::point(1, 7);
        m.extend(3, 2);
        assert_eq!(m.normalize(), (1, 7, 3, 2));
        assert_eq!(m.x_range_for(1), Some((7, usize::MAX)));
        assert_eq!(m.x_range_for(2), Some((0, usize::MAX)));
        assert_eq!(m.x_range_for(3), Some((0, 2)));
        assert_eq!(m.x_range_for(4), None);
        // Tail column 0 means the tail row has no range (selected to its start, excluding it).
        let mut z = Selection::point(1, 3);
        z.extend(2, 0);
        assert_eq!(z.x_range_for(2), None);
    }

    #[test]
    fn afterglow_tints_then_clears() {
        // At full afterglow the input row carries green; at zero it matches the no-afterglow frame exactly.
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut full = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        full.afterglow = 1.0;
        let mut buf = Vec::new();
        render(&mut buf, &full).expect("render");
        let s = String::from_utf8(buf).expect("utf8");
        assert!(s.contains("48;2;94;122;94"), "余辉绿底可见");
        let plain = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        let mut buf2 = Vec::new();
        render(&mut buf2, &plain).expect("render");
        let s2 = String::from_utf8(buf2).expect("utf8");
        assert!(!s2.contains("48;2;94;122;94"), "余辉归零无残留");
    }

    fn sweep_frame(progress: f32, width: u16) -> (Vec<String>, String) {
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Sweeping,
            0,
            width,
            24,
        );
        frame.sweep_progress = progress;
        let mut buf = Vec::new();
        render(&mut buf, &frame).expect("render");
        let s = String::from_utf8(buf).expect("utf8");
        let rows: Vec<String> = s
            .split("\r\n")
            .map(|r| crate::ansi::plain_text(r))
            .collect();
        (rows, s)
    }

    #[test]
    fn prompt_slides_in_from_window_edge() {
        // Content rows start at display row 1: the marker sits near column 1 partway through the slide.
        let (rows, _) = sweep_frame(0.2, 80);
        let first_input = &rows[24 - 6 + 1];
        assert_eq!(first_input.find('❯'), Some(1), "滑入中：{first_input:?}");
        // At completion the marker reaches the content indent (column 3, never hugging the window edge).
        let (rows, _) = sweep_frame(1.0, 80);
        let first_input = &rows[24 - 6 + 1];
        assert_eq!(first_input.find('❯'), Some(3), "归位：{first_input:?}");
    }

    #[test]
    fn sweep_is_a_line_not_a_fill() {
        // Mid-sweep the band sits centered: the same row mixes gray and green (never a flat fill).
        let (rows, raw) = sweep_frame(0.5, 80);
        let _ = rows;
        let input_row = raw.split("\r\n").nth(24 - 6 + 1).unwrap_or("");
        assert!(input_row.contains("48;2;38;38;38"), "灰底保留");
        assert!(input_row.contains("48;2;135;255;135"), "绿色亮带峰值");
    }

    #[test]
    fn sweeping_prompt_fades_with_rgb_gradient() {
        // The sliding marker interpolates as RGB (never hardcoded 256-color jumps): dark blue to unified pale blue.
        let (_, raw_start) = sweep_frame(0.0, 80);
        let row_start = raw_start.split("\r\n").nth(24 - 6 + 1).unwrap_or("");
        assert!(row_start.contains("38;2;0;135;215"), "滑动起点为暗蓝");
        let (_, raw_end) = sweep_frame(1.0, 80);
        let row_end = raw_end.split("\r\n").nth(24 - 6 + 1).unwrap_or("");
        assert!(row_end.contains("38;2;95;215;255"), "滑动终点为统一淡蓝");
    }

    fn celebrate_frame(progress: f32, width: u16) -> (Vec<String>, String) {
        // Plays after the sweep ends (input already unlocked, never blocked): Running plus an independent clock.
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = ConsoleHeader::new("1.0.0ALPHA(fjord)", "Vanilla 1.26.40 · protocol 2168");
        let stats = test_stats();
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            width,
            24,
        );
        frame.celebrate = Some(progress);
        let mut buf = Vec::new();
        render(&mut buf, &frame).expect("render");
        let s = String::from_utf8(buf).expect("utf8");
        let rows: Vec<String> = s
            .split("\r\n")
            .map(|r| crate::ansi::plain_text(r))
            .collect();
        (rows, s)
    }

    #[test]
    fn celebrate_row_shrinks_and_fades() {
        // q=0.0: pack label starts fading (footnote row height-2, still pack text).
        let (rows, _) = celebrate_frame(0.0, 80);
        assert_eq!(rows.len(), 24, "总行数恒定（庆祝复用脚注行）");
        assert!(
            rows[24 - 2].contains("protocol 2168"),
            "渐隐前版本包：{:?}",
            rows[24 - 2]
        );
        // q=0.15: mid-shrink (gathers within 0.5s) with spacing near 6.
        let (rows, _) = celebrate_frame(0.15, 80);
        assert!(
            rows[24 - 2].contains('>') && rows[24 - 2].contains("      "),
            "疏字距：{:?}",
            rows[24 - 2]
        );
        // q=0.78: already gathered into >STARTUP< (per-char colors; asserts the visible text).
        let (rows, _) = celebrate_frame(0.78, 80);
        assert_eq!(rows.len(), 24);
        assert!(
            rows[24 - 2].contains(">STARTUP<"),
            "收拢：{:?}",
            rows[24 - 2]
        );
        // q=0.5: pack label and STARTUP coexist during the hold (input stays usable).
        let (rows, _) = celebrate_frame(0.5, 80);
        assert!(
            rows[24 - 2].contains("protocol 2168"),
            "版本包已显现：{:?}",
            rows[24 - 2]
        );
        assert!(
            rows[24 - 2].contains(">STARTUP<"),
            "STARTUP 保持：{:?}",
            rows[24 - 2]
        );
        // q=0.95: pack label fades back in.
        let (rows, _) = celebrate_frame(0.95, 80);
        assert!(
            rows[24 - 2].contains("protocol 2168"),
            "渐显：{:?}",
            rows[24 - 2]
        );
        // Finished (None): footnote returns to normal.
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        let s = render_text(&frame);
        assert!(!s.contains("STARTUP"), "播完无庆祝");
    }

    #[test]
    fn shine_sweeps_across_letters() {
        // Hold-phase shine: chars in the same row differ (brightness follows wave position).
        // Collects both truecolor (38;2) and 256-color fallback (38;5) foreground codes.
        let (_, raw) = celebrate_frame(0.5, 80);
        let row = raw.split("\r\n").nth(24 - 2).unwrap_or("");
        let mut fgs: Vec<String> = vec![];
        let mut rest = row;
        while let Some(i) = rest.find("38;") {
            let tail = &rest[i + 4..];
            let end = tail.find('m').unwrap_or(tail.len());
            fgs.push(tail[..end].to_string());
            rest = &tail[end..];
            if rest.len() < 6 {
                break;
            }
        }
        let uniq: std::collections::HashSet<String> = fgs.into_iter().collect();
        assert!(uniq.len() >= 3, "扫光应产生多种亮度");
    }

    #[test]
    fn sweep_progress_changes_output() {
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut start = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Sweeping,
            0,
            80,
            24,
        );
        start.sweep_progress = 0.1;
        let mut end = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Sweeping,
            0,
            80,
            24,
        );
        end.sweep_progress = 0.9;
        assert_ne!(render_text(&start), render_text(&end), "横扫进度可见");
    }

    #[test]
    fn ripple_band_is_centered_and_fades() {
        assert_eq!(ripple_bg(40, 2, 80, 0), mix_rgb(BAR_RGB, RIPPLE_PEAK, 0.15));
        assert_eq!(ripple_bg(39, 2, 80, 0), ripple_bg(41, 2, 80, 0), "左右对称");
        assert_eq!(ripple_bg(25, 2, 80, 0), BAR_RGB, "环外灰底");
        // The traveling ring shows at t=1000 (radius near 22, not the background).
        assert_ne!(ripple_bg(62, 2, 80, 1000), BAR_RGB, "行进环可见");
        // Row height counts double: a 2-column offset equals a 1-row offset (looks near-circular).
        assert_eq!(ripple_bg(42, 2, 80, 0), ripple_bg(40, 3, 80, 0));
    }

    #[test]
    fn ripple_brightens_only_after_half_a_second() {
        assert_eq!(ripple_brightness(0), 0.15);
        assert_eq!(ripple_brightness(500), 0.15);
        assert!(ripple_brightness(1000) > 0.15 && ripple_brightness(1000) < 1.0);
        assert_eq!(ripple_brightness(1500), 1.0);
        for ms in 500..1500 {
            let delta = ripple_brightness(ms + 1) - ripple_brightness(ms);
            assert!((0.0..0.002).contains(&delta));
        }
    }

    #[test]
    fn title_uses_continuous_rgb_and_returns_to_running_color() {
        let colors: std::collections::HashSet<_> = (0..1600)
            .map(|ms| {
                let Rgb(r, g, b) = breath_fg(ms);
                (r, g, b)
            })
            .collect();
        assert!(colors.len() > 100);
        assert_eq!(breath_fg(400), TITLE_BLUE);
        assert_eq!(breath_fg(1200), TITLE_DIM);
        assert_eq!(sweep_fg(1.0), TITLE_BLUE);
    }

    #[test]
    fn celebration_fades_to_background_and_stays_within_terminal_width() {
        for width in [1, 10, 24, 80, 160] {
            for step in 0..=125 {
                let q = step as f32 / 125.0;
                let (rows, raw) = celebrate_frame(q, width);
                assert!(rows.iter().all(|row| display_width(row) <= width as usize));
                if width >= 10 {
                    assert!(raw.contains("\x1b[38;2;"));
                }
            }
        }
        let (_, raw) = celebrate_frame(1.0, 80);
        let foot = raw.split("\r\n").nth(22).unwrap();
        assert!(foot.contains("\x1b[38;2;95;215;255m"), "版本包全亮");
        let mut terminal = vt100::Parser::new(24, 80, 0);
        terminal.process(b"\x1b[?7l");
        terminal.process(raw.as_bytes());
        let letter = (0..80)
            .filter_map(|col| terminal.screen().cell(22, col))
            .find(|cell| cell.contents() == ">")
            .expect("庆祝文字");
        assert_eq!(
            letter.fgcolor(),
            vt100::Color::Rgb(38, 38, 38),
            "庆祝文字完全淡到背景"
        );
        let (_, fade_a) = celebrate_frame(0.02, 80);
        let (_, fade_b) = celebrate_frame(0.021, 80);
        assert_ne!(fade_a, fade_b, "版本包渐隐保留细微颜色变化");
        let (_, motion_a) = celebrate_frame(0.2, 80);
        let (_, motion_b) = celebrate_frame(0.2001, 80);
        assert_ne!(motion_a, motion_b, "字距整格之间也有亮度过渡");
    }

    #[test]
    fn sweep_band_is_centered_and_fades() {
        // At half sweep on width 80 the front sits at column 40: peak 120, decaying to gray on both sides.
        assert_eq!(sweep_bar(40, 80, 0.5), Rgb(135, 255, 135));
        assert_eq!(sweep_bar(0, 80, 0.5), SWEEP_TRAIL, "扫过位置留下淡绿");
        assert_eq!(sweep_bar(79, 80, 0.5), BAR_RGB, "未扫过位置保留灰底");
    }

    #[test]
    fn bands_change_smoothly_between_frames_and_end_at_background() {
        for (half_width, peak) in [
            (SWEEP_HALF_WIDTH, SWEEP_PEAK),
            (RIPPLE_HALF_WIDTH, RIPPLE_PEAK),
        ] {
            assert_eq!(band_rgb(0.0, half_width, peak), peak);
            assert_eq!(band_rgb(half_width, half_width, peak), BAR_RGB);
            let colors: std::collections::HashSet<_> = (0..100)
                .map(|i| {
                    let Rgb(r, g, b) = band_rgb(i as f64 * half_width / 100.0, half_width, peak);
                    (r, g, b)
                })
                .collect();
            assert!(colors.len() > 70, "渐变不能退回固定色阶");
        }
        assert_ne!(sweep_bar(38, 80, 0.5), sweep_bar(38, 80, 0.501));
        for progress in [0.0, 1.0] {
            assert!((0..80).all(|col| sweep_bar(col, 80, progress) == BAR_RGB));
        }
    }

    #[test]
    fn ripple_reaches_edges_and_corners_on_narrow_and_wide_terminals() {
        for width in [24, 80, 160, 320] {
            let center = width / 2;
            for (col, row) in [(0, 0), (0, 2), (width - 1, 2), (width - 1, 4)] {
                let distance = (col as f64 - center as f64).hypot((row as f64 - 2.0) * 2.0);
                let peak_ms = (distance / RIPPLE_SPEED).round() as u64;
                let brightness = 0.15 + 0.85 * smoothstep((peak_ms as f32 - 500.0) / 1000.0);
                assert_eq!(
                    ripple_bg(col, row, width, peak_ms),
                    mix_rgb(BAR_RGB, RIPPLE_PEAK, brightness)
                );
                let exit_ms = ((distance + RIPPLE_HALF_WIDTH) / RIPPLE_SPEED).ceil() as u64;
                assert_eq!(ripple_bg(col, row, width, exit_ms), BAR_RGB);
                // Outermost colors decay continuously to the gray base instead of resetting radius at the peak.
                let before_exit = ripple_bg(col, row, width, exit_ms - 100);
                assert!(before_exit.1 > BAR_RGB.1 && before_exit.1 < RIPPLE_PEAK.1);
            }
        }
    }

    #[test]
    fn terminal_replaces_long_logs_with_short_logs_and_empty_rows() {
        let mut logs = VecDeque::from([LogLine::new(LogLevel::Info, "test", &"旧日志".repeat(30))]);
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut renderer = Renderer::default();
        let mut terminal = vt100::Parser::new(24, 80, 0);
        for message in [None, Some("短日志"), Some("")] {
            if let Some(message) = message {
                logs.clear();
                if !message.is_empty() {
                    logs.push_back(LogLine::new(LogLevel::Info, "test", message));
                }
            }
            let frame = sample_frame(
                &logs,
                &editor,
                &header,
                "",
                &stats,
                ServerPhase::Running,
                0,
                80,
                24,
            );
            let mut output = Vec::new();
            renderer.render(&mut output, &frame).expect("render");
            terminal.process(&output);
            let expected = build_frame(&frame).0;
            let actual: Vec<_> = terminal.screen().rows(0, 80).collect();
            for (actual, expected) in actual.iter().zip(&expected) {
                assert_eq!(
                    actual.trim_end(),
                    crate::ansi::plain_text(expected).trim_end()
                );
            }
        }
    }

    #[test]
    fn animation_does_not_redraw_logs_and_unchanged_frame_does_not_repaint() {
        let logs = VecDeque::from([LogLine::new(LogLevel::Info, "test", "日志保持稳定")]);
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Starting,
            0,
            80,
            24,
        );
        let mut renderer = Renderer::default();
        renderer.render(&mut Vec::new(), &frame).expect("first");
        let mut unchanged = Vec::new();
        renderer.render(&mut unchanged, &frame).expect("unchanged");
        assert!(!String::from_utf8(unchanged).unwrap().contains("\x1b[0m"));
        frame.anim_ms = 33;
        let mut output = Vec::new();
        renderer.render(&mut output, &frame).expect("animation");
        let output = String::from_utf8(output).unwrap();
        assert!(!output.contains("日志保持稳定"));
        assert!(!output.contains("\x1b[2J"));
        assert!(!output.contains("\r\n"));
        assert!(!output.contains("\x1b[?25h"), "启动动画不显示编辑光标");
    }

    #[test]
    fn terminal_handles_resize_narrow_popups_and_bottom_right_cell() {
        let logs = VecDeque::from([LogLine::new(
            LogLevel::Debug,
            "test",
            "long §a彩色日志\x1b[2J\x1b[H",
        )]);
        let mut editor = EditorState::new();
        editor.insert_str("/st");
        editor.completion = Some(CompletionState {
            items: vec![CompletionItem::new(
                &"停止服务端".repeat(20),
                "stop",
                &"长描述".repeat(30),
                true,
            )],
            selected: 0,
        });
        let header = test_header();
        let stats = test_stats();
        let mut renderer = Renderer::default();
        let mut terminal = vt100::Parser::new(24, 80, 0);
        for (width, height) in [(80, 24), (24, 12), (10, 9), (1, 1), (80, 24)] {
            terminal.screen_mut().set_size(height, width);
            let frame = sample_frame(
                &logs,
                &editor,
                &header,
                "",
                &stats,
                ServerPhase::Running,
                0,
                width,
                height,
            );
            let mut output = Vec::new();
            renderer.render(&mut output, &frame).expect("render");
            terminal.process(&output);
            let (expected, cursor) = build_frame(&frame);
            assert_eq!(expected.len(), height as usize);
            assert_eq!(terminal.screen().cursor_position(), (cursor.1, cursor.0));
            let actual: Vec<_> = terminal.screen().rows(0, width).collect();
            for (actual, expected) in actual.iter().zip(&expected) {
                assert!(display_width(expected) <= width as usize);
                assert_eq!(
                    actual.trim_end(),
                    crate::ansi::plain_text(expected).trim_end()
                );
            }
        }
    }

    #[test]
    fn wrap_lines_handles_cjk_and_cursor() {
        let chars: Vec<char> = "你好ab世界cd".chars().collect();
        let rows = wrap_lines(&chars, 5, 5);
        // Two wide chars plus one ASCII char fill 5 columns; the next char wraps.
        assert_eq!(rows.len(), 3);
        assert_eq!(locate_cursor(&rows, 0), (0, 0));
        assert_eq!(locate_cursor(&rows, 8), (2, 2));
        // Overlong single rows: the window keeps 4 rows.
        let long: Vec<char> = "x".repeat(500).chars().collect();
        let rows = wrap_lines(&long, 78, 80);
        assert!(rows.len() > INPUT_TEXT_ROWS);
    }

    #[test]
    fn completion_popup_sits_above_input() {
        let logs = VecDeque::new();
        let mut editor = EditorState::new();
        editor.insert_str("st");
        editor.completion = Some(CompletionState {
            items: vec![CompletionItem::new(
                "stop",
                "stop",
                "Stops the server",
                false,
            )],
            selected: 0,
        });
        let header = test_header();
        let stats = test_stats();
        let frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            80,
            24,
        );
        let s = render_text(&frame);
        let rows: Vec<&str> = s.split("\r\n").collect();
        // Completion rows hug the input box (input area starts at height-6).
        assert!(rows[24 - 6 - 1].contains("stop"), "补全应紧贴输入框上方");
    }

    fn boot_test_frame<'a>(
        logs: &'a VecDeque<LogLine>,
        editor: &'a EditorState,
        header: &'a ConsoleHeader,
        stats: &'a ConsoleStats,
        elapsed_ms: u64,
        width: u16,
        height: u16,
    ) -> Frame<'a> {
        let mut frame = sample_frame(
            logs,
            editor,
            header,
            "「山野清风」",
            stats,
            ServerPhase::Starting,
            elapsed_ms,
            width,
            height,
        );
        frame.boot = boot_frame(elapsed_ms);
        frame
    }

    #[test]
    fn boot_timeline_expands_then_types_then_fades() {
        // t=0: everything empty (zero bar length, title and fades unmoved, panel off the right edge).
        let b0 = boot_frame(0).expect("t=0 仍在播");
        assert_eq!(
            (
                b0.header_expand,
                b0.input_expand,
                b0.title_t,
                b0.meta_opacity,
                b0.input_opacity,
                b0.panel_slide
            ),
            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
        );
        // Mid-stretch: easing starts fast then slows; input and title bars run in parallel while title and fades hold.
        let mid = boot_frame(BOOT_BAR_MS / 2).expect("伸展中");
        assert!(
            (mid.header_expand - 0.875).abs() < 1e-5,
            "easeOutCubic(0.5)=0.875，实际 {}",
            mid.header_expand
        );
        assert_eq!(
            mid.input_expand, mid.header_expand,
            "输入框与标题栏并行伸展"
        );
        assert_eq!(mid.panel_slide, mid.header_expand, "面板与底条并行滑入");
        assert_eq!(
            (mid.title_t, mid.meta_opacity, mid.input_opacity),
            (0.0, 0.0, 0.0),
            "伸满前标题/渐显不动"
        );
        // Stretch-fill moment: title not started yet, panel already docked.
        let full = boot_frame(BOOT_BAR_MS).expect("伸满瞬间仍在播");
        assert_eq!((full.header_expand, full.input_expand), (1.0, 1.0));
        assert_eq!(full.panel_slide, 1.0);
        assert_eq!(
            (full.title_t, full.meta_opacity, full.input_opacity),
            (0.0, 0.0, 0.0)
        );
        // Mid-typing: input fades faster than the title (never waits for the title).
        let typing = boot_frame(BOOT_BAR_MS + 200).expect("打字中");
        assert!((0.0..1.0).contains(&typing.title_t));
        assert!(
            typing.input_opacity > typing.meta_opacity,
            "输入框不等标题播完：input={} meta={}",
            typing.input_opacity,
            typing.meta_opacity
        );
        // Finished: None (the driver returns to the steady boot state).
        assert!(boot_frame(BOOT_TOTAL_MS).is_none());
        assert!(boot_frame(BOOT_TOTAL_MS + 5000).is_none());
        // Monotonic: opacity only rises, never falls back (no flashing).
        let mut prev = 0.0;
        let mut step = 0;
        while step < BOOT_TOTAL_MS {
            let b = boot_frame(step).expect("播出中");
            assert!(b.meta_opacity >= prev, "t={step} 回闪");
            prev = b.meta_opacity;
            step += 37;
        }
    }

    #[test]
    fn boot_bars_grow_from_nothing_then_type_title() {
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = ConsoleHeader::new("9.9.9TEST", "Pack · protocol 1");
        let stats = test_stats();
        // t=100: mid-stretch, both title rows and the input box show no text (light bars only).
        let frame = boot_test_frame(&logs, &editor, &header, &stats, 100, 80, 24);
        assert!(frame.boot.is_some());
        let (rows, _) = build_frame(&frame);
        assert!(
            crate::ansi::plain_text(&rows[0]).trim().is_empty(),
            "标题行 1 还是空光条"
        );
        assert!(
            crate::ansi::plain_text(&rows[1]).trim().is_empty(),
            "标题行 2 还是空光条"
        );
        let input_top = &rows[24 - INPUT_ROWS - STATUS_ROWS];
        assert!(
            crate::ansi::plain_text(input_top).trim().is_empty(),
            "输入框还是空光条"
        );
        // t=700: bars filled with the title mid-typing (partial, not complete); CPU/MEM already appears together.
        let frame = boot_test_frame(&logs, &editor, &header, &stats, 700, 80, 24);
        let (rows, _) = build_frame(&frame);
        let row0 = crate::ansi::plain_text(&rows[0]);
        assert!(row0.contains("⬢"), "标记先出：{row0:?}");
        assert!(!row0.contains("9.9.9TEST"), "版本还没打完：{row0:?}");
        assert!(row0.contains("CPU"), "CPU 与标题同时出现：{row0:?}");
        assert!(
            crate::ansi::plain_text(&rows[1]).contains("MEM"),
            "MEM 同步渐显"
        );
        assert!(
            crate::ansi::plain_text(&rows[1]).contains("山野清风"),
            "一言同步渐显"
        );
    }

    #[test]
    fn boot_input_appears_without_waiting_for_title() {
        // t=600: title only half typed while the input boot text already faded in (never waits for the title).
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = ConsoleHeader::new("9.9.9TEST", "Pack · protocol 1");
        let stats = test_stats();
        let frame = boot_test_frame(&logs, &editor, &header, &stats, 600, 80, 24);
        let (rows, _) = build_frame(&frame);
        let row0 = crate::ansi::plain_text(&rows[0]);
        assert!(!row0.contains("9.9.9TEST"), "标题还没打完：{row0:?}");
        let joined = rows
            .iter()
            .map(|r| crate::ansi::plain_text(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("服务端正在启动中..."),
            "输入框内容已同步渐显"
        );
    }

    #[test]
    fn boot_last_frame_matches_steady_content() {
        // Seamless handoff: the last boot frame visible text equals steady boot state (only styling differs).
        let logs = VecDeque::from([
            LogLine::new(LogLevel::Info, "test", "第一条日志"),
            LogLine::new(LogLevel::Warn, "test", "第二条警告"),
        ]);
        let editor = EditorState::new();
        let header = ConsoleHeader::new("9.9.9TEST", "Pack · protocol 1");
        let stats = test_stats();
        let boot = boot_test_frame(&logs, &editor, &header, &stats, BOOT_TOTAL_MS - 1, 80, 24);
        assert!(boot.boot.is_some());
        let steady = sample_frame(
            &logs,
            &editor,
            &header,
            "「山野清风」",
            &stats,
            ServerPhase::Starting,
            BOOT_TOTAL_MS - 1,
            80,
            24,
        );
        let (a, _) = build_frame(&boot);
        let (b, _) = build_frame(&steady);
        let plain_a: Vec<String> = a.iter().map(|r| crate::ansi::plain_text(r)).collect();
        let plain_b: Vec<String> = b.iter().map(|r| crate::ansi::plain_text(r)).collect();
        assert_eq!(plain_a, plain_b, "最后一帧与稳态文本一致");
    }

    #[test]
    fn boot_stays_within_terminal_width() {
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = ConsoleHeader::new("9.9.9TEST", "Pack · protocol 1");
        let stats = test_stats();
        for width in [20u16, 40, 80] {
            for t in [0, 1, 100, 449, 450, 700, BOOT_TOTAL_MS - 1] {
                let frame = boot_test_frame(&logs, &editor, &header, &stats, t, width, 24);
                let (rows, _) = build_frame(&frame);
                assert_eq!(rows.len(), 24, "t={t} w={width} 总行数恒定");
                for row in &rows {
                    assert!(
                        display_width(row) <= width as usize,
                        "t={t} w={width} 溢出：{row:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn boot_logs_type_top_down_after_bars() {
        // A 115-column row at width 40 wraps into 3 segments: typing budgets count visual rows (never logical rows).
        let logs = VecDeque::from([
            LogLine::new(LogLevel::Info, "test", &"a".repeat(100)),
            LogLine::new(LogLevel::Info, "test", &"b".repeat(100)),
            LogLine::new(LogLevel::Info, "test", &"c".repeat(100)),
        ]);
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let frame_at = |elapsed_ms: u64| {
            let mut frame = sample_frame(
                &logs,
                &editor,
                &header,
                "",
                &stats,
                ServerPhase::Starting,
                elapsed_ms,
                40,
                24,
            );
            frame.boot = boot_frame(elapsed_ms);
            frame
        };
        // Mid-stretch (t=100): log area stays blank (content waits for the fill).
        let (rows, _) = build_frame(&frame_at(100));
        for row in rows.iter().skip(HEADER_ROWS).take(3) {
            assert!(
                crate::ansi::plain_text(row).trim().is_empty(),
                "伸展中日志空白：{row:?}"
            );
        }
        // Mid-typing (t=775 gives title_t=0.5): 9 visual rows take budgets [40,40,40,40,20,0,0,0,0].
        let (rows, _) = build_frame(&frame_at(775));
        assert_eq!(
            display_width(&crate::ansi::plain_text(&rows[HEADER_ROWS])),
            40,
            "首段已打满"
        );
        assert_eq!(
            display_width(&crate::ansi::plain_text(&rows[HEADER_ROWS + 2])),
            35,
            "首行末段全显（35 列）"
        );
        assert_eq!(
            display_width(&crate::ansi::plain_text(&rows[HEADER_ROWS + 4])),
            20,
            "次行次段打一半"
        );
        assert!(
            crate::ansi::plain_text(&rows[HEADER_ROWS + 5])
                .trim()
                .is_empty(),
            "后面的子行还没轮到"
        );
        // Boot done (boot=None): all nine segments show at once (no more typewriter afterwards).
        let steady = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Starting,
            BOOT_TOTAL_MS,
            40,
            24,
        );
        let (rows, _) = build_frame(&steady);
        let widths: Vec<usize> = rows
            .iter()
            .skip(HEADER_ROWS)
            .take(9)
            .map(|r| display_width(&crate::ansi::plain_text(r)))
            .collect();
        assert_eq!(
            widths,
            vec![40, 40, 35, 40, 40, 35, 40, 40, 35],
            "稳态九段全显：{widths:?}"
        );
    }

    #[test]
    fn find_matches_is_case_insensitive_and_cjk_safe() {
        // "12:00:00 INFO  Hello": prefix takes 15 columns, Hello sits at 15..20.
        assert_eq!(
            find_matches("12:00:00 INFO  Hello World", "hello"),
            vec![(15, 20)]
        );
        assert_eq!(
            find_matches("12:00:00 INFO  Hello World", "O W"),
            vec![(19, 22)]
        );
        // Each wide char takes 2 columns: two chars cover (2, 6).
        assert_eq!(find_matches("你好世界", "好世"), vec![(2, 6)]);
        // Non-overlapping hits plus empty-query and no-hit cases.
        assert_eq!(find_matches("aa aa", "aa"), vec![(0, 2), (3, 5)]);
        assert!(find_matches("anything", "").is_empty());
        assert!(find_matches("abc", "z").is_empty());
    }

    #[test]
    fn highlight_search_ranges_marks_current_differently() {
        let row = "stop the server stop";
        let ranges = [(0, 4, true), (16, 20, false)];
        // Steady (None): current match uses green plus gold, plain matches use pale blue.
        let out = highlight_search_ranges(row, &ranges, true, None);
        assert!(
            out.contains("\x1b[48;2;56;142;60m"),
            "当前命中绿底：{out:?}"
        );
        assert!(
            out.contains("\x1b[38;2;255;213;79m"),
            "当前命中金字：{out:?}"
        );
        assert!(out.contains("\x1b[104m"), "普通命中淡蓝底");
        assert_eq!(crate::ansi::plain_text(&out), row, "高亮零宽，不改文本");
        // Empty ranges and non-styled environments return unchanged.
        assert_eq!(highlight_search_ranges(row, &[], true, None), row);
        assert_eq!(
            highlight_search_ranges(row, &ranges, false, None),
            row,
            "NO_COLOR 不染色"
        );
    }

    #[test]
    fn highlight_search_current_fades_and_shines() {
        let row = "stop the server stop";
        let ranges = [(0, 4, true)];
        let steady = highlight_search_ranges(row, &ranges, true, None);
        // Fade start (0ms): dark-green background; text already carries shine glow (not exact gold yet).
        let out = highlight_search_ranges(row, &ranges, true, Some(0));
        assert!(out.contains("\x1b[48;2;18;52;20m"), "暗绿起播：{out:?}");
        assert_ne!(out, steady, "动画首帧即与稳态不同");
        // Mid-shine (250ms): differs from steady while the background stays truecolor green.
        let out = highlight_search_ranges(row, &ranges, true, Some(250));
        assert_ne!(out, steady, "扫光中与稳态不同");
        assert!(out.contains("\x1b[48;2;"), "背景真彩");
        assert_eq!(crate::ansi::plain_text(&out), row, "动画零宽，不改文本");
        // Past expiry uses steady (matches the driver cutoff): byte-identical with None.
        assert_eq!(
            highlight_search_ranges(row, &ranges, true, Some(9999)),
            steady
        );
    }

    fn search_test_frame<'a>(
        logs: &'a VecDeque<LogLine>,
        editor: &'a EditorState,
        header: &'a ConsoleHeader,
        stats: &'a ConsoleStats,
        hits: &'a [SearchMatch],
        current: usize,
        query: &'a str,
        cursor: usize,
        width: u16,
        height: u16,
    ) -> Frame<'a> {
        let mut frame = sample_frame(
            logs,
            editor,
            header,
            "",
            stats,
            ServerPhase::Running,
            0,
            width,
            height,
        );
        frame.search = Some(SearchCtx {
            matches: hits,
            current,
            query,
            cursor,
            current_age_ms: None,
        });
        frame
    }

    #[test]
    fn search_bar_shows_query_counts_and_cursor() {
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let hits = [
            SearchMatch {
                seq: 0,
                x0: 0,
                x1: 3,
            },
            SearchMatch {
                seq: 5,
                x0: 2,
                x1: 6,
            },
        ];
        let frame = search_test_frame(&logs, &editor, &header, &stats, &hits, 1, "stop", 2, 80, 24);
        let (rows, cursor) = build_frame(&frame);
        let status = crate::ansi::plain_text(rows.last().map(String::as_str).unwrap_or_default());
        assert!(status.starts_with("查找: stop 2/2"), "搜索框：{status:?}");
        assert!(status.ends_with("补全:static"), "右侧状态保留：{status:?}");
        // Cursor after the 2nd query char: column is prefix 6 plus "st" width 2 = 8, on last row 23.
        assert_eq!(cursor, (8, 23));
        // Empty query prompts for keywords; a query with no hits reports no match.
        let frame = search_test_frame(&logs, &editor, &header, &stats, &[], 0, "", 0, 80, 24);
        let (rows, _) = build_frame(&frame);
        assert!(crate::ansi::plain_text(&rows[23]).contains("输入关键字"));
        let frame = search_test_frame(&logs, &editor, &header, &stats, &[], 0, "zzz", 3, 80, 24);
        let (rows, _) = build_frame(&frame);
        assert!(crate::ansi::plain_text(&rows[23]).contains("无匹配"));
    }

    #[test]
    fn search_highlights_log_rows_without_changing_text() {
        let logs = VecDeque::from([LogLine::new(LogLevel::Info, "test", "stop the server stop")]);
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let plain = log_row_plain(&logs[0]);
        let hits: Vec<SearchMatch> = find_matches(&plain, "stop")
            .into_iter()
            .map(|(x0, x1)| SearchMatch { seq: 0, x0, x1 })
            .collect();
        assert_eq!(hits.len(), 2, "两个 stop 都命中：{plain:?}");
        let query = "stop";
        let frame = search_test_frame(&logs, &editor, &header, &stats, &hits, 0, query, 4, 80, 24);
        let (rows, _) = build_frame(&frame);
        let row = rows.get(HEADER_ROWS).cloned().unwrap_or_default();
        assert!(
            row.contains("\x1b[48;2;56;142;60m"),
            "首个命中是当前（绿底）"
        );
        assert!(
            row.contains("\x1b[38;2;255;213;79m"),
            "当前命中金字：{row:?}"
        );
        assert!(row.contains("\x1b[104m"), "第二个命中淡蓝底");
        // Highlighting never changes text (asserts log content with the panel stripped; content pads by width).
        let content = crate::ansi::plain_text(&row)
            .split('│')
            .next()
            .unwrap_or("")
            .trim_end()
            .to_string();
        assert_eq!(content, plain.trim_end(), "高亮不改文本");
    }

    #[test]
    fn search_bar_stays_within_terminal_width() {
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let hits = [SearchMatch {
            seq: 0,
            x0: 0,
            x1: 3,
        }];
        let long_query = "这是一个很长的查询串 mixed with ascii 0123456789";
        for width in [20u16, 32, 40, 80, 160] {
            let frame = search_test_frame(
                &logs, &editor, &header, &stats, &hits, 0, long_query, 999, width, 24,
            );
            let (rows, cursor) = build_frame(&frame);
            let status =
                crate::ansi::plain_text(rows.last().map(String::as_str).unwrap_or_default());
            assert_eq!(display_width(&status), width as usize, "{status:?}");
            assert!(status.starts_with("查找:"), "前缀保留：{status:?}");
            assert!(cursor.0 < width, "光标不越界");
            assert_eq!(cursor.1, 23);
        }
    }

    #[test]
    fn popup_row_anim_born_slides_and_fades() {
        // With n=3 the birth times (p space) are 0, 0.233, 0.467.
        // Steady shows everything with no offset at full brightness; empty popups stay invisible; the first row is unborn on frame one.
        assert_eq!(popup_row_anim(None, 2, 3), (true, 0, 1.0));
        assert_eq!(popup_row_anim(Some(0), 0, 0), (false, 0, 0.0));
        assert_eq!(popup_row_anim(Some(0), 0, 3), (false, 0, 0.0));
        // At t=62 row 1 is just born (local near 0.083): large offset 6 plus low opacity.
        let (vis, indent, op) = popup_row_anim(Some(62), 1, 3);
        assert!(vis, "已出生");
        assert_eq!(indent, 6, "从远处滑入");
        assert!((0.15..0.35).contains(&op), "渐显中：{op}");
        // At the same time row 0 already settled (opacity near 0.997, not yet full).
        let (vis, indent, op) = popup_row_anim(Some(62), 0, 3);
        assert!(vis);
        assert_eq!(indent, 0);
        assert!(op > 0.99, "op={op}");
        // At t=80 row 0 finished sliding (local clamps to 1, exact math).
        assert_eq!(popup_row_anim(Some(80), 0, 3), (true, 0, 1.0));
        // Finished (past 240ms) matches steady, so no extra repaint is needed.
        assert_eq!(popup_row_anim(Some(POPUP_APPEAR_MS), 2, 3), (true, 0, 1.0));
        assert_eq!(popup_row_anim(Some(5000), 0, 5), (true, 0, 1.0));
        // Visibility falls monotonically with row number at the same time: at t=20 only the first row peeks out.
        let vis: Vec<bool> = (0..5).map(|i| popup_row_anim(Some(20), i, 5).0).collect();
        assert_eq!(vis, vec![true, false, false, false, false]);
    }

    #[test]
    fn animated_popup_count_grows_upward() {
        assert_eq!(animated_popup_count(None, 5), 5, "稳态全高");
        assert_eq!(animated_popup_count(Some(0), 5), 0, "首帧 0 行");
        assert_eq!(animated_popup_count(Some(POPUP_APPEAR_MS), 5), 5, "播完");
        assert_eq!(animated_popup_count(Some(5000), 3), 3);
        assert_eq!(animated_popup_count(Some(90), 0), 0);
        // At t=90 with 9 candidates p=0.375 shows 5 rows (never jumps full at once).
        assert_eq!(animated_popup_count(Some(90), 9), 5);
        // Grows monotonically, never past the target.
        let mut prev = 0;
        let mut t = 0;
        while t < POPUP_APPEAR_MS {
            let k = animated_popup_count(Some(t), 9);
            assert!(k >= prev && k <= 9, "t={t} k={k}");
            prev = k;
            t += 7;
        }
    }

    #[test]
    fn popup_pops_upward_with_slide_and_fade() {
        use crate::completion::CompletionItem;
        use crate::editor::CompletionState;
        fn popup_frame<'a>(
            logs: &'a VecDeque<LogLine>,
            editor: &'a EditorState,
            header: &'a ConsoleHeader,
            stats: &'a ConsoleStats,
            age: Option<u64>,
        ) -> Frame<'a> {
            let mut frame = sample_frame(
                logs,
                editor,
                header,
                "",
                stats,
                ServerPhase::Running,
                0,
                80,
                24,
            );
            frame.popup_age_ms = age;
            frame
        }
        let logs = VecDeque::new();
        let mut editor = EditorState::new();
        editor.completion = Some(CompletionState {
            items: vec![
                CompletionItem::new("stop", "stop", "Stops the server", false),
                CompletionItem::new("start", "start", "Starts it", false),
                CompletionItem::new("status", "status", "Shows status", false),
            ],
            selected: 0,
        });
        let header = test_header();
        let stats = test_stats();
        // Content-area text (strips the right player panel suffix).
        let content_of = |row: &String| {
            crate::ansi::plain_text(row)
                .split('│')
                .next()
                .unwrap_or("")
                .to_string()
        };
        // height=24 with no logs and 3 candidates: steady popup takes rows 15/16/17 (input starts at 18).
        let base = 24 - INPUT_ROWS - STATUS_ROWS - 3;
        // First frame: 0 visible rows, so rows 15/16/17 stay log-area blanks (total rows constant).
        let (rows, _) = build_frame(&popup_frame(&logs, &editor, &header, &stats, Some(0)));
        assert_eq!(rows.len(), 24);
        for row in rows.iter().skip(base).take(3) {
            assert!(content_of(row).trim().is_empty(), "首帧占位：{row:?}");
        }
        // Mid-play (t=60): 2 visible rows take 16/17 (growing upward); first row full bright, second sliding plus fading.
        let (rows, _) = build_frame(&popup_frame(&logs, &editor, &header, &stats, Some(60)));
        assert!(
            content_of(&rows[base]).trim().is_empty(),
            "第 15 行还是日志区"
        );
        assert!(
            crate::ansi::plain_text(&rows[base + 1]).contains("stop"),
            "首行已出"
        );
        let mid_plain = crate::ansi::plain_text(&rows[base + 2]);
        assert!(mid_plain.contains("start"), "次行已出：{mid_plain:?}");
        assert!(rows[base + 2].contains("38;2;"), "次行滑入中带真彩渐显");
        assert!(
            mid_plain.starts_with("    "),
            "次行自远处滑入（缩进未归位）：{mid_plain:?}"
        );
        // Steady: all three rows fully shown.
        let (rows, _) = build_frame(&popup_frame(&logs, &editor, &header, &stats, None));
        let joined = rows
            .iter()
            .skip(base)
            .take(3)
            .map(|r| crate::ansi::plain_text(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("stop") && joined.contains("start") && joined.contains("status"),
            "稳态全显：{joined:?}"
        );
    }

    #[test]
    fn wrapped_follow_bottom_shows_tail_of_last_page() {
        // 20 long logs (119 columns each become 3 sub-rows); 40x24 follows the bottom:
        // the tail 16 visual rows are the log14 tail (39 columns) plus complete log15..19.
        // (A single-row layout would start at log4 with timestamped whole rows; this test pins the wrapped form.)
        let logs: VecDeque<LogLine> = (0..20)
            .map(|i| {
                LogLine::new(
                    LogLevel::Info,
                    "test",
                    &format!("L{i:02}-{}", "x".repeat(100)),
                )
            })
            .collect();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            40,
            24,
        );
        let (rows, _) = build_frame(&frame);
        assert_eq!(rows.len(), 24);
        let plains: Vec<String> = rows
            .iter()
            .skip(HEADER_ROWS)
            .take(16)
            .map(|r| crate::ansi::plain_text(r))
            .collect();
        // First row: log14 tail of 39 columns (all body text, no timestamp, no indent).
        assert_eq!(plains[0], "x".repeat(39));
        // Next row: the new log starts whole (with timestamp and label).
        assert!(plains[1].contains("L15-"), "换行后新日志：{:?}", plains[1]);
        assert!(plains[1].starts_with(char::is_numeric), "时间戳开头");
        // Last row: log19 tail with no fill rows replacing it (all 16 rows hold content).
        assert_eq!(plains[15], "x".repeat(39));
    }

    #[test]
    fn log_wrap_height_counts_visual_rows() {
        let line = |msg: &str| LogLine::new(LogLevel::Info, "test", msg);
        // Row-head prefix "HH:MM:SS INFO  " takes 15 columns.
        assert_eq!(log_wrap_height(&line("hi"), 80), 1);
        assert_eq!(
            log_wrap_height(&line(&"a".repeat(65)), 80),
            1,
            "15+65 恰满一行"
        );
        assert_eq!(
            log_wrap_height(&line(&"a".repeat(66)), 80),
            2,
            "超 1 列即折行"
        );
        // Each wide char takes 2 columns: 15 plus 33 wide chars makes 2 rows.
        assert_eq!(log_wrap_height(&line(&"中".repeat(33)), 80), 2);
        // Empty messages still carry the prefix, always at least 1 row.
        assert_eq!(log_wrap_height(&line(""), 80), 1);
    }

    #[test]
    fn wrapped_layout_snaps_follow_bottom_and_anchors_frozen() {
        // Heights [2,2,1] with capacity 4 (height=12): follow-bottom shows the tail 4 visual rows,
        // with only the second sub-row of row 0 exposed at the top.
        let heights = [2usize, 2, 1];
        let h = |i: usize| heights[i];
        let layout = log_layout_wrapped(12, 0, 3, &h, 0, ScrollView::FollowBottom);
        assert!(layout.valid);
        assert_eq!(layout.log_rows, 4);
        assert_eq!(
            (layout.start, layout.first_sub, layout.end, layout.up),
            (0, 1, 3, 0)
        );
        // Frozen review anchors the logical row head: top_seq=2 fills 3 visual rows forward, so start=2;
        // the last row shows partially (end includes partial rows) and up counts logical rows.
        let heights = [2usize, 2, 2, 2, 2];
        let h = |i: usize| heights[i];
        let layout = log_layout_wrapped(12, 0, 5, &h, 0, ScrollView::Frozen { top_seq: 2 });
        assert_eq!(
            (layout.start, layout.first_sub, layout.end, layout.up),
            (2, 0, 4, 1)
        );
        // New logs never push a frozen review (total 5 to 6 keeps start at 2).
        let heights = [2usize, 2, 2, 2, 2, 2];
        let h = |i: usize| heights[i];
        let layout = log_layout_wrapped(12, 0, 6, &h, 0, ScrollView::Frozen { top_seq: 2 });
        assert_eq!((layout.start, layout.first_sub, layout.end), (2, 0, 4));
        // Anchors near the bottom clamp down to the full page (never leaves a gap): top_seq=4 gives start=3.
        let layout = log_layout_wrapped(12, 0, 5, &h, 0, ScrollView::Frozen { top_seq: 4 });
        assert_eq!((layout.start, layout.first_sub), (3, 0));
    }

    #[test]
    fn wrapped_scroll_moves_by_visual_rows() {
        // Heights [1,5,1,1] (second row is a 5-sub-row block), capacity 4 (height=12).
        let heights = [1usize, 5, 1, 1];
        let h = |i: usize| heights[i];
        // Bottom top sits at (1,3): scrolling up 1 visual row lands mid-row, so anchor the row head.
        assert_eq!(
            scroll_view_wrapped(12, 0, 4, &h, 0, ScrollView::FollowBottom, 1),
            ScrollView::Frozen { top_seq: 1 }
        );
        // Scrolling up 1 while reviewing moves top visual row 1 to 0, landing on log 0.
        assert_eq!(
            scroll_view_wrapped(12, 0, 4, &h, 0, ScrollView::Frozen { top_seq: 1 }, 1),
            ScrollView::Frozen { top_seq: 0 }
        );
        // Scrolling further at the top never accumulates (stays 0, so scrolling back responds at once).
        assert_eq!(
            scroll_view_wrapped(12, 0, 4, &h, 0, ScrollView::Frozen { top_seq: 0 }, 5),
            ScrollView::Frozen { top_seq: 0 }
        );
        // Scrolling down to the bottom returns to follow mode.
        assert_eq!(
            scroll_view_wrapped(12, 0, 4, &h, 0, ScrollView::Frozen { top_seq: 0 }, -10),
            ScrollView::FollowBottom
        );
    }

    #[test]
    fn wrapped_scroll_down_escapes_tall_top_log() {
        // A 5-sub-row top log with wheel step 3 would land inside the same row,
        // and naive anchoring would pin the same row forever. Heights [5,1,1,1,1], capacity 4.
        let heights = [5usize, 1, 1, 1, 1];
        let h = |i: usize| heights[i];
        // Scrolling down 3 from Frozen{0} lands visual row 3 still inside log 0, so advance to log 1.
        assert_eq!(
            scroll_view_wrapped(12, 0, 5, &h, 0, ScrollView::Frozen { top_seq: 0 }, -3),
            ScrollView::Frozen { top_seq: 1 }
        );
        // Scrolling down again reaches the bottom and follows it (never gets stuck).
        assert_eq!(
            scroll_view_wrapped(12, 0, 5, &h, 0, ScrollView::Frozen { top_seq: 1 }, -3),
            ScrollView::FollowBottom
        );
    }

    #[test]
    fn wrapped_scroll_down_follows_tail_when_no_full_page_left() {
        // Heights [10,1,1,1] (first row has 10 sub-rows), capacity 4: scrolling down inside the first row
        // with no full page left follows the bottom directly (shows the row tail) instead of stalling.
        let heights = [10usize, 1, 1, 1];
        let h = |i: usize| heights[i];
        assert_eq!(
            scroll_view_wrapped(12, 0, 4, &h, 0, ScrollView::Frozen { top_seq: 0 }, -3),
            ScrollView::FollowBottom
        );
    }

    #[test]
    fn hit_test_maps_wrapped_subrows_to_global_columns() {
        // Heights [3,1], capacity 4: both rows fully visible; screen row 3 is sub-row 1 of log 0.
        let heights = [3usize, 1];
        let h = |i: usize| heights[i];
        let layout = log_layout_wrapped(12, 0, 2, &h, 0, ScrollView::FollowBottom);
        assert_eq!((layout.start, layout.first_sub, layout.end), (0, 0, 2));
        // At width 80 a continuation column 7 means global column 87.
        assert!(matches!(
            hit_test(&layout, &h, 80, 7, HEADER_ROWS + 1),
            Hit::Log { idx: 0, x: 87 }
        ));
        // Next logical row head: global columns unchanged.
        assert!(matches!(
            hit_test(&layout, &h, 80, 7, HEADER_ROWS + 3),
            Hit::Log { idx: 1, x: 7 }
        ));
    }

    #[test]
    fn side_panel_shows_players_without_overlapping_chrome() {
        // Width 100 shows the panel (22 columns) with a 78-column content area; title/input/status stay full width.
        let logs = VecDeque::from([LogLine::new(LogLevel::Info, "test", "hello")]);
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let players = vec!["Steve".to_string(), "AlexandraTheGreat".to_string()];
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            100,
            24,
        );
        frame.players = &players;
        let (rows, _) = build_frame(&frame);
        assert_eq!(rows.len(), 24);
        for row in &rows {
            assert_eq!(display_width(row), 100, "每行占满整宽：{row:?}");
        }
        // Panel title plus player names take the first three rows; log content stays on the left.
        let plain0 = crate::ansi::plain_text(&rows[HEADER_ROWS]);
        assert!(plain0.contains("hello"), "日志内容保留");
        assert!(plain0.contains("在线玩家(2)"), "标题计数：{plain0:?}");
        assert!(plain0.contains('│'), "面板分隔符");
        // Panel background runs one shade darker than title/input (22 vs 38) and differs from input background.
        let raw0 = &rows[HEADER_ROWS];
        assert!(raw0.contains("\x1b[48;2;22;22;22m"), "面板深灰底：{raw0:?}");
        assert!(
            !raw0.contains("\x1b[48;2;38;38;38m"),
            "面板不用输入框底色：{raw0:?}"
        );
        let plain1 = crate::ansi::plain_text(&rows[HEADER_ROWS + 1]);
        assert!(plain1.contains("Steve"), "玩家名：{plain1:?}");
        let plain2 = crate::ansi::plain_text(&rows[HEADER_ROWS + 2]);
        assert!(plain2.contains("AlexandraTheGreat"), "长名完整：{plain2:?}");
        // Title/input/status rows never enter the panel (no separator, no player names).
        for row in rows.iter().take(HEADER_ROWS) {
            let plain = crate::ansi::plain_text(row);
            assert!(!plain.contains('│'), "标题栏整宽：{plain:?}");
            assert!(!plain.contains("Steve"));
        }
        let input_rows = &rows[24 - INPUT_ROWS - STATUS_ROWS..24 - STATUS_ROWS];
        for row in input_rows {
            let plain = crate::ansi::plain_text(row);
            assert!(!plain.contains('│'), "输入框整宽：{plain:?}");
        }
        assert!(
            !crate::ansi::plain_text(rows.last().unwrap()).contains('│'),
            "状态行整宽"
        );
    }

    #[test]
    fn side_panel_hides_on_narrow_terminal() {
        // Width 60 (below 70) shows no panel: logs take the full width with no separator.
        let logs = VecDeque::from([LogLine::new(LogLevel::Info, "test", "hello")]);
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let players = vec!["Steve".to_string()];
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            60,
            24,
        );
        frame.players = &players;
        let (rows, _) = build_frame(&frame);
        let joined = rows.join("\n");
        assert!(!joined.contains('│'), "窄窗无面板");
        assert!(!joined.contains("Steve"), "窄窗不显示玩家名");
        assert!(joined.contains("hello"));
    }

    #[test]
    fn side_panel_truncates_long_names_and_shows_empty_state() {
        // Overlong names truncate to 21 columns; an empty list shows a placeholder with count 0.
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let players = vec!["这是一个非常非常长的玩家名字0123456789".to_string()];
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            100,
            24,
        );
        frame.players = &players;
        let (rows, _) = build_frame(&frame);
        let plain1 = crate::ansi::plain_text(&rows[HEADER_ROWS + 1]);
        assert!(!plain1.contains("0123456789"), "超长截断：{plain1:?}");
        assert!(plain1.contains("非常"), "保留前部：{plain1:?}");
        // Empty list.
        let empty: Vec<String> = vec![];
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            100,
            24,
        );
        frame.players = &empty;
        let (rows, _) = build_frame(&frame);
        let joined = rows.join("\n");
        assert!(joined.contains("在线玩家(0)"), "空列表计数");
        assert!(joined.contains("暂无玩家在线"), "空列表占位");
    }

    #[test]
    fn side_panel_clicks_do_nothing_and_wrapping_uses_content_width() {
        // Clicks in the panel area are no-ops; wrapping uses the content width (100-22=78).
        let heights = [1usize, 1];
        let h = |i: usize| heights[i];
        let layout = log_layout_wrapped(24, 0, 2, &h, 0, ScrollView::FollowBottom);
        // A log row hit inside the content area.
        assert!(matches!(
            hit_test(&layout, &h, 78, 10, HEADER_ROWS),
            Hit::Log { idx: 0, x: 10 }
        ));
        // Clicks in the panel area (x from 78) are no-ops for both log and hint rows.
        assert_eq!(hit_test(&layout, &h, 78, 78, HEADER_ROWS), Hit::None);
        assert_eq!(hit_test(&layout, &h, 78, 90, HEADER_ROWS), Hit::None);
        // A 115-column row wraps into 2 segments (78+37) at content width 78 instead of staying whole at width 100.
        let logs = VecDeque::from([LogLine::new(LogLevel::Info, "test", &"z".repeat(100))]);
        assert_eq!(log_wrap_height(&logs[0], 78), 2);
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            100,
            24,
        );
        frame.players = &[];
        let (rows, _) = build_frame(&frame);
        let row0 = crate::ansi::plain_text(&rows[HEADER_ROWS]);
        let row1 = crate::ansi::plain_text(&rows[HEADER_ROWS + 1]);
        assert_eq!(display_width(&row0), 100, "面板行占满整宽");
        let content1: String = row0.chars().take(78).collect();
        assert_eq!(display_width(&content1), 78);
        assert!(
            row1.starts_with('z') || row1.trim_start().starts_with('z'),
            "续行：{row1:?}"
        );
    }

    #[test]
    fn side_panel_slides_in_from_right_during_boot() {
        // While the boot bars stretch (0-450ms), the panel slides leftward from the right edge to dock;
        // the log area always lays out at its final width (no reflow).
        let logs = VecDeque::new();
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let players = vec!["Steve".to_string()];
        let frame_at = |elapsed_ms: u64| {
            let mut frame = boot_test_frame(&logs, &editor, &header, &stats, elapsed_ms, 100, 24);
            frame.players = &players;
            frame
        };
        // t=0: panel fully off the right edge (no separator).
        let (rows, _) = build_frame(&frame_at(0));
        assert!(!rows.join("\n").contains('│'), "面板未进入");
        // t=225 (mid-slide, eased 0.875): offset 3 columns with the separator at column 81.
        let (rows, _) = build_frame(&frame_at(225));
        let plain = crate::ansi::plain_text(&rows[HEADER_ROWS]);
        assert_eq!(plain.find('│'), Some(78 + 3), "面板滑入中：{plain:?}");
        assert_eq!(display_width(&plain), 100);
        // t=500 (bars filled): panel docked, separator back at column 78, title complete.
        let (rows, _) = build_frame(&frame_at(500));
        let plain = crate::ansi::plain_text(&rows[HEADER_ROWS]);
        assert_eq!(plain.find('│'), Some(78));
        assert!(plain.contains("在线玩家(1)"));
        assert_eq!(display_width(&plain), 100);
    }

    #[test]
    fn long_logs_wrap_instead_of_truncate() {
        // A 115-column row at width 40 wraps into 3 continuation segments (no indent) with the tail visible; total rows stay height.
        let logs = VecDeque::from([LogLine::new(LogLevel::Info, "test", &"z".repeat(100))]);
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            40,
            24,
        );
        let (rows, _) = build_frame(&frame);
        assert_eq!(rows.len(), 24);
        let plains: Vec<String> = rows
            .iter()
            .skip(HEADER_ROWS)
            .take(3)
            .map(|r| crate::ansi::plain_text(r))
            .collect();
        assert_eq!(display_width(&plains[0]), 40);
        assert!(plains[0].ends_with("zzz"));
        assert_eq!(display_width(&plains[2]), 35);
        assert!(plains[2].starts_with("zzz"), "续行无缩进：{:?}", plains[2]);
        // Concatenated segments equal the whole row (no lost or duplicated chars).
        let full = log_row_plain(&logs[0]);
        assert_eq!(plains.concat(), full);
    }

    #[test]
    fn selection_highlights_wrapped_subrows_in_global_columns() {
        // A 115-column row (40 wide gives sub-rows 40/40/35): selection (0, 70..90) spans the last two segments.
        let logs = VecDeque::from([LogLine::new(LogLevel::Info, "test", &"a".repeat(100))]);
        let editor = EditorState::new();
        let header = test_header();
        let stats = test_stats();
        let mut frame = sample_frame(
            &logs,
            &editor,
            &header,
            "",
            &stats,
            ServerPhase::Running,
            0,
            40,
            24,
        );
        let mut sel = Selection::point(0, 70);
        sel.extend(0, 90);
        frame.selection = Some(sel);
        let mut buf = Vec::new();
        render(&mut buf, &frame).expect("render");
        let raw = String::from_utf8(buf).expect("utf8");
        let row0 = raw.split("\r\n").nth(HEADER_ROWS).unwrap_or("");
        let row1 = raw.split("\r\n").nth(HEADER_ROWS + 1).unwrap_or("");
        let row2 = raw.split("\r\n").nth(HEADER_ROWS + 2).unwrap_or("");
        // (70,90) over [40,80) is [70,80) so in-row (30,40); over [80,120) is [80,90) so (0,10).
        assert!(!row0.contains("\x1b[7m"), "首段不在选区");
        assert!(row1.contains("\x1b[7m"), "次段相交高亮");
        assert!(row2.contains("\x1b[7m"), "末段相交高亮");
        // Highlights are zero width: each segment plain text is its column range.
        assert_eq!(crate::ansi::plain_text(row1), "a".repeat(40));
        assert_eq!(crate::ansi::plain_text(row2), "a".repeat(35));
    }
}
