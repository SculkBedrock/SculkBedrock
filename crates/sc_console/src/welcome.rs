//! First-boot welcome screen: top-left logo plus flowing background plus a "press any key" prompt.
//!
//! - Logo glyphs follow the cfonts tiny font (measured via `cfonts "SCULK BEDROCK" -f tiny`, 3 cols x 2 rows per char),
//!   with a bottom-left `#87A5F8` to top-right `#FDFBFE` diagonal gradient and an outer glow ring;
//! - Background blends `#13348A` and `#0A1026` with dual-sine flow (continuous RGB, truecolor output);
//! - The prompt sits centered-low and shines left-to-right for 600ms of each 1800ms cycle before leaving.
//!
//! Pure rendering plus an explicit clock (caller passes `age_ms`), unit-testable; the driver only waits for a key.
//! Depends on no game crate.

use unicode_width::UnicodeWidthChar;

use crate::ansi::{push_bg_rgb, push_fg_rgb, Rgb};
use crate::render::{mix_rgb, smoothstep};

/// Logo text (measured cfonts tiny layout: 1 leading space, 1 space between chars, 3 between words).
pub(crate) const LOGO_TEXT: &str = "SCULK BEDROCK";
/// Logo grid size (cols x rows, including leading/inter-word spaces, byte-identical to cfonts output).
pub(crate) const LOGO_W: usize = 50;
pub(crate) const LOGO_H: usize = 2;
/// Logo top-left margin (cols/rows).
pub(crate) const LOGO_X: usize = 3;
pub(crate) const LOGO_Y: usize = 2;

/// cfonts tiny font (measured glyphs: letters 3 cols, `1` 2 cols, `.`/space 1 col; all 2 rows).
///
/// Unknown chars map to 3 blank cols, keeping widths stable and iteration terminating. Returns (two glyph rows, per-char display width).
fn tiny_glyph(ch: char) -> ([&'static str; 2], usize) {
    match ch {
        'S' => (["█▀▀", "▄▄█"], 3),
        'C' => (["█▀▀", "█▄▄"], 3),
        'U' => (["█ █", "█▄█"], 3),
        'L' => (["█  ", "█▄▄"], 3),
        'K' => (["█▄▀", "█ █"], 3),
        'B' => (["█▄▄", "█▄█"], 3),
        'O' => (["█▀█", "█▄█"], 3),
        'R' => (["█▀█", "█▀▄"], 3),
        'E' => (["█▀▀", "██▄"], 3),
        'D' => (["█▀▄", "█▄▀"], 3),
        'A' => (["▄▀█", "█▀█"], 3),
        'P' => (["█▀█", "█▀▀"], 3),
        'H' => (["█ █", "█▀█"], 3),
        '1' => (["▄█", " █"], 2),
        '0' => (["█▀█", "█▄█"], 3), // Digit 0 reuses the letter O glyph (quadrant blocks look fragmented)
        '.' => ([" ", "▄"], 1),
        ' ' => ([" ", " "], 1),
        _ => (["   ", "   "], 3),
    }
}

/// Single-line tiny text grid (1 leading space, 1 space separator after each char).
///
/// Byte-identical to cfonts tiny output (including narrow glyphs).
pub(crate) fn tiny_text_grid(text: &str) -> Vec<Vec<Option<char>>> {
    let mut width = 1usize; // leading space
    for ch in text.chars() {
        width += tiny_glyph(ch).1 + 1;
    }
    let width = width.saturating_sub(1); // drop the trailing separator
    let mut grid = vec![vec![None; width]; LOGO_H];
    let mut x = 1usize;
    for ch in text.chars() {
        let (glyph, w) = tiny_glyph(ch);
        for (ly, row) in glyph.iter().enumerate() {
            for (lx, c) in row.chars().enumerate() {
                if lx >= w {
                    break;
                }
                let gx = x + lx;
                if gx < width && c != ' ' {
                    grid[ly][gx] = Some(c);
                }
            }
        }
        x += w + 1;
    }
    grid
}

/// Logo grid (tiny layout of `LOGO_TEXT`; unit tests only, rendering uses the generic layout).
#[cfg(test)]
pub(crate) fn logo_grid() -> Vec<Vec<Option<char>>> {
    tiny_text_grid(LOGO_TEXT)
}

/// Logo gradient: bottom-left `#87A5F8` to top-right `#FDFBFE` (diagonal).
pub(crate) const LOGO_GRAD_FROM: Rgb = Rgb(135, 165, 248); // #87A5F8
pub(crate) const LOGO_GRAD_TO: Rgb = Rgb(253, 251, 254); // #FDFBFE
/// Gold suffix right of the logo and light-blue version below (tiny font).
pub(crate) const LOGO_SUFFIX_TEXT: &str = "ALPHA";
pub(crate) const LOGO_VERSION_TEXT: &str = "1.0.0";
/// Gold suffix / light-blue version.
pub(crate) const SUFFIX_GOLD: Rgb = Rgb(255, 213, 79);
pub(crate) const VERSION_BLUE: Rgb = Rgb(135, 206, 250);
/// Gap cols between suffix and main title; the version is logo-left-aligned with one blank row above.
pub(crate) const LOGO_SUFFIX_GAP: usize = 4;
/// Semi-transparent prompt frame: full width, 3 rows tall, fading in with the prompt.
pub(crate) const PROMPT_BOX_H: usize = 3;
/// Prompt frame background (dark blue, translucent over the backdrop).
pub(crate) const PROMPT_BOX_BLUE: Rgb = Rgb(19, 52, 138); // #13348A
pub(crate) const PROMPT_BOX_ALPHA: f32 = 0.45;

/// Backdrop colors: deep blue `#13348A` and near-black navy `#0A1026` (blob blur base).
pub(crate) const BG_BLUE: Rgb = Rgb(19, 52, 138); // #13348A
pub(crate) const BG_DEEP: Rgb = Rgb(10, 16, 38); // #0A1026

/// Backdrop blob count (deterministic seed, reproducible in unit tests).
pub(crate) const BG_BLOB_COUNT: usize = 3;
/// Blob random seed (fixed; same size and time always give the same output).
const BG_BLOB_SEED: u64 = 0x9E3779B97F4A7C15;

/// Gather duration (ms): blobs rush in from far outside the viewport to random rest spots, then start drifting.
pub(crate) const BLOB_GATHER_MS: u64 = 1100;
/// Logo/prompt fade-in duration (ms): fades in after gathering (fully invisible while gathering).
pub(crate) const WELCOME_FADE_MS: u64 = 600;

/// Blob bright core (brightened `#13348A` family) and blur base (always `#0A1026`).
pub(crate) const BLOB_CORE: Rgb = Rgb(48, 130, 255); // Brightened #13348A family

/// Deterministic PRNG (for blob layout; no third-party dependency).
struct BlobRng(u64);

impl BlobRng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn next_f64(&mut self) -> f64 {
        const DIV: f64 = (1u64 << 53) as f64;
        ((self.next_u64() >> 11) as f64) / DIV
    }
}

/// Blob snapshot (square units with gather/drift resolved; `sigma` is the Gaussian radius, `k` the peak intensity).
#[derive(Debug, PartialEq)]
pub(crate) struct BlobSpot {
    pub x: f64,
    pub y: f64,
    pub sigma: f64,
    pub k: f64,
}

/// Reflecting drift (1D triangle wave): `p0` start, `v` velocity, `dt` duration, bouncing within `[lo, hi]`.
///
/// The box is slightly larger than the viewport (may overshoot slightly, never fully leaves).
fn drift_reflect(p0: f64, v: f64, dt: f64, lo: f64, hi: f64) -> f64 {
    let span = (hi - lo).max(0.001);
    let mut u = (p0 + v * dt - lo) % (2.0 * span);
    if u < 0.0 {
        u += 2.0 * span;
    }
    lo + if u <= span { u } else { 2.0 * span - u }
}

/// Blob diameter (cols): viewport `(w+h)/2`; all three share the size.
pub(crate) fn blob_diameter(w: usize, h: usize) -> f64 {
    (w.max(1) + h.max(1)) as f64 * 0.5
}

/// Exit flight duration (ms): after a keypress blobs accelerate outward radially while the backdrop fades.
pub(crate) const WELCOME_EXIT_MS: u64 = 600;

