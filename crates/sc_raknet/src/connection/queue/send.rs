use super::{FragmentQueue, FragmentQueueError, NetQueue, QueuedItem, RecoveryQueue};
use crate::connection::queue::{MAX_RECOVERY_QUEUE_BYTES, MAX_RECOVERY_QUEUE_ITEMS};
use crate::protocol::ack::{Ack, Ackable, Record, SingleRecord};
use crate::protocol::frame::{Frame, FramePacket};
use crate::protocol::packet::RakPacket;
use crate::protocol::reliability::Reliability;
use crate::protocol::RAKNET_HEADER_FRAME_OVERHEAD;
use crate::utils::{seq24, to_address_token, SafeGenerator};
use log::{trace, warn};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use sc_binary::interfaces::Writer;
use sc_log::t_log;

/// How many seconds a reliable datagram must remain unacknowledged before it is
/// retransmitted by the periodic `update()` tick. This is a coarse fallback on
/// top of NACK-driven retransmission; 2 seconds is well above typical RTT so it
/// does not compete with the client's own NACK requests.
const RESEND_THRESHOLD_SECS: u64 = 2;
const MAX_RESENDS_PER_TICK: usize = 32;
const MAX_NACK_RESENDS_PER_PACKET: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SendQueueError {
    /// The packet is too large to be sent.
    PacketTooLarge,
    /// Parsing Error
    ParseError,
    /// Fragmentation error
    FragmentError(FragmentQueueError),
    /// Send queue error
    SendError,
    /// Unacknowledged reliable data reached its admission bound.
    ///
    /// Terminal for the connection: continuing would silently drop reliable
    /// datagrams the peer still expects, so the caller must close instead.
    RecoveryQueueFull(String),
}

impl SendQueueError {
    /// Whether the connection can no longer deliver a consistent stream.
    pub fn is_terminal(&self) -> bool {
        matches!(self, SendQueueError::RecoveryQueueFull(_))
    }
}

impl std::fmt::Display for SendQueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                SendQueueError::PacketTooLarge => "Packet too large".to_string(),
                SendQueueError::ParseError => "Parse error".to_string(),
                SendQueueError::FragmentError(e) => format!("Fragment error: {}", e),
                SendQueueError::SendError => "Send error".to_string(),
                SendQueueError::RecoveryQueueFull(reason) => {
                    format!("Recovery queue full: {reason}")
                }
            }
        )
    }
}

impl std::error::Error for SendQueueError {}

/// This queue is used to prioritize packets being sent out
/// Packets that are old, are either dropped or requested again.
/// You can define this behavior with the `timeout` property.
#[derive(Debug, Clone)]
pub struct SendQueue {
    mtu_size: u16,

    /// The amount of time that needs to pass for a packet to be
    /// dropped or requested again.
    _timeout: u16,

    /// The amount of times we should retry sending a packet before
    /// dropping it from the queue. This is currently set to `5`.
    _max_tries: u16,

    /// Datagram sequence allocator, shared with the keepalive direct-send
    /// path. Datagram sequence numbers are protocol-global per connection
    /// (the client sorts/ACKs/NACKs datagrams by this number), so both the
    /// send_loop and the ping/pong fast path MUST draw from the same
    /// atomic counter. Frame-level state (reliable_seq, order_channels)
    /// is only touched by the send_loop and stays private.
    datagram_seq: Arc<AtomicU32>,

    /// The current reliable index number.
    /// a packet is sent reliably an sequenced.
    reliable_seq: SafeGenerator<u32>,

    /// Datagram-level recovery queue, owned **exclusively** by the
    /// send_loop task — plain `&mut` access, zero locking. Keyed by
    /// FramePacket sequence (send_seq); the client's ACK/NACK reference
    /// that number, so this is the only correct key.
    recovery: RecoveryQueue<FramePacket>,

    /// The fragment queue.
    fragment_queue: FragmentQueue,

