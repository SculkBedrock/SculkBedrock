use crate::connection::controller::window::{DatagramWindow, ReliableWindow, WindowAccept};
use crate::protocol::ack::{Ack, Ackable, Record, SingleRecord};
use crate::protocol::frame::{Frame, FramePacket};
use crate::protocol::reliability::Reliability;
use crate::protocol::MAX_FRAGS;
use crate::server::current_epoch;
use crate::utils::{add24, contains_inclusive24, forward_distance24, seq24};
use log::trace;
use std::collections::{HashMap, HashSet};

use super::{FragmentQueue, OrderedQueue, OrderedQueueReject};

const MAX_NACK_RECORDS: usize = 256;
const MAX_ACK_SEQUENCES_PER_FLUSH: usize = 1024;
const NACK_RETRY_INTERVAL_SECS: u64 = 1;

/// How long an ordered-delivery gap may persist before the connection is
/// declared unrecoverable.
///
/// A gap is normal for milliseconds: UDP reorders, the server NACKs the hole
/// every second, and the peer's retransmit fills it. When the gap outlives this
/// bound, nothing will ever fill it (the retransmit path already had its
/// chance every second), and every later frame on that channel is already
/// buffered behind it. Holding the connection only freezes both ends with zero
/// diagnostics — the buffering itself logs nothing — so the honest outcome is
/// to close and let the peer reconnect and resume.
pub(crate) const ORDERED_GAP_TTL_SECS: u64 = 10;

#[derive(Debug, Clone)]
pub enum RecvQueueError {
    OldSeq,
    /// Refuse to ACK a datagram whose reliable frames cannot be retained.
    ReliableWindowExhausted {
        index: u32,
        head: u32,
        size: u32,
    },
    /// An ordered channel could not buffer another frame.
    ///
    /// Terminal: ordered delivery has no gap tolerance, so the connection must
    /// be closed instead of stalling silently.
    OrderChannelExhausted {
        entries: usize,
        bytes: usize,
    },
}

#[derive(Debug, Clone)]
pub struct RecvQueue {
    frag_queue: FragmentQueue,
    pub(crate) window: DatagramWindow,
    pub(crate) reliable_window: ReliableWindow,
    order_channels: HashMap<u8, OrderedQueue<Vec<u8>>>,
    /// Bounded, deduplicated pending datagram acknowledgements.
    ack: HashSet<u32>,
    /// Missing sequence -> last time it was included in an outgoing NACK.
    /// Retained only within datagram history, at most `window.size()` entries.
    nack: HashMap<u32, u64>,
    ready: Vec<Vec<u8>>,
    terminal_error: Option<RecvQueueError>,
    /// Order channel -> epoch seconds when its current head gap started.
    ///
    /// A non-empty ordered queue with an old head gap is the silent-freeze
    /// shape: every datagram is still accepted and ACKed, `ready` stays empty,
    /// and nothing is logged. [`Self::stale_ordered_gap`] turns that shape
    /// into an explicit terminal instead of an eternal stall.
    gap_since: HashMap<u8, u64>,
    /// Cumulative receive-health counters (see [`RecvHealth`]).
    datagrams_in: u64,
    frames_ready: u64,
    oldseq_rejects: u64,
    /// Duplicates refused (already accounted for, so acknowledged).
    duplicates: u64,
    /// Datagrams with ambiguous u24 direction; never acknowledged.
    out_of_window: u64,
    ignored_ordered: u64,
}

/// Point-in-time receive health of one session, for logs and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RecvHealth {
    /// Datagrams that reached `insert` (accepted or not).
    pub datagrams_in: u64,
    /// Payloads delivered to `ready` for game-layer processing.
    pub frames_ready: u64,
    /// Datagrams rejected as duplicate or outside the receive window.
    pub oldseq_rejects: u64,
    /// Duplicates refused (already accounted for, acknowledged).
    pub duplicates: u64,
    /// Datagrams with ambiguous u24 direction; never acknowledged.
    pub out_of_window: u64,
    /// Ordered frames dropped as duplicate or beyond the gap bound.
    pub ignored_ordered: u64,
    /// Frames currently buffered behind an order gap.
    pub ordered_buffered: usize,
    /// Bytes currently buffered behind an order gap.
    pub ordered_buffered_bytes: usize,
    /// Datagram sequences currently awaiting retransmit.
    pub nack_pending: usize,
}

