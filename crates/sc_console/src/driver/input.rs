//! 控制台驱动：按键/输入处理、弹层、选择与提交（由 `driver.rs` 拆出；在 `crate::driver` 重新导出）。
//!
//! Console driver: key/input handling, popups, selection and submit
//! (split out of `driver.rs`; re-exported from `crate::driver`).

use super::*;

pub(super) fn push_welcome(state: &mut GuiState, config: &ConsoleConfig) {
    let ready = rust_i18n::t!("tui.console_ready", locale = state.lang.as_str()).to_string();
    state.logs.push_back(LogLine::new(LogLevel::Info, "sc_console", &ready));
    let _ = config;
}

pub(super) fn refresh_completion(handle: &ConsoleHandle, editor: &mut EditorState) {
    let text = editor.text();
    let cursor = editor.cursor();
    // Without a trigger (no `/` or `:` typed) just close the popup; leave the provider alone.
    if crate::completion::trigger_at(&text, cursor).is_none() {
        editor.clear_completion();
        return;
    }
    let items = handle.complete(&text, cursor);
    if items.is_empty() {
        editor.clear_completion();
    } else {
        editor.completion = Some(CompletionState { items, selected: 0 });
    }
}

/// Empty-line Ctrl+C: raises SIGINT manually (raw mode disables the terminal's
/// ISIG handling). The handler only does an atomic store; the main loop then
/// runs the graceful-exit flow. A `stop` command line is also kept as an echo
/// plus command-channel fallback.
/// Only raises when the GUI actually owns the terminal (`gui_active`); tests and Plain mode are unaffected.
pub(super) fn signal_host_shutdown(handle: &ConsoleHandle) {
    if handle.inner.gui_active.load(Ordering::Relaxed) {
        // SAFETY: raise itself is async-signal-safe; the host handler only does an atomic store.
        unsafe {
            libc::raise(libc::SIGINT);
        }
    }
}

