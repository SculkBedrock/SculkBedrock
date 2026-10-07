use crate::events::SCEventTrait;
use crate::iterator::{
    SCEventIterator, SCEventIteratorMut, SCEventIteratorMutWithId, SCEventIteratorWithId,
};
use crate::reader::SCManualEventReader;
use crate::recv::SCEvent;
use sc_ecs::event::Events;
use sc_ecs::params::local::Local;
use sc_ecs::params::resource::{Res, ResMut};
use sc_ecs::system::{SystemParam, SystemParamState};
use sc_ecs::world::World;

pub struct SCEventReader<E: SCEventTrait + Send + Sync + 'static> {
    reader: Local<SCManualEventReader<E>>,
    events: Res<Events<SCEvent<E>>>,
}

impl<E: SCEventTrait + Send + Sync> SCEventReader<E> {
    pub fn read(&mut self) -> SCEventIterator<'_, E> {
        let Self { reader, events } = self;
        reader.read(events)
    }

    pub fn read_with_id(&mut self) -> SCEventIteratorWithId<'_, E> {
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

pub struct SCEventReaderState<E: SCEventTrait + Send + Sync + 'static> {
    local: <Local<SCManualEventReader<E>> as SystemParam>::State,
}

impl<E: SCEventTrait + Send + Sync + 'static> SystemParamState for SCEventReaderState<E> {
    type Item = SCEventReader<E>;

    fn init() -> Self {
        Self {
            local: <Local<SCManualEventReader<E>> as SystemParam>::State::init(),
        }
    }

    fn get_param<'a>(
        state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        let reader = <Local<SCManualEventReader<E>> as SystemParam>::State::get_param(
            &mut state.local,
            world,
        )?;
        let events = world.get_resource()?;
        Some(SCEventReader { reader, events })
    }
}

unsafe impl<E: SCEventTrait + Send + Sync + 'static> SystemParam for SCEventReader<E> {
    type This<'a> = SCEventReader<E>;
    type State = SCEventReaderState<E>;
}

/// Mutable event read: holds the `Events` write guard until the system ends.
pub struct SCEventReaderMut<E: SCEventTrait + Send + Sync + 'static> {
    reader: Local<SCManualEventReader<E>>,
    events: ResMut<Events<SCEvent<E>>>,
}

impl<E: SCEventTrait + Send + Sync> SCEventReaderMut<E> {
    pub fn read(&mut self) -> SCEventIteratorMut<'_, E> {
        let Self { reader, events } = self;
        reader.read_mut(events)
    }

    pub fn read_with_id(&mut self) -> SCEventIteratorMutWithId<'_, E> {
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

pub struct SCEventReaderMutState<E: SCEventTrait + Send + Sync + 'static> {
    local: <Local<SCManualEventReader<E>> as SystemParam>::State,
}

impl<E: SCEventTrait + Send + Sync + 'static> SystemParamState for SCEventReaderMutState<E> {
    type Item = SCEventReaderMut<E>;

    fn init() -> Self {
        Self {
            local: <Local<SCManualEventReader<E>> as SystemParam>::State::init(),
        }
    }

    fn get_param<'a>(
        state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        let reader = <Local<SCManualEventReader<E>> as SystemParam>::State::get_param(
            &mut state.local,
            world,
        )?;
        let events = world.get_resource_mut()?;
        Some(SCEventReaderMut { reader, events })
    }
}

unsafe impl<E: SCEventTrait + Send + Sync + 'static> SystemParam for SCEventReaderMut<E> {
    type This<'a> = SCEventReaderMut<E>;
    type State = SCEventReaderMutState<E>;
}
