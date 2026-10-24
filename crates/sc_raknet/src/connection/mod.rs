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
use sc_error::{ScError, ScSite, capture_site};
use sc_log::t_log;

/// 出站命令与槽位预留共用的计数窗口；容量限制命令条数而非字节数。
///
/// Shared count window for game-layer outbound commands and reserved packet
/// admission; the capacity bounds command count, not bytes.
const OUTBOUND_CHANNEL_CAPACITY: usize = 256;

/// 出站槽位准入失败。
///
/// Outbound slot admission failure.
#[derive(Clone, Debug, Eq, PartialEq, ScError)]
pub enum OutboundAdmissionError {
    /// 当前没有可用的出站槽位。
    ///
    /// No outbound slot is available.
    #[sc_error(code = "raknet.outbound.busy")]
    Busy { detail: String, site: ScSite },
    /// 出站信道已关闭。
    ///
    /// The outbound channel is closed.
    #[sc_error(code = "raknet.outbound.closed")]
    Closed { detail: String, site: ScSite },
}

impl OutboundAdmissionError {
    /// 构造 busy 错误并捕获调用位置。
    ///
    /// Builds a busy error, capturing the call site.
    #[track_caller]
    pub fn busy(detail: impl Into<String>) -> Self {
        Self::Busy {
            detail: detail.into(),
            site: capture_site(),
        }
    }

    /// 构造 closed 错误并捕获调用位置。
    ///
    /// Builds a closed error, capturing the call site.
    #[track_caller]
    pub fn closed(detail: impl Into<String>) -> Self {
        Self::Closed {
            detail: detail.into(),
            site: capture_site(),
        }
    }
}

impl Display for OutboundAdmissionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy { detail, site } => {
                write!(f, "[raknet.outbound.busy] {detail} (at {site})")
            }
            Self::Closed { detail, site } => {
                write!(f, "[raknet.outbound.closed] {detail} (at {site})")
            }
        }
    }
}

impl Error for OutboundAdmissionError {}


/// 出站命令槽位计数（信号量封装）。
///
/// Outbound command slot budget (semaphore wrapper).
#[derive(Clone, Debug)]
struct OutboundSlotBudget {
    semaphore: Arc<Semaphore>,
}

impl OutboundSlotBudget {
    /// 以给定容量创建槽位计数。
    ///
    /// Creates a budget with the given capacity.
    fn new(capacity: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(capacity)),
        }
    }

    /// 获取一个槽位（等待可用槽位）。信道关闭时返回 `SendError`。
    ///
    /// Acquires one slot, waiting until a slot is free. Returns `SendError`
    /// once the channel is closed.
    async fn acquire(&self) -> Result<OutboundSlotPermit, SendQueueError> {
        let permit = Arc::clone(&self.semaphore)
            .acquire_owned()
            .await
            .map_err(|_| SendQueueError::send_error("Send error"))?;
        Ok(OutboundSlotPermit {
            budget: Arc::clone(&self.semaphore),
            _permit: permit,
            guards: Vec::new(),
        })
    }

    /// 非阻塞获取一个槽位；无槽位返回 busy 错误，信道关闭返回 closed 错误。
    ///
    /// Acquires a slot without blocking; returns a busy error when no slot is
    /// free and a closed error when the channel is closed.
    fn try_acquire(&self) -> Result<OutboundSlotPermit, OutboundAdmissionError> {
        let permit =
            Arc::clone(&self.semaphore)
                .try_acquire_owned()
                .map_err(|error| match error {
                    TryAcquireError::NoPermits => {
                        OutboundAdmissionError::busy("Outbound channel is busy")
                    }
                    TryAcquireError::Closed => {
                        OutboundAdmissionError::closed("Outbound channel is closed")
                    }
                })?;
        Ok(OutboundSlotPermit {
            budget: Arc::clone(&self.semaphore),
            _permit: permit,
            guards: Vec::new(),
        })
    }
}

/// 单连接上一个出站命令槽位的 RAII 预留。
///
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
    /// 挂载一个自有记账/固定守卫，使其存活到该出站命令被发送循环消费
    /// （或命令被取消）为止。
    ///
    /// Keeps an owned accounting/pinning guard alive until the outbound command
    /// has been consumed by the send loop (or the command is cancelled).
    pub fn attach_guard(&mut self, guard: impl Send + 'static) {
        self.guards.push(Box::new(guard));
    }
}

/// 出站命令与其槽位预留的配对；预留随命令一同排队并在入队后释放。
///
/// An outbound command paired with its slot reservation; the permit is queued
/// with the command and released after insertion.
#[derive(Debug)]
pub(crate) struct QueuedOutboundCommand {
    command: OutboundCommand,
    _permit: OutboundSlotPermit,
}

/// 高优先级协议命令信道的容量（限制命令条数）。
///
/// Capacity of the high-priority protocol command channel (bounds command
/// count).
const PRIORITY_CHANNEL_CAPACITY: usize = 64;

