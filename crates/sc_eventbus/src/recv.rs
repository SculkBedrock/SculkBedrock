use crate::events::SCEventTrait;
use std::time::{SystemTime, UNIX_EPOCH};
use sc_ecs::entity::EntityId;
use sc_ecs::event::Event;

#[derive(Event, Debug)]
pub struct SCEvent<T: SCEventTrait + Send + Sync + 'static> {
    pub client: EntityId,
    pub timestamp: u128,
    pub event: T,
    cancelled: bool,
}

impl<T: SCEventTrait + Send + Sync + 'static> SCEvent<T> {
    pub fn new(client: EntityId, timestamp: u128, event: T) -> Self {
        SCEvent {
            client,
            timestamp,
            event,
            cancelled: false,
        }
    }

    pub fn new_timestamp(client: EntityId, event: T) -> Self {
        SCEvent {
            client,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            event,
            cancelled: false,
        }
    }

    pub fn get_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn set_cancelled(&mut self, cancelled: bool) -> Option<()> {
        if T::cancellable() {
            self.cancelled = cancelled;
            Some(())
        } else {
            None
        }
    }
}

impl<T: SCEventTrait + Send + Sync + 'static + Clone> Clone for SCEvent<T> {
    fn clone(&self) -> Self {
        SCEvent {
            client: self.client,
            timestamp: self.timestamp,
            event: self.event.clone(),
            cancelled: self.cancelled,
        }
    }
}
