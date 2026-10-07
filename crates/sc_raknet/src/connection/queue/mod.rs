pub(crate) mod recv;
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
use std::collections::BTreeMap;
use std::collections::HashMap;

pub const MAX_RECOVERY_QUEUE_ITEMS: usize = 65_536;
/// Byte budget for unacknowledged datagrams per connection.
///
/// The item limit alone is not a memory bound: at a full-size MTU,
/// `MAX_RECOVERY_QUEUE_ITEMS` frames would retain hundreds of MiB per client.
/// This budget is the real per-connection working-set limit (§13.2).
pub const MAX_RECOVERY_QUEUE_BYTES: usize = 16 * 1024 * 1024;
const MAX_ORDERED_QUEUE_GAP: u32 = 4096;
const MAX_ORDERED_QUEUE_DEPTH: usize = 4096;
/// Byte budget for buffered inbound ordered frames per connection/channel.
pub const MAX_ORDERED_QUEUE_BYTES: usize = 4 * 1024 * 1024;
const MAX_FRAGMENT_SETS: usize = 128;
const MAX_FRAGMENT_FRAMES: usize = 4096;

/// Items that can report the memory they retain inside a network queue.
pub trait QueuedItem {
    /// Owned bytes retained by this item (payload plus queue bookkeeping).
    fn queued_bytes(&self) -> usize;
}

impl QueuedItem for FramePacket {
    fn queued_bytes(&self) -> usize {
        let payload: usize = self
            .frames
            .iter()
            .map(|frame| frame.body.len() + frame_overhead_bytes())
            .sum();
        payload + std::mem::size_of::<FramePacket>()
    }
}

fn frame_overhead_bytes() -> usize {
    std::mem::size_of::<Frame>() + usize::from(RAKNET_HEADER_FRAME_OVERHEAD)
}

/// The recovery queue refused an insertion because a bound is exhausted.
///
/// Reliable datagrams are world facts for the peer: the queue must not evict an
/// older unacknowledged datagram to make room, because that silently creates a
/// permanent hole in the peer's reassembly. The caller has to treat this as a
/// terminal connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryQueueFull {
    pub entries: usize,
    pub bytes: usize,
    pub item_bytes: usize,
    pub max_entries: usize,
    pub max_bytes: usize,
}

impl std::fmt::Display for RecoveryQueueFull {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "recovery queue full ({} entries/{}, {} bytes/{}, next item {} bytes)",
            self.entries, self.max_entries, self.bytes, self.max_bytes, self.item_bytes
        )
    }
}

#[derive(Debug, Clone)]
pub enum NetQueueError<E> {
    /// The insertion failed for any given reason.
    InvalidInsertion,
    /// The insertion failed and the reason is known.
    InvalidInsertionKnown(String),
    /// The `Item` failed to be removed from the queue.
    ItemDeletionFail,
    /// The `Item` is invalid and can not be retrieved.
    InvalidItem,
    /// The queue is empty.
    EmptyQueue,
    /// The error is a custom error.
    Other(E),
}

pub trait NetQueue<Item> {
    /// The `Item` of the queue.
    // type Item = V;

    /// The "key" that each `Item` is stored under
    /// (used for removal)
    type KeyId;

    /// A custom error specifier for NetQueueError
    type Error;

    /// Inserts `Item` into the queue, given the conditions are fulfilled.
    fn insert(&mut self, item: Item) -> Result<Self::KeyId, NetQueueError<Self::Error>>;

    /// Remove an `Item` from the queue by providing an instance of `Self::KeyId`
    fn remove(&mut self, key: Self::KeyId) -> Result<Item, NetQueueError<Self::Error>>;

    /// Retrieves an `Item` from the queue, by reference.
    fn get(&mut self, key: Self::KeyId) -> Result<&Item, NetQueueError<Self::Error>>;