    /// The ordered channels.
    /// (send_seq, reliable_seq)
    order_channels: HashMap<u8, (u32, u32)>,

    ready: Vec<Frame>,

    /// Unacknowledged datagrams dropped by the age policy.
    abandoned_unacked: u64,

    #[cfg(not(target_arch = "wasm32"))]
    socket: Arc<tokio::net::UdpSocket>,
    #[cfg(target_arch = "wasm32")]
    socket: Arc<wasmedge_wasi_socket::UdpSocket>,

    address: SocketAddr,
}

impl SendQueue {
    fn next_send_seq(&mut self) -> u32 {
        seq24(self.datagram_seq.fetch_add(1, Ordering::Relaxed))
    }

    fn next_reliable_seq(&mut self) -> u32 {
        seq24(self.reliable_seq.next())
    }

    pub fn new(
        mtu_size: u16,
        _timeout: u16,
        _max_tries: u16,
        datagram_seq: Arc<AtomicU32>,
        #[cfg(not(target_arch = "wasm32"))] socket: Arc<tokio::net::UdpSocket>,
        #[cfg(target_arch = "wasm32")] socket: Arc<wasmedge_wasi_socket::UdpSocket>,
        address: SocketAddr,
    ) -> Self {
        Self {
            mtu_size,
            _timeout,
            _max_tries,
            datagram_seq,
            reliable_seq: SafeGenerator::new(),
            recovery: RecoveryQueue::with_limits(
                MAX_RECOVERY_QUEUE_ITEMS,
                MAX_RECOVERY_QUEUE_BYTES,
                Some(|packet: &FramePacket| packet.queued_bytes()),
            ),
            fragment_queue: FragmentQueue::new(),
            order_channels: HashMap::new(),
            ready: Vec::new(),
            abandoned_unacked: 0,
            socket,
            address,
        }
    }

