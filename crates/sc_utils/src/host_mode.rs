//! 宿主模式运行时开关（服务端嵌入 GUI/移动应用）。
//!
//! 独立服务端（sc_bootstrap 二进制）不设置 hosted 标志，退出走
//! `std::process::exit(0)`；宿主（如 Tauri 应用 sc_app）在启动前调用
//! [`set_hosted`]，此后 SCExit 的收尾（停止 region 线程 + 刷日志）只置 stopped
//! 标志，宿主 runner 轮询到后正常返回，进程保持存活以便在应用内反复启停。
//!
//! Runtime switch for hosted mode (server embedded in a GUI/mobile app).
//!
//! A standalone server (sc_bootstrap binary) leaves the hosted flag unset and exits via
//! `std::process::exit(0)`; a host (e.g. Tauri app sc_app) calls [`set_hosted`] before
//! startup so SCExit teardown (stop region threads + flush logs) only sets the stopped
//! flag, letting the host runner poll it and return normally while the process stays alive
//! for repeated start/stop inside the app.

use std::sync::atomic::{AtomicBool, Ordering};

static HOSTED: AtomicBool = AtomicBool::new(false);
static SERVER_STOPPED: AtomicBool = AtomicBool::new(false);

/// 将进程标记为托管（在组装服务端 app 之前调用）。
///
/// Marks the process as hosted (call before assembling the server app).
pub fn set_hosted() {
    HOSTED.store(true, Ordering::Relaxed);
}

/// 是否处于托管模式。
///
/// Whether hosted mode is active.
pub fn is_hosted() -> bool {
    HOSTED.load(Ordering::Relaxed)
}

/// 服务端关闭流程已完成：通知宿主 runner 退出其主循环。
///
/// Server shutdown finished: notifies the host runner to leave its main loop.
pub fn notify_server_stopped() {
    SERVER_STOPPED.store(true, Ordering::Relaxed);
}

/// 宿主 runner 轮询：取出停止标志（单次消费）。
///
/// Host runner poll: takes the stopped flag (single consumption).
pub fn take_server_stopped() -> bool {
    SERVER_STOPPED.swap(false, Ordering::Relaxed)
}
