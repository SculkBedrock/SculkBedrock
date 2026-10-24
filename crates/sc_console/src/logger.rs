//! `log` 适配器：把全局日志同时转发到控制台 TUI 与额外的 sink 回调。
//!
//! 仅在 GUI 模式下安装；Plain/Off 模式不会改动宿主日志（例如
//! `pretty_env_logger`）。
//!
//! 背压：向 TUI 推送使用 `try_send`（队列满时丢弃并计数）；
//! 额外回调（如文件 sink）由宿主提供，同样必须**非阻塞**。
//!
//! `log` adapter: forwards global logs to both the console TUI and an extra sink callback.
//!
//! Installed in GUI mode only; Plain/Off modes keep the host logger (e.g.
//! `pretty_env_logger`) untouched by this module.
//!
//! Backpressure: pushes to the TUI use `try_send` (drops and counts when full);
//! extra callbacks (e.g. file sinks) are host-provided and must also be **non-blocking**.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use crate::driver::ConsoleHandle;
use crate::model::LogLine;

/// Console level gate (default Debug).
static CONSOLE_MAX_LEVEL: AtomicU8 = AtomicU8::new(log::LevelFilter::Debug as u8);

/// A record passes when its level is at or below the max.
fn level_allows(level: log::Level, max: log::LevelFilter) -> bool {
    (level as u8) <= (max as u8)
}

/// Current console max as a filter.
fn console_max_filter() -> log::LevelFilter {
    match CONSOLE_MAX_LEVEL.load(Ordering::Relaxed) {
        0 => log::LevelFilter::Off,
        1 => log::LevelFilter::Error,
        2 => log::LevelFilter::Warn,
        3 => log::LevelFilter::Info,
        4 => log::LevelFilter::Debug,
        _ => log::LevelFilter::Trace,
    }
}

/// Apply the configured console level (host calls this at SCStartup after
/// `ServerProperties` loads; harmless when the console logger is absent).
pub fn set_console_max_level(level: log::LevelFilter) {
    CONSOLE_MAX_LEVEL.store(level as u8, Ordering::Relaxed);
}

/// 额外 sink 回调：`(level, target, plain_message)`；必须非阻塞。
///
/// Extra sink callback: `(level, target, plain_message)`; must be non-blocking.
pub type ExtraSink = Arc<dyn Fn(log::Level, &str, &str) + Send + Sync>;

struct ConsoleLogger {
    handle: ConsoleHandle,
    extra: Option<ExtraSink>,
}

impl log::Log for ConsoleLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // The host narrows the gate at SCStartup from `[log] log_level`;
        // Debug default preserves pre-config behavior (incl. existing tests).
        level_allows(metadata.level(), console_max_filter())
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let message = record.args().to_string();
        let plain = crate::ansi::plain_text(&message);
        if let Some(extra) = &self.extra {
            extra(record.level(), record.target(), &plain);
        }
        self.handle.push_log(LogLine::new(
            record.level().into(),
            record.target(),
            &message,
        ));
    }

    fn flush(&self) {}
}

/// Install as the global logger (idempotent: returns Err when a logger exists so the caller falls back).
pub fn install_logger(
    handle: &ConsoleHandle,
    extra: Option<ExtraSink>,
) -> Result<(), log::SetLoggerError> {
    // Global singleton: the logger must be 'static; held via a leaked Box (process-wide singleton, matching
    // the install semantics of pretty_env_logger / Tauri UiLogger).
    let holder: &'static ConsoleLogger = Box::leak(Box::new(ConsoleLogger {
        handle: handle.clone(),
        extra,
    }));
    log::set_logger(holder)?;
    log::set_max_level(log::LevelFilter::Debug);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::level_allows;

    #[test]
    fn console_level_gate_matches_config_values() {
        use log::{Level, LevelFilter};
        assert!(level_allows(Level::Error, LevelFilter::Error));
        assert!(!level_allows(Level::Warn, LevelFilter::Error));
        assert!(level_allows(Level::Info, LevelFilter::Info));
        assert!(!level_allows(Level::Debug, LevelFilter::Info));
        assert!(level_allows(Level::Debug, LevelFilter::Debug));
        assert!(!level_allows(Level::Trace, LevelFilter::Debug));
        assert!(level_allows(Level::Trace, LevelFilter::Trace));
        assert!(!level_allows(Level::Error, LevelFilter::Off));
    }

    #[test]
    fn console_max_level_defaults_to_debug_and_updates() {
        use super::CONSOLE_MAX_LEVEL;
        use std::sync::atomic::Ordering;
        assert_eq!(
            CONSOLE_MAX_LEVEL.load(Ordering::Relaxed),
            log::LevelFilter::Debug as u8
        );
        super::set_console_max_level(log::LevelFilter::Info);
        assert_eq!(
            CONSOLE_MAX_LEVEL.load(Ordering::Relaxed),
            log::LevelFilter::Info as u8
        );
        super::set_console_max_level(log::LevelFilter::Debug);
    }
}
