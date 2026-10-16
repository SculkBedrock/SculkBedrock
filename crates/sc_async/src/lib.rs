use lazy_static::lazy_static;
use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::runtime::{EnterGuard, Runtime};

lazy_static! {
    static ref ASYNC_MANAGER: SCAsyncManager = SCAsyncManager::new();
}

static ASYNC_RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub struct SCAsyncManager(Arc<RwLock<HashSet<String>>>);

impl SCAsyncManager {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(HashSet::new())))
    }

    pub fn global() -> &'static Self {
        &ASYNC_MANAGER
    }

    pub fn insert(&self, key: &str) {
        self.0.write().insert(key.to_owned());
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.0.read().contains(key)
    }

    pub fn enter(&self, key: &str) -> Option<EnterGuard<'_>> {
        self.0.read().contains(key).then(|| global_runtime().enter())
    }
    pub fn runtime(&self, key: &str) -> Option<&Runtime> {
        self.0.read().contains(key).then(global_runtime)
    }
}

fn global_runtime() -> &'static Runtime {
    ASYNC_RUNTIME.get_or_init(|| {
        Runtime::new().unwrap_or_else(|error| panic!("failed to create async runtime: {error}"))
    })
}

/// Format a caught panic payload as a short message.
///
/// Shared by the bounded worker pools (chunk load/generate in
/// `sc_world::chunk_executor`, dirty-column writeback in
/// `sc_world::writeback`): both isolate a plugin/storage panic to one job so
/// its waiters receive a terminal error instead of stranding them.
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string())
}
