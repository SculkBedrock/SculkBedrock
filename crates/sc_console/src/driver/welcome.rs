//! 控制台驱动：欢迎屏选择器与等待（由 `driver.rs` 拆出；在 `crate::driver` 重新导出）。
//!
//! Console driver: welcome-screen selector and wait (split from `driver.rs`; re-exported at `crate::driver`).

use super::*;

/// Welcome screen target frame interval (ms, about 30fps): fullscreen truecolor
/// repaints saturate software terminals at 60fps.
pub(super) const WELCOME_FRAME_MS: u64 = 33;

/// Next frame interval: at least `budget`, and never shorter than the last write cost
/// (a slow terminal lowers the frame rate instead of starving event polling).
pub(crate) fn next_frame_interval(budget: Duration, last_write: Duration) -> Duration {
    budget.max(last_write)
}

/// 诊断用欢迎屏按键延迟轨迹（`SC_CONSOLE_WELCOME_TRACE=<path>`）。
///
/// 记录轮询等待/唤醒、drain 结果与绘制耗时，用于定位退出延迟。
/// 未设置时零开销。
///
/// Diagnostic welcome-screen key latency trace (`SC_CONSOLE_WELCOME_TRACE=<path>`).
///
/// Records poll wait/wakeup, drain results, and paint cost to locate exit delays.
/// Zero cost when unset.
pub(super) struct WelcomeTrace {
    f: Option<std::io::BufWriter<std::fs::File>>,
    t0: std::time::Instant,
}

impl WelcomeTrace {
    fn open(t0: std::time::Instant) -> Self {
        let f = std::env::var_os("SC_CONSOLE_WELCOME_TRACE").and_then(|p| {
            std::fs::File::create(p).ok().map(std::io::BufWriter::new)
        });
        Self { f, t0 }
    }

    fn ms(&self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }

    fn log(&mut self, msg: &str) {
        let t = self.ms();
        if let Some(f) = self.f.as_mut() {
            use std::io::Write as _;
            let _ = writeln!(f, "t={t} {msg}");
            let _ = f.flush(); // flush per line so the log survives a crash
        }
    }
}

/// Welcome-screen exit condition.
///
/// `dismissed` is the first-key time in ms; when this returns true the caller stops waiting
/// without painting again.
pub(crate) fn welcome_should_exit(age_ms: u64, dismissed_ms: Option<u64>) -> bool {
    match dismissed_ms {
        Some(d) => age_ms.saturating_sub(d) >= crate::welcome::WELCOME_EXIT_MS,
        None => false,
    }
}

/// 交给宿主的欢迎屏选择结果（一次性）。
///
/// Welcome-screen choice handed to the host (one-shot).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WelcomeChoice {
    /// Canonical locale code (`en-US` / `zh-CN`).
    pub lang: String,
    /// Daily-quote subtitle enabled (selector checkbox, Chinese only —
    /// confirming English forces this off).
    pub hitokoto: bool,
}

/// Selector navigation with wraparound.
pub(crate) fn select_move(selected: usize, delta: i32, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    (selected as i32 + delta).rem_euclid(count as i32) as usize
}

/// 选择器焦点：语言列表，或每日一句勾选行。
///
/// Selector focus: the language list, or the daily-quote checkbox row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectFocus {
    List,
    Quote,
}

/// 语言选择器的交互状态。
///
/// 勾选框只在高亮中文选项时存在；默认关闭，确认英文时强制关闭。
///
/// Language-selector interaction state.
///
/// The quote checkbox exists only while a Chinese option is highlighted;
/// it defaults off and English confirms force it off.
#[derive(Debug)]
pub(crate) struct SelectMenu {
    pub selected: usize,
    pub focus: SelectFocus,
    pub hitokoto: bool,
}

/// 选择器按键结果。
///
/// Selector key outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectAction {
    Confirm,
    Back,
    Updated,
}

impl SelectMenu {
    pub(crate) fn new(selected: usize) -> Self {
        Self {
            selected,
            focus: SelectFocus::List,
            hitokoto: false,
        }
    }

