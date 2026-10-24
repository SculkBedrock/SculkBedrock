//! `EventReader`/`EventReaderMut` 参数：读取事件流。
//!
//! 两个参数都以 `Local<ManualEventReader<E>>` 记录每个系统的读取游标，
//! 因此同一系统每次执行只会读到自上次执行以来新产生的事件。
//!
//! The `EventReader`/`EventReaderMut` parameters: reading event streams.
//!
//! Both parameters keep a `Local<ManualEventReader<E>>` that records each
//! system's read cursor, so a system only sees the events produced since its
//! previous run.
use crate::event::iterator::{
    EventIterator, EventIteratorMut, EventIteratorMutWithId, EventIteratorWithId,
};
use crate::event::reader::ManualEventReader;
use crate::event::{Event, Events};
use crate::params::local::Local;
use crate::params::resource::{Res, ResMut};
use crate::system::{SystemParam, SystemParamState};
use crate::world::World;

/// 只读事件访问。持有 `Events<E>` 的读守卫直到系统结束；在同一个系统内
/// **不要**对同一事件类型 `E` 调用 `world.send_event`（对同一资源同时读写会
/// 死锁）。发送其他事件类型不受影响。
///
/// Read-only event access. Holds the `Events<E>` read guard until the system
/// ends; within the same system **do not** call `world.send_event` for the
/// same type `E` (read + write on one resource deadlocks). Sending other event
/// types is unaffected.
pub struct EventReader<E: Event + Send + Sync + 'static> {
    /// 每个系统独享的读取游标。
    ///
    /// The per-system read cursor.
    reader: Local<ManualEventReader<E>>,
    /// 事件流的只读守卫。
    ///
    /// The read guard of the event stream.
    events: Res<Events<E>>,
}

impl<E: Event + Send + Sync> EventReader<E> {
    /// 迭代自上次读取以来新产生的事件。
    ///
    /// Iterates the events produced since the previous read.
    pub fn read(&mut self) -> EventIterator<'_, E> {
        let Self { reader, events } = self;
        reader.read(events)
    }

    /// 与 `read` 相同，但同时给出事件 id。
    ///
    /// Same as `read`, but also yields the event id.
    pub fn read_with_id(&mut self) -> EventIteratorWithId<'_, E> {
        let Self { reader, events } = self;
        reader.read_with_id(events)
    }

    /// 当前可读的事件数量。
    ///
    /// The number of currently readable events.
    pub fn len(&self) -> usize {
        self.reader.len(&self.events)
    }

    /// 当前是否没有可读事件。
    ///
    /// Whether there is currently no readable event.
    pub fn is_empty(&self) -> bool {
        self.reader.is_empty(&self.events)
    }

    /// 丢弃当前可读的全部事件。
    ///
    /// Discards every currently readable event.
    pub fn clear(&mut self) {
        self.reader.clear(&self.events);
    }
}

/// `EventReader<E>` 的参数状态：保存读取游标的本地状态。
///
/// The parameter state of `EventReader<E>`: holds the local state of the read
/// cursor.
pub struct EventReaderState<E: Event + Send + Sync + 'static> {
    /// 读取游标的本地状态。
    ///
    /// The local state of the read cursor.
    local: <Local<ManualEventReader<E>> as SystemParam>::State,
}

impl<E: Event + Send + Sync + 'static> SystemParamState for EventReaderState<E> {
    type Item = EventReader<E>;

    /// 初始化参数状态。
    ///
    /// Initializes the parameter state.
    fn init() -> Self {
        Self {
            local: <Local<ManualEventReader<E>> as SystemParam>::State::init(),
        }
    }

    /// 提取本次执行的事件读取器；事件流资源缺失时返回 `None`。
    ///
    /// Extracts the event reader for this run; returns `None` when the event
    /// stream resource is absent.
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

/// `EventReader<E>` 可作为系统参数注入。
///
/// `EventReader<E>` can be injected as a system parameter.
unsafe impl<E: Event + Send + Sync + 'static> SystemParam for EventReader<E> {
    type This<'a> = EventReader<E>;
    type State = EventReaderState<E>;
}

/// 可变事件访问：持有 `Events<E>` 的**写**守卫直到系统结束。
///
/// Mutable event access: holds the `Events<E>` **write** guard until the system ends.
pub struct EventReaderMut<E: Event + Send + Sync + 'static> {
    /// 每个系统独享的读取游标。
    ///
    /// The per-system read cursor.
    reader: Local<ManualEventReader<E>>,
    /// 事件流的写守卫。
    ///
    /// The write guard of the event stream.
    events: ResMut<Events<E>>,
}

impl<E: Event + Send + Sync> EventReaderMut<E> {
    /// 迭代自上次读取以来新产生的事件。
    ///
    /// Iterates the events produced since the previous read.
    pub fn read(&mut self) -> EventIteratorMut<'_, E> {
        let Self { reader, events } = self;
        reader.read_mut(events)
    }

    /// 与 `read` 相同，但同时给出事件 id。
    ///
    /// Same as `read`, but also yields the event id.
    pub fn read_with_id(&mut self) -> EventIteratorMutWithId<'_, E> {
        let Self { reader, events } = self;
        reader.read_mut_with_id(events)
    }

    /// 当前可读的事件数量。
    ///
    /// The number of currently readable events.
    pub fn len(&self) -> usize {
        self.reader.len(&self.events)
    }

    /// 当前是否没有可读事件。
    ///
    /// Whether there is currently no readable event.
    pub fn is_empty(&self) -> bool {
        self.reader.is_empty(&self.events)
    }

    /// 丢弃当前可读的全部事件。
    ///
    /// Discards every currently readable event.
    pub fn clear(&mut self) {
        self.reader.clear(&self.events);
    }
}

/// `EventReaderMut<E>` 的参数状态：保存读取游标的本地状态。
///
/// The parameter state of `EventReaderMut<E>`: holds the local state of the
/// read cursor.
pub struct EventReaderMutState<E: Event + Send + Sync + 'static> {
    /// 读取游标的本地状态。
    ///
    /// The local state of the read cursor.
    local: <Local<ManualEventReader<E>> as SystemParam>::State,
}

impl<E: Event + Send + Sync + 'static> SystemParamState for EventReaderMutState<E> {
    type Item = EventReaderMut<E>;

    /// 初始化参数状态。
    ///
    /// Initializes the parameter state.
    fn init() -> Self {
        Self {
            local: <Local<ManualEventReader<E>> as SystemParam>::State::init(),
        }
    }

    /// 提取本次执行的可变事件读取器；事件流资源缺失时返回 `None`。
    ///
    /// Extracts the mutable event reader for this run; returns `None` when the
    /// event stream resource is absent.
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

/// `EventReaderMut<E>` 可作为系统参数注入。
///
/// `EventReaderMut<E>` can be injected as a system parameter.
unsafe impl<E: Event + Send + Sync + 'static> SystemParam for EventReaderMut<E> {
    type This<'a> = EventReaderMut<E>;
    type State = EventReaderMutState<E>;
}
