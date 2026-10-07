use crate::events::SCEventTrait;
use crate::reader::SCManualEventReader;
use crate::recv::SCEvent;
use std::iter::Chain;
use std::slice::{Iter, IterMut};
use sc_ecs::event::{EventId, EventInstance, Events};

#[derive(Debug)]
pub struct SCEventIterator<'a, E: SCEventTrait + Send + Sync + 'static> {
    iter: SCEventIteratorWithId<'a, E>,
}

impl<'a, E: SCEventTrait + Send + Sync + 'static> Iterator for SCEventIterator<'a, E> {
    type Item = &'a SCEvent<E>;
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|(event, _)| event)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

#[derive(Debug)]
pub struct SCEventIteratorWithId<'a, E: SCEventTrait + Send + Sync + 'static> {
    reader: &'a mut SCManualEventReader<E>,
    chain: Chain<Iter<'a, EventInstance<SCEvent<E>>>, Iter<'a, EventInstance<SCEvent<E>>>>,
    unread: usize,
}

impl<'a, E: SCEventTrait + Send + Sync + 'static> SCEventIteratorWithId<'a, E> {
    pub fn new(reader: &'a mut SCManualEventReader<E>, events: &'a Events<SCEvent<E>>) -> Self {
        let old_index = reader
            .last_event_count
            .saturating_sub(events.old_events.start_event_count);
        let new_index = reader
            .last_event_count
            .saturating_sub(events.new_events.start_event_count);
        let old = events.old_events.get(old_index..).unwrap_or_default();
        let new = events.new_events.get(new_index..).unwrap_or_default();

        let unread_count = old.len() + new.len();
        reader.last_event_count = events.event_count - unread_count;
        // Iterate the oldest first, then the newer events
        let chain = old.iter().chain(new.iter());

        Self {
            reader,
            chain,
            unread: unread_count,
        }
    }

    /// Iterate over only the events.
    pub fn without_id(self) -> SCEventIterator<'a, E> {
        SCEventIterator { iter: self }
    }
}

impl<'a, E: SCEventTrait + Send + Sync + 'static> Iterator for SCEventIteratorWithId<'a, E> {
    type Item = (&'a SCEvent<E>, EventId);
    fn next(&mut self) -> Option<Self::Item> {
        // Cancelled events are skipped (continue) instead of ending iteration;
        // returning None on cancelled would swallow all later unread events.
        loop {
            let instance = self.chain.next()?;
            self.reader.last_event_count += 1;
            self.unread -= 1;
            if instance.get_event().get_cancelled() {
                continue;
            }
            return Some((instance.get_event(), instance.event_id));
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, self.chain.size_hint().1)
    }
}

#[derive(Debug)]
pub struct SCEventIteratorMut<'a, E: SCEventTrait + Send + Sync + 'static> {
    iter: SCEventIteratorMutWithId<'a, E>,
}

impl<'a, E: SCEventTrait + Send + Sync + 'static> Iterator for SCEventIteratorMut<'a, E> {
    type Item = &'a mut SCEvent<E>;
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|(event, _)| event)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

/// Mutable iteration: requires exclusive `Events` access (`ResMut` write guard);
/// cancellation (`set_cancelled`) applies directly via `&mut SCEvent<E>`.
#[derive(Debug)]
pub struct SCEventIteratorMutWithId<'a, E: SCEventTrait + Send + Sync + 'static> {
    reader: &'a mut SCManualEventReader<E>,
    chain: Chain<IterMut<'a, EventInstance<SCEvent<E>>>, IterMut<'a, EventInstance<SCEvent<E>>>>,
    unread: usize,
}

impl<'a, E: SCEventTrait + Send + Sync + 'static> SCEventIteratorMutWithId<'a, E> {
    pub fn new(reader: &'a mut SCManualEventReader<E>, events: &'a mut Events<SCEvent<E>>) -> Self {
        let old_index = reader
            .last_event_count
            .saturating_sub(events.old_events.start_event_count);
        let new_index = reader
            .last_event_count
            .saturating_sub(events.new_events.start_event_count);
        let event_count = events.event_count;

        let old_len = events.old_events.len();
        let new_len = events.new_events.len();
        let unread_count = old_len.saturating_sub(old_index) + new_len.saturating_sub(new_index);
        reader.last_event_count = event_count - unread_count;

        let (old_events, new_events) = (&mut events.old_events, &mut events.new_events);
        let old = old_events.get_mut(old_index..).unwrap_or_default();
        let new = new_events.get_mut(new_index..).unwrap_or_default();
        let chain = old.iter_mut().chain(new.iter_mut());

        Self {
            reader,
            chain,
            unread: unread_count,
        }
    }

    pub fn without_id(self) -> SCEventIteratorMut<'a, E> {
        SCEventIteratorMut { iter: self }
    }
}

impl<'a, E: SCEventTrait + Send + Sync + 'static> Iterator for SCEventIteratorMutWithId<'a, E> {
    type Item = (&'a mut SCEvent<E>, EventId);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let instance = self.chain.next()?;
            self.reader.last_event_count += 1;
            self.unread -= 1;
            if instance.get_event().get_cancelled() {
                continue;
            }
            let event_id = instance.event_id;
            return Some((instance.get_event_mut(), event_id));
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, self.chain.size_hint().1)
    }
}