/// Exit progress 0..1 (always 0 for `None` or before the dismiss moment).
pub(crate) fn exit_progress(age_ms: u64, dismissed_ms: Option<u64>) -> f32 {
    match dismissed_ms {
        Some(d) if age_ms >= d => ((age_ms - d) as f32 / WELCOME_EXIT_MS as f32).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// Blob layout (square units: cols scaled by 0.5 for terminal cell aspect ratio, so blobs look round):
/// Three blobs share one diameter (half the viewport w+h sum) and rest in left/center/right thirds (jittered by half a third,
/// never leaving their third or clumping); each starts pushed from the center along a random direction by 1.2x the diagonal plus radius
/// (far outside the viewport); gathering eases start to rest with easeOutCubic (fast entry), then drifts slowly
/// in a random direction from rest with edge reflection (slight overshoot allowed, never detaches);
/// After keypress dismissal (`dismissed_ms`): positions freeze at the keypress moment, then accelerate
/// outward radially with easeInCubic (far enough to leave the screen).
///
/// `shift` 0..1 slides the rest thirds rightward (language selector: the left
/// panel takes the left side, blobs glide to the right; eases via [`shift_at`]).
/// Pure function (seed plus size plus clock decides everything) for easy unit tests.
pub(crate) fn blob_spots(
    w: usize,
    h: usize,
    age_ms: u64,
    dismissed_ms: Option<u64>,
    shift: f32,
) -> Vec<BlobSpot> {
    use crate::render::ease_out_sine;
    use std::f64::consts::TAU;
    let wu = (w.max(1) as f64) * 0.5;
    let hu = h.max(1) as f64;
    let diag = (wu * wu + hu * hu).sqrt();
    let radius = blob_diameter(w, h) / 4.0;
    // Rest thirds slide right with `shift`: full width at 0, right portion at 1.
    let lo = shift.clamp(0.0, 1.0) as f64 * wu * 0.5;
    let zone_w = (wu - lo) / BG_BLOB_COUNT as f64;
    let mut rng = BlobRng(BG_BLOB_SEED);
    let mut out = Vec::with_capacity(BG_BLOB_COUNT);
    // On exit freeze at the keypress moment, then play only the flight animation (backdrop/text fade separately).
    let frozen_ms = dismissed_ms.map(|d| age_ms.min(d)).unwrap_or(age_ms);
    let exit_p = exit_progress(age_ms, dismissed_ms);
    let exit_e = exit_p * exit_p * exit_p; // easeInCubic: accelerating exit
    for i in 0..BG_BLOB_COUNT {
        // Rest spot: blob i stays in vertical third i (jittered by +-1/4 third width, never out of third).
        let tx = lo + (i as f64 + 0.5) * zone_w + (rng.next_f64() - 0.5) * zone_w * 0.5;
        let ty = (0.08 + rng.next_f64() * 0.84) * hu;
        let sa = rng.next_f64() * TAU;
        // Start: pushed from the center along a random direction by 1.2x diagonal plus radius (rushing in from far outside).
        let l = diag * 1.2 + radius + 4.0;
        let sx = wu * 0.5 + sa.cos() * l;
        let sy = hu * 0.5 + sa.sin() * l;
        let da = rng.next_f64() * TAU;
        let sp = 1.0 + rng.next_f64() * 1.5;
        let k = 0.85 + rng.next_f64() * 0.15;
        let frozen_sec = frozen_ms as f64 / 1000.0;
        let (mut px, mut py) = if frozen_ms < BLOB_GATHER_MS {
            // Sine easing: soft entry with zero velocity at rest.
            let e = ease_out_sine((frozen_ms as f64 / BLOB_GATHER_MS as f64) as f32) as f64;
            (sx + (tx - sx) * e, sy + (ty - sy) * e)
        } else {
            let dt = frozen_sec - BLOB_GATHER_MS as f64 / 1000.0;
            (
                drift_reflect(tx, da.cos() * sp, dt, lo - 2.0, wu + 2.0),
                drift_reflect(ty, da.sin() * sp, dt, -2.0, hu + 2.0),
            )
        };
        if exit_p > 0.0 {
            // Fly out radially (relative to screen center; exactly-centered blobs go +x so a direction always exists).
            let mut dx = px - wu * 0.5;
            let dy = py - hu * 0.5;
            if dx == 0.0 && dy == 0.0 {
                dx = 1.0;
            }
            let len = (dx * dx + dy * dy).sqrt().max(0.001);
            let travel = diag + radius * 2.0 + 8.0;
            px += dx / len * travel * exit_e as f64;
            py += dy / len * travel * exit_e as f64;
        }
        out.push(BlobSpot {
            x: px,
            y: py,
            // Visible influence radius is ~1.1x the radius; three blobs split the viewport roughly half bright/half dark (locked by unit test).
            sigma: (radius / 1.85).max(0.5),
            k,
        });
    }
    out
}

/// Gaussian field (multi-blob sum clamped to 0..1): base always `#0A1026` with bright cores rising.
pub(crate) fn blob_field(spots: &[BlobSpot], x: usize, y: usize) -> f32 {
    let ux = x as f64 * 0.5;
    let uy = y as f64;
    let mut acc = 0.0;
    for s in spots {
        let dx = ux - s.x;
        let dy = uy - s.y;
        acc += s.k * (-(dx * dx + dy * dy) / (2.0 * s.sigma * s.sigma)).exp();
    }
    acc.clamp(0.0, 1.0) as f32
}

/// Blob color (bright core to bright blue to navy base, keeping the `#13348A` family present).
fn blob_color(field: f32) -> Rgb {
    let f = field.clamp(0.0, 1.0);
    if f < 0.5 {
        mix_rgb(BG_DEEP, BG_BLUE, f * 2.0)
    } else {
        mix_rgb(BG_BLUE, BLOB_CORE, f * 2.0 - 1.0)
    }
}

/// Welcome backdrop color (blob blur base always `#0A1026`; unit tests only, rendering reuses snapshots).
#[cfg(test)]
pub(crate) fn welcome_bg(
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    age_ms: u64,
    dismissed_ms: Option<u64>,
) -> Rgb {
    blob_color(blob_field(&blob_spots(w, h, age_ms, dismissed_ms, 0.0), x, y))
}

/// Language-selector glide duration (ms): blobs ease from full-width thirds to
/// the right portion after the selector opens.
pub(crate) const SELECT_SHIFT_MS: u64 = 600;

/// Blob right-shift 0..1 (`None` = selector never opened, always 0).
///
/// Pure (clocks only) for unit tests; the render side freezes it at dismissal
/// together with positions (see `render_welcome_rows_view`).
pub(crate) fn shift_at(age_ms: u64, select_enter_ms: Option<u64>) -> f32 {
    match select_enter_ms {
        Some(enter) if age_ms >= enter => {
            smoothstep(((age_ms - enter) as f32 / SELECT_SHIFT_MS as f32).clamp(0.0, 1.0))
        }
        _ => 0.0,
    }
}

/// Auto-enter moment (ms): a returning run (language already chosen) shows the
/// gather plus fade-in, holds briefly, then dismisses itself into the exit
/// flight — no press-any-key, no selector.
pub(crate) const WELCOME_HOLD_MS: u64 = 800;
pub(crate) const WELCOME_AUTO_DISMISS_MS: u64 = BLOB_GATHER_MS + WELCOME_FADE_MS + WELCOME_HOLD_MS;

/// Welcome stage (drives prompt/selector/auto rendering; the driver owns transitions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WelcomeStage {
    /// First run: logo, blobs, press-any-key prompt.
    Prompt,
    /// Choosing a language: logo, blobs gliding right, selector panel on the left.
    Select,
    /// Returning run: logo and blobs only; the driver auto-dismisses.
    Auto,
}

/// Per-frame view params for [`render_welcome_rows_view`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct WelcomeView {
    pub stage: WelcomeStage,
    /// Highlighted option index (selector only).
    pub selected: usize,
    /// Selector-open moment (selector only; drives the blob rightward glide).
    pub select_enter_ms: Option<u64>,
    /// Checkbox focus (selector only; the quote row takes the accent).
    pub quote_focus: bool,
    /// Quote checkbox state (selector only; meaningful for Chinese).
    pub hitokoto: bool,
}

impl WelcomeView {
    pub(crate) fn prompt() -> Self {
        Self {
            stage: WelcomeStage::Prompt,
            selected: 0,
            select_enter_ms: None,
            quote_focus: false,
            hitokoto: false,
        }
    }

    pub(crate) fn auto() -> Self {
        Self {
            stage: WelcomeStage::Auto,
            selected: 0,
            select_enter_ms: None,
            quote_focus: false,
            hitokoto: false,
        }
    }

    pub(crate) fn select(
        selected: usize,
        enter_ms: u64,
        quote_focus: bool,
        hitokoto: bool,
    ) -> Self {
        Self {
            stage: WelcomeStage::Select,
            selected,
            select_enter_ms: Some(enter_ms),
            quote_focus,
            hitokoto,
        }
    }
}

