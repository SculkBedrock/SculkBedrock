pub mod controller;
pub mod queue;
pub mod state;

use crate::connection::queue::recv::{RecvHealth, RecvQueue, RecvQueueError};
use crate::connection::queue::send::SendQueue;
use crate::connection::queue::SendQueueError;
use crate::connection::state::{AtomicConnectionState, ConnectionState};
use crate::loop_exec;
use crate::notify::Notify;
use crate::protocol::ack::{Ack, Ackable};
use crate::protocol::ack::{ACK, NACK};
use crate::protocol::frame::{Frame, FramePacket};
use crate::protocol::packet::offline::OfflinePacket;
use crate::protocol::packet::online::{
    ConnectedPing, ConnectedPong, ConnectionAccept, Disconnect, OnlinePacket,
};
use crate::protocol::packet::RakPacket;
use crate::protocol::reliability::Reliability;
use crate::server::current_epoch;
use crate::utils::{seq24, to_address_token, LoopResult};
use async_channel::{bounded, Receiver, Sender, TrySendError};
use log::{debug, error, trace};
use parking_lot::Mutex;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::select;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use sc_binary::interfaces::{Reader, Writer};
use sc_ecs::component::Component;
use sc_log::t_log;

/// Shared count window for game-layer outbound commands and reserved
/// packet admission. Capacity bounds command count, not bytes.
const OUTBOUND_CHANNEL_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutboundAdmissionError {
    Busy,
    Closed,
}

#[derive(Clone, Debug)]
struct OutboundSlotBudget {
    semaphore: Arc<Semaphore>,
}

impl OutboundSlotBudget {
    fn new(capacity: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(capacity)),
        }
    }

    async fn acquire(&self) -> Result<OutboundSlotPermit, SendQueueError> {
        let permit = Arc::clone(&self.semaphore)
            .acquire_owned()
            .await
            .map_err(|_| SendQueueError::SendError)?;
        Ok(OutboundSlotPermit {
            budget: Arc::clone(&self.semaphore),
            _permit: permit,
            guards: Vec::new(),
        })
    }

    fn try_acquire(&self) -> Result<OutboundSlotPermit, OutboundAdmissionError> {
        let permit =
            Arc::clone(&self.semaphore)
                .try_acquire_owned()
                .map_err(|error| match error {
                    TryAcquireError::NoPermits => OutboundAdmissionError::Busy,
                    TryAcquireError::Closed => OutboundAdmissionError::Closed,
                })?;
        Ok(OutboundSlotPermit {
            budget: Arc::clone(&self.semaphore),
            _permit: permit,
            guards: Vec::new(),
        })
    }
}

/// RAII reservation for one outbound command slot on a single connection.
pub struct OutboundSlotPermit {
    budget: Arc<Semaphore>,
    _permit: OwnedSemaphorePermit,
    guards: Vec<Box<dyn Send>>,
}

impl std::fmt::Debug for OutboundSlotPermit {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OutboundSlotPermit")
    }
}

impl OutboundSlotPermit {
    /// Keep an owned accounting/pinning guard alive until the outbound command
    /// has been consumed by the send loop (or the command is cancelled).
    pub fn attach_guard(&mut self, guard: impl Send + 'static) {
        self.guards.push(Box::new(guard));
    }
}

#[derive(Debug)]
pub(crate) struct QueuedOutboundCommand {
    command: OutboundCommand,
    _permit: OutboundSlotPermit,
}

/// High-priority protocol command channel capacity.
/// Handlers are microsecond-scale; capacity absorbs short bursts.
const PRIORITY_CHANNEL_CAPACITY: usize = 64;

/// Debug interval (seconds) for net_recv health summaries.
const RECV_HEALTH_LOG_INTERVAL_SECS: u64 = 10;

#[derive(Debug)]
pub enum RecvError {
    Closed,
}

impl Display for RecvError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for RecvError {}

#[derive(Debug, Clone, Copy)]
pub struct ConnMeta {
    /// This is important, and is stored within the server itself
    /// This value is 0 until the connection state is `Connecting`
    pub mtu_size: u16,
    /// The time this connection last sent any data. This will be used during server tick.
    pub recv_time: u64,
}

pub(crate) type ConnNetChan = Arc<Receiver<Vec<u8>>>;

impl ConnMeta {
    pub fn new(mtu_size: u16) -> Self {
        Self {
            mtu_size,
            recv_time: current_epoch(),
        }
    }
}

/// Game-layer outbound commands (normal priority): encoded batch bytes.
#[derive(Debug)]
pub enum OutboundCommand {
    /// Game-layer encoded raw bytes (batch) over ReliableOrd / channel 0.
    Send { bytes: Vec<u8>, immediate: bool },
}

/// High-priority protocol-layer commands.
///
/// The send loop consumes this channel first: control plane (pong replies,
/// ACK cleanup, NACK retransmit, disconnect) stays isolated from game data,
/// so chunk-send storms never queue-block control traffic. Control handlers
/// are microsecond-scale memory operations.
#[derive(Debug)]
pub enum PriorityCommand {
    /// RakNet protocol packets (ConnectedPong / ConnectionAccept / Disconnect).
    RakPacket {
        packet: RakPacket,
        reliability: Reliability,
        immediate: bool,
    },
    /// Client ACKs drop acknowledged datagrams from the recovery queue.
    Ack(Ack),
    /// Client NACKs retransmit lost datagrams immediately.
    Nack(Ack),
    /// Shut down send_loop (queued after earlier commands for flush semantics).
    Close,
}

/// Dedicated keepalive send path (ConnectedPing/ConnectedPong).
///
/// Ping/pong is the critical liveness path: clients disconnect after ~5s
/// without a response, so it never queues behind game packets. The path
/// bypasses queues and SendQueue:
///
/// 1. datagram numbers come from a shared atomic counter (per-connection
///    monotonic unique numbers for client ordering and ACK/NACK);
/// 2. unreliable probes skip the recovery queue entirely and recover
///    through the periodic 3s resend;
/// 3. direct `try_send_to` with no queueing, channels, or lock waits.
///
/// Out-of-order arrival is harmless: client recv already handles UDP reorder.
#[derive(Clone, Debug)]
pub(crate) struct KeepaliveChannel {
    socket: Arc<UdpSocket>,
    address: SocketAddr,
    datagram_seq: Arc<AtomicU32>,
}

impl KeepaliveChannel {
    /// Send one keepalive packet immediately (unreliable, no retransmit).
    pub(crate) fn send(&self, packet: RakPacket) -> Result<(), SendQueueError> {
        let buf = packet
            .write_to_bytes()
            .map_err(|_| SendQueueError::ParseError)?;

        let frame = Frame::new(Reliability::Unreliable, Some(buf.as_slice()));
        let sequence = seq24(self.datagram_seq.fetch_add(1, Ordering::Relaxed));

        let mut pk = FramePacket::new();
        pk.sequence = sequence;
        pk.reliability = frame.reliability;
        pk.frames.push(frame);

        let bytes = pk
            .write_to_bytes()
            .map_err(|_| SendQueueError::ParseError)?;
        let wire = bytes.as_slice();
        crate::dump::record("out", self.address, wire, "datagram");

        if let Err(e) = self.socket.try_send_to(wire, self.address) {
            trace!(
                "[{}] keepalive direct send failed: {:?}",
                to_address_token(self.address),
                e
            );
            return Err(SendQueueError::SendError);
        }
        Ok(())
    }
}

/// Cadence of one housekeeping round.
///
/// Housekeeping covers the three duties that must keep running for a healthy
/// connection no matter how busy the command channels are: the keepalive probe,
/// flushing queued frames with retransmission, and the unacknowledged-age
/// policy.
const HOUSEKEEPING_INTERVAL: Duration = Duration::from_millis(50);

