use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::watch;

/// 关闭信号：一次 `notify` 广播唤醒所有等待者。
///
/// Shutdown signal: one `notify` broadcast to many waiters.
///
/// 基于 `tokio::sync::watch` 构建（内部无锁）：
/// - `notify()` 是同步且幂等的，会唤醒全部等待者；
/// - 即使在 `notify()` 之后调用 `wait()` 也会立即返回，
///   消除“通知丢失”竞态；
/// - 无需克隆 receiver 或访问 RwLock。
#[derive(Debug)]
pub struct Notify {
    /// 已关闭标志：快速路径的原子短路判断。
    ///
    /// Closed flag: atomic fast-path short-circuit.
    closed: AtomicBool,
    /// 广播发送端：通过 watch channel 向所有等待者推送关闭值。
    ///
    /// Broadcast sender: pushes the closed value to all waiters via watch channel.
    tx: watch::Sender<bool>,
    /// 初始接收端：`wait()` 克隆它来订阅变更。
    ///
    /// Initial receiver: `wait()` clones it to subscribe to changes.
    rx: watch::Receiver<bool>,
}

impl Notify {
    /// 创建未触发的关闭信号。
    ///
    /// Creates a shutdown signal that has not been fired.
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            closed: AtomicBool::new(false),
            tx,
            rx,
        }
    }

    /// 广播关闭信号（幂等）。
    /// 返回本次调用是否真正触发了关闭。
    ///
    /// Broadcasts the shutdown signal (idempotent).
    /// Returns whether this call triggered the shutdown.
    pub fn notify(&self) -> bool {
        if self.closed.swap(true, Ordering::AcqRel) {
            return false;
        }
        // Wake all waiters; the closed flag carries the value.
        let _ = self.tx.send(true);
        true
    }

    /// 关闭信号是否已触发（非阻塞查询）。
    ///
    /// Whether the shutdown signal has fired (non-blocking query).
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// 等待关闭信号；返回 `true` 表示已关闭。
    ///
    /// Waits for the shutdown signal; `true` means shut down.
    pub async fn wait(&self) -> bool {
        let mut rx = self.rx.clone();
        if self.closed.load(Ordering::Acquire) {
            return true;
        }
        loop {
            if rx.changed().await.is_err() {
                // All senders dropped (cannot happen: Notify holds one sender itself).
                return self.closed.load(Ordering::Acquire);
            }
            if *rx.borrow() {
                return true;
            }
        }
    }
}

impl Default for Notify {
    /// 等价于 [`Notify::new`]。
    ///
    /// Equivalent to [`Notify::new`].
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
        assert!(!notify.notify()); // Idempotent: false once already closed
        for w in waiters {
            assert!(w.await.unwrap());
        }
    }

    #[tokio::test]
    async fn wait_after_notify_returns_immediately() {
        let notify = Notify::new();
        assert!(notify.notify());
        assert!(notify.wait().await);
        assert!(notify.wait().await); // Repeated wait also returns immediately
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
