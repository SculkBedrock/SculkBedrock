//! TUI rendering: frame section (split from `render.rs`; re-exported at `crate::render`).

use super::*;
use super::boot::render_boot_header;
use super::logview::{build_log_styled, emit_log_zone_row};

pub(super) fn build_frame(f: &Frame) -> (Vec<String>, (u16, u16)) {
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
    } // end of boot branch (both boot rows already drawn by render_boot_header)
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
        }, // `None` = no boot: regular footnote / celebration
    } // end of boot footnote branch
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
pub(super) fn render<W: Write>(w: &mut W, f: &Frame) -> std::io::Result<(u16, u16)> {
    let (rows, cursor) = build_frame(f);
    w.write_all(rows.join("\r\n").as_bytes())?;
    Ok(cursor)
}

/// Title row: left `mark + text`, right `label + value` (right aligned), full-width bar background.
///
/// Right values stay light gray (250), never follow the terminal default foreground.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_title_row(
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
pub(super) fn split_pinned_closer(full: &str) -> (&str, &str) {
    if let Some(cut) = full.strip_suffix('」') {
        (cut, "」")
    } else {
        (full, "")
    }
}

/// Brightness of the gaussian light front at char i (0..1).
///
/// `front` is the wave center char index; larger `spread` means a wider light band.
pub(super) fn wave_glow(i: usize, front: f32, spread: f32) -> f32 {
    let d = i as f32 - front;
    (-(d * d) / (2.0 * spread * spread)).exp()
}

/// Title row shine variant: brightens the body per char along the gaussian wave; labels and values stay normal.
///
/// `t` in [0,1]: the wave sweeps left to right across the body and leaves; both ends equal the base color (no jump).
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_shining_title_row(
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
/// Used for the quote, where the typewriter pins the bracket at the row end.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_shining_title_row_pinned(
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
/// `text_opacity`: boot text opacity; 1.0 is the steady fixed color 253.
pub(super) fn render_starting_box(
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