/// Interval between keepalive probes of a peer that is answering.
const KEEPALIVE_INTERVAL: Duration = Duration::from_millis(3_000);

/// Bookkeeping that runs on a wall-clock deadline rather than on a `select!`
/// branch.
#[derive(Debug, Default)]
struct HousekeepingState {
    /// When the last round ran. `None` means "never ran".
    last_round: Option<Instant>,
    /// When the last keepalive probe was sent.
    last_ping: Option<Instant>,
}

impl HousekeepingState {
    /// Whether a round is due at `now`.
    fn due(&self, now: Instant) -> bool {
        match self.last_round {
            None => true,
            Some(last) => now.saturating_duration_since(last) >= HOUSEKEEPING_INTERVAL,
        }
    }

    /// Whether the peer is due for a keepalive probe at `now`.
    ///
    /// Kept separate from the round cadence so that running extra rounds — which
    /// is exactly what happens under load — never turns into a faster ping.
    fn ping_due(&self, now: Instant) -> bool {
        match self.last_ping {
            None => true,
            Some(last) => now.saturating_duration_since(last) >= KEEPALIVE_INTERVAL,
        }
    }
}

/// One housekeeping round. Returns `true` when the connection must be closed.
///
/// This deliberately does **not** live in one `select!` branch. The send loop
/// polls with `biased` ordering, which commits to the first *ready* branch and
/// never polls the rest, so a timer branch placed after two command channels
/// stops running entirely for as long as both channels stay backlogged — and a
/// backlogged control *and* data plane is the normal state while chunks stream.
///
/// When that happened, three failures appeared together and none of them looked
/// related: `SendQueue::ready` stopped being flushed (so chunk payloads were
/// accepted by the game layer but never left), unacknowledged datagrams were
/// never retransmitted (so a single loss left a permanent hole that stalled the
/// peer's reliable reassembly, which is what a "frozen" client actually is), and
/// the keepalive ping stopped (so the client timed the server out and left while
/// the congested server never observed the disconnect). Every branch therefore
/// calls this through [`HousekeepingState::due`], which makes progress a
/// function of elapsed time instead of branch position.
fn housekeeping_round(
    send_q: &mut SendQueue,
    keepalive: &KeepaliveChannel,
    address: SocketAddr,
    state: &mut HousekeepingState,
    now: Instant,
) -> bool {
    state.last_round = Some(now);

    // Keepalive probe, sent ahead of the flush so a congested queue can never
    // delay it.
    if state.ping_due(now) {
        let ping_time = current_epoch() as i64;
        match keepalive.send(ConnectedPing { time: ping_time }.into()) {
            Ok(()) => debug!(
                "[{}] Sent periodic ConnectedPing (time={})",
                to_address_token(address),
                ping_time
            ),
            Err(_) => trace!(
                "[{}] Failed to send periodic ConnectedPing!",
                to_address_token(address)
            ),
        }
        state.last_ping = Some(now);
    }

    // Flush queued frames and retransmit what the peer never acknowledged.
    send_q.update();

    // Age policy: a peer that stopped acknowledging must not keep the reliable
    // queue (and this process) pinned. This is terminal — the peer's stream
    // already has a hole.
    if send_q.expire_stale() {
        error!(
            "{}",
            t_log!(
                "console.raknet.send_expired",
                addr = to_address_token(address),
                count = send_q.abandoned_unacked()
            )
        );
        return true;
    }
    false
}

/// Outcome of offering one control fact to the send loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlOffer {
    /// The send loop owns it now.
    Handed,
    /// The send loop was too far behind to take it.
    Dropped,
}

/// Receive-path counters for control facts the send loop could not accept.
///
/// Owned by the net_recv task, so the numbers need no synchronisation.
#[derive(Debug, Default)]
struct ControlHandoff {
    /// ACK/NACK the send loop never saw.
    ///
    /// Recoverable: `SendQueue::update` retransmits whatever the peer has not
    /// acknowledged and the peer re-acknowledges what it receives.
    deferred: u64,
    /// Handshake responses that could not be queued at all.
    ///
    /// Not recoverable — there is no retransmit for a handshake — so these
    /// fail the connection.
    refused: u64,
    /// Reassembled game packets the game layer had no room for.
    overrun: u64,
}

/// Offer one control fact to the send loop **without ever blocking**.
///
/// The net_recv task owns `RecvQueue` and is the only reader of the per-session
/// datagram channel, so waiting here stops the whole inbound direction for this
/// session: the session channel fills, and the server socket loop then drops
/// every further inbound datagram (`server::recv_packet` drops on `Full` rather
/// than blocking). The peer keeps answering keepalives that nobody reads and
/// then leaves, and the server never observes it. Blocking the receive path on
/// the send path is therefore never acceptable, no matter how slow the send
/// loop gets.
///
/// A full channel means the send loop has not drained
/// [`PRIORITY_CHANNEL_CAPACITY`] control commands. That is already past the
/// point where protocol facts are produced faster than reliable recovery can
/// absorb them, so the caller decides what an unaccepted fact means rather than
/// this function deciding for it: ACK/NACK are counted and left to retransmission,
/// while a handshake response fails the connection.
fn offer_control(
    priority: &Sender<PriorityCommand>,
    command: PriorityCommand,
    recoverable: bool,
    stats: &mut ControlHandoff,
) -> ControlOffer {
    match priority.try_send(command) {
        Ok(()) => ControlOffer::Handed,
        Err(TrySendError::Full(_)) => {
            if recoverable {
                stats.deferred = stats.deferred.saturating_add(1);
                // Counted, not logged per event: a NACK storm would otherwise
                // turn a single congestion episode into a log flood.
                trace!(
                    "control channel full: {} recoverable control fact(s) deferred to retransmission",
                    stats.deferred
                );
            } else {
                stats.refused = stats.refused.saturating_add(1);
                error!(
                    "{}",
                    t_log!("console.raknet.control_refused", count = stats.refused)
                );
            }
            ControlOffer::Dropped
        }
        Err(TrySendError::Closed(_)) => ControlOffer::Dropped,
    }
}

/// Hand one reassembled game packet to the game layer **without ever blocking**.
///
/// This is the same invariant as [`offer_control`], one hop further along: the
/// net_recv task is the only reader of the per-session datagram channel, so
/// waiting for the game layer to catch up stalls the whole inbound direction.
/// Once that happens the session channel fills, the server socket loop drops
/// every further inbound datagram, and the peer looks dead to us while its
/// keepalive answers and its disconnect both go unread — the session then only
/// ends when the inactivity policy happens to fire.
///
/// The reassembled message cannot be recovered either: RakNet has already
/// acknowledged the datagram it came from, so the peer will not resend it.
/// Silently dropping it would lose gameplay state, so a full game-layer channel
/// fails the connection instead — the same honest terminal the outbound path
/// takes when reliable datagrams can no longer be admitted.
fn hand_to_game_layer(
    sender: &Sender<Vec<u8>>,
    buffer: &[u8],
    stats: &mut ControlHandoff,
) -> Result<(), &'static str> {
    match sender.try_send(buffer.to_vec()) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => {
            stats.overrun = stats.overrun.saturating_add(1);
            Err("game layer is not draining inbound packets: closing connection")
        }
        Err(TrySendError::Closed(_)) => Err("game layer stopped receiving: closing connection"),
    }
}

/// Lock-free concurrency model (split by ownership, not locks):
///
/// | State              | Owner          | Sync                    |
/// |--------------------|----------------|-------------------------|
/// | `RecvQueue`        | net_recv task  | exclusive `&mut`, no locks |
/// | `SendQueue`        | send_loop task | exclusive `&mut`, no locks |