impl RecvQueue {
    pub fn new() -> Self {
        Self {
            frag_queue: FragmentQueue::new(),
            ack: HashSet::new(),
            nack: HashMap::new(),
            window: DatagramWindow::new(),
            reliable_window: ReliableWindow::new(),
            ready: Vec::new(),
            terminal_error: None,
            order_channels: HashMap::new(),
            gap_since: HashMap::new(),
            datagrams_in: 0,
            frames_ready: 0,
            oldseq_rejects: 0,
            duplicates: 0,
            out_of_window: 0,
            ignored_ordered: 0,
        }
    }

    pub fn insert(&mut self, packet: FramePacket) -> Result<(), RecvQueueError> {
        self.datagrams_in = self.datagrams_in.saturating_add(1);
        if let Some(error) = &self.terminal_error {
            return Err(error.clone());
        }
        let (old_head, old_next) = self.window.range();
        let sequence = seq24(packet.sequence);
        let mut expired = false;
        match self.window.try_insert(sequence) {
            WindowAccept::Accepted => {}
            WindowAccept::Duplicate => {
                self.duplicates = self.duplicates.saturating_add(1);
                self.oldseq_rejects = self.oldseq_rejects.saturating_add(1);
                // Already delivered and accounted for, so acknowledging it is
                // what stops the peer retransmitting something we have.
                self.queue_ack(sequence);
                return Err(RecvQueueError::OldSeq);
            }
            WindowAccept::Behind => {
                // A very late original datagram may contain the missing reliable
                // frame. Let reliable-index deduplication handle it, even after
                // its UDP sequence has fallen out of datagram history.
                expired = true;
            }
            WindowAccept::OutOfWindow => {
                self.out_of_window = self.out_of_window.saturating_add(1);
                self.oldseq_rejects = self.oldseq_rejects.saturating_add(1);

                return Err(RecvQueueError::OldSeq);
            }
        }
        self.nack.remove(&sequence);

        let (head, next) = self.window.range();
        if head != old_head {
            self.nack.retain(|seq, _| self.window.is_missing(*seq));
        }
        if next != old_next {
            // Discover only NEW gaps, never rebuild holes from the oldest
            // missing sequence (which re-NACKed already received datagrams).
            let start = if self.window.contains(old_next) {
                old_next
            } else {
                head
            };
            for offset in 0..forward_distance24(start, sequence) {
                self.nack.entry(add24(start, offset)).or_insert(0);
            }
        }
        for frame in &packet.frames {
            // Unreliable payloads outside history are stale and cannot be
            // deduplicated. Reliable payloads have their own durable prefix.
            if !expired || frame.reliable_index.is_some() {
                if let Err(error) = self.handle_frame(frame) {
                    self.terminal_error = Some(error.clone());
                    return Err(error);
                }
            }
        }
        // ACK after reliable-window and ordered-channel admission.
        self.queue_ack(sequence);
        Ok(())
    }

    fn queue_ack(&mut self, sequence: u32) {
        if self.ack.len() < MAX_ACK_SEQUENCES_PER_FLUSH {
            self.ack.insert(sequence);
        }
        // If a flush is overdue, the peer will retry unacknowledged reliable
        // data. Duplicate reception can ACK it on the next flush.
    }

    pub fn flush(&mut self) -> Vec<Vec<u8>> {
        let ready = self.ready.drain(..).collect::<Vec<Vec<u8>>>();
        self.frames_ready = self.frames_ready.saturating_add(ready.len() as u64);
        ready
    }

    pub fn ack_flush(&mut self) -> Vec<u32> {
        let mut sequences = Vec::with_capacity(MAX_ACK_SEQUENCES_PER_FLUSH);
        self.ack.retain(|seq| {
            if sequences.len() < MAX_ACK_SEQUENCES_PER_FLUSH {
                sequences.push(*seq);
                false
            } else {
                true
            }
        });
        sequences
    }

