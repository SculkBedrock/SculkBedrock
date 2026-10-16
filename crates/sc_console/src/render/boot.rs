//! TUI rendering: boot section (split from `render.rs`; re-exported at `crate::render`).

use super::*;

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
pub(super) const META_LABEL_RGB: Rgb = Rgb(138, 138, 138);
pub(super) const META_VALUE_RGB: Rgb = Rgb(188, 188, 188);
/// Boot text steady light gray (fixed color 253).
pub(super) const STARTUP_TEXT_RGB: Rgb = Rgb(218, 218, 218);

/// Stretching boot bar row: the first `bar_len` columns use the bar background, the rest keep the default
/// (transparent, never pre-painted), growing left to right; no text appears until the stretch fills.
pub(super) fn emit_growing_bar(buf: &mut String, bar_len: usize, width: usize, styled: bool) {
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
pub(super) fn emit_boot_title_row(
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
pub(super) fn render_boot_header(
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
