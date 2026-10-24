/// 接收队列子模块：处理入站帧的重组与排序。
///
/// Receive queue submodule: reassembly and ordering of inbound frames.
pub(crate) mod recv;
/// 发送队列子模块：管理待确认数据报与重传。
///
/// Send queue submodule: unacknowledged datagrams and retransmission.
pub(crate) mod send;

pub use self::recv::*;
pub use self::send::*;

use crate::protocol::frame::{FragmentMeta, Frame, FramePacket};
use crate::protocol::reliability::Reliability;
use crate::protocol::MAX_FRAGS;
use crate::protocol::RAKNET_HEADER_FRAME_OVERHEAD;
use crate::server::current_epoch;
use crate::utils::{add24, contains_inclusive24, forward_distance24, seq24};
use log::trace;
use sc_error::{ScError, ScSite, capture_site};
use std::collections::BTreeMap;
use std::collections::HashMap;

/// 恢复队列的最大条目数（未确认数据报个数上限）。
///
/// Maximum number of entries (unacknowledged datagrams) in the recovery
/// queue.
pub const MAX_RECOVERY_QUEUE_ITEMS: usize = 65_536;
/// 单连接未确认数据报的字节预算；条目数上限本身不构成内存边界。
///
/// Byte budget for unacknowledged datagrams per connection; the item limit
/// alone is not a memory bound.
pub const MAX_RECOVERY_QUEUE_BYTES: usize = 16 * 1024 * 1024;
/// 有序队列允许的最大序号缺口（超出即视为乱序攻击/失效）。
///
/// Maximum sequence gap tolerated by the ordered queue before an index is
/// treated as stale.
const MAX_ORDERED_QUEUE_GAP: u32 = 4096;
/// 有序队列单信道可缓存的帧数上限。
///
/// Maximum number of frames buffered per ordered-queue channel.
const MAX_ORDERED_QUEUE_DEPTH: usize = 4096;
/// 单连接/单信道已缓存入站有序帧的字节预算。
///
/// Byte budget for buffered inbound ordered frames per connection/channel.
pub const MAX_ORDERED_QUEUE_BYTES: usize = 4 * 1024 * 1024;
/// 同时缓存的分片集合数上限。
///
/// Maximum number of concurrently buffered fragment sets.
const MAX_FRAGMENT_SETS: usize = 128;
/// 分片队列中可缓存的帧总数上限。
///
/// Maximum total number of frames buffered inside the fragment queue.
const MAX_FRAGMENT_FRAMES: usize = 4096;

/// 可报告其在网络队列中所占内存的条目。
///
/// Items that can report the memory they retain inside a network queue.
pub trait QueuedItem {
    /// 该条目自身持有的字节数（负载加队列记账开销）。
    ///
    /// Owned bytes retained by this item (payload plus queue bookkeeping).
    fn queued_bytes(&self) -> usize;
}

impl QueuedItem for FramePacket {
    /// 汇总所有帧体长度与每帧固定开销，加上结构体自身大小。
    ///
    /// Sums every frame body length plus per-frame overhead, and adds the
    /// struct's own size.
    fn queued_bytes(&self) -> usize {
        let payload: usize = self
            .frames
            .iter()
            .map(|frame| frame.body.len() + frame_overhead_bytes())
            .sum();
        payload + std::mem::size_of::<FramePacket>()
    }
}

/// 单个帧在队列记账中的固定额外开销。
///
/// Fixed per-frame overhead accounted for inside queues.
fn frame_overhead_bytes() -> usize {
    std::mem::size_of::<Frame>() + usize::from(RAKNET_HEADER_FRAME_OVERHEAD)
}

/// 恢复队列因某项上限耗尽而拒绝插入。
///
/// 队列不会淘汰较早的未确认数据报来腾出空间；调用方必须把这视为连接的
/// 终止状态。
///
/// The recovery queue refused an insertion because a bound is exhausted.
///
/// The queue never evicts an older unacknowledged datagram to make room; the
/// caller must treat this as a terminal connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryQueueFull {
    /// 当前条目数。
    ///
    /// Current number of entries.
    pub entries: usize,
    /// 当前已记账字节数。
    ///
    /// Currently accounted bytes.
    pub bytes: usize,
    /// 被拒绝插入条目的字节数。
    ///
    /// Byte size of the refused item.
    pub item_bytes: usize,
    /// 条目数上限。
    ///
    /// Maximum number of entries.
    pub max_entries: usize,
    /// 字节上限。
    ///
    /// Maximum number of bytes.
    pub max_bytes: usize,
}

impl std::fmt::Display for RecoveryQueueFull {
    /// 以 `entries/bytes/item` 形式格式化上限信息。
    ///
    /// Formats the limit information as `entries/bytes/item`.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "recovery queue full ({} entries/{}, {} bytes/{}, next item {} bytes)",
            self.entries, self.max_entries, self.bytes, self.max_bytes, self.item_bytes
        )
    }
}

/// 队列插入或取出失败。
///
/// Queue insertion or retrieval failure.
#[derive(Debug, Clone, ScError)]
#[sc_error(bound = "E: ScError + 'static")]
pub enum NetQueueError<E> {
    /// 插入失败（原因未细分）。
    ///
    /// The insertion failed for any given reason.
    #[sc_error(code = "raknet.queue.invalid_insertion")]
    InvalidInsertion { detail: String, site: ScSite },
    /// 插入失败且已知原因。
    ///
    /// The insertion failed and the reason is known.
    #[sc_error(code = "raknet.queue.invalid_insertion_known")]
    InvalidInsertionKnown {
        reason: String,
        detail: String,
        site: ScSite,
    },
    /// 从队列中移除条目失败。
    ///
    /// The `Item` failed to be removed from the queue.
    #[sc_error(code = "raknet.queue.item_deletion_fail")]
    ItemDeletionFail { detail: String, site: ScSite },
    /// 条目非法，无法取出。
    ///
    /// The `Item` is invalid and can not be retrieved.
    #[sc_error(code = "raknet.queue.invalid_item")]
    InvalidItem { detail: String, site: ScSite },
    /// 队列为空。
    ///
    /// The queue is empty.
    #[sc_error(code = "raknet.queue.empty")]
    EmptyQueue { detail: String, site: ScSite },
    /// 自定义错误（包装上层错误源）。
    ///
    /// The error is a custom error.
    #[sc_error(code = "raknet.queue.other")]
    Other {
        source: E,
        detail: String,
        site: ScSite,
    },
}

impl<E> NetQueueError<E> {
    /// 构造插入失败错误，并捕获调用点。
    ///
    /// Build an insertion failure, capturing the call site.
    #[track_caller]
    pub fn invalid_insertion(detail: impl Into<String>) -> Self {
        Self::InvalidInsertion {
            detail: detail.into(),
            site: capture_site(),
        }
    }

    /// 构造已知原因的插入失败错误，并捕获调用点。
    ///
    /// Build a known-reason insertion failure, capturing the call site.
    #[track_caller]
    pub fn invalid_insertion_known(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        let detail = reason.clone();
        Self::InvalidInsertionKnown {
            reason,
            detail,
            site: capture_site(),
        }
    }

