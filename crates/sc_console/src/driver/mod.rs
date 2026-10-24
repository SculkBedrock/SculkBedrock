//! 控制台驱动：专用线程 + 有界信道。
//!
//! - GUI 模式：接管终端（备用屏 + raw 模式），以事件循环渲染；
//! - Plain 模式：阻塞式 stdin 行读取，不改动终端模式；
//! - Off 模式：不启动任何线程（handle 仍可用，输入始终为空）。
//!
//! 线程安全约定：
//!
//! - 控制台线程从不触碰游戏状态，只通过 `input_tx` 发送拥有所有权的 `String`；
//! - 日志/输入信道全部有界（见 [`ConsoleConfig`]），队列满时丢弃并计数；
//! - 终端恢复分三层：正常 `shutdown()` -> `Last` 阶段的关闭系统 ->
//!   `atexit` 兜底（`process::exit` / panic 路径）。
//!
//! Console driver: dedicated thread plus bounded channels.
//!
//! - GUI mode: takes over the terminal (alternate screen + raw mode) with event-loop rendering;
//! - Plain mode: blocking stdin line reads without touching terminal modes;
//! - Off mode: starts no thread (the handle stays usable and input stays empty).
//!
//! Thread-safety contract:
//!
//! - The console thread never touches game state; it only sends owned `String`s via `input_tx`;
//! - Log/input channels are all bounded (see [`ConsoleConfig`]); full channels drop and count;
//! - Terminal restore has three layers: normal `shutdown()` -> shutdown system on `Last` ->
//!   `atexit` fallback (`process::exit` / panic paths).

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;

use crate::completion::{CompletionProvider, NoopCompletionProvider};
use crate::config::{ConsoleConfig, ResolvedMode};
use crate::editor::{CompletionState, EditorState};
use crate::model::{ConsoleHeader, ConsoleStats, LogLevel, LogLine, ServerPhase};
use crate::render::{Frame, Renderer};
use crate::render::{SearchMatch, Selection};

pub mod events;
pub mod input;
pub mod sampler;
pub mod search;
pub mod welcome;
use self::events::*;
use self::input::*;
use self::sampler::*;
use self::search::*;
use self::welcome::*;
pub(crate) use self::events::MAIN_DRAIN_MAX;
pub use self::welcome::WelcomeChoice;
pub(crate) use self::events::drain_events;

/// 对外暴露的控制台句柄（`Clone + Send + Sync`，与日志层和宿主共享）。
///
/// Public console handle (`Clone + Send + Sync`, shared with the log layer and host).
#[derive(Clone)]
pub struct ConsoleHandle {
    inner: Arc<ConsoleShared>,
}

struct ConsoleShared {
    log_tx: SyncSender<LogLine>,
    provider: RwLock<Arc<dyn CompletionProvider>>,
    header: RwLock<ConsoleHeader>,
    /// Online player name snapshot (pushed by the host each tick; shown read-only in the side panel).
    players: RwLock<Vec<String>>,
    running: AtomicBool,
    gui_active: AtomicBool,
    /// Server state set by the host (set true once boot succeeds; one-way; drives the sweep effect and input unlock).
    server_running: AtomicBool,
    /// Whether the welcome screen has finished (set true on dismiss, one-way; the host waits on it before loading).
    /// True from construction when the welcome screen is disabled (non-GUI / plain / hosted / config off).
    welcome_dismissed: AtomicBool,
    /// Subtitle version (bumped when `set_subtitle` content changes; the driver restarts the animation on change).
    subtitle_ver: AtomicU64,
    /// Welcome-screen choice (set once by the console thread on selector
    /// confirm; taken once by the host to persist into `server.properties`).
    /// One-shot: `take` leaves `None` behind.
    welcome_choice: Mutex<Option<WelcomeChoice>>,
    dropped_logs: AtomicU64,
    dropped_inputs: AtomicU64,
    max_log_lines: usize,
    max_completions: usize,
}

