//! Connection-scoped Bedrock protocol version for packet encoding.
//!
//! Packet layouts differ between protocol versions. Every encode and decode
//! branch must use the version negotiated for the connection being served,
//! never a process-wide default. This module carries that version through
//! the synchronous serialize and deserialize sections.

use std::cell::Cell;

use super::ProtocolInfo;

/// Bedrock 1.26.40 network protocol version.
pub const PROTOCOL_VERSION_1_26_40: u32 = 2168;
/// Bedrock 1.26.50 and 1.26.51 network protocol version.
pub const PROTOCOL_VERSION_1_26_50: u32 = 2193;
/// Bedrock 1.26.60 network protocol version (preview line).
pub const PROTOCOL_VERSION_1_26_60: u32 = 2225;

/// Protocol versions this server accepts, oldest first.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[u32] = &[
    PROTOCOL_VERSION_1_26_40,
    PROTOCOL_VERSION_1_26_50,
    PROTOCOL_VERSION_1_26_60,
];

thread_local! {
    static SCOPED_PROTOCOL_VERSION: Cell<u32> = const { Cell::new(0) };
}

/// Guard that pins the connection protocol version for one synchronous
/// encode or decode section. Serialization never awaits, so the value
/// cannot leak across tasks.
pub struct ProtocolVersionScope {
    previous: u32,
}

impl ProtocolVersionScope {
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

/// Run `f` with the connection protocol version pinned.
pub fn with_protocol_version<R>(version: u32, f: impl FnOnce() -> R) -> R {
    let _scope = ProtocolVersionScope::new(version);
    f()
}

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

/// True when the packet in progress targets at least `version`.
pub fn protocol_at_least(version: u32) -> bool {
    current_protocol_version() >= version
}