    /// 构造条目删除失败错误，并捕获调用点。
    ///
    /// Build an item-deletion failure, capturing the call site.
    #[track_caller]
    pub fn item_deletion_fail(detail: impl Into<String>) -> Self {
        Self::ItemDeletionFail {
            detail: detail.into(),
            site: capture_site(),
        }
    }

    /// 构造非法条目错误，并捕获调用点。
    ///
    /// Build an invalid-item error, capturing the call site.
    #[track_caller]
    pub fn invalid_item(detail: impl Into<String>) -> Self {
        Self::InvalidItem {
            detail: detail.into(),
            site: capture_site(),
        }
    }

    /// 构造空队列错误，并捕获调用点。
    ///
    /// Build an empty-queue error, capturing the call site.
    #[track_caller]
    pub fn empty_queue(detail: impl Into<String>) -> Self {
        Self::EmptyQueue {
            detail: detail.into(),
            site: capture_site(),
        }
    }

    /// 构造自定义错误，并捕获调用点。
    ///
    /// Build a custom error, capturing the call site.
    #[track_caller]
    pub fn other(source: E, detail: impl Into<String>) -> Self {
        Self::Other {
            source,
            detail: detail.into(),
            site: capture_site(),
        }
    }
}

impl<E> std::fmt::Display for NetQueueError<E> {
    /// 以 `[code] detail (at site)` 形式格式化错误。
    ///
    /// Formats the error as `[code] detail (at site)`.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInsertion { detail, site } => {
                write!(formatter, "[raknet.queue.invalid_insertion] {detail} (at {site})")
            }
            Self::InvalidInsertionKnown { detail, site, .. } => {
                write!(
                    formatter,
                    "[raknet.queue.invalid_insertion_known] {detail} (at {site})"
                )
            }
            Self::ItemDeletionFail { detail, site } => {
                write!(
                    formatter,
                    "[raknet.queue.item_deletion_fail] {detail} (at {site})"
                )
            }
            Self::InvalidItem { detail, site } => {
                write!(formatter, "[raknet.queue.invalid_item] {detail} (at {site})")
            }
            Self::EmptyQueue { detail, site } => {
                write!(formatter, "[raknet.queue.empty] {detail} (at {site})")
            }
            Self::Other { detail, site, .. } => {
                write!(formatter, "[raknet.queue.other] {detail} (at {site})")
            }
        }
    }
}

impl<E> std::error::Error for NetQueueError<E>
where
    E: std::error::Error + 'static,
{
    /// 自定义变体返回其包装的错误源，其余返回 `None`。
    ///
    /// Returns the wrapped error source for the custom variant, `None`
    /// otherwise.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Other { source, .. } => Some(source),
            _ => None,
        }
    }
}


/// 网络队列抽象：插入、移除、查询与清空。
///
/// Network queue abstraction: insert, remove, query, and flush.
pub trait NetQueue<Item> {
    /// 队列存放的条目类型。
    ///
    /// The `Item` of the queue.
    // type Item = V;

    /// 每个 `Item` 存储所用的键（用于移除）。
    ///
    /// The "key" that each `Item` is stored under (used for removal).
    type KeyId;

    /// [`NetQueueError`] 的自定义错误类型参数。
    ///
    /// A custom error specifier for NetQueueError.
    type Error;

    /// 在条件满足时把 `Item` 插入队列，返回其键。
    ///
    /// Inserts `Item` into the queue, given the conditions are fulfilled,
    /// and returns its key.
    fn insert(&mut self, item: Item) -> Result<Self::KeyId, NetQueueError<Self::Error>>;

    /// 通过 `Self::KeyId` 从队列中移除一个 `Item`。
    ///
    /// Removes an `Item` from the queue by providing an instance of
    /// `Self::KeyId`.
    fn remove(&mut self, key: Self::KeyId) -> Result<Item, NetQueueError<Self::Error>>;

    /// 以引用方式从队列中取出一个 `Item`。
    ///
    /// Retrieves an `Item` from the queue, by reference.
    fn get(&mut self, key: Self::KeyId) -> Result<&Item, NetQueueError<Self::Error>>;

    /// 清空整个队列，返回其中所有条目。
    ///
    /// Clears the entire queue, returning every item it held.
    fn flush(&mut self) -> Result<Vec<Item>, NetQueueError<Self::Error>>;
}

/// 恢复队列：存放需要重传的报文，用于可靠与排序/有序帧。
///
/// A recovery queue is used to store packets that need to be resent; it
/// is used for sequenced and ordered packets.
#[derive(Debug, Clone)]
pub struct RecoveryQueue<Item> {
    /// 当前报文队列，按时间组织：(序号, (报文, 时间戳))。
    ///
    /// The current queue of packets by timestamp:
    /// (seq, (packet, timestamp)).
    // TODO: use the timestamp for round trip time (RTT).
    queue: HashMap<u32, (u64, Item)>,
    /// 队列条目自身持有的字节数（结构性估算）。
    ///
    /// Owned bytes retained by the queued items (structural estimate).
    queued_bytes: usize,
    /// 条目数上限。
    ///
    /// Maximum number of entries.
    max_items: usize,
    /// 字节预算上限。
    ///
    /// Maximum retained bytes.
    max_bytes: usize,
    /// 可选的字节测量函数；`None` 时不做字节准入。
    ///
    /// Optional owned-byte sizer; `None` disables byte admission.
    sizer: Option<fn(&Item) -> usize>,
}

