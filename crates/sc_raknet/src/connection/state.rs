use std::sync::atomic::{AtomicU8, Ordering};

/// 连接状态：RakNet 会话所有可能的状态。
///
/// 虽然对外可见，但请注意这些状态并非原始 RakNet 实现所依赖的状态集，
/// 原实现只保留 "Unconnected" 与 "Connected"。
///
/// Connection states: all possible states of a RakNet session.
///
/// While accessible externally, note that these are not the states relied
/// on within the original implementation of raknet, which preserve both
/// "Unconnected" and "Connected".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConnectionState {
    /// 会话尚未连接，但正在主动尝试连接。
    /// 处于该状态的客户端被视为正在积极建立连接。
    ///
    /// The session is not yet connected, but is actively trying to
    /// connect. Clients in this state are considered to be actively
    /// trying to connect.
    Connecting,

    /// 会话已连接，可以收发报文。
    /// 这是连接建立之后的状态。
    ///
    /// 该状态在 `ConnectionHandshake` 完成后应用。
    ///
    /// The session is connected and ready to send and receive packets.
    /// This is the state after a connection has been established.
    ///
    /// This state is applied once the `ConnectionHandshake` has been
    /// completed.
    Connected,

    /// 会话因长时间未发送报文而即将超时。
    /// 超时间隔可在会话选项中设置。
    ///
    /// The session is being timed out because it has not sent a packet in
    /// a while. The interval for this can be set in the session options.
    TimingOut,

    /// 会话已被断开，但仍在清理过程中。
    /// 这是已请求断开、但客户端仍希望继续发送报文直至结束时的状态。
    ///
    /// The session has been disconnected but is still in the process of
    /// cleaning up. This is the state after a disconnect has been
    /// requested, but the client still wants to send packets until it's
    /// done.
    Disconnecting,

    /// 会话已断开，可以被移除。
    /// 这是断开请求发出且客户端完成发送后的状态，几乎不会使用。
    ///
    /// The session has been disconnected and is ready to be removed.
    /// This is the state after a disconnect has been requested and the
    /// client has finished sending. This is almost never used.
    Disconnected,

    /// 会话在响应服务端但并未真正连接。
    /// 该状态下收发 ping/pong 报文；类似原始的 "Unconnected" 状态，
    /// 因而有 "UnconnectedPing"。
    ///
    /// The session is replying to the server but is not actually
    /// connected. This is the state where ping and pong packets are being
    /// sent. Similarly, this is the "Unconnected" state, hence
    /// "UnconnectedPing".
    Unidentified,

    /// 会话已被识别，可以进入连接。
    /// 这是连接建立之后的状态。
    ///
    /// The session has been identified and is ready to be connected.
    /// This is the state after a connection has been established.
    Identified,

    /// 会话未连接且未尝试连接。
    /// 处于该状态时会话会被丢弃；发生在客户端完全停止响应或其套接字
    /// 已销毁时。该状态与 [`Disconnected`] 不同。
    ///
    /// The session is not connected and is not trying to connect. During
    /// this state the session will be dropped. This state occurs when a
    /// client has completely stopped responding to packets or their
    /// socket is destroyed. This is not the same as the [`Disconnected`]
    /// state.
    ///
    /// [`Disconnected`]: crate::connection::state::ConnectionState::Disconnected
    Offline,
}

impl ConnectionState {
    /// 状态的 u8 编码（供 [`AtomicConnectionState`] 使用）。
    ///
    /// u8 encoding of the state (for [`AtomicConnectionState`]).
    pub fn as_u8(&self) -> u8 {
        match self {
            Self::Connecting => 0,
            Self::Connected => 1,
            Self::TimingOut => 2,
            Self::Disconnecting => 3,
            Self::Disconnected => 4,
            Self::Unidentified => 5,
            Self::Identified => 6,
            Self::Offline => 7,
        }
    }

