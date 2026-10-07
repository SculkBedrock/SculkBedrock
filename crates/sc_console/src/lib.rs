//! `sc_console`: standalone command-line console.
//!
//! # Role
//!
//! This crate is a **pure terminal UI layer** and deliberately depends on no game crate
//! (it references none of `sc_ecs` / `sc_command` / `sc_game` / `sc_world` / `sc_network`).
//! It does exactly three things:
//!
//! 1. Draw a flat minimal console TUI on a dedicated thread (version bar + log stream + completion list + input bar + status line);
//! 2. Expose command completion through the [`CompletionProvider`] trait (caller hot-swaps the implementation);
//! 3. Exchange data with the host over **bounded channels**: logs in, commands out, never blocking the game thread.
//!
//! # Architecture boundaries
//!
//! - **Workers prepare; owners publish**: the console thread only does IO and rendering,
//!   user input travels to the host as owned `String`s over a bounded `sync_channel`;
//!   the host drains them on its own tick (owner context) and publishes authoritative command events.
//!   The console thread never touches game state.
//! - **Every queue needs limits**: log/input channels, history, log window,
//!   and completion list are all bounded; overload drops with a visible counter, never blocking or growing unboundedly.
//! - **Game/terminal boundary**: this crate knows no command semantics; the host pushes completion data
//!   as snapshots (see [`StaticCompletionProvider`]), decoupled from the registry.
//!
//! # Quick start
//!
//! ```no_run
//! use sc_console::{spawn_console, ConsoleConfig};
//!
//! let driver = spawn_console(ConsoleConfig::from_env()).expect("console");
//! let handle = driver.handle();
//! // Push logs (usually done by the logger adapter):
//! handle.push_info("server", "hello console");
//! // Drain input in the main loop / game tick:
//! while let Some(line) = driver.try_recv_line() {
//!     println!("user typed: {line}");
//! }
//! // Restore the terminal before exit:
//! driver.shutdown();
//! ```

pub mod ansi;
pub mod clipboard;
pub mod completion;
pub mod config;
pub mod driver;
pub mod editor;
pub mod logger;
pub mod model;
mod render;
mod welcome;

// Console chrome i18n (own `locales/` table, `tui.*` keys; never touch the
// process-global locale — every lookup passes an explicit `locale =`, so
// server log language is unaffected).
rust_i18n::i18n!("locales", fallback = "en-US");

pub use ansi::{nearest_256, Rgb};
pub use completion::{
    common_prefix, current_token, shared_provider, trigger_at, CommandEntry, CompletionItem,
    CompletionProvider, NoopCompletionProvider, StaticCompletionProvider,
};
pub use config::{ConsoleConfig, ConsoleMode, ResolvedMode};
pub use driver::{spawn_console, spawn_console_hosted, ConsoleDriver, ConsoleHandle, WelcomeChoice};
pub use logger::{install_logger, ExtraSink};
pub use model::{ConsoleHeader, ConsoleStats, LogLevel, LogLine, ServerPhase};

// Quote typewriter (`Typewriter` / `TwFrame`) is defined below: the state machine is independent of terminal IO,
// easy to drive in unit tests (delete old, type new, shine; the right `」` bracket stays pinned).

/// Milliseconds per typed char (quote reveal speed).
pub const TYPE_MS_PER_CHAR: u64 = 80;
/// Milliseconds per deleted char (twice as fast as typing; switches delete before typing).
pub const DELETE_MS_PER_CHAR: u64 = 40;
/// Shine duration after typing completes (ms).
pub const SHINE_MS: u64 = 600;

