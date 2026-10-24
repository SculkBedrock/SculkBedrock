//! `ManualEventReader<E>`：手动维护读取游标的事件读取器。
//!
//! 读取器记录自己已读到的最后事件序号（`last_event_count`），从而在多次
//! 执行之间只读到新产生的事件；落后太多时可通过 `missed_events` 得知漏掉了
//! 多少事件。
//!
//! `ManualEventReader<E>`: an event reader with a manually maintained read
//! cursor.
//!
//! The reader records the last event sequence number it has seen
//! (`last_event_count`), so it only reads newly produced events across runs;
//! when it falls too far behind, `missed_events` reports how many events were
//! dropped.
use crate::event::iterator::{
    EventIterator, EventIteratorMut, EventIteratorMutWithId, EventIteratorWithId,
};
use crate::event::{Event, Events};
use std::marker::PhantomData;

/// 手动事件读取器：保存每个系统自己的事件读取游标。
///
/// Manual event reader: holds the per-system event read cursor.
#[derive(Debug)]
pub struct ManualEventReader<E: Event> {
    /// 已读到的最后事件序号。
    ///
    /// The sequence number of the last event that was read.
    pub last_event_count: usize,
    /// 事件类型占位。
    ///
    /// Phantom data for the event type.
    _marker: PhantomData<E>,
}

impl<E: Event> Default for ManualEventReader<E> {
    /// 创建一个游标位于起始位置的读取器。
    ///
    /// Creates a reader whose cursor sits at the start.
    fn default() -> Self {
        ManualEventReader {
            last_event_count: 0,
            _marker: Default::default(),
        }
    }
}

impl<E: Event> Clone for ManualEventReader<E> {
    /// 复制读取器（游标一并复制）。
    ///
    /// Clones the reader (including its cursor).
    fn clone(&self) -> Self {
        ManualEventReader {
            last_event_count: self.last_event_count,
            _marker: PhantomData,
        }
    }
}

#[allow(clippy::len_without_is_empty)] // Check fails since the is_empty implementation has a signature other than `(&self) -> bool`
impl<E: Event> ManualEventReader<E> {
    /// 构造只读事件迭代器。
    ///
    /// Builds a read-only event iterator.
    ///
    /// - `events`: 事件流
    pub fn read<'a>(&'a mut self, events: &'a Events<E>) -> EventIterator<'a, E> {
        self.read_with_id(events).without_id()
    }

    /// 构造带事件 id 的只读事件迭代器。
    ///
    /// Builds a read-only event iterator that also yields event ids.
    ///
    /// - `events`: 事件流
    pub fn read_with_id<'a>(&'a mut self, events: &'a Events<E>) -> EventIteratorWithId<'a, E> {
        EventIteratorWithId::new(self, events)
    }

    /// 异步构造带事件 id 的只读事件迭代器。
    ///
    /// 构建异步可用的带 id 事件迭代器；当前实现与同步版本等价。
    ///
    /// Asynchronously builds a read-only event iterator that also yields event
    /// ids; currently equivalent to the synchronous version.
    ///
    /// - `events`: 事件流
    pub async fn read_async_with_id<'a>(
        &'a mut self,
        events: &'a Events<E>,
    ) -> EventIteratorWithId<'a, E> {
        EventIteratorWithId::new(self, events)
    }

    /// 构造可变事件迭代器。
    ///
    /// 可变读取需要独占访问 `Events<E>`（通过 `ResMut` 写守卫）。
    ///
    /// Mutable reads need exclusive access to `Events<E>` (via the `ResMut` write guard).
    ///
    /// - `events`: 事件流
    pub fn read_mut<'a>(&'a mut self, events: &'a mut Events<E>) -> EventIteratorMut<'a, E> {
        self.read_mut_with_id(events).without_id()
    }

    /// 构造带事件 id 的可变事件迭代器。
    ///
    /// Builds a mutable event iterator that also yields event ids.
    ///
    /// - `events`: 事件流
    pub fn read_mut_with_id<'a>(
        &'a mut self,
        events: &'a mut Events<E>,
    ) -> EventIteratorMutWithId<'a, E> {
        EventIteratorMutWithId::new(self, events)
    }

    /// 当前可读的事件数量：最新事件序号与已读序号之差，且不超过事件流
    /// 当前保留的事件数（其余已被丢弃）。
    ///
    /// The number of currently readable events: the difference between the
    /// latest event sequence number and the last read one, capped by the
    /// events the stream still retains (the rest were already dropped).
    ///
    /// - `events`: 事件流
    pub fn len(&self, events: &Events<E>) -> usize {
        events
            .event_count
            .saturating_sub(self.last_event_count)
            .min(events.len())
    }

    /// 漏掉的事件数量。
    ///
    /// Amount of events we missed.
    ///
    /// - `events`: 事件流
    pub fn missed_events(&self, events: &Events<E>) -> usize {
        events
            .oldest_event_count()
            .saturating_sub(self.last_event_count)
    }

    /// 当前是否没有可读事件。
    ///
    /// Whether there is currently no readable event.
    ///
    /// - `events`: 事件流
    pub fn is_empty(&self, events: &Events<E>) -> bool {
        self.len(events) == 0
    }

    /// 丢弃当前可读的全部事件（把游标推进到事件流末尾）。
    ///
    /// Discards every currently readable event (advancing the cursor to the
    /// end of the stream).
    ///
    /// - `events`: 事件流
    pub fn clear(&mut self, events: &Events<E>) {
        self.last_event_count = events.event_count;
    }
}
