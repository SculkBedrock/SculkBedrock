//! 事件迭代器：按顺序产出事件负载（可选带事件 id）。
//!
//! 迭代器在构造时一次性定位「旧事件 + 新事件」的切片范围，并在推进时同步
//! 更新读取器的游标，因此丢弃未读完的迭代器会导致这些事件被跳过。
//!
//! Event iterators: yielding event payloads (optionally with event ids) in
//! order.
//!
//! An iterator locates the "old events + new events" slice range once at
//! construction and advances the reader's cursor as it yields, so dropping an
//! iterator before it is exhausted skips the remaining events.
use crate::event::reader::ManualEventReader;
use crate::event::{Event, EventId, EventInstance, Events};
use std::iter::Chain;
use std::slice::{Iter, IterMut};

/// 只读事件迭代器：只产出事件负载。
///
/// Read-only event iterator: yields only event payloads.
#[derive(Debug)]
pub struct EventIterator<'a, E: Event> {
    /// 内层带 id 的迭代器。
    ///
    /// The inner iterator that also yields ids.
    pub(crate) iter: EventIteratorWithId<'a, E>,
}

impl<'a, E: Event> Iterator for EventIterator<'a, E> {
    type Item = &'a E;

    /// 取出下一个事件负载。
    ///
    /// Takes the next event payload.
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|(event, _)| event)
    }

    /// 返回剩余元素数量的下界与上界。
    ///
    /// Returns the lower and upper bounds of the remaining element count.
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }

    /// 消耗迭代器并返回元素总数。
    ///
    /// Consumes the iterator and returns the total element count.
    fn count(self) -> usize {
        self.iter.count()
    }

    /// 消耗迭代器并返回最后一个元素。
    ///
    /// Consumes the iterator and returns the last element.
    fn last(self) -> Option<Self::Item>
    where
        Self: Sized,
    {
        self.iter.last().map(|(event, _)| event)
    }

    /// 跳过 `n` 个元素并返回第 `n` 个元素。
    ///
    /// Skips `n` elements and returns the n-th one.
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.iter.nth(n).map(|(event, _)| event)
    }
}

impl<'a, E: Event> ExactSizeIterator for EventIterator<'a, E> {
    /// 返回剩余元素数量。
    ///
    /// Returns the number of remaining elements.
    fn len(&self) -> usize {
        self.iter.len()
    }
}

/// 带事件 id 的只读事件迭代器。
///
/// Read-only event iterator that also yields event ids.
#[derive(Debug)]
pub struct EventIteratorWithId<'a, E: Event> {
    /// 读取器：迭代时同步推进其游标。
    ///
    /// The reader whose cursor advances as the iterator yields.
    reader: &'a mut ManualEventReader<E>,
    /// 旧事件与新事件的串联迭代器。
    ///
    /// The chained iterator over the old and new events.
    chain: Chain<Iter<'a, EventInstance<E>>, Iter<'a, EventInstance<E>>>,
    /// 尚未读出的事件数量。
    ///
    /// The number of events not yet yielded.
    pub(crate) unread: usize,
}

impl<'a, E: Event> EventIteratorWithId<'a, E> {
    /// 创建一个迭代器，产出 `reader` 尚未见过的全部事件。
    ///
    /// Creates a new iterator that yields any `events` that have not yet been seen by `reader`.
    ///
    /// - `reader`: 读取器，其游标会随迭代推进
    /// - `events`: 事件流
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

    /// 转换为只产出事件负载的迭代器。
    ///
    /// Iterate over only the events.
    pub fn without_id(self) -> EventIterator<'a, E> {
        EventIterator { iter: self }
    }
}

impl<'a, E: Event> Iterator for EventIteratorWithId<'a, E> {
    type Item = (&'a E, EventId);

    /// 取出下一个（事件负载, 事件 id）。
    ///
    /// Takes the next (event payload, event id).
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

    /// 返回剩余元素数量的下界与上界。
    ///
    /// Returns the lower and upper bounds of the remaining element count.
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.chain.size_hint()
    }

