use crate::connection::{ConnMeta, Connection};
use crate::loop_exec;
use crate::notify::Notify;
use crate::protocol::mcbe::UnconnectedPong;
use crate::protocol::packet::offline::{
    IncompatibleProtocolVersion, OfflinePacket, OpenConnectReply, SessionInfoReply,
};
use crate::protocol::packet::RakPacket;
use crate::protocol::Magic;
use crate::utils::{to_address_token, LoopResult};
use arc_swap::ArcSwap;
use async_channel::{bounded, Receiver, Sender, TrySendError};
use log::trace;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::select;
use tokio::task::JoinHandle;
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::ByteReader;
use sc_ecs::resource::Resource;
use sc_utils::game::structs::motd::Motd;

pub(crate) type Session = (ConnMeta, Sender<Vec<u8>>);

const MIN_MTU_SIZE: u16 = 576;
const MAX_MTU_SIZE: u16 = 2048;
const MAX_SESSIONS: usize = 4096;
const MAX_SESSIONS_PER_IP: usize = 32;
const MAX_PENDING_CONNECTIONS: usize = 256;

/// UDP read buffer size = `MAX_MTU_SIZE` plus headroom.
///
/// Without headroom, a full-MTU client fills the buffer exactly and any
/// over-MTU malformed/attack packet is silently truncated by `recv_from`
/// instead of being recognized and dropped. Headroom lets over-length
/// datagrams be detected and rejected.
const RECV_BUFFER_SIZE: usize = MAX_MTU_SIZE as usize + 64;

/// Per-connection listener to net_recv channel capacity.
/// Login/chunk phases burst datagrams; too-small capacity with drop-on-full
/// would kill live connections in packet storms.
const SESSION_CHANNEL_CAPACITY: usize = 1024;

/// Connection-close report channel capacity (tick task to cleanup task).
const CLOSE_NOTIFY_CAPACITY: usize = 128;

fn clamp_mtu_size(mtu_size: u16) -> u16 {
    mtu_size.clamp(MIN_MTU_SIZE, MAX_MTU_SIZE)
}

pub enum SCSocketAddr<'a> {
    SocketAddr(SocketAddr),
    Str(&'a str),
    String(String),
    ActuallyNot,
}

impl SCSocketAddr<'_> {
    pub fn to_socket_addr(self) -> Option<SocketAddr> {
        match self {
            SCSocketAddr::SocketAddr(addr) => Some(addr),
            SCSocketAddr::Str(addr) => addr.parse::<SocketAddr>().ok(),
            SCSocketAddr::String(addr) => {
                if let Ok(addr) = addr.parse::<SocketAddr>() {
                    Some(addr.clone())
                } else {
                    if let Ok(mut addr) = addr.to_socket_addrs() {
                        if let Some(v) = addr.next() {
                            Some(v)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
            }
            _ => None,
        }
    }
}

impl From<&str> for SCSocketAddr<'_> {
    fn from(s: &str) -> Self {
        Self::String(s.to_string())
    }
}

impl From<String> for SCSocketAddr<'_> {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}

impl From<SocketAddr> for SCSocketAddr<'_> {
    fn from(s: SocketAddr) -> Self {
        Self::SocketAddr(s)
    }
}

impl Display for SCSocketAddr<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SCSocketAddr::SocketAddr(addr) => write!(f, "{}", addr),
            SCSocketAddr::Str(addr) => write!(f, "{}", addr),
            SCSocketAddr::String(addr) => write!(f, "{}", addr),
            SCSocketAddr::ActuallyNot => write!(f, "Not a valid address!"),
        }
    }
}

#[derive(Debug)]
pub enum ListenerError {
    InvalidAddress(String),
    UdpSocketError(io::Error),
}

impl Display for ListenerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for ListenerError {}

#[derive(Debug)]
pub enum ServerError {
    AlreadyOnline,
    NotListening,
    Killed,
}

impl Display for ServerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for ServerError {}

#[derive(Resource, Clone)]
pub struct Listener {
    pub motd: Arc<RwLock<Motd>>,
    pub guid: Arc<AtomicU64>,
    pub versions: &'static [u8],
    udp_socket: Arc<RwLock<Option<Arc<UdpSocket>>>>,
    /// Session table: read-heavy (once per inbound datagram, writes only
    /// on connect/disconnect). Lock-free reads with copy-on-write updates.
    connections: Arc<ArcSwap<HashMap<SocketAddr, Session>>>,
    serving: Arc<AtomicBool>,
    connection_sender: Arc<Sender<Connection>>,
    connection_receiver: Arc<Receiver<Connection>>,
    closer: Arc<Notify>,
    loop_thread: Arc<RwLock<Option<JoinHandle<()>>>>,
}