/// Search-mode keys (also available while locked; read-only, never touches the game).
///
/// - `Esc`: exit and restore the entry view;
/// - `Enter` / Down / `Ctrl+F`: next; `Shift+Enter` / `BackTab` / Up: previous;
/// - Printable chars / `Backspace` / `Delete`: edit the query (recompute and jump to the first match);
/// - Left/Right/Home/End / `Ctrl+A/E/U`: move the query cursor;
/// - `Ctrl+C`: clears a non-empty query, otherwise exits (never triggers the shutdown confirm);
/// - `PgUp/PgDn`: scroll (keeps the search).
pub(super) fn handle_search_key(state: &mut GuiState, key: KeyEvent) {
    debug_assert!(state.search.is_some());
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => exit_search(state),
        (KeyCode::Enter, m) if m.contains(KeyModifiers::SHIFT) => search_step(state, -1),
        (KeyCode::Enter, _) => search_step(state, 1),
        (KeyCode::BackTab, _) => search_step(state, -1),
        (KeyCode::Up, _) => search_step(state, -1),
        (KeyCode::Down, _) => search_step(state, 1),
        (KeyCode::Char('f'), m) if m.contains(KeyModifiers::CONTROL) => search_step(state, 1),
        (KeyCode::PageUp, _) => {
            state.view = scroll_log(state, 10);
            state.dirty = true;
        }
        (KeyCode::PageDown, _) => {
            state.view = scroll_log(state, -10);
            state.dirty = true;
        }
        (KeyCode::Left, _) => {
            if let Some(search) = state.search.as_mut() {
                search.cursor = search.cursor.saturating_sub(1);
                state.dirty = true;
            }
        }
        (KeyCode::Right, _) => {
            if let Some(search) = state.search.as_mut() {
                search.cursor = search.cursor.saturating_add(1).min(search.query.len());
                state.dirty = true;
            }
        }
        (KeyCode::Home, _) => {
            if let Some(search) = state.search.as_mut() {
                search.cursor = 0;
                state.dirty = true;
            }
        }
        (KeyCode::End, _) => {
            if let Some(search) = state.search.as_mut() {
                search.cursor = search.query.len();
                state.dirty = true;
            }
        }
        (KeyCode::Char('a'), m) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(search) = state.search.as_mut() {
                search.cursor = 0;
                state.dirty = true;
            }
        }
        (KeyCode::Char('e'), m) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(search) = state.search.as_mut() {
                search.cursor = search.query.len();
                state.dirty = true;
            }
        }
        (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(search) = state.search.as_mut() {
                search.clear();
            }
            refresh_search(state, true);
            state.dirty = true;
        }
        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
            let empty = state.search.as_ref().is_some_and(|s| s.query.is_empty());
            if empty {
                exit_search(state);
            } else {
                if let Some(search) = state.search.as_mut() {
                    search.clear();
                }
                refresh_search(state, true);
                state.dirty = true;
            }
        }
        (KeyCode::Backspace, _) => {
            if let Some(search) = state.search.as_mut() {
                search.backspace();
            }
            refresh_search(state, true);
            jump_to_current(state);
            state.dirty = true;
        }
        (KeyCode::Delete, _) => {
            if let Some(search) = state.search.as_mut() {
                search.delete();
            }
            refresh_search(state, true);
            jump_to_current(state);
            state.dirty = true;
        }
        (KeyCode::Char(c), m)
            if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
        {
            if let Some(search) = state.search.as_mut() {
                search.insert_char(c);
            }
            refresh_search(state, true);
            jump_to_current(state);
            state.dirty = true;
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_key(
    handle: &ConsoleHandle,
    state: &mut GuiState,
    input_tx: &SyncSender<String>,
    key: KeyEvent,
    config: &ConsoleConfig,
    locked: bool,
    need_complete_refresh: &mut bool,
) {
    // Search state has its own keys (also available while locked; read-only).
    if state.search.is_some() {
        handle_search_key(state, key);
        return;
    }
    // Input stays locked until boot finishes: only stop (empty-line Ctrl+C/D), scroll, and clear-screen
    // are allowed; other edits, submits, and completions are ignored (the renderer shows a locked placeholder).
    if locked {
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), m)
                if m.contains(KeyModifiers::CONTROL) && state.editor.is_empty() =>
            {
                request_shutdown(handle, state, input_tx, config);
            }
            (KeyCode::Char('d'), m)
                if m.contains(KeyModifiers::CONTROL) && state.editor.is_empty() =>
            {
                request_shutdown(handle, state, input_tx, config);
            }
            (KeyCode::PageUp, _) => {
                state.view = scroll_log(state, 10);
            }
            (KeyCode::PageDown, _) => {
                state.view = scroll_log(state, -10);
            }
            (KeyCode::Char('f'), m) if m.contains(KeyModifiers::CONTROL) => {
                // Searching is also allowed while locked (read-only highlight plus jump; input row untouched).
                enter_search(state);
            }
            (KeyCode::Esc, _) => {
                // Dismiss the exit confirm first when present; otherwise follow the "press Esc for bottom" hint.
                if !cancel_shutdown_confirm(state) {
                    state.view = crate::render::ScrollView::FollowBottom;
                }
            }
            (KeyCode::Char('l'), m) if m.contains(KeyModifiers::CONTROL) => {
                state.logs.clear();
                state.view = crate::render::ScrollView::FollowBottom;
            }
            _ => {}
        }
        return;
    }
    // Esc priority: close popup, then back to bottom (keeping the input line), then clear the line.
    // The first Esc while reviewing only returns to the bottom without swallowing input.
    if key.code == KeyCode::Esc {
        if cancel_shutdown_confirm(state) {
            // An Esc that just dismissed the exit confirm goes no further (avoids swallowing input).
        } else if state.editor.completion.is_some() {
            state.editor.clear_completion();
        } else if state.view.is_frozen() {
            state.view = crate::render::ScrollView::FollowBottom;
        } else {
            state.editor.clear_line();
        }
        return;
    }
    // Search entry (Ctrl+F: the status line becomes a read-only search box; the input line is unaffected).
    if key.code == KeyCode::Char('f') && key.modifiers.contains(KeyModifiers::CONTROL) {
        enter_search(state);
        return;
    }
    let editor = &mut state.editor;
    match (key.code, key.modifiers) {
        // Submit (Enter always submits; Tab applies completions).
        (KeyCode::Enter, _) => {
            if let Some(line) = editor.submit(config.max_history) {
                submit_line(handle, input_tx, line, config, &mut state.logs);
            }
            state.view = crate::render::ScrollView::FollowBottom;
        }
        // Completions (Tab: a lone candidate applies directly; several first extend the common prefix).
        // Trigger chars (`/` and `:`) are kept: only the query after them is replaced.
        (KeyCode::Tab, _) => {
            let text = editor.text();
            let cursor = editor.cursor();
            let Some((anchor, query)) = crate::completion::trigger_at(&text, cursor) else {
                editor.clear_completion();
                return;
            };
            if editor.completion.is_none() {
                refresh_completion(handle, editor);
            }
            if let Some(comp) = editor.completion.clone() {
                let typed_len = query.chars().count();
                if comp.items.len() == 1 {
                    let replace = comp.items[0].replace.clone();
                    editor.apply_completion(anchor, &replace, true);
                } else {
                    let mut prefix = comp.items[0].replace.clone();
                    for item in comp.items.iter().skip(1) {
                        prefix = crate::common_prefix(&prefix, &item.replace);
                        if prefix.chars().count() <= typed_len {
                            break;
                        }
                    }
                    if prefix.chars().count() > typed_len {
                        editor.apply_completion(anchor, &prefix, false);
                        refresh_completion(handle, editor);
                    } else if let Some(item) = comp.selected() {
                        let replace = item.replace.clone();
                        editor.apply_completion(anchor, &replace, true);
                    }
                }
            }
        }
        (KeyCode::BackTab, _) => {
            if let Some(comp) = editor.completion.as_mut() {
                comp.select_prev();
            }
        }
        // History and popup navigation.
        (KeyCode::Up, _) => {
            if let Some(comp) = editor.completion.as_mut() {
                comp.select_prev();
            } else {
                editor.history_prev();
            }
        }
        (KeyCode::Down, _) => {
            if let Some(comp) = editor.completion.as_mut() {
                comp.select_next();
            } else {
                editor.history_next();
            }
        }
        // Cursor moves.
        (KeyCode::Left, m) if m.contains(KeyModifiers::CONTROL) => editor.move_home(),
        (KeyCode::Left, _) => editor.move_left(),
        (KeyCode::Right, m) if m.contains(KeyModifiers::CONTROL) => editor.move_end(),
        (KeyCode::Right, _) => editor.move_right(),
        (KeyCode::Home, _) => editor.move_home(),
        (KeyCode::End, _) => editor.move_end(),
        // Deletions.
        (KeyCode::Backspace, _) => {
            editor.backspace();
            *need_complete_refresh = true;
        }
        (KeyCode::Delete, _) => {
            editor.delete();
            *need_complete_refresh = true;
        }
        // Control combos.
        (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => {
            editor.kill_to_start();
            *need_complete_refresh = true;
        }
        (KeyCode::Char('k'), m) if m.contains(KeyModifiers::CONTROL) => {
            editor.kill_to_end();
            *need_complete_refresh = true;
        }
        (KeyCode::Char('w'), m) if m.contains(KeyModifiers::CONTROL) => {
            editor.delete_word_before();
            *need_complete_refresh = true;
        }
        (KeyCode::Char('a'), m) if m.contains(KeyModifiers::CONTROL) => editor.move_home(),
        (KeyCode::Char('e'), m) if m.contains(KeyModifiers::CONTROL) => editor.move_end(),
        (KeyCode::Char('l'), m) if m.contains(KeyModifiers::CONTROL) => {
            state.logs.clear();
            // Clearing logs invalidates anchors (total=0), so fall back to following the bottom.
            state.view = crate::render::ScrollView::FollowBottom;
        }
        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
            if editor.is_empty() {
                // Empty-line Ctrl+C needs a second confirm; on confirm reuse the original SIGINT path
                // plus a `stop` line fallback.
                // (matches the pre-TUI shutdown flow with saving).
                request_shutdown(handle, state, input_tx, config);
            } else {
                editor.clear_line();
            }
        }
        (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) => {
            if editor.is_empty() {
                request_shutdown(handle, state, input_tx, config);
            }
        }
        // Scrolling.
        (KeyCode::PageUp, _) => {
            state.view = scroll_log(state, 10);
        }
        (KeyCode::PageDown, _) => {
            state.view = scroll_log(state, -10);
        }
        // Plain characters.
        (KeyCode::Char(c), m)
            if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
        {
            if editor.len_chars() < config.max_input_chars {
                editor.insert_char(c);
                *need_complete_refresh = true;
            }
        }
        _ => {}
    }
}