/// | Connection state   | shared         | `AtomicConnectionState` |
/// | Datagram numbers   | shared         | `AtomicU32`             |
/// | Last receive time  | shared         | `AtomicU64`             |
/// | Shutdown signal    | shared         | `Notify`                |
/// | ACK/NACK send      | net_recv       | direct UDP, no locks    |
/// | Keepalive send     | any task       | direct UDP, no locks    |
#[derive(Component, Clone, Debug)]
pub struct Connection {
    pub address: SocketAddr,
    pub state: Arc<AtomicConnectionState>,
    outbound_tx: Sender<QueuedOutboundCommand>,
    outbound_slots: OutboundSlotBudget,
    priority_tx: Sender<PriorityCommand>,
    /// Dedicated keepalive send path (no queueing, no contention).
    pub(crate) keepalive: KeepaliveChannel,
    pub internal_net_recv: ConnNetChan,
    disconnect: Arc<Notify>,
    close_notifier: Arc<Sender<SocketAddr>>,
    recv_time: Arc<AtomicU64>,
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl Connection {
    pub async fn new(
        address: SocketAddr,
        socket: &Arc<UdpSocket>,
        net: Receiver<Vec<u8>>,
        notifier: Arc<Sender<SocketAddr>>,
        mtu: u16,
    ) -> Self {
        // net_recv to game-layer channel: reassembled game packets queue
        // here. Too-small capacity amplifies game-layer pauses into backpressure.
        let (net_sender, net_receiver) = bounded::<Vec<u8>>(256);
        let (outbound_tx, outbound_rx) =
            bounded::<QueuedOutboundCommand>(OUTBOUND_CHANNEL_CAPACITY);
        let outbound_slots = OutboundSlotBudget::new(OUTBOUND_CHANNEL_CAPACITY);
        let (priority_tx, priority_rx) = bounded::<PriorityCommand>(PRIORITY_CHANNEL_CAPACITY);
        // Datagram numbers share one atomic counter across send paths
        // (numbers must be globally monotonic unique).
        let datagram_seq = Arc::new(AtomicU32::new(0));
        let keepalive = KeepaliveChannel {
            socket: socket.clone(),
            address,
            datagram_seq: datagram_seq.clone(),
        };
        let c = Self {
            address,
            outbound_tx,
            outbound_slots,
            priority_tx,
            keepalive,
            internal_net_recv: Arc::new(net_receiver),
            state: Arc::new(AtomicConnectionState::new(ConnectionState::Unidentified)),
            disconnect: Arc::new(Notify::new()),
            close_notifier: notifier.clone(),
            recv_time: Arc::new(AtomicU64::new(current_epoch())),
            tasks: Arc::new(Mutex::new(Vec::new())),
        };

        let tk = c.tasks.clone();
        let mut tasks = tk.lock();
        tasks.push(c.init_tick(notifier));
        tasks.push(c.init_net_recv(net, net_sender, socket.clone()));
        // send_loop owns SendQueue exclusively: the only outbound owner, no locks.
        tasks.push(c.init_send_loop(
            SendQueue::new(mtu, 12000, 5, datagram_seq, socket.clone(), address),
            outbound_rx,
            priority_rx,
            c.keepalive.clone(),
        ));

        c
    }

    /// Dedicated outbound task with exclusive SendQueue ownership.
    ///
    /// - `biased` select polls in fixed order: shutdown, high-priority
    ///   (control), normal (data), periodic tick, so pong/ACK/NACK never
    ///   wait behind chunk storms;
    /// - control handlers are microsecond-scale memory operations;
    /// - ACK/NACK replies bypass this task: net_recv answers datagrams
    ///   directly for lower latency.
    pub(crate) fn init_send_loop(
        &self,
        mut send_q: SendQueue,
        outbound_rx: Receiver<QueuedOutboundCommand>,
        priority_rx: Receiver<PriorityCommand>,
        keepalive: KeepaliveChannel,
    ) -> JoinHandle<()> {
        let address = self.address;
        let closer = self.disconnect.clone();

        tokio::spawn(async move {
            let mut housekeeping = HousekeepingState::default();
            let mut ticker = tokio::time::interval(HOUSEKEEPING_INTERVAL);
            // Skip missed ticks when busy; never send catch-up bursts.
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // The first interval tick fires immediately; skip it so a fresh
            // connection does not spin idly.
            ticker.tick().await;

            loop {
                tokio::select! {
                    biased;
                    _ = closer.wait() => {
                        break;
                    }
                    cmd = priority_rx.recv() => {
                        match cmd {
                            Ok(PriorityCommand::RakPacket { packet, reliability, immediate }) => {
                                if let Err(e) = send_q.send_packet(packet, reliability, immediate) {
                                    if e.is_terminal() {
                                        error!(
                                            "{}",
                                            t_log!(
                                                "console.raknet.send_stop",
                                                addr = to_address_token(address),
                                                error = e
                                            )
                                        );
                                        break;
                                    }
                                    trace!(
                                        "[{}] [task: send_loop] failed to send rak packet: {:?}",
                                        to_address_token(address),
                                        e
                                    );
                                }
                            }
                            Ok(PriorityCommand::Ack(ack)) => {
                                send_q.ack(ack);
                            }
                            Ok(PriorityCommand::Nack(nack)) => {
                                // Re-send the original FramePacket bytes directly.
                                // This preserves the original sequence number that
                                // the client referenced in its NACK, so the client
                                // can reassemble the fragment in place. Re-inserting
                                // via `insert()` would wrap the datagram in a
                                // brand-new FramePacket with a new sequence number
                                // *and* Unreliable reliability, which breaks
                                // fragment reassembly for split packets.
                                for packet in send_q.nack(nack) {
                                    if let Ok(buffer) = packet.write_to_bytes() {
                                        let _ = send_q.send_stream(buffer.as_slice());
                                    }
                                }
                            }
                            Ok(PriorityCommand::Close) => break,
                            Err(_) => break, // 所有 Sender 已 drop
                        }
                        // A backlogged control plane must not be able to starve
                        // the flush/retransmit/keepalive round: see
                        // `housekeeping_round`.
                        let now = Instant::now();
                        if housekeeping.due(now)
                            && housekeeping_round(&mut send_q, &keepalive, address, &mut housekeeping, now)
                        {
                            break;
                        }
                    }
                    cmd = outbound_rx.recv() => {
                        match cmd {
                            Ok(queued) => {
                                let QueuedOutboundCommand { command, _permit } = queued;
                                match command {
                                    OutboundCommand::Send { bytes, immediate } => {
                                        if let Err(e) = send_q.insert(&bytes, Reliability::ReliableOrd, immediate, Some(0)) {
                                            if e.is_terminal() {
                                                // Reliable datagrams can no longer be
                                                // admitted. Closing is the only honest
                                                // outcome: keeping the connection would
                                                // silently drop reliable data.
                                                error!(
                                                    "{}",
                                                    t_log!(
                                                        "console.raknet.send_stop",
                                                        addr = to_address_token(address),
                                                        error = e
                                                    )
                                                );
                                                break;
                                            }
                                            trace!(
                                                "[{}] [task: send_loop] failed to insert game packet: {:?}",
                                                to_address_token(address),
                                                e
                                            );
                                        }
                                    }
                                }
                                // The count slot covers the channel and the short
                                // insertion step; SendQueue owns its recovery copy after this.
                                drop(_permit);
                            }
                            Err(_) => break, // 所有 Sender 已 drop
                        }
                        // Same invariant as the control-plane branch: streaming
                        // chunks must not be able to starve housekeeping.
                        let now = Instant::now();
                        if housekeeping.due(now)
                            && housekeeping_round(&mut send_q, &keepalive, address, &mut housekeeping, now)
                        {
                            break;
                        }
                    }
                    _ = ticker.tick() => {
                        let now = Instant::now();
                        if housekeeping_round(&mut send_q, &keepalive, address, &mut housekeeping, now) {
                            break;
                        }
                    }
                }
            }

            // Free recovery/fragment/ready buffer memory.
            send_q.clear();
            trace!(
                "[{}] [task: send_loop] exited, send queue cleared",
                to_address_token(address)
            );
        })
    }

    pub(crate) fn init_tick(&self, notifier: Arc<Sender<SocketAddr>>) -> JoinHandle<()> {
        let address = self.address;
        let closer = self.disconnect.clone();
        let last_recv = self.recv_time.clone();
        let state = self.state.clone();

        // initialize the event io
        // we initialize the ticking function here, it's purpose is to update the state of the current connection
        // while handling throttle
        tokio::spawn(async move {
            loop {
                select! {
                    _ = closer.wait() => {
                        trace!("[{}] [task: tick] Connection has been closed due to closer!", to_address_token(address));
                        break;
                    }
                    _ = sleep(Duration::from_millis(50)) => {
                       loop_exec!(Self::tick_body(
                            address,
                            closer.clone(),
                            last_recv.clone(),
                            state.clone(),
                        ).await);
                    }
                }
            }

            if let Ok(_) = notifier.send(address).await {
                trace!(
                    "[{}] [task: tick] Connection has been closed due to closer!",
                    to_address_token(address)
                );
            } else {
                trace!(
                    "[{}] [task: tick] Connection has been closed due to closer!",
                    to_address_token(address)
                );
            }
            trace!(
                "[{}] Connection has been cleaned up!",
                to_address_token(address)
            );
        })
    }

    async fn tick_body(
        address: SocketAddr,
        closer: Arc<Notify>,
        last_recv: Arc<AtomicU64>,
        state: Arc<AtomicConnectionState>,
    ) -> LoopResult {
        let recv = last_recv.load(Ordering::Relaxed);
        let now = current_epoch();

        let should_close = {
            let cstate = state.load();

            if cstate == ConnectionState::Disconnected {
                trace!(
                    "[{}] Connection has been closed due to state!",
                    to_address_token(address)
                );
                true
            } else if recv + 15 <= now {
                state.store(ConnectionState::Disconnected);
                trace!(
                    "[{}] Connection has been closed due to inactivity!",
                    to_address_token(address)
                );
                true
            } else {
                if recv + 10 <= now && cstate.is_reliable() {
                    state.store(ConnectionState::TimingOut);
                    trace!(
                        "[{}] Connection is timing out, sending a ping!",
                        to_address_token(address)
                    );
                }
                false
            }
        };

        if should_close {
            closer.notify();
            return LoopResult::Break;
        }

        LoopResult::Continue
    }

    /// This function initializes the raw internal packet handling task!
    ///
    /// This task owns `RecvQueue` exclusively (reassembly, ACK records,
    /// NACK detection, ordered dispatch) and answers every datagram with
    /// ACK/NACK control packets directly: datagram-level packets need no
    /// sequence or retransmit, so plain `try_send_to` suffices.
    pub(crate) fn init_net_recv(
        &self,
        net: Receiver<Vec<u8>>,
        sender: Sender<Vec<u8>>,
        socket: Arc<UdpSocket>,
    ) -> JoinHandle<()> {
        let recv_time = self.recv_time.clone();
        // RecvQueue is task-exclusive: access via &mut borrow, no locks.
        let mut recv_q = RecvQueue::new();
        let priority = self.priority_tx.clone();
        let keepalive = self.keepalive.clone();
        let disconnect = self.disconnect.clone();
        let state = self.state.clone();
        let address = self.address;

        tokio::spawn(async move {
            // Task-exclusive receive-path counters need no sync.
            let mut stats = ControlHandoff::default();
            // Ordered gaps heal through retransmission within about a second, so
            // a one-second inspection cadence cannot mistake a normal reorder
            // for a stall; the health summary below runs an order of magnitude
            // slower and is purely diagnostic.
            let mut gap_watch = tokio::time::interval(Duration::from_secs(1));
            gap_watch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last_health_log = current_epoch();
            let mut last_health = RecvHealth::default();
            loop {
                select! {
                    _ = disconnect.wait() => {
                        trace!("[{}] [task: net_recv] Connection has been closed due to closer!", to_address_token(address));
                        break;
                    }
                    res = net.recv() => {
                        match res {
                            Ok(payload) => {
                                loop_exec!(Self::handle_payload(
                                    payload,
                                    &mut recv_q,
                                    &recv_time,
                                    &priority,
                                    &keepalive,
                                    &disconnect,
                                    &state,
                                    address,
                                    &sender,
                                    &socket,
                                    &mut stats,
                                ));
                            }
                            _ => continue,
                        }
                    }
                    _ = gap_watch.tick() => {
                        let now = current_epoch();
                        // Missing UDP sequences do not gate frame delivery.
                        // Retry observed gaps even while no datagrams arrive.
                        Self::flush_ack_nack(&socket, address, &mut recv_q);
                        // A gap that outlives retransmission is a hole nothing
                        // will fill; every later ordered frame is already
                        // buffered behind it. Close with an explicit reason so
                        // the peer reconnects instead of both ends freezing
                        // with zero diagnostics.
                        if let Some((channel, age)) = recv_q.stale_ordered_gap(now) {
                            let health = recv_q.health();
                            error!(
                                "{}",
                                t_log!(
                                    "console.raknet.stalled",
                                    addr = to_address_token(address),
                                    channel = channel,
                                    age = age,
                                    inn = health.datagrams_in,
                                    ready = health.frames_ready,
                                    rejects = health.oldseq_rejects,
                                    ignored = health.ignored_ordered,
                                    buffered = health.ordered_buffered,
                                    nack = health.nack_pending
                                )
                            );
                            disconnect.notify();
                            break;
                        }
                        if now.saturating_sub(last_health_log) >= RECV_HEALTH_LOG_INTERVAL_SECS {
                            last_health_log = now;
                            let health = recv_q.health();
                            log::debug!(
                                "[{}] [task: net_recv] health: datagrams_in={} frames_ready={} \
                                 refused(duplicates={} out_of_window={}) ignored_ordered={} \
                                 ordered_buffered={} ({}B) nack_pending={} \
                                 handoff(deferred={} refused={} overrun={})",
                                to_address_token(address),
                                health.datagrams_in,
                                health.frames_ready,
                                health.duplicates,
                                health.out_of_window,
                                health.ignored_ordered,
                                health.ordered_buffered,
                                health.ordered_buffered_bytes,
                                health.nack_pending,
                                stats.deferred,
                                stats.refused,
                                stats.overrun,
                            );
                            if health.oldseq_rejects > 0
                                && health.frames_ready == last_health.frames_ready
                            {
                                log::warn!(
                                    "{}",
                                    t_log!(
                                        "console.raknet.inbound_refused",
                                        addr = to_address_token(address),
                                        dups = health.duplicates,
                                        window = health.out_of_window,
                                        total = health.datagrams_in,
                                        nack = health.nack_pending
                                    )
                                );
                            }
                            last_health = health;
                        }
                    }
                }
            }
            if stats.deferred > 0 || stats.refused > 0 || stats.overrun > 0 {
                trace!(
                    "[{}] [task: net_recv] control handoff: {} deferred to retransmission, \
                     {} refused, {} game packet(s) rejected",
                    to_address_token(address),
                    stats.deferred,
                    stats.refused,
                    stats.overrun
                );
            }
            // Graceful exit: free fragment/order/ACK buffers.
            recv_q.clear();
        })
    }

    /// Direct ACK/NACK send: datagram-level packets (0xc0/0xa0) take no
    /// datagram numbers and need no retransmit. Prompt per-datagram ACK is
    /// standard RakNet behavior (clients stop retransmitting sooner).
    fn flush_ack_nack(socket: &UdpSocket, address: SocketAddr, recv_q: &mut RecvQueue) {
        let ack_seqs = recv_q.ack_flush();
        if !ack_seqs.is_empty() {
            let ack = Ack::from_records(ack_seqs, false);
            if let Ok(p) = ack.write_to_bytes() {
                if let Err(e) = socket.try_send_to(p.as_slice(), address) {
                    trace!(
                        "[{}] Failed to send ACK: {:?}",
                        to_address_token(address),
                        e
                    );
                }
            }
        }

        let nack_seqs = recv_q.nack_queue();
        if !nack_seqs.is_empty() {
            let nack = Ack::from_records(nack_seqs, true);
            if let Ok(p) = nack.write_to_bytes() {
                if let Err(e) = socket.try_send_to(p.as_slice(), address) {
                    trace!(
                        "[{}] Failed to send NACK: {:?}",
                        to_address_token(address),
                        e
                    );
                }
            }
        }
    }

    /// Deliberately **synchronous**.
    ///
    /// This runs on the net_recv task, the only reader of the per-session
    /// datagram channel, so an `await` here is an await on the outbound path.
    /// Keeping it sync makes that a compile error rather than a runtime stall.
    fn handle_payload(
        payload: Vec<u8>,
        recv_q: &mut RecvQueue,
        recv_time: &AtomicU64,
        priority: &Sender<PriorityCommand>,
        keepalive: &KeepaliveChannel,
        disconnect: &Arc<Notify>,
        state: &Arc<AtomicConnectionState>,
        address: SocketAddr,
        sender: &Sender<Vec<u8>>,
        socket: &UdpSocket,
        stats: &mut ControlHandoff,
    ) -> LoopResult {
        recv_time.store(current_epoch(), Ordering::Relaxed);
        crate::dump::record("in", address, &payload, "datagram");
        let cstate = state.load();

        if cstate == ConnectionState::TimingOut {
            trace!(
                "[{}] Connection is no longer timing out!",
                to_address_token(address)
            );
            state.store(ConnectionState::Connected);
        }

        let Some(&id) = payload.first() else {
            trace!(
                "[{}] Ignoring empty RakNet datagram",
                to_address_token(address)
            );
            return LoopResult::Continue;
        };

        match id {
            // This is a frame packet.
            // This packet will be handled by the recv_queue
            0x80..=0x8d => {
                if let Ok(pk) = FramePacket::read_from_slice(&payload[..]) {
                    // RecvQueue is task-exclusive (&mut), no lock contention:
                    // insert the datagram, reassemble fragments, drain ready
                    // buffers, then ACK/NACK immediately.
                    if let Err(e) = recv_q.insert(pk) {
                        if matches!(
                            e,
                            RecvQueueError::OrderChannelExhausted { .. }
                                | RecvQueueError::ReliableWindowExhausted { .. }
                        ) {
                            // Capacity exhaustion cannot be ACKed as successful
                            // reliable delivery. End this session immediately.
                            error!("{}", t_log!("console.raknet.recv_error", addr = to_address_token(address), reason = format!("{e:?}")));
                            disconnect.notify();
                            return LoopResult::Break;
                        }
                        trace!(
                            "[{}] Failed to insert frame packet! {:?}",
                            to_address_token(address),
                            e
                        );
                    }
                    Self::flush_ack_nack(socket, address, recv_q);
                    let buffers = recv_q.flush();

                    for buffer in buffers {
                        let res = Connection::process_packet(
                            &buffer, &address, sender, priority, keepalive, state, stats,
                        );
                        if let Ok(v) = res {
                            if v == true {
                                // DISCONNECT
                                trace!(
                                    "[{}] Connection::process_packet returned true!",
                                    to_address_token(address)
                                );
                                disconnect.notify();
                                break;
                            }
                        }
                        if let Err(e) = res {
                            trace!(
                                "[{}] Failed to process packet: {:?}!",
                                to_address_token(address),
                                e
                            );
                        };
                    }
                } else {
                    trace!(
                        "[{}] Failed to parse frame packet!",
                        to_address_token(address)
                    );
                }
            }
            NACK => {
                // Validate this is a nack packet

                if let Ok(nack) = Ack::read_from_slice(&payload[..]) {
                    // The client acknowledges it did not recieve these packets
                    // Forward to send_loop for lock-free retransmit.
                    // Never waits (see `offer_control`).
                    offer_control(priority, PriorityCommand::Nack(nack), true, stats);
                }
            }
            ACK => {
                // first lets validate this is an ack packet
                if let Ok(ack) = Ack::read_from_slice(&payload[..]) {
                    // The client acknowledges it received these FramePackets.
                    // Remove them from the send recovery queue so they are no
                    // longer retransmitted.
                    //
                    // NOTE: Do NOT call recv_q.ack() here. The client's ACK
                    // references *server-sent* sequence numbers (send_seq),
                    // but the RecvQueue's nack set tracks *client-sent*
                    // sequence numbers — a completely different sequence space.
                    // Calling recv_q.ack() with the client's ACK would
                    // accidentally clear legitimate NACK entries whenever the
                    // two sequence spaces happen to overlap, causing the server
                    // to stop requesting missing client packets.
                    //
                    // A deferred ACK keeps its recovery entry a little longer
                    // and is retransmitted once; see `offer_control`.
                    offer_control(priority, PriorityCommand::Ack(ack), true, stats);
                }
            }
            _ => {
                trace!(
                    "[{}] Unknown RakNet packet recieved (Or packet is sent out of scope).",
                    to_address_token(address)
                );
            }
        }
        LoopResult::Continue
    }

    /// Deliberately **synchronous**, for the same reason as
    /// [`Self::handle_payload`]: no step of inbound processing may wait on the
    /// outbound path.
    fn process_packet(
        buffer: &[u8],
        address: &SocketAddr,
        sender: &Sender<Vec<u8>>,
        priority: &Sender<PriorityCommand>,
        keepalive: &KeepaliveChannel,
        state: &Arc<AtomicConnectionState>,
        stats: &mut ControlHandoff,
    ) -> Result<bool, ()> {
        if buffer.is_empty() {
            trace!(
                "[{}] Ignoring empty RakNet payload",
                to_address_token(*address)
            );
            return Ok(false);
        }
        if let Ok(online_packet) = OnlinePacket::read_from_slice(&buffer) {
            return match online_packet {
                OnlinePacket::ConnectedPing(pk) => {
                    debug!(
                        "[{}] Received ConnectedPing (time={})",
                        to_address_token(*address),
                        pk.time
                    );
                    let response = ConnectedPong {
                        ping_time: pk.time,
                        pong_time: current_epoch() as i64,
                    };
                    let pong_time = response.pong_time;
                    // Pong uses the keepalive direct path (unreliable + immediate
                    // UDP send): zero latency even in game packet storms.
                    match keepalive.send(response.into()) {
                        Ok(()) => {
                            debug!(
                                "[{}] Sent ConnectedPong (ping_time={}, pong_time={})",
                                to_address_token(*address),
                                pk.time,
                                pong_time
                            );
                            Ok(false)
                        }
                        Err(_) => {
                            debug!(
                                "[{}] Failed to send ConnectedPong packet!",
                                to_address_token(*address)
                            );
                            Err(())
                        }
                    }
                }
                OnlinePacket::ConnectedPong(_pk) => {
                    // do nothing rn
                    // TODO: add ping calculation
                    debug!(
                        "[{}] Received ConnectedPong (ping_time={}, pong_time={})",
                        to_address_token(*address),
                        _pk.ping_time,
                        _pk.pong_time
                    );
                    Ok(false)
                }
                OnlinePacket::ConnectionRequest(pk) => {
                    let internal_ids = vec![
                        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)), 19132),
                        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)), 19133),
                        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)), 19134),
                        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)), 19135),
                        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)), 19136),
                    ];
                    let response = ConnectionAccept {
                        system_index: 0,
                        client_address: *address,
                        internal_ids,
                        request_time: pk.time,
                        timestamp: current_epoch() as i64,
                    };
                    state.store(ConnectionState::Connecting);
                    // Route through the high-priority channel to send_loop so login
                    // handshakes never compete with game-layer sends. Never waits.
                    match offer_control(
                        priority,
                        PriorityCommand::RakPacket {
                            packet: response.clone().into(),
                            reliability: Reliability::Reliable,
                            immediate: true,
                        },
                        false,
                        stats,
                    ) {
                        ControlOffer::Handed => Ok(false),
                        ControlOffer::Dropped => {
                            trace!(
                                "[{}] Failed to send ConnectionAccept packet!",
                                to_address_token(*address)
                            );
                            Err(())
                        }
                    }
                }
                OnlinePacket::Disconnect(_) => {
                    // Disconnect the client immediately.
                    // connection.disconnect("Client disconnected.", false);
                    Ok(true)
                }
                OnlinePacket::LostConnection(_) => {
                    // Disconnect the client immediately.
                    // connection.disconnect("Client disconnected.", false);
                    trace!(
                        "[{}] Client has lost connection, disconnecting client!",
                        to_address_token(*address)
                    );
                    Ok(true)
                }
                OnlinePacket::NewConnection(_) => {
                    // if we are already connected, disconnect the client.
                    if state.load() == ConnectionState::Connected {
                        trace!(
                            "[{}] Client is already connected, disconnecting client!",
                            to_address_token(*address)
                        );
                        return Ok(true);
                    }

                    state.store(ConnectionState::Connected);
                    Ok(false)
                }
                _ => {
                    trace!(
                        "[{}] Forwarding packet to socket!\n{:?}",
                        to_address_token(*address),
                        buffer
                    );
                    if let Err(reason) = hand_to_game_layer(sender, &buffer, stats) {
                        error!("{}", t_log!("console.raknet.handoff_error", addr = to_address_token(*address), reason = reason));
                        return Err(());
                    }
                    Ok(false)
                }
            };
        } else if let Ok(_) = OfflinePacket::read_from_slice(&buffer) {
            state.store(ConnectionState::Disconnecting);
            trace!(
                "[{}] Invalid protocol! Disconnecting client!",
                to_address_token(*address)
            );
            return Err(());
        }

        trace!(
            "[{}] Either Game-packet or unknown packet, sending buffer to client...",
            to_address_token(*address)
        );
        if let Err(reason) = hand_to_game_layer(sender, &buffer, stats) {
            error!("{}", t_log!("console.raknet.handoff_error", addr = to_address_token(*address), reason = reason));
            return Err(());
        }
        Ok(false)
    }

    pub async fn recv(&self) -> Result<Vec<u8>, RecvError> {
        self.internal_net_recv
            .recv()
            .await
            .map_err(|_| RecvError::Closed)
    }

    pub async fn is_closed(&self) -> bool {
        !self.state.load().is_available()
    }

    /// Reserve one count slot before packet hooks/encoding. The reservation is
    /// held until the send loop has transferred the command into SendQueue.
    pub async fn reserve_outbound_slot(&self) -> Result<OutboundSlotPermit, SendQueueError> {
        self.outbound_slots.acquire().await
    }

    /// Nonblocking admission used by retryable chunk delivery.
    pub fn try_reserve_outbound_slot(&self) -> Result<OutboundSlotPermit, OutboundAdmissionError> {
        self.outbound_slots.try_acquire()
    }

    pub async fn send(&self, buffer: &[u8], immediate: bool) -> Result<(), SendQueueError> {
        let permit = self.reserve_outbound_slot().await?;
        self.send_owned_with_permit(buffer.to_vec(), immediate, permit)
            .await
    }

    /// Queue an owned wire buffer without duplicating it.
    ///
    /// The encoded network path already owns its final Vec. Transferring that
    /// allocation into the bounded outbound channel avoids a full packet copy
    /// per send; callers with borrowed buffers retain [`Self::send`].
    pub async fn send_owned(&self, bytes: Vec<u8>, immediate: bool) -> Result<(), SendQueueError> {
        let permit = self.reserve_outbound_slot().await?;
        self.send_owned_with_permit(bytes, immediate, permit).await
    }

    /// Transfer an owned wire buffer using a slot reserved before encoding.
    pub async fn send_owned_with_permit(
        &self,
        bytes: Vec<u8>,
        immediate: bool,
        permit: OutboundSlotPermit,
    ) -> Result<(), SendQueueError> {
        self.try_send_owned_with_permit(bytes, immediate, permit)
    }

    /// A reserved slot covers every queued command and in-progress insertion.
    /// Queue admission is synchronous so callers can publish an ordered receipt
    /// without an await between encryption and enqueue.
    pub fn try_send_owned_with_permit(
        &self,
        bytes: Vec<u8>,
        immediate: bool,
        permit: OutboundSlotPermit,
    ) -> Result<(), SendQueueError> {
        if !Arc::ptr_eq(&self.outbound_slots.semaphore, &permit.budget) {
            return Err(SendQueueError::SendError);
        }
        let cmd = QueuedOutboundCommand {
            command: OutboundCommand::Send { bytes, immediate },
            _permit: permit,
        };
        self.outbound_tx
            .try_send(cmd)
            .map_err(|_| SendQueueError::SendError)
    }

    pub async fn close(&self) {
        trace!("[{}] Dropping connection!", to_address_token(self.address));
        self.state.store(ConnectionState::Disconnected);

        // Send the Disconnect packet first through the high-priority channel,
        // then the Close command: channel FIFO guarantees Disconnect goes out
        // before the loop exits. Use try_send, never blocking send (a dead
        // send_loop would hang close() forever).
        let disconnect_packet = OnlinePacket::Disconnect(Disconnect {});
        let _ = self.priority_tx.try_send(PriorityCommand::RakPacket {
            packet: disconnect_packet.into(),
            reliability: Reliability::Reliable,
            immediate: true,
        });
        let _ = self.priority_tx.try_send(PriorityCommand::Close);

        self.disconnect.notify();

        // `close()` aborts the tick task before it can report its address.
        // Notify the listener explicitly so its session map cannot retain a
        // dead sender until the process shuts down.
        let _ = self.close_notifier.try_send(self.address);

        // Give send_loop a scheduling window to flush the Disconnect datagram
        // (microsecond-scale handling; clients time out after 15s on loss).
        tokio::time::sleep(Duration::from_millis(5)).await;

        // The net_recv task owns recv_queue: it frees with the aborted future,
        // so no explicit cleanup happens (or can happen) here.

        let tasks = self.tasks.clone();

        for task in tasks.lock().drain(..) {
            task.abort();
        }
    }
}