/// net_recv 健康摘要日志的最小间隔（秒）。
///
/// Minimum interval (seconds) between net_recv health summary logs.
const RECV_HEALTH_LOG_INTERVAL_SECS: u64 = 10;

/// 入站游戏层接收失败。
///
/// Inbound game-layer receive failure.
#[derive(Debug, ScError)]
pub enum RecvError {
    /// 游戏层接收信道已关闭。
    ///
    /// The game-layer channel is closed.
    #[sc_error(code = "raknet.recv.closed")]
    Closed { detail: String, site: ScSite },
}

impl RecvError {
    /// 构造 closed 错误并捕获调用位置。
    ///
    /// Builds a closed error, capturing the call site.
    #[track_caller]
    pub fn closed(detail: impl Into<String>) -> Self {
        Self::Closed {
            detail: detail.into(),
            site: capture_site(),
        }
    }
}

impl Display for RecvError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed { detail, site } => {
                write!(f, "[raknet.recv.closed] {detail} (at {site})")
            }
        }
    }
}

impl Error for RecvError {}


/// 连接的对外元信息（MTU 与最近收包时间）。
///
/// External metadata for a connection (MTU and last receive time).
#[derive(Debug, Clone, Copy)]
pub struct ConnMeta {
    /// 该连接协商到的 MTU；状态为 `Connecting` 之前为 0。
    ///
    /// MTU negotiated for this connection; 0 until the state is `Connecting`.
    pub mtu_size: u16,
    /// 最近一次收到任意数据的时间（Unix 秒），供服务端 tick 判定存活。
    ///
    /// Time (Unix seconds) this connection last received any data; used by
    /// the server tick.
    pub recv_time: u64,
}

/// net_recv 任务读取的会话级数据报信道。
///
/// Per-session datagram channel read by the net_recv task.
pub(crate) type ConnNetChan = Arc<Receiver<Vec<u8>>>;

impl ConnMeta {
    /// 以给定 MTU 创建元信息，收包时间取当前时间。
    ///
    /// Creates metadata with the given MTU and the current receive time.
    pub fn new(mtu_size: u16) -> Self {
        Self {
            mtu_size,
            recv_time: current_epoch(),
        }
    }
}

/// 游戏层出站命令（普通优先级）：已编码的批量字节。
///
/// Game-layer outbound commands (normal priority): encoded batch bytes.
#[derive(Debug)]
pub enum OutboundCommand {
    /// 游戏层编码后的原始字节（批量），以 ReliableOrd / 信道 0 发送。
    ///
    /// Game-layer encoded raw bytes (batch) over ReliableOrd / channel 0.
    Send { bytes: Vec<u8>, immediate: bool },
}

/// 高优先级协议层命令。
///
/// High-priority protocol-layer commands.
#[derive(Debug)]
pub enum PriorityCommand {
    /// RakNet 协议报文（ConnectedPong / ConnectionAccept / Disconnect）。
    ///
    /// RakNet protocol packets (ConnectedPong / ConnectionAccept / Disconnect).
    RakPacket {
        packet: RakPacket,
        reliability: Reliability,
        immediate: bool,
    },
    /// 客户端 ACK：从恢复队列移除已确认的数据报。
    ///
    /// Client ACK: drops acknowledged datagrams from the recovery queue.
    Ack(Ack),
    /// 客户端 NACK：立即重传丢失的数据报。
    ///
    /// Client NACK: retransmits lost datagrams immediately.
    Nack(Ack),
    /// 关闭发送循环；排在先前命令之后，保证先刷新后退出。
    ///
    /// Shuts down the send loop; queued after earlier commands for flush
    /// semantics.
    Close,
}

/// 专用的保活发送路径（ConnectedPing/ConnectedPong）。
///
/// 该路径不经过队列与 SendQueue，直接 `try_send_to`：
/// 1. 数据报序号来自共享原子计数器（每连接单调递增）；
/// 2. 探测报文不入恢复队列，由周期性重发恢复；
/// 3. 乱序到达由客户端接收侧的 UDP 乱序处理承担。
///
/// Dedicated keepalive send path (ConnectedPing/ConnectedPong).
///
/// The path bypasses queues and SendQueue and calls `try_send_to` directly:
/// 1. datagram numbers come from a shared atomic counter (monotonic per
///    connection);
/// 2. probes skip the recovery queue and recover through the periodic resend;
/// 3. out-of-order arrival is handled by the client's UDP reorder support.
#[derive(Clone, Debug)]
pub(crate) struct KeepaliveChannel {
    socket: Arc<UdpSocket>,
    address: SocketAddr,
    datagram_seq: Arc<AtomicU32>,
}

