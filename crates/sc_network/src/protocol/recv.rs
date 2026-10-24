//! 客户端协议包接收事件定义。
//!
//! Client protocol packet reception event definitions.

use crate::protocol::MinecraftPacket;
use sc_ecs::entity::EntityId;
use sc_ecs::event::Event;

/// 收到一个客户端协议包时发出的事件。
///
/// The event emitted when a client protocol packet is received.
#[derive(Event, Clone, Debug)]
pub struct MinecraftPacketReceiver<T: MinecraftPacket + Send + Sync + 'static> {
    /// 发送该包的实体 ID。
    ///
    /// The entity id that sent the packet.
    pub entity: EntityId,
    /// 收到该包的时间戳。
    ///
    /// The timestamp at which the packet was received.
    pub timestamp: u128,
    /// 收到的协议包本体。
    ///
    /// The received protocol packet itself.
    pub packet: T,
    /// 是否取消该事件的默认处理。
    ///
    /// Whether the default handling of this event is cancelled.
    pub cancelled: bool,
}

impl<T: MinecraftPacket + Send + Sync + 'static> MinecraftPacketReceiver<T> {
    /// 构造一个未取消的包接收事件。
    ///
    /// Constructs a non-cancelled packet reception event.
    ///
    /// - `entity`: 发送该包的实体 ID
    /// - `timestamp`: 收到该包的时间戳
    /// - `packet`: 收到的协议包本体
    pub fn new(entity: EntityId, timestamp: u128, packet: T) -> Self {
        Self {
            entity,
            timestamp,
            packet,
            cancelled: false,
        }
    }
}