    /// Send a packet based on its reliability.
    /// Note, reliability will be set to `Reliability::ReliableOrd` if
    /// the buffer is larger than max MTU.
    pub fn insert(
        &mut self,
        packet: &[u8],
        reliability: Reliability,
        immediate: bool,
        channel: Option<u8>,
    ) -> Result<(), SendQueueError> {
        trace!("Inserting packet into send queue: {} bytes", packet.len());
        trace!("Write is now processing packet");
        let max_payload = usize::from(self.mtu_size.saturating_sub(RAKNET_HEADER_FRAME_OVERHEAD));
        let reliable = if packet.len() > max_payload {
            Reliability::ReliableOrd
        } else {
            reliability
        };

        trace!("Write is now processing packet: {:?}", reliable);

        if packet.len() > max_payload {
            // we need to split this packet!
            // pass the buffer to the fragment queue.
            trace!("Write is now splitting, too large: {:?}", reliability);

            let fragmented = self.fragment_queue.split_insert(&packet, self.mtu_size);

            if let Ok(frag_id) = fragmented {
                // Take the fragmented frames out of the fragment queue so we actually
                // own them and can send them. The previous implementation only sent an
                // empty FramePacket (frames was never populated), which meant any packet
                // larger than the MTU - such as ResourcePackChunkData's 8KB payload -
                // never reached the client. This stalled the resource pack download at 0%.
                let mut frames_to_send = match self.fragment_queue.take(frag_id) {
                    Ok((_, frames)) => frames,
                    Err(_) => {
                        return Err(SendQueueError::FragmentError(
                            FragmentQueueError::FragmentInvalid,
                        ))
                    }
                };

                // `reliable` is the reliability decided for this (oversized) packet.
                // For packets larger than the MTU it is always forced to ReliableOrd
                // above, so the fragments MUST use `reliable` — not the original
                // `reliability` argument — otherwise the wire encoding of the
                // order_index/order_channel fields would be skipped when the caller
                // requested a non-ordered reliability, corrupting the frame on the wire.
                let channel = channel.unwrap_or(0);
                let (sequence_index, order_index) = {
                    let (ord_seq, ord_index) = self.order_channels.entry(channel).or_insert((0, 0));
                    let sequence_index = seq24(*ord_seq);
                    let order_index = seq24(*ord_index);
                    *ord_index = seq24(ord_index.wrapping_add(1));
                    *ord_seq = seq24(ord_seq.wrapping_add(1));
                    (sequence_index, order_index)
                };

                for frame in frames_to_send.iter_mut() {
                    frame.reliability = reliable;
                    frame.sequence_index = Some(sequence_index);
                    frame.order_channel = Some(channel);
                    frame.order_index = Some(order_index);

                    if frame.reliability.is_reliable() {
                        frame.reliable_index = Some(self.next_reliable_seq());
                    }
                }

                // Send each fragmented frame as its own datagram. Each fragment is
                // already <= MTU, so it fits in a single FramePacket. Track every
                // datagram in the recovery queue so lost fragments can be
                // retransmitted — datagram-level recovery is UNCONDITIONAL in
                // RakNet: the client sorts datagrams by sequence number and NACKs
                // gaps, so an untracked datagram that is lost would leave a
                // permanent hole that stalls all later frames.
                for frame in frames_to_send {
                    let mut pk = FramePacket::new();
                    pk.sequence = self.next_send_seq();
                    pk.reliability = frame.reliability;
                    pk.frames.push(frame);

                    self.recovery
                        .insert_id(pk.sequence, pk.clone())
                        .map_err(|full| SendQueueError::RecoveryQueueFull(full.to_string()))?;

                    if let Ok(buf) = pk.write_to_bytes() {
                        trace!("Write is sending fragment stream: {:?}", reliability);
                        self.send_stream(buf.as_slice())?;
                    }
                }

                Ok(())
            } else {
                // we couldn't send this frame!
                Err(SendQueueError::FragmentError(fragmented.unwrap_err()))
            }
        } else {
            // we're not gonna send this frame out yet!
            // we need to wait for the next tick.
            let mut frame = Frame::new(reliable, Some(packet));

            if frame.reliability.is_reliable() {
                frame.reliable_index = Some(self.next_reliable_seq());
            }

            if frame.reliability.is_ordered() {
                let (_, ord_index) = self
                    .order_channels
                    .entry(channel.unwrap_or(0))
                    .or_insert((0, 0));
                frame.order_index = Some(seq24(*ord_index));
                frame.sequence_index = Some(seq24(self.datagram_seq.load(Ordering::Relaxed)));
                *ord_index = seq24(ord_index.wrapping_add(1));
            } else if frame.reliability.is_sequenced() {
                let (seq_index, ord_index) = self
                    .order_channels
                    .entry(channel.unwrap_or(0))
                    .or_insert((0, 0));
                *seq_index = seq24(seq_index.wrapping_add(1));
                frame.order_index = Some(seq24(*ord_index));
                frame.sequence_index = Some(seq24(*seq_index));
            }

            if immediate {
                self.send_frame(frame)?;
            } else {
                self.ready.push(frame);
            }

            Ok(())
        }
    }

    /// A wrapper to send a single frame over the wire.
    /// While also reliabily tracking it.
    fn send_frame(&mut self, mut frame: Frame) -> Result<(), SendQueueError> {
        let mut pk = FramePacket::new();
        pk.sequence = self.next_send_seq();
        pk.reliability = frame.reliability;

        if pk.reliability.is_reliable() {
            frame.reliable_index = Some(self.next_reliable_seq());
        }

        pk.frames.push(frame);

        // Track this datagram in the recovery queue keyed by its FramePacket
        // sequence number (send_seq). ACK and NACK from the client reference
        // the FramePacket sequence number, so the key MUST be pk.sequence —
        // using reliable_seq.get() here (as before) made every ACK/NACK lookup
        // miss, so acknowledged packets were never removed and lost packets
        // could never be retransmitted via NACK.
        // Datagram-level recovery is UNCONDITIONAL (see fragment path note).
        self.recovery
            .insert_id(pk.sequence, pk.clone())
            .map_err(|full| SendQueueError::RecoveryQueueFull(full.to_string()))?;

        if let Ok(buf) = pk.write_to_bytes() {
            trace!("[!] Write sent the packet.. {:?}", buf.as_slice());
            self.send_stream(buf.as_slice())?;
        } else {
            trace!("SendQ: Failed to send frame: {:?}", pk);
            return Err(SendQueueError::ParseError);
        }
        Ok(())
    }