/// Exit double-confirm: valid window after the first Ctrl+C (ms).
pub(super) const EXIT_ARM_MS: u64 = 3000;
/// Exit confirm prompt fade-in duration (ms).
pub(super) const EXIT_FADE_IN_MS: u64 = 150;
/// Exit confirm prompt fade-out duration (ms; after cancel or timeout).
pub(super) const EXIT_FADE_OUT_MS: u64 = 400;

/// Exit confirm prompt opacity: fades in to full while armed, fades to 0 after timeout/cancel.
/// 0 returns the line to the normal key hints.
pub(super) fn exit_prompt_opacity(
    armed: Option<std::time::Instant>,
    fade_from: Option<std::time::Instant>,
    now: std::time::Instant,
) -> f32 {
    if let Some(t0) = armed {
        let elapsed = now.saturating_duration_since(t0).as_millis() as u64;
        if elapsed < EXIT_ARM_MS {
            return (elapsed as f32 / EXIT_FADE_IN_MS.max(1) as f32).clamp(0.0, 1.0);
        }
    }
    if let Some(t0) = fade_from {
        let elapsed = now.saturating_duration_since(t0).as_millis() as u64;
        if elapsed < EXIT_FADE_OUT_MS {
            return 1.0 - elapsed as f32 / EXIT_FADE_OUT_MS as f32;
        }
    }
    0.0
}