/// Quote typewriter: on version change delete the old text before typing the new one, then shine; the right bracket stays pinned.
///
/// ```text
/// New version arrives -> delete old text (right to left, `」` pinned) -> type new text (left to right, `」` pinned)
///            -> shine (left to right) -> steady
/// ```
///
/// Pure state machine plus explicit clock (caller ticks [`Typewriter::update`] per frame), unit-testable;
/// the driver only passes `(full text, version, now ms)` and repaints per the return value.
#[derive(Clone, Debug)]
pub struct Typewriter {
    text: String,
    ver: u64,
    phase: TwPhase,
    t0_ms: u64,
    /// Old full-text snapshot used during the delete phase.
    old: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TwPhase {
    Steady,
    Deleting,
    Typing,
    Shining,
}

/// Display frame: text plus whether animation continues plus shine progress (`None` means no shine).
#[derive(Clone, Debug)]
pub struct TwFrame {
    pub text: String,
    pub animating: bool,
    pub shine: Option<f32>,
}

impl Default for Typewriter {
    fn default() -> Self {
        Self::new()
    }
}

impl Typewriter {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            // Sentinel: guarantees the first update is recognized as a new version.
            ver: u64::MAX,
            phase: TwPhase::Steady,
            t0_ms: 0,
            old: String::new(),
        }
    }

    /// Split off the fixed right bracket: returns (body, `」`) when ending with `」`, else (full text, `""`).
    ///
    /// Delete/type only touch the body; the right bracket stays pinned at the line end.
    fn split_closer(full: &str) -> (&str, &str) {
        if full.ends_with('」') {
            let cut = full.len() - '」'.len_utf8();
            (&full[..cut], &full[cut..])
        } else {
            (full, "")
        }
    }

    /// Take the first n body chars plus the right bracket (CJK safe).
    fn partial(full: &str, n: usize) -> String {
        let (body, closer) = Self::split_closer(full);
        let kept: String = body.chars().take(n).collect();
        format!("{kept}{closer}")
    }

    fn body_len(full: &str) -> usize {
        Self::split_closer(full).0.chars().count()
    }

    /// Advance one frame. Returns the display frame; the caller keeps repainting while `animating` is true.
    pub fn update(&mut self, full: &str, ver: u64, now_ms: u64) -> TwFrame {
        if ver != self.ver {
            self.old = std::mem::replace(&mut self.text, full.to_string());
            self.ver = ver;
            self.t0_ms = now_ms;
            self.phase = if Self::body_len(&self.text) == 0 {
                // Empty new text: steady immediately, no delete/type/shine.
                TwPhase::Steady
            } else if Self::body_len(&self.old) == 0 {
                // Empty old text types directly, otherwise delete before typing.
                TwPhase::Typing
            } else {
                TwPhase::Deleting
            };
        }
        loop {
            let elapsed = now_ms.saturating_sub(self.t0_ms);
            match self.phase {
                TwPhase::Steady => {
                    return TwFrame {
                        text: self.text.clone(),
                        animating: false,
                        shine: None,
                    };
                }
                TwPhase::Deleting => {
                    let total = Self::body_len(&self.old);
                    let gone = (elapsed / DELETE_MS_PER_CHAR.max(1)) as usize;
                    if gone >= total {
                        self.phase = TwPhase::Typing;
                        self.t0_ms = now_ms;
                        continue;
                    }
                    return TwFrame {
                        text: Self::partial(&self.old, total - gone),
                        animating: true,
                        shine: None,
                    };
                }
                TwPhase::Typing => {
                    let total = Self::body_len(&self.text);
                    if total == 0 {
                        self.phase = TwPhase::Shining;
                        self.t0_ms = now_ms;
                        continue;
                    }
                    // First char appears immediately to avoid a blank first frame.
                    let n = ((elapsed / TYPE_MS_PER_CHAR.max(1)) as usize + 1).min(total);
                    if n >= total {
                        self.phase = TwPhase::Shining;
                        self.t0_ms = now_ms;
                        continue;
                    }
                    return TwFrame {
                        text: Self::partial(&self.text, n),
                        animating: true,
                        shine: None,
                    };
                }
                TwPhase::Shining => {
                    let t = elapsed as f32 / SHINE_MS as f32;
                    if t >= 1.0 {
                        self.phase = TwPhase::Steady;
                        continue;
                    }
                    return TwFrame {
                        text: self.text.clone(),
                        animating: true,
                        shine: Some(t.min(1.0)),
                    };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Typewriter, DELETE_MS_PER_CHAR, SHINE_MS, TYPE_MS_PER_CHAR};

    fn step(tw: &mut Typewriter, full: &str, ver: u64, now: u64) -> (String, bool, Option<f32>) {
        let f = tw.update(full, ver, now);
        (f.text, f.animating, f.shine)
    }

    /// Step at 10ms increments until steady; returns the steady timestamp (for asserting later phases).
    fn settle(tw: &mut Typewriter, full: &str, ver: u64, mut now: u64) -> u64 {
        loop {
            let (_, anim, _) = step(tw, full, ver, now);
            if !anim {
                return now;
            }
            now += 10;
            assert!(now < 1_000_000, "动画不应无限持续");
        }
    }

    #[test]
    fn types_char_by_char_with_pinned_closer() {
        // Body is `「abc` (left bracket participates in typing); right bracket pinned.
        let mut tw = Typewriter::new();
        // First char appears immediately to avoid a blank first frame.
        assert_eq!(
            step(&mut tw, "「abc」", 1, 0),
            ("「」".to_string(), true, None)
        );
        // Type the body char by char; the right bracket stays at the line end.
        assert_eq!(
            step(&mut tw, "「abc」", 1, TYPE_MS_PER_CHAR),
            ("「a」".to_string(), true, None)
        );
        assert_eq!(
            step(&mut tw, "「abc」", 1, TYPE_MS_PER_CHAR * 2),
            ("「ab」".to_string(), true, None)
        );
        // The moment typing finishes, shining starts (full text visible).
        let (s, anim, shine) = step(&mut tw, "「abc」", 1, TYPE_MS_PER_CHAR * 3);
        assert_eq!(s, "「abc」");
        assert!(anim);
        assert_eq!(shine, Some(0.0));
    }

    #[test]
    fn shine_runs_after_typing_then_settles() {
        let mut tw = Typewriter::new();
        // Body `「ab` has 3 chars, so typing ends at 2xTYPE_MS.
        let done_at = TYPE_MS_PER_CHAR * 2;
        assert_eq!(step(&mut tw, "「ab」", 1, 0).0, "「」");
        // Shine progress starts at 0 (the moment typing completes).
        let (s, anim, shine) = step(&mut tw, "「ab」", 1, done_at);
        assert_eq!(s, "「ab」");
        assert!(anim);
        assert_eq!(shine, Some(0.0));
        // Shine shows progress mid-flight.
        let (_, _, shine) = step(&mut tw, "「ab」", 1, done_at + SHINE_MS / 2);
        assert!(
            matches!(shine, Some(p) if p > 0.0 && p < 1.0),
            "扫光中途有进度"
        );
        // Steady once shine completes (never replayed).
        let end = settle(&mut tw, "「ab」", 1, done_at);
        assert!(end <= done_at + SHINE_MS + 20, "扫光时长有界");
        let (s, anim, shine) = step(&mut tw, "「ab」", 1, end + 5_000);
        assert_eq!((s, anim, shine), ("「ab」".to_string(), false, None));
    }

    #[test]
    fn switch_deletes_before_typing_keeping_closer() {
        let mut tw = Typewriter::new();
        let t0 = settle(&mut tw, "「ab」", 1, 0);
        // New version: delete the old text first (right to left); right bracket stays pinned.
        // Old body `「ab` has 3 chars.
        assert_eq!(
            step(&mut tw, "「xy」", 2, t0),
            ("「ab」".to_string(), true, None)
        );
        assert_eq!(
            step(&mut tw, "「xy」", 2, t0 + DELETE_MS_PER_CHAR),
            ("「a」".to_string(), true, None)
        );
        assert_eq!(
            step(&mut tw, "「xy」", 2, t0 + DELETE_MS_PER_CHAR * 2),
            ("「」".to_string(), true, None)
        );
        // Type the new first char (left bracket) right after deletion empties the line.
        let t1 = t0 + DELETE_MS_PER_CHAR * 3;
        assert_eq!(
            step(&mut tw, "「xy」", 2, t1),
            ("「」".to_string(), true, None)
        );
        assert_eq!(
            step(&mut tw, "「xy」", 2, t1 + TYPE_MS_PER_CHAR),
            ("「x」".to_string(), true, None)
        );
        // Finally converges to the new steady text.
        let t2 = settle(&mut tw, "「xy」", 2, t1);
        assert_eq!(step(&mut tw, "「xy」", 2, t2).0, "「xy」");
    }

    #[test]
    fn cjk_delete_is_per_char_and_pinned() {
        let mut tw = Typewriter::new();
        let t0 = settle(&mut tw, "「山野清风」", 1, 0);
        assert_eq!(step(&mut tw, "「x」", 2, t0).0, "「山野清风」");
        assert_eq!(
            step(&mut tw, "「x」", 2, t0 + DELETE_MS_PER_CHAR * 2).0,
            "「山野」"
        );
        assert_eq!(
            step(&mut tw, "「x」", 2, t0 + DELETE_MS_PER_CHAR * 3).0,
            "「山」"
        );
    }

    #[test]
    fn no_closer_text_cycles_plain() {
        let mut tw = Typewriter::new();
        assert_eq!(step(&mut tw, "hi", 1, 0).0, "h");
        let t0 = settle(&mut tw, "hi", 1, 0);
        assert_eq!(step(&mut tw, "yo", 2, t0).0, "hi");
        assert_eq!(step(&mut tw, "yo", 2, t0 + DELETE_MS_PER_CHAR).0, "h");
    }

    #[test]
    fn same_version_never_replays() {
        let mut tw = Typewriter::new();
        let t0 = settle(&mut tw, "「ab」", 7, 0);
        let (s, anim, shine) = step(&mut tw, "「ab」", 7, t0 + 10_000);
        assert_eq!((s, anim, shine), ("「ab」".to_string(), false, None));
    }

    #[test]
    fn empty_text_completes_immediately() {
        let mut tw = Typewriter::new();
        assert_eq!(step(&mut tw, "", 1, 0), (String::new(), false, None));
    }
}