    pub(crate) fn send_stream(&self, packet: &[u8]) -> Result<(), SendQueueError> {
        trace!("SendQ: {}\n{:?}\n", packet.len(), packet);
        crate::dump::record("out", self.address, packet, "datagram");

        #[cfg(not(target_arch = "wasm32"))]
        let result = self.socket.try_send_to(packet, self.address);
        #[cfg(target_arch = "wasm32")]
        let result = self.socket.send_to(packet, &self.address);

        if let Err(e) = result {
            trace!(
                "[{}] Failed to send packet! {:?}",
                to_address_token(self.address),
                e
            );
            return Err(SendQueueError::SendError);
        }
        Ok(())
    }

    pub fn send_packet(
        &mut self,
        packet: RakPacket,
        reliability: Reliability,
        immediate: bool,
    ) -> Result<(), SendQueueError> {
        // parse the packet
        if let Ok(buf) = packet.write_to_bytes() {
            if let Err(e) = self.insert(buf.as_slice(), reliability, immediate, None) {
                trace!(
                    "[{}] Failed to insert packet into send queue: {:?}",
                    to_address_token(self.address),
                    e
                );
                return Err(e);
            }
            Ok(())
        } else {
            Err(SendQueueError::ParseError)
        }
    }

    /// Unacknowledged datagrams unacknowledged for longer than this are treated
    /// as a dead peer rather than retained forever.
    pub const MAX_UNACKED_AGE_SECS: u64 = 30;

    /// Diagnostics for the reliable stream (§19.1 network metrics).
    pub fn recovery_bytes(&self) -> usize {
        self.recovery.queued_bytes()
    }

    pub fn recovery_entries(&self) -> usize {
        self.recovery.len()
    }

    /// Frames accepted but not yet written to the wire.
    ///
    /// Only `update()` flushes this queue, so a non-zero value that does not
    /// shrink across housekeeping rounds means the flush is not running.
    pub fn ready_len(&self) -> usize {
        self.ready.len()
    }

    pub fn oldest_unacked_age(&self) -> Option<u64> {
        self.recovery.oldest_unacked_age()
    }

    /// Drop datagrams that exceeded [`Self::MAX_UNACKED_AGE_SECS`].
    ///
    /// Returns `true` when reliable data had to be abandoned. The send loop must
    /// close the connection in that case: the peer's stream already has a hole
    /// and keeping the connection open would only accumulate bytes.
    pub fn expire_stale(&mut self) -> bool {
        let dropped = self.recovery.flush_old(Self::MAX_UNACKED_AGE_SECS).len() as u64;
        if dropped == 0 {
            return false;
        }
        self.abandoned_unacked = self.abandoned_unacked.saturating_add(dropped);
        warn!(
            "{}",
            t_log!(
                "console.raknet.abandoned",
                addr = to_address_token(self.address),
                dropped = dropped,
                age = Self::MAX_UNACKED_AGE_SECS,
                total = self.abandoned_unacked,
                bytes = self.recovery.queued_bytes()
            )
        );
        true
    }

    pub fn abandoned_unacked(&self) -> u64 {
        self.abandoned_unacked
    }

