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

// ================= Boot animation =================

/// Boot bar stretch duration (ms): title rows and input rows stretch to the edges from the left in parallel.
pub(crate) const BOOT_BAR_MS: u64 = 450;
/// Title typing plus CPU/MEM fade-in duration (ms): starts after the bars fill.
pub(crate) const BOOT_TITLE_MS: u64 = 650;
/// Input content fade-in duration (ms): starts once the bars fill (in parallel with typing, never waits for the title).
pub(crate) const BOOT_INPUT_FADE_MS: u64 = 450;
/// Total boot duration (ms): returns to the steady boot state when done (~1.1s).
pub(crate) const BOOT_TOTAL_MS: u64 = BOOT_BAR_MS + BOOT_TITLE_MS; // 1100

/// Boot animation frame (plain value; assembled by the driver from boot age, read-only for rendering).
///
/// Timeline: `header_expand`/`input_expand` (0 to 1, eased, stretching in parallel),
/// then `title_t` (title typing progress) advances together with `meta_opacity` (CPU/MEM fade),
/// while `input_opacity` (input content fade) runs in parallel (never waits for the title).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BootFrame {
    /// Title bar stretch 0..1 (eased).
    pub header_expand: f32,
    /// Input bar stretch 0..1 (eased, in parallel with the title bar).
    pub input_expand: f32,
    /// Title typing progress 0..1 (linear, starts after the bars fill).
    pub title_t: f32,
    /// CPU/MEM opacity 0..1 (fades in together with title typing).
    pub meta_opacity: f32,
    /// Input content opacity 0..1 (starts once the bars fill, never waits for the title).
    pub input_opacity: f32,
    /// Right panel slide-in progress 0..1 (eased, in parallel with the bar stretch; 1 means docked).
    pub panel_slide: f32,
}

/// Boot frame from boot age: `None` means finished (the caller returns to steady boot state).
///
/// Pure function (explicit clock) for unit tests; every progress clamps to 0..1, narrow terminals clamp columns.
pub(crate) fn boot_frame(elapsed_ms: u64) -> Option<BootFrame> {
    if elapsed_ms >= BOOT_TOTAL_MS {
        return None;
    }
    let bar_p = (elapsed_ms as f32 / BOOT_BAR_MS as f32).clamp(0.0, 1.0);
    let eased = ease_out_cubic(bar_p);
    let title_p =
        (elapsed_ms.saturating_sub(BOOT_BAR_MS) as f32 / BOOT_TITLE_MS as f32).clamp(0.0, 1.0);
    let input_p =
        (elapsed_ms.saturating_sub(BOOT_BAR_MS) as f32 / BOOT_INPUT_FADE_MS as f32).clamp(0.0, 1.0);
    Some(BootFrame {
        header_expand: eased,
        input_expand: eased,
        title_t: title_p,
        meta_opacity: smoothstep(title_p),
        input_opacity: smoothstep(input_p),
        // The panel slides in together with the bars (same easing, same window).
        panel_slide: eased,
    })
}

/// Boot log typewriter: after the bars fill, visible logs type out top to bottom.
///
/// Returns per-row displayable columns (`visible_rows` rows, advancing linearly with `title_t` for even typing):
/// the budget fills whole rows top to bottom; when done everything shows,
/// matching steady state with no jump. `title_t` is the linear typing progress shared with the title.
/// Wide-char safety comes from [`crate::ansi::truncate_styled`] (never splits a wide char).
///
/// Pure function (explicit progress) for unit tests.
pub(crate) fn boot_log_budgets(visible_rows: usize, width: usize, title_t: f32) -> Vec<usize> {
    let progress = title_t.clamp(0.0, 1.0);
    let total = visible_rows.saturating_mul(width);
    let revealed = (total as f32 * progress) as usize;
    (0..visible_rows)
        .map(|i| revealed.saturating_sub(i.saturating_mul(width)).min(width))
        .collect()
}

/// CPU/MEM label steady gray (near 256-color 245) and value steady light gray (near 250):
/// the boot fade interpolates from the background toward steady RGB, then switches to fixed colors (no jump).
const META_LABEL_RGB: Rgb = Rgb(138, 138, 138);
const META_VALUE_RGB: Rgb = Rgb(188, 188, 188);
/// Boot text steady light gray (fixed color 253).
const STARTUP_TEXT_RGB: Rgb = Rgb(218, 218, 218);

/// Stretching boot bar row: the first `bar_len` columns use the bar background, the rest keep the default
/// (transparent, never pre-painted), growing left to right; no text appears until the stretch fills.
fn emit_growing_bar(buf: &mut String, bar_len: usize, width: usize, styled: bool) {
    let bar_len = bar_len.min(width);
    if bar_len > 0 {
        push_bg_rgb(buf, BAR_RGB, styled);
        buf.push_str(&" ".repeat(bar_len));
    }
    if bar_len < width {
        // Unreached areas keep the terminal default background so rows never pre-fill.
        if styled {
            buf.push_str("\x1b[49m");
        }
        buf.push_str(&" ".repeat(width - bar_len));
    }
    push_reset(buf);
}

/// Boot title row (used once the bar fills): left text plus right `label + value` (right aligned),
/// with the right side fading in from the background by `meta` opacity (steady fixed colors when done).
#[allow(clippy::too_many_arguments)]
fn emit_boot_title_row(
    buf: &mut String,
    left: &str,
    left_fg: Fg,
    right_label: &str,
    right_value: &str,
    meta: f32,
    width: usize,
    styled: bool,
) {
    let left_w = display_width(left);
    let right_w = display_width(right_label) + display_width(right_value);
    let pad = " ".repeat(width.saturating_sub(left_w + right_w));
    let segs: Vec<Segment> = vec![
        (left, Some(left_fg)),
        (pad.as_str(), None),
        (
            right_label,
            Some(Fg::Rgb(mix_rgb(BAR_RGB, META_LABEL_RGB, meta))),
        ),
        (
            right_value,
            Some(Fg::Rgb(mix_rgb(BAR_RGB, META_VALUE_RGB, meta))),
        ),
    ];
    let bar = |_: usize, _: usize| BAR_RGB;
    emit_shaded_row(buf, &segs, &bar, 0, 0, width, styled);
}

/// Two boot title rows: light bars while stretching (no text); after filling, the first row types out
/// while CPU/MEM (including the second-row quote/MEM) fades in together.
#[allow(clippy::too_many_arguments)]
fn render_boot_header(
    buf: &mut String,
    boot: &BootFrame,
    core: &str,
    cpu_value: &str,
    sub_text: &str,
    mem_value: &str,
    width: usize,
    anim_ms: u64,
    styled: bool,
) {
    let bar_len = (width as f32 * boot.header_expand).round() as usize;
    if bar_len < width {
        emit_growing_bar(buf, bar_len, width, styled);
        buf.push_str("\r\n");
        emit_growing_bar(buf, bar_len, width, styled);
        return;
    }
    // Bars filled: the first row types out (the marker joins the typing first), using the breathing title color
    // (same function as the post-stretch steady boot state, so handoff is seamless); CPU fades in on the right.
    let full: Vec<char> = format!("⬢ {core}").chars().collect();
    let n = (full.len() as f32 * boot.title_t).ceil() as usize;
    let shown: String = full.iter().take(n.min(full.len())).collect();
    emit_boot_title_row(
        buf,
        &shown,
        Fg::Rgb(breath_fg(anim_ms)),
        "CPU ",
        cpu_value,
        boot.meta_opacity,
        width,
        styled,
    );
    buf.push_str("\r\n");
    // Second row: the quote (typewriter intermediate text, never retyped) fades in together with MEM.
    emit_boot_title_row(
        buf,
        sub_text,
        Fg::Rgb(mix_rgb(BAR_RGB, SUB_BASE, boot.meta_opacity)),
        "MEM ",
        mem_value,
        boot.meta_opacity,
        width,
        styled,
    );
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

// ================= Input wrapping =================

/// Wrap input chars by display width: first row budget `first`, remaining rows `rest`.
///
/// Returns one char vector per row (at least one row; char-boundary and wide-char safe; an overlong
/// single char takes its own row so iteration always terminates).
pub(crate) fn wrap_lines(chars: &[char], first: usize, rest: usize) -> Vec<Vec<char>> {
    let mut rows: Vec<Vec<char>> = vec![Vec::new()];
    let mut width = 0usize;
    let mut budget = first.max(1);
    for &c in chars {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if width + cw > budget && !rows.last().map(|r| r.is_empty()).unwrap_or(true) {
            rows.push(Vec::new());
            width = 0;
            budget = rest.max(1);
        }
        rows.last_mut().expect("rows").push(c);
        width += cw;
    }
    rows
}

/// Cursor char index to (visual row, chars before the cursor in that row).
fn locate_cursor(rows: &[Vec<char>], cursor: usize) -> (usize, usize) {
    let mut acc = 0usize;
    for (i, row) in rows.iter().enumerate() {
        if cursor <= acc + row.len() {
            return (i, cursor - acc);
        }
        acc += row.len();
    }
    let last = rows.len().saturating_sub(1);
    (last, rows.last().map(|r| r.len()).unwrap_or(0))
}

/// Display width of the first n chars in a row.
fn row_prefix_width(row: &[char], n: usize) -> usize {
    row.iter()
        .take(n)
        .map(|c| UnicodeWidthChar::width(*c).unwrap_or(0))
        .sum()
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

/// Mouse drag selection (auto-copies on release).
///
/// Endpoints anchor by log index (not screen rows): new logs shift screen rows down while
/// the selection still tracks the originally selected rows; `x` is a display column (wide chars count 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    anchor_idx: usize,
    anchor_x: usize,
    head_idx: usize,
    head_x: usize,
}

impl Selection {
    /// New origin (on press; `head = anchor`, a tap with no drag is a point).
    pub(crate) fn point(idx: usize, x: usize) -> Self {
        Self {
            anchor_idx: idx,
            anchor_x: x,
            head_idx: idx,
            head_x: x,
        }
    }

    /// Extend the drag (only moves `head`).
    pub(crate) fn extend(&mut self, idx: usize, x: usize) {
        self.head_idx = idx;
        self.head_x = x;
    }

    /// Whether nothing was dragged (a tap): taps never copy and never highlight.
    pub(crate) fn is_point(&self) -> bool {
        self.anchor_idx == self.head_idx && self.anchor_x == self.head_x
    }

    /// Normalize to (start row, start col, end row, end col) ordered row-first.
    pub(crate) fn normalize(&self) -> (usize, usize, usize, usize) {
        let (a_idx, a_x, h_idx, h_x) = (self.anchor_idx, self.anchor_x, self.head_idx, self.head_x);
        if (a_idx, a_x) <= (h_idx, h_x) {
            (a_idx, a_x, h_idx, h_x)
        } else {
            (h_idx, h_x, a_idx, a_x)
        }
    }

    /// Column range of one log row inside the selection (text-selection semantics: first row runs to the end,
    /// last row starts at the beginning, middle rows are whole; `None` means the row is outside).
    pub(crate) fn x_range_for(&self, idx: usize) -> Option<(usize, usize)> {
        let (lo, lo_x, hi, hi_x) = self.normalize();
        if idx < lo || idx > hi {
            return None;
        }
        if lo == hi {
            let (a, b) = if lo_x <= hi_x {
                (lo_x, hi_x)
            } else {
                (hi_x, lo_x)
            };
            return (a < b).then_some((a, b));
        }
        if idx == lo {
            Some((lo_x, usize::MAX))
        } else if idx == hi {
            (hi_x > 0).then_some((0, hi_x))
        } else {
            Some((0, usize::MAX))
        }
    }
}

/// Log viewport position.
///
/// Key semantic: while reviewing, anchor the absolute row number ([`Frozen`](Self::Frozen)) instead of
/// "rows from the bottom". A from-the-bottom offset would shift the viewed rows up as new logs arrive,
/// pushing away the rows being read. Anchoring the absolute index keeps the view still while
/// new logs only accumulate below (visible after returning to the bottom).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ScrollView {
    /// Follow the newest logs (not reviewing).
    #[default]
    FollowBottom,
    /// Anchored at one log global sequence `top_seq` (monotonic from 0, including evicted history);
    /// new logs and buffer eviction never move the view.
    Frozen { top_seq: u64 },
}