/// Safety net: if `close()` was never called (e.g. due to a panic or logic
/// error that skips `PlayerConnection::disconnect()`), the RakNet tick and
/// net_recv tasks would survive forever, continuously holding memory and
/// CPU. This Drop implementation aborts any remaining tasks when the last
/// `Connection` clone is dropped.
///
/// The `Arc::strong_count` check ensures we only abort when this is truly the
/// last reference — temporary clones (e.g. from `get_component`) must not
/// prematurely kill tasks that the original still needs.
impl Drop for Connection {
    fn drop(&mut self) {
        if Arc::strong_count(&self.tasks) == 1 {
            let mut tasks = self.tasks.lock();
            for task in tasks.drain(..) {
                task.abort();
            }
        }
    }
}

#[cfg(test)]
mod receive_path_tests {
    use super::*;
    use async_channel::bounded as async_bounded;

    /// An ACK datagram exactly as `flush_ack_nack` puts it on the wire.
    fn ack_datagram(sequences: Vec<u32>) -> Vec<u8> {
        Ack::from_records(sequences, false)
            .write_to_bytes()
            .expect("encode ack")
            .as_slice()
            .to_vec()
    }

    #[test]
    fn a_full_control_channel_defers_recoverable_facts_and_refuses_others() {
        let (tx, rx) = async_bounded::<PriorityCommand>(1);
        let mut stats = ControlHandoff::default();

        assert_eq!(
            offer_control(&tx, PriorityCommand::Close, true, &mut stats),
            ControlOffer::Handed
        );
        assert_eq!(stats.deferred, 0);
        assert_eq!(stats.refused, 0);

        // Full: recoverable facts are counted for retransmission to recover...
        assert_eq!(
            offer_control(
                &tx,
                PriorityCommand::Ack(Ack::from_records(vec![1], false)),
                true,
                &mut stats
            ),
            ControlOffer::Dropped
        );
        assert_eq!(stats.deferred, 1);
        assert_eq!(stats.refused, 0);

        // ...while a fact with no retransmit path fails instead.
        assert_eq!(
            offer_control(
                &tx,
                PriorityCommand::RakPacket {
                    packet: OnlinePacket::ConnectedPong(ConnectedPong {
                        ping_time: 1,
                        pong_time: 2,
                    })
                    .into(),
                    reliability: Reliability::Reliable,
                    immediate: true,
                },
                false,
                &mut stats
            ),
            ControlOffer::Dropped
        );
        assert_eq!(stats.refused, 1);
        // The queued command is untouched: nothing was evicted to make room.
        assert_eq!(rx.len(), 1);
    }