impl<Item> RecoveryQueue<Item>
where
    Item: Clone,
{
    /// 以默认上限（条目数 [`MAX_RECOVERY_QUEUE_ITEMS`]、不限字节）创建队列。
    ///
    /// Creates a queue with the default limits
    /// ([`MAX_RECOVERY_QUEUE_ITEMS`] entries, unbounded bytes).
    pub fn new() -> Self {
        Self::with_limits(MAX_RECOVERY_QUEUE_ITEMS, usize::MAX, None)
    }

    /// 有界队列。`sizer` 启用字节预算；未提供时仅应用条目上限
    /// （供测试与非负载队列使用）。
    ///
    /// Bounded queue. `sizer` enables the byte budget; without it only the
    /// item budget applies (used by tests and non-payload queues).
    ///
    /// - `max_items`: 条目数上限
    /// - `max_bytes`: 字节预算上限
    /// - `sizer`: 字节测量函数
    pub fn with_limits(
        max_items: usize,
        max_bytes: usize,
        sizer: Option<fn(&Item) -> usize>,
    ) -> Self {
        Self {
            queue: HashMap::new(),
            queued_bytes: 0,
            max_items: max_items.max(1),
            max_bytes,
            sizer,
        }
    }

    /// 登记一个待重传的数据报。
    ///
    /// 达到上限时返回 [`RecoveryQueueFull`] 而不淘汰较早的未确认数据报：
    /// 可靠数据不得从对端流中静默消失，因此队列饱和对连接而言是终止状态。
    ///
    /// - `seq`: 数据报序号（内部截断为 24 位）
    /// - `item`: 待跟踪的报文
    ///
    /// Tracks a datagram for retransmission.
    ///
    /// Returns [`RecoveryQueueFull`] instead of evicting an older
    /// unacknowledged datagram: reliable data must not silently disappear
    /// from the peer's stream, so a saturated queue is a terminal state
    /// for the connection.
    ///
    /// - `seq`: datagram sequence number (truncated to 24 bits internally)
    /// - `item`: the packet to track
    pub fn insert_id(&mut self, seq: u32, item: Item) -> Result<(), RecoveryQueueFull> {
        let seq = seq24(seq);
        let item_bytes = self.item_bytes(&item);
        if self.queue.contains_key(&seq) {
            // Replacing an entry must not leak the previous byte accounting.
            if let Some((_, previous)) = self.queue.remove(&seq) {
                self.queued_bytes = self.queued_bytes.saturating_sub(self.item_bytes(&previous));
            }
        } else if self.queue.len() >= self.max_items
            || self.queued_bytes.saturating_add(item_bytes) > self.max_bytes
        {
            return Err(RecoveryQueueFull {
                entries: self.queue.len(),
                bytes: self.queued_bytes,
                item_bytes,
                max_entries: self.max_items,
                max_bytes: self.max_bytes,
            });
        }
        self.queued_bytes = self.queued_bytes.saturating_add(item_bytes);
        self.queue.insert(seq, (current_epoch(), item));
        Ok(())
    }

    /// 按 sizer 测量条目字节数；未配置 sizer 时为 0。
    ///
    /// Measures an item's bytes via the sizer; 0 when no sizer is set.
    fn item_bytes(&self, item: &Item) -> usize {
        self.sizer.map(|sizer| sizer(item)).unwrap_or(0)
    }

    /// 未确认数据报的数量。
    ///
    /// Number of unacknowledged datagrams.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// 队列是否为空。
    ///
    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// 未确认数据报所占用的字节数。
    ///
    /// Owned bytes retained by unacknowledged datagrams.
    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    /// 条目数上限。
    ///
    /// The entry-count limit.
    pub fn max_items(&self) -> usize {
        self.max_items
    }

    /// 字节预算上限。
    ///
    /// The byte budget limit.
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// 下一次插入是否必定被拒绝。
    ///
    /// Whether the next insertion is guaranteed to be refused.
    pub fn is_full(&self) -> bool {
        self.queue.len() >= self.max_items || self.queued_bytes >= self.max_bytes
    }

    /// 最旧的未确认数据报的存活秒数。
    ///
    /// Age in seconds of the oldest unacknowledged datagram.
    pub fn oldest_unacked_age(&self) -> Option<u64> {
        self.queue
            .values()
            .map(|(time, _)| current_epoch().saturating_sub(*time))
            .max()
    }

    /// 取出全部条目（序号, 条目）快照。
    ///
    /// Takes a snapshot of all entries as (sequence, item) pairs.
    pub fn get_all(&mut self) -> Vec<(u32, Item)> {
        self.queue
            .iter()
            .map(|(seq, (_, item))| (*seq, item.clone()))
            .collect::<Vec<_>>()
    }

    /// 移除闭区间 `[start, end]` 内的所有序号（24 位回绕安全）。
    ///
    /// Removes every sequence inside the inclusive range `[start, end]`
    /// (24-bit wrap-safe).
    ///
    /// - `start`: 区间起始序号
    /// - `end`: 区间结束序号
    pub fn remove_inclusive_range(&mut self, start: u32, end: u32) {
        let start = seq24(start);
        let end = seq24(end);
        let keys = self
            .queue
            .keys()
            .copied()
            .filter(|seq| contains_inclusive24(start, end, *seq))
            .collect::<Vec<_>>();
        for key in keys {
            let _ = self.remove(key);
        }
    }

    /// 取闭区间 `[start, end]` 内至多 `limit` 个条目。
    ///
    /// Returns at most `limit` items inside the inclusive range
    /// `[start, end]`.
    ///
    /// - `start`: 区间起始序号
    /// - `end`: 区间结束序号
    /// - `limit`: 返回条目数上限
    pub fn get_inclusive_range(&self, start: u32, end: u32, limit: usize) -> Vec<Item> {
        let start = seq24(start);
        let end = seq24(end);
        let span = forward_distance24(start, end);
        self.queue
            .iter()
            .filter(|(seq, _)| forward_distance24(start, **seq) <= span)
            .take(limit)
            .map(|(_, (_, item))| item.clone())
            .collect()
    }

    /// 丢弃超过 `threshold` 秒仍未确认的数据报。
    ///
    /// 对端已停止确认；调用方应据此关闭连接，而不是继续按可靠流处理。
    ///
    /// - `threshold`: 超时秒数
    ///
    /// Drops datagrams unacknowledged for longer than `threshold` seconds.
    ///
    /// The peer stopped acknowledging; callers are expected to close the
    /// connection rather than keep treating the reliable stream as intact.
    ///
    /// - `threshold`: seconds before a datagram is considered stale
    pub fn flush_old(&mut self, threshold: u64) -> Vec<Item> {
        let now = current_epoch();
        let old: Vec<u32> = self
            .queue
            .iter()
            .filter(|(_, (time, _))| time.saturating_add(threshold) < now)
            .map(|(seq, _)| *seq)
            .collect();
        old.into_iter()
            .filter_map(|seq| self.remove(seq).ok())
            .collect()
    }

    /// 返回至多 `limit` 个超过 `threshold` 秒未确认的报文，且只重置被选中
    /// 报文的时间戳。未被选中的过期条目在下一个 tick 仍然立即可用，
    /// 避免发送循环的每 tick 预算反复推迟它并未真正尝试过的报文。
    ///
    /// 报文**不会被移除**——它们留在队列中直到被显式 ACK 移除，
    /// 以保留 NACK 驱动的重传能力。
    ///
    /// - `threshold`: 超时秒数
    /// - `limit`: 本次返回条目数上限
    ///
    /// Returns at most `limit` packets that have been unacknowledged for
    /// longer than `threshold` seconds and resets timestamps only for the
    /// selected packets. Unselected expired entries remain immediately
    /// eligible on the next tick, preventing the send loop's per-tick
    /// budget from postponing packets it did not actually attempt.
    ///
    /// Packets are **NOT removed** — they stay in the queue until an
    /// explicit ACK removes them. This preserves NACK-driven recovery.
    ///
    /// - `threshold`: seconds before a packet is considered stale
    /// - `limit`: maximum number of entries returned by this call
    pub fn resend_old(&mut self, threshold: u64, limit: usize) -> Vec<Item> {
        if limit == 0 {
            return Vec::new();
        }
        let now = current_epoch();
        let mut to_resend = Vec::with_capacity(limit.min(self.queue.len()));
        for (_, (time, item)) in self.queue.iter_mut() {
            if to_resend.len() >= limit {
                break;
            }
            if time.saturating_add(threshold) < now {
                to_resend.push(item.clone());
                // Only selected retransmissions consume the timeout budget.
                *time = now;
            }
        }
        to_resend
    }

    /// 清空恢复队列并释放已分配容量；由 `Connection::close()` 调用，以便
    /// 立即释放大缓冲区。
    ///
    /// Clears the recovery queue and releases its allocated capacity; called
    /// during `Connection::close()` so large buffers are freed immediately.
    pub fn clear(&mut self) {
        self.queue.clear();
        self.queue.shrink_to_fit();
        self.queued_bytes = 0;
    }
}