impl ScrollView {
    /// Whether currently reviewing (drives the hint and the Esc-to-bottom behavior).
    pub(crate) fn is_frozen(self) -> bool {
        matches!(self, ScrollView::Frozen { .. })
    }
}

/// Log area layout (shared by rendering and mouse hit-testing so both see the same rows).
///
/// Long logs wrap automatically: one logical log may take several visual rows (continuations have no indent).
/// `start/end/up` still count logical log indexes (identical to old behavior when every log is one row);
/// `first_sub` records the sub-row offset of the first visible log (only the tail may show at follow-bottom).
#[derive(Clone, Copy, Debug)]
pub(crate) struct LogLayout {
    /// Invalid when the terminal is too short (hit-testing ignores it).
    pub valid: bool,
    /// Completion popup rows and log area rows (visual rows).
    pub popup_rows: usize,
    pub log_rows: usize,
    /// First log-area row (screen row number).
    pub log_top: usize,
    /// Visible log indexes `[start, end)` (edge rows may show only some sub-rows).
    pub start: usize,
    pub end: usize,
    /// Clamped scroll amount (at most one full page to the very top).
    pub up: usize,
    /// Top sub-row offset: log `start` shows from its `first_sub`-th sub-row
    /// (always 0 when frozen; may exceed 0 at follow-bottom, showing only that row tail).
    pub first_sub: usize,
    /// Review hint rows (0/1): when scrolled off the bottom with no completion popup, one row just above
    /// the input box shows a centered "press Esc for bottom" hint and belongs to the log area.
    pub hint_rows: usize,
    /// First input-box row (screen row number).
    pub input_base: usize,
}

/// Log wrap height (visual rows): greedy plain-text fill.
///
/// Same break conditions as [`crate::ansi::split_styled_rows`] (zero-width styles, never splits wide chars,
/// an overwide single char takes its own overflowing row), always at least 1. OSC sequences inside messages
/// are dropped by the style splitter (zero width) but counted literally here, so extreme input may read tall;
/// the render layer pads with empty rows and never panics or drops later content (selection/copy uses plain text).
pub(crate) fn log_wrap_height(line: &LogLine, width: usize) -> usize {
    let width = width.max(1);
    let text = log_row_plain(line);
    let mut rows = 1usize;
    let mut col = 0usize;
    for c in text.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if col > 0 && col + cw > width {
            rows += 1;
            col = 0;
        }
        col += cw;
    }
    rows
}

/// Visual row total for indexes `[0, idx)` (heights evaluated on demand, only near the window).
fn visual_prefix(total: usize, height_of: &dyn Fn(usize) -> usize, idx: usize) -> usize {
    (0..idx.min(total)).map(height_of).sum()
}

/// Logical log index owning visual row `v` (`v` past the end clamps to the last entry).
fn log_at_visual(total: usize, height_of: &dyn Fn(usize) -> usize, v: usize) -> usize {
    let mut acc = 0usize;
    for i in 0..total {
        acc += height_of(i);
        if v < acc {
            return i;
        }
    }
    total.saturating_sub(1)
}

/// Start index that exactly fills `log_rows` visual rows from the end (shared by review clamp and follow-bottom).
///
/// Accumulates backward and stops once full; returns `(start, visual rows after start)`.
/// Returns `(0, total_visual)` when the logs fill less than one page. Only touches heights near the tail.
fn full_page_start(
    total: usize,
    height_of: &dyn Fn(usize) -> usize,
    log_rows: usize,
) -> (usize, usize) {
    let mut start = total;
    let mut acc = 0usize;
    while start > 0 && acc < log_rows {
        start -= 1;
        acc += height_of(start);
    }
    (start, acc)
}

/// End index after taking `log_rows` visual rows forward from `(start, first_sub)`
/// (includes the partially visible last row; `end` is the first invisible index).
fn end_from(
    total: usize,
    height_of: &dyn Fn(usize) -> usize,
    start: usize,
    first_sub: usize,
    log_rows: usize,
) -> usize {
    let mut acc = 0usize;
    let mut idx = start.min(total);
    if idx < total {
        acc += height_of(idx).saturating_sub(first_sub);
        idx += 1;
    }
    while idx < total && acc < log_rows {
        acc += height_of(idx);
        idx += 1;
    }
    idx
}

/// Log area layout (`build_frame` and driver hit-testing share this function, so row numbers always agree).
///
/// The viewport anchors absolute row numbers: new logs only grow `total`, while a frozen
/// `start` never moves, so the view is not pushed. Frozen anchors show the anchored row from its first
/// sub-row (`first_sub` is 0); follow-bottom may show only a row tail (`first_sub` above 0).
pub(crate) fn log_layout_wrapped(
    height: usize,
    popup_items: usize,
    total: usize,
    height_of: &dyn Fn(usize) -> usize,
    evicted: u64,
    view: ScrollView,
) -> LogLayout {
    let chrome = HEADER_ROWS + INPUT_ROWS + STATUS_ROWS; // 2+5+1
    if height <= chrome {
        return LogLayout {
            valid: false,
            popup_rows: 0,
            log_rows: 0,
            log_top: 0,
            start: 0,
            end: 0,
            up: 0,
            first_sub: 0,
            hint_rows: 0,
            input_base: 0,
        };
    }
    // Completion list rows: shrink to keep at least 1 log row; total rows always equal height.
    let popup_rows = popup_items.min(9).min(height.saturating_sub(chrome + 1));
    let base_log_rows = height.saturating_sub(chrome + popup_rows).max(1);
    // Whether the hint row exists changes `log_rows`, which changes the bottom row and bottom detection,
    // so iterate at most twice (the second pass always agrees).
    let mut hint_rows = 0usize;
    let mut log_rows = base_log_rows;
    let mut start = 0usize;
    let mut first_sub = 0usize;
    for _ in 0..2 {
        // Start index that fills the last page (review lower clamp; equals total minus rows when single-line).
        let (full_start, full_acc) = full_page_start(total, height_of, log_rows);
        let (req_start, req_sub) = match view {
            // Follow-bottom: fill from the end; the top may show only a row tail.
            ScrollView::FollowBottom => (full_start, full_acc.saturating_sub(log_rows)),
            // Frozen anchor: global sequence to current deque index (minus evicted rows),
            // then clamped down to the full-page start (never leaves a gap at the bottom); anchored rows show whole.
            ScrollView::Frozen { top_seq } => {
                let anchored = top_seq.saturating_sub(evicted);
                let anchored = usize::try_from(anchored).unwrap_or(usize::MAX);
                (anchored.min(full_start), 0)
            }
        };
        start = req_start;
        first_sub = req_sub;
        // Review hint: takes one log-area row when content sits below the viewport with no popup.
        // Skipped when only 1 row remains (never squeezes out the last log).
        let want_hint = usize::from(start < full_start && popup_rows == 0 && base_log_rows > 1);
        if want_hint == hint_rows {
            break;
        }
        hint_rows = want_hint;
        log_rows = base_log_rows - hint_rows;
    }
    let end = end_from(total, height_of, start, first_sub, log_rows);
    let up = total.saturating_sub(end);
    LogLayout {
        valid: true,
        popup_rows,
        log_rows,
        log_top: HEADER_ROWS,
        start,
        end,
        up,
        first_sub,
        hint_rows,
        input_base: height - INPUT_ROWS - STATUS_ROWS,
    }
}

