use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::watch;

/// Shutdown signal: one `notify` broadcast to many waiters.
///
/// Built on `tokio::sync::watch` (internally lock-free):
/// - `notify()` is synchronous and idempotent, waking all waiters;
/// - `wait()` returns immediately even when called after notify,
///   eliminating missed-notification races;
/// - no receiver clones or RwLock access are needed.
/// no receiver clones or RwLock access are needed.
#[derive(Debug)]
pub struct Notify {
    closed: AtomicBool,
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
}

impl Notify {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            closed: AtomicBool::new(false),
            tx,
            rx,
        }
    }

    /// Broadcast shutdown (idempotent).
    /// Whether this call triggered the shutdown.
    pub fn notify(&self) -> bool {
        if self.closed.swap(true, Ordering::AcqRel) {
            return false;
        }
        // Wake all waiters; the closed flag carries the value.
        let _ = self.tx.send(true);
        true
    }

    /// Whether shutdown fired (non-blocking).
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Wait for the shutdown signal. True means shut down.
    pub async fn wait(&self) -> bool {
        let mut rx = self.rx.clone();
        if self.closed.load(Ordering::Acquire) {
            return true;
        }
        loop {
            if rx.changed().await.is_err() {
                // All senders dropped (cannot happen; Notify holds one).
                return self.closed.load(Ordering::Acquire);
            }
            if *rx.borrow() {
                return true;
            }
        }
    }
}

impl Default for Notify {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn notify_wakes_all_waiters() {
        let notify = std::sync::Arc::new(Notify::new());
        let mut waiters = Vec::new();
        for _ in 0..4 {
            let n = notify.clone();
            waiters.push(tokio::spawn(async move { n.wait().await }));
        }
        // Let waiters suspend first.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(notify.notify());
        assert!(!notify.notify()); // 幂等：已关闭时返回 false
        for w in waiters {
            assert!(w.await.unwrap());
        }
    }

    #[tokio::test]
    async fn wait_after_notify_returns_immediately() {
        let notify = Notify::new();
        assert!(notify.notify());
        assert!(notify.wait().await);
        assert!(notify.wait().await); // 之后重复 wait 也立即返回
    }

    #[tokio::test]
    async fn unnotified_wait_parks() {
        let notify = Notify::new();
        let task = tokio::spawn(async move { notify.wait().await });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!task.is_finished());
        task.abort();
    }
}