impl KeepaliveChannel {
    /// 立即发送一个保活报文（不可靠、不重传）。
    ///
    /// Sends one keepalive packet immediately (unreliable, no retransmit).
    pub(crate) fn send(&self, packet: RakPacket) -> Result<(), SendQueueError> {
        let buf = packet
            .write_to_bytes()
            .map_err(|_| SendQueueError::parse_error("Parse error"))?;

        let frame = Frame::new(Reliability::Unreliable, Some(buf.as_slice()));
        let sequence = seq24(self.datagram_seq.fetch_add(1, Ordering::Relaxed));

        let mut pk = FramePacket::new();
        pk.sequence = sequence;
        pk.reliability = frame.reliability;
        pk.frames.push(frame);

        let bytes = pk
            .write_to_bytes()
            .map_err(|_| SendQueueError::parse_error("Parse error"))?;
        let wire = bytes.as_slice();
        crate::dump::record("out", self.address, wire, "datagram");

        if let Err(e) = self.socket.try_send_to(wire, self.address) {
            trace!(
                "[{}] keepalive direct send failed: {:?}",
                to_address_token(self.address),
                e
            );
            return Err(SendQueueError::send_error("Send error"));
        }
        Ok(())
    }
}

/// 一轮周期性维护的间隔。
///
/// 维护内容包括保活探测、冲刷待发帧并重传未确认数据报、执行未确认年龄
/// 策略。
///
/// Cadence of one housekeeping round.
///
/// Housekeeping covers the keepalive probe, flushing queued frames with
/// retransmission, and the unacknowledged-age policy.
const HOUSEKEEPING_INTERVAL: Duration = Duration::from_millis(50);

/// 对端仍在响应时，两次保活探测之间的间隔。
///
/// Interval between keepalive probes of a peer that is answering.
const KEEPALIVE_INTERVAL: Duration = Duration::from_millis(3_000);

/// 按墙钟 deadline 运行、而非由 `select!` 分支驱动的记账状态。
///
/// Bookkeeping that runs on a wall-clock deadline rather than on a `select!`
/// branch.
#[derive(Debug, Default)]
struct HousekeepingState {
    /// 上一轮维护的执行时刻；`None` 表示从未执行。
    ///
    /// When the last round ran; `None` means "never ran".
    last_round: Option<Instant>,
    /// 上一次保活探测的发送时刻。
    ///
    /// When the last keepalive probe was sent.
    last_ping: Option<Instant>,
}

impl HousekeepingState {
    /// 判断 `now` 时刻是否到了执行一轮维护的时间。
    ///
    /// Whether a round is due at `now`.
    fn due(&self, now: Instant) -> bool {
        match self.last_round {
            None => true,
            Some(last) => now.saturating_duration_since(last) >= HOUSEKEEPING_INTERVAL,
        }
    }

    /// 判断 `now` 时刻是否该向对端发送保活探测。
    ///
    /// 该判定独立于维护轮次间隔，额外的维护轮不会加快探测频率。
    ///
    /// Whether the peer is due for a keepalive probe at `now`.
    ///
    /// Kept separate from the round cadence, so extra rounds never turn into
    /// a faster ping.
    fn ping_due(&self, now: Instant) -> bool {
        match self.last_ping {
            None => true,
            Some(last) => now.saturating_duration_since(last) >= KEEPALIVE_INTERVAL,
        }
    }
}

/// 执行一轮周期性维护；返回 `true` 表示必须关闭连接。
///
/// 调用方在每个 `select!` 分支后都通过 [`HousekeepingState::due`] 检查，
/// 使推进取决于流逝时间而非分支位置（`biased` 轮询会跳过后面的定时器分支）。
///
/// One housekeeping round. Returns `true` when the connection must be closed.
///
/// Callers gate on [`HousekeepingState::due`] after every `select!` branch so
/// progress depends on elapsed time rather than branch position (`biased`
/// polling skips the later timer branches).
fn housekeeping_round(
    send_q: &mut SendQueue,
    keepalive: &KeepaliveChannel,
    address: SocketAddr,
    state: &mut HousekeepingState,
    now: Instant,
) -> bool {
    state.last_round = Some(now);

    // Keepalive probe, sent before the flush so a congested queue cannot delay it.
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

    // Unacked-age policy: terminal, the peer's reliable stream already has a hole.
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

/// 向发送循环提交一条控制事实的结果。
///
/// Outcome of offering one control fact to the send loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlOffer {
    /// 发送循环已接管该控制事实。
    ///
    /// The send loop owns it now.
    Handed,
    /// 发送循环积压，未能接收该控制事实。
    ///
    /// The send loop was too far behind to take it.
    Dropped,
}

