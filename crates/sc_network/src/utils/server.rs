use crate::player_connection::PlayerConnection;
use crate::protocol::MinecraftPacket;
use async_trait::async_trait;
use sc_ecs::entity::EntityId;
use sc_ecs::world::World;
use sc_utils::game::structs::server::Server;

#[async_trait]
pub trait SCNetworkServer {
    async fn broadcast_packet<P: MinecraftPacket + Clone + Send + Sync + 'static>(
        &self,
        clients: Vec<EntityId>,
        packet: P,
        immediate: bool,
    );
}

#[async_trait]
impl SCNetworkServer for Server {
    async fn broadcast_packet<P: MinecraftPacket + Clone + Send + Sync + 'static>(
        &self,
        clients: Vec<EntityId>,
        packet: P,
        immediate: bool,
    ) {
        for client in clients {
            if let Some(connection) = self.world.get_component::<PlayerConnection>(&client) {
                let _ = connection.send_packet(packet.clone(), immediate).await;
            }
        }
    }
}

pub async fn broadcast_packet_from_world<P: MinecraftPacket + Clone + Send + Sync + 'static>(
    world: World,
    clients: Vec<EntityId>,
    packet: P,
    immediate: bool,
) {
    for client in clients {
        if let Some(connection) = world.get_component::<PlayerConnection>(&client) {
            let _ = connection.send_packet(packet.clone(), immediate).await;
        }
    }
}
