use crate::player_connection::{PlayerConnection, PlayerConnectionError};
use crate::protocol::server::entity::UpdateAttributes;
use sc_entity::MinecraftEntityId;
use sc_packloader::definitions::attribute::EntityAttributes;
use sc_raknet::connection::RecvError;
use sc_utils::game::client::MinecraftClient;

pub trait MinecraftClientNetwork {
    async fn sync_attributes(&self) -> Result<(), PlayerConnectionError>;
    async fn disconnect(
        &self,
        reason: &str,
        hide_disconnect_screen: bool,
    ) -> Result<(), PlayerConnectionError>;
}

impl MinecraftClientNetwork for MinecraftClient {
    async fn sync_attributes(&self) -> Result<(), PlayerConnectionError> {
        let attributes = self
            .world
            .get_component::<EntityAttributes>(&self.entity_id)
            .map(|attributes| (*attributes).clone())
            .ok_or(PlayerConnectionError::RecvError(RecvError::Closed))?;
        // Debug: log the attributes being sent (min/max/current/default).
        for (name, attr) in attributes.iter() {
            let a = attr.read();
            log::debug!(
                "[attributes] {name}: min={} max={} current={} dmin={} dmax={} dvalue={}",
                a.min_value,
                a.max_value,
                a.current_value,
                a.default_min_value,
                a.default_max_value,
                a.default_value,
            );
        }
        let entity_id = *self
            .world
            .get_component::<MinecraftEntityId>(&self.entity_id)
            .ok_or(PlayerConnectionError::RecvError(RecvError::Closed))?;
        let connection = self
            .world
            .get_component::<PlayerConnection>(&self.entity_id)
            .ok_or(PlayerConnectionError::RecvError(RecvError::Closed))?;
        connection
            .send_packet(
                UpdateAttributes {
                    entity_id,
                    attributes,
                    frame: 0,
                },
                true,
            )
            .await
    }

    async fn disconnect(
        &self,
        reason: &str,
        hide_disconnect_screen: bool,
    ) -> Result<(), PlayerConnectionError> {
        let connection = self
            .world
            .get_component::<PlayerConnection>(&self.entity_id)
            .ok_or(PlayerConnectionError::RecvError(RecvError::Closed))?;
        connection.disconnect(reason, hide_disconnect_screen).await
    }
}
