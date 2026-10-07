use sc_ecs::event::Event;
use std::error::Error;
use std::num::NonZeroU8;
use std::sync::atomic::{AtomicBool, Ordering};

/// 收到过操作系统关闭请求（Ctrl+C / SIGTERM / SIGHUP）。
///
/// 由信号处理器置位（只做原子存储，async-signal-safe），任何线程可读——
/// 包括主循环启动前阻塞在启动期等待（如等世界）里的逻辑，那里收不到
/// `SCExit` 事件，只能轮询此标志。
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// 记录一次关闭请求（信号处理器调用）。
pub fn note_shutdown_requested() {
    SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
}

/// 是否收到过关闭请求。
pub fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::Relaxed)
}

pub enum SCExitReason {
    Success,
    ErrorCode(NonZeroU8),
    Error(Box<dyn Error + Send + Sync + 'static>),
}

pub enum SCExitType {
    Shutdown,
    Restart,
}

#[derive(Event)]
pub struct SCExit {
    pub reason: SCExitReason,
    pub exit_type: SCExitType,
}

impl SCExit {
    pub fn new(reason: SCExitReason, exit_type: SCExitType) -> Self {
        Self { reason, exit_type }
    }
}