impl<Item> NetQueue<Item> for RecoveryQueue<Item>
where
    Item: Clone,
{
    type KeyId = u32;
    type Error = ();

    /// 分配一个空闲序号并插入条目。
    ///
    /// Finds a free sequence number and inserts the item under it.
    fn insert(&mut self, item: Item) -> Result<Self::KeyId, NetQueueError<Self::Error>> {
        let start = seq24(self.queue.len() as u32);
        let mut index = start;
        while self.queue.contains_key(&index) {
            index = seq24(index.wrapping_add(1));
            if index == start {
                return Err(NetQueueError::invalid_insertion_known(
                    "recovery queue key space exhausted",
                ));
            }
        }
        self.insert_id(index, item)
            .map_err(|full| NetQueueError::invalid_insertion_known(full.to_string()))?;
        Ok(index)
    }

    /// 按键移除条目，并释放其字节记账。
    ///
    /// Removes an item by key and releases its byte accounting.
    fn remove(&mut self, key: Self::KeyId) -> Result<Item, NetQueueError<Self::Error>> {
        if let Some((_, item)) = self.queue.remove(&key) {
            self.queued_bytes = self.queued_bytes.saturating_sub(self.item_bytes(&item));
            Ok(item)
        } else {
            Err(NetQueueError::item_deletion_fail(
                "Queue item deletion failed",
            ))
        }
    }

    /// 按键取出条目引用。
    ///
    /// Borrows an item by key.
    fn get(&mut self, key: Self::KeyId) -> Result<&Item, NetQueueError<Self::Error>> {
        if let Some((_, item)) = self.queue.get(&key) {
            Ok(item)
        } else {
            Err(NetQueueError::item_deletion_fail(
                "Queue item deletion failed",
            ))
        }
    }

    /// 清空队列并返回所有条目。
    ///
    /// Drains the queue and returns every item.
    fn flush(&mut self) -> Result<Vec<Item>, NetQueueError<Self::Error>> {
        let mut items = Vec::new();
        for (_, (_, item)) in self.queue.drain() {
            items.push(item);
        }
        self.queued_bytes = 0;
        Ok(items)
    }
}

/// 有序队列：在可靠窗口期内按信道整理入站报文的序号。
///
/// 用法：
///
/// An ordered queue is used to index incoming packets over a channel
/// within a reliable window time.
///
/// Usage:
/// ```ignore
/// use sc_raknet::connection::queue::OrderedQueue;
/// let mut ord_qu: OrderedQueue<Vec<u8>> = OrderedQueue::new();
/// // Insert a packet with the id of "1"
/// ord_qu.insert(1, vec![0, 1]);
/// ord_qu.insert(5, vec![1, 0]);
/// ord_qu.insert(3, vec![2, 0]);
///
/// // Get the packets we still need.
/// let needed: Vec<u32> = ord_qu.missing();
/// assert_eq!(needed, vec![0, 2, 4]);
///
/// // We would in theory, request these packets, but we're going to insert them
/// ord_qu.insert(4, vec![2, 0, 0, 1]);
/// ord_qu.insert(2, vec![1, 0, 0, 2]);
///
/// // Now let's return our packets in order.
/// // Will return a vector of these packets in order by their "id".
/// let ordered: Vec<Vec<u8>> = ord_qu.flush();
/// ```
#[derive(Debug, Clone)]
pub struct OrderedQueue<Item: Clone + std::fmt::Debug> {
    /// 当前有序队列信道：信道 -> (最高序号, 有序序号, 条目)。
    ///
    /// The current ordered queue channels:
    /// Channel, (Highest Index, Ord Index, Item).
    pub queue: BTreeMap<u32, Item>,
    /// 该队列的窗口（下界, 上界）。
    ///
    /// The window for this queue.
    pub window: (u32, u32),
    /// 缓存的乱序帧所占用的字节数。
    ///
    /// Owned bytes retained by buffered out-of-order frames.
    queued_bytes: usize,
    /// 字节预算上限。
    ///
    /// Maximum retained bytes.
    max_bytes: usize,
}

impl QueuedItem for Vec<u8> {
    /// 字节向量条目按其负载长度加 `Vec` 头大小记账。
    ///
    /// A byte-vector item costs its payload length plus the `Vec` header.
    fn queued_bytes(&self) -> usize {
        self.len() + std::mem::size_of::<Self>()
    }
}

/// 有序帧未被接受的原因。
///
/// Why an ordered frame was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderedQueueReject {
    /// 重复或落后于当前窗口；无害。
    ///
    /// Duplicate or behind the current window; harmless.
    Ignored,
    /// 某项上限耗尽，该信道已产生永久空洞。
    ///
    /// 有序投递不能跳过序号：一旦某帧被丢弃，该信道上后续所有帧都会
    /// 永远停滞。必须终止连接，而不是把它吞掉。
    ///
    /// A bound is exhausted, so the channel has a permanent hole.
    ///
    /// Ordered delivery cannot skip an index: once a frame is dropped,
    /// every later frame on that channel stalls forever. This must
    /// terminate the connection instead of being swallowed.
    Exhausted { entries: usize, bytes: usize },
}