    pub fn update(&mut self) {
        // send all the ready packets
        // TODO batch these packets together
        // TODO by lengths
        for frame in self.ready.drain(..).collect::<Vec<Frame>>() {
            if let Err(error) = self.send_frame(frame) {
                trace!("SendQ: failed to send queued frame: {error}");
            }
        }

        // Retransmit only packets that have been unacknowledged for longer than
        // the threshold. `resend_old` resets each retransmitted packet's timer but
        // keeps it in the queue (until an ACK removes it), so NACK-driven
        // retransmission still works. The previous code called `flush()` which
        // drained the *entire* recovery queue every tick — causing a retransmit
        // storm that overflowed UDP buffers and made lost fragments unrecoverable,
        // which froze resource pack downloads partway through.
        // Select within the budget so packets beyond this tick's limit are not
        // cloned or given a fresh timeout before they are actually attempted.
        let resend_queue = self
            .recovery
            .resend_old(RESEND_THRESHOLD_SECS, MAX_RESENDS_PER_TICK);

        // A small batch per 50 ms tick avoids flooding the UDP receive buffer.
        for packet in resend_queue {
            if let Ok(buf) = packet.write_to_bytes() {
                if let Err(error) = self.send_stream(buf.as_slice()) {
                    trace!("SendQ: failed to resend packet: {error}");
                }
            }
        }
    }

    /// Clears all buffered data in the send queue and releases allocated
    /// capacity. This should be called when a connection is being closed to
    /// free the large recovery queue, fragment queue, and ready buffers
    /// immediately — rather than waiting for the Arc-shared task futures to
    /// be dropped asynchronously by the tokio runtime (which can be delayed
    /// and prevents the OS allocator from reclaiming memory).
    pub fn clear(&mut self) {
        self.recovery.clear();
        self.fragment_queue.clear();
        self.order_channels.clear();
        self.order_channels.shrink_to_fit();
        self.ready.clear();
        self.ready.shrink_to_fit();
    }
}

impl Ackable for SendQueue {
    type NackItem = FramePacket;

    fn ack(&mut self, ack: Ack) {
        if ack.is_nack() {
            return;
        }

        // these packets are acknowledged, so we can remove them from the queue.
        // RakNet range records [start, end] are INCLUSIVE on both ends.
        // Using `start..end` (exclusive) silently drops the last sequence in
        // every range, leaving it in the recovery queue forever. Over a large
        // transfer (e.g. a 94 MB resource pack) this accumulates thousands of
        // "phantom" unacknowledged entries that are retransmitted every 2 s,
        // eventually congesting the link and stalling the download.
        for record in ack.records.iter() {
            match record {
                Record::Single(SingleRecord { sequence }) => {
                    let _ = self.recovery.remove(sequence.0);
                }
                Record::Range(ranged) => {
                    self.recovery
                        .remove_inclusive_range(ranged.start.0, ranged.end.0);
                }
            }
        }
    }

    fn nack(&mut self, nack: Ack) -> Vec<FramePacket> {
        if !nack.is_nack() {
            return Vec::new();
        }

        let mut resend_queue = Vec::<FramePacket>::new();

        // we need to get the packets to resend.
        for record in nack.records.iter() {
            match record {
                Record::Single(single) => {
                    if let Ok(packet) = self.recovery.get(single.sequence.0) {
                        resend_queue.push(packet.clone());
                    }
                }
                Record::Range(ranged) => {
                    let remaining = MAX_NACK_RESENDS_PER_PACKET.saturating_sub(resend_queue.len());
                    resend_queue.extend(self.recovery.get_inclusive_range(
                        ranged.start.0,
                        ranged.end.0,
                        remaining,
                    ));
                }
            }

            if resend_queue.len() >= MAX_NACK_RESENDS_PER_PACKET {
                resend_queue.truncate(MAX_NACK_RESENDS_PER_PACKET);
                break;
            }
        }

        resend_queue
    }
}
