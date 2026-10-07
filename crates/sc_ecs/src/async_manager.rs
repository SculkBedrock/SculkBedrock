//! `SCECSAsync`: process-lifetime shared tokio runtime.
//!
//! Network/IO async tasks are spawned on this runtime; the main-thread 20 TPS
//! schedule loop runs concurrently with the tokio workers (see
//! docs/ecs_concurrency.md for the concurrency rules).

use lazy_static::lazy_static;
use std::ops::Deref;
use tokio::runtime::Runtime;
pub use sc_ecs_macros::async_system;

lazy_static! {
    static ref ASYNC_RUNTIME: Runtime = Runtime::new()
        .unwrap_or_else(|error| panic!("failed to create ECS async runtime: {error}"));
}

pub struct SCECSAsync;

impl SCECSAsync {
    pub fn runtime() -> &'static Runtime {
        ASYNC_RUNTIME.deref()
    }

    pub fn temp_runtime() -> Runtime {
        Runtime::new().unwrap_or_else(|error| panic!("failed to create temporary runtime: {error}"))
    }
}
