use sc_ecs::entity::EntityId;
use sc_ecs::event::Event;
use sc_eventbus::events::SCCancellableEvent;

#[derive(Clone, Event)]
pub struct CreatePlayer {
    pub entity: EntityId,
}

#[derive(Clone, SCCancellableEvent)]
pub struct PlayerSpawn;

#[derive(Clone, SCCancellableEvent)]
pub struct PlayerLogin;