    /// Clears the entire queue.
    fn flush(&mut self) -> Result<Vec<Item>, NetQueueError<Self::Error>>;
}

/// A recovery queue is used to store packets that need to be resent.
/// This is used for sequenced and ordered packets.
#[derive(Debug, Clone)]
pub struct RecoveryQueue<Item> {
    /// The current queue of packets by timestamp
    /// (seq, (packet, timestamp))
    // TODO use the timestamp for round trip time (RTT)
    queue: HashMap<u32, (u64, Item)>,
    /// Owned bytes retained by the queued items (structural estimate).
    queued_bytes: usize,
    max_items: usize,
    max_bytes: usize,
    /// Optional owned-byte sizer; `None` disables byte admission.
    sizer: Option<fn(&Item) -> usize>,
}

impl<Item> RecoveryQueue<Item>
where
    Item: Clone,
{
    pub fn new() -> Self {
        Self::with_limits(MAX_RECOVERY_QUEUE_ITEMS, usize::MAX, None)
    }

    /// Bounded queue. `sizer` enables the byte budget; without it only the item
    /// budget applies (used by tests and non-payload queues).
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

    /// Track a datagram for retransmission.
    ///
    /// Returns [`RecoveryQueueFull`] instead of evicting an older unacknowledged
    /// datagram: reliable data must not silently disappear from the peer's
    /// stream, so a saturated queue is a terminal state for the connection.
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

    fn item_bytes(&self, item: &Item) -> usize {
        self.sizer.map(|sizer| sizer(item)).unwrap_or(0)
    }

    /// Number of unacknowledged datagrams.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Owned bytes retained by unacknowledged datagrams.
    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    pub fn max_items(&self) -> usize {
        self.max_items
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Whether the next insertion is guaranteed to be refused.
    pub fn is_full(&self) -> bool {
        self.queue.len() >= self.max_items || self.queued_bytes >= self.max_bytes
    }

    /// Age in seconds of the oldest unacknowledged datagram.
    pub fn oldest_unacked_age(&self) -> Option<u64> {
        self.queue
            .values()
            .map(|(time, _)| current_epoch().saturating_sub(*time))
            .max()
    }

    pub fn get_all(&mut self) -> Vec<(u32, Item)> {
        self.queue
            .iter()
            .map(|(seq, (_, item))| (*seq, item.clone()))
            .collect::<Vec<_>>()
    }

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

    /// Drop datagrams unacknowledged for longer than `threshold` seconds.
    ///
    /// This is an explicit age policy, not silent eviction: the peer stopped
    /// acknowledging, so the retained bytes are pure overhead. Callers are
    /// expected to close the connection instead of pretending the reliable
    /// stream is still intact.
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

    /// Returns at most `limit` packets that have been unacknowledged for longer
    /// than `threshold` seconds and resets timestamps only for the selected
    /// packets. Unselected expired entries remain immediately eligible on the
    /// next tick, preventing the send loop's per-tick budget from postponing
    /// packets it did not actually attempt.
    ///
    /// Packets are **NOT removed** — they stay in the queue until an explicit
    /// ACK removes them. This preserves NACK-driven recovery.
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

    /// Clears all entries from the recovery queue and releases the allocated
    /// capacity. Called during `Connection::close()` to free the large buffers
    /// (potentially thousands of FramePackets from a resource pack download)
    /// immediately rather than waiting for the Arc-shared task futures to be
    /// dropped asynchronously by the tokio runtime.
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

    fn insert(&mut self, item: Item) -> Result<Self::KeyId, NetQueueError<Self::Error>> {
        let start = seq24(self.queue.len() as u32);
        let mut index = start;
        while self.queue.contains_key(&index) {
            index = seq24(index.wrapping_add(1));
            if index == start {
                return Err(NetQueueError::InvalidInsertionKnown(
                    "recovery queue key space exhausted".to_string(),
                ));
            }
        }
        self.insert_id(index, item)
            .map_err(|full| NetQueueError::InvalidInsertionKnown(full.to_string()))?;
        Ok(index)
    }

    fn remove(&mut self, key: Self::KeyId) -> Result<Item, NetQueueError<Self::Error>> {
        if let Some((_, item)) = self.queue.remove(&key) {
            self.queued_bytes = self.queued_bytes.saturating_sub(self.item_bytes(&item));
            Ok(item)
        } else {
            Err(NetQueueError::ItemDeletionFail)
        }
    }

    fn get(&mut self, key: Self::KeyId) -> Result<&Item, NetQueueError<Self::Error>> {
        if let Some((_, item)) = self.queue.get(&key) {
            Ok(item)
        } else {
            Err(NetQueueError::ItemDeletionFail)
        }
    }

    fn flush(&mut self) -> Result<Vec<Item>, NetQueueError<Self::Error>> {
        let mut items = Vec::new();
        for (_, (_, item)) in self.queue.drain() {
            items.push(item);
        }
        self.queued_bytes = 0;
        Ok(items)
    }
}

/// An ordered queue is used to Index incoming packets over a channel
/// within a reliable window time.
///
/// Usage:
/// ```ignore
/// use rak_rs::connection::queue::OrderedQueue;
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
    /// The current ordered queue channels
    /// Channel, (Highest Index, Ord Index, Item)
    pub queue: BTreeMap<u32, Item>,
    /// The window for this queue.
    pub window: (u32, u32),
    /// Owned bytes retained by buffered out-of-order frames.
    queued_bytes: usize,
    max_bytes: usize,
}

