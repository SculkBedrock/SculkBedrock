//! `log` adapter: forwards global logs to both the console TUI and an extra sink callback.
//!
//! Installed in GUI mode only; Plain/Off modes keep the host logger (e.g.
//! `pretty_env_logger`) untouched by this module.
//!
//! Backpressure: pushes to the TUI use `try_send` (drops and counts when full);
//! extra callbacks (e.g. file sinks) are host-provided and must also be **non-blocking**.

use std::sync::Arc;

use crate::driver::ConsoleHandle;
use crate::model::LogLine;

/// Extra sink callback: `(level, target, plain_message)`; must be non-blocking.
pub type ExtraSink = Arc<dyn Fn(log::Level, &str, &str) + Send + Sync>;

struct ConsoleLogger {
    handle: ConsoleHandle,
    extra: Option<ExtraSink>,
}

impl log::Log for ConsoleLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Debug
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