/// Selector panel geometry (viewport-left option list; unit-tested, shared by
/// rendering; keyboard-only, no hit-testing needed).
#[derive(Debug)]
pub(crate) struct SelectPanel {
    pub x0: usize,
    pub y0: usize,
    pub width: usize,
    pub height: usize,
    pub title_y: usize,
    pub opt_y: Vec<usize>,
    pub hint_y: usize,
    pub hint_y2: usize,
}

/// Selector chrome (bilingual by construction: shown before any choice exists,
/// so both languages are visible at once; option labels come from each
/// locale's own `tui.language_name` and need no translation).
pub(crate) const SELECT_TITLE: &str = "Choose language / 请选择语言";
/// Selector hints, one row per language (the combined line no longer fits).
pub(crate) const SELECT_HINT_ZH: &str = "↑↓ 选语言 · ←→ 一言框 · 空格打钩 · 回车确认";
pub(crate) const SELECT_HINT_EN: &str = "↑↓ language · ←→ quote · Space check · Enter confirm";
/// Daily-quote box title (bilingual static row).
pub(crate) const SELECT_QUOTE_TITLE: &str = "一言 / Daily quote";
/// Daily-quote checkbox text (bilingual static row; the box glyph toggles too).
pub(crate) const SELECT_QUOTE_ON: &str = "开启 / ON";
pub(crate) const SELECT_QUOTE_OFF: &str = "关闭 / OFF";
/// Selector panel box (same translucent dark-blue family as the prompt box).
pub(crate) const SELECT_BOX_ALPHA: f32 = 0.5;
/// Selected option row (accent background, near-white text, `❯` marker).
pub(crate) const SELECT_BG: Rgb = Rgb(52, 110, 205);
pub(crate) const SELECT_FG: Rgb = Rgb(255, 255, 255);

/// Selector option labels in selector order (each in its own language).
pub(crate) fn select_options() -> Vec<String> {
    crate::config::selector_order()
        .iter()
        .map(|code| rust_i18n::t!("tui.language_name", locale = code.as_str()).to_string())
        .collect()
}

/// Selector panel layout: left-docked box, vertically centered block
/// (title, blank, one row per option, blank, hint) with one padding row/col around.
pub(crate) fn select_panel(w: usize, h: usize, options: &[String]) -> SelectPanel {
    use crate::ansi::display_width;
    let title_w = display_width(SELECT_TITLE);
    let hint_w = display_width(SELECT_HINT_ZH).max(display_width(SELECT_HINT_EN));
    let opts_w = options.iter().map(|o| display_width(o) + 2).max().unwrap_or(0); // 2-col marker
    let inner_w = title_w.max(hint_w).max(opts_w).max(1);
    // Box width caps at the viewport (narrow windows truncate rows, never panic).
    let width = (inner_w + 4).min(w.max(1));
    let x0 = 4.min(w.saturating_sub(width));
    let rows = options.len() + 5; // title + blank + options + blank + hint×2
    let height = rows + 2; // padding rows top/bottom
    let y0 = h.saturating_sub(height) / 2;
    let title_y = y0 + 1;
    let opt_y = (0..options.len()).map(|i| title_y + 2 + i).collect();
    let hint_y = title_y + 2 + options.len() + 1;
    SelectPanel {
        x0,
        y0,
        width,
        height,
        title_y,
        opt_y,
        hint_y,
        hint_y2: hint_y + 1,
    }
}

/// Daily-quote box geometry (viewport-right companion of the selector panel).
///
/// Shown only while a Chinese option is highlighted; the `←→` keys move focus
/// between the left language list and this box.
#[derive(Debug)]
pub(crate) struct QuotePanel {
    pub x0: usize,
    pub y0: usize,
    pub width: usize,
    pub height: usize,
    pub title_y: usize,
    pub box_y: usize,
}

/// Daily-quote box layout: right-docked box (title, blank, checkbox row) with
/// one padding row/col around, vertically centered.
pub(crate) fn quote_panel(w: usize, h: usize) -> QuotePanel {
    use crate::ansi::display_width;
    let title_w = display_width(SELECT_QUOTE_TITLE);
    // Checkbox row: marker (2) + box glyph (4) + longer of the on/off labels.
    let box_w =
        2 + 4 + display_width(SELECT_QUOTE_ON).max(display_width(SELECT_QUOTE_OFF));
    let inner_w = title_w.max(box_w).max(1);
    let width = (inner_w + 4).min(w.max(1));
    let x1 = w.saturating_sub(4);
    let x0 = x1.saturating_sub(width);
    let height = 3 + 2; // title + blank + checkbox + padding rows top/bottom
    let y0 = h.saturating_sub(height) / 2;
    QuotePanel {
        x0,
        y0,
        width,
        height,
        title_y: y0 + 1,
        box_y: y0 + 3,
    }
}

/// Logo/prompt fade progress 0..1 (0 before gathering completes, then fades in).
pub(crate) fn welcome_fade_op(age_ms: u64) -> f32 {
    if age_ms < BLOB_GATHER_MS {
        0.0
    } else {
        smoothstep(((age_ms - BLOB_GATHER_MS) as f32 / WELCOME_FADE_MS as f32).clamp(0.0, 1.0))
    }
}

/// Logo diagonal gradient progress 0..1 (bottom-left 0 to top-right 1; `lx/ly` are logo-local coords).
pub(crate) fn logo_gradient_t(lx: usize, ly: usize) -> f32 {
    let fx = if LOGO_W > 1 {
        lx as f32 / (LOGO_W - 1) as f32
    } else {
        0.0
    };
    let fy = if LOGO_H > 1 {
        ly as f32 / (LOGO_H - 1) as f32
    } else {
        0.0
    };
    ((fx + (1.0 - fy)) * 0.5).clamp(0.0, 1.0)
}

/// Prompt text (centered, inside the translucent frame) and shine timing.
pub(crate) const PROMPT_TEXT: &str = "PRESS ANY KEY TO START";
/// Prompt base color (light gray-blue) and shine peak (near white).
pub(crate) const PROMPT_BASE: Rgb = Rgb(168, 186, 214);
pub(crate) const PROMPT_PEAK: Rgb = Rgb(255, 255, 255);
/// Shine period/sweep length (ms): one sweep per period, then waits offstage.
pub(crate) const PROMPT_PERIOD_MS: u64 = 1800;
pub(crate) const PROMPT_SWEEP_MS: u64 = 600;

/// Prompt shine progress 0..1 (`None` means the in-period wait with no shine).
pub(crate) fn prompt_shine_t(age_ms: u64) -> Option<f32> {
    sweep_t(age_ms, PROMPT_PERIOD_MS, PROMPT_SWEEP_MS)
}

/// Suffix shine progress 0..1 (staggered against the prompt; `None` means waiting).
pub(crate) const SUFFIX_PERIOD_MS: u64 = 1800;
pub(crate) const SUFFIX_SWEEP_MS: u64 = 700;
pub(crate) const SUFFIX_PHASE_MS: u64 = 900;

/// Generic shine phase: one sweep per period, then waits offstage.
fn sweep_t(age_ms: u64, period_ms: u64, sweep_ms: u64) -> Option<f32> {
    let ph = age_ms % period_ms;
    if ph < sweep_ms {
        Some(ph as f32 / sweep_ms as f32)
    } else {
        None
    }
}

/// Suffix shine progress (half a period off from the prompt, alternating flashes).
pub(crate) fn suffix_shine_t(age_ms: u64) -> Option<f32> {
    sweep_t(
        age_ms.wrapping_add(SUFFIX_PHASE_MS),
        SUFFIX_PERIOD_MS,
        SUFFIX_SWEEP_MS,
    )
}

/// Shine-wave highlight at char `i` (wave sweeps left to right then leaves; zero at both ends).
///
/// Same waveform as the title shine (leading edge `t*(n+6)-3`, width 1.5).
pub(crate) fn shine_glow(i: usize, n: usize, shine_t: Option<f32>) -> f32 {
    let Some(t) = shine_t else {
        return 0.0;
    };
    let front = t.clamp(0.0, 1.0) * (n as f32 + 6.0) - 3.0;
    let d = i as f32 - front;
    (-(d * d) / (2.0 * 1.5 * 1.5)).exp()
}

/// Backdrop quantization step (merges adjacent same-color escapes; dark steps are invisible to the eye).
///
/// Keep 4 (fine): a measured step of 32 cuts backdrop escapes from 2101 to 533,
/// but adjacent-cell relative luminance jumps by 4.0 (visible dark banding), not worth it: sluggish keys come
/// from animation length and input lock, not bandwidth (see `next_frame_interval`).
const BG_QUANT: u8 = 4;