    pub fn nack_queue(&mut self) -> Vec<u32> {
        let now = current_epoch();
        let mut sequences = Vec::with_capacity(MAX_NACK_RECORDS);
        let head = self.window.range().0;
        let mut missing: Vec<_> = self.nack.iter().map(|(seq, time)| (*seq, *time)).collect();
        // Least recently requested first, then wire order: no hash iteration
        // starvation when more holes are pending than one control packet fits.
        missing.sort_unstable_by_key(|(seq, time)| (*time, forward_distance24(head, *seq)));
        for (sequence, last_sent) in missing {
            if last_sent != 0 && now.saturating_sub(last_sent) < NACK_RETRY_INTERVAL_SECS {
                continue;
            }
            sequences.push(sequence);
            self.nack.insert(sequence, now);
            if sequences.len() >= MAX_NACK_RECORDS {
                break;
            }
        }
        sequences
    }

    /// Clears all buffered data in the receive queue and releases allocated
    /// capacity. Called during `Connection::close()` to free fragment queues,
    /// ordered channel buffers, and ack/nack sets immediately — rather than
    /// waiting for the Arc-shared task futures to be dropped asynchronously.
    pub fn clear(&mut self) {
        self.frag_queue.clear();
        self.window = DatagramWindow::new();
        self.reliable_window = ReliableWindow::new();
        self.order_channels.clear();
        self.order_channels.shrink_to_fit();
        self.ack.clear();
        self.ack.shrink_to_fit();
        self.nack.clear();
        self.nack.shrink_to_fit();
        self.ready.clear();
        self.ready.shrink_to_fit();
        self.gap_since.clear();
        self.terminal_error = None;
    }

    fn handle_frame(&mut self, frame: &Frame) -> Result<(), RecvQueueError> {
        if let Some(reliable_index) = frame.reliable_index {
            match self.reliable_window.try_insert(reliable_index) {
                WindowAccept::Accepted => {}
                WindowAccept::Duplicate | WindowAccept::Behind => return Ok(()),
                WindowAccept::OutOfWindow => {
                    return Err(RecvQueueError::ReliableWindowExhausted {
                        index: seq24(reliable_index),
                        head: self.reliable_window.range().0,
                        size: self.reliable_window.size(),
                    });
                }
            }
        }

        if let Some(meta) = frame.fragment_meta.as_ref() {
            if meta.size > MAX_FRAGS {
                trace!("Fragment size is too large, rejected {}!", meta.size);
                return Ok(());
            }
            if let Err(_) = self.frag_queue.insert(frame.clone()) {}

            let res = self.frag_queue.collect(meta.id);
            if let Ok(data) = res {
                // Reconstructed frame packet. All fragments of one set share
                // the same order header, so the reassembled payload must go
                // through the same ordered dispatch as a non-fragmented
                // frame. Pushing straight to `ready` used to skip the
                // ordered queue, leaving its window stuck on this order
                // index and stalling every later packet on the channel
                // (e.g. the encrypted ClientToServerHandshake arriving
                // right after a fragmented Login was never delivered).
                self.dispatch(
                    frame.reliability,
                    frame.order_channel,
                    frame.order_index,
                    data,
                )?;
            } else {
                trace!("Still missing some fragments for id {}", meta.id);
            }
            return Ok(());
        }

        trace!(
            "RecvQueue: {}\n{:?}\n",
            frame.body.len(),
            frame.body.clone()
        );

        self.dispatch(
            frame.reliability,
            frame.order_channel,
            frame.order_index,
            frame.body.clone(),
        )
    }

    /// Snapshot of receive health for logs and tests.
    pub(crate) fn health(&self) -> RecvHealth {
        let (ordered_buffered, ordered_buffered_bytes) = self
            .order_channels
            .values()
            .fold((0, 0), |(entries, bytes), queue| {
                (entries + queue.len(), bytes + queue.queued_bytes())
            });
        RecvHealth {
            datagrams_in: self.datagrams_in,
            frames_ready: self.frames_ready,
            oldseq_rejects: self.oldseq_rejects,
            duplicates: self.duplicates,
            out_of_window: self.out_of_window,
            ignored_ordered: self.ignored_ordered,
            ordered_buffered,
            ordered_buffered_bytes,
            nack_pending: self.nack.len(),
        }
    }

