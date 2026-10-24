use std::collections::HashSet;

use crate::utils::{add24, forward_distance24, is_newer24, seq24, U24_HALF_RANGE};

const DATAGRAM_HISTORY_SIZE: u32 = 2048;

/// 保留 [`DATAGRAM_HISTORY_SIZE`] 大小的有界数据报序号历史，记录哪些数据报
/// 已收到，用于判定重复、缺失与超出窗口。
///
/// 重传可能在新的数据报序号中携带相同的可靠索引，因此数据报丢失本身并不
/// 意味着可靠消息出现缺口。
///
/// Bounded datagram-sequence history of [`DATAGRAM_HISTORY_SIZE`] slots tracking
/// which datagrams were received, used to classify duplicates, holes, and
/// out-of-window sequences.
///
/// Retransmission may carry the same reliable index in a new datagram, so a lost
/// datagram does not by itself imply a reliable-message gap.
#[derive(Debug, Clone)]
pub(crate) struct DatagramWindow {
    /// 窗口起点（最早仍被保留的序号）。
    ///
    /// Window start: the oldest sequence still retained.
    head: u32,
    /// 窗口终点的下一个位置（已见过的最大序号加一）。
    ///
    /// One past the window end: highest seen sequence plus one.
    next: u32,
    /// 按序号取模索引的“已收到”位表。
    ///
    /// Received flags, indexed by sequence modulo the history size.
    received: Box<[bool; DATAGRAM_HISTORY_SIZE as usize]>,
}

impl DatagramWindow {
    /// 创建空窗口。
    ///
    /// Creates an empty window.
    pub(crate) fn new() -> Self {
        Self {
            head: 0,
            next: 0,
            received: Box::new([false; DATAGRAM_HISTORY_SIZE as usize]),
        }
    }

    /// 返回当前窗口的 `(head, next)`。
    ///
    /// Returns the current `(head, next)` range.
    pub(crate) fn range(&self) -> (u32, u32) {
        (self.head, self.next)
    }

    /// 返回窗口（历史）容量。
    ///
    /// Returns the window (history) capacity.
    pub(crate) fn size(&self) -> u32 {
        DATAGRAM_HISTORY_SIZE
    }

    /// 序号 `index` 是否落在当前保留的历史区间内。
    ///
    /// Whether `index` lies inside the retained history range.
    pub(crate) fn contains(&self, index: u32) -> bool {
        forward_distance24(self.head, seq24(index)) < forward_distance24(self.head, self.next)
    }

    /// 序号 `index` 在窗口内但尚未收到。
    ///
    /// Whether `index` is inside the window but not yet received.
    pub(crate) fn is_missing(&self, index: u32) -> bool {
        self.contains(index) && !self.received[Self::slot(index)]
    }

    /// 计算序号在位表中的槽位（对历史大小取模）。
    ///
    /// Maps a sequence to its slot index (modulo the history size).
    fn slot(index: u32) -> usize {
        (seq24(index) % DATAGRAM_HISTORY_SIZE) as usize
    }

    /// 记录序号 `index` 已收到，并视需要推进窗口。
    ///
    /// Returns the [`WindowAccept`] outcome: newly accepted, duplicate, behind
    /// the retained history, or outside the window. Sequence numbers ahead of
    /// `next` slide the window forward, clearing the slots it passes over.
    pub(crate) fn try_insert(&mut self, index: u32) -> WindowAccept {
        let index = seq24(index);
        let empty = self.head == self.next;
        let reference = if empty {
            self.next
        } else {
            add24(self.next, u32::MAX)
        };
        let distance = forward_distance24(reference, index);
        if distance < U24_HALF_RANGE && (empty || distance != 0) {
            let advance = forward_distance24(self.next, index) + 1;
            if advance >= self.size() {
                self.received.fill(false);
            } else {
                for offset in 0..advance {
                    self.received[Self::slot(add24(self.next, offset))] = false;
                }
            }
            self.next = add24(index, 1);
            if forward_distance24(self.head, self.next) > self.size() {
                self.head = add24(self.next, 0u32.wrapping_sub(self.size()));
            }
        } else if distance == U24_HALF_RANGE {
            // Exactly half a cycle has no unambiguous direction.
            return WindowAccept::OutOfWindow;
        } else if !self.contains(index) {
            // History expired; the reliable-frame window decides admission.
            return WindowAccept::Behind;
        }

        let received = &mut self.received[Self::slot(index)];
        if *received {
            WindowAccept::Duplicate
        } else {
            *received = true;
            WindowAccept::Accepted
        }
    }
}

/// 向接收窗口提交一个 u24 序号的结果。
///
/// Outcome of offering a u24 sequence to a receive window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAccept {
    /// 在窗口内且首次见到；窗口可能已向前推进。
    ///
    /// Inside the window and newly seen; the window may have advanced.
    Accepted,
    /// 已见过且仍在历史中保留：可安全确认。
    ///
    /// Already seen and still retained: safe to acknowledge.
    Duplicate,
    /// 早于保留的历史；只有可靠帧窗口能证明其已交付。
    ///
    /// Behind retained history. Only the reliable-frame window proves delivery.
    Behind,
    /// 超出可靠接收范围，或在半程序号处方向不明确。
    ///
    /// Outside reliable admission or ambiguous at half a sequence cycle.
    OutOfWindow,
}