impl ConsoleHandle {
    /// Push one log line (non-blocking; drops and counts when full, never blocks the caller).
    pub fn push_log(&self, line: LogLine) {
        match self.inner.log_tx.try_send(line) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.inner.dropped_logs.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn push_info(&self, target: &str, message: &str) {
        self.push_log(LogLine::new(LogLevel::Info, target, message));
    }

    /// Hot-swap the completion provider (called by the host after rebuilding from a registry snapshot).
    pub fn set_provider(&self, provider: Arc<dyn CompletionProvider>) {
        if let Ok(mut slot) = self.inner.provider.write() {
            *slot = provider;
        }
    }

    /// Set the top info bar (server version plus pack label; pushed by the host when ready).
    ///
    /// Only update the version fields, keeping the existing subtitle
    /// (maintained independently by [`ConsoleHandle::set_subtitle`]).
    pub fn set_header(&self, header: ConsoleHeader) {
        if let Ok(mut slot) = self.inner.header.write() {
            slot.core_version = header.core_version;
            slot.pack_label = header.pack_label;
        }
    }

    /// Set the title second-line quote (callable anytime, thread-safe).
    ///
    /// Does nothing when the content is unchanged (avoids retriggering the typewriter animation).
    pub fn set_subtitle(&self, subtitle: &str) {
        let mut changed = false;
        if let Ok(mut slot) = self.inner.header.write() {
            if slot.subtitle != subtitle {
                slot.subtitle = subtitle.to_string();
                changed = true;
            }
        }
        if changed {
            self.inner.subtitle_ver.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Full quote text plus version (the driver restarts the animation on change).
    pub fn subtitle(&self) -> (String, u64) {
        let ver = self.inner.subtitle_ver.load(Ordering::Relaxed);
        let text = self
            .inner
            .header
            .read()
            .map(|h| h.subtitle.clone())
            .unwrap_or_default();
        (text, ver)
    }

    pub fn header(&self) -> ConsoleHeader {
        self.inner
            .header
            .read()
            .map(|h| h.clone())
            .unwrap_or_default()
    }

    /// Set the online player name snapshot (pushed by the host each tick; replaced on change; thread-safe).
    pub fn set_online_players(&self, players: Vec<String>) {
        if let Ok(mut slot) = self.inner.players.write() {
            if *slot != players {
                *slot = players;
            }
        }
    }

    /// Online player name snapshot (polled by the driver each frame; shown read-only in the side panel).
    pub fn online_players(&self) -> Vec<String> {
        self.inner
            .players
            .read()
            .map(|p| p.clone())
            .unwrap_or_default()
    }

    pub fn provider_name(&self) -> String {
        self.inner
            .provider
            .read()
            .map(|p| p.name().to_string())
            .unwrap_or_default()
    }

    /// Query completions (shared by the UI thread and host tests; implementations must return fast per trait contract).
    pub fn complete(&self, line: &str, cursor: usize) -> Vec<crate::CompletionItem> {
        let max = self.inner.max_completions;
        let items = self
            .inner
            .provider
            .read()
            .map(|p| p.complete(line, cursor))
            .unwrap_or_default();
        if items.len() > max {
            items.into_iter().take(max).collect()
        } else {
            items
        }
    }

    pub fn is_running(&self) -> bool {
        self.inner.running.load(Ordering::Relaxed)
    }

    /// Mark the server as booted (one-way, idempotent; the driver plays the green sweep then unlocks input).
    pub fn set_server_running(&self) {
        self.inner.server_running.store(true, Ordering::Relaxed);
    }

    /// Whether the welcome screen has finished (always true when disabled).
    ///
    /// The host polls this flag after assembling the app and before the main loop: with GUI plus
    /// welcome enabled, loading starts only after a keypress.
    pub fn welcome_done(&self) -> bool {
        self.inner.welcome_dismissed.load(Ordering::Relaxed)
    }

    /// Record the welcome-screen choice (console thread, selector confirm only).
    ///
    /// One-shot handoff: the host takes it once to persist into
    /// `server_properties.toml` (see `take_welcome_choice`).
    pub fn set_welcome_choice(&self, choice: WelcomeChoice) {
        if let Ok(mut slot) = self.inner.welcome_choice.lock() {
            *slot = Some(choice);
        }
    }

    /// Take the welcome-screen choice, if any (host, after [`welcome_done`]).
    ///
    /// `None` means no selector confirm happened (returning run, env override,
    /// or Ctrl+C entry): nothing to persist.
    pub fn take_welcome_choice(&self) -> Option<WelcomeChoice> {
        self.inner.welcome_choice.lock().ok().and_then(|mut s| s.take())
    }

    pub fn server_running(&self) -> bool {
        self.inner.server_running.load(Ordering::Relaxed)
    }

    pub fn dropped_logs(&self) -> u64 {
        self.inner.dropped_logs.load(Ordering::Relaxed)
    }

    pub fn dropped_inputs(&self) -> u64 {
        self.inner.dropped_inputs.load(Ordering::Relaxed)
    }

    /// Request console thread exit (idempotent; the actual join happens in [`ConsoleDriver::shutdown`]).
    pub fn request_shutdown(&self) {
        self.inner.running.store(false, Ordering::Relaxed);
    }
}

/// 控制台驱动（由宿主持有；`shutdown` 恢复终端并等待线程退出）。
///
/// Console driver (held by the host; `shutdown` restores the terminal and joins the thread).
pub struct ConsoleDriver {
    handle: ConsoleHandle,
    input_rx: Option<Receiver<String>>,
    thread: Option<JoinHandle<()>>,
    mode: ResolvedMode,
}

impl ConsoleDriver {
    pub fn handle(&self) -> ConsoleHandle {
        self.handle.clone()
    }

    pub fn mode(&self) -> ResolvedMode {
        self.mode
    }

    /// Take one user input line without blocking (called by the host each tick, possibly repeatedly per budget).
    pub fn try_recv_line(&self) -> Option<String> {
        self.input_rx.as_ref().and_then(|rx| rx.try_recv().ok())
    }

    /// Restore the terminal and join the thread (idempotent; also runs on drop, but call explicitly for timing).
    pub fn shutdown(mut self) {
        self.shutdown_ref();
    }

    fn shutdown_ref(&mut self) {
        self.handle.request_shutdown();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        restore_terminal(&self.handle);
    }
}

impl Drop for ConsoleDriver {
    fn drop(&mut self) {
        self.shutdown_ref();
    }
}

/// Start the console from config (thread named `sc-console`; Off mode starts no thread).
///
/// Shortcut with `hosted = false`; embedded hosts (Tauri/mobile) should use
/// [`spawn_console_hosted`] with the canonical host flag
/// (e.g. `sc_utils::host_mode::is_hosted()`).
pub fn spawn_console(config: ConsoleConfig) -> std::io::Result<ConsoleDriver> {
    spawn_console_hosted(config, false)
}

/// Same as [`spawn_console`] but lets the caller declare hosted mode explicitly (forces `Off`).
pub fn spawn_console_hosted(config: ConsoleConfig, hosted: bool) -> std::io::Result<ConsoleDriver> {
    let stdin_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let mode = config.resolve(hosted, stdin_tty);
    spawn_with_mode(config, mode, stdin_tty)
}

fn spawn_with_mode(
    config: ConsoleConfig,
    mode: ResolvedMode,
    stdin_tty: bool,
) -> std::io::Result<ConsoleDriver> {
    let (log_tx, log_rx) = sync_channel::<LogLine>(config.log_queue.max(16));
    let (input_tx, input_rx) = sync_channel::<String>(config.input_queue.max(1));
    let handle = ConsoleHandle {
        inner: Arc::new(ConsoleShared {
            log_tx,
            provider: RwLock::new(Arc::new(NoopCompletionProvider)),
            header: RwLock::new(ConsoleHeader::default()),
            players: RwLock::new(Vec::new()),
            running: AtomicBool::new(true),
            gui_active: AtomicBool::new(false),
            server_running: AtomicBool::new(false),
            // Welcome gate: closed only when the welcome screen will actually show (GUI plus config on);
            // all other modes start open so the host never waits.
            welcome_dismissed: AtomicBool::new(!(mode == ResolvedMode::Gui && config.welcome)),
            subtitle_ver: AtomicU64::new(0),
            welcome_choice: Mutex::new(None),
            dropped_logs: AtomicU64::new(0),
            dropped_inputs: AtomicU64::new(0),
            max_log_lines: config.max_log_lines.max(64),
            max_completions: config.max_completions.max(1),
        }),
    };

    match mode {
        ResolvedMode::Off => {
            handle.request_shutdown();
            Ok(ConsoleDriver {
                handle,
                input_rx: None,
                thread: None,
                mode,
            })
        }
        ResolvedMode::Plain => {
            let thread_handle = handle.clone();
            let lang =
                crate::config::resolve_lang(config.lang.clone(), config.initial_lang.clone());
            let thread = std::thread::Builder::new()
                .name("sc-console".into())
                .spawn(move || plain_loop(thread_handle, input_tx, lang))?;
            Ok(ConsoleDriver {
                handle,
                input_rx: Some(input_rx),
                thread: Some(thread),
                mode,
            })
        }
        ResolvedMode::Gui => {
            if !stdin_tty {
                let lang = crate::config::resolve_lang(
                    config.lang.clone(),
                    config.initial_lang.clone(),
                );
                eprintln!("{}", rust_i18n::t!("tui.not_tty", locale = lang.as_str()));
                return spawn_with_mode(config, ResolvedMode::Plain, stdin_tty);
            }
            // Move the log receiver and input sender to the UI thread.
            let thread_handle = handle.clone();
            let thread = std::thread::Builder::new()
                .name("sc-console".into())
                .spawn(move || gui_loop(thread_handle, log_rx, input_tx, &config))
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            Ok(ConsoleDriver {
                handle,
                input_rx: Some(input_rx),
                thread: Some(thread),
                mode,
            })
        }
    }
}

// ================= Plain mode =================

fn plain_loop(handle: ConsoleHandle, input_tx: SyncSender<String>, lang: String) {
    eprintln!("{}", rust_i18n::t!("tui.plain_ready", locale = lang.as_str()));
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        if !handle.is_running() {
            break;
        }
        let Ok(line) = line else { break };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        if input_tx.try_send(line).is_err() {
            handle.inner.dropped_inputs.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ================= GUI mode =================

struct GuiState {
    logs: VecDeque<LogLine>,
    editor: EditorState,
    /// Log viewport: follow-bottom or frozen anchor (frozen view ignores new logs).
    view: crate::render::ScrollView,
    /// Total enqueued log count (monotonic; minus `logs.len()` gives evicted rows).
    log_seq: u64,
    /// Frozen watermark: the `log_seq` at freeze time (only later arrivals count as new;
    /// cleared on exit, retaken on next freeze).
    frozen_base_seq: Option<u64>,
    /// Exit double-confirm: time of the first Ctrl+C (a second press within the window stops).
    exit_armed: Option<std::time::Instant>,
    /// Confirm cancel/timeout time (drives the fade; cleared when the fade ends).
    exit_fade_from: Option<std::time::Instant>,
    /// Mouse press origin (distinguishes log-drag from input-click; consumed on release).
    press: Option<PressKind>,
    /// Active log selection (anchored by log index; tracks the same rows as logs arrive).
    selection: Option<Selection>,
    /// Copy notice (input box top-right; disappears after about 2.5s).
    copy_flash: Option<(String, std::time::Instant)>,
    dirty: bool,
    phase: ServerPhase,
    /// Boot animation start (`Some` means stretch/type/fade is playing; `None` means steady boot state).
    /// Always `None` when `ConsoleConfig::boot_anim` is false (plain boot state directly).
    boot_t0: Option<std::time::Instant>,
    /// Log search state (`Some` means searching: match highlight plus status-line search box).
    search: Option<SearchState>,
    /// Completion popup appear-animation start (restarts when items change; `None` when steady).
    completion_t0: Option<std::time::Instant>,
    /// Last rendered completion snapshot (`replace` strings; selection moves do not replay).
    last_comp_key: Option<Vec<String>>,
    /// Sweep entry time (clocks the `Sweeping` phase).
    sweep_t0: Option<std::time::Instant>,
    /// Celebration start time (plays `>STARTUP<` after the sweep without blocking input).
    celebrate_t0: Option<std::time::Instant>,
    /// Afterglow start time (faint green input background after the sweep, fades in ~800ms).
    afterglow_t0: Option<std::time::Instant>,
    /// Animation clock origin (phases the ripple/breathing effects).
    anim_start: std::time::Instant,
    /// Quote typewriter state (delete old, type new, then shine; `」` stays pinned).
    subtitle_anim: crate::Typewriter,
    /// Current display string (typewriter intermediate state).
    sub_display: String,
    /// Quote shine progress (`None` means no shine).
    sub_shine: Option<f32>,
    /// Last rendered terminal size (fullscreen refresh on the next frame after a change).
    last_size: Option<(u16, u16)>,
    /// Session console language (canonical code from the welcome selector, env
    /// override, or host initial; threads all chrome lookups).
    lang: String,
}

/// Boot-success sweep duration (ms).
const SWEEP_MS: u64 = 900;
/// Post-sweep celebration duration (ms): quick fade 150ms, shrink 0.5s, shine hold 1s, fade out.
const CELEBRATE_MS: u64 = 2000;
/// Post-sweep afterglow duration (ms): the faint green input background fades away.
const AFTERGLOW_MS: u64 = 800;
/// Animation frame interval (ms): about 60fps; idle frames follow the configured tick.
const ANIM_FRAME_MS: u64 = 16;
fn gui_loop(
    handle: ConsoleHandle,
    log_rx: Receiver<LogLine>,
    input_tx: SyncSender<String>,
    config: &ConsoleConfig,
) {
    if enter_terminal(&handle).is_err() {
        let lang =
            crate::config::resolve_lang(config.lang.clone(), config.initial_lang.clone());
        eprintln!("{}", rust_i18n::t!("tui.take_over_fail", locale = lang.as_str()));
        return;
    }
    install_atexit_guard();

    // Welcome screen (first GUI launch): fullscreen logo plus flowing backdrop, then the
    // language selector on first runs; returning runs auto-enter after the animation.
    // The animation clock starts after welcome so the boot animation never overlaps it.
    // Session language: env override > host initial (server.properties) > default;
    // the selector updates it and hands the choice to the host for persistence.
    let lang = if config.welcome {
        welcome_wait(
            &handle,
            config.lang.clone(),
            crate::config::resolve_lang(None, config.initial_lang.clone()),
        )
    } else {
        crate::config::resolve_lang(config.lang.clone(), config.initial_lang.clone())
    };
    // Mouse capture is enabled only in the console: the welcome screen consumes keys only,
    // and capture would flood the queue with mouse events that bury keys.
    enable_mouse_capture();

    let now = std::time::Instant::now();
    let mut state = GuiState {
        logs: VecDeque::with_capacity(config.max_log_lines.min(512)),
        editor: EditorState::new(),
        view: crate::render::ScrollView::FollowBottom,
        log_seq: 0,
        frozen_base_seq: None,
        exit_armed: None,
        exit_fade_from: None,
        press: None,
        selection: None,
        copy_flash: None,
        dirty: true,
        phase: ServerPhase::Starting,
        boot_t0: config.boot_anim.then_some(now),
        search: None,
        completion_t0: None,
        last_comp_key: None,
        sweep_t0: None,
        celebrate_t0: None,
        afterglow_t0: None,
        anim_start: now,
        subtitle_anim: crate::Typewriter::new(),
        sub_display: String::new(),
        sub_shine: None,
        last_size: None,
        lang,
    };
    push_welcome(&mut state, config);

    let mut out = std::io::stdout();
    if config.boot_anim {
        // Clear once before the boot animation to remove ghosting from the alternate screen,
        // then let the bars grow left to right.
        let _ = out.write_all(b"\x1b[2J\x1b[H");
        let _ = out.flush();
    }
    let mut sampler = ProcSampler::new();
    let mut renderer = Renderer::default();
    // Clipboard background results (reported by the helper thread without blocking the event loop).
    let (copy_tx, copy_rx) = std::sync::mpsc::channel::<crate::clipboard::CopyOutcome>();

    while handle.is_running() {
        // Sample own-process resources (once per second; fresh samples trigger a repaint for CPU/MEM).
        let (stats, stats_fresh) = sampler.poll();
        if stats_fresh {
            state.dirty = true;
        }
        // Advance the boot phase: host success marker leads to sweep, then unlock.
        // Defer the sweep until the boot animation finishes (both are exclusive locked-state animations).
        let now = std::time::Instant::now();
        // Completion popup animation clock (hit-testing and rendering share the visible count for ~240ms).
        let popup_age = popup_age_for(
            &mut state.last_comp_key,
            &mut state.completion_t0,
            state.editor.completion.as_ref(),
            now,
        );
        if popup_age.is_some() {
            state.dirty = true; // repaint every frame while the popup plays
        }
        match state.phase {
            ServerPhase::Starting
                if should_start_sweep(handle.server_running(), state.boot_t0.is_some()) =>
            {
                state.phase = ServerPhase::Sweeping;
                state.sweep_t0 = Some(now);
                state.dirty = true;
            }
            ServerPhase::Sweeping => {
                let elapsed = state.sweep_t0.map(|t0| now - t0).unwrap_or_default();
                if elapsed >= Duration::from_millis(SWEEP_MS) {
                    state.phase = ServerPhase::Running;
                    state.sweep_t0 = None;
                    // Sweep done: start the celebration (input already unlocked, display only)
                    // plus the afterglow (faint green input background fades out).
                    state.celebrate_t0 = Some(now);
                    state.afterglow_t0 = Some(now);
                }
                // Repaint every frame while the sweep plays.
                state.dirty = true;
            }
            // Keep repainting for the boot ripple and breathing title.
            ServerPhase::Starting => {
                state.dirty = true;
            }
            ServerPhase::Running => {}
        }
        let anim_ms = state.anim_start.elapsed().as_millis() as u64;
        let sweep_progress = match state.phase {
            ServerPhase::Sweeping => {
                let elapsed = state.sweep_t0.map(|t0| now - t0).unwrap_or_default();
                (elapsed.as_millis() as f32 / SWEEP_MS as f32).min(1.0)
            }
            ServerPhase::Running => 1.0,
            ServerPhase::Starting => 0.0,
        };
        // Boot animation: stretch, then type plus fade-in (~1.1s); returns to steady boot state when done.
        // Repaint while playing; `None` means finished or disabled.
        let boot = match state.boot_t0 {
            Some(t0) => {
                let elapsed_ms = now.saturating_duration_since(t0).as_millis() as u64;
                match crate::render::boot_frame(elapsed_ms) {
                    Some(frame) => {
                        state.dirty = true;
                        Some(frame)
                    }
                    None => {
                        state.boot_t0 = None;
                        state.dirty = true; // last frame: bar/text switch to steady state
                        None
                    }
                }
            }
            None => None,
        };
        // Afterglow strength (faint green input background after the sweep, ~800ms; clears when done).
        // Celebration progress (plays independently after the sweep without blocking input; clears when done).
        let afterglow = match state.afterglow_t0.map(|t0| now - t0) {
            Some(elapsed) if elapsed < Duration::from_millis(AFTERGLOW_MS) => {
                state.dirty = true;
                1.0 - elapsed.as_millis() as f32 / AFTERGLOW_MS as f32
            }
            Some(_) => {
                state.afterglow_t0 = None;
                state.dirty = true; // last frame: background color restored
                0.0
            }
            None => 0.0,
        };
        let celebrate = match state.celebrate_t0.map(|t0| now - t0) {
            Some(elapsed) if elapsed < Duration::from_millis(CELEBRATE_MS) => {
                state.dirty = true;
                Some((elapsed.as_millis() as f32 / CELEBRATE_MS as f32).min(1.0))
            }
            Some(_) => {
                state.celebrate_t0 = None;
                state.dirty = true; // last frame: the row disappears
                None
            }
            None => None,
        };
        // Quote typewriter: delete the old text before typing the new one (pin the bracket), then shine.
        let (full_sub, sub_ver) = handle.subtitle();
        let tw = state.subtitle_anim.update(
            &full_sub,
            sub_ver,
            state.anim_start.elapsed().as_millis() as u64,
        );
        if tw.text != state.sub_display {
            state.sub_display = tw.text;
            state.dirty = true;
        }
        state.sub_shine = tw.shine;
        if tw.animating {
            // Keep the frame rate up during delete/type/shine.
            state.dirty = true;
        }
        // Drain logs (bounded batch so one frame never stalls).
        let mut drained = 0;
        while drained < config.drain_per_frame {
            match log_rx.try_recv() {
                Ok(line) => {
                    state.logs.push_back(line);
                    while state.logs.len() > handle.inner.max_log_lines {
                        state.logs.pop_front();
                    }
                    drained += 1;
                    // Global sequence number (shared by anchors and the new-arrival watermark).
                    state.log_seq += 1;
                    state.dirty = true;
                }
                Err(_) => break,
            }
        }
        // Search state: recompute matches when new logs arrive (keeps the current match, not the view).
        if drained > 0 && state.search.is_some() {
            refresh_search(&mut state, false);
            state.dirty = true;
        }
        // Poll events (high frame rate during animations, configured cadence otherwise).
        // The popup appearance (~240ms) also uses a high frame rate: each row slides/fades in over ~4 frames,
        // which a low frame rate would finish in a single frame.
        let animating = state.phase != ServerPhase::Running
            || state.celebrate_t0.is_some()
            || popup_age.is_some();
        let poll_ms = if animating {
            ANIM_FRAME_MS
        } else {
            config.frame_ms.max(10)
        };
        let mut need_complete_refresh = false;
        if event::poll(Duration::from_millis(poll_ms)).unwrap_or(false) {
            // Handle events one by one per frame (cap `MAIN_DRAIN_MAX`): reading only one would bury keys
            // behind resize/wheel floods; each branch matches the old per-event handling,
            // just repeated; `dirty`/`need_complete_refresh` are cumulative flags.
            for _ in 0..MAIN_DRAIN_MAX {
                match event::read() {
                    Ok(Event::Key(key)) => {
                        // Boot animation is skippable: the welcome exit plus boot sequence are both
                        // input-locked (typing is ignored while `locked`), so the combined wait is skipped
                        // on keypress instead of blocking input.
                        if skip_boot_anim(&mut state) {
                            state.dirty = true;
                        }
                        let locked = state.phase != ServerPhase::Running;
                        handle_key(
                            &handle,
                            &mut state,
                            &input_tx,
                            key,
                            config,
                            locked,
                            &mut need_complete_refresh,
                        );
                        state.dirty = true;
                    }
                    Ok(Event::Resize(_, _)) => {
                        state.dirty = true;
                    }
                    Ok(Event::Mouse(mouse)) => {
                        use crossterm::event::{MouseButton, MouseEventKind};
                        match mouse.kind {
                            MouseEventKind::ScrollUp => {
                                state.view = scroll_log(&state, 3);
                                state.dirty = true;
                            }
                            MouseEventKind::ScrollDown => {
                                state.view = scroll_log(&state, -3);
                                state.dirty = true;
                            }
                            // Left press: log area starts a drag selection; input box moves the cursor.
                            // Heights use the content width (the log area narrows when the panel shows).
                            MouseEventKind::Down(MouseButton::Left) => {
                                let (w, h) = size().unwrap_or((100, 30));
                                let cw = crate::render::log_content_width(w as usize);
                                let heights: Vec<usize> = state
                                    .logs
                                    .iter()
                                    .map(|line| crate::render::log_wrap_height(line, cw))
                                    .collect();
                                let total = heights.len();
                                let height_of = |idx: usize| heights.get(idx).copied().unwrap_or(1);
                                let layout = crate::render::log_layout_wrapped(
                                    h as usize,
                                    crate::render::animated_popup_count(
                                        popup_age,
                                        popup_items(&state.editor),
                                    ),
                                    total,
                                    &height_of,
                                    state.log_seq.saturating_sub(total as u64),
                                    state.view,
                                );
                                if !layout.valid {
                                    state.press = None;
                                } else {
                                    let (x, y) = (
                                        (mouse.column as usize).min(w as usize),
                                        mouse.row as usize,
                                    );
                                    match crate::render::hit_test(&layout, &height_of, cw, x, y) {
                                        crate::render::Hit::Log { idx, x } => {
                                            state.press = Some(PressKind::Select);
                                            state.selection = Some(Selection::point(idx, x));
                                            state.dirty = true;
                                        }
                                        crate::render::Hit::Input { row, x } => {
                                            state.press = Some(PressKind::Input);
                                            // Clicking the input box refocuses it: jump the cursor to the click (ignored while locked).
                                            if state.phase == ServerPhase::Running {
                                                let at = crate::render::input_click_cursor(
                                                    &state.editor.text(),
                                                    state.editor.cursor(),
                                                    w as usize,
                                                    row,
                                                    x,
                                                );
                                                state.editor.move_to(at);
                                                state.dirty = true;
                                            }
                                        }
                                        crate::render::Hit::JumpToLatest => {
                                            // Clicking the review hint row jumps back to the bottom (same as Esc).
                                            state.press = Some(PressKind::JumpToLatest);
                                            state.view = crate::render::ScrollView::FollowBottom;
                                            state.dirty = true;
                                        }
                                        crate::render::Hit::None => {
                                            state.press = Some(PressKind::Other);
                                        }
                                    }
                                }
                            }
                            // Drag: only a press that started in the log area extends the selection (clamped to log rows).
                            MouseEventKind::Drag(MouseButton::Left) => {
                                if !matches!(state.press, Some(PressKind::Select)) {
                                    // Ignore drags that did not start a selection.
                                } else {
                                    let (w, h) = size().unwrap_or((100, 30));
                                    let cw = crate::render::log_content_width(w as usize);
                                    let heights: Vec<usize> = state
                                        .logs
                                        .iter()
                                        .map(|line| crate::render::log_wrap_height(line, cw))
                                        .collect();
                                    let total = heights.len();
                                    let height_of =
                                        |idx: usize| heights.get(idx).copied().unwrap_or(1);
                                    let layout = crate::render::log_layout_wrapped(
                                        h as usize,
                                        crate::render::animated_popup_count(
                                            popup_age,
                                            popup_items(&state.editor),
                                        ),
                                        total,
                                        &height_of,
                                        state.log_seq.saturating_sub(total as u64),
                                        state.view,
                                    );
                                    if layout.valid && layout.end > layout.start {
                                        let y = (mouse.row as usize).clamp(
                                            layout.log_top,
                                            layout.log_top + layout.log_rows.saturating_sub(1),
                                        );
                                        let x = (mouse.column as usize).min(w as usize);
                                        // Shares the press mapping (wrapped global columns); drags into the pad extend
                                        // to the content end instead of swallowing the selection.
                                        let (idx, x) = match crate::render::hit_test(
                                            &layout, &height_of, cw, x, y,
                                        ) {
                                            crate::render::Hit::Log { idx, x } => (idx, x),
                                            _ => (layout.end.saturating_sub(1), x),
                                        };
                                        if let Some(sel) = state.selection.as_mut() {
                                            sel.extend(idx, x);
                                            state.dirty = true;
                                        }
                                    }
                                }
                            }
                            // Release: a non-tap drag selection auto-copies its text without extra keys.
                            MouseEventKind::Up(MouseButton::Left) => {
                                let was_select = matches!(state.press, Some(PressKind::Select));
                                state.press = None;
                                if was_select {
                                    if let Some(sel) = state.selection.take() {
                                        if !sel.is_point() {
                                            let (w, _) = size().unwrap_or((100, 30));
                                            let text =
                                                extract_selection(&state.logs, &sel, w as usize);
                                            if !text.trim().is_empty() {
                                                finish_copy(&mut out, &copy_tx, &mut state, text);
                                            }
                                        }
                                        state.dirty = true;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
                // Queue empty means this frame is done; otherwise continue (cap is the `for` bound).
                if !event::poll(Duration::ZERO).unwrap_or(false) {
                    break;
                }
            } // for MAIN_DRAIN_MAX
        }
        if need_complete_refresh {
            refresh_completion(&handle, &mut state.editor);
            state.dirty = true;
        }
        // Clipboard background results become an input-box notice; it fades after ~2.5s.
        while let Ok(outcome) = copy_rx.try_recv() {
            let lang = state.lang.as_str();
            state.copy_flash = Some((
                if outcome.ok {
                    rust_i18n::t!("tui.copy_ok", locale = lang, n = outcome.chars).to_string()
                } else {
                    rust_i18n::t!("tui.copy_fail_clipboard", locale = lang).to_string()
                },
                std::time::Instant::now(),
            ));
            state.dirty = true;
        }
        // Freeze watermark: record `log_seq` on entering review (keep it while reviewing,
        // otherwise scrolling up would reset it and the count would stick at zero); cleared on exit.
        state.frozen_base_seq = freeze_watermark(state.view, state.frozen_base_seq, state.log_seq);
        // Copy notice: lifetime comes from the render-layer timing (fade in, hold, fade out).
        let copy_note = match &state.copy_flash {
            Some((msg, t0)) => {
                let elapsed_ms = t0.elapsed().as_millis() as u64;
                if elapsed_ms < crate::render::COPY_FLASH_MS {
                    state.dirty = true; // keep ticking while the toast exists (fade in/out)
                    Some((msg.clone(), crate::render::copy_opacity(elapsed_ms)))
                } else {
                    state.copy_flash = None;
                    state.dirty = true; // last frame: the toast disappears
                    None
                }
            }
            None => None,
        };
        // Advance the exit double-confirm state: armed timeout turns into fade; fade end clears it.
        let now_i = std::time::Instant::now();
        if state
            .exit_armed
            .is_some_and(|t0| now_i.saturating_duration_since(t0).as_millis() as u64 >= EXIT_ARM_MS)
        {
            state.exit_armed = None;
            state.exit_fade_from = Some(now_i);
        }
        if state.exit_fade_from.is_some_and(|t0| {
            now_i.saturating_duration_since(t0).as_millis() as u64 >= EXIT_FADE_OUT_MS
        }) {
            state.exit_fade_from = None;
            // The final frame must repaint: the confirm pill disappears and the hints fade back in.
            // Without this the line would stall nearly empty until the next log arrives.
            state.dirty = true;
        }
        let exit_prompt = exit_prompt_opacity(state.exit_armed, state.exit_fade_from, now_i);
        if state.exit_armed.is_some() || state.exit_fade_from.is_some() {
            state.dirty = true; // keep the frame rate going during fade in/out
        }
        debug_assert!(
            !(state.exit_armed.is_some() && state.exit_fade_from.is_some()),
            "武装与渐隐互斥（否则提示会闪一下）"
        );
        // Keep the frame rate up during fade in/out.
        if state.dirty {
            state.dirty = false;
            let (w, h) = size().unwrap_or((100, 30));
            // Size changes invalidate the row cache, so the renderer refreshes every row.
            let full_clear = state.last_size != Some((w, h));
            state.last_size = Some((w, h));
            let name = handle.provider_name();
            let header = handle.header();
            // Online player snapshot (read-only side panel; valid for this frame).
            let players = handle.online_players();
            // Search query string (`SearchCtx` borrows it plus the match list; valid for this frame).
            let search_query: String = state
                .search
                .as_ref()
                .map(|s| s.query_text())
                .unwrap_or_default();
            // Popup clock chase: a popup opened by this frame starts playing from 0.
            // Reusing the old clock would flash one steady frame before regrowing;
            // repeated calls with the same key are idempotent, so reuse is safe.
            let popup_age = popup_age_for(
                &mut state.last_comp_key,
                &mut state.completion_t0,
                state.editor.completion.as_ref(),
                now,
            );
            let frame = Frame {
                logs: &state.logs,
                editor: &state.editor,
                view: state.view,
                log_evicted: state.log_seq.saturating_sub(state.logs.len() as u64),
                log_seq: state.log_seq,
                frozen_base_seq: state.frozen_base_seq,
                selection: state.selection,
                players: &players,
                search: state
                    .search
                    .as_ref()
                    .map(|s| search_ctx_for(s, &search_query, now)),
                copy_note: copy_note.clone(),
                exit_prompt: (exit_prompt > 0.0).then_some(exit_prompt),
                dropped_logs: handle.dropped_logs(),
                provider_name: &name,
                header: &header,
                subtitle: &state.sub_display,
                subtitle_shine: state.sub_shine,
                lang: state.lang.as_str(),
                stats: &stats,
                phase: state.phase,
                sweep_progress,
                boot,
                celebrate,
                afterglow,
                popup_age_ms: popup_age,
                full_clear,
                anim_ms,
                width: w,
                height: h,
            };
            if renderer.render(&mut out, &frame).is_err() {
                // Render failure (e.g. terminal gone): leave the loop and restore the terminal.
                break;
            }
        }
    }

    restore_terminal(&handle);
}

/// Whether to start the sweep: the server is marked done and the boot animation finished or is off.
/// Boot and sweep play in sequence (only used in the `Starting` branch).
fn should_start_sweep(server_running: bool, boot_active: bool) -> bool {
    server_running && !boot_active
}
// ================= Terminal takeover and restore =================

fn enter_terminal(handle: &ConsoleHandle) -> std::io::Result<()> {
    enable_raw_mode()?;
    let mut out = std::io::stdout();
    out.execute(EnterAlternateScreen)?;
    out.execute(Hide)?;
    // Mouse capture stays off here (welcome only consumes keys; capture would flood and bury keys;
    // it is enabled explicitly by `enable_mouse_capture` before the console takes over).
    handle.inner.gui_active.store(true, Ordering::Relaxed);
    Ok(())
}

/// Enable mouse capture (after welcome: only the console needs wheel scrolling and selection).
///
/// Best effort: terminals without support ignore the error; keyboard paging still works.
/// `restore_terminal` always disables capture (a no-op when never enabled).
fn enable_mouse_capture() {
    use crossterm::event::EnableMouseCapture;
    let _ = std::io::stdout().execute(EnableMouseCapture);
}

fn restore_terminal(handle: &ConsoleHandle) {
    if !handle.inner.gui_active.swap(false, Ordering::Relaxed) {
        return;
    }
    let mut out = std::io::stdout();
    use crossterm::event::DisableMouseCapture;
    let _ = out.write_all(b"\x1b[?2026l\x1b[?7h\x1b[0m");
    let _ = out.execute(DisableMouseCapture);
    let _ = out.execute(Show);
    let _ = out.execute(LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = out.flush();
}

/// Atexit fallback: still restores the terminal on `process::exit` / panic paths.
///
/// Only lock-free work here (atomic flags plus direct stderr writes plus raw-mode off);
/// never touches game state.
fn install_atexit_guard() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| unsafe {
        extern "C" fn restore() {
            // Write the leave sequences via stdout; ignore failures (nothing left to do).
            // Includes mouse-capture off (?1000l/?1006l) so scrolling still works after exit.
            let _ = (&mut std::io::stdout() as &mut dyn Write)
                .write_all(b"\x1b[?2026l\x1b[?7h\x1b[0m\x1b[?1000l\x1b[?1006l\x1b[?1049l\x1b[?25h");
            let _ = (&mut std::io::stdout() as &mut dyn Write).flush();
            let _ = disable_raw_mode();
        }
        libc::atexit(restore);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    #[test]
    fn any_key_skips_boot_anim_once() {
        let (_h, _tx, _rx, mut state) = locked_state();
        // Not playing: skipping is a no-op and must not report success.
        assert!(!skip_boot_anim(&mut state));
        // Playing: jump to the end state, exactly once.
        state.boot_t0 = Some(std::time::Instant::now());
        assert!(skip_boot_anim(&mut state));
        assert!(state.boot_t0.is_none());
        assert!(!skip_boot_anim(&mut state), "只跳一次");
    }

    #[test]
    fn welcome_should_exit_only_after_anim_and_only_when_dismissed() {
        // The exit predicate is unit-tested on its own; the loop stays single-level.
        // (Keeps exit control flow unambiguous so keys always leave.)
        assert!(!welcome_should_exit(0, None));
        assert!(!welcome_should_exit(u64::MAX, None), "没按键永不退出");
        let d = 2000u64;
        let exit = crate::welcome::WELCOME_EXIT_MS;
        assert!(!welcome_should_exit(d, Some(d)), "首键瞬间不退出");
        assert!(!welcome_should_exit(d + exit - 1, Some(d)), "差 1ms 也不退出");
        assert!(welcome_should_exit(d + exit, Some(d)), "播完即退出");
        assert!(welcome_should_exit(d + exit + 9999, Some(d)));
        assert!(
            !welcome_should_exit(d.saturating_sub(1), Some(d)),
            "时钟回绕/抖动不误退"
        );
    }

    #[test]
    fn select_move_wraps_and_clamps() {
        assert_eq!(select_move(0, -1, 2), 1, "wraps upward");
        assert_eq!(select_move(1, 1, 2), 0, "wraps downward");
        assert_eq!(select_move(0, 1, 2), 1);
        assert_eq!(select_move(0, 0, 0), 0, "empty list is safe");
        assert_eq!(select_move(5, 3, 1), 0, "single option stays");
    }

    #[test]
    fn selector_menu_moves_toggles_confirms() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use SelectAction as A;
        let none = KeyModifiers::empty();
        // Order [en(0), zh(1)]: checkbox exists for Chinese only.
        let zh = Some(1usize);
        let mut m = SelectMenu::new(0);
        // Down from English moves (no checkbox there).
        assert_eq!(m.key(2, zh, KeyCode::Down, none), A::Updated);
        assert_eq!((m.selected, m.focus), (1, SelectFocus::List));
        // Down through Chinese parks focus on the right-hand checkbox.
        assert_eq!(m.key(2, zh, KeyCode::Down, none), A::Updated);
        assert_eq!((m.selected, m.focus), (1, SelectFocus::Quote));
        assert!(!m.hitokoto, "default off");
        // Left/Right travel between the boxes (directional); Space toggles.
        assert_eq!(m.key(2, zh, KeyCode::Right, none), A::Updated);
        assert_eq!(m.focus, SelectFocus::Quote, "right stays");
        assert_eq!(m.key(2, zh, KeyCode::Left, none), A::Updated);
        assert_eq!((m.selected, m.focus), (1, SelectFocus::List));
        assert_eq!(m.key(2, zh, KeyCode::Right, none), A::Updated);
        assert_eq!(m.key(2, zh, KeyCode::Char(' '), none), A::Updated);
        assert!(m.hitokoto, "space toggles on");
        assert!(effective_hitokoto(&m, zh), "checkbox counts for Chinese");
        assert_eq!(m.key(2, zh, KeyCode::Char(' '), none), A::Updated);
        assert!(!m.hitokoto, "space toggles back off");
        // Vertical keys leave the checkbox (highlight stays).
        assert_eq!(m.key(2, zh, KeyCode::Right, none), A::Updated); // focus Quote again
        assert_eq!(m.key(2, zh, KeyCode::Down, none), A::Updated);
        assert_eq!((m.selected, m.focus), (1, SelectFocus::List));
        // Enter on the checkbox checks the box instead of confirming.
        assert_eq!(m.key(2, zh, KeyCode::Right, none), A::Updated);
        assert_eq!(m.key(2, zh, KeyCode::Enter, none), A::Updated);
        assert!(m.hitokoto, "enter checks the box");
        assert_eq!(m.focus, SelectFocus::Quote, "stays for more toggling");
        assert_eq!(m.key(2, zh, KeyCode::Enter, none), A::Updated);
        assert!(!m.hitokoto, "enter toggles back off");
        // Leave the box, then Enter confirms.
        assert_eq!(m.key(2, zh, KeyCode::Left, none), A::Updated);
        assert_eq!(m.key(2, zh, KeyCode::Enter, none), A::Confirm);
        // English confirm forces the flag off even if it was somehow on.
        m.selected = 0;
        m.hitokoto = true;
        assert!(!effective_hitokoto(&m, zh));
        // Wrap: up from English lands on Chinese; Esc backs out.
        let mut m = SelectMenu::new(0);
        assert_eq!(m.key(2, zh, KeyCode::Up, none), A::Updated);
        assert_eq!(m.selected, 1);
        assert_eq!(m.key(2, zh, KeyCode::Esc, none), A::Back);
        // Digits jump and drop focus back to the list.
        let mut m = SelectMenu::new(0);
        assert_eq!(m.key(2, zh, KeyCode::Char('2'), none), A::Updated);
        assert_eq!((m.selected, m.focus), (1, SelectFocus::List));
        // No Chinese option: no checkbox anywhere (Down wraps past).
        let mut m = SelectMenu::new(0);
        assert_eq!(m.key(1, None, KeyCode::Down, none), A::Updated);
        assert_eq!((m.selected, m.focus), (0, SelectFocus::List));
        assert!(!effective_hitokoto(&m, None));
        // English highlighted: Left/Right cannot reach any checkbox.
        let mut m = SelectMenu::new(0);
        assert_eq!(m.key(2, zh, KeyCode::Right, none), A::Updated);
        assert_eq!((m.selected, m.focus), (0, SelectFocus::List));
        assert_eq!(m.key(2, zh, KeyCode::Left, none), A::Updated);
        assert_eq!((m.selected, m.focus), (0, SelectFocus::List));
    }

    #[test]
    fn selector_ctrl_c_contract() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        assert!(is_ctrl_c(&KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
        assert!(is_ctrl_c(&KeyEvent::new(
            KeyCode::Char('C'),
            KeyModifiers::CONTROL
        )));
        assert!(!is_ctrl_c(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)));
        assert!(!is_ctrl_c(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    }

    #[test]
    fn push_welcome_greeting_follows_session_language() {
        // locked_state pins en-US: the greeting renders English.
        let (_h, _tx, _rx, mut state) = locked_state();
        assert_eq!(state.lang, crate::config::DEFAULT_LANG);
        push_welcome(&mut state, &ConsoleConfig::default());
        let last = state.logs.back().expect("greeting logged");
        assert!(last.message.contains("Console GUI ready"), "{:?}", last.message);
        assert!(!last.message.contains("已就绪"), "no Chinese in en session");
        // Same call in Chinese renders the zh greeting.
        state.lang = crate::config::LANG_ZH_CN.to_string();
        push_welcome(&mut state, &ConsoleConfig::default());
        let last = state.logs.back().expect("greeting logged");
        assert!(last.message.contains("已就绪"), "{:?}", last.message);
    }

    #[test]
    fn next_frame_interval_never_runs_faster_than_last_write() {
        let budget = Duration::from_millis(16);
        // Fast writes: keep the budgeted frame rate.
        assert_eq!(
            next_frame_interval(budget, Duration::from_millis(2)),
            budget
        );
        // Slow writes (terminal cannot keep up): yield the frame interval to the write cost, never catch up.
        // A fixed cadence would park the loop in paint and starve polling.
        assert_eq!(
            next_frame_interval(budget, Duration::from_millis(551)),
            Duration::from_millis(551)
        );
        // Equal to the budget means no shortening.
        assert_eq!(next_frame_interval(budget, budget), budget);
    }

    #[test]
    fn next_frame_interval_is_monotonic_in_last_write() {
        let budget = Duration::from_millis(16);
        let mut prev = Duration::ZERO;
        for ms in [0u64, 1, 15, 16, 17, 100, 551] {
            let iv = next_frame_interval(budget, Duration::from_millis(ms));
            assert!(iv >= prev, "间隔随写耗时单调不减：{ms}ms → {iv:?}");
            assert!(iv >= budget, "不得快于目标帧率：{ms}ms → {iv:?}");
            prev = iv;
        }
    }

    #[test]
    fn drain_events_finds_key_behind_mouse_flood() {
        // With mouse capture on, mouse moves keep producing events.
        // Reading one per frame buries keys behind the flood and delays console entry by seconds.
        //
        // Scale note: mouse moves run near 100Hz, only a few events per frame; this test uses
        // 300 events as a stress assertion, still far below `EVENT_DRAIN_MAX` (4096, about 240k
        // events/s), which real dragging can never hit.
        use crossterm::event::{MouseEvent, MouseEventKind};
        const FLOOD: usize = 300;
        let mut queue: Vec<Event> = (0..FLOOD)
            .map(|_| {
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Moved,
                    column: 0,
                    row: 0,
                    modifiers: KeyModifiers::empty(),
                })
            })
            .collect();
        queue.push(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::empty(),
        )));
        let mut iter = queue.into_iter();
        let batch = drain_events(|| iter.next());
        assert_eq!(batch.keys.len(), 1, "洪水后的按键必须被扫到");
        assert_eq!(batch.keys[0].code, KeyCode::Enter);
        assert_eq!(batch.other, FLOOD, "洪水计入非按键");
    }

    #[test]
    fn drain_events_reports_empty_queue() {
        let batch = drain_events(|| None);
        assert!(batch.keys.is_empty());
        assert_eq!(batch.other, 0);
    }

    #[test]
    fn drain_events_caps_hunger_but_key_within_cap_survives() {
        // An unbounded flood (producing faster than painting) must not starve painting: stop at the cap, continue next frame.
        let batch = drain_events(|| {
            Some(Event::Resize(1, 1))
        });
        assert_eq!(batch.keys.len() + batch.other, EVENT_DRAIN_MAX);
        assert!(batch.keys.is_empty(), "纯缩放事件不误报按键");
        // Keys within the cap are always scanned (never pushed past the cap by the flood).
        let mut n = 0usize;
        let batch = drain_events(|| {
            n += 1;
            if n == EVENT_DRAIN_MAX {
                Some(Event::Key(KeyEvent::new(
                    KeyCode::Enter,
                    KeyModifiers::empty(),
                )))
            } else {
                Some(Event::Resize(1, 1))
            }
        });
        assert_eq!(batch.keys.len(), 1);
        assert_eq!(batch.keys.len() + batch.other, EVENT_DRAIN_MAX);
    }

    fn locked_state() -> (
        ConsoleHandle,
        SyncSender<String>,
        Receiver<String>,
        GuiState,
    ) {
        let driver = spawn_with_mode(ConsoleConfig::default(), ResolvedMode::Off, false).unwrap();
        let (tx, rx) = sync_channel::<String>(8);
        let state = GuiState {
            logs: VecDeque::new(),
            editor: EditorState::new(),
            view: crate::render::ScrollView::FollowBottom,
            log_seq: 0,
            frozen_base_seq: None,
            exit_armed: None,
            exit_fade_from: None,
            press: None,
            selection: None,
            copy_flash: None,
            dirty: false,
            phase: ServerPhase::Starting,
            boot_t0: None,
            search: None,
            completion_t0: None,
            last_comp_key: None,
            sweep_t0: None,
            celebrate_t0: None,
            afterglow_t0: None,
            anim_start: std::time::Instant::now(),
            subtitle_anim: crate::Typewriter::new(),
            sub_display: String::new(),
            sub_shine: None,
            last_size: None,
            lang: crate::config::DEFAULT_LANG.to_string(),
        };
        (driver.handle(), tx, rx, state)
    }

    #[test]
    fn locked_input_ignores_typing_but_allows_stop() {
        let (handle, tx, rx, mut state) = locked_state();
        let config = ConsoleConfig::default();
        let mut refresh = false;
        // Typing is ignored.
        handle_key(
            &handle,
            &mut state,
            &tx,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            &config,
            true,
            &mut refresh,
        );
        assert!(state.editor.is_empty());
        assert!(rx.try_recv().is_err());
        // Enter is ignored.
        handle_key(
            &handle,
            &mut state,
            &tx,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &config,
            true,
            &mut refresh,
        );
        assert!(rx.try_recv().is_err());
        // Empty-line Ctrl+C arms the double confirm: first press only arms, sends no command.
        let ctrl_c = || KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        handle_key(
            &handle,
            &mut state,
            &tx,
            ctrl_c(),
            &config,
            true,
            &mut refresh,
        );
        assert!(rx.try_recv().is_err(), "第一次只确认提示，不停止");
        assert!(state.exit_armed.is_some(), "已武装");
        // The second press (inside the window) actually stops via the normal command channel.
        handle_key(
            &handle,
            &mut state,
            &tx,
            ctrl_c(),
            &config,
            true,
            &mut refresh,
        );
        assert_eq!(rx.try_recv().unwrap(), "stop");
        assert!(state.exit_armed.is_none());
        // Typing works again after unlock.
        handle_key(
            &handle,
            &mut state,
            &tx,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
            &config,
            false,
            &mut refresh,
        );
        assert_eq!(state.editor.text(), "b");
    }

    #[test]
    fn ctrl_c_requires_second_press_and_esc_cancels() {
        let config = ConsoleConfig::default();
        let mut refresh = false;
        let ctrl_c = || KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let esc = || KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

        for locked in [true, false] {
            let (handle, tx, rx, mut state) = locked_state();
            state.phase = ServerPhase::Running;
            let press = |state: &mut GuiState, refresh: &mut bool| {
                handle_key(&handle, state, &tx, ctrl_c(), &config, locked, refresh);
            };
            // First press: armed, no command.
            press(&mut state, &mut refresh);
            assert!(state.exit_armed.is_some(), "locked={locked} 首次武装");
            assert!(rx.try_recv().is_err(), "首次不停止");
            // Esc cancels the arming (enters fade); no second confirm needed.
            handle_key(
                &handle,
                &mut state,
                &tx,
                esc(),
                &config,
                locked,
                &mut refresh,
            );
            assert!(state.exit_armed.is_none(), "Esc 取消武装");
            assert!(state.exit_fade_from.is_some(), "取消后渐隐");
            assert!(rx.try_recv().is_err(), "取消不停止");
            // Another Ctrl+C counts as a fresh round: still only arms.
            press(&mut state, &mut refresh);
            assert!(state.exit_armed.is_some());
            assert!(rx.try_recv().is_err(), "取消后需重新两次确认");
            // Second press: actually stops.
            press(&mut state, &mut refresh);
            assert_eq!(rx.try_recv().unwrap(), "stop");
        }
    }

    #[test]
    fn exit_prompt_opacity_fades_in_holds_and_fades_out() {
        let now = std::time::Instant::now();
        let at = |ms: u64| now + std::time::Duration::from_millis(ms);
        // While armed: fades in within 150ms, then stays full.
        assert_eq!(exit_prompt_opacity(Some(now), None, now), 0.0);
        assert_eq!(exit_prompt_opacity(Some(now), None, at(75)), 0.5);
        assert_eq!(
            exit_prompt_opacity(Some(now), None, at(EXIT_FADE_IN_MS)),
            1.0
        );
        assert_eq!(
            exit_prompt_opacity(Some(now), None, at(EXIT_ARM_MS - 1)),
            1.0
        );
        // After cancel: fades to 0 within 400ms.
        assert_eq!(exit_prompt_opacity(None, Some(now), now), 1.0);
        assert_eq!(
            exit_prompt_opacity(None, Some(now), at(EXIT_FADE_OUT_MS / 2)),
            0.5
        );
        assert_eq!(
            exit_prompt_opacity(None, Some(now), at(EXIT_FADE_OUT_MS)),
            0.0
        );
        // Nothing armed means 0 (the status line shows the normal key hints).
        assert_eq!(exit_prompt_opacity(None, None, now), 0.0);
        // An armed timeout (already fading in the driver) prefers the fade.
        assert_eq!(
            exit_prompt_opacity(None, Some(now), at(EXIT_FADE_OUT_MS + 1)),
            0.0
        );
    }

    #[test]
    fn esc_priority_popup_then_bottom_then_clear() {
        use crate::completion::CompletionItem;
        use crate::editor::CompletionState;
        let config = ConsoleConfig::default();
        let mut refresh = false;
        let esc = || KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

        // 1) Popup open means only close the popup (scroll and input row stay).
        let (handle, tx, _rx, mut state) = locked_state();
        state.phase = ServerPhase::Running;
        state.view = crate::render::ScrollView::Frozen { top_seq: 5 };
        state.editor.insert_char('x');
        state.editor.completion = Some(CompletionState {
            items: vec![CompletionItem {
                display: "stop".into(),
                replace: "stop".into(),
                description: String::new(),
                from_alias: false,
            }],
            selected: 0,
        });
        handle_key(
            &handle,
            &mut state,
            &tx,
            esc(),
            &config,
            false,
            &mut refresh,
        );
        assert!(state.editor.completion.is_none(), "弹窗关闭");
        assert_eq!(
            state.view,
            crate::render::ScrollView::Frozen { top_seq: 5 },
            "滚动保持"
        );
        assert_eq!(state.editor.text(), "x", "输入行保留");

        // 2) Reviewing without a popup means back to the bottom (input row kept).
        handle_key(
            &handle,
            &mut state,
            &tx,
            esc(),
            &config,
            false,
            &mut refresh,
        );
        assert_eq!(
            state.view,
            crate::render::ScrollView::FollowBottom,
            "回到到底部"
        );
        assert_eq!(state.editor.text(), "x", "输入行保留");

        // 3) At the bottom without a popup means clear the line (original behavior).
        handle_key(
            &handle,
            &mut state,
            &tx,
            esc(),
            &config,
            false,
            &mut refresh,
        );
        assert!(state.editor.is_empty(), "清空行");
        // 4) Esc during lock also returns to the bottom (matches the review hint).
        let (handle, tx, _rx, mut state) = locked_state();
        state.view = crate::render::ScrollView::Frozen { top_seq: 7 };
        handle_key(&handle, &mut state, &tx, esc(), &config, true, &mut refresh);
        assert_eq!(state.view, crate::render::ScrollView::FollowBottom);
    }

    #[test]
    fn scroll_clamps_at_top_and_never_accumulates() {
        use crate::render::ScrollView::{FollowBottom, Frozen};
        // height=24 with no popup shows 16 log rows; 100 logs put the bottom top at 84.
        // One step up from the bottom enters review (anchored by global sequence).
        assert_eq!(
            crate::render::scroll_view(24, 0, 100, 0, FollowBottom, 1),
            Frozen { top_seq: 83 }
        );
        // Scrolling further up at the very top pins top at 0 without accumulating.
        assert_eq!(
            crate::render::scroll_view(24, 0, 100, 0, Frozen { top_seq: 2 }, 10),
            Frozen { top_seq: 0 }
        );
        assert_eq!(
            crate::render::scroll_view(24, 0, 100, 0, Frozen { top_seq: 0 }, 3),
            Frozen { top_seq: 0 }
        );
        // Scrolling back down one step takes effect immediately.
        assert_eq!(
            crate::render::scroll_view(24, 0, 100, 0, Frozen { top_seq: 0 }, -3),
            Frozen { top_seq: 3 }
        );
        // Scrolling to the bottom returns to follow mode (the hint disappears).
        assert_eq!(
            crate::render::scroll_view(24, 0, 100, 0, Frozen { top_seq: 80 }, -10),
            FollowBottom
        );
        // Scrolling further down at the bottom stays in follow mode without entering review.
        assert_eq!(
            crate::render::scroll_view(24, 0, 100, 0, FollowBottom, -5),
            FollowBottom
        );
        // Fewer logs than one page means nothing to review; stays in follow mode.
        assert_eq!(
            crate::render::scroll_view(24, 0, 3, 0, FollowBottom, 10),
            FollowBottom
        );
    }

    #[test]
    fn frozen_view_survives_ring_eviction() {
        use crate::render::{log_layout, ScrollView};
        // Anchored at global sequence 50: after 10 rows are evicted the same log stays first in view
        // (its deque index moves from 50 to 40, but the content is unchanged).
        let view = ScrollView::Frozen { top_seq: 50 };
        assert_eq!(log_layout(24, 0, 100, 0, view).start, 50);
        assert_eq!(
            log_layout(24, 0, 100, 10, view).start,
            40,
            "淘汰后锚定行仍居首行"
        );
        // An anchor row already evicted (top_seq < evicted) clamps to the very top without going out of bounds.
        let gone = log_layout(24, 0, 100, 10, ScrollView::Frozen { top_seq: 3 });
        assert_eq!(gone.start, 0);
        assert!(gone.valid);
    }

    #[test]
    fn freeze_watermark_marks_entry_point_once() {
        use crate::render::ScrollView::{FollowBottom, Frozen};
        // Not reviewing means no watermark (badge count 0).
        assert_eq!(freeze_watermark(FollowBottom, None, 100), None);
        assert_eq!(freeze_watermark(FollowBottom, Some(100), 130), None);
        // First entry into review takes the current log_seq (earlier arrivals never count as new).
        assert_eq!(
            freeze_watermark(Frozen { top_seq: 5 }, None, 100),
            Some(100)
        );
        // Already reviewing keeps the old watermark (scrolling up must not reset it).
        assert_eq!(
            freeze_watermark(Frozen { top_seq: 1 }, Some(100), 130),
            Some(100)
        );
        // Back to the bottom and reviewing again retakes the watermark (old rows never count as new).
        assert_eq!(freeze_watermark(FollowBottom, Some(100), 130), None);
        assert_eq!(
            freeze_watermark(Frozen { top_seq: 120 }, None, 130),
            Some(130)
        );
    }

    #[test]
    fn frozen_view_is_not_pushed_by_new_logs() {
        use crate::render::{log_layout, ScrollView};
        // Anchored at top_seq=3 with 7 new logs: the first visible row is unchanged and content never moves.
        let view = ScrollView::Frozen { top_seq: 3 };
        let before = log_layout(24, 0, 100, 0, view);
        let after = log_layout(24, 0, 107, 0, view);
        assert_eq!(before.start, 3);
        assert_eq!(after.start, 3, "新日志不顶动视图");
        assert_eq!(after.end - after.start, before.log_rows, "视口高度不变");
        // Follow-bottom mode keeps following as before.
        let follow = log_layout(24, 0, 107, 0, ScrollView::FollowBottom);
        assert_eq!(follow.end, 107, "跟随底部时贴住最新");
        assert_eq!(follow.up, 0);
    }

    #[test]
    fn extract_selection_has_no_escape_codes() {
        // End to end: a styled log row yields plain text for the clipboard.
        let logs = VecDeque::from([
            LogLine::new(LogLevel::Info, "t", "§a你好\x1b[31m世界\x1b[0m"),
            LogLine::new(LogLevel::Warn, "t", "plain line"),
        ]);
        let mut sel = Selection::point(0, 0);
        sel.extend(1, 100);
        let text = extract_selection(&logs, &sel, 80);
        assert!(!text.contains('\x1b'), "无 ANSI 转义");
        assert!(!text.contains('§'), "无 § 颜色码");
        assert!(text.contains("你好世界"), "中文内容完整");
        assert!(text.contains("plain line"), "第二行完整");
    }

    #[test]
    fn sweep_waits_for_boot_to_finish() {
        // No sweep before the boot animation finishes (sequential animations); a disabled boot sweeps normally.
        assert!(should_start_sweep(true, false));
        assert!(!should_start_sweep(true, true), "开场播完前暂缓横扫");
        assert!(!should_start_sweep(false, false));
        assert!(!should_start_sweep(false, true));
    }

    /// Search-state fixture with 3 logs (global sequences 0/1/2, advanced with `log_seq`, simulating drain).
    fn search_state_with_logs() -> GuiState {
        let (_handle, _tx, _rx, mut state) = locked_state();
        for msg in ["stop server", "hello world", "stopping now"] {
            state.logs.push_back(LogLine::new(LogLevel::Info, "t", msg));
            state.log_seq += 1;
        }
        state
    }

    fn type_query(state: &mut GuiState, query: &str) {
        for c in query.chars() {
            handle_search_key(state, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    #[test]
    fn search_types_steps_and_exits_without_side_effects() {
        let mut state = search_state_with_logs();
        // Review before entering: exiting must restore this view (not the bottom).
        state.view = crate::render::ScrollView::Frozen { top_seq: 0 };
        enter_search(&mut state);
        assert!(state.search.is_some());
        // Typing "stop" char by char: incremental recompute plus auto-jump to the first match.
        type_query(&mut state, "stop");
        {
            let search = state.search.as_ref().expect("查找态");
            assert_eq!(search.query_text(), "stop");
            assert_eq!(search.matches.len(), 2);
            assert_eq!(search.current, 0);
            assert_eq!(search.matches[0].seq, 0);
        }
        // Enter goes next to 1; Enter again wraps to 0; Shift+Enter goes back to 1.
        let enter = || KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        handle_search_key(&mut state, enter());
        assert_eq!(state.search.as_ref().unwrap().current, 1);
        handle_search_key(&mut state, enter());
        assert_eq!(state.search.as_ref().unwrap().current, 0);
        handle_search_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
        );
        assert_eq!(state.search.as_ref().unwrap().current, 1);
        // Up goes to the previous one (0).
        handle_search_key(&mut state, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(state.search.as_ref().unwrap().current, 0);
        // Ctrl+C with a query only clears it (never exits, never arms shutdown).
        handle_search_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(state.search.as_ref().unwrap().query.is_empty());
        assert!(state.search.as_ref().unwrap().matches.is_empty());
        assert!(state.search.is_some(), "清空不退出");
        assert!(state.exit_armed.is_none(), "查找态不武装关服");
        // Empty-query Ctrl+C exits and restores the entry view (frozen 0, not the bottom).
        handle_search_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(state.search.is_none());
        assert_eq!(
            state.view,
            crate::render::ScrollView::Frozen { top_seq: 0 },
            "退出恢复进入时视图"
        );
    }

    #[test]
    fn search_esc_restores_entry_view() {
        let mut state = search_state_with_logs();
        enter_search(&mut state);
        type_query(&mut state, "hello");
        assert_eq!(state.search.as_ref().unwrap().matches.len(), 1);
        handle_search_key(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(state.search.is_none());
        assert_eq!(state.view, crate::render::ScrollView::FollowBottom);
    }

    #[test]
    fn search_ctx_carries_current_anim_age() {
        use std::time::{Duration, Instant};
        let mut search = SearchState {
            query: vec!['s'],
            cursor: 1,
            matches: vec![SearchMatch {
                seq: 0,
                x0: 0,
                x1: 1,
            }],
            current: 0,
            current_t0: Instant::now(),
            entry_view: crate::render::ScrollView::FollowBottom,
        };
        let now = Instant::now();
        // Just selected: small age, the renderer plays fade/shine.
        let ctx = search_ctx_for(&search, "s", now);
        assert!(ctx
            .current_age_ms
            .is_some_and(|a| a < crate::render::SEARCH_SHINE_MS));
        assert_eq!(ctx.current, 0);
        // 600ms after selection: finished, the renderer uses the steady style.
        search.current_t0 = now.checked_sub(Duration::from_millis(600)).unwrap();
        let ctx = search_ctx_for(&search, "s", now);
        assert_eq!(ctx.current_age_ms, None);
        // Empty matches: no highlight and no animation.
        search.matches.clear();
        search.current_t0 = Instant::now();
        let ctx = search_ctx_for(&search, "s", now);
        assert_eq!(ctx.current_age_ms, None);
    }

    #[test]
    fn search_keeps_current_match_when_new_logs_arrive() {
        let mut state = search_state_with_logs();
        enter_search(&mut state);
        type_query(&mut state, "stop");
        assert_eq!(state.search.as_ref().unwrap().current, 0);
        // A new non-matching log (simulated drain): the current match identity is unchanged.
        state
            .logs
            .push_back(LogLine::new(LogLevel::Info, "t", "nothing here"));
        state.log_seq += 1;
        refresh_search(&mut state, false);
        {
            let search = state.search.as_ref().unwrap();
            assert_eq!(search.matches.len(), 2);
            assert_eq!(search.current, 0);
            assert_eq!(search.matches[0].seq, 0);
        }
        // A new matching log: total grows by one, the current match stays pinned.
        state
            .logs
            .push_back(LogLine::new(LogLevel::Info, "t", "please stop"));
        state.log_seq += 1;
        refresh_search(&mut state, false);
        {
            let search = state.search.as_ref().unwrap();
            assert_eq!(search.matches.len(), 3);
            assert_eq!(search.current, 0);
            assert_eq!(search.matches[0].seq, 0);
        }
    }

    #[test]
    fn jump_target_view_centers_offscreen_match() {
        use crate::render::ScrollView::{FollowBottom, Frozen};
        // height=30 with no popup: 22 log rows, minus 1 hint row leaves 21.
        // Target sequence 99 is outside the view, so center it: 99-10=89, clamped to max_start=79.
        assert_eq!(
            jump_target_view(30, 0, 100, 0, Frozen { top_seq: 0 }, 99),
            Frozen { top_seq: 79 }
        );
        // A target already in view keeps the view still (less jumping).
        assert_eq!(
            jump_target_view(30, 0, 100, 0, Frozen { top_seq: 0 }, 5),
            Frozen { top_seq: 0 }
        );
        assert_eq!(
            jump_target_view(30, 0, 100, 0, FollowBottom, 99),
            FollowBottom,
            "跟随底部且目标可见（末行）则不动"
        );
        // A tiny terminal with an invalid layout keeps the old view.
        assert_eq!(
            jump_target_view(8, 0, 100, 0, Frozen { top_seq: 0 }, 99),
            Frozen { top_seq: 0 }
        );
    }

    #[test]
    fn completion_up_down_navigates_with_wrap() {
        use crate::completion::CompletionItem;
        use crate::editor::CompletionState;
        let (handle, tx, _rx, mut state) = locked_state();
        let config = ConsoleConfig::default();
        let mut refresh = false;
        state.editor.completion = Some(CompletionState {
            items: vec![
                CompletionItem::new("stop", "stop", "Stops the server", false),
                CompletionItem::new("start", "start", "Starts it", false),
                CompletionItem::new("status", "status", "Shows status", false),
            ],
            selected: 0,
        });
        let up = || KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let down = || KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let selected = |state: &GuiState| state.editor.completion.as_ref().unwrap().selected;
        // Up at the top wraps to the bottom (mirrors Down wrapping to the top).
        handle_key(&handle, &mut state, &tx, up(), &config, false, &mut refresh);
        assert_eq!(selected(&state), 2);
        // Down returns to the top.
        handle_key(
            &handle,
            &mut state,
            &tx,
            down(),
            &config,
            false,
            &mut refresh,
        );
        assert_eq!(selected(&state), 0);
        handle_key(
            &handle,
            &mut state,
            &tx,
            down(),
            &config,
            false,
            &mut refresh,
        );
        assert_eq!(selected(&state), 1);
        // Without a popup, Up walks history (never conjures a popup).
        state.editor.clear_completion();
        handle_key(&handle, &mut state, &tx, up(), &config, false, &mut refresh);
        assert!(state.editor.completion.is_none());
    }

    #[test]
    fn popup_age_replays_only_on_appear() {
        use crate::completion::CompletionItem;
        use crate::editor::CompletionState;
        fn comp(replace: &str, selected: usize) -> CompletionState {
            CompletionState {
                items: vec![CompletionItem::new(replace, replace, "", false)],
                selected,
            }
        }
        let t0 = std::time::Instant::now();
        let mut last: Option<Vec<String>> = None;
        let mut start: Option<std::time::Instant> = None;
        // Popup appears (closed to open): the animation starts.
        let c = comp("stop", 0);
        assert!(popup_age_for(&mut last, &mut start, Some(&c), t0)
            .is_some_and(|a| a < crate::render::POPUP_APPEAR_MS));
        // Selection moves only: no restart (start time unchanged).
        let saved = start;
        let c = comp("stop", 0);
        let _ = popup_age_for(&mut last, &mut start, Some(&c), std::time::Instant::now());
        assert_eq!(start, saved, "选区移动不重播出现动画");
        // Query change while open (different candidates): still no restart.
        let c = comp("status", 0);
        let _ = popup_age_for(&mut last, &mut start, Some(&c), std::time::Instant::now());
        assert_eq!(start, saved, "开着改查询不重播");
        // Finished (started 500ms ago): back to steady with the start cleared.
        let old = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_millis(500))
            .unwrap();
        start = Some(old);
        assert_eq!(
            popup_age_for(&mut last, &mut start, Some(&c), std::time::Instant::now()),
            None
        );
        assert_eq!(start, None);
        // Query change after finishing: still steady (no replay).
        let c = comp("stop", 0);
        assert_eq!(
            popup_age_for(&mut last, &mut start, Some(&c), std::time::Instant::now()),
            None
        );
        assert_eq!(start, None);
        // Popup closed: steady.
        assert_eq!(
            popup_age_for(&mut last, &mut start, None, std::time::Instant::now()),
            None
        );
        // Closed then reopened: replays.
        let c = comp("stop", 0);
        assert!(
            popup_age_for(&mut last, &mut start, Some(&c), std::time::Instant::now())
                .is_some_and(|a| a < crate::render::POPUP_APPEAR_MS)
        );
        assert_eq!(last, Some(vec!["stop".to_string()]));
    }

    #[test]
    fn popup_age_second_call_in_frame_never_flashes_steady() {
        use crate::completion::CompletionItem;
        use crate::editor::CompletionState;
        // One `/` keypress must pop only once. Frame order is: loop top (pre-event, no popup),
        // then the key opens the popup, then pre-render chase. The chase must play from 0 directly,
        // never show one steady (None) full frame first and then regrow.
        let t = std::time::Instant::now();
        let mut last: Option<Vec<String>> = None;
        let mut start: Option<std::time::Instant> = None;
        // Loop top: no popup yet.
        assert_eq!(popup_age_for(&mut last, &mut start, None, t), None);
        // The same frame opens the popup via the event; the pre-render chase plays from 0 (Some(0), 0 rows).
        let c = CompletionState {
            items: vec![CompletionItem::new("stop", "stop", "", false)],
            selected: 0,
        };
        let age = popup_age_for(&mut last, &mut start, Some(&c), t);
        assert_eq!(age, Some(0), "追算从 0 播，不闪稳态");
        assert_eq!(
            crate::render::animated_popup_count(age, 1),
            0,
            "首帧有效 0 行"
        );
    }

    #[test]
    fn off_mode_has_no_input() {
        let driver = spawn_with_mode(ConsoleConfig::default(), ResolvedMode::Off, false).unwrap();
        assert_eq!(driver.mode(), ResolvedMode::Off);
        assert!(driver.try_recv_line().is_none());
    }

    #[test]
    fn online_players_roundtrip() {
        let driver = spawn_with_mode(ConsoleConfig::default(), ResolvedMode::Off, false).unwrap();
        let handle = driver.handle();
        assert!(handle.online_players().is_empty(), "初始空列表");
        handle.set_online_players(vec!["Steve".to_string(), "Alex".to_string()]);
        assert_eq!(
            handle.online_players(),
            vec!["Steve".to_string(), "Alex".to_string()]
        );
        // Repeated pushes of identical content stay harmless without growing.
        handle.set_online_players(vec!["Steve".to_string(), "Alex".to_string()]);
        assert_eq!(handle.online_players().len(), 2);
        handle.set_online_players(Vec::new());
        assert!(handle.online_players().is_empty(), "全下线清空");
    }

    #[test]
    fn welcome_gate_releases_only_when_no_welcome() {
        // Off / Plain / config-disabled: open from construction, the host never waits.
        let off = spawn_with_mode(ConsoleConfig::default(), ResolvedMode::Off, false).unwrap();
        assert!(off.handle().welcome_done());
        let plain = spawn_with_mode(ConsoleConfig::default(), ResolvedMode::Plain, false).unwrap();
        assert!(plain.handle().welcome_done());
        let no_welcome = spawn_with_mode(
            ConsoleConfig {
                welcome: false,
                ..Default::default()
            },
            ResolvedMode::Gui,
            true,
        )
        .unwrap();
        assert!(no_welcome.handle().welcome_done(), "欢迎屏关闭即放行");
        // GUI plus welcome enabled: not released at construction (waits for a key; no TTY in tests,
        // so terminal takeover fails and the thread exits, but the gate flag stays stable for assertions).
        let gated = spawn_with_mode(
            ConsoleConfig {
                welcome: true,
                ..Default::default()
            },
            ResolvedMode::Gui,
            true,
        )
        .unwrap();
        assert!(!gated.handle().welcome_done(), "欢迎屏开启时等待按键");
    }

    #[test]
    fn handle_push_without_consumer_counts_drop() {
        let driver = spawn_with_mode(
            ConsoleConfig {
                log_queue: 16,
                ..Default::default()
            },
            ResolvedMode::Off,
            false,
        )
        .unwrap();
        // Off mode has no consumer thread: sends fill up immediately (cap>=16, fill it; only checks non-blocking).
        for _ in 0..32 {
            driver.handle().push_info("t", "x");
        }
        // try_send never blocks (reaching here proves it).
        assert!(driver.handle().dropped_logs() > 0 || driver.handle().dropped_logs() == 0);
    }

    #[test]
    fn cpu_normalizes_to_whole_machine_and_never_exceeds_100() {
        // On 12 cores, raw 200% (two cores busy) normalizes to 16.7%, never above 100%.
        assert!((normalize_cpu_pct(199.5, 12.0) - 16.625).abs() < 0.01);
        assert_eq!(normalize_cpu_pct(0.0, 12.0), 0.0);
        // Huge raw values (all 12 cores busy at 1200%) clamp to 100%.
        assert_eq!(normalize_cpu_pct(1200.0, 12.0), 100.0);
        // Negative values and zero cores clamp without panic or NaN.
        assert_eq!(normalize_cpu_pct(-5.0, 12.0), 0.0);
        assert!((normalize_cpu_pct(50.0, 0.0) - 50.0).abs() < 0.001);
        // A single-core machine keeps the raw value.
        assert!((normalize_cpu_pct(80.0, 1.0) - 80.0).abs() < 0.001);
    }

    #[test]
    fn sampler_caches_cpu_count_at_least_one() {
        let s = ProcSampler::new();
        assert!(s.num_cpus >= 1.0, "逻辑 CPU 数至少为 1，raw/0 不会 NaN");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_footprint_matches_activity_monitor_caliber() {
        // Footprint should be available, nonzero, and no larger than RSS (RSS includes shared mappings).
        let fp = macos_footprint_bytes();
        assert!(fp.is_some(), "proc_pid_rusage 应成功");
        let fp = fp.unwrap();
        assert!(fp > 0, "footprint 非零");
        let pid = sysinfo::Pid::from_u32(std::process::id());
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        if let Some(p) = sys.process(pid) {
            assert!(
                fp <= p.memory(),
                "footprint({fp}) 应 ≤ RSS({})，否则与活动监视器口径矛盾",
                p.memory()
            );
        }
    }
}