    #[test]
    fn a_full_game_layer_channel_fails_rather_than_losing_the_packet() {
        let (tx, _rx) = async_bounded::<Vec<u8>>(1);
        let mut stats = ControlHandoff::default();
        tx.try_send(vec![0u8; 4]).expect("occupy");

        let reason = hand_to_game_layer(&tx, &[0u8; 32], &mut stats)
            .expect_err("an unaccepted game packet must fail the connection");
        assert!(reason.contains("not draining"), "{reason}");
        assert_eq!(stats.overrun, 1);
        assert_eq!(stats.deferred, 0);
    }

    #[tokio::test]
    async fn reliable_window_exhaustion_closes_on_the_current_wire_datagram() {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = peer.local_addr().unwrap();
        let (priority_tx, _priority_rx) = async_bounded(4);
        let (game_tx, game_rx) = async_bounded(4);
        let keepalive = KeepaliveChannel {
            socket: socket.clone(),
            address,
            datagram_seq: Arc::new(AtomicU32::new(0)),
        };
        let disconnect = Arc::new(Notify::new());
        let state = Arc::new(AtomicConnectionState::new(ConnectionState::Connected));
        let mut recv_q = RecvQueue::new();
        let mut stats = ControlHandoff::default();
        let mut frame = Frame::new(Reliability::Reliable, Some(&[0xfe, 0xA]));
        frame.reliable_index = Some(2048);
        let mut packet = FramePacket::new();
        packet.frames.push(frame);
        let result = Connection::handle_payload(
            packet.write_to_bytes().unwrap().as_slice().to_vec(),
            &mut recv_q,
            &AtomicU64::new(current_epoch()),
            &priority_tx,
            &keepalive,
            &disconnect,
            &state,
            address,
            &game_tx,
            &socket,
            &mut stats,
        );
        assert!(matches!(result, LoopResult::Break));
        assert!(disconnect.is_closed());
        assert!(game_rx.is_empty());
        assert!(recv_q.ack_flush().is_empty());
        let mut buffer = [0; 1500];
        assert!(
            tokio::time::timeout(Duration::from_millis(20), peer.recv_from(&mut buffer))
                .await
                .is_err(),
            "failed reliable admission must send no ACK"
        );
    }

