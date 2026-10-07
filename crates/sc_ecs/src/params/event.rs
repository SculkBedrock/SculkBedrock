use crate::event::iterator::{
    EventIterator, EventIteratorMut, EventIteratorMutWithId, EventIteratorWithId,
};
use crate::event::reader::ManualEventReader;
use crate::event::{Event, Events};
use crate::params::local::Local;
use crate::params::resource::{Res, ResMut};
use crate::system::{SystemParam, SystemParamState};
use crate::world::World;

/// Read-only event access. Holds the `Events<E>` read guard until the system
/// ends; within the same system **do not** call `world.send_event` for the
/// same type `E` (read + write on one resource deadlocks). Sending other event
/// types is unaffected.
pub struct EventReader<E: Event + Send + Sync + 'static> {
    reader: Local<ManualEventReader<E>>,
    events: Res<Events<E>>,
}

impl<E: Event + Send + Sync> EventReader<E> {
    pub fn read(&mut self) -> EventIterator<'_, E> {
        let Self { reader, events } = self;
        reader.read(events)
    }

    pub fn read_with_id(&mut self) -> EventIteratorWithId<'_, E> {
        let Self { reader, events } = self;
        reader.read_with_id(events)
    }

    pub fn len(&self) -> usize {
        self.reader.len(&self.events)
    }

    pub fn is_empty(&self) -> bool {
        self.reader.is_empty(&self.events)
    }

    pub fn clear(&mut self) {
        self.reader.clear(&self.events);
    }
}

pub struct EventReaderState<E: Event + Send + Sync + 'static> {
    local: <Local<ManualEventReader<E>> as SystemParam>::State,
}

impl<E: Event + Send + Sync + 'static> SystemParamState for EventReaderState<E> {
    type Item = EventReader<E>;

    fn init() -> Self {
        Self {
            local: <Local<ManualEventReader<E>> as SystemParam>::State::init(),
        }
    }

    fn get_param<'a>(
        state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        let reader = <Local<ManualEventReader<E>> as SystemParam>::State::get_param(
            &mut state.local,
            world,
        )?;
        let events = world.get_resource()?;
        Some(EventReader { reader, events })
    }
}

unsafe impl<E: Event + Send + Sync + 'static> SystemParam for EventReader<E> {
    type This<'a> = EventReader<E>;
    type State = EventReaderState<E>;
}

/// Mutable event access: holds the `Events<E>` **write** guard until the system ends.
pub struct EventReaderMut<E: Event + Send + Sync + 'static> {
    reader: Local<ManualEventReader<E>>,
    events: ResMut<Events<E>>,
}

impl<E: Event + Send + Sync> EventReaderMut<E> {
    pub fn read(&mut self) -> EventIteratorMut<'_, E> {
        let Self { reader, events } = self;
        reader.read_mut(events)
    }

    pub fn read_with_id(&mut self) -> EventIteratorMutWithId<'_, E> {
        let Self { reader, events } = self;
        reader.read_mut_with_id(events)
    }

    pub fn len(&self) -> usize {
        self.reader.len(&self.events)
    }

    pub fn is_empty(&self) -> bool {
        self.reader.is_empty(&self.events)
    }

    pub fn clear(&mut self) {
        self.reader.clear(&self.events);
    }
}

pub struct EventReaderMutState<E: Event + Send + Sync + 'static> {
    local: <Local<ManualEventReader<E>> as SystemParam>::State,
}

impl<E: Event + Send + Sync + 'static> SystemParamState for EventReaderMutState<E> {
    type Item = EventReaderMut<E>;

    fn init() -> Self {
        Self {
            local: <Local<ManualEventReader<E>> as SystemParam>::State::init(),
        }
    }

    fn get_param<'a>(
        state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        let reader = <Local<ManualEventReader<E>> as SystemParam>::State::get_param(
            &mut state.local,
            world,
        )?;
        let events = world.get_resource_mut()?;
        Some(EventReaderMut { reader, events })
    }
}

unsafe impl<E: Event + Send + Sync + 'static> SystemParam for EventReaderMut<E> {
    type This<'a> = EventReaderMut<E>;
    type State = EventReaderMutState<E>;
}
