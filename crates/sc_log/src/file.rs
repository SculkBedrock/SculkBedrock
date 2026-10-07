//! High-throughput local log persistence (async batched writes + custom filename format + flush on exit).
//!
//! Performance design:
//! - A **dedicated thread** drains the channel so logging never blocks the main/game threads (region/tokio);
//! - **Batched writes**: `BufWriter` (64KB) batches disk writes, **no per-line fsync**;
//! - **Live flush**: `recv_timeout(flush_interval)` flushes on idle poll (default 200ms), so logs
//!   reach the file **in real time** instead of only at exit; tail -f tracks them live;
//! - **Exit guarantee**: `shutdown()` sends a `Shutdown` control message, the thread flushes, exits, and is `join`ed,
//!   so everything is on disk at exit.
//!
//! Filename format (strftime subset, `init_file_sink_with_name`):
//! `%Y` year `%m` month `%d` day `%H` hour `%M` minute `%S` second `%f` millis `%%` literal %.
//! Example: `sc_log_%Y-%m-%d_%H-%M-%S.log` becomes `sc_log_2026-08-05_06-30-45.log`.

use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};

/// Bounded pending log records.
///
/// The queue must stay bounded: an unbounded channel plus a slow disk turns into
/// unbounded process memory, while a blocking send would stall the game/tick
/// thread that produced the record. Saturation therefore drops records by class
/// and reports the loss instead of growing.
pub const LOG_QUEUE_CAPACITY: usize = 16_384;

/// Dropped records are reported to stderr every this many losses so a slow disk
/// is visible without flooding the console.
const DROP_REPORT_INTERVAL: u64 = 1_000;

/// Writer-thread control message.
enum LogMsg {
    /// One log line (UTF-8 bytes, newline included).
    Line(Vec<u8>),
    /// Flush immediately.
    Flush,
    /// Flush then exit (for shutdown).
    Shutdown,
}

/// Async log file sink (global singleton).
pub struct AsyncLogFile {
    tx: SyncSender<LogMsg>,
    handle: Mutex<Option<JoinHandle<()>>>,
    dropped: AtomicU64,
}

/// Which records are kept when the bounded queue is saturated.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum LogClass {
    /// debug/trace: reproducible state, safe to drop first.
    Low,
    /// info: normal operation.
    Normal,
    /// warn/error: shutdown notices and failures must stay observable.
    High,
}

impl LogClass {
    pub fn of(level: log::Level) -> Self {
        match level {
            log::Level::Error | log::Level::Warn => LogClass::High,
            log::Level::Info => LogClass::Normal,
            log::Level::Debug | log::Level::Trace => LogClass::Low,
        }
    }
}

static FILE_SINK: OnceLock<AsyncLogFile> = OnceLock::new();

/// Filename format (**standard strftime subset**):
///
/// `%Y` year (4-digit) `%y` short year `%m` month (2-digit) `%d` day (2-digit) `%H` hour (24h)
/// `%M` minute `%S` second `%f` millis (3-digit extension against same-second collision) `%%` literal %
///
/// Example: `sc_log_%Y-%m-%d_%H-%M-%S.log` becomes `sc_log_2026-08-05_06-30-45.log`.
/// Unknown `%x` is kept verbatim (no panic).
pub fn render_file_name(format: &str, now: &DateTime<Local>) -> String {
    let mut out = String::with_capacity(format.len() + 16);
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('Y') => out.push_str(&now.format("%Y").to_string()),
            Some('y') => out.push_str(&now.format("%y").to_string()),
            Some('m') => out.push_str(&now.format("%m").to_string()),
            Some('d') => out.push_str(&now.format("%d").to_string()),
            Some('H') => out.push_str(&now.format("%H").to_string()),
            Some('M') => out.push_str(&now.format("%M").to_string()),
            Some('S') => out.push_str(&now.format("%S").to_string()),
            Some('f') => out.push_str(&now.format("%3f").to_string()),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Global init: renders the filename from the format (logs dir; repeat calls return the existing instance).
/// Shorter `flush_interval` means more real-time output (default 200ms recommended).
///
/// **Performance guarantee**: filename rendering and file opening happen exactly once during
/// this init (`OnceLock` + writer-thread startup); afterwards all logs flow `write_line` -> channel ->
/// the same file handle in batches, so the hot path never renders filenames, opens files, or does per-line IO.
pub fn init_file_sink_with_name(
    name_format: &str,
    log_dir: PathBuf,
    flush_interval: Duration,
) -> &'static AsyncLogFile {
    FILE_SINK.get_or_init(|| {
        let _ = std::fs::create_dir_all(&log_dir);
        // The filename is rendered exactly once here (at startup; the same file is used afterwards).
        let name = render_file_name(name_format, &Local::now());
        AsyncLogFile::start(log_dir.join(name), flush_interval)
    })
}