impl<Item> OrderedQueue<Item>
where
    Item: Clone + std::fmt::Debug + QueuedItem,
{
    /// 以默认字节预算 [`MAX_ORDERED_QUEUE_BYTES`] 创建有序队列。
    ///
    /// Creates an ordered queue with the default byte budget
    /// [`MAX_ORDERED_QUEUE_BYTES`].
    pub fn new() -> Self {
        Self::with_byte_budget(MAX_ORDERED_QUEUE_BYTES)
    }

    /// 以指定字节预算创建有序队列。
    ///
    /// Creates an ordered queue with the given byte budget.
    ///
    /// - `max_bytes`: 字节预算上限
    pub fn with_byte_budget(max_bytes: usize) -> Self {
        Self {
            queue: BTreeMap::new(),
            window: (0, 0),
            queued_bytes: 0,
            max_bytes,
        }
    }

    /// 当前为乱序投递缓存的字节数。
    ///
    /// Owned bytes currently buffered for out-of-order delivery.
    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    /// 推进窗口下界并返回新值。
    ///
    /// Advances the window lower bound and returns the new value.
    pub fn next(&mut self) -> u32 {
        self.window.0 = self.window.0.wrapping_add(1);
        self.window.0
    }

    /// 把条目插入指定序号位置。
    ///
    /// 越界/重复返回 [`OrderedQueueReject::Ignored`]；深度或字节预算耗尽
    /// 返回 [`OrderedQueueReject::Exhausted`]。
    ///
    /// - `index`: 目标序号
    /// - `item`: 待缓存条目
    ///
    /// Inserts an item at the given sequence index.
    ///
    /// Out-of-range/duplicate yields [`OrderedQueueReject::Ignored`];
    /// exhausted depth or byte budget yields
    /// [`OrderedQueueReject::Exhausted`].
    ///
    /// - `index`: target sequence number
    /// - `item`: the entry to buffer
    pub fn insert(&mut self, index: u32, item: Item) -> Result<(), OrderedQueueReject> {
        let index = seq24(index);
        if forward_distance24(self.window.0, index) > MAX_ORDERED_QUEUE_GAP {
            return Err(OrderedQueueReject::Ignored);
        }

        if self.queue.contains_key(&index) {
            return Err(OrderedQueueReject::Ignored);
        }

        if self.queue.len() >= MAX_ORDERED_QUEUE_DEPTH {
            return Err(OrderedQueueReject::Exhausted {
                entries: self.queue.len(),
                bytes: self.queued_bytes,
            });
        }

        let item_bytes = item.queued_bytes();
        if self.queued_bytes.saturating_add(item_bytes) > self.max_bytes {
            return Err(OrderedQueueReject::Exhausted {
                entries: self.queue.len(),
                bytes: self.queued_bytes,
            });
        }

        let index_offset = forward_distance24(self.window.0, index);
        let end_offset = forward_distance24(self.window.0, self.window.1);
        if index_offset >= end_offset {
            self.window.1 = add24(index, 1);
        }

        self.queued_bytes = self.queued_bytes.saturating_add(item_bytes);
        self.queue.insert(index, item);
        Ok(())
    }

    /// 无条件按序号插入（允许覆盖已有序号），并维护字节记账。
    ///
    /// Inserts by index unconditionally (overwriting an existing index)
    /// while maintaining the byte accounting.
    ///
    /// - `index`: 目标序号
    /// - `item`: 待缓存条目
    pub fn insert_abs(&mut self, index: u32, item: Item) {
        let index = seq24(index);
        if forward_distance24(self.window.0, index) > MAX_ORDERED_QUEUE_GAP {
            return;
        }
        if self.queue.len() >= MAX_ORDERED_QUEUE_DEPTH && !self.queue.contains_key(&index) {
            return;
        }
        let index_offset = forward_distance24(self.window.0, index);
        let end_offset = forward_distance24(self.window.0, self.window.1);
        if index_offset >= end_offset {
            self.window.1 = add24(index, 1);
        }

        if let Some(previous) = self.queue.insert(index, item) {
            self.queued_bytes = self
                .queued_bytes
                .saturating_add(self.current_item_bytes(index));
            self.queued_bytes = self.queued_bytes.saturating_sub(previous.queued_bytes());
        } else {
            self.queued_bytes = self
                .queued_bytes
                .saturating_add(self.current_item_bytes(index));
        }
    }

    /// 读取指定序号当前条目的字节数；不存在时为 0。
    ///
    /// Reads the byte size of the item at `index`; 0 when absent.
    fn current_item_bytes(&self, index: u32) -> usize {
        self.queue.get(&index).map(Item::queued_bytes).unwrap_or(0)
    }

    /// 列出窗口内仍缺失的序号。
    ///
    /// Lists the sequence numbers still missing inside the window.
    pub fn missing(&self) -> Vec<u32> {
        let mut missing = Vec::new();
        let count = forward_distance24(self.window.0, self.window.1);
        for offset in 0..count {
            let i = add24(self.window.0, offset);
            if !self.queue.contains_key(&i) {
                missing.push(i);
            }
        }
        missing
    }

    /// 强制冲刷入站队列，把窗口上界重置为下界后按序取出。
    ///
    /// Forcefully flushes the incoming queue, resetting the highest window
    /// to the lowest window before draining in order.
    pub fn flush(&mut self) -> Vec<Item> {
        let mut items = Vec::new();

        while let Some(item) = self.queue.remove(&self.window.0) {
            self.queued_bytes = self.queued_bytes.saturating_sub(item.queued_bytes());
            items.push(item);
            self.window.0 = add24(self.window.0, 1);
        }
        items
    }

    /// 缓存在信道头空隙之后的帧数。
    ///
    /// Frames currently buffered behind the channel's head gap.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// 队列是否为空。
    ///
    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// 从窗口起点开始逐个取走队列中连续存在的序号，遇到缺口即停止。
    ///
    /// Flushes the contiguous run of indexes present in the queue starting at
    /// the window base, stopping at the first missing index.
    pub fn flush_old_impl(&mut self) -> Vec<Item> {
        let mut items = Vec::<(u32, Item)>::new();

        let mut i = self.window.0;

        while self.queue.contains_key(&i) {
            trace!("[!>] Removing: {}", &i);
            if let Some(item) = self.queue.remove(&i) {
                self.queued_bytes = self.queued_bytes.saturating_sub(item.queued_bytes());
                items.push((self.window.0, item));
                i += 1;
            } else {
                break;
            }
        }

        self.window.0 = i;

        items.sort_by(|a, b| a.0.cmp(&b.0));
        items
            .iter()
            .map(|(_, item)| item.clone())
            .collect::<Vec<Item>>()
    }
}

/// 专用的线上分片重排结构，也可用于把帧拆分。
///
/// **注意：** 该结构不会更新帧的可靠序号，调用方需要自行处理。
///
/// A specialized structure for re-ordering fragments over the wire; you
/// can use this structure to fragment frames as well.
///
/// **NOTE:** This structure will NOT update a frame's reliable index;
/// the sender is required to do so.
#[derive(Clone, Debug)]
pub struct FragmentQueue {
    /// 当前使用的分片 ID。
    /// 若因故回绕到 0 且分片队列已满，则清除并复用 0。
    ///
    /// The current fragment id to use. If for some reason this wraps back
    /// to 0 and the fragment queue is full, 0 is then cleared and reused.
    fragment_id: u16,

    /// 当前分片集合：以分片 ID 为键，值为 (`size`, Vec<Frame>)。
    ///
    /// The current fragments: a hashmap keyed by fragment id, with the
    /// value being (`size`, Vec<Frame>).
    fragments: HashMap<u16, (u32, Vec<Frame>)>,
    /// 已缓存的分片帧总数（跨所有集合）。
    ///
    /// Total number of buffered fragment frames across all sets.
    fragment_frames: usize,
}

impl FragmentQueue {
    /// 创建空的分片队列。
    ///
    /// Creates an empty fragment queue.
    pub fn new() -> Self {
        Self {
            fragment_id: 0,
            fragments: HashMap::new(),
            fragment_frames: 0,
        }
    }