/// 鼠标左键按下时的起点（决定拖拽/松开的语义）。
///
/// What a left mouse press started on (decides the drag/release meaning).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PressKind {
    /// Press started in the log area: drag extends the selection, release auto-copies.
    Select,
    /// Press started in the input box: click positions the cursor (dragging is meaningless).
    Input,
    /// Review hint row: click jumps back to the bottom.
    JumpToLatest,
    /// Anywhere else (title/status/popup): no action.
    Other,
}

use crate::render::Selection;

/// Completion popup appear age (ms; `None` means steady): replays only on a fresh (closed-to-open) popup;
/// query changes and selection moves never replay, and a closed popup is steady.
///
/// Called every frame by the caller with an explicit `now` clock; once the
/// animation ends the output equals the steady value.
pub(super) fn popup_age_for(
    last_key: &mut Option<Vec<String>>,
    t0: &mut Option<std::time::Instant>,
    completion: Option<&CompletionState>,
    now: std::time::Instant,
) -> Option<u64> {
    // The popup holds few items and the snapshot is tiny; two `None`s skip allocation directly.
    if completion.is_none() && last_key.is_none() {
        *t0 = None;
        return None;
    }
    let key = completion.map(|c| {
        c.items
            .iter()
            .map(|item| item.replace.clone())
            .collect::<Vec<_>>()
    });
    if key != *last_key {
        // Only closed-to-open restarts; open-to-open (query change) keeps the clock, never replays.
        let appearing = last_key.is_none() && key.is_some();
        *last_key = key;
        if appearing {
            *t0 = Some(now);
        } else if last_key.is_none() {
            // Open-to-closed: reset.
            *t0 = None;
        }
    }
    match *t0 {
        Some(start) => {
            let age = now.saturating_duration_since(start).as_millis() as u64;
            if age >= crate::render::POPUP_APPEAR_MS {
                *t0 = None;
                None
            } else {
                Some(age)
            }
        }
        None => None,
    }
}