impl Listener {
    pub async fn bind<I: for<'a> Into<SCSocketAddr<'a>>>(
        address: I,
        motd: Arc<RwLock<Motd>>,
    ) -> Result<Self, ListenerError> {
        let address = address.into();
        let a_string = format!("{}", address);
        let address = address
            .to_socket_addr()
            .ok_or(ListenerError::InvalidAddress(a_string))?;

        let udp_socket = UdpSocket::bind(address)
            .await
            .map_err(|e| ListenerError::UdpSocketError(e))?;
        let guid = motd.read().server_guid;

        let (sender, receiver) = async_channel::bounded(MAX_PENDING_CONNECTIONS);

        Ok(Self {
            motd,
            guid: Arc::new(AtomicU64::new(guid)),
            versions: &[10, 11],
            udp_socket: Arc::new(RwLock::new(Some(Arc::new(udp_socket)))),
            connections: Arc::new(ArcSwap::from_pointee(HashMap::new())),
            serving: Arc::new(AtomicBool::new(false)),
            connection_sender: Arc::new(sender),
            connection_receiver: Arc::new(receiver),
            closer: Arc::new(Notify::new()),
            loop_thread: Arc::new(RwLock::new(None)),
        })
    }

    pub async fn start(&self) -> Result<(), ServerError> {
        if self.serving.load(Ordering::Acquire) {
            return Err(ServerError::AlreadyOnline);
        }

        let socket = self
            .udp_socket
            .read()
            .as_ref()
            .cloned()
            .ok_or(ServerError::NotListening)?;
        let connections = self.connections.clone();
        let connection_sender = self.connection_sender.clone();
        let closer = self.closer.clone();
        let motd = self.motd.clone();
        let guid = self.guid.clone();
        let versions = self.versions;
        self.serving.store(true, Ordering::Release);

        let (cs, client_close_recv) = bounded::<SocketAddr>(CLOSE_NOTIFY_CAPACITY);
        let client_close_send = Arc::new(cs);

        // Spawn a cleanup task that consumes the client_close channel.
        // When a Connection's tick task exits (timeout or disconnect), it
        // sends the client's SocketAddr through this channel. Without this
        // consumer, the `sessions` HashMap grew unbounded — one entry per
        // unique client address, never removed, each holding a Sender<Vec<u8>>
        // and ConnMeta that could never be garbage collected.
        let cleanup_connections = connections.clone();
        let cleanup_closer = closer.clone();
        tokio::spawn(async move {
            loop {
                select! {
                    _ = cleanup_closer.wait() => {
                        trace!("[RakNet Server] Session cleanup task shutting down!");
                        break;
                    }
                    recv = client_close_recv.recv() => {
                        match recv {
                            Ok(address) => {
                                let existed = cleanup_connections.load().contains_key(&address);
                                if existed {
                                    cleanup_connections.rcu(|sessions| {
                                        let mut m = (**sessions).clone();
                                        m.remove(&address);
                                        m
                                    });
                                    trace!(
                                        "[{}] Removed closed session from sessions map",
                                        to_address_token(address)
                                    );
                                }
                            }
                            Err(_) => {
                                // Channel closed (sender dropped), exit
                                trace!("[RakNet Server] Session cleanup channel closed, exiting!");
                                break;
                            }
                        }
                    }
                }
            }
        });

        let loop_thread = tokio::spawn(async move {
            // Buffer size derives from MAX_MTU_SIZE (see RECV_BUFFER_SIZE):
            // a hardcoded equal-size buffer would silently truncate full-MTU clients.
            let mut buf = [0u8; RECV_BUFFER_SIZE];
            loop {
                select! {
                    _ = closer.wait() => {
                        trace!("[RakNet Server] Server has recieved the shutdown notification!");
                        break;
                    }
                    recv = socket.recv_from(&mut buf) => {
                        loop_exec!(Self::recv_packet(recv,
                            &mut buf,
                            socket.clone(),
                            connections.clone(),
                            motd.clone(),
                            guid.clone(),
                            versions,
                            connection_sender.clone(),
                            client_close_send.clone(),
                        ).await);
                    }
                }
            }
        });

        self.loop_thread.write().replace(loop_thread);

        Ok(())
    }