    /// 把帧插入分片队列，返回 (`fragment_size`, `fragment_index`)。
    ///
    /// 帧未携带分片元信息、序号越界、重复或队列已满时返回错误。
    ///
    /// Inserts the frame into the fragment queue, returning a tuple of
    /// (`fragment_size`, `fragment_index`).
    ///
    /// Errors when the frame carries no fragment metadata, the index is
    /// out of bounds, the frame already exists, or the queue is full.
    pub fn insert(&mut self, fragment: Frame) -> Result<(u32, u32), FragmentQueueError> {
        if let Some(meta) = fragment.fragment_meta.clone() {
            if meta.size == 0 || meta.size > MAX_FRAGS {
                return Err(FragmentQueueError::too_many_fragments());
            }
            return if let Some((size, frames)) = self.fragments.get_mut(&meta.id) {
                // check if the frame index is out of bounds
                // The index starts at 0 while the size starts at 1, hence
                // the `>=` bound check.
                if meta.index >= *size {
                    return Err(FragmentQueueError::frame_index_out_of_bounds());
                }
                // the frame exists, and we have parts, check if we have this particular frame already.
                if frames.iter().any(|f| {
                    f.fragment_meta
                        .as_ref()
                        .is_some_and(|fragment| fragment.index == meta.index)
                }) {
                    // We already have this frame! Do not replace it!!
                    Err(FragmentQueueError::frame_exists())
                } else if self.fragment_frames >= MAX_FRAGMENT_FRAMES {
                    Err(FragmentQueueError::queue_full())
                } else {
                    frames.push(fragment);
                    self.fragment_frames += 1;
                    Ok((meta.size, meta.index))
                }
            } else {
                if self.fragments.len() >= MAX_FRAGMENT_SETS
                    || self.fragment_frames >= MAX_FRAGMENT_FRAMES
                {
                    return Err(FragmentQueueError::queue_full());
                }
                // We don't already have this fragment index!
                let (size, mut frames) = (meta.size, Vec::<Frame>::new());
                frames.push(fragment);

                self.fragments.insert(meta.id, (size, frames));
                self.fragment_frames += 1;
                Ok((meta.size, meta.index))
            };
        }

        Err(FragmentQueueError::frame_not_fragmented())
    }

    /// 尝试收集指定分片 ID 的全部碎片；未齐时失败。
    ///
    /// Attempts to collect all fragments from a given fragment id; fails
    /// if not all fragments are present.
    ///
    /// - `id`: 分片集合 ID
    pub fn collect(&mut self, id: u16) -> Result<Vec<u8>, FragmentQueueError> {
        let Some((size, frames)) = self.fragments.get_mut(&id) else {
            return Err(FragmentQueueError::fragment_invalid());
        };
        if *size != frames.len() as u32 {
            return Err(FragmentQueueError::fragments_missing());
        }

        frames.sort_by(|a, b| {
            let a_index = a.fragment_meta.as_ref().map_or(u32::MAX, |meta| meta.index);
            let b_index = b.fragment_meta.as_ref().map_or(u32::MAX, |meta| meta.index);
            a_index.cmp(&b_index)
        });

        let mut buffer = Vec::<u8>::new();
        for frame in frames.iter() {
            buffer.extend_from_slice(&frame.body);
        }

        self.remove(&id);
        Ok(buffer)
    }

    /// 把给定缓冲在指定 MTU 限制内拆成一批较小的帧并入队。
    ///
    /// Splits a given buffer into a bunch of smaller frames within the
    /// specified restriction and enqueues them.
    ///
    /// - `buffer`: 待拆分的原始数据
    /// - `mtu`: 可用的 MTU 大小
    pub fn split_insert(&mut self, buffer: &[u8], mtu: u16) -> Result<u16, FragmentQueueError> {
        // Fragment ids advance by one and wrap at u16.
        self.fragment_id = self.fragment_id.wrapping_add(1);

        let id = self.fragment_id;

        self.remove(&id);

        if let Ok(frames) = Self::split(buffer, id, mtu) {
            if frames.len() > MAX_FRAGMENT_FRAMES
                || (self.fragments.len() >= MAX_FRAGMENT_SETS && !self.fragments.contains_key(&id))
            {
                return Err(FragmentQueueError::queue_full());
            }
            if let Some((_, old_frames)) = self.fragments.remove(&id) {
                self.fragment_frames = self.fragment_frames.saturating_sub(old_frames.len());
            }
            self.fragment_frames += frames.len();
            self.fragments.insert(id, (frames.len() as u32, frames));
            return Ok(id);
        }

        Err(FragmentQueueError::does_not_need_split())
    }

    /// 把缓冲拆成带分片元信息的可靠有序帧；无需拆分时报错。
    ///
    /// Splits a buffer into reliable-ordered frames carrying fragment
    /// metadata; errors when no split is needed.
    ///
    /// - `buffer`: 待拆分的原始数据
    /// - `id`: 分片集合 ID
    /// - `mtu`: 可用的 MTU 大小
    pub fn split(buffer: &[u8], id: u16, mtu: u16) -> Result<Vec<Frame>, FragmentQueueError> {
        let Some(max_mtu) = mtu.checked_sub(RAKNET_HEADER_FRAME_OVERHEAD) else {
            return Err(FragmentQueueError::does_not_need_split());
        };

        if max_mtu == 0 {
            return Err(FragmentQueueError::does_not_need_split());
        }

        if buffer.len() > max_mtu.into() {
            let splits = buffer
                .chunks(max_mtu.into())
                .map(|c| c.to_vec())
                .collect::<Vec<Vec<u8>>>();
            if splits.len() > MAX_FRAGS as usize {
                return Err(FragmentQueueError::too_many_fragments());
            }
            let mut frames: Vec<Frame> = Vec::new();
            let mut index: u32 = 0;

            for buf in splits.iter() {
                let mut f = Frame::new(Reliability::ReliableOrd, Some(&buf[..]));
                f.fragment_meta = Some(FragmentMeta {
                    index,
                    size: splits.len() as u32,
                    id,
                });

                index += 1;

                frames.push(f);
            }

            return Ok(frames);
        }

        Err(FragmentQueueError::does_not_need_split())
    }

    /// 按 ID 借阅分片集合 (`size`, 帧列表)。
    ///
    /// Borrows the fragment set (`size`, frames) by id.
    pub fn get(&self, id: &u16) -> Result<&(u32, Vec<Frame>), FragmentQueueError> {
        if let Some(v) = self.fragments.get(id) {
            return Ok(v);
        }

        Err(FragmentQueueError::fragment_invalid())
    }

    /// 按 ID 可变借阅分片集合 (`size`, 帧列表)。
    ///
    /// Mutably borrows the fragment set (`size`, frames) by id.
    pub fn get_mut(&mut self, id: &u16) -> Result<&mut (u32, Vec<Frame>), FragmentQueueError> {
        if let Some(v) = self.fragments.get_mut(id) {
            return Ok(v);
        }

        Err(FragmentQueueError::fragment_invalid())
    }

    /// 按 ID 取走并移除整个分片集合。
    ///
    /// Takes and removes the whole fragment set by id.
    pub fn take(&mut self, id: u16) -> Result<(u32, Vec<Frame>), FragmentQueueError> {
        if let Some((size, frames)) = self.fragments.remove(&id) {
            self.fragment_frames = self.fragment_frames.saturating_sub(frames.len());
            return Ok((size, frames));
        }
        Err(FragmentQueueError::fragment_invalid())
    }

