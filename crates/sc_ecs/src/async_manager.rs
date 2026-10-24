//! `SCECSAsync`：进程级共享的 tokio 运行时。
//!
//! `SCECSAsync`: a process-lifetime shared tokio runtime.

use lazy_static::lazy_static;
use std::ops::Deref;
use tokio::runtime::Runtime;
pub use sc_ecs_macros::async_system;

lazy_static! {
    static ref ASYNC_RUNTIME: Runtime = Runtime::new()
        .unwrap_or_else(|error| panic!("failed to create ECS async runtime: {error}"));
}

/// `SCECSAsync`：访问进程级共享 tokio 运行时的入口。
///
/// `SCECSAsync`: entry point for accessing the process-wide shared tokio
/// runtime.
pub struct SCECSAsync;

impl SCECSAsync {
    /// 获取进程级共享运行时的静态引用。
    ///
    /// Returns a static reference to the shared runtime.
    pub fn runtime() -> &'static Runtime {
        ASYNC_RUNTIME.deref()
    }

    /// 创建一个新的临时运行时（用于隔离的短生命周期任务）。
    ///
    /// Creates a new temporary runtime (for isolated short-lived tasks).
    pub fn temp_runtime() -> Runtime {
        Runtime::new().unwrap_or_else(|error| panic!("failed to create temporary runtime: {error}"))
    }
}