/// 按可靠消息索引推进的接收窗口；与 UDP 数据报序号是两套独立编号。
///
/// A receive window advanced by reliable message index; a separate numbering
/// from UDP datagram sequences.
#[derive(Debug, Clone)]
pub struct ReliableWindow {
    // The current window start and end
    window: (u32, u32),
    // The current window size
    size: u32,
    queue: HashSet<u32>,
}

impl ReliableWindow {
    /// 创建初始窗口（起点 0，容量 2048）。
    ///
    /// Creates the initial window (start 0, capacity 2048).
    pub fn new() -> Self {
        Self {
            window: (0, 2048),
            size: 2048,
            queue: HashSet::new(),
        }
    }

    /// 插入可靠索引，仅当首次接收时返回 `true`。
    ///
    /// Inserts a reliable index, returning `true` only for a first receipt.
    pub fn insert(&mut self, index: u32) -> bool {
        matches!(self.try_insert(index), WindowAccept::Accepted)
    }

    /// 接收一个可靠消息索引，但不丢弃未见的前缀。
    ///
    /// 超出窗口时按方向区分：仍在前方的返回 `OutOfWindow`，已落后的返回
    /// `Behind`。
    ///
    /// Admit a reliable-message index without discarding unseen gaps.
    ///
    /// Outside the window the outcome is direction-dependent: a sequence still
    /// ahead returns `OutOfWindow`, one already behind returns `Behind`.
    pub fn try_insert(&mut self, index: u32) -> WindowAccept {
        let index = seq24(index);
        // Only the forward half of the u24 space, inside the bounded window.
        let distance = forward_distance24(self.window.0, index);
        if distance >= self.size {
            // u24 arithmetic wraps, so a sequence behind the head also measures
            // as far beyond the window; separate the two by direction.
            return if is_newer24(index, self.window.0) || distance == U24_HALF_RANGE {
                WindowAccept::OutOfWindow
            } else {
                WindowAccept::Behind
            };
        }
        if self.queue.contains(&index) {
            return WindowAccept::Duplicate;
        }

        self.queue.insert(index);

        if index == self.window.0 {
            self.adjust();
        }

        return WindowAccept::Accepted;
    }

    /// 推进窗口起点，移除队列中已交付的索引，并把窗口校正为 `size` 宽。
    ///
    /// Attempts to adjust the window size, removing all out of date packets
    /// from the queue.
    pub fn adjust(&mut self) {
        // Drop the contiguous received prefix, advancing head and end with it.
        while self.queue.contains(&self.window.0) {
            self.queue.remove(&self.window.0);
            self.window.0 = add24(self.window.0, 1);
            self.window.1 = add24(self.window.1, 1);
        }

        // Keep the window exactly `size` wide.
        let curr_size = forward_distance24(self.window.0, self.window.1);
        if curr_size < self.size {
            self.window.1 = add24(self.window.0, self.size);
        } else if curr_size > self.size {
            self.window.0 = add24(self.window.1, 0u32.wrapping_sub(self.size));
        }
    }

    /// 列出前向窗口内缺失的可靠消息索引。
    ///
    /// Returns missing reliable-message indexes in the forward window.
    pub fn missing(&self) -> Vec<u32> {
        let mut missing = Vec::new();

        for offset in 0..self.size {
            let i = add24(self.window.0, offset);
            if !self.queue.contains(&i) {
                missing.push(i);
            }
        }

        missing
    }

    /// 返回当前窗口的 `(start, end)`。
    ///
    /// Returns the current `(start, end)` range.
    pub fn range(&self) -> (u32, u32) {
        self.window
    }

    /// 丢弃窗口之外的可靠索引，用于窗口容纳不下全部报文时。
    ///
    /// Forcefully clears packets that are not in the window.
    /// This is used when the window is too small to fit all the packets.
    pub fn clear_outdated(&mut self) {
        self.queue
            .retain(|k| forward_distance24(self.window.0, *k) < self.size);
    }

    /// 窗口起点的可靠索引是否仍缺失。
    ///
    /// Whether the next reliable-message index is missing.
    pub fn head_missing(&self) -> bool {
        !self.queue.contains(&self.window.0)
    }

    /// 从窗口起点到 `index` 的向前距离，上限为窗口大小。
    ///
    /// Forward distance from the head to `index`, clamped to the window size.
    pub fn distance_from_head(&self, index: u32) -> u32 {
        forward_distance24(self.window.0, seq24(index)).min(self.size)
    }