    /// Vertical step shared by arrows and `k/j` (`delta` −1 up, +1 down).
    ///
    /// Vertical keys leave the checkbox (focus back to the list, highlight
    /// stays); down through a highlighted Chinese option parks focus on the
    /// checkbox. Horizontal travel lives in [`SelectMenu::key`] (`←→`).
    fn step(&mut self, delta: i32, count: usize, zh_index: Option<usize>) {
        if self.focus == SelectFocus::Quote {
            self.focus = SelectFocus::List;
        } else if delta < 0 {
            self.selected = select_move(self.selected, -1, count);
        } else if zh_index.is_some_and(|z| self.selected == z) {
            self.focus = SelectFocus::Quote;
        } else {
            self.selected = select_move(self.selected, 1, count);
        }
    }

    /// Handle one key (`zh_index`: position of zh-CN in the order, `None` =
    /// no Chinese option, so no checkbox). `Ctrl+C` never reaches here (the
    /// caller dismisses first).
    pub(crate) fn key(
        &mut self,
        count: usize,
        zh_index: Option<usize>,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> SelectAction {
        match code {
            // Primary button acts contextually: on the checkbox it checks the
            // box, on the language list it confirms the whole selector.
            KeyCode::Enter => {
                if self.focus == SelectFocus::Quote {
                    self.hitokoto = !self.hitokoto;
                    SelectAction::Updated
                } else {
                    SelectAction::Confirm
                }
            }
            KeyCode::Esc => SelectAction::Back,
            KeyCode::Char(' ') if modifiers.is_empty() => {
                if self.focus == SelectFocus::Quote {
                    self.hitokoto = !self.hitokoto;
                    SelectAction::Updated
                } else {
                    SelectAction::Confirm
                }
            }
            KeyCode::Up => {
                self.step(-1, count, zh_index);
                SelectAction::Updated
            }
            KeyCode::Down => {
                self.step(1, count, zh_index);
                SelectAction::Updated
            }
            KeyCode::Left | KeyCode::Right => {
                // Directional travel between the boxes (quote box sits right):
                // Right enters it from a highlighted Chinese option, Left
                // returns; anywhere else these keys are a no-op.
                if self.focus == SelectFocus::Quote {
                    if code == KeyCode::Left {
                        self.focus = SelectFocus::List;
                    }
                } else if code == KeyCode::Right
                    && zh_index.is_some_and(|z| self.selected == z)
                {
                    self.focus = SelectFocus::Quote;
                }
                SelectAction::Updated
            }
            KeyCode::Char(c) if c.eq_ignore_ascii_case(&'k') => {
                self.step(-1, count, zh_index);
                SelectAction::Updated
            }
            KeyCode::Char(c) if c.eq_ignore_ascii_case(&'j') => {
                self.step(1, count, zh_index);
                SelectAction::Updated
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let idx = (c as usize).saturating_sub('1' as usize);
                if idx < count {
                    self.selected = idx;
                    self.focus = SelectFocus::List;
                }
                SelectAction::Updated
            }
            _ => SelectAction::Updated,
        }
    }
}

/// Effective quote flag on confirm: the checkbox counts only for Chinese
/// (confirming English forces it off).
pub(crate) fn effective_hitokoto(menu: &SelectMenu, zh_index: Option<usize>) -> bool {
    zh_index.is_some_and(|z| menu.selected == z) && menu.hitokoto
}

/// `Ctrl+C` enters the console directly in any welcome stage (never saves a choice).
pub(crate) fn is_ctrl_c(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('c' | 'C'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// Welcome wait (on GUI startup): repaints flowing effects toward 30fps.
///
/// Stages (first run: prompt, then the language selector; returning runs skip both):
/// first run shows logo, blobs and press-any-key; any key opens the selector
/// (`Ctrl+C` enters directly instead, choosing nothing). The selector lists
/// options on the left with blobs gliding right: `↑↓/k/j` move (wrapping),
/// `1-9` jump, `←→` travels between the language list and the right-hand
/// daily-quote checkbox (Chinese only; `Space`/`Enter` on the box checks it,
/// `Enter`/`Space` on the list confirms and plays the exit flight),
/// `Esc` backs out to the prompt. A returning run (choice already saved) shows logo and
/// blobs only and auto-dismisses after `WELCOME_AUTO_DISMISS_MS` into the
/// exit flight; any key skips the hold.
///
/// After the choice (or with an env/config override) the exit flight plays,
/// then the console boots; a second key during the flight skips it.
/// The loop drains events with a bounded per-frame cap, keeps mouse capture off,
/// and lowers the frame rate on slow terminals. Returns the session language.
pub(super) fn welcome_wait(
    handle: &ConsoleHandle,
    forced_lang: Option<String>,
    initial_lang: String,
) -> String {
    use std::io::Write as _;
    use crate::config::{
        create_chosen_marker, has_chosen_marker, normalize_lang, selector_order, MARKER_FILE_NAME,
    };
    use crate::welcome::{WelcomeView, WELCOME_AUTO_DISMISS_MS};
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b[?25l");
    let _ = out.flush();
    let t0 = std::time::Instant::now();
    let budget = Duration::from_millis(WELCOME_FRAME_MS);
    let mut dismissed_ms: Option<u64> = None;
    // Session language: override wins, else the host-provided initial
    // (`server_properties.toml`), else the default. Updated on selector confirm.
    let forced = forced_lang.and_then(|s| normalize_lang(&s));
    let mut lang = forced.clone().unwrap_or(initial_lang);
    // Selector state: `None` = prompt stage. The marker means a past run chose.
    let chosen_before = has_chosen_marker();
    let auto_mode = chosen_before;
    let mut selecting: Option<u64> = None;
    let order = selector_order();
    let zh_index = order.iter().position(|c| c == crate::config::LANG_ZH_CN);
    let mut menu = SelectMenu::new(
        order.iter().position(|c| c == &lang).unwrap_or(0),
    );
    // Next frame time; paints immediately, pushes back when writes are slow (never catches up).
    let mut next_paint = std::time::Instant::now();
    let mut trace = WelcomeTrace::open(t0);
    trace.log("TEMP-DIAG welcome_wait start");
    loop {
        if !handle.is_running() {
            trace.log("TEMP-DIAG not running, break");
            break;
        }
        let age = t0.elapsed().as_millis() as u64;
        // Exit animation done: hand over without painting (avoids one extra black flash).
        if welcome_should_exit(age, dismissed_ms) {
            trace.log(&format!(
                "TEMP-DIAG exit anim done age={age} dismissed={dismissed_ms:?}"
            ));
            break;
        }
        // Returning run: auto-dismiss after the hold (any key below skips it sooner).
        if auto_mode && dismissed_ms.is_none() && age >= WELCOME_AUTO_DISMISS_MS {
            dismissed_ms = Some(age);
            trace.log(&format!("TEMP-DIAG auto dismiss age={age}"));
        }
        // Render view for this frame.
        let view = if auto_mode {
            WelcomeView::auto()
        } else if let Some(enter) = selecting {
            let count = order.len().max(1);
            WelcomeView::select(
                menu.selected % count,
                enter,
                menu.focus == SelectFocus::Quote,
                menu.hitokoto,
            )
        } else {
            WelcomeView::prompt()
        };
        // Wait for the next frame time or the next event, whichever comes first.
        let wait = next_paint.saturating_duration_since(std::time::Instant::now());
        if event::poll(wait).unwrap_or(false) {
            // Bounded collect (resize/mouse ignored here, repaint covers them).
            let batch = drain_events(|| match event::poll(std::time::Duration::ZERO) {
                Ok(false) => None,
                Ok(true) => event::read().ok(),
                Err(_) => None,
            });
            let keys = batch.keys;
            trace.log(&format!(
                "TEMP-DIAG event wait={}ms n={} dismissed={dismissed_ms:?} selecting={selecting:?}",
                wait.as_millis(),
                keys.len(),
            ));
            let mut skip_exit = false;
            for key in &keys {
                // A key while already dismissed: skip the exit flight at once.
                if dismissed_ms.is_some() {
                    trace.log("TEMP-DIAG second key, skip exit");
                    skip_exit = true;
                    break;
                }
                if is_ctrl_c(key) {
                    // Enter directly, choosing nothing (existing contract).
                    dismissed_ms = Some(t0.elapsed().as_millis() as u64);
                    selecting = None;
                    trace.log(&format!(
                        "TEMP-DIAG ctrl-c dismissed={dismissed_ms:?}"
                    ));
                    continue;
                }
                if auto_mode {
                    // Skip the remaining hold, play the flight now.
                    dismissed_ms = Some(t0.elapsed().as_millis() as u64);
                    continue;
                }
                if selecting.is_some() {
                    if order.is_empty() {
                        dismissed_ms = Some(t0.elapsed().as_millis() as u64);
                        continue;
                    }
                    let count = order.len();
                    match menu.key(count, zh_index, key.code, key.modifiers) {
                        SelectAction::Confirm => {
                            let code = order[menu.selected % count].clone();
                            let quote_on = effective_hitokoto(&menu, zh_index);
                            // Marker first (pure flag, no id inside): later runs
                            // auto-enter. The choice itself travels to the host
                            // via the handle for `server_properties.toml`.
                            if create_chosen_marker().is_err() {
                                log::warn!(
                                    "[sc_console] cannot record language choice ({}), continuing unsaved",
                                    MARKER_FILE_NAME,
                                );
                            }
                            handle.set_welcome_choice(WelcomeChoice {
                                lang: code.clone(),
                                hitokoto: quote_on,
                            });
                            lang = code;
                            dismissed_ms = Some(t0.elapsed().as_millis() as u64);
                            trace.log(&format!(
                                "TEMP-DIAG choice lang={lang} hitokoto={quote_on} dismissed={dismissed_ms:?}"
                            ));
                        }
                        SelectAction::Back => {
                            selecting = None; // back out to the prompt
                            menu = SelectMenu::new(menu.selected);
                        }
                        SelectAction::Updated => {
                            // Repaint at once so highlight/toggle feel instant.
                            next_paint = std::time::Instant::now();
                        }
                    }
                    continue;
                }
                // Prompt stage, plain key: open the selector — unless already
                // chosen (marker) or overridden, then dismiss straight away.
                if !chosen_before && forced.is_none() {
                    selecting = Some(t0.elapsed().as_millis() as u64);
                    menu.focus = SelectFocus::List;
                    menu.hitokoto = false;
                    trace.log(&format!(
                        "TEMP-DIAG selector opened selecting={selecting:?}"
                    ));
                } else {
                    dismissed_ms = Some(t0.elapsed().as_millis() as u64);
                }
            }
            if skip_exit {
                break;
            }
            // Dismissal starts the flight at once (first flight frame paints now,
            // not at the next scheduled tick).
            if dismissed_ms.is_some() {
                next_paint = std::time::Instant::now();
            }
        }
        // Not at frame time yet (woken early by a non-key event): keep waiting without painting.
        if std::time::Instant::now() < next_paint {
            continue;
        }
        let (w, h) = size().unwrap_or((100, 30));
        let age = t0.elapsed().as_millis() as u64;
        // Check exit once more before painting: skip the fully black final frame and hand over directly.
        if welcome_should_exit(age, dismissed_ms) {
            trace.log(&format!(
                "TEMP-DIAG exit anim done before paint age={age} {w}x{h}"
            ));
            break;
        }
        let paint_t0 = std::time::Instant::now();
        let drew = crate::welcome::paint_welcome(
            &mut out,
            w,
            h,
            age,
            dismissed_ms,
            crate::ansi::style_enabled(),
            view,
        )
        .is_ok();
        let spent = paint_t0.elapsed();
        // Trace slow frames and the exit phase only; avoids spamming on normal frames.
        if spent > Duration::from_millis(50) || dismissed_ms.is_some() {
            trace.log(&format!(
                "TEMP-DIAG paint age={age} {w}x{h} spent={}ms dismissed={dismissed_ms:?}",
                spent.as_millis(),
            ));
        }
        next_paint = std::time::Instant::now() + next_frame_interval(budget, spent);
        if !drew {
            trace.log("TEMP-DIAG paint failed, break");
            break;
        }
    }
    // Release the host when welcome ends (key entry or exit request; never block loading).
    handle
        .inner
        .welcome_dismissed
        .store(true, Ordering::Relaxed);
    lang
}