    /// The oldest unhealed ordered-delivery gap, if it outlived its bound.
    ///
    /// Returns the channel and the gap's age in seconds. Normal reorder gaps
    /// heal through retransmission long before the bound; anything older is a
    /// hole nothing will fill, and the caller must close the connection rather
    /// than buffer behind it forever.
    pub fn stale_ordered_gap(&mut self, now: u64) -> Option<(u8, u64)> {
        for (channel, queue) in &self.order_channels {
            if !queue.is_empty() {
                self.gap_since.entry(*channel).or_insert(now);
            }
        }
        let mut worst: Option<(u8, u64)> = None;
        self.gap_since.retain(|channel, since| {
            let still_waiting = self
                .order_channels
                .get(channel)
                .is_some_and(|queue| !queue.is_empty());
            if !still_waiting {
                return false;
            }
            let age = now.saturating_sub(*since);
            if worst.is_none_or(|(_, worst_age)| age > worst_age) {
                worst = Some((*channel, age));
            }
            true
        });
        match worst {
            Some((channel, age)) if age >= ORDERED_GAP_TTL_SECS => Some((channel, age)),
            _ => None,
        }
    }

    /// Routes a frame payload (or a reassembled fragment set) through the
    /// reliability handling: ordered frames wait for their turn on their
    /// channel, everything else is delivered immediately.
    fn dispatch(
        &mut self,
        reliability: Reliability,
        order_channel: Option<u8>,
        order_index: Option<u32>,
        body: Vec<u8>,
    ) -> Result<(), RecvQueueError> {
        match reliability {
            Reliability::ReliableOrd => {
                let Some(channel) = order_channel else {
                    trace!("Ordered frame missing order channel");
                    self.ready.push(body);
                    return Ok(());
                };
                let Some(index) = order_index else {
                    trace!("Ordered frame missing order index");
                    self.ready.push(body);
                    return Ok(());
                };
                let queue = self
                    .order_channels
                    .entry(channel)
                    .or_insert(OrderedQueue::new());

                // A rejected ordered frame is not harmless: dropping it leaves a
                // permanent hole, so every later frame on the channel stalls.
                // Report it as a terminal receive error instead.
                match queue.insert(index, body) {
                    Ok(()) => {
                        for pk in queue.flush() {
                            self.ready.push(pk);
                        }
                    }
                    Err(OrderedQueueReject::Ignored) => {
                        // A silently swallowed frame still leaves its hole
                        // behind: counted here so a stall after mass drops is
                        // diagnosable instead of invisible.
                        self.ignored_ordered = self.ignored_ordered.saturating_add(1);
                    }
                    Err(OrderedQueueReject::Exhausted { entries, bytes }) => {
                        return Err(RecvQueueError::OrderChannelExhausted { entries, bytes });
                    }
                }
            }
            _ => {
                self.ready.push(body);
            }
        }
        Ok(())
    }
}

impl Ackable for RecvQueue {
    type NackItem = ();

