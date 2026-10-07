use std::collections::HashSet;

use crate::utils::{add24, forward_distance24, is_newer24, seq24, U24_HALF_RANGE};

const DATAGRAM_HISTORY_SIZE: u32 = 2048;

/// Datagram loss does not imply a reliable-message gap: retransmission may
/// carry the same reliable index in a new datagram. Retain bounded duplicate
/// history behind the newest datagram rather than waiting for every UDP slot.
#[derive(Debug, Clone)]
pub(crate) struct DatagramWindow {
    head: u32,
    next: u32,
    received: Box<[bool; DATAGRAM_HISTORY_SIZE as usize]>,
}

impl DatagramWindow {
    pub(crate) fn new() -> Self {
        Self {
            head: 0,
            next: 0,
            received: Box::new([false; DATAGRAM_HISTORY_SIZE as usize]),
        }
    }

    pub(crate) fn range(&self) -> (u32, u32) {
        (self.head, self.next)
    }

    pub(crate) fn size(&self) -> u32 {
        DATAGRAM_HISTORY_SIZE
    }

    pub(crate) fn contains(&self, index: u32) -> bool {
        forward_distance24(self.head, seq24(index)) < forward_distance24(self.head, self.next)
    }

    pub(crate) fn is_missing(&self, index: u32) -> bool {
        self.contains(index) && !self.received[Self::slot(index)]
    }

    fn slot(index: u32) -> usize {
        (seq24(index) % DATAGRAM_HISTORY_SIZE) as usize
    }

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
            // History has expired; this does not prove its reliable frames
            // were received. The frame window must decide their admission.
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

/// Outcome of offering a u24 sequence to a receive window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAccept {
    /// Inside the window and newly seen; the window may have advanced.
    Accepted,
    /// Already seen and still retained: safe to acknowledge.
    Duplicate,
    /// Behind retained history. Only the reliable-frame window proves delivery.
    Behind,
    /// Outside reliable admission or ambiguous at half a sequence cycle.
    OutOfWindow,
}

/// Reliable message indexes must never skip an unseen prefix: unlike UDP
/// sequences these identities survive retransmission in another datagram.
#[derive(Debug, Clone)]
pub struct ReliableWindow {
    // The current window start and end
    window: (u32, u32),
    // The current window size
    size: u32,
    queue: HashSet<u32>,
}

impl ReliableWindow {
    pub fn new() -> Self {
        Self {
            window: (0, 2048),
            size: 2048,
            queue: HashSet::new(),
        }
    }

    pub fn insert(&mut self, index: u32) -> bool {
        matches!(self.try_insert(index), WindowAccept::Accepted)
    }

    /// Admit a reliable-message index without discarding unseen gaps.
    pub fn try_insert(&mut self, index: u32) -> WindowAccept {
        let index = seq24(index);
        // The receiver accepts only packets in the forward half of the u24
        // sequence space and inside the bounded receive window.
        let distance = forward_distance24(self.window.0, index);
        if distance >= self.size {
            // u24 arithmetic wraps, so a sequence *behind* the head also
            // measures as far beyond the window. Separate the two by direction:
            // only a sequence ahead of the head is still expected.
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

        // we need to update the window to check if the is within it.
        if index == self.window.0 {
            self.adjust();
        }

        return WindowAccept::Accepted;
    }

    /// Attempts to adjust the window size, removing all out of date packets
    /// from the queue.
    pub fn adjust(&mut self) {
        // remove all packets that are out of date, that we got before the window,
        // increasing the window start and end if we can.
        while self.queue.contains(&self.window.0) {
            self.queue.remove(&self.window.0);
            self.window.0 = add24(self.window.0, 1);
            self.window.1 = add24(self.window.1, 1);
        }

        // if the window is too small or too big, make sure it's the right size.
        // corresponding to self.size
        let curr_size = forward_distance24(self.window.0, self.window.1);
        if curr_size < self.size {
            self.window.1 = add24(self.window.0, self.size);
        } else if curr_size > self.size {
            self.window.0 = add24(self.window.1, 0u32.wrapping_sub(self.size));
        }
    }

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

    pub fn range(&self) -> (u32, u32) {
        self.window
    }

    /// Forcefully clears packets that are not in the window.
    /// This is used when the window is too small to fit all the packets.
    pub fn clear_outdated(&mut self) {
        self.queue
            .retain(|k| forward_distance24(self.window.0, *k) < self.size);
    }

    /// Whether the next reliable-message index is missing.
    pub fn head_missing(&self) -> bool {
        !self.queue.contains(&self.window.0)
    }

    /// Forward distance from the head to `index`, clamped to the window size.
    ///
    pub fn distance_from_head(&self, index: u32) -> u32 {
        forward_distance24(self.window.0, seq24(index)).min(self.size)
    }

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
    fn datagram_history_matches_a_reference_under_loss_reordering_and_wrap() {
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

pub struct Window {
    // last round trip time
    pub rtt: u32,
}