    /// The regression this guards: with a full control channel, inbound
    /// processing must return immediately. Blocking there stops the only reader
    /// of the session's datagram channel, after which the server socket loop
    /// drops every further inbound datagram — the peer then looks dead to the
    /// server while its keepalive answers and its disconnect go unread.
    #[tokio::test]
    async fn a_wedged_send_loop_cannot_stall_inbound_processing() {
        let socket = Arc::new(
            UdpSocket::bind("127.0.0.1:0")
                .await
                .expect("bind test socket"),
        );
        let address = "127.0.0.1:9".parse().unwrap();
        let (priority_tx, priority_rx) = async_bounded::<PriorityCommand>(1);
        priority_tx
            .try_send(PriorityCommand::Close)
            .expect("wedge the control channel");
        let (game_tx, _game_rx) = async_bounded::<Vec<u8>>(4);

        let keepalive = KeepaliveChannel {
            socket: socket.clone(),
            address,
            datagram_seq: Arc::new(AtomicU32::new(0)),
        };
        let disconnect = Arc::new(Notify::new());
        let state = Arc::new(AtomicConnectionState::new(ConnectionState::Connected));
        let recv_time = AtomicU64::new(current_epoch());

        let handled = tokio::task::spawn_blocking(move || {
            let mut recv_q = RecvQueue::new();
            let mut stats = ControlHandoff::default();
            let payload = ack_datagram(vec![7, 8]);
            Connection::handle_payload(
                payload,
                &mut recv_q,
                &recv_time,
                &priority_tx,
                &keepalive,
                &disconnect,
                &state,
                address,
                &game_tx,
                &socket,
                &mut stats,
            );
            stats
        });

        let stats = tokio::time::timeout(Duration::from_secs(5), handled)
            .await
            .expect("inbound processing must not wait on the send path")
            .expect("join inbound processing");
        assert_eq!(
            stats.deferred, 1,
            "the unaccepted ACK must be counted for retransmission to recover"
        );
        assert_eq!(priority_rx.len(), 1);
    }
}

