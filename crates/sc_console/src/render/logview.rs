//! TUI rendering: logview section (split from `render.rs`; re-exported at `crate::render`).

use super::*;

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
pub(super) fn locate_cursor(rows: &[Vec<char>], cursor: usize) -> (usize, usize) {
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
pub(super) fn row_prefix_width(row: &[char], n: usize) -> usize {
    row.iter()
        .take(n)
        .map(|c| UnicodeWidthChar::width(*c).unwrap_or(0))
        .sum()
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
pub(super) fn visual_prefix(total: usize, height_of: &dyn Fn(usize) -> usize, idx: usize) -> usize {
    (0..idx.min(total)).map(height_of).sum()
}

/// Logical log index owning visual row `v` (`v` past the end clamps to the last entry).
pub(super) fn log_at_visual(total: usize, height_of: &dyn Fn(usize) -> usize, v: usize) -> usize {
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
pub(super) fn full_page_start(
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
pub(super) fn end_from(
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
pub(super) fn build_log_styled(line: &LogLine, styled: bool) -> String {
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

// ================= Right online-player panel =================

/// Right panel cell (`visible_cols` columns, steady `SIDE_PANEL_WIDTH`):
/// `zone_row` is the in-panel row number (from 0, covering log rows, fills, hints, and popup rows).
///
/// Background uses one shade darker than the title/input bars (`SIDE_PANEL_BG`), standing apart;
/// row 0 is the title (online count), then one player name per row (overlong names truncate),
/// an empty list shows a low-key placeholder below the first row. A dim gray separator blends the edge.
/// While sliding in, only the left `visible_cols` columns show (the panel grows leftward from the right edge).
pub(super) fn side_panel_cell(
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
pub(super) fn emit_log_zone_row(
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