    /// 按 ID 移除分片集合；返回是否存在。
    ///
    /// Removes the fragment set by id; returns whether it existed.
    pub fn remove(&mut self, id: &u16) -> bool {
        if let Some((_, frames)) = self.fragments.remove(id) {
            self.fragment_frames = self.fragment_frames.saturating_sub(frames.len());
            true
        } else {
            false
        }
    }

    /// 硬清空分片队列，仅在内存紧张时使用。
    ///
    /// Hard-clears the fragment queue; this should only be used if memory
    /// becomes an issue.
    pub fn clear(&mut self) {
        self.fragment_id = 0;
        self.fragments.clear();
        self.fragments.shrink_to_fit();
        self.fragment_frames = 0;
    }
}

/// 分片重组失败。
///
/// Fragment reassembly failure.
#[derive(Debug, Clone, PartialEq, Eq, Hash, ScError)]
pub enum FragmentQueueError {
    /// 该序号的分片已接收过。
    ///
    /// A fragment with this index was already received.
    #[sc_error(code = "raknet.fragment.frame_exists")]
    FrameExists { detail: String, site: ScSite },
    /// 该帧不携带分片元信息。
    ///
    /// The frame carries no fragment metadata.
    #[sc_error(code = "raknet.fragment.frame_not_fragmented")]
    FrameNotFragmented { detail: String, site: ScSite },
    /// 缓冲可放入单个数据报，无需拆分。
    ///
    /// The buffer fits in one datagram.
    #[sc_error(code = "raknet.fragment.does_not_need_split")]
    DoesNotNeedSplit { detail: String, site: ScSite },
    /// 分片集合 ID 未知。
    ///
    /// The fragment set id is unknown.
    #[sc_error(code = "raknet.fragment.fragment_invalid")]
    FragmentInvalid { detail: String, site: ScSite },
    /// 该分片集合尚未收齐。
    ///
    /// Not all fragments of the set arrived yet.
    #[sc_error(code = "raknet.fragment.fragments_missing")]
    FragmentsMissing { detail: String, site: ScSite },
    /// 分片序号超出集合大小。
    ///
    /// The fragment index is outside the set size.
    #[sc_error(code = "raknet.fragment.frame_index_out_of_bounds")]
    FrameIndexOutOfBounds { detail: String, site: ScSite },
    /// 所需分片数超过允许上限。
    ///
    /// The buffer needs more fragments than allowed.
    #[sc_error(code = "raknet.fragment.too_many_fragments")]
    TooManyFragments { detail: String, site: ScSite },
    /// 无法再缓存更多分片集合。
    ///
    /// No more fragment sets can be buffered.
    #[sc_error(code = "raknet.fragment.queue_full")]
    QueueFull { detail: String, site: ScSite },
}

impl FragmentQueueError {
    /// 构造“帧已存在”错误，并捕获调用点。
    ///
    /// Builds a frame-exists error, capturing the call site.
    #[track_caller]
    pub fn frame_exists() -> Self {
        Self::FrameExists {
            detail: "Frame already exists".to_string(),
            site: capture_site(),
        }
    }

    /// 构造“帧未分片”错误，并捕获调用点。
    ///
    /// Builds a frame-not-fragmented error, capturing the call site.
    #[track_caller]
    pub fn frame_not_fragmented() -> Self {
        Self::FrameNotFragmented {
            detail: "Frame is not fragmented".to_string(),
            site: capture_site(),
        }
    }

    /// 构造“无需拆分”错误，并捕获调用点。
    ///
    /// Builds a does-not-need-split error, capturing the call site.
    #[track_caller]
    pub fn does_not_need_split() -> Self {
        Self::DoesNotNeedSplit {
            detail: "Frame does not need to be split".to_string(),
            site: capture_site(),
        }
    }

    /// 构造“分片非法”错误，并捕获调用点。
    ///
    /// Builds a fragment-invalid error, capturing the call site.
    #[track_caller]
    pub fn fragment_invalid() -> Self {
        Self::FragmentInvalid {
            detail: "Fragment is invalid".to_string(),
            site: capture_site(),
        }
    }

    /// 构造“分片缺失”错误，并捕获调用点。
    ///
    /// Builds a fragments-missing error, capturing the call site.
    #[track_caller]
    pub fn fragments_missing() -> Self {
        Self::FragmentsMissing {
            detail: "Fragments are missing".to_string(),
            site: capture_site(),
        }
    }

    /// 构造“分片序号越界”错误，并捕获调用点。
    ///
    /// Builds a frame-index-out-of-bounds error, capturing the call site.
    #[track_caller]
    pub fn frame_index_out_of_bounds() -> Self {
        Self::FrameIndexOutOfBounds {
            detail: "Frame index is out of bounds".to_string(),
            site: capture_site(),
        }
    }

    /// 构造“分片过多”错误，并捕获调用点。
    ///
    /// Builds a too-many-fragments error, capturing the call site.
    #[track_caller]
    pub fn too_many_fragments() -> Self {
        Self::TooManyFragments {
            detail: "Too many fragments".to_string(),
            site: capture_site(),
        }
    }

    /// 构造“队列已满”错误，并捕获调用点。
    ///
    /// Builds a queue-full error, capturing the call site.
    #[track_caller]
    pub fn queue_full() -> Self {
        Self::QueueFull {
            detail: "Fragment queue is full".to_string(),
            site: capture_site(),
        }
    }
}

impl std::fmt::Display for FragmentQueueError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FrameExists { detail, site } => {
                write!(formatter, "[raknet.fragment.frame_exists] {detail} (at {site})")
            }
            Self::FrameNotFragmented { detail, site } => {
                write!(
                    formatter,
                    "[raknet.fragment.frame_not_fragmented] {detail} (at {site})"
                )
            }
            Self::DoesNotNeedSplit { detail, site } => {
                write!(
                    formatter,
                    "[raknet.fragment.does_not_need_split] {detail} (at {site})"
                )
            }
            Self::FragmentInvalid { detail, site } => {
                write!(
                    formatter,
                    "[raknet.fragment.fragment_invalid] {detail} (at {site})"
                )
            }
            Self::FragmentsMissing { detail, site } => {
                write!(
                    formatter,
                    "[raknet.fragment.fragments_missing] {detail} (at {site})"
                )
            }
            Self::FrameIndexOutOfBounds { detail, site } => {
                write!(
                    formatter,
                    "[raknet.fragment.frame_index_out_of_bounds] {detail} (at {site})"
                )
            }
            Self::TooManyFragments { detail, site } => {
                write!(
                    formatter,
                    "[raknet.fragment.too_many_fragments] {detail} (at {site})"
                )
            }
            Self::QueueFull { detail, site } => {
                write!(formatter, "[raknet.fragment.queue_full] {detail} (at {site})")
            }
        }
    }
}

impl std::error::Error for FragmentQueueError {}


#[cfg(test)]
mod bounded_queue_tests {
    use super::*;