fn quantize(c: Rgb) -> Rgb {
    Rgb(
        c.0 & !(BG_QUANT - 1),
        c.1 & !(BG_QUANT - 1),
        c.2 & !(BG_QUANT - 1),
    )
}

/// One selector panel text row (absolute row plus display-col-precise cells).
struct PanelTextRow {
    y: usize,
    cells: Vec<(usize, char, Rgb)>,
    selected: bool,
}

/// Daily-quote checkbox row state (`None` = row hidden, Chinese not highlighted).
pub(crate) struct QuoteRow {
    pub focused: bool,
    pub on: bool,
}

/// Lay one text line into display-col-precise cells (CJK-safe).
///
/// Rows start at `x0 + 2` (one padding col each side); overflow chars are
/// dropped whole (never split).
fn lay_text_line(text: &str, fg: Rgb, x0: usize, max_col: usize) -> Vec<(usize, char, Rgb)> {
    let mut cells = Vec::new();
    let mut col = x0 + 2;
    for ch in text.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
        if col + cw > max_col {
            break;
        }
        cells.push((col, ch, fg));
        col += cw;
    }
    cells
}

/// Lay out panel text (title, options, hint) inside the panel box, CJK-safe.
///
/// Rows past the viewport simply never match.
fn panel_text_rows(
    panel: &SelectPanel,
    options: &[String],
    selected: usize,
) -> Vec<PanelTextRow> {
    let max_col = panel.x0 + panel.width.saturating_sub(1);
    let mut rows = Vec::new();
    rows.push(PanelTextRow {
        y: panel.title_y,
        cells: lay_text_line(SELECT_TITLE, PROMPT_BASE, panel.x0, max_col),
        selected: false,
    });
    for (i, opt) in options.iter().enumerate() {
        let is_sel = i == selected;
        let marker = if is_sel { "❯ " } else { "  " };
        let text = format!("{marker}{opt}");
        rows.push(PanelTextRow {
            y: panel.opt_y[i],
            cells: lay_text_line(
                &text,
                if is_sel { SELECT_FG } else { PROMPT_BASE },
                panel.x0,
                max_col,
            ),
            selected: is_sel,
        });
    }
    rows.push(PanelTextRow {
        y: panel.hint_y,
        cells: lay_text_line(SELECT_HINT_ZH, VERSION_BLUE, panel.x0, max_col),
        selected: false,
    });
    rows.push(PanelTextRow {
        y: panel.hint_y2,
        cells: lay_text_line(SELECT_HINT_EN, VERSION_BLUE, panel.x0, max_col),
        selected: false,
    });
    rows
}

/// Daily-quote box text rows (title plus checkbox over the right-docked box).
fn quote_text_rows(panel: &QuotePanel, quote: QuoteRow) -> Vec<PanelTextRow> {
    let max_col = panel.x0 + panel.width.saturating_sub(1);
    let marker = if quote.focused { "❯ " } else { "  " };
    let box_glyph = if quote.on { "[✓] " } else { "[ ] " };
    let label = if quote.on {
        SELECT_QUOTE_ON
    } else {
        SELECT_QUOTE_OFF
    };
    vec![
        PanelTextRow {
            y: panel.title_y,
            cells: lay_text_line(SELECT_QUOTE_TITLE, PROMPT_BASE, panel.x0, max_col),
            selected: false,
        },
        PanelTextRow {
            y: panel.box_y,
            cells: lay_text_line(
                &format!("{marker}{box_glyph}{label}"),
                if quote.focused { SELECT_FG } else { PROMPT_BASE },
                panel.x0,
                max_col,
            ),
            selected: quote.focused,
        },
    ]
}

