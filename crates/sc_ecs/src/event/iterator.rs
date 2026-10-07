use crate::event::reader::ManualEventReader;
use crate::event::{Event, EventId, EventInstance, Events};
use std::iter::Chain;
use std::slice::{Iter, IterMut};

#[derive(Debug)]
pub struct EventIterator<'a, E: Event> {
    pub(crate) iter: EventIteratorWithId<'a, E>,
}

impl<'a, E: Event> Iterator for EventIterator<'a, E> {
    type Item = &'a E;
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|(event, _)| event)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }

    fn count(self) -> usize {
        self.iter.count()
    }

    fn last(self) -> Option<Self::Item>
    where
        Self: Sized,
    {
        self.iter.last().map(|(event, _)| event)
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.iter.nth(n).map(|(event, _)| event)
    }
}

impl<'a, E: Event> ExactSizeIterator for EventIterator<'a, E> {
    fn len(&self) -> usize {
        self.iter.len()
    }
}

#[derive(Debug)]
pub struct EventIteratorWithId<'a, E: Event> {
    reader: &'a mut ManualEventReader<E>,
    chain: Chain<Iter<'a, EventInstance<E>>, Iter<'a, EventInstance<E>>>,
    pub(crate) unread: usize,
}

impl<'a, E: Event> EventIteratorWithId<'a, E> {
    /// Creates a new iterator that yields any `events` that have not yet been seen by `reader`.
    pub fn new(reader: &'a mut ManualEventReader<E>, events: &'a Events<E>) -> Self {
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
    pub fn without_id(self) -> EventIterator<'a, E> {
        EventIterator { iter: self }
    }
}

impl<'a, E: Event> Iterator for EventIteratorWithId<'a, E> {
    type Item = (&'a E, EventId);
    fn next(&mut self) -> Option<Self::Item> {
        match self
            .chain
            .next()
            .map(|instance| (instance.get_event(), instance.event_id))
        {
            Some(item) => {
                self.reader.last_event_count += 1;
                self.unread -= 1;
                Some(item)
            }
            None => None,
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.chain.size_hint()
    }

    fn count(self) -> usize {
        self.reader.last_event_count += self.unread;
        self.unread
    }

    fn last(self) -> Option<Self::Item>
    where
        Self: Sized,
    {
        let instance = self.chain.last()?;
        self.reader.last_event_count += self.unread;
        Some((instance.get_event(), instance.event_id))
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        if let Some(instance) = self.chain.nth(n) {
            self.reader.last_event_count += n + 1;
            self.unread -= n + 1;
            Some((instance.get_event(), instance.event_id))
        } else {
            self.reader.last_event_count += self.unread;
            self.unread = 0;
            None
        }
    }
}

impl<'a, E: Event> ExactSizeIterator for EventIteratorWithId<'a, E> {
    fn len(&self) -> usize {
        self.unread
    }
}

#[derive(Debug)]
pub struct EventIteratorMut<'a, E: Event> {
    iter: EventIteratorMutWithId<'a, E>,
}

impl<'a, E: Event> Iterator for EventIteratorMut<'a, E> {
    type Item = &'a mut E;
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|(event, _)| event)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a, E: Event> ExactSizeIterator for EventIteratorMut<'a, E> {
    fn len(&self) -> usize {
        self.iter.len()
    }
}

/// Mutable event iterator: requires exclusive access to `Events<E>` (`ResMut`/`&mut`);
/// mutual exclusion is provided by the resource write lock.
#[derive(Debug)]
pub struct EventIteratorMutWithId<'a, E: Event> {
    reader: &'a mut ManualEventReader<E>,
    chain: Chain<IterMut<'a, EventInstance<E>>, IterMut<'a, EventInstance<E>>>,
    unread: usize,
}

impl<'a, E: Event> EventIteratorMutWithId<'a, E> {
    pub fn new(reader: &'a mut ManualEventReader<E>, events: &'a mut Events<E>) -> Self {
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

    pub fn without_id(self) -> EventIteratorMut<'a, E> {
        EventIteratorMut { iter: self }
    }
}

impl<'a, E: Event> Iterator for EventIteratorMutWithId<'a, E> {
    type Item = (&'a mut E, EventId);

    fn next(&mut self) -> Option<Self::Item> {
        match self.chain.next() {
            Some(instance) => {
                self.reader.last_event_count += 1;
                self.unread -= 1;
                Some((&mut instance.event, instance.event_id))
            }
            None => None,
        }
    }
}

impl<'a, E: Event> ExactSizeIterator for EventIteratorMutWithId<'a, E> {
    fn len(&self) -> usize {
        self.unread
    }
}
