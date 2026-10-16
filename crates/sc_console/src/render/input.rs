//! TUI rendering: input section (split from `render.rs`; re-exported at `crate::render`).

use super::*;

/// Boot input box: an empty light bar while the bars stretch (in parallel with the title); once filled, the ripple
/// content fades in by `input_opacity` (never waits for the title). The cursor stays hidden; caller default is kept.
pub(super) fn render_boot_input(buf: &mut String, f: &Frame, boot: &BootFrame, width: usize, styled: bool) {
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
pub(super) const POPUP_SLIDE_COLS: usize = 8;
/// Birth span: row i emerges at `p = i/n*0.7` (every row is born before 0.7),
/// then slides plus fades into place within 0.3 units (same schedule as [`animated_popup_count`]).
pub(super) const POPUP_BIRTH_SPAN: f32 = 0.7;
/// Popup fade start (pure black): mid-animation foreground and selection backgrounds interpolate from black,
/// which reads as fade-in on dark terminals; on light terminals the first frame is readable black text.
pub(super) const POPUP_FADE_FROM: Rgb = Rgb(0, 0, 0);
/// Popup text steady color approximations (bright cyan/yellow): animation interpolation only,
/// steady still uses theme colors (switches back with no jump); description gray reuses the exact 245 gray.
pub(super) const POPUP_DISPLAY_RGB: Rgb = Rgb(95, 255, 255);
pub(super) const POPUP_ALIAS_RGB: Rgb = Rgb(255, 255, 135);
/// Popup selected-row background (steady gray): fades in from black the same way, with matching endpoints.
pub(super) const POPUP_SELECTED_BG: Rgb = Rgb(58, 58, 58);

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
pub(super) fn render_input_box(
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