/// Full welcome render with an explicit view (stage, selection, selector clock).
///
/// Selector differences from the plain rows: the press-any-key prompt and its
/// frame stay hidden, blobs glide right ([`shift_at`], frozen at dismissal
/// together with positions), and the option panel draws over the left side.
pub(crate) fn render_welcome_rows_view(
    w: usize,
    h: usize,
    age_ms: u64,
    dismissed_ms: Option<u64>,
    styled: bool,
    view: WelcomeView,
) -> Vec<String> {
    let (w, h) = (w.max(1), h.max(1));
    // Ink cells (char plus base foreground): gradient main title, gold suffix, light-blue version.
    // The latter two fade in with the main title; overflow is clipped.
    let mut ink: Vec<Vec<Option<(char, Rgb)>>> = vec![vec![None; w]; h];
    let mut place_tiny = |text: &str, x0: usize, y0: usize, fg: &dyn Fn(usize, usize) -> Rgb| {
        for (ly, row) in tiny_text_grid(text).iter().enumerate() {
            for (lx, cell) in row.iter().enumerate() {
                if let Some(ch) = cell {
                    let (gx, gy) = (x0 + lx, y0 + ly);
                    if gx < w && gy < h {
                        ink[gy][gx] = Some((*ch, fg(lx, ly)));
                    }
                }
            }
        }
    };
    place_tiny(LOGO_TEXT, LOGO_X, LOGO_Y, &|lx, ly| {
        mix_rgb(LOGO_GRAD_FROM, LOGO_GRAD_TO, logo_gradient_t(lx, ly))
    });
    // Note: the gradient uses main-title-local coords (suffix/version use flat colors).
    place_tiny(
        LOGO_SUFFIX_TEXT,
        LOGO_X + LOGO_W + LOGO_SUFFIX_GAP,
        LOGO_Y,
        &|_, _| SUFFIX_GOLD,
    );
    place_tiny(LOGO_VERSION_TEXT, LOGO_X, LOGO_Y + LOGO_H + 1, &|_, _| {
        VERSION_BLUE
    });
    // Prompt position (centered, inside the translucent frame, frame 3 rows tall).
    let prompt: Vec<char> = PROMPT_TEXT.chars().collect();
    let prompt_w: usize = prompt
        .iter()
        .map(|c| UnicodeWidthChar::width(*c).unwrap_or(0))
        .sum();
    let prompt_x = w.saturating_sub(prompt_w) / 2;
    let prompt_y = (h * 4 / 5).min(h.saturating_sub(1));
    let box_top = prompt_y.saturating_sub(1);
    let shine = prompt_shine_t(age_ms);
    // Suffix shine (staggered against the prompt cycle); suffix span follows the tiny grid width.
    let suffix_shine = suffix_shine_t(age_ms);
    let suffix_x = LOGO_X + LOGO_W + LOGO_SUFFIX_GAP;
    let suffix_w = tiny_text_grid(LOGO_SUFFIX_TEXT)[0].len();
    // Logo/prompt fade in only after blobs finish gathering (gathering shows backdrop only).
    let fade = welcome_fade_op(age_ms);
    // On exit: backdrop fades to black and text goes dark together (opacity tracks the backdrop).
    let exit_op = smoothstep(exit_progress(age_ms, dismissed_ms));
    let dim = fade * (1.0 - exit_op);
    let show_text = dim > 0.0;
    // Press-any-key prompt text/frame only in the Prompt stage (selector and
    // auto runs show logo plus blobs only).
    let show_prompt = view.stage == WelcomeStage::Prompt && show_text;
    // Selector: panel content plus geometry (options come from loaded locales,
    // each in its own language, so no translation step exists). The quote
    // checkbox lives in its own right-docked box, shown only while a Chinese
    // option is highlighted (`←→` moves focus between the boxes).
    let selecting = view.stage == WelcomeStage::Select;
    let options: Vec<String> = if selecting { select_options() } else { Vec::new() };
    let order: Vec<String> = if selecting {
        crate::config::selector_order()
    } else {
        Vec::new()
    };
    let show_quote = selecting
        && order
            .get(view.selected % order.len().max(1))
            .is_some_and(|c| c == crate::config::LANG_ZH_CN);
    let panel: Option<SelectPanel> = if selecting {
        Some(select_panel(w, h, &options))
    } else {
        None
    };
    let quote_panel: Option<QuotePanel> = if show_quote {
        Some(quote_panel(w, h))
    } else {
        None
    };
    let quote = show_quote.then_some(QuoteRow {
        focused: view.quote_focus,
        on: view.hitokoto,
    });
    let mut panel_rows: Vec<PanelTextRow> = match &panel {
        Some(p) => panel_text_rows(p, &options, view.selected % options.len().max(1)),
        None => Vec::new(),
    };
    if let (Some(qp), Some(q)) = (&quote_panel, quote) {
        panel_rows.extend(quote_text_rows(qp, q));
    }
    let panel_rects: Vec<(usize, usize, usize, usize)> = panel
        .as_ref()
        .map(|p| {
            (
                p.x0,
                p.y0,
                p.x0.saturating_add(p.width),
                p.y0.saturating_add(p.height),
            )
        })
        .into_iter()
        .chain(quote_panel.as_ref().map(|p| {
            (
                p.x0,
                p.y0,
                p.x0.saturating_add(p.width),
                p.y0.saturating_add(p.height),
            )
        }))
        .collect();
    // Blob snapshot (once per frame, reused per cell; frozen plus flight on exit;
    // the selector glide freezes at dismissal together with positions).
    let shift_age = dismissed_ms.map(|d| age_ms.min(d)).unwrap_or(age_ms);
    let shift = shift_at(shift_age, view.select_enter_ms);
    let spots = blob_spots(w, h, age_ms, dismissed_ms, shift);
    // Draw cell by cell (backdrop run compression: one escape per same-color run).
    let mut rows = Vec::with_capacity(h);
    for y in 0..h {
        let mut row = String::with_capacity(w * 4);
        let mut last_bg: Option<Rgb> = None;
        let mut last_fg: Option<Rgb> = None;
        // Prompt chars for this row (col to (char, index); wide chars take two cells, second skipped).
        let mut x = 0usize;
        while x < w {
            let bg_raw = blob_color(blob_field(&spots, x, y));
            // Prompt blue frame (full width, 3 rows, fading with the prompt; 0 outside the frame).
            // Selector boxes instead (language left, quote right; same translucent family).
            let in_box = show_prompt && y >= box_top && y < box_top + PROMPT_BOX_H.min(h);
            let in_panel = panel_rects
                .iter()
                .any(|(x0, y0, x1, y1)| x >= *x0 && x < *x1 && y >= *y0 && y < *y1);
            let box_k = if in_box {
                PROMPT_BOX_ALPHA * dim
            } else if in_panel {
                SELECT_BOX_ALPHA * dim
            } else {
                0.0
            };
            let mut cell_bg = mix_rgb(
                mix_rgb(bg_raw, PROMPT_BOX_BLUE, box_k),
                Rgb(0, 0, 0),
                exit_op,
            );
            // Selected rows (language highlight or focused checkbox): accent
            // background (fades with everything else). Boxes sit side by side,
            // so scan every row sharing this y for a cell starting at x.
            let panel_hit: Option<(char, Rgb, bool)> = panel_rows
                .iter()
                .filter(|r| r.y == y)
                .find_map(|r| {
                    r.cells
                        .iter()
                        .find(|(c, _, _)| *c == x)
                        .map(|(_, ch, fg)| (*ch, *fg, r.selected))
                });
            if let Some((_, _, is_sel)) = panel_hit {
                if is_sel {
                    cell_bg = mix_rgb(cell_bg, SELECT_BG, 0.85 * dim);
                }
            }
            let bg = quantize(cell_bg);
            if last_bg != Some(bg) {
                push_bg_rgb(&mut row, bg, styled);
                last_bg = Some(bg);
            }
            // Selector option text wins over logo/prompt ink (the panel sits on top).
            if let Some((ch, base, _)) = panel_hit {
                let cw = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
                if x + cw <= w {
                    let fg = mix_rgb(cell_bg, base, dim);
                    if last_fg != Some(fg) {
                        push_fg_rgb(&mut row, fg, styled);
                        last_fg = Some(fg);
                    }
                    row.push(ch);
                    x += cw;
                    continue;
                }
            }
            // Ink text (logo gradient / gold suffix / light-blue version, fading toward the cell backdrop;
            // the suffix adds a looping gold shine advancing by column; without shine it stays at its base color).
            if show_text {
                if let Some((ch, base)) = ink.get(y).and_then(|r| r.get(x)).copied().flatten() {
                    let mut text_fg = base;
                    if (suffix_x..suffix_x + suffix_w).contains(&x)
                        && (LOGO_Y..LOGO_Y + LOGO_H).contains(&y)
                    {
                        let glow = shine_glow(x - suffix_x, suffix_w, suffix_shine);
                        text_fg = mix_rgb(SUFFIX_GOLD, PROMPT_PEAK, glow * 0.9);
                    }
                    let fg = mix_rgb(cell_bg, text_fg, dim);
                    if last_fg != Some(fg) {
                        push_fg_rgb(&mut row, fg, styled);
                        last_fg = Some(fg);
                    }
                    row.push(ch);
                    x += 1;
                    continue;
                }
            }
            // Prompt chars (shine-highlighted, fading toward the live backdrop; wide-char second cell skipped).
            // Prompt stage only (selector/auto runs show no prompt).
            if show_prompt && y == prompt_y && x >= prompt_x {
                let rel = x - prompt_x;
                // Map col to char index (accumulate by display cols, CJK safe).
                let mut acc = 0usize;
                let mut hit: Option<(usize, char)> = None;
                for (i, ch) in prompt.iter().enumerate() {
                    let cw = UnicodeWidthChar::width(*ch).unwrap_or(0);
                    if rel >= acc && rel < acc + cw {
                        hit = Some((i, *ch));
                        break;
                    }
                    acc += cw;
                }
                if let Some((i, ch)) = hit {
                    let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                    // Drop whole chars past the right edge (never split).
                    if x + cw <= w {
                        let glow = shine_glow(i, prompt.len(), shine);
                        let shine_fg = mix_rgb(PROMPT_BASE, PROMPT_PEAK, glow * 0.9);
                        let fg = mix_rgb(cell_bg, shine_fg, dim);
                        if last_fg != Some(fg) {
                            push_fg_rgb(&mut row, fg, styled);
                            last_fg = Some(fg);
                        }
                        row.push(ch);
                        x += cw;
                        continue;
                    }
                }
            }
            // Everything else is empty backdrop (logo has no glow or backing, text only).
            row.push(' ');
            x += 1;
        }
        rows.push(row);
    }
    rows
}