/// 接收路径中记录发送循环未能接受的控制事实的计数。
///
/// 由 net_recv 任务独占，因此这些数值不需要同步。
///
/// Receive-path counters for control facts the send loop could not accept.
///
/// Owned by the net_recv task, so the numbers need no synchronisation.
#[derive(Debug, Default)]
struct ControlHandoff {
    /// 发送循环未看到的 ACK/NACK 数。
    ///
    /// 可恢复：`SendQueue::update` 会重传对端未确认的数据报，对端也会
    /// 重新确认收到的数据报。
    ///
    /// ACK/NACK the send loop never saw.
    ///
    /// Recoverable: `SendQueue::update` retransmits whatever the peer has not
    /// acknowledged and the peer re-acknowledges what it receives.
    deferred: u64,
    /// 完全无法入队的握手响应数。
    ///
    /// 握手没有重传路径，因此这些计数会导致连接失败。
    ///
    /// Handshake responses that could not be queued at all.
    ///
    /// Not recoverable — there is no retransmit for a handshake — so these
    /// fail the connection.
    refused: u64,
    /// 游戏层没有空间的重组后游戏报文数。
    ///
    /// Reassembled game packets the game layer had no room for.
    overrun: u64,
}

/// 向发送循环提交一条控制事实，绝不阻塞。
///
/// `priority` 为目标信道，`command` 是要提交的控制事实，`recoverable` 表示
/// 该事实是否可由重传恢复，`stats` 记录被丢弃的事实。
///
/// 信道满或已关闭时返回 [`ControlOffer::Dropped`]，由调用方决定其后果；
/// ACK/NACK 记入 `deferred` 交给重传，握手响应记入 `refused` 并导致连接失败。
///
/// Offer one control fact to the send loop without ever blocking.
///
/// `priority` is the target channel, `command` the control fact, `recoverable`
/// whether retransmission can recover it, and `stats` records dropped facts.
///
/// Returns [`ControlOffer::Dropped`] when the channel is full or closed,
/// leaving the consequence to the caller: ACK/NACK counts go to `deferred` for
/// retransmission, handshake responses to `refused`, which fails the
/// connection.
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
                // Counted, not logged per event, to survive NACK storms.
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

/// 把一个重组后的游戏报文交给游戏层，绝不阻塞。
///
/// `sender` 是游戏层信道，`buffer` 是报文字节，`stats` 记录被拒绝的报文。
/// 信道满或已关闭时返回 `Err(原因)`，调用方据此关闭连接。
///
/// 与 [`offer_control`] 相同的约束：本函数不得等待发送路径。RakNet 已确认
/// 该报文所在数据报，因此无法重传恢复。
///
/// Hand one reassembled game packet to the game layer without ever blocking.
///
/// `sender` is the game-layer channel, `buffer` the packet bytes, `stats`
/// records rejected packets. Returns `Err(reason)` when the channel is full or
/// closed, which the caller turns into a connection close.
///
/// Same invariant as [`offer_control`]: this must never wait on the send path.
/// RakNet already acknowledged the datagram carrying the message, so it cannot
/// be recovered by retransmission.
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

