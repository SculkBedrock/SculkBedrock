use crate::event::iterator::{
    EventIterator, EventIteratorMut, EventIteratorMutWithId, EventIteratorWithId,
};
use crate::event::{Event, Events};
use std::marker::PhantomData;

#[derive(Debug)]
pub struct ManualEventReader<E: Event> {
    pub last_event_count: usize,
    _marker: PhantomData<E>,
}

impl<E: Event> Default for ManualEventReader<E> {
    fn default() -> Self {
        ManualEventReader {
            last_event_count: 0,
            _marker: Default::default(),
        }
    }
}

impl<E: Event> Clone for ManualEventReader<E> {
    fn clone(&self) -> Self {
        ManualEventReader {
            last_event_count: self.last_event_count,
            _marker: PhantomData,
        }
    }
}

#[allow(clippy::len_without_is_empty)] // Check fails since the is_empty implementation has a signature other than `(&self) -> bool`
impl<E: Event> ManualEventReader<E> {
    /// See [`EventReader::read`]
    pub fn read<'a>(&'a mut self, events: &'a Events<E>) -> EventIterator<'a, E> {
        self.read_with_id(events).without_id()
    }

    /// See [`EventReader::read_with_id`]
    pub fn read_with_id<'a>(&'a mut self, events: &'a Events<E>) -> EventIteratorWithId<'a, E> {
        EventIteratorWithId::new(self, events)
    }

    pub async fn read_async_with_id<'a>(
        &'a mut self,
        events: &'a Events<E>,
    ) -> EventIteratorWithId<'a, E> {
        EventIteratorWithId::new(self, events)
    }

    /// Mutable reads need exclusive access to `Events<E>` (via the `ResMut` write guard).
    pub fn read_mut<'a>(&'a mut self, events: &'a mut Events<E>) -> EventIteratorMut<'a, E> {
        self.read_mut_with_id(events).without_id()
    }

    /// See [`EventReader::read_mut_with_id`]
    pub fn read_mut_with_id<'a>(
        &'a mut self,
        events: &'a mut Events<E>,
    ) -> EventIteratorMutWithId<'a, E> {
        EventIteratorMutWithId::new(self, events)
    }

    /// See [`EventReader::len`]
    pub fn len(&self, events: &Events<E>) -> usize {
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
    pub fn missed_events(&self, events: &Events<E>) -> usize {
        events
            .oldest_event_count()
            .saturating_sub(self.last_event_count)
    }

    /// See [`EventReader::is_empty()`]
    pub fn is_empty(&self, events: &Events<E>) -> bool {
        self.len(events) == 0
    }

    /// See [`EventReader::clear()`]
    pub fn clear(&mut self, events: &Events<E>) {
        self.last_event_count = events.event_count;
    }
}