    fn frame_packet(payload: usize) -> FramePacket {
        let mut packet = FramePacket::new();
        let mut frame = Frame::new(Reliability::ReliableOrd, Some(&vec![7u8; payload][..]));
        frame.body = vec![7u8; payload];
        packet.frames.push(frame);
        packet
    }

    #[test]
    fn recovery_queue_refuses_instead_of_evicting_reliable_datagrams() {
        let mut queue: RecoveryQueue<FramePacket> =
            RecoveryQueue::with_limits(2, usize::MAX, Some(|p| p.queued_bytes()));
        assert!(queue.insert_id(1, frame_packet(8)).is_ok());
        assert!(queue.insert_id(2, frame_packet(8)).is_ok());

        let full = queue
            .insert_id(3, frame_packet(8))
            .expect_err("must refuse");
        assert_eq!(full.entries, 2);
        assert_eq!(full.max_entries, 2);
        assert_eq!(queue.len(), 2, "no unacknowledged datagram is evicted");
        assert!(queue.get_inclusive_range(1, 2, 8).len() == 2);
    }

    #[test]
    fn recovery_queue_byte_budget_rejects_and_accounts_exactly() {
        let packet = frame_packet(64);
        let packet_bytes = packet.queued_bytes();
        let mut queue: RecoveryQueue<FramePacket> =
            RecoveryQueue::with_limits(64, packet_bytes * 2, Some(|p| p.queued_bytes()));
        assert!(queue.insert_id(1, packet).is_ok());
        assert_eq!(queue.queued_bytes(), packet_bytes);
        let second = frame_packet(64);
        assert!(queue.insert_id(2, second).is_ok());
        assert_eq!(queue.queued_bytes(), packet_bytes * 2);
        assert!(queue.is_full());

        let refused = queue
            .insert_id(3, frame_packet(64))
            .expect_err("byte budget is exhausted");
        assert_eq!(refused.max_bytes, packet_bytes * 2);
        assert_eq!(refused.bytes, packet_bytes * 2);
        assert_eq!(queue.queued_bytes(), packet_bytes * 2);

        // ACKing frees exactly the accounted bytes.
        let _ = queue.remove(1);
        assert_eq!(queue.queued_bytes(), packet_bytes);
        assert!(queue.insert_id(3, frame_packet(64)).is_ok());
        assert_eq!(queue.queued_bytes(), packet_bytes * 2);
    }

    #[test]
    fn recovery_queue_replacing_a_sequence_keeps_byte_accounting_exact() {
        let mut queue: RecoveryQueue<FramePacket> =
            RecoveryQueue::with_limits(8, usize::MAX, Some(|p| p.queued_bytes()));
        let small = frame_packet(8).queued_bytes();
        let large = frame_packet(512).queued_bytes();
        queue.insert_id(7, frame_packet(8)).expect("insert");
        assert_eq!(queue.queued_bytes(), small);
        queue.insert_id(7, frame_packet(512)).expect("replace");
        assert_eq!(queue.queued_bytes(), large);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn recovery_queue_age_policy_is_explicit_and_frees_bytes() {
        let mut queue: RecoveryQueue<FramePacket> =
            RecoveryQueue::with_limits(8, usize::MAX, Some(|p| p.queued_bytes()));
        let item_bytes = frame_packet(16).queued_bytes();
        queue.insert_id(1, frame_packet(16)).expect("insert stale");
        queue.insert_id(2, frame_packet(16)).expect("insert fresh");
        // Age the first entry the way a silent peer would.
        if let Some((time, _)) = queue.queue.get_mut(&1) {
            *time = current_epoch().saturating_sub(60);
        }
        let accounted = queue.queued_bytes();
        assert_eq!(accounted, item_bytes * 2);

        let dropped = queue.flush_old(30);
        assert_eq!(dropped.len(), 1);
        assert_eq!(
            queue.queued_bytes(),
            accounted - frame_packet(16).queued_bytes(),
            "age eviction must release accounted bytes"
        );
        assert!(queue.oldest_unacked_age().is_some_and(|age| age < 30));
    }

    #[test]
    fn ordered_queue_reports_exhaustion_instead_of_silent_drop() {
        // A `Vec<u8>` item costs its payload plus the `Vec` header.
        let item_bytes = 16 + std::mem::size_of::<Vec<u8>>();
        let mut queue: OrderedQueue<Vec<u8>> = OrderedQueue::with_byte_budget(item_bytes * 2);
        assert!(queue.insert(0, vec![0u8; 16]).is_ok());
        assert!(queue.insert(1, vec![0u8; 16]).is_ok());
        assert_eq!(queue.queued_bytes(), item_bytes * 2);
        assert_eq!(
            queue.insert(0, vec![0u8; 16]),
            Err(OrderedQueueReject::Ignored)
        );

        let payload = vec![0u8; 48];
        let reject = queue.insert(2, payload).expect_err("byte budget exhausted");
        assert_eq!(
            reject,
            OrderedQueueReject::Exhausted {
                entries: 2,
                bytes: item_bytes * 2
            }
        );

        // Flushing the in-order prefix releases the accounted bytes.
        let flushed = queue.flush();
        assert_eq!(flushed.len(), 2);
        assert_eq!(queue.queued_bytes(), 0);
        assert!(queue.insert(2, vec![0u8; 48]).is_ok());
        assert_eq!(queue.queued_bytes(), 48 + std::mem::size_of::<Vec<u8>>());
    }
}

#[cfg(test)]
mod recovery_queue_tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn resend_budget_only_clones_and_resets_selected_packets() {
        let mut queue = RecoveryQueue::new();
        let old_timestamp = current_epoch().saturating_sub(10);
        for sequence in 0..5u32 {
            queue.queue.insert(sequence, (old_timestamp, sequence));
        }

        let first = queue.resend_old(2, 2);
        assert_eq!(first.len(), 2);
        let first_set: HashSet<_> = first.into_iter().collect();
        for (sequence, (timestamp, _)) in &queue.queue {
            if first_set.contains(sequence) {
                assert!(*timestamp > old_timestamp);
            } else {
                assert_eq!(*timestamp, old_timestamp);
            }
        }

        let second = queue.resend_old(2, 2);
        assert_eq!(second.len(), 2);
        assert!(second.iter().all(|sequence| !first_set.contains(sequence)));
        let second_set: HashSet<_> = second.into_iter().collect();

        let third = queue.resend_old(2, 2);
        assert_eq!(third.len(), 1);
        assert!(third
            .iter()
            .all(|sequence| !first_set.contains(sequence) && !second_set.contains(sequence)));
        assert!(queue.resend_old(2, 2).is_empty());
    }

    #[test]
    fn zero_resend_budget_leaves_expired_timestamps_untouched() {
        let mut queue = RecoveryQueue::new();
        let old_timestamp = current_epoch().saturating_sub(10);
        queue.queue.insert(7, (old_timestamp, 7));

        assert!(queue.resend_old(2, 0).is_empty());
        assert_eq!(
            queue.queue.get(&7).map(|(time, _)| *time),
            Some(old_timestamp)
        );
    }
}