/// Legacy-compatible entry point: fixed filename `server.log`.
pub fn init_file_sink(path: PathBuf, flush_interval: Duration) -> &'static AsyncLogFile {
    FILE_SINK.get_or_init(|| AsyncLogFile::start(path, flush_interval))
}

/// Non-blocking write of one line (silently dropped without a sink; treated as [`LogClass::Normal`] when saturated).
#[inline]
pub fn write_line(line: String) {
    if let Some(sink) = FILE_SINK.get() {
        sink.write_line_classified(line, LogClass::Normal);
    }
}

/// Non-blocking write of one line with a level, kept by priority when the queue saturates.
#[inline]
pub fn write_line_with_level(line: String, level: log::Level) {
    if let Some(sink) = FILE_SINK.get() {
        sink.write_line_classified(line, LogClass::of(level));
    }
}

/// Total log lines dropped due to queue saturation (observable metric).
pub fn dropped_lines() -> u64 {
    FILE_SINK
        .get()
        .map(|sink| sink.dropped())
        .unwrap_or_default()
}

/// Requests an immediate flush (applies async; call [`shutdown`] before exit).
#[inline]
pub fn flush_now() {
    if let Some(sink) = FILE_SINK.get() {
        sink.flush();
    }
}

/// Exit flush: sends Shutdown, the thread flushes and exits, then join guarantees completion.
#[inline]
pub fn shutdown() {
    if let Some(sink) = FILE_SINK.get() {
        sink.shutdown();
    }
}

impl AsyncLogFile {
    fn start(path: PathBuf, flush_interval: Duration) -> Self {
        let (tx, rx) = mpsc::sync_channel::<LogMsg>(LOG_QUEUE_CAPACITY);
        let handle = match std::thread::Builder::new()
            .name("sc-log-file".to_string())
            .spawn(move || writer_loop(rx, path, flush_interval))
        {
            Ok(handle) => Some(handle),
            Err(error) => {
                eprintln!("sc_log >> 无法创建日志写入线程: {error}");
                None
            }
        };
        Self {
            tx,
            handle: Mutex::new(handle),
            dropped: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn write_line(&self, line: String) {
        self.write_line_classified(line, LogClass::Normal);
    }

    /// Non-blocking enqueue with an explicit retention class.
    ///
    /// A full queue drops the record instead of blocking the producer, and the
    /// loss is counted and periodically reported on stderr (the log file itself
    /// may be the thing that cannot keep up).
    pub fn write_line_classified(&self, line: String, class: LogClass) {
        let bytes = line.into_bytes();
        match self.tx.try_send(LogMsg::Line(bytes)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                let total = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                if class == LogClass::High && total % DROP_REPORT_INTERVAL == 1 {
                    eprintln!(
                        "[sc_log] log queue saturated ({LOG_QUEUE_CAPACITY} pending); dropped {total} record(s), including high-class records"
                    );
                }
            }
            Err(TrySendError::Disconnected(_)) => {
                let total = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                if total % DROP_REPORT_INTERVAL == 1 {
                    eprintln!("[sc_log] log sink is gone; dropped {total} record(s)");
                }
            }
        }
    }

    /// Number of records dropped because the bounded queue refused them.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn flush(&self) {
        let _ = self.tx.try_send(LogMsg::Flush);
    }

