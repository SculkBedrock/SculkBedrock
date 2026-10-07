use sc_ecs::entity::EntityId;
use sc_ecs::event::Event;
use sc_raknet::connection::Connection;

#[derive(Clone, Event)]
pub struct AcceptConnection {
    pub connection: Option<Connection>,
}

#[derive(Clone, Event)]
pub struct DropConnection {
    pub entity: EntityId,
}
