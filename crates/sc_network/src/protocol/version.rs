//! 连接级 Bedrock 协议版本，用于包编解码时的版本分支选择。
//!
//! Connection-scoped Bedrock protocol version for packet encoding.
//!
//! Packet layouts differ between protocol versions. Every encode and decode
//! branch must use the version negotiated for the connection being served,
//! never a process-wide default. This module carries that version through
//! the synchronous serialize and deserialize sections.

use std::cell::Cell;

use super::ProtocolInfo;

/// Bedrock 1.26.40 网络协议版本。
///
/// Bedrock 1.26.40 network protocol version.
pub const PROTOCOL_VERSION_1_26_40: u32 = 2168;
/// Bedrock 1.26.50 与 1.26.51 网络协议版本。
///
/// Bedrock 1.26.50 and 1.26.51 network protocol version.
pub const PROTOCOL_VERSION_1_26_50: u32 = 2193;
/// Bedrock 1.26.60 网络协议版本（预览线）。
///
/// Bedrock 1.26.60 network protocol version (preview line).
pub const PROTOCOL_VERSION_1_26_60: u32 = 2225;

/// 本服务端接受的协议版本列表，由旧到新排列。
///
/// Protocol versions this server accepts, oldest first.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[u32] = &[
    PROTOCOL_VERSION_1_26_40,
    PROTOCOL_VERSION_1_26_50,
    PROTOCOL_VERSION_1_26_60,
];

thread_local! {
    static SCOPED_PROTOCOL_VERSION: Cell<u32> = const { Cell::new(0) };
}

/// 在一次同步编解码区间内固定连接协议版本的守卫。
///
/// Guard that pins the connection protocol version for one synchronous
/// encode or decode section. Serialization never awaits, so the value
/// cannot leak across tasks.
pub struct ProtocolVersionScope {
    previous: u32,
}

impl ProtocolVersionScope {
    /// 创建守卫并将线程局部协议版本替换为 `version`。
    ///
    /// Creates the guard and replaces the thread-local protocol version with
    /// `version`.
    ///
    /// - `version`: 要固定的协议版本
    pub fn new(version: u32) -> Self {
        let previous = SCOPED_PROTOCOL_VERSION.with(|slot| slot.replace(version));
        Self { previous }
    }
}

impl Drop for ProtocolVersionScope {
    fn drop(&mut self) {
        SCOPED_PROTOCOL_VERSION.with(|slot| slot.set(self.previous));
    }
}

/// 在固定连接协议版本的前提下执行 `f`。
///
/// Runs `f` with the connection protocol version pinned.
///
/// - `version`: 要固定的协议版本
/// - `f`: 待执行的闭包
pub fn with_protocol_version<R>(version: u32, f: impl FnOnce() -> R) -> R {
    let _scope = ProtocolVersionScope::new(version);
    f()
}

/// 当前正在编解码的包所使用的协议版本。
///
/// Protocol version for the packet currently being encoded or decoded.
///
/// Resolution order: scoped connection version, negotiated range maximum,
/// zero (legacy path). Zero also covers the pre-login phase, where the
/// connection has no version yet and the previous global-max behavior
/// applies unchanged.
pub fn current_protocol_version() -> u32 {
    let scoped = SCOPED_PROTOCOL_VERSION.with(|slot| slot.get());
    if scoped != 0 {
        return scoped;
    }
    ProtocolInfo::global()
        .and_then(|info| info.protocol_versions.get_max_version())
        .unwrap_or(0)
}

/// 当前包所针对的协议版本是否不低于 `version`。
///
/// True when the packet in progress targets at least `version`.
///
/// - `version`: 待比较的协议版本
pub fn protocol_at_least(version: u32) -> bool {
    current_protocol_version() >= version
}