/// Log area layout (single-line form: every logical log takes exactly one row, matching old behavior).
#[cfg(test)]
pub(crate) fn log_layout(
    height: usize,
    popup_items: usize,
    total: usize,
    evicted: u64,
    view: ScrollView,
) -> LogLayout {
    log_layout_wrapped(height, popup_items, total, &|_| 1, evicted, view)
}

/// Count of "new logs not yet scrolled past" while reviewing (the badge text).
///
/// Meaning: among arrived logs, those both below the viewport and past the freeze watermark:
/// - the freeze watermark excludes older reviewed rows (scrolling up a few rows must not invent dozens);
/// - below-the-viewport shrinks the count while scrolling back: fewer near the bottom,
///   exactly 0 at the bottom (badge disappears); newly arriving logs only grow it.
pub(crate) fn unseen_count(
    log_seq: u64,
    evicted: u64,
    end: usize,
    frozen_base_seq: Option<u64>,
) -> usize {
    let Some(base) = frozen_base_seq else {
        return 0;
    };
    // Global sequence of the first log below the viewport.
    let below = evicted.saturating_add(end as u64);
    let watermark = below.max(base);
    usize::try_from(log_seq.saturating_sub(watermark)).unwrap_or(usize::MAX)
}

/// Scroll the viewport `delta` rows (positive looks up at history, negative heads back down), clamped to geometry.
///
/// `delta` counts visual rows (including wrap sub-rows): scrolling past the very top stops accumulating
/// (scrolling back responds immediately); scrolling to the bottom returns to [`ScrollView::FollowBottom`].
/// Landing mid-row anchors that row start (shows one extra row head, never drops content).
/// Scrolling down that still lands in the top row (taller than the step) advances one row instead;
/// with no full page left it follows the bottom, so tall rows can never trap the wheel.
/// Heights are evaluated on demand (only near the window, never over the whole log).
/// Identical to old logic for all-single-row logs. A too-small window (invalid layout) keeps the view.
pub(crate) fn scroll_view_wrapped(
    height: usize,
    popup_items: usize,
    total: usize,
    height_of: &dyn Fn(usize) -> usize,
    evicted: u64,
    view: ScrollView,
    delta: i32,
) -> ScrollView {
    if delta == 0 {
        return view;
    }
    let layout = log_layout_wrapped(height, popup_items, total, height_of, evicted, view);
    if !layout.valid {
        return view;
    }
    // Current top visual row (frozen anchors the row head; follow-bottom includes the first-sub offset).
    let top_visual = visual_prefix(total, height_of, layout.start) + layout.first_sub;
    let total_visual = visual_prefix(total, height_of, total);
    let anchor = |start: usize| ScrollView::Frozen {
        top_seq: evicted.saturating_add(start as u64),
    };
    if delta > 0 {
        if total_visual <= layout.log_rows {
            // Everything fits in the viewport: nothing to review, stay in follow mode.
            return ScrollView::FollowBottom;
        }
        anchor(log_at_visual(
            total,
            height_of,
            top_visual.saturating_sub(delta as usize),
        ))
    } else {
        let new_top = top_visual.saturating_add(delta.unsigned_abs() as usize);
        if new_top + layout.log_rows >= total_visual {
            ScrollView::FollowBottom
        } else {
            let found = log_at_visual(total, height_of, new_top);
            let cur = log_at_visual(total, height_of, top_visual);
            if found > cur {
                anchor(found)
            } else {
                // Landing still inside the same top row: advance one row; follow the bottom when no page fits.
                let (full_start, _) = full_page_start(total, height_of, layout.log_rows);
                let next = (cur + 1).min(full_start);
                if next > cur {
                    anchor(next)
                } else {
                    ScrollView::FollowBottom
                }
            }
        }
    }
}

/// Viewport scroll (single-line form: `delta` counts logical rows with identical behavior; tests only).
#[cfg(test)]
pub(crate) fn scroll_view(
    height: usize,
    popup_items: usize,
    total: usize,
    evicted: u64,
    view: ScrollView,
    delta: i32,
) -> ScrollView {
    scroll_view_wrapped(height, popup_items, total, &|_| 1, evicted, view, delta)
}

/// Jump geometry (wrapped form): keeps the view when the target is visible, otherwise centers by visual rows.
///
/// Pure function for unit tests; identical to old logic for all-single-row logs.
pub(crate) fn jump_target_view_wrapped(
    height: usize,
    popup: usize,
    total: usize,
    height_of: &dyn Fn(usize) -> usize,
    evicted: u64,
    view: ScrollView,
    target_seq: u64,
) -> ScrollView {
    let layout = log_layout_wrapped(height, popup, total, height_of, evicted, view);
    if !layout.valid || layout.log_rows == 0 {
        return view;
    }
    let idx = usize::try_from(target_seq.saturating_sub(evicted)).unwrap_or(usize::MAX);
    if idx >= layout.start && idx < layout.end {
        return view; // 已可见，不动（少跳动）。
    }
    // Leave half a page of visual rows above the target; the landing row shows whole (clamped to the full-page start).
    let want_top = visual_prefix(total, height_of, idx).saturating_sub(layout.log_rows / 2);
    let found = log_at_visual(total, height_of, want_top);
    let (full_start, _) = full_page_start(total, height_of, layout.log_rows);
    ScrollView::Frozen {
        top_seq: evicted.saturating_add(found.min(full_start) as u64),
    }
}

/// Mouse hit region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hit {
    /// Log row (`idx` is the logical index, `x` is the global display column; wrap continuations add `sub-row x width`).
    Log { idx: usize, x: usize },
    /// Review hint row ("press Esc for bottom" plus the new-log badge): click jumps to the bottom.
    JumpToLatest,
    /// Input box (`row` is the in-box row 0..5, `x` is the display column).
    Input { row: usize, x: usize },
    /// Anything else (title/popup/status): no action.
    None,
}

/// Hit-testing (screen coordinates to region; the caller guarantees `layout.valid`).
///
/// Heights are evaluated on demand (only near the hit row); wrap continuations hit the same logical log,
/// and the returned column is the global column (`sub-row x width` plus in-row column), so callers keep the
/// whole-row selection semantics (first row to the end, last row from the start, whole rows copied).
pub(crate) fn hit_test(
    layout: &LogLayout,
    height_of: &dyn Fn(usize) -> usize,
    width: usize,
    x: usize,
    y: usize,
) -> Hit {
    // `width` is the log content width (clicks in the panel area are no-ops).
    let width = width.max(1);
    if y >= layout.log_top && y < layout.log_top + layout.log_rows {
        if x >= width {
            return Hit::None;
        }
        let row = y - layout.log_top;
        // Walk `row` visual rows forward from (start, first_sub).
        let mut idx = layout.start;
        let mut sub = layout.first_sub;
        for _ in 0..row {
            sub += 1;
            if sub >= height_of(idx) {
                idx += 1;
                sub = 0;
            }
        }
        if idx < layout.end && sub < height_of(idx) {
            return Hit::Log {
                idx,
                x: sub.saturating_mul(width).saturating_add(x),
            };
        }
        return Hit::None;
    }
    // Review hint row: the row between the log area and the input box; clicking jumps to the bottom.
    // Clicks in the panel area (right of the content width) are no-ops.
    if layout.hint_rows > 0 && y == layout.log_top + layout.log_rows {
        if x >= width {
            return Hit::None;
        }
        return Hit::JumpToLatest;
    }
    if y >= layout.input_base && y < layout.input_base + INPUT_ROWS {
        return Hit::Input {
            row: y - layout.input_base,
            x,
        };
    }
    Hit::None
}

/// Input-box click to cursor char index (same wrap/viewport math as `render_input_box`).
///
/// - Row 0 (top padding) jumps to the line end;
/// - content rows map columns to wrapped-row chars (wide chars belong by start column, past-end clamps).
pub(crate) fn input_click_cursor(
    text: &str,
    cur_cursor: usize,
    width: usize,
    clicked_i: usize,
    x: usize,
) -> usize {
    let chars: Vec<char> = text.chars().collect();
    if clicked_i == 0 || chars.is_empty() {
        return chars.len();
    }
    let rows = wrap_lines(
        &chars,
        width.saturating_sub(TEXT_COL),
        width.saturating_sub(INPUT_MARGIN),
    );
    if rows.is_empty() {
        return 0;
    }
    // Viewport (same as rendering: keeps the cursor visible, shows at most WRAP_ROWS rows).
    const WRAP_ROWS: usize = INPUT_TEXT_ROWS - INPUT_PAD_TOP; // 3
    let cursor = cur_cursor.min(chars.len());
    let (cursor_vrow, _) = locate_cursor(&rows, cursor);
    let max_start = rows.len().saturating_sub(WRAP_ROWS);
    let start_row = cursor_vrow.saturating_sub(WRAP_ROWS - 1).min(max_start);
    let vrow =
        (start_row + clicked_i.saturating_sub(INPUT_PAD_TOP)).min(rows.len().saturating_sub(1));
    let base = if vrow == 0 && start_row == 0 {
        TEXT_COL
    } else {
        INPUT_MARGIN
    };
    // Column to in-row char offset (the first char starting past x is the landing spot).
    let row = &rows[vrow];
    let mut w = 0usize;
    let mut k = row.len();
    for (i, c) in row.iter().enumerate() {
        let cw = UnicodeWidthChar::width(*c).unwrap_or(0);
        if x < base + w + cw {
            k = i;
            break;
        }
        w += cw;
    }
    rows[..vrow].iter().map(|r| r.len()).sum::<usize>() + k
}