    async fn recv_packet(
        result: io::Result<(usize, SocketAddr)>,
        buf: &mut [u8],
        socket: Arc<UdpSocket>,
        connections: Arc<ArcSwap<HashMap<SocketAddr, Session>>>,
        motd: Arc<RwLock<Motd>>,
        guid: Arc<AtomicU64>,
        versions: &'static [u8],
        connection_sender: Arc<Sender<Connection>>,
        client_close_send: Arc<Sender<SocketAddr>>,
    ) -> LoopResult {
        let guid = guid.load(Ordering::Acquire);
        let length: usize;
        let origin: SocketAddr;
        match result {
            Ok((l, o)) => {
                length = l;
                origin = o;
            }
            Err(e) => {
                return match e.kind() {
                    io::ErrorKind::ConnectionReset => LoopResult::Continue,
                    _ => {
                        trace!("[SERVER-SOCKET] Failed to recieve packet! {}", e);
                        LoopResult::Continue
                    }
                }
            }
        }

        if let Ok(pk) = OfflinePacket::read(&mut ByteReader::from(&buf[..length])) {
            match pk {
                OfflinePacket::UnconnectedPing(_) => {
                    let motd = (*motd.read()).clone();
                    let resp = UnconnectedPong {
                        timestamp: current_epoch(),
                        server_id: guid,
                        magic: Magic::new(),
                        motd,
                    };

                    send_packet_to_socket(&socket, resp.into(), origin);
                    LoopResult::Continue
                }
                OfflinePacket::OpenConnectRequest(mut pk) => {
                    if !versions.contains(&pk.protocol) {
                        let resp = IncompatibleProtocolVersion {
                            protocol: pk.protocol,
                            magic: Magic::new(),
                            server_id: guid,
                        };

                        trace!("[{}] Sent ({}) which is invalid RakNet protocol. Version is incompatible with server.", pk.protocol, to_address_token(*&origin));

                        send_packet_to_socket(&socket, resp.into(), origin);
                        return LoopResult::Continue;
                    }

                    trace!(
                        "[{}] Client requested Mtu Size: {}",
                        to_address_token(*&origin),
                        pk.mtu_size
                    );

                    let requested_mtu = pk.mtu_size;
                    pk.mtu_size = clamp_mtu_size(pk.mtu_size);
                    if pk.mtu_size != requested_mtu {
                        trace!(
                            "[{}] Client requested Mtu Size: {} which was clamped to {}",
                            to_address_token(*&origin),
                            requested_mtu,
                            pk.mtu_size
                        );
                    }

                    let resp = OpenConnectReply {
                        server_id: guid,
                        // TODO allow encryption
                        security: false,
                        magic: Magic::new(),
                        // TODO make this configurable, this is sent to the client to change
                        // it's mtu size, right now we're using what the client prefers.
                        // however in some cases this may not be the preferred use case, for instance
                        // on servers with larger worlds, you may want a larger mtu size, or if
                        // your limited on network bandwith
                        mtu_size: pk.mtu_size,
                    };
                    send_packet_to_socket(&socket, resp.into(), origin);
                    LoopResult::Continue
                }
                OfflinePacket::SessionInfoRequest(pk) => {
                    let mtu_size = clamp_mtu_size(pk.mtu_size);
                    let resp = SessionInfoReply {
                        server_id: guid,
                        client_address: origin,
                        magic: Magic::new(),
                        mtu_size,
                        security: false,
                    };

                    // This is a valid packet, let's check if a session exists, if not, we should create it.
                    // Event if the connection is only in offline mode.
                    let should_create_session = !connections.load().contains_key(&origin);

                    if should_create_session {
                        {
                            let sessions = connections.load();
                            if sessions.len() >= MAX_SESSIONS && !sessions.contains_key(&origin) {
                                trace!(
                                    "[{}] Refusing new RakNet session: session limit {} reached",
                                    to_address_token(origin),
                                    MAX_SESSIONS
                                );
                                return LoopResult::Continue;
                            }
                            let ip_sessions = sessions
                                .keys()
                                .filter(|address| address.ip() == origin.ip())
                                .count();
                            if ip_sessions >= MAX_SESSIONS_PER_IP {
                                trace!(
                                    "[{}] Refusing new RakNet session: per-IP limit {} reached",
                                    to_address_token(origin),
                                    MAX_SESSIONS_PER_IP
                                );
                                return LoopResult::Continue;
                            }
                        }

                        trace!("Creating new session for {}", origin);
                        let meta = ConnMeta::new(mtu_size);
                        let (net_send, net_recv) = bounded::<Vec<u8>>(SESSION_CHANNEL_CAPACITY);
                        let connection = Connection::new(
                            origin,
                            &socket,
                            net_recv,
                            client_close_send.clone(),
                            mtu_size,
                        )
                        .await;
                        trace!("Created Session for {}", origin);

                        // Add the connection to the available connections list.
                        // we're using the name "sessions" here to differeniate
                        // for some reason the reciever likes to be dropped, so we're saving it here.
                        //
                        // Duplicate/capacity check and insert complete in one atomic
                        // update; concurrent SessionInfoRequest races retry
                        // automatically, and only the winning attempt counts.
                        let inserted = {
                            let mut did_insert = false;
                            connections.rcu(|sessions| {
                                did_insert = false;
                                let mut m = (**sessions).clone();
                                if !m.contains_key(&origin)
                                    && m.len() < MAX_SESSIONS
                                    && m.keys()
                                        .filter(|address| address.ip() == origin.ip())
                                        .count()
                                        < MAX_SESSIONS_PER_IP
                                {
                                    m.insert(origin, (meta, net_send.clone()));
                                    did_insert = true;
                                }
                                m
                            });
                            did_insert
                        };

                        if !inserted {
                            trace!(
                                "[{}] RakNet session creation lost a capacity race",
                                to_address_token(origin)
                            );
                            connection.close().await;
                            return LoopResult::Continue;
                        }

                        // notify the connection communicator
                        if let Err(err) = connection_sender.send(connection).await {
                            let connection = err.0;
                            // there was an error, and we should terminate this connection immediately.
                            trace!("[{}] Error while communicating with internal connection channel! Connection withdrawn.", to_address_token(connection.address));
                            connections.rcu(|sessions| {
                                let mut m = (**sessions).clone();
                                m.remove(&origin);
                                m
                            });
                            connection.close().await;
                            return LoopResult::Continue;
                        }
                    }

                    // update the sessions mtuSize, this is referred to internally, we also will send this event to the client
                    // event channel. However we are not expecting a response.

                    connections.rcu(|sessions| {
                        let mut m = (**sessions).clone();
                        if let Some(session) = m.get_mut(&origin) {
                            session.0.mtu_size = mtu_size;
                        }
                        m
                    });
                    trace!(
                        "[{}] Updated mtu size to {}",
                        to_address_token(origin),
                        mtu_size
                    );

                    send_packet_to_socket(&socket, resp.into(), origin);
                    LoopResult::Continue
                }
                _ => {
                    trace!("[{}] Received invalid packet!", to_address_token(*&origin));
                    LoopResult::Continue
                }
            }
        } else {
            // Hot path: lock-free ArcSwap load; the Guard releases as soon as the
            // Sender is cloned (never held across await).
            let sender = connections
                .load()
                .get(&origin)
                .map(|session| session.1.clone());
            if let Some(sender) = sender {
                match sender.try_send(buf[..length].to_vec()) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        // Channel full: drop the datagram. It never entered
                        // RecvQueue, so the server won't ACK it and the client
                        // recovers via NACK/retransmit. Never kill the
                        // connection on Full: packet storms would murder live
                        // connections.
                        trace!(
                            "[{}] Inbound session channel full, dropping datagram (client will retransmit)",
                            to_address_token(origin)
                        );
                    }
                    Err(TrySendError::Closed(_)) => {
                        // Channel truly closed (connection destroyed): remove the session.
                        trace!(
                            "[{}] Session channel closed, removing session",
                            to_address_token(origin)
                        );
                        connections.rcu(|sessions| {
                            let mut m = (**sessions).clone();
                            m.remove(&origin);
                            m
                        });
                    }
                }
            }
            LoopResult::Continue
        }
    }

    pub async fn accept(&self) -> Result<Connection, ServerError> {
        if !self.serving.load(Ordering::Acquire) {
            Err(ServerError::NotListening)
        } else {
            self.connection_receiver
                .recv()
                .await
                .map_err(|_| ServerError::Killed)
        }
    }

    pub async fn stop(&self) -> Result<(), ServerError> {
        if !self.serving.swap(false, Ordering::AcqRel) {
            return Ok(());
        }
        self.closer.notify();
        *self.udp_socket.write() = None;

        let loop_thread = self.loop_thread.write().take();
        if let Some(loop_thread) = loop_thread {
            loop_thread.abort();
            let _ = loop_thread.await;
        }

        // The listener only stores the packet senders; the owning Connection
        // components may live in ECS. Dropping these session senders prevents
        // a stopped listener from retaining every client address indefinitely.
        self.connections.store(Arc::new(HashMap::new()));
        Ok(())
    }
}

fn send_packet_to_socket(socket: &Arc<UdpSocket>, packet: RakPacket, origin: SocketAddr) {
    if let Ok(bytes) = packet.write_to_bytes() {
        if let Err(e) = socket.try_send_to(bytes.as_slice(), origin) {
            trace!(
                "[{}] Failed sending payload to socket! {}",
                to_address_token(origin),
                e
            );
        }
    } else {
        trace!(
            "[{}] Failed serializing offline RakNet response",
            to_address_token(origin)
        );
    }
}

pub(crate) fn current_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