    /// 消耗迭代器并返回元素总数。
    ///
    /// Consumes the iterator and returns the total element count.
    fn count(self) -> usize {
        self.reader.last_event_count += self.unread;
        self.unread
    }

    /// 消耗迭代器并返回最后一个元素。
    ///
    /// Consumes the iterator and returns the last element.
    fn last(self) -> Option<Self::Item>
    where
        Self: Sized,
    {
        let instance = self.chain.last()?;
        self.reader.last_event_count += self.unread;
        Some((instance.get_event(), instance.event_id))
    }

    /// 跳过 `n` 个元素并返回第 `n` 个元素。
    ///
    /// Skips `n` elements and returns the n-th one.
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
    /// 返回剩余元素数量。
    ///
    /// Returns the number of remaining elements.
    fn len(&self) -> usize {
        self.unread
    }
}

/// 可变事件迭代器：只产出事件负载。
///
/// Mutable event iterator: yields only event payloads.
#[derive(Debug)]
pub struct EventIteratorMut<'a, E: Event> {
    /// 内层带 id 的迭代器。
    ///
    /// The inner iterator that also yields ids.
    iter: EventIteratorMutWithId<'a, E>,
}

impl<'a, E: Event> Iterator for EventIteratorMut<'a, E> {
    type Item = &'a mut E;

    /// 取出下一个事件负载。
    ///
    /// Takes the next event payload.
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|(event, _)| event)
    }

    /// 返回剩余元素数量的下界与上界。
    ///
    /// Returns the lower and upper bounds of the remaining element count.
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a, E: Event> ExactSizeIterator for EventIteratorMut<'a, E> {
    /// 返回剩余元素数量。
    ///
    /// Returns the number of remaining elements.
    fn len(&self) -> usize {
        self.iter.len()
    }
}

/// 可变事件迭代器：需要独占访问 `Events<E>`（`ResMut`/`&mut`）；互斥由资源
/// 写锁保证。
///
/// Mutable event iterator: requires exclusive access to `Events<E>` (`ResMut`/`&mut`);
/// mutual exclusion is provided by the resource write lock.
#[derive(Debug)]
pub struct EventIteratorMutWithId<'a, E: Event> {
    /// 读取器：迭代时同步推进其游标。
    ///
    /// The reader whose cursor advances as the iterator yields.
    reader: &'a mut ManualEventReader<E>,
    /// 旧事件与新事件的串联可变迭代器。
    ///
    /// The chained mutable iterator over the old and new events.
    chain: Chain<IterMut<'a, EventInstance<E>>, IterMut<'a, EventInstance<E>>>,
    /// 尚未读出的事件数量。
    ///
    /// The number of events not yet yielded.
    unread: usize,
}

impl<'a, E: Event> EventIteratorMutWithId<'a, E> {
    /// 创建一个迭代器，产出 `reader` 尚未见过的全部事件（可修改负载）。
    ///
    /// Creates an iterator yielding every event `reader` has not yet seen (the
    /// payloads can be modified in place).
    ///
    /// - `reader`: 读取器，其游标会随迭代推进
    /// - `events`: 事件流（独占借用）
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

    /// 转换为只产出事件负载的迭代器。
    ///
    /// Converts into an iterator yielding only event payloads.
    pub fn without_id(self) -> EventIteratorMut<'a, E> {
        EventIteratorMut { iter: self }
    }
}

impl<'a, E: Event> Iterator for EventIteratorMutWithId<'a, E> {
    type Item = (&'a mut E, EventId);

    /// 取出下一个（事件负载, 事件 id）。
    ///
    /// Takes the next (event payload, event id).
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
    /// 返回剩余元素数量。
    ///
    /// Returns the number of remaining elements.
    fn len(&self) -> usize {
        self.unread
    }
}