/// Fullscreen draw (absolute positioning, no clear/scroll; caller already entered the alternate screen).
///
/// Never clear or erase line tails on resize: both cause visible flicker while dragging/narrowing.
pub(crate) fn paint_welcome(
    out: &mut std::io::Stdout,
    w: u16,
    h: u16,
    age_ms: u64,
    dismissed_ms: Option<u64>,
    styled: bool,
    view: WelcomeView,
) -> std::io::Result<()> {
    use std::io::Write as _;
    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    let rows = render_welcome_rows_view(w, h, age_ms, dismissed_ms, styled, view);
    let mut buf = String::with_capacity(w * h * 2);
    buf.push_str("\x1b[?7l");
    for (i, row) in rows.iter().enumerate() {
        buf.push_str(&format!("\x1b[{};1H\x1b[0m", i + 1));
        buf.push_str(row);
        buf.push_str("\x1b[0m");
    }
    buf.push_str("\x1b[?7h");
    out.write_all(buf.as_bytes())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Measured cfonts tiny output (`cfonts "SCULK BEDROCK" -f tiny`, surrounding blank lines stripped).
    const CFONTS_ROW0: &str = " █▀▀ █▀▀ █ █ █   █▄▀   █▄▄ █▀▀ █▀▄ █▀█ █▀█ █▀▀ █▄▀";
    const CFONTS_ROW1: &str = " ▄▄█ █▄▄ █▄█ █▄▄ █ █   █▄█ ██▄ █▄▀ █▀▄ █▄█ █▄▄ █ █";

    fn grid_plain() -> Vec<String> {
        logo_grid()
            .iter()
            .map(|row| row.iter().map(|c| c.unwrap_or(' ')).collect::<String>())
            .collect()
    }

    #[test]
    fn logo_matches_cfonts_tiny() {
        // Grid is byte-identical to cfonts output (including leading space and 3-space word gaps).
        let plain = grid_plain();
        assert_eq!(plain.len(), LOGO_H);
        assert_eq!(plain[0], CFONTS_ROW0);
        assert_eq!(plain[1], CFONTS_ROW1);
        assert_eq!(crate::ansi::display_width(&plain[0]), LOGO_W);
        assert_eq!(crate::ansi::display_width(&plain[1]), LOGO_W);
    }

    #[test]
    fn logo_gradient_runs_bottom_left_to_top_right() {
        // Bottom-left marks 0 (#87A5F8), top-right marks 1 (#FDFBFE).
        assert_eq!(logo_gradient_t(0, LOGO_H - 1), 0.0);
        assert_eq!(logo_gradient_t(LOGO_W - 1, 0), 1.0);
        assert_eq!(
            mix_rgb(LOGO_GRAD_FROM, LOGO_GRAD_TO, logo_gradient_t(0, LOGO_H - 1)),
            LOGO_GRAD_FROM
        );
        assert_eq!(
            mix_rgb(LOGO_GRAD_FROM, LOGO_GRAD_TO, logo_gradient_t(LOGO_W - 1, 0)),
            LOGO_GRAD_TO
        );
        assert_eq!(LOGO_GRAD_FROM, Rgb(135, 165, 248), "#87A5F8");
        assert_eq!(LOGO_GRAD_TO, Rgb(253, 251, 254), "#FDFBFE");
    }

    #[test]
    fn background_blobs_drift_within_endpoints() {
        // Same params always give the same output (deterministic seed); blob drift varies a cell over time;
        // every channel stays inside the three-endpoint bounding box (blur base always #0A1026).
        let (w, h) = (100, 30);
        assert_eq!(
            welcome_bg(10, 5, w, h, 1000, None),
            welcome_bg(10, 5, w, h, 1000, None)
        );
        let changed = {
            // The blob-center cell must be bright (gathered onscreen by t=5); the same cell is deep space at t=0.
            let spots = blob_spots(w, h, 5000, None, 0.0);
            let s = &spots[0];
            let (cx, cy) = ((s.x * 2.0) as usize, s.y as usize);
            let (cx, cy) = (cx.min(w - 1), cy.min(h - 1));
            blob_field(&spots, cx, cy) > 0.3
                && welcome_bg(cx, cy, w, h, 0, None) != welcome_bg(cx, cy, w, h, 5000, None)
        };
        assert!(changed, "光斑随时间漂移");
        let corners = [BG_DEEP, BG_BLUE, BLOB_CORE];
        let (lo, hi) = (
            Rgb(
                corners.iter().map(|c| c.0).min().unwrap(),
                corners.iter().map(|c| c.1).min().unwrap(),
                corners.iter().map(|c| c.2).min().unwrap(),
            ),
            Rgb(
                corners.iter().map(|c| c.0).max().unwrap(),
                corners.iter().map(|c| c.1).max().unwrap(),
                corners.iter().map(|c| c.2).max().unwrap(),
            ),
        );
        for (x, y, t) in [(0, 0, 0.0), (10, 5, 0.0), (71, 19, 2.5), (99, 29, 9.0)] {
            let c = welcome_bg(x, y, w, h, (t * 1000.0) as u64, None);
            assert!(
                (lo.0..=hi.0).contains(&c.0)
                    && (lo.1..=hi.1).contains(&c.1)
                    && (lo.2..=hi.2).contains(&c.2),
                "包围盒内：{c:?}"
            );
        }
        assert_eq!(BG_BLUE, Rgb(19, 52, 138), "#13348A");
        assert_eq!(BG_DEEP, Rgb(10, 16, 38), "#0A1026");
        // The Gaussian field stays in 0..1 (clamped blending never breaks).
        let spots = blob_spots(w, h, 3300, None, 0.0);
        assert_eq!(spots.len(), BG_BLOB_COUNT);
        for (x, y) in [(0, 0), (50, 15), (99, 29)] {
            let f = blob_field(&spots, x, y);
            assert!((0.0..=1.0).contains(&f), "{f}");
        }
    }

    #[test]
    fn background_bright_dark_ratio_near_half() {
        // Bright/dark split is ~1:1: count bright cells against the dark/bright endpoint luminance midpoint.
        fn lum(c: Rgb) -> f32 {
            0.2126 * c.0 as f32 + 0.7152 * c.1 as f32 + 0.0722 * c.2 as f32
        }
        let mid = (lum(BG_DEEP) + lum(BLOB_CORE)) / 2.0;
        let (w, h) = (100, 30);
        for t in [2.0, 5.0, 9.0] {
            let mut bright = 0usize;
            for y in 0..h {
                for x in 0..w {
                    if lum(welcome_bg(x, y, w, h, (t * 1000.0) as u64, None)) > mid {
                        bright += 1;
                    }
                }
            }
            let ratio = bright as f32 / (w * h) as f32;
            assert!((0.35..=0.65).contains(&ratio), "t={t} 亮比 {ratio:.2}");
        }
    }

    #[test]
    fn blobs_gather_from_outside_then_drift_inside() {
        let (w, h) = (100, 30);
        let wu = w as f64 * 0.5;
        let outside = |p: (f64, f64)| p.0 < 0.0 || p.0 > wu || p.1 < 0.0 || p.1 > h as f64;
        // t=0: all three centers sit outside the viewport.
        let spots = blob_spots(w, h, 0, None, 0.0);
        assert_eq!(spots.len(), 3);
        assert!(
            spots.iter().all(|s| outside((s.x, s.y))),
            "起点屏外：{spots:?}"
        );
        // Gathering done (900ms): all at random onscreen rest spots.
        let spots = blob_spots(w, h, BLOB_GATHER_MS, None, 0.0);
        assert!(
            spots.iter().all(|s| !outside((s.x, s.y))),
            "落点屏内：{spots:?}"
        );
        // Mid-drift (60s): centers may only overshoot slightly, never fully detach.
        let spots = blob_spots(w, h, 60000, None, 0.0);
        for s in &spots {
            assert!(
                s.x >= -2.0 && s.x <= wu + 2.0 && s.y >= -2.0 && s.y <= h as f64 + 2.0,
                "有界漂移：{s:?}"
            );
        }
    }

    #[test]
    fn blobs_share_viewport_diameter_and_spread_across_zones() {
        // Diameter is pinned to viewport (w+h)/2: all three share sigma, derived from that diameter
        // (100x30 yields D=65; sigma=R/1.85 calibrated by the 1:1 bright/dark ratio test).
        let (w, h) = (100, 30);
        assert_eq!(blob_diameter(w, h), 65.0);
        let spots = blob_spots(w, h, BLOB_GATHER_MS, None, 0.0);
        assert_eq!(spots.len(), 3);
        for s in &spots {
            assert!((s.sigma - blob_diameter(w, h) / 4.0 / 1.85).abs() < 1e-9);
        }
        // Rest spots split across left/center/right thirds (jitter never leaves a third or clumps).
        let wu = w as f64 * 0.5;
        let zw = wu / 3.0;
        for (i, s) in spots.iter().enumerate() {
            assert!(
                s.x >= i as f64 * zw && s.x < (i + 1) as f64 * zw,
                "第 {i} 团在第 {i} 区：{s:?}"
            );
        }
    }

    #[test]
    fn shift_glides_blobs_right_and_freezes_at_dismiss() {
        let (w, h) = (100, 30);
        let wu = w as f64 * 0.5;
        // Glide clock: 0 before opening, 1 after the glide, easing between.
        assert_eq!(shift_at(0, None), 0.0);
        assert_eq!(shift_at(9999, None), 0.0);
        let enter = 2000u64;
        assert_eq!(shift_at(enter - 1, Some(enter)), 0.0);
        assert_eq!(shift_at(enter, Some(enter)), 0.0);
        assert_eq!(shift_at(enter + SELECT_SHIFT_MS, Some(enter)), 1.0);
        assert_eq!(shift_at(enter + 9999, Some(enter)), 1.0);
        let mid = shift_at(enter + SELECT_SHIFT_MS / 2, Some(enter));
        assert!((0.0..1.0).contains(&mid), " gliding: {mid}");
        // At the gather moment positions equal targets: shift 1 keeps every
        // target in the right portion (left edge at half width).
        for s in blob_spots(w, h, BLOB_GATHER_MS, None, 1.0) {
            assert!(s.x >= wu * 0.5 - 0.01, "right portion: {s:?}");
        }
        // Shift 0 keeps the classic full-width thirds (min target left of center).
        let min0 = blob_spots(w, h, BLOB_GATHER_MS, None, 0.0)
            .iter()
            .map(|s| s.x)
            .fold(f64::INFINITY, f64::min);
        let min1 = blob_spots(w, h, BLOB_GATHER_MS, None, 1.0)
            .iter()
            .map(|s| s.x)
            .fold(f64::INFINITY, f64::min);
        assert!(min0 < wu * 0.5 && min1 >= wu * 0.5 - 0.01, "{min0} vs {min1}");
    }

    #[test]
    fn selector_panel_lists_options_and_hides_prompt() {
        let (w, h) = (100, 30);
        let view = WelcomeView::select(1, 2000, false, false);
        let rows = render_welcome_rows_view(w, h, 2500, None, true, view);
        assert_eq!(rows.len(), h, "rows fill the viewport");
        let plain: String = rows
            .iter()
            .map(|r| crate::ansi::plain_text(r))
            .collect::<Vec<_>>()
            .join("\n");
        // Both options self-label (no translation step); the marker sits on #1.
        assert!(plain.contains("English (US)"), "option 0");
        assert!(plain.contains("中文"), "option 1");
        assert!(plain.contains("❯"), "selection marker");
        assert!(plain.contains("请选择语言"), "bilingual title");
        assert!(!plain.contains("PRESS ANY KEY"), "no prompt in selector");
        // Logo still shows above the panel.
        assert!(plain.contains('█'), "logo stays");
    }

    #[test]
    fn quote_box_shows_only_for_chinese_and_docks_right() {
        let q = quote_panel(100, 30);
        assert_eq!(q.x0 + q.width, 100 - 4, "right docked with margin");
        assert!(q.y0 + q.height <= 30);
        assert!(q.title_y < q.box_y, "title above checkbox");
        // Narrow window: geometry never escapes.
        let narrow = quote_panel(24, 10);
        assert!(narrow.x0 + narrow.width <= 24);

        let plain_at = |selected: usize, focus: bool, on: bool| {
            render_welcome_rows_view(
                100,
                30,
                2500,
                None,
                true,
                WelcomeView::select(selected, 2000, focus, on),
            )
            .iter()
            .map(|r| crate::ansi::plain_text(r))
            .collect::<Vec<_>>()
            .join("\n")
        };
        // English highlighted: no quote box at all (the hint line still
        // mentions it, so assert on the box title, not the bare words).
        let en = plain_at(0, false, false);
        assert!(!en.contains("一言 / Daily quote"), "no quote box for English");
        assert!(!en.contains("[ ]"), "no checkbox for English");
        // Chinese highlighted: box with title, off by default.
        let zh = plain_at(1, false, false);
        assert!(zh.contains("一言"), "quote title");
        assert!(zh.contains("[ ]"), "default off");
        assert!(!zh.contains("[✓]"), "not on");
        // Toggled on: checked glyph plus ON wording.
        let zh_on = plain_at(1, false, true);
        assert!(zh_on.contains("[✓]"), "checked");
        assert!(zh_on.contains("开启"), "on wording");
        // Focused: the row takes the selection marker.
        let zh_focus = plain_at(1, true, true);
        assert!(zh_focus.contains("❯ [✓]"), "focused marker");
        // Right side: the quote title sits in the right half (byte columns
        // preserve order; the CJK bytes in between only inflate the gap).
        let row = zh
            .lines()
            .find(|l| l.contains("一言 / Daily quote"))
            .expect("quote title");
        let quote_col = row.find("一言 / Daily quote").expect("title pos");
        assert!(quote_col > 40, "quote sits right");
        // Checkbox row likewise starts deep right (leading spaces are exact).
        let box_row = zh
            .lines()
            .find(|l| l.contains("[ ]"))
            .expect("checkbox row");
        let lead = box_row.chars().take_while(|c| *c == ' ').count();
        assert!(lead > 40, "checkbox sits right: {lead}");
    }

    #[test]
    fn selector_panel_geometry_is_left_docked_and_inside() {
        let options = select_options();
        assert_eq!(options.len(), 2);
        let p = select_panel(100, 30, &options);
        assert_eq!(p.x0, 4, "left docked");
        assert!(p.x0 + p.width <= 100);
        assert!(p.y0 + p.height <= 30);
        assert_eq!(p.opt_y.len(), 2);
        assert!(p.opt_y[0] < p.opt_y[1], "options stack downward");
        assert!(p.hint_y > p.opt_y[1], "hint below options");
        assert_eq!(p.hint_y2, p.hint_y + 1, "two hint rows (zh/en)");
        assert!(p.title_y < p.opt_y[0], "title above options");
        // Narrow window: geometry never escapes, rendering never panics.
        let narrow = select_panel(20, 10, &options);
        assert!(narrow.x0 + narrow.width <= 20);
        let rows = render_welcome_rows_view(20, 10, 2500, None, true, WelcomeView::select(0, 2000, false, false));
        assert_eq!(rows.len(), 10);
    }

    #[test]
    fn auto_stage_shows_animation_without_prompt() {
        let rows = render_welcome_rows_view(100, 30, 2500, None, true, WelcomeView::auto());
        let plain: String = rows
            .iter()
            .map(|r| crate::ansi::plain_text(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!plain.contains("PRESS ANY KEY"), "no prompt on returning runs");
        assert!(!plain.contains("请选择语言"), "no selector on returning runs");
        assert!(plain.contains('█'), "logo animates");
        // Prompt stage (the old default rows) still shows the prompt.
        let prompt_plain: String = render_welcome_rows_view(100, 30, 2500, None, true, WelcomeView::prompt())
            .iter()
            .map(|r| crate::ansi::plain_text(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(prompt_plain.contains("PRESS ANY KEY TO START"));
    }

    #[test]
    fn exit_freezes_then_flies_out_and_fades_to_black() {
        let (w, h) = (100, 30);
        let d = 2000u64;
        // Exit positions are continuous (no jump): dismissed=D at age=D equals the same frame without dismissal.
        assert_eq!(blob_spots(w, h, d, Some(d), 0.0), blob_spots(w, h, d, None, 0.0));
        // Exit progress: 0 before arrival, clamped to 1 when done.
        assert_eq!(exit_progress(d - 1, Some(d)), 0.0);
        assert_eq!(exit_progress(d, Some(d)), 0.0);
        assert_eq!(exit_progress(d + WELCOME_EXIT_MS, Some(d)), 1.0);
        assert_eq!(exit_progress(d + 9999, Some(d)), 1.0);
        assert_eq!(exit_progress(d, None), 0.0);
        // When done, all three centers sit outside the viewport.
        let wu = w as f64 * 0.5;
        for s in blob_spots(w, h, d + WELCOME_EXIT_MS, Some(d), 0.0) {
            let outside = s.x < 0.0 || s.x > wu || s.y < 0.0 || s.y > h as f64;
            assert!(outside, "飞出视口：{s:?}");
        }
        // The finished frame renders all black with no text.
        let rows = render_welcome_rows_view(w, h, d + WELCOME_EXIT_MS, Some(d), true, WelcomeView::prompt());
        for row in &rows {
            assert!(row.contains("\x1b[48;2;0;0;0m"), "背景黑");
        }
        let plain: String = rows
            .iter()
            .map(|r| crate::ansi::plain_text(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!plain.contains("PRESS"), "文字熄灭");
        assert!(!plain.contains('█'), "logo 熄灭");
    }

    #[test]
    fn welcome_fades_in_only_after_gather() {
        assert_eq!(welcome_fade_op(0), 0.0);
        assert_eq!(welcome_fade_op(BLOB_GATHER_MS - 1), 0.0, "聚集时不可见");
        assert_eq!(welcome_fade_op(BLOB_GATHER_MS + WELCOME_FADE_MS), 1.0);
        let mid = welcome_fade_op(BLOB_GATHER_MS + WELCOME_FADE_MS / 2);
        assert!((0.0..1.0).contains(&mid), "渐显中：{mid}");
        // While gathering, logo/prompt render nothing (pure backdrop spaces).
        let rows = render_welcome_rows_view(100, 30, 100, None, true, WelcomeView::prompt());
        let plain = rows.join("\n");
        assert!(!plain.contains('█'), "聚集时无 logo");
        assert!(!plain.contains("PRESS ANY KEY"), "聚集时无提示");
        // They appear after fade-in (compare visible text: escapes split raw substrings).
        let rows = render_welcome_rows_view(100, 30, 2000, None, true, WelcomeView::prompt());
        let plain: Vec<String> = rows.iter().map(|r| crate::ansi::plain_text(r)).collect();
        assert!(plain.join("\n").contains("PRESS ANY KEY TO START"));
    }

    #[test]
    fn prompt_shine_sweeps_then_parks() {
        assert_eq!(prompt_shine_t(0), Some(0.0), "周期起点开扫");
        assert_eq!(prompt_shine_t(300), Some(0.5), "中途");
        assert_eq!(prompt_shine_t(1800), Some(0.0), "周期回绕");
        assert_eq!(prompt_shine_t(1000), None, "扫完离场等待");
        // Peaks highlight near-white, zero outside the wave (n=8, t=0.5 peaks at char 5).
        assert!(shine_glow(4, 8, Some(0.5)) > 0.5, "波峰亮");
        assert_eq!(shine_glow(0, 8, None), 0.0, "等待期无扫光");
        let tail = shine_glow(7, 8, Some(1.0));
        assert!(tail < 0.2, "离场后归零：{tail}");
    }

    #[test]
    fn tiny_glyph_widths_match_cfonts() {
        // Letters take 3 cols, `1` takes 2, `.`/space take 1 (measured cfonts tiny).
        assert_eq!(tiny_glyph('A').1, 3);
        assert_eq!(tiny_glyph('1').1, 2);
        assert_eq!(tiny_glyph('.').1, 1);
        assert_eq!(tiny_glyph(' ').1, 1);
        assert_eq!(tiny_glyph('1').0, ["▄█", " █"]);
        assert_eq!(tiny_glyph('.').0, [" ", "▄"]);
        assert_eq!(tiny_glyph('A').0, ["▄▀█", "█▀█"]);
        assert_eq!(tiny_glyph('P').0, ["█▀█", "█▀▀"]);
        assert_eq!(tiny_glyph('H').0, ["█ █", "█▀█"]);
        // "1.0.0" spans 15 cols (per-char widths plus separators; 0 reuses the O glyph).
        let grid = tiny_text_grid("1.0.0");
        assert_eq!(grid[0].len(), 15);
        let plain: String = grid[0].iter().map(|c| c.unwrap_or(' ')).collect();
        assert_eq!(plain, " ▄█   █▀█   █▀█");
    }

    #[test]
    fn suffix_and_version_place_with_own_colors() {
        // ALPHA sits right of the logo (gold) and the version below it (light blue); both appear after fade-in.
        let rows = render_welcome_rows_view(100, 30, 2000, None, true, WelcomeView::prompt());
        let plain: Vec<String> = rows.iter().map(|r| crate::ansi::plain_text(r)).collect();
        // ALPHA shares the two main-title rows (2-3), starting past the tiny leading space.
        let suffix_x = LOGO_X + LOGO_W + LOGO_SUFFIX_GAP + 1;
        for (dy, expected) in [(0, "▄▀█"), (1, "█▀█")] {
            let row: String = plain[LOGO_Y + dy].chars().skip(suffix_x).take(3).collect();
            assert_eq!(row, expected, "ALPHA 首字");
        }
        // The version sits one blank row below the logo, also past the leading space.
        let vy = LOGO_Y + LOGO_H + 1;
        let vrow: String = plain[vy].chars().skip(LOGO_X + 1).take(2).collect();
        assert_eq!(vrow, "▄█", "版本号首字 1");
        // Colors: gold suffix plus light-blue version.
        let raw = rows.join("\n");
        assert!(raw.contains("38;2;255;213;79"), "后缀金色");
        assert!(raw.contains("38;2;135;206;250"), "版本淡蓝");
        // While gathering, suffix/version stay invisible with the main title.
        let rows = render_welcome_rows_view(100, 30, 100, None, true, WelcomeView::prompt());
        let plain = rows.join("\n");
        assert!(!plain.contains("▄▀█"), "聚集时无后缀");
    }

    #[test]
    fn prompt_box_spans_full_width_behind_text() {
        // Gray frame spans full width and 3 rows around the prompt: inside cells blend backdrop with gray by opacity.
        let (w, h, age) = (100, 30, 2000u64);
        let rows = render_welcome_rows_view(w, h, age, None, true, WelcomeView::prompt());
        let py = (h * 4 / 5).min(h - 1);
        let spots = blob_spots(w, h, age, None, 0.0);
        let fade = welcome_fade_op(age);
        assert_eq!(fade, 1.0);
        for dy in [-1i32, 0, 1] {
            let y = (py as i32 + dy).clamp(0, h as i32 - 1) as usize;
            let raw = &rows[y];
            let bg_raw = blob_color(blob_field(&spots, 0, y));
            let boxed = mix_rgb(bg_raw, PROMPT_BOX_BLUE, PROMPT_BOX_ALPHA * fade);
            let code = format!("48;2;{};{};{}", boxed.0 & !3, boxed.1 & !3, boxed.2 & !3);
            assert!(raw.contains(&code), "第 {y} 行框底：{raw:?}");
        }
        assert_eq!(crate::ansi::display_width(&rows[py]), 100);
    }

    #[test]
    fn rendered_logo_keeps_glyph_shapes() {
        // Regression: rendering must use the original glyphs (upper/lower half blocks), never all full blocks (which smear into bars).
        // (Capture after fade-in; the logo is invisible while gathering. Split per char.)
        let rows = render_welcome_rows_view(100, 30, 2000, None, false, WelcomeView::prompt());
        let plain: Vec<Vec<char>> = rows
            .iter()
            .map(|r| crate::ansi::plain_text(r).chars().collect())
            .collect();
        let row0: String = plain[LOGO_Y][LOGO_X..LOGO_X + LOGO_W].iter().collect();
        let row1: String = plain[LOGO_Y + 1][LOGO_X..LOGO_X + LOGO_W].iter().collect();
        assert_eq!(row0, CFONTS_ROW0);
        assert_eq!(row1, CFONTS_ROW1);
        assert!(row0.contains('▀'), "保留上半块");
        assert!(row1.contains('▄'), "保留下半块");
    }

    #[test]
    fn rows_always_fill_screen() {
        // Normal and tiny sizes: row count is always h, each row exactly w display cols.
        for (w, h) in [(100, 30), (80, 24), (45, 12), (5, 3), (1, 1)] {
            let rows = render_welcome_rows_view(w, h, 1234, None, true, WelcomeView::prompt());
            assert_eq!(rows.len(), h, "{w}x{h}");
            for row in &rows {
                assert_eq!(crate::ansi::display_width(row), w, "{w}x{h} {row:?}");
            }
        }
        // Non-styled environments emit no escapes but keep the layout (capture after fade-in).
        let rows = render_welcome_rows_view(80, 24, 2000, None, false, WelcomeView::prompt());
        assert_eq!(rows.len(), 24);
        assert!(!rows.join("").contains('\x1b'), "NO_COLOR 无转义");
        assert!(crate::ansi::plain_text(&rows.join("\n")).contains("PRESS ANY KEY TO START"));
    }

    #[test]
    fn suffix_shine_loops_offset_from_prompt() {
        // Staggered against the prompt: the suffix waits while the prompt shines mid-cycle (t=0.5) and vice versa.
        assert_eq!(suffix_shine_t(0), None, "t=0 提示扫，后缀等");
        // age=1250: suffix phase (1250+900)%1800=350 yields 0.5; prompt phase 1250%1800=1250 waits.
        assert_eq!(suffix_shine_t(1250), Some(0.5));
        assert_eq!(prompt_shine_t(1250), None);
        // age=300: prompt 0.5, suffix (1200)%1800=1200 waits.
        assert_eq!(suffix_shine_t(300), None);
        assert_eq!(prompt_shine_t(300), Some(0.5));
    }

    #[test]
    fn suffix_row_shows_gold_sweep_peak() {
        // t=3050: fade done, suffix shine peaks at col 11 (front=0.5*26-3=10).
        // Peak mixes gold toward white at 0.9 = (255,251,237), same formula as the prompt peak.
        let rows = render_welcome_rows_view(100, 30, 3050, None, true, WelcomeView::prompt());
        let row = &rows[LOGO_Y];
        assert!(row.contains("38;2;255;251;237"), "后缀扫光峰值：{row:?}");
        // The prompt row is waiting then, with no peak.
        let py = (30 * 4 / 5).min(29);
        assert!(!rows[py].contains("38;2;255;251;237"));
    }

    #[test]
    fn prompt_centers_and_shines() {
        // Prompt sits centered-low (4/5 height); the peak color appears mid-shine (t=2100: fade done plus mid-shine).
        let rows = render_welcome_rows_view(100, 30, 2100, None, true, WelcomeView::prompt());
        let plain: Vec<String> = rows.iter().map(|r| crate::ansi::plain_text(r)).collect();
        let prow = &plain[30 * 4 / 5];
        assert!(prow.contains("PRESS ANY KEY TO START"), "{prow:?}");
        let leading = prow.chars().take_while(|c| *c == ' ').count();
        assert_eq!(leading, (100 - 22) / 2, "居中");
        let raw = &rows[30 * 4 / 5];
        // Shine peaks at char 5 (front=4.0): base mixed toward white at 0.9 = (246,248,251).
        assert!(raw.contains("38;2;246;248;251"), "扫光峰值：{raw:?}");
        // The wait period (t=2500: fade done plus shine gone) shows only the base color.
        let rows = render_welcome_rows_view(100, 30, 2500, None, true, WelcomeView::prompt());
        assert!(!rows[30 * 4 / 5].contains("38;2;246;248;251"));
        assert!(rows[30 * 4 / 5].contains("38;2;168;186;214"), "基色");
    }
}
