use crate::events::SCEventTrait;
use crate::iterator::{
    SCEventIterator, SCEventIteratorMut, SCEventIteratorMutWithId, SCEventIteratorWithId,
};
use crate::recv::SCEvent;
use std::marker::PhantomData;
use sc_ecs::event::Events;

#[derive(Debug)]
pub struct SCManualEventReader<E: SCEventTrait + Send + Sync + 'static> {
    pub last_event_count: usize,
    _marker: PhantomData<E>,
}

impl<E: SCEventTrait + Send + Sync + 'static> Default for SCManualEventReader<E> {
    fn default() -> Self {
        SCManualEventReader {
            last_event_count: 0,
            _marker: Default::default(),
        }
    }
}

impl<E: SCEventTrait + Send + Sync + 'static> Clone for SCManualEventReader<E> {
    fn clone(&self) -> Self {
        SCManualEventReader {
            last_event_count: self.last_event_count,
            _marker: PhantomData,
        }
    }
}

#[allow(clippy::len_without_is_empty)] // Check fails since the is_empty implementation has a signature other than `(&self) -> bool`
impl<E: SCEventTrait + Send + Sync + 'static> SCManualEventReader<E> {
    /// See [`EventReader::read`]
    pub fn read<'a>(&'a mut self, events: &'a Events<SCEvent<E>>) -> SCEventIterator<'a, E> {
        self.read_with_id(events).without_id()
    }

    /// See [`EventReader::read_with_id`]
    pub fn read_with_id<'a>(
        &'a mut self,
        events: &'a Events<SCEvent<E>>,
    ) -> SCEventIteratorWithId<'a, E> {
        SCEventIteratorWithId::new(self, events)
    }

    /// Mutable read requires exclusive `Events` access (via `ResMut` write guard).
    pub fn read_mut<'a>(
        &'a mut self,
        events: &'a mut Events<SCEvent<E>>,
    ) -> SCEventIteratorMut<'a, E> {
        self.read_mut_with_id(events).without_id()
    }

    /// See [`EventReader::read_mut_with_id`]
    pub fn read_mut_with_id<'a>(
        &'a mut self,
        events: &'a mut Events<SCEvent<E>>,
    ) -> SCEventIteratorMutWithId<'a, E> {
        SCEventIteratorMutWithId::new(self, events)
    }

    /// See [`EventReader::len`]
    pub fn len(&self, events: &Events<SCEvent<E>>) -> usize {
        // The number of events in this reader is the difference between the most recent event
        // and the last event seen by it. This will be at most the number of events contained
        // with the events (any others have already been dropped)
        // TODO: Warn when there are dropped events, or return e.g. a `Result<usize, (usize, usize)>`
        events
            .event_count
            .saturating_sub(self.last_event_count)
            .min(events.len())
    }

    /// Amount of events we missed.
    pub fn missed_events(&self, events: &Events<SCEvent<E>>) -> usize {
        events
            .oldest_event_count()
            .saturating_sub(self.last_event_count)
    }

    /// See [`EventReader::is_empty()`]
    pub fn is_empty(&self, events: &Events<SCEvent<E>>) -> bool {
        self.len(events) == 0
    }

    /// See [`EventReader::clear()`]
    pub fn clear(&mut self, events: &Events<SCEvent<E>>) {
        self.last_event_count = events.event_count;
    }
}
