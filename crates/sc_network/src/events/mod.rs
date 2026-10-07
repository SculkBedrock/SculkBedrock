use crate::events::connection::*;
use crate::events::player::*;
use sc_ecs::event::EnumEvents;
use sc_eventbus::events::SCEnumEvents;

pub mod connection;
pub mod player;

#[derive(EnumEvents)]
pub enum SCNetworkEvents {
    //Connection
    AcceptConnection(AcceptConnection),
    DropConnection(DropConnection),

    //Player
    CreatePlayer(CreatePlayer),
}

#[derive(SCEnumEvents)]
pub enum SCGameEvents {
    PlayerSpawn,
    PlayerLogin,
}