impl QueuedItem for Vec<u8> {
    fn queued_bytes(&self) -> usize {
        self.len() + std::mem::size_of::<Self>()
    }
}

/// Why an ordered frame was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderedQueueReject {
    /// Duplicate or behind the current window; harmless.
    Ignored,
    /// A bound is exhausted, so the channel has a permanent hole.
    ///
    /// Ordered delivery cannot skip an index: once a frame is dropped, every
    /// later frame on that channel stalls forever. This must terminate the
    /// connection instead of being swallowed.
    Exhausted { entries: usize, bytes: usize },
}

impl<Item> OrderedQueue<Item>
where
    Item: Clone + std::fmt::Debug + QueuedItem,
{
    pub fn new() -> Self {
        Self::with_byte_budget(MAX_ORDERED_QUEUE_BYTES)
    }

    pub fn with_byte_budget(max_bytes: usize) -> Self {
        Self {
            queue: BTreeMap::new(),
            window: (0, 0),
            queued_bytes: 0,
            max_bytes,
        }
    }

    /// Owned bytes currently buffered for out-of-order delivery.
    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    pub fn next(&mut self) -> u32 {
        self.window.0 = self.window.0.wrapping_add(1);
        self.window.0
    }

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

    fn current_item_bytes(&self, index: u32) -> usize {
        self.queue.get(&index).map(Item::queued_bytes).unwrap_or(0)
    }

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

    /// Forcefully flushes the incoming queue resetting the highest window
    /// to the lowest window.
    ///
    /// THIS IS A PATCH FIX UNTIL I CAN FIGURE OUT WHY THE OTHER FLUSH IS BROKEN
    pub fn flush(&mut self) -> Vec<Item> {
        let mut items = Vec::new();

        while let Some(item) = self.queue.remove(&self.window.0) {
            self.queued_bytes = self.queued_bytes.saturating_sub(item.queued_bytes());
            items.push(item);
            self.window.0 = add24(self.window.0, 1);
        }
        items
    }

    /// Frames currently buffered behind the channel's head gap.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Older, broken implementation, idk what is causing this to break
    /// after index 3
    /// The logic here is supposed to be, remove all indexes until the highest most up to date index.
    /// and retain older indexes until the order is correct.
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

/// A specialized structure for re-ordering fragments over the wire.
/// You can use this structure to fragment frames as well.
///
/// **NOTE:** This structure will NOT update a frame's reliable index!
/// The sender is required to this!
#[derive(Clone, Debug)]
pub struct FragmentQueue {
    /// The current fragment id to use
    /// If for some reason this wraps back to 0,
    /// and the fragment queue is full, 0 is then cleared and reused.
    fragment_id: u16,

    /// The current Fragments
    /// Hashmap is by Fragment id, with the value being
    /// (`size`, Vec<Frame>)
    fragments: HashMap<u16, (u32, Vec<Frame>)>,
    fragment_frames: usize,
}

impl FragmentQueue {
    pub fn new() -> Self {
        Self {
            fragment_id: 0,
            fragments: HashMap::new(),
            fragment_frames: 0,
        }
    }

    /// Inserts the frame into the fragment queue.
    /// Returns a result tuple of (`fragment_size`, `fragment_index`)
    pub fn insert(&mut self, fragment: Frame) -> Result<(u32, u32), FragmentQueueError> {
        if let Some(meta) = fragment.fragment_meta.clone() {
            if meta.size == 0 || meta.size > MAX_FRAGS {
                return Err(FragmentQueueError::TooManyFragments);
            }
            return if let Some((size, frames)) = self.fragments.get_mut(&meta.id) {
                // check if the frame index is out of bounds
                // todo: Check if == or >, I think it's > but I'm not sure.
                // todo: This is because the index starts at 0 and the size starts at 1.
                if meta.index >= *size {
                    return Err(FragmentQueueError::FrameIndexOutOfBounds);
                }
                // the frame exists, and we have parts, check if we have this particular frame already.
                if frames.iter().any(|f| {
                    f.fragment_meta
                        .as_ref()
                        .is_some_and(|fragment| fragment.index == meta.index)
                }) {
                    // We already have this frame! Do not replace it!!
                    Err(FragmentQueueError::FrameExists)
                } else if self.fragment_frames >= MAX_FRAGMENT_FRAMES {
                    Err(FragmentQueueError::QueueFull)
                } else {
                    frames.push(fragment);
                    self.fragment_frames += 1;
                    Ok((meta.size, meta.index))
                }
            } else {
                if self.fragments.len() >= MAX_FRAGMENT_SETS
                    || self.fragment_frames >= MAX_FRAGMENT_FRAMES
                {
                    return Err(FragmentQueueError::QueueFull);
                }
                // We don't already have this fragment index!
                let (size, mut frames) = (meta.size, Vec::<Frame>::new());
                frames.push(fragment);

                self.fragments.insert(meta.id, (size, frames));
                self.fragment_frames += 1;
                Ok((meta.size, meta.index))
            };
        }

        Err(FragmentQueueError::FrameNotFragmented)
    }

    /// Attempts to collect all fragments from a given fragment id.
    /// Will fail if not all fragments are specified.
    pub fn collect(&mut self, id: u16) -> Result<Vec<u8>, FragmentQueueError> {
        let Some((size, frames)) = self.fragments.get_mut(&id) else {
            return Err(FragmentQueueError::FragmentInvalid);
        };
        if *size != frames.len() as u32 {
            return Err(FragmentQueueError::FragmentsMissing);
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

    /// This will split a given frame into a bunch of smaller frames within the specified
    /// restriction.
    pub fn split_insert(&mut self, buffer: &[u8], mtu: u16) -> Result<u16, FragmentQueueError> {
        // Advance the fragment id by one (wrapping). The previous code used
        // `self.fragment_id += self.fragment_id.wrapping_add(1)`, which double-counts
        // and produces a non-linear sequence (1, 3, 7, 15, ...). It stayed unique so
        // reassembly still worked, but the id space was consumed far faster than
        // necessary and the intent was clearly a single increment.
        self.fragment_id = self.fragment_id.wrapping_add(1);

        let id = self.fragment_id;

        self.remove(&id);

        if let Ok(frames) = Self::split(buffer, id, mtu) {
            if frames.len() > MAX_FRAGMENT_FRAMES
                || (self.fragments.len() >= MAX_FRAGMENT_SETS && !self.fragments.contains_key(&id))
            {
                return Err(FragmentQueueError::QueueFull);
            }
            if let Some((_, old_frames)) = self.fragments.remove(&id) {
                self.fragment_frames = self.fragment_frames.saturating_sub(old_frames.len());
            }
            self.fragment_frames += frames.len();
            self.fragments.insert(id, (frames.len() as u32, frames));
            return Ok(id);
        }

        Err(FragmentQueueError::DoesNotNeedSplit)
    }

    pub fn split(buffer: &[u8], id: u16, mtu: u16) -> Result<Vec<Frame>, FragmentQueueError> {
        let Some(max_mtu) = mtu.checked_sub(RAKNET_HEADER_FRAME_OVERHEAD) else {
            return Err(FragmentQueueError::DoesNotNeedSplit);
        };

        if max_mtu == 0 {
            return Err(FragmentQueueError::DoesNotNeedSplit);
        }

        if buffer.len() > max_mtu.into() {
            let splits = buffer
                .chunks(max_mtu.into())
                .map(|c| c.to_vec())
                .collect::<Vec<Vec<u8>>>();
            if splits.len() > MAX_FRAGS as usize {
                return Err(FragmentQueueError::TooManyFragments);
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

        Err(FragmentQueueError::DoesNotNeedSplit)
    }

    pub fn get(&self, id: &u16) -> Result<&(u32, Vec<Frame>), FragmentQueueError> {
        if let Some(v) = self.fragments.get(id) {
            return Ok(v);
        }

        Err(FragmentQueueError::FragmentInvalid)
    }

    pub fn get_mut(&mut self, id: &u16) -> Result<&mut (u32, Vec<Frame>), FragmentQueueError> {
        if let Some(v) = self.fragments.get_mut(id) {
            return Ok(v);
        }

        Err(FragmentQueueError::FragmentInvalid)
    }

    pub fn take(&mut self, id: u16) -> Result<(u32, Vec<Frame>), FragmentQueueError> {
        if let Some((size, frames)) = self.fragments.remove(&id) {
            self.fragment_frames = self.fragment_frames.saturating_sub(frames.len());
            return Ok((size, frames));
        }
        Err(FragmentQueueError::FragmentInvalid)
    }

    pub fn remove(&mut self, id: &u16) -> bool {
        if let Some((_, frames)) = self.fragments.remove(id) {
            self.fragment_frames = self.fragment_frames.saturating_sub(frames.len());
            true
        } else {
            false
        }
    }

    /// This will hard clear the fragment queue, this should only be used if memory becomes an issue!
    pub fn clear(&mut self) {
        self.fragment_id = 0;
        self.fragments.clear();
        self.fragments.shrink_to_fit();
        self.fragment_frames = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FragmentQueueError {
    FrameExists,
    FrameNotFragmented,
    DoesNotNeedSplit,
    FragmentInvalid,
    FragmentsMissing,
    FrameIndexOutOfBounds,
    TooManyFragments,
    QueueFull,
}

impl std::fmt::Display for FragmentQueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                FragmentQueueError::FrameExists => "Frame already exists",
                FragmentQueueError::FrameNotFragmented => "Frame is not fragmented",
                FragmentQueueError::DoesNotNeedSplit => "Frame does not need to be split",
                FragmentQueueError::FragmentInvalid => "Fragment is invalid",
                FragmentQueueError::FragmentsMissing => "Fragments are missing",
                FragmentQueueError::FrameIndexOutOfBounds => "Frame index is out of bounds",
                FragmentQueueError::TooManyFragments => "Too many fragments",
                FragmentQueueError::QueueFull => "Fragment queue is full",
            }
        )
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