/// 单个 RakNet 客户端连接：持有地址、会话状态，并启动 net_recv、send_loop
/// 与 tick 三个任务。
///
/// 所有权划分：`RecvQueue` 由 net_recv 任务独占，`SendQueue` 由 send_loop
/// 任务独占；连接状态、数据报序号、最近收包时间与关闭信号为共享原子值。
///
/// A single RakNet client connection: holds the address and session state and
/// spawns the net_recv, send_loop, and tick tasks.
///
/// Ownership: `RecvQueue` is exclusive to the net_recv task and `SendQueue` to
/// the send_loop task; connection state, datagram numbers, last receive time,
/// and the shutdown signal are shared atomic values.
#[derive(Component, Clone, Debug)]
pub struct Connection {
    pub address: SocketAddr,
    pub state: Arc<AtomicConnectionState>,
    outbound_tx: Sender<QueuedOutboundCommand>,
    outbound_slots: OutboundSlotBudget,
    priority_tx: Sender<PriorityCommand>,
    /// 保活直发路径（不排队、不竞争锁）。
    ///
    /// Keepalive direct-send path (no queueing, no lock contention).
    pub(crate) keepalive: KeepaliveChannel,
    pub internal_net_recv: ConnNetChan,
    disconnect: Arc<Notify>,
    close_notifier: Arc<Sender<SocketAddr>>,
    recv_time: Arc<AtomicU64>,
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl Connection {
    /// 创建连接并启动其任务。
    ///
    /// `address` 为对端地址，`socket` 为服务端 UDP 套接字，`net` 为会话级
    /// 入站数据报信道，`notifier` 用于上报关闭的连接，`mtu` 为协商的 MTU。
    ///
    /// Creates a connection and spawns its tasks.
    ///
    /// `address` is the peer address, `socket` the server UDP socket, `net` the
    /// per-session inbound datagram channel, `notifier` reports closed
    /// connections, and `mtu` the negotiated MTU.
    pub async fn new(
        address: SocketAddr,
        socket: &Arc<UdpSocket>,
        net: Receiver<Vec<u8>>,
        notifier: Arc<Sender<SocketAddr>>,
        mtu: u16,
    ) -> Self {
        // net_recv to game-layer channel: reassembled game packets queue here.
        let (net_sender, net_receiver) = bounded::<Vec<u8>>(256);
        let (outbound_tx, outbound_rx) =
            bounded::<QueuedOutboundCommand>(OUTBOUND_CHANNEL_CAPACITY);
        let outbound_slots = OutboundSlotBudget::new(OUTBOUND_CHANNEL_CAPACITY);
        let (priority_tx, priority_rx) = bounded::<PriorityCommand>(PRIORITY_CHANNEL_CAPACITY);
        // One atomic counter feeds every send path: datagram numbers must be
        // monotonic and unique per connection.
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
        // send_loop owns SendQueue exclusively.
        tasks.push(c.init_send_loop(
            SendQueue::new(mtu, 12000, 5, datagram_seq, socket.clone(), address),
            outbound_rx,
            priority_rx,
            c.keepalive.clone(),
        ));

        c
    }

    /// 启动出站任务：独占 `SendQueue`，消费高优先级与普通命令信道，并按
    /// 周期执行维护轮。
    ///
    /// `select!` 以 `biased` 顺序轮询：关闭信号、高优先级（控制）、普通
    /// （数据）、周期定时器。数据报级 ACK/NACK 由 net_recv 直发，不经过
    /// 本任务。
    ///
    /// Spawns the outbound task: it owns `SendQueue` exclusively, drains the
    /// high-priority and normal command channels, and runs a housekeeping round
    /// periodically.
    ///
    /// The `select!` polls in `biased` order: shutdown, high-priority
    /// (control), normal (data), periodic tick. Datagram-level ACK/NACK replies
    /// are sent directly by net_recv and do not pass through this task.
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
                                // Re-send the stored FramePacket bytes unchanged: the client's
                                // NACK references that datagram's sequence number, and fragment
                                // reassembly depends on it. Re-inserting through `insert()`
                                // would assign a new sequence number and Unreliable reliability.
                                for packet in send_q.nack(nack) {
                                    if let Ok(buffer) = packet.write_to_bytes() {
                                        let _ = send_q.send_stream(buffer.as_slice());
                                    }
                                }
                            }
                            Ok(PriorityCommand::Close) => break,
                            Err(_) => break, // All senders dropped
                        }
                        // Housekeeping must also run while the control plane stays backlogged.
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
                                                // Reliable datagrams can no longer be admitted:
                                                // closing is terminal, keeping the connection
                                                // would silently drop reliable data.
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
                            Err(_) => break, // All senders dropped
                        }
                        // Housekeeping must also run while the data plane stays backlogged.
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

    /// 启动 tick 任务：每 50ms 调用 `tick_body` 更新会话状态，
    /// 连接结束时通过 `notifier` 上报地址。
    ///
    /// Spawns the tick task: it calls `tick_body` every 50 ms to update
    /// the session state and reports the address through `notifier` when the
    /// connection ends.
    pub(crate) fn init_tick(&self, notifier: Arc<Sender<SocketAddr>>) -> JoinHandle<()> {
        let address = self.address;
        let closer = self.disconnect.clone();
        let last_recv = self.recv_time.clone();
        let state = self.state.clone();

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

    /// 单次 tick 的健康检查。
    ///
    /// `address` 用于日志，`closer` 用于发出关闭信号，`last_recv` 为最近
    /// 收包时间，`state` 为共享会话状态。
    ///
    /// 状态迁移：已 `Disconnected` 或 `recv + 15 <= now` 时关闭；
    /// `recv + 10 <= now` 且状态可靠时置为 `TimingOut`。
    ///
    /// One tick of the health check.
    ///
    /// `address` is used for logging, `closer` signals shutdown, `last_recv`
    /// holds the last receive time, and `state` is the shared session state.
    ///
    /// Transitions: closes when already `Disconnected` or when
    /// `recv + 15 <= now`; marks `TimingOut` when `recv + 10 <= now` and the
    /// state is still reliable.
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

    /// 启动入站处理任务：独占 `RecvQueue`（分片重组、ACK 记录、NACK 检测、
    /// 有序分发），并直接用 ACK/NACK 控制报文应答每个数据报。
    ///
    /// `net` 是会话级入站数据报信道，`sender` 是游戏层信道，`socket` 用于
    /// 直发控制报文。
    ///
    /// Spawns the inbound task: it owns `RecvQueue` exclusively (fragment
    /// reassembly, ACK records, NACK detection, ordered dispatch) and answers
    /// every datagram with ACK/NACK control packets sent directly, since
    /// datagram-level packets take no sequence number and need no retransmit.
    ///
    /// `net` is the per-session datagram channel, `sender` the game-layer
    /// channel, and `socket` is used to send control packets directly.
    pub(crate) fn init_net_recv(
        &self,
        net: Receiver<Vec<u8>>,
        sender: Sender<Vec<u8>>,
        socket: Arc<UdpSocket>,
    ) -> JoinHandle<()> {
        let recv_time = self.recv_time.clone();
        // RecvQueue is task-exclusive: accessed through a &mut borrow.
        let mut recv_q = RecvQueue::new();
        let priority = self.priority_tx.clone();
        let keepalive = self.keepalive.clone();
        let disconnect = self.disconnect.clone();
        let state = self.state.clone();
        let address = self.address;

        tokio::spawn(async move {
            // Task-exclusive receive-path counters.
            let mut stats = ControlHandoff::default();
            // Ordered gaps normally heal through retransmission within about a
            // second, so the inspection runs every second; the diagnostic
            // health summary runs slower still.
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
                        // Missing UDP sequences do not gate frame delivery:
                        // retry observed gaps even while no datagrams arrive.
                        Self::flush_ack_nack(&socket, address, &mut recv_q);
                        // A gap that outlives retransmission leaves every later
                        // ordered frame buffered behind it; close the session
                        // with an explicit reason.
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

    /// 直发挂起的 ACK（0xc0）与 NACK（0xa0）数据报。
    ///
    /// 数据报级控制报文不占用数据报序号、也不参与重传，因此直接
    /// `try_send_to` 即可；及时逐报 ACK 是 RakNet 的标准行为，可让对端
    /// 尽早停止重发。
    ///
    /// Sends pending ACK (0xc0) and NACK (0xa0) datagrams directly.
    ///
    /// Datagram-level control packets take no datagram sequence number and are
    /// never retransmitted, so plain `try_send_to` suffices; prompt per-datagram
    /// ACK is standard RakNet behaviour and makes the peer stop resending
    /// sooner.
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

    /// 处理一个入站数据报：更新收包时间、解码帧包、重组并分发报文。
    ///
    /// 必须保持同步：net_recv 是会话数据报信道的唯一读取者，此处的任何
    /// `await` 都会阻塞整个入站方向。
    ///
    /// 返回 `LoopResult`：`Break` 表示必须关闭连接，`Continue` 表示继续。
    ///
    /// Handles one inbound datagram: updates the receive time, decodes the
    /// frame packet, then reassembles and dispatches its messages.
    ///
    /// Must stay synchronous: net_recv is the only reader of the session
    /// datagram channel, so an `await` here would block the whole inbound
    /// direction.
    ///
    /// Returns `LoopResult::Break` when the connection must be closed,
    /// otherwise `LoopResult::Continue`.
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
                    // Insert the datagram, reassemble fragments, then ACK/NACK
                    // immediately.
                    if let Err(e) = recv_q.insert(pk) {
                        if matches!(
                            e,
                            RecvQueueError::OrderChannelExhausted { .. }
                                | RecvQueueError::ReliableWindowExhausted { .. }
                        ) {
                            // Capacity exhaustion must not be ACKed as successful
                            // reliable delivery: end this session immediately.
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
                if let Ok(nack) = Ack::read_from_slice(&payload[..]) {
                    // The peer reports datagrams it did not receive; hand the fact
                    // to send_loop for retransmission (never waits, `offer_control`).
                    offer_control(priority, PriorityCommand::Nack(nack), true, stats);
                }
            }
            ACK => {
                if let Ok(ack) = Ack::read_from_slice(&payload[..]) {
                    // The peer confirms receipt of these server-sent FramePackets;
                    // send_loop drops them from the recovery queue.
                    //
                    // Do NOT call recv_q.ack() here: this ACK references server-sent
                    // sequence numbers, while the RecvQueue NACK set tracks
                    // client-sent ones — separate sequence spaces. Clearing NACK
                    // entries across them would stop the server requesting missing
                    // client packets.
                    //
                    // A deferred ACK keeps its recovery entry longer and is
                    // retransmitted once; see `offer_control`.
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

    /// 分发一个重组后的 RakNet 报文：处理在线协议报文，其余转交游戏层。
    ///
    /// `buffer` 为重组后的报文字节，`address` 为对端地址，`sender` 为游戏层
    /// 信道，`priority` 为高优先级控制信道，`keepalive` 为保活直发路径，
    /// `state` 为共享会话状态，`stats` 记录交接失败的计数。
    ///
    /// 返回 `Ok(true)` 表示应当断开，`Ok(false)` 表示继续，`Err(())` 表示
    /// 处理失败需要关闭连接。
    ///
    /// 与 [`Self::handle_payload`] 一样必须保持同步：入站处理不得等待出站
    /// 路径。
    ///
    /// Dispatches one reassembled RakNet message: online packets are handled
    /// here, everything else is forwarded to the game layer.
    ///
    /// `buffer` holds the reassembled message, `address` the peer, `sender` the
    /// game-layer channel, `priority` the control channel, `keepalive` the
    /// keepalive path, `state` the shared session state, and `stats` counts
    /// failed handoffs.
    ///
    /// Returns `Ok(true)` to disconnect, `Ok(false)` to continue, and
    /// `Err(())` when the connection must be closed.
    ///
    /// Like [`Self::handle_payload`], this must never wait on the outbound
    /// path.
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
                    // Pong goes through the keepalive direct path: unreliable and
                    // sent immediately, never queued behind game packets.
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
                    // High-priority channel, never waits (see `offer_control`).
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
                    Ok(true)
                }
                OnlinePacket::LostConnection(_) => {
                    // Disconnect the client immediately.
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

    /// 接收游戏层的下一个入站报文字节；连接关闭时返回 [`RecvError::Closed`]。
    ///
    /// Receives the next inbound game-layer packet; returns [`RecvError::Closed`]
    /// once the connection is closed.
    pub async fn recv(&self) -> Result<Vec<u8>, RecvError> {
        self.internal_net_recv
            .recv()
            .await
            .map_err(|_| RecvError::closed("Connection is closed"))
    }

    /// 连接是否已关闭（会话状态不再可接收）。
    ///
    /// Whether the connection is closed (the session state no longer accepts
    /// packets).
    pub async fn is_closed(&self) -> bool {
        !self.state.load().is_available()
    }

    /// 在报文钩子与编码之前预留一个计数槽位。
    ///
    /// 预留持续到发送循环把命令转入 `SendQueue` 为止。
    ///
    /// Reserves one count slot before packet hooks and encoding.
    ///
    /// The reservation is held until the send loop has transferred the command
    /// into `SendQueue`.
    pub async fn reserve_outbound_slot(&self) -> Result<OutboundSlotPermit, SendQueueError> {
        self.outbound_slots.acquire().await
    }

    /// 非阻塞预留一个出站槽位，供可重试的分块投递使用。
    ///
    /// Reserves an outbound slot without blocking; used by retryable chunk
    /// delivery.
    pub fn try_reserve_outbound_slot(&self) -> Result<OutboundSlotPermit, OutboundAdmissionError> {
        self.outbound_slots.try_acquire()
    }

    /// 复制 `buffer` 并排队发送；`immediate` 表示立即冲刷。
    ///
    /// Copies `buffer` and queues it for sending; `immediate` flushes at once.
    pub async fn send(&self, buffer: &[u8], immediate: bool) -> Result<(), SendQueueError> {
        let permit = self.reserve_outbound_slot().await?;
        self.send_owned_with_permit(buffer.to_vec(), immediate, permit)
            .await
    }

    /// 排队发送自有的线字节缓冲区，避免额外复制。
    ///
    /// `bytes` 是调用方自有的完整报文，`immediate` 表示立即冲刷。
    ///
    /// Queues an owned wire buffer without duplicating it.
    ///
    /// `bytes` is the caller-owned packet, `immediate` flushes at once.
    pub async fn send_owned(&self, bytes: Vec<u8>, immediate: bool) -> Result<(), SendQueueError> {
        let permit = self.reserve_outbound_slot().await?;
        self.send_owned_with_permit(bytes, immediate, permit).await
    }

    /// 使用编码前预留的槽位转移一个自有线字节缓冲区。
    ///
    /// `bytes` 为报文字节，`immediate` 表示立即冲刷，`permit` 为已预留的槽位。
    ///
    /// Transfers an owned wire buffer using a slot reserved before encoding.
    ///
    /// `bytes` is the packet, `immediate` flushes at once, and `permit` is the
    /// reserved slot.
    pub async fn send_owned_with_permit(
        &self,
        bytes: Vec<u8>,
        immediate: bool,
        permit: OutboundSlotPermit,
    ) -> Result<(), SendQueueError> {
        self.try_send_owned_with_permit(bytes, immediate, permit)
    }

    /// 同步入队版本的 [`Self::send_owned_with_permit`]。
    ///
    /// `bytes` 为报文字节，`immediate` 表示立即冲刷，`permit` 必须是本连接
    /// 预留的槽位。
    ///
    /// 一个预留槽位覆盖所有已排队命令与正在进行的插入；入队为同步操作，
    /// 调用方可在加密与入队之间无需 `await` 地发布有序回执。
    ///
    /// 槽位不属于本连接或信道已满/关闭时返回 `SendError`。
    ///
    /// Synchronous variant of [`Self::send_owned_with_permit`].
    ///
    /// `bytes` is the packet, `immediate` flushes at once, and `permit` must be
    /// a slot reserved on this connection.
    ///
    /// A reserved slot covers every queued command and in-progress insertion;
    /// admission is synchronous so callers can publish an ordered receipt
    /// without an await between encryption and enqueue.
    ///
    /// Returns `SendError` when the permit belongs to another connection or the
    /// channel is full or closed.
    pub fn try_send_owned_with_permit(
        &self,
        bytes: Vec<u8>,
        immediate: bool,
        permit: OutboundSlotPermit,
    ) -> Result<(), SendQueueError> {
        if !Arc::ptr_eq(&self.outbound_slots.semaphore, &permit.budget) {
            return Err(SendQueueError::send_error("Send error"));
        }
        let cmd = QueuedOutboundCommand {
            command: OutboundCommand::Send { bytes, immediate },
            _permit: permit,
        };
        self.outbound_tx
            .try_send(cmd)
            .map_err(|_| SendQueueError::send_error("Send error"))
    }

    /// 关闭连接：把状态置为 `Disconnected`，向高优先级信道排队 Disconnect
    /// 与 `Close`，通知监听侧，并终止本连接的所有任务。
    ///
    /// 只使用 `try_send`：发送循环已死亡时阻塞发送会永久挂起本函数。
    ///
    /// Closes the connection: marks the state `Disconnected`, queues Disconnect
    /// and `Close` on the high-priority channel, reports the address to the
    /// listener, and aborts every task of this connection.
    ///
    /// Only `try_send` is used; a blocking send would hang here forever once
    /// the send loop is dead.
    pub async fn close(&self) {
        trace!("[{}] Dropping connection!", to_address_token(self.address));
        self.state.store(ConnectionState::Disconnected);

        // Disconnect first, then Close: channel FIFO flushes the Disconnect
        // datagram before the loop exits.
        let disconnect_packet = OnlinePacket::Disconnect(Disconnect {});
        let _ = self.priority_tx.try_send(PriorityCommand::RakPacket {
            packet: disconnect_packet.into(),
            reliability: Reliability::Reliable,
            immediate: true,
        });
        let _ = self.priority_tx.try_send(PriorityCommand::Close);

        self.disconnect.notify();

        // Report the address explicitly: the tick task is aborted before it can.
        let _ = self.close_notifier.try_send(self.address);

        // Leave send_loop a scheduling window to flush the Disconnect datagram.
        tokio::time::sleep(Duration::from_millis(5)).await;

        // recv_queue is freed together with the aborted net_recv future.
        let tasks = self.tasks.clone();

        for task in tasks.lock().drain(..) {
            task.abort();
        }
    }
}

/// 最后一个 `Connection` 克隆被丢弃时终止仍在运行的任务。
///
/// `Arc::strong_count(&tasks) == 1` 保证只在真正的最后一个引用上终止：
/// 临时克隆（例如取自 `get_component`）不得提前杀掉原连接仍在使用的任务。
///
/// Aborts the remaining tasks when the last `Connection` clone is dropped, so a
/// connection that never called `close()` does not leave its tick and net_recv
/// tasks running.
///
/// The `Arc::strong_count(&tasks) == 1` check keeps temporary clones (e.g. from
/// `get_component`) from killing tasks the original still uses.
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

    /// 按 `flush_ack_nack` 的线上格式编码一个 ACK 数据报。
    ///
    /// Encodes an ACK datagram in the wire form `flush_ack_nack` produces.
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

    /// 控制信道满时入站处理必须立即返回：在此等待会停住会话数据报信道的
    /// 唯一读取者，之后服务端会丢弃所有后续入站数据报。
    ///
    /// With a full control channel, inbound processing must return immediately:
    /// blocking there stops the only reader of the session datagram channel, and
    /// the server socket loop then drops every further inbound datagram.
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

    /// 绑定一个真实对端套接字，以便观察到达的保活探测。
    ///
    /// Binds a real peer socket so keepalive probes can be observed arriving.
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

    /// 统计对端收到的保活探测数量，忽略其他数据报。
    ///
    /// Counts the keepalive probes a peer received, ignoring other datagrams.
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

        // A small non-immediate payload stays in `ready` until `update()` writes
        // it to the wire, so the queue length shows whether the round ran.
        send_q
            .insert(&[0xABu8; 64], Reliability::ReliableOrd, false, Some(0))
            .expect("queue a non-immediate payload");
        assert!(
            send_q.ready_len() > 0,
            "payload must be waiting for the flush"
        );

        // `biased` polls the first ready branch only, so with both planes
        // backlogged the timer branch is never selected.
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

        // Drain probes already sent, re-arm both deadlines, and check that a round
        // still emits one probe without any timer branch.
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
            Err(OutboundAdmissionError::Busy { .. })
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
            Err(OutboundAdmissionError::Busy { .. })
        ));
        assert_eq!(guards_dropped.load(Ordering::Relaxed), 0);

        drop(queued);
        assert_eq!(guards_dropped.load(Ordering::Relaxed), 1);
        assert!(budget.try_acquire().is_ok());
        drop(second);
    }
}