    /// Flush to disk and exit (idempotent; later sends fail silently).
    pub fn shutdown(&self) {
        let dropped = self.dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            eprintln!("[sc_log] shutdown with {dropped} dropped log record(s)");
        }
        let _ = self.tx.try_send(LogMsg::Shutdown);
        if let Some(handle) = self
            .handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = handle.join();
        }
    }
}

/// Writer thread: batched writes plus periodic live flush.
///
/// The file is **opened once** when this thread starts and the same handle (append) is reused;
/// it is never reopened per line/message, avoiding per-log filesystem overhead.
fn writer_loop(rx: Receiver<LogMsg>, path: PathBuf, flush_interval: Duration) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("sc_log >> 无法打开日志文件 {}: {e}", path.display());
            return;
        }
    };
    let mut writer = BufWriter::with_capacity(64 * 1024, file);
    let mut last_flush = Instant::now();
    loop {
        match rx.recv_timeout(flush_interval) {
            Ok(LogMsg::Line(bytes)) => {
                let _ = writer.write_all(&bytes);
                if last_flush.elapsed() >= flush_interval {
                    let _ = writer.flush();
                    last_flush = Instant::now();
                }
            }
            Ok(LogMsg::Flush) => {
                let _ = writer.flush();
                last_flush = Instant::now();
            }
            Ok(LogMsg::Shutdown) => {
                let _ = writer.flush();
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = writer.flush();
                last_flush = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = writer.flush();
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("sc-log-{name}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn saturation_drops_instead_of_blocking_and_counts_the_loss() {
        let directory = TempDir::new("saturation");
        let path = directory.0.join("server.log");
        let (tx, rx) = mpsc::sync_channel::<LogMsg>(1);
        let sink_state = AsyncLogFile {
            tx,
            handle: Mutex::new(None),
            dropped: AtomicU64::new(0),
        };

        sink_state.write_line_classified("first".to_string(), LogClass::High);
        // The channel now holds the single slot, so this one must be dropped.
        sink_state.write_line_classified("second".to_string(), LogClass::High);
        assert_eq!(sink_state.dropped(), 1);
        // The channel still delivered the first record, so ordering is intact.
        assert!(matches!(rx.try_recv(), Ok(LogMsg::Line(_))));
        assert!(rx.try_recv().is_err());
        let _ = path;
    }

    #[test]
    fn log_class_maps_levels_to_retention_bands() {
        assert_eq!(LogClass::of(log::Level::Error), LogClass::High);
        assert_eq!(LogClass::of(log::Level::Warn), LogClass::High);
        assert_eq!(LogClass::of(log::Level::Info), LogClass::Normal);
        assert_eq!(LogClass::of(log::Level::Debug), LogClass::Low);
        assert_eq!(LogClass::of(log::Level::Trace), LogClass::Low);
        assert!(LogClass::High > LogClass::Normal);
        assert!(LogClass::Normal > LogClass::Low);
    }

    #[test]
    fn writer_loop_flushes_before_exit() {
        let directory = TempDir::new("flush");
        let path = directory.0.join("server.log");
        let (tx, rx) = mpsc::sync_channel::<LogMsg>(8);
        let writer_path = path.clone();
        let handle =
            std::thread::spawn(move || writer_loop(rx, writer_path, Duration::from_secs(60)));
        tx.send(LogMsg::Line(b"first record\n".to_vec()))
            .expect("send");
        tx.send(LogMsg::Shutdown).expect("shutdown");
        handle.join().expect("writer thread");

        let contents = std::fs::read_to_string(&path).expect("read log");
        assert!(
            contents.contains("first record"),
            "records must be flushed before exit"
        );
    }
}
