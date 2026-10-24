//! 控制台驱动：日志搜索状态与导航（由 `driver.rs` 拆出；在 `crate::driver` 重新导出）。
//!
//! Console driver: log search state and navigation (split from `driver.rs`; re-exported at `crate::driver`).

use super::*;

/// Global match cap (bounded; keeps the oldest matches).
pub(super) const MAX_SEARCH_MATCHES: usize = 2000;
/// Query string char cap (longer input is ignored).
pub(super) const MAX_QUERY_CHARS: usize = 64;

/// 日志搜索状态（`Ctrl+F` 进入；只读，不触碰游戏状态）。
///
/// Log search state (entered with `Ctrl+F`; read-only, never touches game state).
pub(super) struct SearchState {
    /// Query string (chars; cursor is a char index, wide-char safe).
    pub(super) query: Vec<char>,
    /// Query cursor (char index).
    pub(super) cursor: usize,
    /// All matches (ascending by global log sequence; bounded by `MAX_SEARCH_MATCHES`).
    pub(super) matches: Vec<SearchMatch>,
    /// Current match index (0 with no highlight when empty).
    pub(super) current: usize,
    /// Selection time of the current match (clocks the background fade plus text shine; replays on switch).
    pub(super) current_t0: std::time::Instant,
    /// View at search entry (restored on `Esc` so position survives searching).
    pub(super) entry_view: crate::render::ScrollView,
}

impl SearchState {
    pub(super) fn query_text(&self) -> String {
        self.query.iter().collect()
    }

    pub(super) fn insert_char(&mut self, c: char) {
        if self.query.len() >= MAX_QUERY_CHARS {
            return;
        }
        let at = self.cursor.min(self.query.len());
        self.query.insert(at, c);
        self.cursor = at + 1;
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor == 0 || self.query.is_empty() {
            return;
        }
        let at = self.cursor.min(self.query.len());
        self.query.remove(at - 1);
        self.cursor = at - 1;
    }

    pub(super) fn delete(&mut self) {
        let at = self.cursor.min(self.query.len());
        if at < self.query.len() {
            self.query.remove(at);
        }
    }

    pub(super) fn clear(&mut self) {
        self.query.clear();
        self.cursor = 0;
    }
}

/// Build the search render context (with the current selection animation age; `None` once steady).
/// The selection animation lasts 500ms.
pub(super) fn search_ctx_for<'a>(
    search: &'a SearchState,
    query: &'a str,
    now: std::time::Instant,
) -> crate::render::SearchCtx<'a> {
    let age = now.saturating_duration_since(search.current_t0).as_millis() as u64;
    crate::render::SearchCtx {
        matches: &search.matches,
        current: search.current,
        query,
        cursor: search.cursor,
        current_age_ms: if search.matches.is_empty() {
            None
        } else {
            (age < crate::render::SEARCH_SHINE_MS).then_some(age)
        },
    }
}

/// Enter search (`Ctrl+F`; records the current view so `Esc` can restore it).
pub(super) fn enter_search(state: &mut GuiState) {
    if state.search.is_none() {
        state.search = Some(SearchState {
            query: Vec::new(),
            cursor: 0,
            matches: Vec::new(),
            current: 0,
            current_t0: std::time::Instant::now(),
            entry_view: state.view,
        });
        state.dirty = true;
    }
}

/// Exit search (`Esc`, or `Ctrl+C` on an empty query; restores the entry view).
pub(super) fn exit_search(state: &mut GuiState) {
    if let Some(search) = state.search.take() {
        state.view = search.entry_view;
        state.dirty = true;
    }
}

/// Recompute matches (called on query change, new log arrival, or after clearing logs).
///
/// - `reset_current`: true when the query changed (jump back to the first match via the caller);
///   false for new arrivals (keeps the current `(seq, x0)` match identity, clamps to the end when evicted).
pub(super) fn refresh_search(state: &mut GuiState, reset_current: bool) {
    let Some(search) = state.search.as_mut() else {
        return;
    };
    let keep = (!reset_current)
        .then(|| search.matches.get(search.current).map(|m| (m.seq, m.x0)))
        .flatten();
    search.matches.clear();
    let query = search.query_text();
    if !query.is_empty() {
        let evicted = state.log_seq.saturating_sub(state.logs.len() as u64);
        'outer: for (i, line) in state.logs.iter().enumerate() {
            let seq = evicted.saturating_add(i as u64);
            let plain = crate::render::log_row_plain(line);
            for (x0, x1) in crate::render::find_matches(&plain, &query) {
                if search.matches.len() >= MAX_SEARCH_MATCHES {
                    break 'outer;
                }
                search.matches.push(SearchMatch { seq, x0, x1 });
            }
        }
    }
    search.current = if search.matches.is_empty() {
        0
    } else {
        match keep {
            Some((seq, x0)) => search
                .matches
                .iter()
                .position(|m| (m.seq, m.x0) == (seq, x0))
                .or_else(|| {
                    search
                        .matches
                        .iter()
                        .position(|m| m.seq > seq || (m.seq == seq && m.x0 > x0))
                })
                .unwrap_or(search.matches.len() - 1),
            None => 0,
        }
    };
    // A changed query resets the current match and replays the selection animation.
    if reset_current && !search.matches.is_empty() {
        search.current_t0 = std::time::Instant::now();
    }
}

/// Next/previous match (`dir > 0` means next, otherwise previous; wraps around).
pub(super) fn search_step(state: &mut GuiState, dir: i32) {
    let len = state.search.as_ref().map(|s| s.matches.len()).unwrap_or(0);
    if len == 0 {
        return;
    }
    if let Some(search) = state.search.as_mut() {
        search.current = if dir > 0 {
            (search.current + 1) % len
        } else {
            (search.current + len - 1) % len
        };
        // Switching the selection replays the animation.
        search.current_t0 = std::time::Instant::now();
    }
    jump_to_current(state);
    state.dirty = true;
}

/// Jump the view to the current match (stays put when visible; otherwise centers; skips tiny terminals).
pub(super) fn jump_to_current(state: &mut GuiState) {
    let hit_seq = match state.search.as_ref() {
        Some(search) => match search.matches.get(search.current) {
            Some(hit) => hit.seq,
            None => return,
        },
        None => return,
    };
    let (w, height) = size().unwrap_or((100, 30));
    let cw = crate::render::log_content_width(w as usize);
    let heights: Vec<usize> = state
        .logs
        .iter()
        .map(|line| crate::render::log_wrap_height(line, cw))
        .collect();
    let total = heights.len();
    let evicted = state.log_seq.saturating_sub(total as u64);
    let new_view = crate::render::jump_target_view_wrapped(
        height as usize,
        popup_items(&state.editor),
        total,
        &|idx| heights.get(idx).copied().unwrap_or(1),
        evicted,
        state.view,
        hit_seq,
    );
    if new_view != state.view {
        state.view = new_view;
        state.dirty = true;
    }
}

/// Jump geometry (single-line form; used by tests only).
#[cfg(test)]
pub(super) fn jump_target_view(
    height: usize,
    popup: usize,
    total: usize,
    evicted: u64,
    view: crate::render::ScrollView,
    target_seq: u64,
) -> crate::render::ScrollView {
    crate::render::jump_target_view_wrapped(height, popup, total, &|_| 1, evicted, view, target_seq)
}