/// Completion item count for the popup row budget (hit-testing and rendering share one layout).
pub(super) fn popup_items(editor: &EditorState) -> usize {
    editor
        .completion
        .as_ref()
        .map(|c| c.items.len())
        .unwrap_or(0)
}

/// Exit request (empty-line `Ctrl+C` / `Ctrl+D`): actually stops only after a second confirm.
///
/// The first press only arms it (status-line hint plus fade); a second press within `EXIT_ARM_MS` sends
/// a `stop` line plus SIGINT; timeout or `Esc` fades it out. Guards against accidental shutdown.
/// (especially while the boot command pipeline has not ticked yet).
pub(super) fn request_shutdown(
    handle: &ConsoleHandle,
    state: &mut GuiState,
    input_tx: &SyncSender<String>,
    config: &ConsoleConfig,
) {
    if state.exit_armed.is_some() {
        // Second press: actually stops. The hint fades out (leaves no stale block if signals lag).
        state.exit_armed = None;
        state.exit_fade_from = Some(std::time::Instant::now());
        submit_line(
            handle,
            input_tx,
            "stop".to_string(),
            config,
            &mut state.logs,
        );
        signal_host_shutdown(handle);
    } else {
        state.exit_armed = Some(std::time::Instant::now());
        state.exit_fade_from = None;
    }
    state.dirty = true;
}

/// Cancel the exit confirm (`Esc`): fades out immediately; returns whether one was armed.
pub(super) fn cancel_shutdown_confirm(state: &mut GuiState) -> bool {
    if state.exit_armed.take().is_some() {
        state.exit_fade_from = Some(std::time::Instant::now());
        state.dirty = true;
        true
    } else {
        false
    }
}

/// Freeze watermark: `Some` while reviewing (taken from `log_seq` on first entry); otherwise `None`.
pub(super) fn freeze_watermark(
    view: crate::render::ScrollView,
    current: Option<u64>,
    log_seq: u64,
) -> Option<u64> {
    if view.is_frozen() {
        Some(current.unwrap_or(log_seq))
    } else {
        None
    }
}

/// Scroll `delta` rows (positive scrolls up into history, negative toward the bottom; wrapped visual rows).
///
/// Uses the same layout as rendering for bounds: scrolling past the top stops accumulating,
/// scrolling to the bottom returns to follow-bottom. Tiny windows (invalid layout) keep the view.
/// Heights are evaluated in one pass (reused across batched events instead of re-parsing per row).
pub(super) fn scroll_log(state: &GuiState, delta: i32) -> crate::render::ScrollView {
    let (w, h) = size().unwrap_or((100, 30));
    let cw = crate::render::log_content_width(w as usize);
    let heights: Vec<usize> = state
        .logs
        .iter()
        .map(|line| crate::render::log_wrap_height(line, cw))
        .collect();
    let total = heights.len();
    crate::render::scroll_view_wrapped(
        h as usize,
        popup_items(&state.editor),
        total,
        &|idx| heights.get(idx).copied().unwrap_or(1),
        state.log_seq.saturating_sub(total as u64),
        state.view,
        delta,
    )
}

