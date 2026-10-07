use crate::events::SCEventTrait;
use crate::recv::SCEvent;
use sc_ecs::entity::EntityId;
use sc_ecs::world::World;

pub mod events;
pub mod iterator;
pub mod param;
pub mod reader;
pub mod recv;

pub trait SCSendEvent {
    fn send_sc_event<E: SCEventTrait + Send + Sync + 'static>(&self, entity_id: EntityId, event: E);
}

impl SCSendEvent for World {
    fn send_sc_event<E: SCEventTrait + Send + Sync + 'static>(
        &self,
        entity_id: EntityId,
        event: E,
    ) {
        self.send_event(SCEvent::new_timestamp(entity_id, event));
    }
}