    fn ack(&mut self, ack: Ack) {
        if ack.is_nack() {
            trace!("Invalid ack: {:?}", ack.clone());
            return;
        }

        trace!("Got ack item: {:?}", ack.clone());

        // these packets are acknowledged, so we can remove them from the queue.
        // RakNet range records [start, end] are INCLUSIVE on both ends.
        for record in ack.records.iter() {
            match record {
                Record::Single(SingleRecord { sequence }) => {
                    self.nack.remove(&seq24((*sequence).into()));
                }
                Record::Range(ranged) => {
                    let start = seq24(ranged.start.into());
                    let end = seq24(ranged.end.into());
                    self.nack
                        .retain(|seq, _| !contains_inclusive24(start, end, *seq));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ordered_frame(order_index: u32, byte: u8) -> Frame {
        let mut frame = Frame::init();
        frame.reliability = Reliability::ReliableOrd;
        frame.order_channel = Some(0);
        frame.order_index = Some(order_index);
        frame.body = vec![byte];
        frame
    }

    /// An unreliable frame, so a test exercises one mechanism at a time.
    fn unordered_frame(byte: u8) -> Frame {
        let mut frame = Frame::init();
        frame.reliability = Reliability::Unreliable;
        frame.body = vec![byte];
        frame
    }

    fn datagram(sequence: u32, frames: Vec<Frame>) -> FramePacket {
        let mut packet = FramePacket::new();
        packet.sequence = sequence;
        packet.frames = frames;
        packet
    }

    #[test]
    fn a_permanently_lost_datagram_does_not_stop_later_delivery() {
        let mut queue = RecvQueue::new();
        // An unreliable datagram is never retransmitted. Its sequence is not
        // an ordering dependency for any subsequent payload.
        for sequence in 1..20_000 {
            queue
                .insert(datagram(sequence, vec![unordered_frame(0xA)]))
                .expect("a datagram gap must not pin the receive window");
            assert_eq!(queue.flush(), vec![vec![0xA]]);
            assert!(queue.ack_flush().contains(&sequence));
        }
    }

    #[test]
    fn reordered_datagrams_do_not_reintroduce_received_nacks() {
        let mut queue = RecvQueue::new();
        queue.insert(datagram(2, vec![])).unwrap();
        queue.insert(datagram(1, vec![])).unwrap();
        queue.insert(datagram(3, vec![])).unwrap();
        assert_eq!(queue.nack_queue(), vec![0]);
    }

    #[test]
    fn an_ordered_gap_buffers_silently_then_turns_terminal_after_ttl() {
        let mut queue = RecvQueue::new();
        let now = current_epoch();

        // order_index 0 never arrives: index 1 buffers behind the gap and
        // nothing is delivered, with no error and no log.
        queue
            .insert(datagram(0, vec![ordered_frame(1, 0xA)]))
            .expect("datagram accepted");
        assert!(queue.flush().is_empty());
        let health = queue.health();
        assert_eq!(health.datagrams_in, 1);
        assert_eq!(health.frames_ready, 0);
        assert_eq!(health.ordered_buffered, 1);

        // A fresh gap is normal reorder weather, not a stall.
        assert_eq!(queue.stale_ordered_gap(now), None);
        // Past the TTL the hole is permanent: retransmission had its chance
        // every second and never filled it.
        let (channel, age) = queue
            .stale_ordered_gap(now + ORDERED_GAP_TTL_SECS)
            .expect("stale gap must be terminal");
        assert_eq!(channel, 0);
        assert!(age >= ORDERED_GAP_TTL_SECS);
    }

    #[test]
    fn a_healed_gap_never_turns_terminal() {
        let mut queue = RecvQueue::new();
        let now = current_epoch();

        queue
            .insert(datagram(0, vec![ordered_frame(1, 0xA)]))
            .expect("datagram accepted");
        assert!(queue.flush().is_empty());
        queue
            .insert(datagram(1, vec![ordered_frame(0, 0xB)]))
            .expect("datagram accepted");
        // The missing head arrives: both frames deliver in order.
        assert_eq!(queue.flush(), vec![vec![0xB], vec![0xA]]);
        assert_eq!(
            queue.stale_ordered_gap(now + ORDERED_GAP_TTL_SECS * 10),
            None
        );
        assert_eq!(queue.health().ordered_buffered, 0);
    }

    #[test]
    fn in_order_datagrams_keep_delivering() {
        let mut queue = RecvQueue::new();
        for sequence in 0..20_000u32 {
            queue
                .insert(datagram(sequence, vec![unordered_frame(0xA)]))
                .expect("in-order datagrams accepted");
            assert_eq!(queue.flush(), vec![vec![0xA]]);
            queue.ack_flush();
            assert!(queue.nack.is_empty());
        }
        assert_eq!(queue.health().oldseq_rejects, 0);
    }

    #[test]
    fn nack_budget_is_bounded_and_fair() {
        let mut queue = RecvQueue::new();
        queue.insert(datagram(1000, vec![])).unwrap();
        assert_eq!(queue.nack_queue(), (0..256).collect::<Vec<_>>());
        assert_eq!(queue.nack_queue(), (256..512).collect::<Vec<_>>());
        // Pretend the first batch has reached its retry deadline. Unrequested
        // holes must still get their first request before retrying that batch.
        for seq in 0..256 {
            queue.nack.insert(seq, current_epoch().saturating_sub(1));
        }
        assert_eq!(queue.nack_queue(), (512..768).collect::<Vec<_>>());
        queue.insert(datagram(1_000_000, vec![])).unwrap();
        assert!(queue.nack.len() < queue.window.size() as usize);
        assert!(!queue.nack.contains_key(&0));
    }

    #[test]
    fn reliable_frame_can_be_retransmitted_in_a_new_datagram() {
        let mut queue = RecvQueue::new();
        let mut later = ordered_frame(1, 0xB);
        later.reliable_index = Some(1);
        queue.insert(datagram(1, vec![later])).unwrap();
        assert!(queue.flush().is_empty());
        for sequence in 2..5000 {
            queue.insert(datagram(sequence, vec![])).unwrap();
            queue.ack_flush();
        }
        let mut missing = ordered_frame(0, 0xA);
        missing.reliable_index = Some(0);
        queue.insert(datagram(5000, vec![missing.clone()])).unwrap();
        assert_eq!(queue.flush(), vec![vec![0xA], vec![0xB]]);
        queue.insert(datagram(5001, vec![missing])).unwrap();
        assert!(queue.flush().is_empty());
        assert!(queue.ack_flush().contains(&5001));
    }

    #[test]
    fn duplicates_are_acked_and_newer_datagrams_slide_the_window() {
        let mut queue = RecvQueue::new();

        queue
            .insert(datagram(0, vec![unordered_frame(0xA)]))
            .expect("datagram accepted");
        queue
            .insert(datagram(1, vec![unordered_frame(0xB)]))
            .expect("datagram accepted");
        let acked = queue.ack_flush();
        assert!(acked.contains(&0) && acked.contains(&1));

        // A duplicate is already accounted for: acknowledging it is what makes
        // the peer stop retransmitting something we have.
        assert!(queue
            .insert(datagram(1, vec![unordered_frame(0xB)]))
            .is_err());
        assert!(
            queue.ack_flush().contains(&1),
            "a duplicate must be acknowledged"
        );
        assert_eq!(queue.health().duplicates, 1);

        queue
            .insert(datagram(5000, vec![unordered_frame(0xC)]))
            .unwrap();
        assert!(queue.ack_flush().contains(&5000));
        assert_eq!(queue.health().out_of_window, 0);
    }

    #[test]
    fn an_expired_original_datagram_can_still_fill_a_reliable_gap() {
        let mut queue = RecvQueue::new();
        let mut later = ordered_frame(1, 0xB);
        later.reliable_index = Some(1);
        queue.insert(datagram(5000, vec![later])).unwrap();
        let mut missing = ordered_frame(0, 0xA);
        missing.reliable_index = Some(0);
        let original = datagram(0, vec![missing, unordered_frame(0xC)]);
        queue.insert(original.clone()).unwrap();
        assert_eq!(queue.flush(), vec![vec![0xA], vec![0xB]]);
        assert!(queue.ack_flush().contains(&0));
        queue.insert(original).unwrap();
        assert!(queue.flush().is_empty());
    }

    #[test]
    fn reliable_window_exhaustion_is_terminal_and_is_not_acked() {
        let mut queue = RecvQueue::new();
        let mut frame = ordered_frame(0, 0xA);
        frame.reliable_index = Some(2048);
        assert!(matches!(
            queue.insert(datagram(0, vec![frame])),
            Err(RecvQueueError::ReliableWindowExhausted {
                index: 2048,
                head: 0,
                size: 2048
            })
        ));
        assert!(queue.ack_flush().is_empty());
        assert!(queue.flush().is_empty());
        assert!(matches!(
            queue.insert(datagram(0, vec![])),
            Err(RecvQueueError::ReliableWindowExhausted { .. })
        ));
        assert!(queue.ack_flush().is_empty());
    }

    #[test]
    fn ack_backlog_is_bounded_and_can_recover_through_duplicate_reception() {
        let mut queue = RecvQueue::new();
        for sequence in 0..1500 {
            queue.insert(datagram(sequence, vec![])).unwrap();
        }
        assert_eq!(queue.ack.len(), MAX_ACK_SEQUENCES_PER_FLUSH);
        assert_eq!(queue.ack_flush().len(), MAX_ACK_SEQUENCES_PER_FLUSH);
        assert!(queue.insert(datagram(1499, vec![])).is_err());
        assert_eq!(queue.ack_flush(), vec![1499]);
    }

    #[test]
    fn nacks_keep_wire_sequences_correct_across_u24_wrap() {
        let mut queue = RecvQueue::new();
        queue.insert(datagram(0x007f_ffff, vec![])).unwrap();
        queue.insert(datagram(0x00ff_fffe, vec![])).unwrap();
        queue.insert(datagram(1, vec![])).unwrap();
        queue.insert(datagram(0, vec![])).unwrap();
        queue.nack.retain(|seq, _| *seq == 0x00ff_ffff || *seq <= 1);
        assert_eq!(queue.nack_queue(), vec![0x00ff_ffff]);
        queue.insert(datagram(0x00ff_ffff, vec![])).unwrap();
        assert!(queue.nack_queue().is_empty());
    }

    #[test]
    fn a_late_fragment_completes_reassembly_and_ordered_delivery() {
        use crate::protocol::frame::FragmentMeta;
        let mut queue = RecvQueue::new();
        let mut first = ordered_frame(0, 0xA);
        first.reliable_index = Some(0);
        first.fragment_meta = Some(FragmentMeta {
            size: 2,
            id: 7,
            index: 0,
        });
        let mut last = ordered_frame(0, 0xB);
        last.reliable_index = Some(1);
        last.fragment_meta = Some(FragmentMeta {
            size: 2,
            id: 7,
            index: 1,
        });
        queue.insert(datagram(1, vec![last])).unwrap();
        let mut later = ordered_frame(1, 0xC);
        later.reliable_index = Some(2);
        queue.insert(datagram(5000, vec![later])).unwrap();
        assert!(queue.flush().is_empty());
        queue.insert(datagram(0, vec![first])).unwrap();
        assert_eq!(queue.flush(), vec![vec![0xA, 0xB], vec![0xC]]);
    }

    #[test]
    fn ordered_capacity_failure_is_reported_on_the_same_datagram_without_ack() {
        let mut queue = RecvQueue::new();
        let mut frame = ordered_frame(1, 0xA);
        frame.body = vec![0; super::super::MAX_ORDERED_QUEUE_BYTES + 1];
        assert!(matches!(
            queue.insert(datagram(0, vec![frame])),
            Err(RecvQueueError::OrderChannelExhausted { .. })
        ));
        assert!(queue.ack_flush().is_empty());
    }

    #[test]
    fn a_retransmitted_datagram_is_acked_without_redelivery() {
        let mut queue = RecvQueue::new();

        queue
            .insert(datagram(0, vec![unordered_frame(0xA)]))
            .expect("datagram accepted");
        queue
            .insert(datagram(1, vec![unordered_frame(0xB)]))
            .expect("datagram accepted");
        queue.flush();
        queue.ack_flush();

        // Retained duplicate history prevents unreliable redelivery.
        assert!(queue
            .insert(datagram(0, vec![unordered_frame(0xA)]))
            .is_err());
        assert!(
            queue.ack_flush().contains(&0),
            "a datagram behind the head was already delivered and must be acked"
        );
        assert_eq!(queue.health().duplicates, 1);
        assert_eq!(
            queue.health().out_of_window,
            0,
            "a behind datagram must not be counted as still-expected traffic"
        );
    }

    #[test]
    fn health_counts_rejects_and_drops() {
        let mut queue = RecvQueue::new();

        queue
            .insert(datagram(0, vec![ordered_frame(0, 0xA)]))
            .expect("datagram accepted");
        assert_eq!(queue.flush(), vec![vec![0xA]]);

        // Same datagram sequence twice: duplicate, rejected.
        assert!(queue
            .insert(datagram(0, vec![ordered_frame(1, 0xB)]))
            .is_err());
        // Same order index behind the window: ignored.
        queue
            .insert(datagram(1, vec![ordered_frame(0, 0xC)]))
            .expect("datagram accepted");
        assert!(queue.flush().is_empty());

        let health = queue.health();
        assert_eq!(health.datagrams_in, 3);
        assert_eq!(health.frames_ready, 1);
        assert_eq!(health.oldseq_rejects, 1);
        assert_eq!(health.ignored_ordered, 1);
    }

    #[test]
    fn nack_queue_suppresses_retries_until_interval() {
        let mut queue = RecvQueue::new();
        queue.nack.insert(42, 0);

        assert_eq!(queue.nack_queue(), vec![42]);

        queue.nack.insert(42, current_epoch().saturating_add(100));
        assert!(queue.nack_queue().is_empty());

        queue
            .nack
            .insert(42, current_epoch().saturating_sub(NACK_RETRY_INTERVAL_SECS));
        assert_eq!(queue.nack_queue(), vec![42]);
    }
}
