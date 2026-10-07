use crate::protocol::MinecraftPacket;
use sc_ecs::entity::EntityId;
use sc_ecs::event::Event;

#[derive(Event, Clone, Debug)]
pub struct MinecraftPacketReceiver<T: MinecraftPacket + Send + Sync + 'static> {
    pub entity: EntityId,
    pub timestamp: u128,
    pub packet: T,
    pub cancelled: bool,
}

impl<T: MinecraftPacket + Send + Sync + 'static> MinecraftPacketReceiver<T> {
    pub fn new(entity: EntityId, timestamp: u128, packet: T) -> Self {
        Self {
            entity,
            timestamp,
            packet,
            cancelled: false,
        }
    }
}