/// One styled log row (same text as [`log_row_plain`]; styles are zero width).
///
/// Rendering wrap and hit heights share this shape: `ts + level + first message line` (tabs widened,
/// carriage returns stripped, multiline takes the first line), keeping the column math consistent.
fn build_log_styled(line: &LogLine, styled: bool) -> String {
    let mut log_row = String::new();
    push_fg(&mut log_row, Color::DarkGrey, styled);
    log_row.push_str(&line.ts);
    log_row.push(' ');
    push_fg(&mut log_row, level_color(line.level), styled);
    log_row.push_str(level_label(line.level));
    push_reset(&mut log_row);
    log_row.push(' ');
    let first = line.message.replace('\t', "  ").replace('\r', "");
    let first = first.split('\n').next().unwrap_or("");
    log_row.push_str(first);
    log_row
}

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
///
/// Pure function for unit tests; columns use [`crate::ansi::display_width`] (same as render truncation).
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
fn count_visible_chars(s: &str) -> usize {
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
fn paint_current_match(mid: &str, bg: Rgb, shine_t: Option<f32>) -> String {
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
fn render_search_bar(
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
/// ANSI escapes and section-sign color codes in messages must be stripped, or copies to the clipboard would carry them
/// (they render as zero-width styles, and column mapping must use the stripped visible columns).
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

// ================= Right online-player panel =================

/// Right panel cell (`visible_cols` columns, steady `SIDE_PANEL_WIDTH`):
/// `zone_row` is the in-panel row number (from 0, covering log rows, fills, hints, and popup rows).
///
/// Background uses one shade darker than the title/input bars (`SIDE_PANEL_BG`), standing apart;
/// row 0 is the title (online count), then one player name per row (overlong names truncate),
/// an empty list shows a low-key placeholder below the first row. A dim gray separator blends the edge.
/// While sliding in, only the left `visible_cols` columns show (the panel grows leftward from the right edge).
fn side_panel_cell(
    buf: &mut String,
    players: &[String],
    zone_row: usize,
    styled: bool,
    visible_cols: usize,
    lang: &str,
) {
    let visible_cols = visible_cols.min(SIDE_PANEL_WIDTH);
    let mut cell = String::new();
    // Paint the whole cell background first (including later pad spaces, so the color bar stays complete).
    push_bg_rgb(&mut cell, SIDE_PANEL_BG, styled);
    push_fg(&mut cell, Color::DarkGrey, styled);
    cell.push('│');
    if styled {
        cell.push_str("\x1b[39m");
    }
    let inner = SIDE_PANEL_WIDTH - 1;
    if zone_row == 0 {
        let title =
            rust_i18n::t!("tui.players_title", locale = lang, n = players.len()).to_string();
        let shown = truncate_to_width(&title, inner);
        push_fg(&mut cell, Color::Cyan, styled);
        cell.push_str(&shown);
        push_bg_rgb(&mut cell, SIDE_PANEL_BG, styled);
        cell.push_str(&" ".repeat(inner.saturating_sub(display_width(&shown))));
    } else if let Some(name) = players.get(zone_row - 1) {
        let shown = truncate_styled(name, inner);
        cell.push_str(&shown);
        // A styled name tail (`truncate_styled` may append a reset) re-asserts the background afterwards.
        push_bg_rgb(&mut cell, SIDE_PANEL_BG, styled);
        cell.push_str(&" ".repeat(inner.saturating_sub(display_width(&shown))));
    } else if zone_row == 1 && players.is_empty() {
        let empty = rust_i18n::t!("tui.players_empty", locale = lang).to_string();
        push_fg(&mut cell, Color::DarkGrey, styled);
        cell.push_str(&empty);
        cell.push_str(&" ".repeat(inner.saturating_sub(display_width(&empty))));
    } else {
        cell.push_str(&" ".repeat(inner));
    }
    // While sliding in, take only the visible left columns; a truncation ending in a reset would clear the background, so re-assert before padding.
    let cell = truncate_styled(&cell, visible_cols);
    buf.push_str(&cell);
    push_bg_rgb(buf, SIDE_PANEL_BG, styled);
    buf.push_str(&" ".repeat(visible_cols.saturating_sub(display_width(&cell))));
    push_reset(buf);
}

/// Log row tail: content (already truncated to the content width) pads to `cw` columns plus panel plus newline.
///
/// With the panel hidden the bytes match the old behavior exactly (no pad spaces; the render layer erases tails);
/// while sliding in (`panel_cols` below full width) a transparent gap separates the panel from the content.
#[allow(clippy::too_many_arguments)]
fn emit_log_zone_row(
    buf: &mut String,
    row_out: &str,
    players: &[String],
    zone_row: usize,
    cw: usize,
    show_panel: bool,
    styled: bool,
    panel_cols: usize,
    lang: &str,
) {
    buf.push_str(row_out);
    if show_panel {
        let used = display_width(row_out);
        buf.push_str(&" ".repeat(cw.saturating_sub(used)));
        buf.push_str(&" ".repeat(SIDE_PANEL_WIDTH.saturating_sub(panel_cols)));
        side_panel_cell(buf, players, zone_row, styled, panel_cols, lang);
    }
    push_reset(buf);
    buf.push_str("\r\n");
}

fn build_frame(f: &Frame) -> (Vec<String>, (u16, u16)) {
    let width = (f.width as usize).max(1);
    let height = (f.height as usize).max(1);
    if height <= HEADER_ROWS + INPUT_ROWS + STATUS_ROWS {
        let mut rows = vec![String::new(); height];
        rows[0] = truncate_to_width(
            &rust_i18n::t!("tui.term_too_small", locale = f.lang),
            width,
        );
        return (rows, (0, 0));
    }
    let styled = style_enabled();
    let locked = f.phase != ServerPhase::Running;
    // Right player panel: wide windows reserve fixed columns on the log right (title/input/status stay full width).
    let show_panel = side_panel_visible(width);
    let cw = log_content_width(width);
    // Panel slide-in (boot): grows leftward from the right edge to dock (log area keeps its final width, no reflow).
    let panel_shift: usize = match f.boot {
        Some(b) if show_panel => {
            ((1.0 - b.panel_slide.clamp(0.0, 1.0)) * SIDE_PANEL_WIDTH as f32).round() as usize
        }
        _ => 0,
    };
    let panel_cols = SIDE_PANEL_WIDTH.saturating_sub(panel_shift.min(SIDE_PANEL_WIDTH));

    // Layout (shares the wrapped layout with mouse hit-testing; heights on demand; both sides strictly agree).
    // Completion list rows: shrink to keep at least 1 log row; total rows always equal height.
    // (The celebration reuses the footnote row, never takes an extra row.)
    let target_popup = f
        .editor
        .completion
        .as_ref()
        .map(|c| c.items.len())
        .unwrap_or(0);
    // Log wrap heights (plain-text greedy measure, same break rules as the splitter; only near the window).
    // Widths use the content width (yields to the panel when visible).
    let height_of = |idx: usize| -> usize {
        f.logs
            .get(idx)
            .map(|line| log_wrap_height(line, cw))
            .unwrap_or(1)
    };
    // Pops upward: the visible count eases from 0 to target while logs push up row by row (never jumps).
    let layout = log_layout_wrapped(
        height,
        animated_popup_count(f.popup_age_ms, target_popup),
        f.logs.len(),
        &height_of,
        f.log_evicted,
        f.view,
    );
    debug_assert!(layout.valid);
    let popup_rows = layout.popup_rows;
    let log_rows = layout.log_rows;
    // Rows below the viewport (shared by the status-line review flag and the new-log badge).
    let scroll_up = layout.up;

    // Title colors (phase driven; steady state uses the pale-blue theme).
    // Sweep progress (shared by the title shine and the input box).
    let p = f.sweep_progress.clamp(0.0, 1.0);
    let (title_fg, mark_fg) = match f.phase {
        ServerPhase::Starting => {
            let b = Fg::Rgb(breath_fg(f.anim_ms));
            (b, b)
        }
        ServerPhase::Sweeping => {
            let g = Fg::Rgb(sweep_fg(f.sweep_progress));
            (g, g)
        }
        ServerPhase::Running => (Fg::Rgb(TITLE_BLUE), Fg::Code(81)),
    };
    // The input background function lives in the input section (phase driven: steady gray, ripple, green band).

    let mut buf = String::with_capacity(12 * 1024);

    // ---- Title bars (two rows, full-width bar background) ----
    let core = if f.header.core_version.is_empty() {
        "SculkBedrock".to_string()
    } else {
        format!("SculkBedrock {}", f.header.core_version)
    };
    let cpu_value = if f.stats.has_data {
        format!("{:.1}%", f.stats.cpu_pct.max(0.0))
    } else {
        "--".to_string()
    };
    // Second row left: the quote display string (typewriter intermediate state; the pack label lives only in the input footnote).
    let pack = f.header.pack_label.clone();
    let sub_text = f.subtitle.to_string();
    let mem_value = if f.stats.has_data {
        format!("{:.1} MB", f.stats.mem_mb.max(0.0))
    } else {
        "--".to_string()
    };
    if let Some(boot) = f.boot {
        // Boot: bars stretch, then the title types out plus CPU/MEM fades in (wrapping handled inside the two rows).
        render_boot_header(
            &mut buf, &boot, &core, &cpu_value, &sub_text, &mem_value, width, f.anim_ms, styled,
        );
    } else {
        // Row 1: after turning green in the sweep, adds a left-to-right shine, then returns to steady;
        // base colors match the gradient on both sides, no jump.
        let title_shining = matches!(f.phase, ServerPhase::Sweeping) && (0.5..0.9).contains(&p);
        if title_shining {
            let base = sweep_fg(p);
            // Remap shine progress to [0,1] (sweep p in [0.5,0.9) becomes t in [0,1)).
            let t = (p - 0.5) / 0.4;
            emit_shining_title_row(
                &mut buf, "⬢ ", mark_fg, &core, base, t, "CPU ", &cpu_value, width, styled,
            );
        } else {
            emit_title_row(
                &mut buf,
                "⬢ ",
                mark_fg,
                &core,
                Some(title_fg),
                "CPU ",
                &cpu_value,
                width,
                styled,
            );
        }
        // Row 1 normal path. Lower row: shines after turning sweep-green (brightens per char, then steady).
        buf.push_str("\r\n");
        // Post-typing quote shine (`subtitle_shine`; None means no shine); the pinned bracket never shines,
        // always keeps its base color so the bracket never flickers at the end.
        match f.subtitle_shine {
            Some(t) if !sub_text.is_empty() => {
                let base = SUB_BASE;
                let (body, pinned) = split_pinned_closer(&sub_text);
                emit_shining_title_row_pinned(
                    &mut buf,
                    "",
                    mark_fg,
                    body,
                    pinned,
                    base,
                    t.clamp(0.0, 1.0),
                    "MEM ",
                    &mem_value,
                    width,
                    styled,
                );
            }
            _ => {
                let sub_fg: Option<Fg> = if sub_text.is_empty() {
                    None
                } else {
                    Some(Fg::Code(117))
                };
                emit_title_row(
                    &mut buf, "", mark_fg, &sub_text, sub_fg, "MEM ", &mem_value, width, styled,
                );
            }
        }
    } // 开场分支结束（开场两行已由 render_boot_header 画完）
    buf.push_str("\r\n");

    // ---- Log stream (auto-wrapped) ----
    // Visual rows are sub-rows of logical logs split by width (no indent on continuations, global columns stay contiguous);
    // walks log_rows visual rows forward from (start, first_sub) (`layout` only fixes the bounds).
    // (`height_of` reuses the layout-section closure to avoid re-parsing.)
    let mut visual: Vec<(usize, usize)> = Vec::new();
    {
        let mut idx = layout.start;
        let mut sub = layout.first_sub;
        while visual.len() < log_rows && idx < layout.end {
            let h = height_of(idx);
            if sub >= h {
                // Guard: skip the row when the height table disagrees with the splitter (extreme OSC input), never panic.
                idx += 1;
                sub = 0;
                continue;
            }
            visual.push((idx, sub));
            sub += 1;
            if sub >= h {
                idx += 1;
                sub = 0;
            }
        }
    }
    // Boot log typewriter: after the bars fill, visible rows type out top to bottom;
    // after boot ends or is disabled, rows render whole again (later logs skip the typewriter).
    // Budgets use the content width (the log area narrows when the panel shows).
    let log_budgets = f
        .boot
        .map(|b| boot_log_budgets(visual.len(), cw, b.title_t));
    for (pos, (idx, sub)) in visual.iter().copied().enumerate() {
        let Some(line) = f.logs.get(idx) else {
            continue;
        };
        let log_row = build_log_styled(line, styled);
        // Sub-row global column window (no indent: sub-row k covers [k*cw, (k+1)*cw)).
        let win_start = sub.saturating_mul(cw);
        let win_end = win_start.saturating_add(cw);
        let chunks = crate::ansi::split_styled_rows(&log_row, cw);
        let chunk = chunks.get(sub).map(String::as_str).unwrap_or("");
        let budget = log_budgets
            .as_ref()
            .map(|v| v[pos])
            .unwrap_or(cw)
            .min(width);
        let mut row_out = truncate_styled(chunk, budget);
        // Global columns intersected with this sub-row window give in-row columns (selection/search keep whole-row semantics per sub-row).
        let intersect = |x0: usize, x1: usize| -> Option<(usize, usize)> {
            let l0 = x0.max(win_start).saturating_sub(win_start);
            let l1 = x1.min(win_end).saturating_sub(win_start);
            (l0 < l1).then_some((l0, l1))
        };
        // Mouse drag selection highlight (taps have no range, never highlight).
        if let Some(sel) = &f.selection {
            if let Some((x0, x1)) = sel.x_range_for(idx) {
                if let Some((l0, l1)) = intersect(x0, x1) {
                    row_out = highlight_range(&row_out, l0, l1, styled);
                }
            }
        }
        // Search highlight (plain pale-blue plus current green/gold with fade/shine; stacks with selection).
        if let Some(search) = &f.search {
            if !search.matches.is_empty() {
                let row_seq = f.log_evicted.saturating_add(idx as u64);
                let current = search.matches.get(search.current);
                let ranges: Vec<(usize, usize, bool)> = search
                    .matches
                    .iter()
                    .filter(|m| m.seq == row_seq)
                    .filter_map(|m| {
                        let is_current =
                            current.is_some_and(|c| c.seq == m.seq && c.x0 == m.x0 && c.x1 == m.x1);
                        intersect(m.x0, m.x1).map(|(l0, l1)| (l0, l1, is_current))
                    })
                    .collect();
                row_out = highlight_search_ranges(&row_out, &ranges, styled, search.current_age_ms);
            }
        }
        // Log row tail (panel rows track the visual row number).
        emit_log_zone_row(
            &mut buf, &row_out, f.players, pos, cw, show_panel, styled, panel_cols, f.lang,
        );
    }
    let shown = visual.len();
    // Panel rows advance together with visual rows (including fill rows below).
    let mut zone_row = shown;
    for _ in shown..log_rows {
        if show_panel {
            emit_log_zone_row(
                &mut buf, "", f.players, zone_row, cw, true, styled, panel_cols, f.lang,
            );
            zone_row += 1;
        } else {
            buf.push_str("\r\n");
        }
    }

    // ---- Review hint row (just above the input box: centered hint plus right-aligned new-log badge) ----
    if layout.hint_rows > 0 {
        // The badge only appears with real new arrivals; the count tracks scrolling (0 at the bottom means it disappears).
        // Widths use the content width (yields to the panel when visible).
        let unseen = unseen_count(f.log_seq, f.log_evicted, layout.end, f.frozen_base_seq);
        let badge = (unseen > 0).then(|| {
            rust_i18n::t!("tui.new_logs", locale = f.lang, n = unseen).to_string()
        });
        let badge_w = badge.as_deref().map_or(0, display_width);
        let hint_text =
            rust_i18n::t!("tui.hint_back_to_bottom", locale = f.lang).to_string();
        let hint_w = display_width(&hint_text);
        // When short on space, drop the centered hint first and keep the right-side count (carries more signal).
        let hint = if cw > badge_w + hint_w + 4 {
            Some(hint_text.as_str())
        } else {
            None
        };
        let mut hint_row = String::new();
        push_fg(&mut hint_row, Color::DarkGrey, styled);
        let left_used = match hint {
            Some(text) => {
                // Center the hint in the space left of the badge (centering over the whole row would skew against the badge).
                let avail = cw.saturating_sub(badge_w + 1);
                let pad = avail.saturating_sub(display_width(text)) / 2;
                hint_row.push_str(&" ".repeat(pad));
                hint_row.push_str(text);
                pad + display_width(text) + 1
            }
            None => 0,
        };
        hint_row.push_str(&" ".repeat(cw.saturating_sub(left_used + badge_w)));
        if let Some(badge) = badge.as_deref() {
            // The badge uses gray-blue (same family as the copy notice, low-key).
            push_fg_rgb(&mut hint_row, COPY_BRIGHT, styled);
            hint_row.push_str(badge);
        }
        push_reset(&mut hint_row);
        let hint_row = truncate_styled(&hint_row, cw);
        // The hint row takes panel row log_rows.
        emit_log_zone_row(
            &mut buf, &hint_row, f.players, log_rows, cw, show_panel, styled, panel_cols, f.lang,
        );
    }

    // ---- Completion list (hugs the input box top) ----
    if let Some(comp) = &f.editor.completion {
        // Appear animation: rows emerge upward from the input box (see layout for the visible count), each sliding 8 columns plus fading;
        // steady shows everything with no offset at full brightness. `n` is the target row total.
        let mut popup_zone = 0usize;
        for (i, item) in comp.items.iter().take(popup_rows).enumerate() {
            let (visible, indent, opacity) = popup_row_anim(f.popup_age_ms, i, target_popup);
            if !visible {
                // Rows not yet born hold blank placeholders (total row count stays constant).
                let zone_row = log_rows + layout.hint_rows + popup_zone;
                popup_zone += 1;
                emit_log_zone_row(
                    &mut buf, "", f.players, zone_row, cw, show_panel, styled, panel_cols, f.lang,
                );
                continue;
            }
            // Fade: opacity 1 is the steady theme color; mid-animation interpolates from black toward steady RGB.
            let faded = opacity < 1.0;
            let selected = i == comp.selected;
            let mut popup_row = String::new();
            if selected && styled {
                push_bg_rgb(
                    &mut popup_row,
                    mix_rgb(POPUP_FADE_FROM, POPUP_SELECTED_BG, opacity),
                    styled,
                );
            }
            if indent > 0 {
                popup_row.push_str(&" ".repeat(indent));
            }
            popup_row.push_str("  ");
            if faded {
                push_fg_rgb(
                    &mut popup_row,
                    mix_rgb(POPUP_FADE_FROM, POPUP_DISPLAY_RGB, opacity),
                    styled,
                );
            } else {
                push_fg(&mut popup_row, Color::Cyan, styled);
            }
            popup_row.push_str(&truncate_to_width(&item.display, 24));
            popup_row.push_str("   ");
            if faded {
                push_fg_rgb(
                    &mut popup_row,
                    mix_rgb(POPUP_FADE_FROM, META_LABEL_RGB, opacity),
                    styled,
                );
            } else {
                push_fg(&mut popup_row, Color::DarkGrey, styled);
            }
            popup_row.push_str(&truncate_to_width(&item.description, 48));
            if item.from_alias {
                popup_row.push_str("  ");
                if faded {
                    push_fg_rgb(
                        &mut popup_row,
                        mix_rgb(POPUP_FADE_FROM, POPUP_ALIAS_RGB, opacity),
                        styled,
                    );
                } else {
                    push_fg(&mut popup_row, Color::Yellow, styled);
                }
                popup_row.push_str(&rust_i18n::t!("tui.alias_tag", locale = f.lang));
            }
            let mut popup_row = truncate_styled(&popup_row, cw);
            // Popup rows sit after the log rows in panel coordinates (hint row further after); row numbers stay contiguous.
            let zone_row = log_rows + layout.hint_rows + popup_zone;
            popup_zone += 1;
            if selected && styled {
                let w = display_width(&popup_row);
                if w < cw {
                    popup_row.push_str(&" ".repeat(cw - w));
                }
            }
            emit_log_zone_row(
                &mut buf, &popup_row, f.players, zone_row, cw, show_panel, styled, panel_cols, f.lang,
            );
        }
    }

    // ---- Input box (five rows) ----
    //
    // - Starting: no marker; row 3 centers the boot text with a continuously spreading ripple to the edges;
    // - Sweeping: the bright-green gradient band slides left to right; the marker slides in from the edge and fades in;
    // - Running: normal wrapped editing (content indented, never hugging the edge).
    // (`p` reuses the sweep progress already computed for the title.)
    // Input block start row (shifts up one row while the celebration row shows).
    let input_base = height - INPUT_ROWS - STATUS_ROWS;
    let mut cursor_xy = (TEXT_COL as u16, (input_base + INPUT_PAD_TOP) as u16);
    let input_bg = |col: usize, row: usize| match f.phase {
        ServerPhase::Starting => ripple_bg(col, row, width, f.anim_ms),
        ServerPhase::Sweeping => sweep_bar(col, width, p),
        // Running: pure gray background once the afterglow (fading pale green) clears.
        ServerPhase::Running => {
            let k = f.afterglow.clamp(0.0, 1.0) * 0.55;
            mix_rgb(BAR_RGB, Rgb(140, 190, 140), k)
        }
    };

    if let Some(boot) = f.boot {
        render_boot_input(&mut buf, f, &boot, width, styled);
    } else if f.phase == ServerPhase::Starting {
        render_starting_box(&mut buf, &input_bg, width, styled, 1.0, f.lang);
    } else {
        render_input_box(
            &mut buf,
            f,
            &input_bg,
            p,
            width,
            input_base,
            styled,
            &mut cursor_xy,
        );
    }
    // Footnote row: normally the pack version at bottom left; during celebration it plays
    // a pack fade-out, STARTUP shrink, pack fade-in sequence (inside the input box).
    let (foot_plain, foot_fg) = if pack.is_empty() {
        (
            rust_i18n::t!("tui.no_packs", locale = f.lang).to_string(),
            Fg::Code(245),
        )
    } else {
        (pack.clone(), Fg::Rgb(PACK_COLOR))
    };
    let foot_pad = " ".repeat(INPUT_MARGIN);
    match f.boot {
        Some(boot) => {
            // Boot footnote: an empty light bar while stretching; the pack label fades in with the input content once filled.
            // (Boot only appears in Starting, never together with celebration, so celebrate needs no handling.)
            let bar_len = (width as f32 * boot.input_expand).round() as usize;
            if bar_len < width {
                emit_growing_bar(&mut buf, bar_len, width, styled);
            } else {
                let op = boot.input_opacity;
                let pack_rgb = if pack.is_empty() {
                    META_LABEL_RGB
                } else {
                    PACK_COLOR
                };
                let faded_fg = Fg::Rgb(mix_rgb(BAR_RGB, pack_rgb, op));
                // Background matches the input row above (the ripple fades in with the same opacity).
                let faded_bg = |col: usize, row: usize| {
                    mix_rgb(BAR_RGB, ripple_bg(col, row, width, f.anim_ms), op)
                };
                emit_shaded_row(
                    &mut buf,
                    &[
                        (foot_pad.as_str(), None),
                        (foot_plain.as_str(), Some(faded_fg)),
                    ],
                    &faded_bg,
                    INPUT_ROWS - 1,
                    0,
                    width,
                    styled,
                );
            }
        }
        None => match f.celebrate.map(|q| q.clamp(0.0, 1.0)) {
            None => {
                emit_shaded_row(
                    &mut buf,
                    &[
                        (foot_pad.as_str(), None),
                        (foot_plain.as_str(), Some(foot_fg)),
                    ],
                    &input_bg,
                    INPUT_ROWS - 1,
                    0,
                    width,
                    styled,
                );
            }
            Some(q) if q < 0.075 => {
                // Pack label quick fade (150ms, clears the stage before celebration).
                let fg = Fg::Rgb(mix_rgb(PACK_COLOR, BAR_RGB, smoothstep(q / 0.075)));
                emit_shaded_row(
                    &mut buf,
                    &[(foot_pad.as_str(), None), (foot_plain.as_str(), Some(fg))],
                    &input_bg,
                    INPUT_ROWS - 1,
                    0,
                    width,
                    styled,
                );
            }
            Some(q) => {
                // Shrink [0.075,0.325] (0.5s): eased gathering with continuous neighbor brightness;
                // hold [0.325,0.825] (1s): gathered hold plus left-to-right shine plus pack label appearing;
                // fade [0.825,1.0]: STARTUP dims while the pack label reaches full brightness.
                // Letter spacing (only changes during shrink).
                let contraction = smoothstep((q - 0.075) / 0.25);
                let spacing = width.saturating_sub(9) as f32 / 8.0 * (1.0 - contraction);
                // STARTUP per-char colors: steady during shrink; shining during hold; dimming as a whole during fade.
                let hold_t = ((q - 0.325) / 0.5).clamp(0.0, 1.0);
                let fade_t = ((q - 0.825) / 0.175).clamp(0.0, 1.0);
                let shine_base = Rgb(135, 255, 135);
                let shine_peak = Rgb(230, 255, 230);
                let front = hold_t * 15.0 - 3.0;
                let char_fg = |i: usize| -> Fg {
                    if q < 0.825 {
                        if q < 0.325 {
                            Fg::Rgb(mix_rgb(BAR_RGB, shine_base, smoothstep((q - 0.075) / 0.05)))
                        } else {
                            let dd = i as f32 - front;
                            let glow = (-(dd * dd) / (2.0 * 1.5 * 1.5)).exp();
                            Fg::Rgb(mix_rgb(shine_base, shine_peak, glow))
                        }
                    } else {
                        Fg::Rgb(mix_rgb(shine_base, BAR_RGB, smoothstep(fade_t)))
                    }
                };
                // Pack label: appears during the hold ([0.35,0.6]), invisible before, full after.
                let pack_t = ((q - 0.35) / 0.25).clamp(0.0, 1.0);
                let pack_fg = Fg::Rgb(mix_rgb(BAR_RGB, PACK_COLOR, smoothstep(pack_t)));
                // Prefers centering; long pack labels yield rightward for the text, narrow terminals truncate the label.
                let target_start = (width.saturating_sub(9) / 2)
                    .max(INPUT_MARGIN + display_width(&foot_plain))
                    .min(width.saturating_sub(9));
                let center_start = target_start as f32 * contraction;
                let pack_budget = (center_start.floor() as usize).saturating_sub(INPUT_MARGIN);
                let pack_shown = truncate_to_width(&foot_plain, pack_budget);
                let pack_w = INPUT_MARGIN + display_width(&pack_shown);
                let mut cells = vec![(" ".to_string(), BAR_RGB); width];
                for (i, letter) in ['>', 'S', 'T', 'A', 'R', 'T', 'U', 'P', '<']
                    .iter()
                    .enumerate()
                {
                    let x = center_start + i as f32 * (1.0 + spacing);
                    let col = x.floor() as usize;
                    let fraction = x.fract();
                    let Fg::Rgb(color) = char_fg(i) else {
                        unreachable!()
                    };
                    // Neighbor brightness cross-fades to reduce jumping from whole-cell terminal moves.
                    for (position, weight) in [(col, 1.0 - fraction), (col + 1, fraction)] {
                        if position < width && weight > 0.0 {
                            let color = mix_rgb(BAR_RGB, color, weight);
                            if color.1 >= cells[position].1 .1 {
                                cells[position] = (letter.to_string(), color);
                            }
                        }
                    }
                }
                let mut segs: Vec<Segment> = vec![
                    (foot_pad.as_str(), None),
                    (pack_shown.as_str(), Some(pack_fg)),
                ];
                for (letter, color) in cells.iter().skip(pack_w.min(width)) {
                    segs.push((letter.as_str(), Some(Fg::Rgb(*color))));
                }
                emit_shaded_row(&mut buf, &segs, &input_bg, INPUT_ROWS - 1, 0, width, styled);
            }
        }, // `None` = 无开场：常规脚注 / 庆祝
    } // 开场脚注分支结束
    buf.push_str("\r\n");

    // ---- Status line (right status always visible; the left side swaps to the confirm pill on double confirm) ----
    let mut right =
        rust_i18n::t!("tui.status_complete", locale = f.lang, n = f.provider_name).to_string();
    if scroll_up > 0 {
        right.push_str(
            rust_i18n::t!("tui.status_reviewing", locale = f.lang, n = scroll_up).as_ref(),
        );
    }
    if f.dropped_logs > 0 {
        right.push_str(
            rust_i18n::t!("tui.status_dropped", locale = f.lang, n = f.dropped_logs).as_ref(),
        );
    }
    if locked {
        right.push_str(&rust_i18n::t!("tui.status_locked", locale = f.lang));
    }
    let right_w = display_width(&right);
    // The confirm hint fades in/out over a fixed left area, then restores the normal hints when done.
    let exit_op = f.exit_prompt.unwrap_or(0.0).clamp(0.0, 1.0);
    let hint_op = 1.0 - exit_op;
    let left_budget = width.saturating_sub(right_w + 1);
    let hint_shown = truncate_to_width(
        &rust_i18n::t!("tui.hint_keys", locale = f.lang),
        left_budget,
    );
    let pill_shown = truncate_to_width(
        &rust_i18n::t!("tui.exit_confirm", locale = f.lang),
        left_budget,
    );
    // Both wordings share the left area; the transition only changes colors, never appends a second span.
    let used;
    if let Some(search) = &f.search {
        // Search state: the left side becomes the search box (exit confirm cannot co-occur: Ctrl+C never arms in search).
        let (search_used, search_cx) = render_search_bar(&mut buf, search, width, right_w, styled, f.lang);
        used = search_used;
        // The terminal cursor moves into the search box (at the query cursor, last row).
        cursor_xy.0 = (search_cx as u16).min(width.saturating_sub(1) as u16);
        cursor_xy.1 = height.saturating_sub(1) as u16;
    } else {
        if exit_op <= 0.0 {
            push_fg_rgb(&mut buf, mix_rgb(HINT_FG_DIM, HINT_FG, hint_op), styled);
            buf.push_str(&hint_shown);
            push_reset(&mut buf);
        }
        // 2) Exit confirm pill: pale-red background plus golden text (opacity driven).
        if exit_op > 0.0 {
            push_bg_rgb(&mut buf, mix_rgb(EXIT_BG_DIM, EXIT_BG, exit_op), styled);
            push_fg_rgb(&mut buf, mix_rgb(EXIT_FG_DIM, EXIT_FG, exit_op), styled);
            buf.push_str(&pill_shown);
            push_reset(&mut buf);
            used = display_width(&pill_shown);
        } else {
            used = display_width(&hint_shown);
        }
    }
    // 3) Right status (always on, never fades).
    if used + 1 + right_w <= width {
        buf.push_str(&" ".repeat(width - used - right_w));
        buf.push_str(&right);
    } else {
        // Very narrow terminals: the left side wins and the right status yields.
        buf.push_str(&truncate_to_width(&right, width.saturating_sub(used)));
    }
    push_reset(&mut buf);

    cursor_xy.0 = cursor_xy.0.min(width.saturating_sub(1) as u16);
    cursor_xy.1 = cursor_xy.1.min(height.saturating_sub(1) as u16);
    (buf.split("\r\n").map(str::to_owned).collect(), cursor_xy)
}

#[cfg(test)]
fn render<W: Write>(w: &mut W, f: &Frame) -> std::io::Result<(u16, u16)> {
    let (rows, cursor) = build_frame(f);
    w.write_all(rows.join("\r\n").as_bytes())?;
    Ok(cursor)
}

/// Title row: left `mark + text`, right `label + value` (right aligned), full-width bar background.
///
/// Right values stay light gray (250), never follow the terminal default foreground.
#[allow(clippy::too_many_arguments)]
fn emit_title_row(
    buf: &mut String,
    mark: &str,
    mark_fg: Fg,
    text: &str,
    text_fg: Option<Fg>,
    right_label: &str,
    right_value: &str,
    width: usize,
    styled: bool,
) {
    let left_w = display_width(mark) + display_width(text);
    let right_w = display_width(right_label) + display_width(right_value);
    let pad = " ".repeat(width.saturating_sub(left_w + right_w));
    let segs: Vec<Segment> = vec![
        (mark, Some(mark_fg)),
        (text, text_fg),
        (pad.as_str(), None),
        (right_label, Some(Fg::Code(245))),
        (right_value, Some(Fg::Code(250))),
    ];
    let bar = |_: usize, _: usize| Rgb(38, 38, 38);
    emit_shaded_row(buf, &segs, &bar, 0, 0, width, styled);
}

/// Split off the pinned right bracket: trailing bracket gives (body, bracket), else (full text, empty).
///
/// Matches the [`crate::Typewriter`] split rule so the shine range equals the typing range.
fn split_pinned_closer(full: &str) -> (&str, &str) {
    if let Some(cut) = full.strip_suffix('」') {
        (cut, "」")
    } else {
        (full, "")
    }
}

/// Brightness of the gaussian light front at char i (0..1).
///
/// `front` is the wave center char index; larger `spread` means a wider light band.
fn wave_glow(i: usize, front: f32, spread: f32) -> f32 {
    let d = i as f32 - front;
    (-(d * d) / (2.0 * spread * spread)).exp()
}

/// Title row shine variant: brightens the body per char along the gaussian wave; labels and values stay normal.
///
/// `t` in [0,1]: the wave sweeps left to right across the body and leaves; both ends equal the base color (no jump).
#[allow(clippy::too_many_arguments)]
fn emit_shining_title_row(
    buf: &mut String,
    mark: &str,
    mark_fg: Fg,
    text: &str,
    base: Rgb,
    t: f32,
    right_label: &str,
    right_value: &str,
    width: usize,
    styled: bool,
) {
    let chars: Vec<String> = text.chars().map(|c| c.to_string()).collect();
    let n = chars.len();
    // The wave enters from 3 chars outside and leaves 3 chars past the end (edge chars get the full pass).
    let front = t.clamp(0.0, 1.0) * (n as f32 + 6.0) - 3.0;
    let mut segs: Vec<Segment> = vec![(mark, Some(mark_fg))];
    for (i, ch) in chars.iter().enumerate() {
        let glow = wave_glow(i, front, 1.5);
        segs.push((
            ch.as_str(),
            Some(Fg::Rgb(mix_rgb(base, Rgb(235, 245, 255), glow * 0.9))),
        ));
    }
    let left_w = display_width(mark) + display_width(text);
    let right_w = display_width(right_label) + display_width(right_value);
    let pad = " ".repeat(width.saturating_sub(left_w + right_w));
    segs.push((pad.as_str(), None));
    segs.push((right_label, Some(Fg::Code(245))));
    segs.push((right_value, Some(Fg::Code(250))));
    let bar = |_: usize, _: usize| Rgb(38, 38, 38);
    emit_shaded_row(buf, &segs, &bar, 0, 0, width, styled);
}

/// Title row shine variant (pinned tail bracket): the body shines while the bracket keeps the base color.
///
/// Used for the quote: the typewriter pins the bracket at the row end, and shining it too would flicker the ending.
#[allow(clippy::too_many_arguments)]
fn emit_shining_title_row_pinned(
    buf: &mut String,
    mark: &str,
    mark_fg: Fg,
    body: &str,
    pinned: &str,
    base: Rgb,
    t: f32,
    right_label: &str,
    right_value: &str,
    width: usize,
    styled: bool,
) {
    let chars: Vec<String> = body.chars().map(|c| c.to_string()).collect();
    let n = chars.len();
    let front = t.clamp(0.0, 1.0) * (n as f32 + 6.0) - 3.0;
    let mut segs: Vec<Segment> = vec![(mark, Some(mark_fg))];
    for (i, ch) in chars.iter().enumerate() {
        let glow = wave_glow(i, front, 1.5);
        segs.push((
            ch.as_str(),
            Some(Fg::Rgb(mix_rgb(base, SUB_SHINE_PEAK, glow * 0.9))),
        ));
    }
    // The pinned bracket never joins the shine.
    segs.push((pinned, Some(Fg::Rgb(base))));
    let left_w = display_width(mark) + display_width(body) + display_width(pinned);
    let right_w = display_width(right_label) + display_width(right_value);
    let pad = " ".repeat(width.saturating_sub(left_w + right_w));
    segs.push((pad.as_str(), None));
    segs.push((right_label, Some(Fg::Code(245))));
    segs.push((right_value, Some(Fg::Code(250))));
    let bar = |_: usize, _: usize| Rgb(38, 38, 38);
    emit_shaded_row(buf, &segs, &bar, 0, 0, width, styled);
}

/// Booting input box: no marker; row 3 centers the boot text with ripple-supplied backgrounds.
///
/// `text_opacity`: boot text opacity (boot fade-in use; 1.0 is the steady fixed color 253,
/// matching old behavior exactly).
fn render_starting_box(
    buf: &mut String,
    bg_at: &dyn Fn(usize, usize) -> Rgb,
    width: usize,
    styled: bool,
    text_opacity: f32,
    lang: &str,
) {
    let text = rust_i18n::t!("tui.server_starting", locale = lang).to_string();
    let text_fg = if text_opacity >= 1.0 {
        Fg::Code(253)
    } else {
        Fg::Rgb(mix_rgb(BAR_RGB, STARTUP_TEXT_RGB, text_opacity))
    };
    for i in 0..INPUT_TEXT_ROWS {
        if i == 2 {
            // Centered (wide-char safe; width follows the translation).
            let pad = width.saturating_sub(display_width(&text)) / 2;
            let pad_str = " ".repeat(pad);
            emit_shaded_row(
                buf,
                &[(pad_str.as_str(), None), (text.as_str(), Some(text_fg))],
                bg_at,
                i,
                0,
                width,
                styled,
            );
        } else {
            emit_shaded_row(buf, &[], bg_at, i, 0, width, styled);
        }
        buf.push_str("\r\n");
    }
}

/// Boot input box: an empty light bar while the bars stretch (in parallel with the title); once filled, the ripple
/// content fades in by `input_opacity` (never waits for the title). The cursor stays hidden; caller default is kept.
fn render_boot_input(buf: &mut String, f: &Frame, boot: &BootFrame, width: usize, styled: bool) {
    let bar_len = (width as f32 * boot.input_expand).round() as usize;
    if bar_len < width {
        for _ in 0..INPUT_TEXT_ROWS {
            emit_growing_bar(buf, bar_len, width, styled);
            buf.push_str("\r\n");
        }
        return;
    }
    // Filled: ripple backgrounds and boot text fade in together (steady endpoints match the normal boot state, no jump).
    let op = boot.input_opacity;
    let faded_bg =
        |col: usize, row: usize| mix_rgb(BAR_RGB, ripple_bg(col, row, width, f.anim_ms), op);
    render_starting_box(buf, &faded_bg, width, styled, op, f.lang);
}

// ================= Completion popup appear animation =================

/// Popup appear duration (ms): rows pop upward from the input box while logs push up.
/// 240ms at 60fps is about 14 frames, so each row slides into view over ~4 frames: visible but never sluggish.
pub(crate) const POPUP_APPEAR_MS: u64 = 240;
/// Per-row slide columns (easeOutCubic, same easing as the marker slide; 8 columns read clearly).
const POPUP_SLIDE_COLS: usize = 8;
/// Birth span: row i emerges at `p = i/n*0.7` (every row is born before 0.7),
/// then slides plus fades into place within 0.3 units (same schedule as [`animated_popup_count`]).
const POPUP_BIRTH_SPAN: f32 = 0.7;
/// Popup fade start (pure black): mid-animation foreground and selection backgrounds interpolate from black,
/// which reads as fade-in on dark terminals; on light terminals the first frame is readable black text.
const POPUP_FADE_FROM: Rgb = Rgb(0, 0, 0);
/// Popup text steady color approximations (bright cyan/yellow): animation interpolation only,
/// steady still uses theme colors (switches back with no jump); description gray reuses the exact 245 gray.
const POPUP_DISPLAY_RGB: Rgb = Rgb(95, 255, 255);
const POPUP_ALIAS_RGB: Rgb = Rgb(255, 255, 135);
/// Popup selected-row background (steady gray): fades in from black the same way, with matching endpoints.
const POPUP_SELECTED_BG: Rgb = Rgb(58, 58, 58);

/// Popup visible rows (popping upward): grows from 0 to `target` with appear age while logs push up instead of jumping.
///
/// Same birth schedule as [`popup_row_anim`]: count of born rows (indexes `i` with `birth_i` below `p`);
/// pure function (explicit clock) for unit tests.
pub(crate) fn animated_popup_count(age_ms: Option<u64>, target: usize) -> usize {
    let Some(age) = age_ms else {
        return target;
    };
    if target == 0 {
        return 0;
    }
    let p = (age as f32 / POPUP_APPEAR_MS as f32).clamp(0.0, 1.0);
    if p >= 1.0 {
        return target;
    }
    // birth_i = i/n*0.7 below p means i below p*n/0.7; the count is those rows (0 rows at p=0).
    ((p * target as f32 / POPUP_BIRTH_SPAN).ceil() as usize).min(target)
}

/// Row `i` of `n` target rows at appear age `age_ms`:
/// (visible, left slide-in spaces, opacity 0..1).
///
/// `None` means steady (fully shown with no offset at full brightness); `n` is the target row total
/// (layout inclusion already guarantees birth; the animation only decorates first appearance).
pub(crate) fn popup_row_anim(age_ms: Option<u64>, i: usize, n: usize) -> (bool, usize, f32) {
    let Some(age) = age_ms else {
        return (true, 0, 1.0);
    };
    if n == 0 {
        return (false, 0, 0.0);
    }
    let p = (age as f32 / POPUP_APPEAR_MS as f32).clamp(0.0, 1.0);
    if p >= 1.0 {
        return (true, 0, 1.0);
    }
    let birth = i as f32 / n as f32 * POPUP_BIRTH_SPAN;
    let local = ((p - birth) / (1.0 - POPUP_BIRTH_SPAN)).clamp(0.0, 1.0);
    if local <= 0.0 {
        return (false, 0, 0.0);
    }
    let eased = ease_out_cubic(local);
    let indent = ((1.0 - eased) * POPUP_SLIDE_COLS as f32).round() as usize;
    (true, indent, eased)
}

/// Sweeping/Running input box: wrapped editing plus marker (slides in from the window edge during the sweep).
#[allow(clippy::too_many_arguments)]
fn render_input_box(
    buf: &mut String,
    f: &Frame,
    bg_at: &dyn Fn(usize, usize) -> Rgb,
    progress: f32,
    width: usize,
    input_base: usize,
    styled: bool,
    cursor_xy: &mut (u16, u16),
) {
    let sweeping = f.phase == ServerPhase::Sweeping;
    let text = f.editor.text();
    let cursor = f.editor.cursor().min(text.chars().count());
    let chars: Vec<char> = text.chars().collect();
    let rows = wrap_lines(
        &chars,
        width.saturating_sub(TEXT_COL),
        width.saturating_sub(INPUT_MARGIN),
    );
    let (cursor_vrow, cursor_in_row) = locate_cursor(&rows, cursor);
    // Display window (keeps the cursor visible; the content area is rows 1..3, at most 3 rows at once).
    const WRAP_ROWS: usize = INPUT_TEXT_ROWS - INPUT_PAD_TOP; // 3
    let max_start = rows.len().saturating_sub(WRAP_ROWS);
    let start_row = cursor_vrow.saturating_sub(WRAP_ROWS - 1).min(max_start);
    // During the sweep the marker slides from the window edge (column 0) to the content indent and fades in.
    let slide_x = if sweeping {
        ((INPUT_MARGIN as f32 * ease_out_cubic(progress)) as usize).min(INPUT_MARGIN)
    } else {
        INPUT_MARGIN
    };
    let slide_fg = if sweeping {
        mix_rgb(SLIDE_BLUE_DIM, LIGHT_BLUE, progress)
    } else {
        LIGHT_BLUE
    };
    let margin = " ".repeat(INPUT_MARGIN);
    let show_startup_text = sweeping && progress < 0.3;
    let show_content = !sweeping || progress > 0.55;
    for i in 0..INPUT_TEXT_ROWS {
        // Row 0 is top padding; content rows i=1..=3 map to wrap rows start_row+(i-1).
        let content_i = i.saturating_sub(INPUT_PAD_TOP);
        let vrow = start_row + content_i;
        let is_first_display = i == INPUT_PAD_TOP && start_row == 0;
        if i == 0 {
            // First-row top right: copy notice (gray-blue, opacity drives the fade).
            if let Some((note, opacity)) = f.copy_note.as_ref() {
                let fg = mix_rgb(COPY_DIM, COPY_BRIGHT, *opacity);
                let note_w = display_width(note);
                let pad = " ".repeat(width.saturating_sub(note_w));
                emit_shaded_row(
                    buf,
                    &[(pad.as_str(), None), (note.as_str(), Some(Fg::Rgb(fg)))],
                    bg_at,
                    i,
                    0,
                    width,
                    styled,
                );
            } else {
                emit_shaded_row(buf, &[], bg_at, i, 0, width, styled);
            }
        } else if show_startup_text && i == 2 {
            // Early sweep: the boot text still sits centered before the light wave swallows it.
            let starting =
                rust_i18n::t!("tui.server_starting", locale = f.lang).to_string();
            let pad = width.saturating_sub(display_width(&starting)) / 2;
            let pad_str = " ".repeat(pad);
            emit_shaded_row(
                buf,
                &[
                    (pad_str.as_str(), None),
                    (starting.as_str(), Some(Fg::Code(253))),
                ],
                bg_at,
                i,
                0,
                width,
                styled,
            );
        } else if i >= INPUT_PAD_TOP
            && vrow < rows.len()
            && (show_content || (sweeping && is_first_display))
        {
            let row = &rows[vrow];
            // Owned dynamic spans: prepared first, then assembled (same scope, never dropped before emit).
            let row_text: String = row.iter().collect();
            let placeholder =
                rust_i18n::t!("tui.input_placeholder", locale = f.lang).to_string();
            let slide_pad = " ".repeat(slide_x);
            let fill_pad = " ".repeat((INPUT_MARGIN + PROMPT_W).saturating_sub(slide_x + 1));
            let mut segs: Vec<Segment> = Vec::new();
            if is_first_display {
                if sweeping {
                    // While sliding: the marker stays visible the whole way (slides alone before content), then pads to the text column.
                    if slide_x > 0 {
                        segs.push((slide_pad.as_str(), None));
                    }
                    segs.push(("❯", Some(Fg::Rgb(slide_fg))));
                    if !fill_pad.is_empty() {
                        segs.push((fill_pad.as_str(), None));
                    }
                    if show_content {
                        if text.is_empty() {
                            segs.push((
                                placeholder.as_str(),
                                Some(Fg::Code(PLACEHOLDER_GRAY)),
                            ));
                        } else {
                            segs.push((row_text.as_str(), None));
                        }
                    }
                } else {
                    segs.push((margin.as_str(), None));
                    segs.push((PROMPT_TEXT, Some(Fg::Rgb(LIGHT_BLUE))));
                    if text.is_empty() {
                        segs.push((
                            placeholder.as_str(),
                            Some(Fg::Code(PLACEHOLDER_GRAY)),
                        ));
                    } else {
                        segs.push((row_text.as_str(), None));
                    }
                }
            } else {
                segs.push((margin.as_str(), None));
                if show_content {
                    segs.push((row_text.as_str(), None));
                }
            }
            // Cursor column: first row uses the text start column, otherwise the indent.
            if vrow == cursor_vrow {
                let base = if is_first_display {
                    TEXT_COL
                } else {
                    INPUT_MARGIN
                };
                cursor_xy.0 = (base + row_prefix_width(row, cursor_in_row)) as u16;
                cursor_xy.1 = (input_base + i) as u16;
            }
            emit_shaded_row(buf, &segs, bg_at, i, 0, width, styled);
        } else {
            emit_shaded_row(buf, &[], bg_at, i, 0, width, styled);
        }
        buf.push_str("\r\n");
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