    /// 从 u8 编码还原状态；未知值退化为 [`ConnectionState::Offline`]。
    ///
    /// Restores a state from its u8 encoding; unknown values degrade to
    /// [`ConnectionState::Offline`].
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Connecting,
            1 => Self::Connected,
            2 => Self::TimingOut,
            3 => Self::Disconnecting,
            4 => Self::Disconnected,
            5 => Self::Unidentified,
            6 => Self::Identified,
            _ => Self::Offline,
        }
    }

    /// 会话是否“可靠”（即可继续收发可靠报文）。
    /// 不可靠的状态为：
    /// - Offline
    /// - Disconnected
    /// - TimingOut
    ///
    /// Returns whether or not the session is reliable.
    /// Reliable sessions are sessions that are not:
    /// - Offline
    /// - Disconnected
    /// - TimingOut
    pub fn is_reliable(&self) -> bool {
        match self {
            Self::Disconnected | Self::TimingOut | Self::Offline => false,
            _ => true,
        }
    }

    /// 会话当前是否可接收报文。属于该集合的状态：
    /// - Connected
    /// - Connecting
    /// - Unidentified
    /// - Disconnecting
    ///
    /// Returns whether or not the session is available to receive
    /// packets. Sessions in this state are:
    /// - Connected
    /// - Connecting
    /// - Unidentified
    /// - Disconnecting
    pub fn is_available(&self) -> bool {
        match self {
            Self::Connected | Self::Connecting | Self::Unidentified | Self::Disconnecting => true,
            _ => false,
        }
    }

    /// 会话是否处于任一“已连接”状态。属于该集合的状态：
    /// - Connected
    /// - Connecting
    ///
    /// Returns whether or not the session is in any "connected" state.
    /// Sessions in this state are:
    /// - Connected
    /// - Connecting
    pub fn is_connected(&self) -> bool {
        match self {
            Self::Connected | Self::Connecting => true,
            _ => false,
        }
    }
}

impl std::fmt::Display for ConnectionState {
    /// 以变体名（如 `Connected`）格式化输出。
    ///
    /// Formats the state as its variant name (e.g. `Connected`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connecting => write!(f, "Connecting"),
            Self::Connected => write!(f, "Connected"),
            Self::TimingOut => write!(f, "TimingOut"),
            Self::Disconnecting => write!(f, "Disconnecting"),
            Self::Disconnected => write!(f, "Disconnected"),
            Self::Unidentified => write!(f, "Unidentified"),
            Self::Identified => write!(f, "Identified"),
            Self::Offline => write!(f, "Offline"),
        }
    }
}

/// [`ConnectionState`] 的无锁原子包装。
///
/// 状态迁移来自并发的 tick / net_recv / close 任务，但每次访问都是单次
/// load/store，因此普通原子 u8 足够（Relaxed：该状态不守护其他数据）。
///
/// Lock-free atomic wrapper for [`ConnectionState`].
///
/// Transitions fire from concurrent tick / net_recv / close tasks, but
/// each access is a single load/store, so a plain atomic u8 suffices
/// (Relaxed: the state guards no other data).
#[derive(Debug)]
pub struct AtomicConnectionState(AtomicU8);

impl AtomicConnectionState {
    /// 以给定初始状态构造原子状态。
    ///
    /// Constructs an atomic state with the given initial state.
    pub fn new(state: ConnectionState) -> Self {
        Self(AtomicU8::new(state.as_u8()))
    }

    /// 读取当前状态。
    ///
    /// Loads the current state.
    pub fn load(&self) -> ConnectionState {
        ConnectionState::from_u8(self.0.load(Ordering::Relaxed))
    }

    /// 写入新状态。
    ///
    /// Stores a new state.
    pub fn store(&self, state: ConnectionState) {
        self.0.store(state.as_u8(), Ordering::Relaxed);
    }
}

impl Default for AtomicConnectionState {
    /// 默认状态为 [`ConnectionState::Unidentified`]。
    ///
    /// The default state is [`ConnectionState::Unidentified`].
    fn default() -> Self {
        Self::new(ConnectionState::Unidentified)
    }
}