#[cfg(test)]
mod housekeeping_tests {
    use super::*;
    use async_channel::bounded as async_bounded;

    /// Bind a real peer socket so keepalive probes can be observed arriving.
    async fn test_peer() -> (Arc<UdpSocket>, SocketAddr) {
        let peer = Arc::new(
            UdpSocket::bind("127.0.0.1:0")
                .await
                .expect("bind test peer"),
        );
        let address = peer.local_addr().expect("peer address");
        (peer, address)
    }

    fn test_send_queue(
        socket: Arc<UdpSocket>,
        datagram_seq: Arc<AtomicU32>,
        address: SocketAddr,
    ) -> SendQueue {
        SendQueue::new(1492, 12000, 5, datagram_seq, socket, address)
    }

    /// Count keepalive probes a peer received, ignoring other datagrams.
    async fn received_pings(peer: &UdpSocket) -> usize {
        let mut buf = [0u8; 2048];
        let mut pings = 0;
        while let Ok(Ok((length, _))) =
            tokio::time::timeout(Duration::from_millis(20), peer.recv_from(&mut buf)).await
        {
            let Ok(packet) = FramePacket::read_from_slice(&buf[..length]) else {
                continue;
            };
            let is_ping = packet.frames.first().is_some_and(|frame| {
                matches!(
                    OnlinePacket::read_from_slice(&frame.body),
                    Ok(OnlinePacket::ConnectedPing(_))
                )
            });
            if is_ping {
                pings += 1;
            }
        }
        pings
    }

