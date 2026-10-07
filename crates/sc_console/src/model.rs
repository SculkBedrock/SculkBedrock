//! Console data model: log lines and levels (pure value types, game-agnostic).

use std::fmt::{Display, Formatter};

/// Log level (console-side mapping of `log::Level`; keeps the format layer off `log` display).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<log::Level> for LogLevel {
    fn from(level: log::Level) -> Self {
        match level {
            log::Level::Error => Self::Error,
            log::Level::Warn => Self::Warn,
            log::Level::Info => Self::Info,
            log::Level::Debug => Self::Debug,
            log::Level::Trace => Self::Trace,
        }
    }
}

impl Display for LogLevel {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        };
        f.write_str(s)
    }
}

/// One displayable log line (value object, safe to move across threads).
#[derive(Clone, Debug)]
pub struct LogLine {
    /// `HH:MM:SS` (the render layer only displays it, never parses time).
    pub ts: String,
    pub level: LogLevel,
    pub target: String,
    /// Raw message text (may hold Minecraft `§` codes and ANSI escapes; normalized by the render layer).
    pub message: String,
}

impl LogLine {
    pub fn new(level: LogLevel, target: &str, message: &str) -> Self {
        Self {
            ts: chrono::Local::now().format("%H:%M:%S").to_string(),
            level,
            target: target.to_string(),
            message: message.to_string(),
        }
    }
}

/// Top info-bar snapshot (value object pushed by the host, read-only in the TUI).
///
/// - `core_version`: server core version (e.g. `1.0.0ALPHA(fjord)`);
/// - `pack_label`: pack label (e.g. `Vanilla 1.26.40 * protocol 2168`,
///   used only in the input footnote since the title second line hosts the quote);
/// - `subtitle`: title second-line text (quote from the host API or the built-in fallback).
/// When empty the render layer falls back to the default title; the host may push again once packs load.
#[derive(Clone, Debug, Default)]
pub struct ConsoleHeader {
    pub core_version: String,
    pub pack_label: String,
    pub subtitle: String,
}

impl ConsoleHeader {
    pub fn new(core_version: &str, pack_label: &str) -> Self {
        Self {
            core_version: core_version.to_string(),
            pack_label: pack_label.to_string(),
            subtitle: String::new(),
        }
    }
}

/// Own-process resource snapshot (sampled locally on the console thread, no host involved).
///
/// `has_data=false` means two samples are not ready yet (CPU % needs an interval),
/// and the render layer shows a placeholder.
#[derive(Clone, Debug, Default)]
pub struct ConsoleStats {
    /// Own-process CPU usage (%, normalized to whole-machine 0-100%;
    /// the raw single-core sysinfo figure divided by logical CPU count).
    pub cpu_pct: f32,
    /// Own-process memory usage (MiB; on macOS uses `phys_footprint` matching Activity Monitor,
    /// RSS elsewhere).
    pub mem_mb: f64,
    pub has_data: bool,
}

/// Server boot phase (drives the boot effect and input lock).
///
/// - `Starting`: server still booting: input locked plus blue ripple plus breathing title;
/// - `Sweeping`: boot-success moment: green sweep (~900ms) with input still locked;
/// - `Running`: input accepted normally.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ServerPhase {
    #[default]
    Starting,
    Sweeping,
    Running,
}
