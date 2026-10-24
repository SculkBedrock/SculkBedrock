//! 控制台数据模型：日志行与日志级别（纯值类型，与游戏无关）。
//!
//! Console data models: log lines and log levels (plain value types, no game types).

use std::fmt::{Display, Formatter};

/// 日志级别（控制台侧对 `log::Level` 的映射）。
///
/// Log level (console-side mapping of `log::Level`).
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

/// 一条可展示的日志行（值对象，可安全跨线程移动）。
///
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

/// 顶部信息栏快照（宿主推送的值对象，TUI 内只读）。
///
/// - `core_version`：服务端核心版本（如 `1.0.0ALPHA(fjord)`）；
/// - `pack_label`：资源包标签（如 `Vanilla 1.26.40 * protocol 2168`），
///   只用于输入区脚注；
/// - `subtitle`：标题第二行文本（宿主 API 提供的引言或内置兜底）。
///   为空时渲染层回落到默认标题；资源包加载后宿主可以再次推送。
///
/// Top info-bar snapshot (value object pushed by the host, read-only in the TUI).
///
/// - `core_version`: server core version (e.g. `1.0.0ALPHA(fjord)`);
/// - `pack_label`: pack label (e.g. `Vanilla 1.26.40 * protocol 2168`,
///   used only in the input footnote);
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

/// 本进程的资源快照（由控制台线程本地采样，不涉及宿主）。
///
/// `has_data=false` 表示尚未取得两次采样（CPU % 需要间隔），
/// 渲染层此时显示占位内容。
///
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

/// 服务端启动阶段（驱动开场效果与输入锁定）。
///
/// - `Starting`：服务端仍在启动：输入锁定 + 蓝色流光 + 标题呼吸；
/// - `Sweeping`：启动成功瞬间：绿色扫描（约 900ms），输入仍锁定；
/// - `Running`：正常接受输入。
///
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