    /// 无法再保留新的可靠消息索引的边界。
    ///
    /// The bound past which new reliable-message indexes cannot be retained.
    pub fn size(&self) -> u32 {
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::{DatagramWindow, ReliableWindow, WindowAccept, DATAGRAM_HISTORY_SIZE};
    use crate::utils::{add24, U24_HALF_RANGE};

    #[test]
    fn datagram_window_slides_across_wrap_without_waiting_for_holes() {
        let mut window = DatagramWindow::new();
        window.head = 0x00ff_fffe;
        window.next = window.head;
        assert_eq!(window.try_insert(0), WindowAccept::Accepted);
        assert!(window.is_missing(0x00ff_fffe));
        assert!(window.is_missing(0x00ff_ffff));
        assert_eq!(window.try_insert(0x00ff_ffff), WindowAccept::Accepted);
        assert_eq!(window.try_insert(0x00ff_ffff), WindowAccept::Duplicate);
        assert_eq!(window.try_insert(0), WindowAccept::Duplicate);
        for index in 1..DATAGRAM_HISTORY_SIZE * 3 {
            assert_eq!(window.try_insert(index), WindowAccept::Accepted);
        }
        assert!(!window.contains(0x00ff_fffe));
        assert_eq!(window.try_insert(0x00ff_fffe), WindowAccept::Behind);
    }

    #[test]
    fn large_datagram_jumps_reset_bounded_history() {
        let mut window = DatagramWindow::new();
        assert_eq!(window.try_insert(0), WindowAccept::Accepted);
        assert_eq!(window.try_insert(1_000_000), WindowAccept::Accepted);
        let (head, next) = window.range();
        assert_eq!(head, next - DATAGRAM_HISTORY_SIZE);
        assert!(window.is_missing(head));
        assert_eq!(window.try_insert(head), WindowAccept::Accepted);
        assert_eq!(window.try_insert(0), WindowAccept::Behind);
        assert_eq!(window.try_insert(1_000_000), WindowAccept::Duplicate);
        assert_eq!(
            window.try_insert(add24(1_000_000, U24_HALF_RANGE)),
            WindowAccept::OutOfWindow
        );
        assert_eq!(window.range(), (head, next));
    }

    #[test]
    fn datagram_window_accepts_only_fresh_sequences_under_loss_reordering_and_wrap() {
        let origin = 0x00ff_f000u64;
        let mut next = origin;
        let mut head = origin;
        let mut seen = std::collections::HashSet::new();
        let mut window = DatagramWindow::new();
        window.head = origin as u32;
        window.next = origin as u32;
        let mut random = 17u64;

        for _ in 0..30_000 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let candidate = if random & 3 == 0 {
                next.saturating_sub(1 + (random >> 16) % 3000).max(origin)
            } else {
                next + (random >> 16) % 100
            };
            let expected = if candidate < head {
                WindowAccept::Behind
            } else if seen.contains(&candidate) {
                WindowAccept::Duplicate
            } else {
                next = next.max(candidate + 1);
                head = origin.max(next.saturating_sub(u64::from(DATAGRAM_HISTORY_SIZE)));
                seen.retain(|seq| *seq >= head);
                seen.insert(candidate);
                WindowAccept::Accepted
            };
            assert_eq!(window.try_insert(candidate as u32), expected);
            assert_eq!(
                window.range(),
                (super::seq24(head as u32), super::seq24(next as u32))
            );
            let probe = head + (random >> 32) % (next - head);
            assert_eq!(window.is_missing(probe as u32), !seen.contains(&probe));
        }
    }

    #[test]
    fn reliable_window_preserves_gaps_across_wrap() {
        let mut window = ReliableWindow::new();
        window.window = (0x00ff_fffe, add24(0x00ff_fffe, window.size));
        assert_eq!(window.try_insert(0), WindowAccept::Accepted);
        assert_eq!(window.try_insert(0), WindowAccept::Duplicate);
        assert_eq!(window.try_insert(0x00ff_fffe), WindowAccept::Accepted);
        assert_eq!(window.range().0, 0x00ff_ffff);
        assert_eq!(window.try_insert(0x00ff_ffff), WindowAccept::Accepted);
        assert_eq!(window.range().0, 1);
        assert_eq!(window.try_insert(0), WindowAccept::Behind);
        assert_eq!(window.try_insert(2049), WindowAccept::OutOfWindow);
        assert_eq!(window.try_insert(0x0080_0001), WindowAccept::OutOfWindow);
    }

    #[test]
    fn clear_outdated_handles_u24_wraparound() {
        let mut window = ReliableWindow::new();
        window.window = (0x00ff_fffe, 1);
        window.size = 3;
        window.queue.extend([0x00ff_fffd, 0x00ff_fffe, 0, 1, 2]);

        window.clear_outdated();

        assert!(!window.queue.contains(&0x00ff_fffd));
        assert!(window.queue.contains(&0x00ff_fffe));
        assert!(window.queue.contains(&0));
        assert!(!window.queue.contains(&1));
        assert!(!window.queue.contains(&2));
    }
}

/// 往返时间窗口（当前仅保留最近一次 RTT）。
///
/// Round-trip-time window (currently only the most recent RTT).
pub struct Window {
    // last round trip time
    pub rtt: u32,
}