/// Extract plain text from a log selection (slice columns by display width, trim pad spaces, drop edge blanks).
pub(super) fn extract_selection(logs: &VecDeque<LogLine>, sel: &Selection, width: usize) -> String {
    // Column ranges come from `x_range_for` per row; only the row range is taken here.
    let (lo, _, hi, _) = sel.normalize();
    let mut lines = Vec::new();
    for idx in lo..=hi {
        let Some(line) = logs.get(idx) else {
            continue;
        };
        let plain = crate::render::log_row_plain(line);
        // A tail column of 0 (selected to the row start, excluding that row) yields no range; leave an empty row to drop later.
        let Some((x0, x1)) = sel.x_range_for(idx) else {
            lines.push(String::new());
            continue;
        };
        let seg = crate::ansi::slice_by_width(&plain, x0.min(width), x1.min(width));
        lines.push(seg.trim_end().to_string());
    }
    // Leading/trailing blank rows come from selecting pad/out-of-range areas; drop them directly.
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Finish one copy: OSC 52 writes straight to the terminal; external commands go to a helper thread.
pub(super) fn finish_copy(
    out: &mut std::io::Stdout,
    copy_tx: &std::sync::mpsc::Sender<crate::clipboard::CopyOutcome>,
    state: &mut GuiState,
    text: String,
) {
    use std::io::Write as _;
    let chars = text.chars().count();
    let lang = state.lang.as_str();
    if crate::clipboard::backend() == crate::clipboard::Backend::Osc52 {
        let esc = crate::clipboard::osc52_escape(&text);
        if out.write_all(esc.as_bytes()).is_ok() && out.flush().is_ok() {
            state.copy_flash = Some((
                rust_i18n::t!("tui.copy_ok", locale = lang, n = chars).to_string(),
                std::time::Instant::now(),
            ));
        } else {
            state.copy_flash = Some((
                rust_i18n::t!("tui.copy_fail_write", locale = lang).to_string(),
                std::time::Instant::now(),
            ));
        }
        state.dirty = true;
        return;
    }
    // External commands may block (Wayland needs a resident clipboard service): run them on a helper thread.
    let tx = copy_tx.clone();
    let backend = crate::clipboard::backend();
    if std::thread::Builder::new()
        .name("sc-clipboard".into())
        .spawn(move || {
            let ok = crate::clipboard::copy_via_command(backend, &text);
            let _ = tx.send(crate::clipboard::CopyOutcome { chars, ok });
        })
        .is_err()
    {
        let lang = state.lang.as_str();
        state.copy_flash = Some((
            rust_i18n::t!("tui.copy_fail_spawn", locale = lang).to_string(),
            std::time::Instant::now(),
        ));
        state.dirty = true;
    }
}

pub(super) fn submit_line(
    handle: &ConsoleHandle,
    input_tx: &SyncSender<String>,
    line: String,
    config: &ConsoleConfig,
    logs: &mut VecDeque<LogLine>,
) {
    let mut line = line;
    // Session language from config (submit path has no GuiState; resolves the same way).
    let lang = crate::config::resolve_lang(config.lang.clone(), config.initial_lang.clone());
    if line.chars().count() > config.max_input_chars {
        line = line.chars().take(config.max_input_chars).collect();
        logs.push_back(LogLine::new(
            LogLevel::Warn,
            "sc_console",
            rust_i18n::t!("tui.input_truncated", locale = lang.as_str(), n = config.max_input_chars)
                .as_ref(),
        ));
    }
    // Echo the user command (a `>` prefix; dim styling comes from the render layer).
    logs.push_back(LogLine::new(
        LogLevel::Debug,
        "console",
        &format!("> {line}"),
    ));
    if input_tx.try_send(line).is_err() {
        handle.inner.dropped_inputs.fetch_add(1, Ordering::Relaxed);
        logs.push_back(LogLine::new(
            LogLevel::Warn,
            "sc_console",
            &rust_i18n::t!("tui.queue_full", locale = lang.as_str()),
        ));
    }
}