    #[test]
    fn a_round_is_due_again_only_after_the_interval_elapsed() {
        let mut state = HousekeepingState::default();
        let start = Instant::now();

        // A state that has never run is always due.
        assert!(state.due(start));
        assert!(state.ping_due(start));

        state.last_round = Some(start);
        state.last_ping = Some(start);
        assert!(!state.due(start));
        assert!(!state.ping_due(start));

        // Deadline driven, not tick-count driven: extra rounds inside one
        // interval must not become due early.
        for offset in [1, 25, 49] {
            assert!(!state.due(start + Duration::from_millis(offset)));
        }
        assert!(state.due(start + HOUSEKEEPING_INTERVAL));
        // Extra rounds must not accelerate the keepalive probe either.
        assert!(!state.ping_due(start + HOUSEKEEPING_INTERVAL));
        assert!(state.ping_due(start + KEEPALIVE_INTERVAL));
    }

    #[tokio::test]
    async fn a_saturated_control_plane_can_no_longer_starve_housekeeping() {
        let (peer, address) = test_peer().await;
        let socket = Arc::new(
            UdpSocket::bind("127.0.0.1:0")
                .await
                .expect("bind test socket"),
        );
        let datagram_seq = Arc::new(AtomicU32::new(0));
        let keepalive = KeepaliveChannel {
            socket: socket.clone(),
            address,
            datagram_seq: datagram_seq.clone(),
        };
        let mut send_q = test_send_queue(socket, datagram_seq, address);

        // A small non-immediate payload waits in `ready`, and only `update()`
        // writes it to the wire. Nothing in the game layer uses `immediate =
        // false`, so this queue is normally empty; it is used here purely to
        // observe that `update()` — which also drives retransmission and the
        // unacked-age policy — is reached at all.
        send_q
            .insert(&[0xABu8; 64], Reliability::ReliableOrd, false, Some(0))
            .expect("queue a non-immediate payload");
        assert!(
            send_q.ready_len() > 0,
            "payload must be waiting for the flush"
        );

        // The control channel is permanently backlogged and the timer branch is
        // polled last, which is what `biased` ordering actually does: the first
        // ready branch wins and the remaining ones are never polled.
        let (priority_tx, priority_rx) =
            async_bounded::<PriorityCommand>(PRIORITY_CHANNEL_CAPACITY);
        let (outbound_tx, outbound_rx) = async_bounded::<QueuedOutboundCommand>(1);
        let nack = || PriorityCommand::Nack(Ack::from_records(vec![0], true));
        for _ in 0..PRIORITY_CHANNEL_CAPACITY {
            priority_tx
                .try_send(nack())
                .expect("fill the control plane");
        }
        outbound_tx
            .try_send(QueuedOutboundCommand {
                command: OutboundCommand::Send {
                    bytes: vec![1, 2, 3],
                    immediate: true,
                },
                _permit: OutboundSlotBudget::new(1)
                    .try_acquire()
                    .expect("outbound slot"),
            })
            .expect("fill the data plane");

        let mut housekeeping = HousekeepingState::default();
        let mut ticker = tokio::time::interval(HOUSEKEEPING_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;

        let mut rounds = 0u32;
        let mut timer_starved = false;
        // Long enough for several housekeeping intervals to elapse.
        let window = HOUSEKEEPING_INTERVAL * 4;
        let started = Instant::now();
        while started.elapsed() < window {
            tokio::select! {
                biased;
                cmd = priority_rx.recv() => {
                    assert!(cmd.is_ok());
                    // Exactly what the send loop does after every command.
                    let now = Instant::now();
                    if housekeeping.due(now)
                        && !housekeeping_round(&mut send_q, &keepalive, address, &mut housekeeping, now)
                    {
                        rounds += 1;
                    }
                    // Keep the control plane saturated so the timer branch can
                    // never be selected for the whole window.
                    let _ = priority_tx.try_send(nack());
                }
                cmd = outbound_rx.recv() => {
                    let Ok(queued) = cmd else { break };
                    let QueuedOutboundCommand { command, _permit } = queued;
                    let OutboundCommand::Send { bytes, immediate } = command;
                    let _ = send_q.insert(&bytes, Reliability::ReliableOrd, immediate, Some(0));
                    drop(_permit);
                    let now = Instant::now();
                    if housekeeping.due(now)
                        && !housekeeping_round(&mut send_q, &keepalive, address, &mut housekeeping, now)
                    {
                        rounds += 1;
                    }
                    let _ = outbound_tx.try_send(QueuedOutboundCommand {
                        command: OutboundCommand::Send { bytes: vec![4, 5, 6], immediate: true },
                        _permit: OutboundSlotBudget::new(1).try_acquire().expect("outbound slot"),
                    });
                }
                _ = ticker.tick() => {
                    timer_starved = true;
                }
            }
        }

        assert!(
            rounds >= 3,
            "housekeeping must keep a {HOUSEKEEPING_INTERVAL:?} cadence under saturation, \
             ran {rounds} (timer branch fired: {timer_starved})"
        );
        assert_eq!(
            send_q.ready_len(),
            0,
            "housekeeping must flush queued payloads while both command planes are backlogged"
        );
        assert!(
            send_q.recovery_entries() > 0,
            "flushed datagrams must be tracked so a loss can be retransmitted"
        );

        // The reported symptom: the server stops probing, so the client leaves.
        // Drain whatever the loop already sent, re-arm the probe deadline, and
        // prove a round still emits the probe without any timer branch.
        let _ = received_pings(&peer).await;
        housekeeping.last_ping = Some(Instant::now() - KEEPALIVE_INTERVAL);
        housekeeping.last_round = Some(Instant::now() - HOUSEKEEPING_INTERVAL);
        // `true` means "close the connection": nothing here is stale yet.
        assert!(!housekeeping_round(
            &mut send_q,
            &keepalive,
            address,
            &mut housekeeping,
            Instant::now()
        ));
        assert_eq!(
            received_pings(&peer).await,
            1,
            "a due housekeeping round must emit exactly one keepalive probe"
        );
    }
}

#[cfg(test)]
mod outbound_slot_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DropSignal(Arc<AtomicUsize>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn outbound_slot_budget_is_bounded_and_command_owns_its_permit() {
        let budget = OutboundSlotBudget::new(2);
        let first = budget.try_acquire().expect("first slot");
        let second = budget.try_acquire().expect("second slot");
        assert!(matches!(
            budget.try_acquire(),
            Err(OutboundAdmissionError::Busy)
        ));

        drop(first);
        let mut permit = budget.try_acquire().expect("released slot");
        let guards_dropped = Arc::new(AtomicUsize::new(0));
        permit.attach_guard(DropSignal(Arc::clone(&guards_dropped)));
        let queued = QueuedOutboundCommand {
            command: OutboundCommand::Send {
                bytes: vec![1, 2, 3],
                immediate: true,
            },
            _permit: permit,
        };
        assert!(matches!(
            budget.try_acquire(),
            Err(OutboundAdmissionError::Busy)
        ));
        assert_eq!(guards_dropped.load(Ordering::Relaxed), 0);

        drop(queued);
        assert_eq!(guards_dropped.load(Ordering::Relaxed), 1);
        assert!(budget.try_acquire().is_ok());
        drop(second);
    }
}
