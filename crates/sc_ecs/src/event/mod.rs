//! 事件系统：`Event` trait、事件流 `Events<E>` 与事件更新器。
//!
//! 事件流采用双缓冲：每次 `update()` 把 `new_events` 换入 `old_events` 并清空
//! `new_events`，因此落后一个 tick 的读取者仍能读到上一个 tick 的事件，更早的事件被丢弃。
//!
//! The event system: the `Event` trait, the event stream `Events<E>` and the
//! event updaters.
//!
//! Event streams are double-buffered: each `update()` swaps `new_events` into
//! `old_events` and clears `new_events`, so a reader lagging one tick can still
//! read the previous round's events while older ones are dropped.

use crate::app::App;
use crate::world::World;
use sc_ecs_macros::Resource;
pub use sc_ecs_macros::{EnumEvents, Event};

pub mod iterator;
pub mod priority;
pub mod reader;

/// 事件枚举 trait：由 derive 宏为事件枚举生成发送与注册能力。
///
/// Event enum trait: derive-generated sending and registration for event enums.
pub trait EnumEvents {
    /// 向世界中发送该枚举的一个变体，返回事件 id。
    ///
    /// Sends one variant of this enum into the world and returns its event id.
    ///
    /// - `self`: 要发送的枚举变体
    /// - `world`: 目标世界
    fn send_event(self, world: &World) -> Option<EventId>;

    /// 为应用注册该枚举所有变体的事件流资源。
    ///
    /// Registers the event stream resources of every variant of this enum for
    /// an app.
    ///
    /// - `app`: 目标应用
    fn add_events(app: &App);
}

/// 事件 id：事件在流中的单调递增序号。
///
/// Event id: the monotonically increasing sequence number of an event in the
/// stream.
#[derive(Clone, Copy, Debug)]
pub struct EventId {
    /// 事件序号：自事件流创建以来的连续计数。
    ///
    /// The event sequence number: counted continuously since the stream was
    /// created.
    pub id: usize,
}

/// 事件序列：按顺序保存同一类型的事件实例。
///
/// Event sequence: holds the event instances of one event type in order.
pub struct EventSequence<E: Event> {
    /// 事件实例列表。
    ///
    /// The list of event instances.
    events: Vec<EventInstance<E>>,
    /// 序列中第一个事件的序号。
    ///
    /// The sequence number of the first event in the sequence.
    pub start_event_count: usize,
}

// Derived Default impl would incorrectly require E: Default
impl<E: Event> Default for EventSequence<E> {
    /// 创建空的事件序列。
    ///
    /// Creates an empty event sequence.
    fn default() -> Self {
        Self {
            events: Vec::new(),
            start_event_count: Default::default(),
        }
    }
}

impl<E: Event> std::ops::Deref for EventSequence<E> {
    type Target = Vec<EventInstance<E>>;

    /// 解引用到事件实例列表。
    ///
    /// Dereferences to the list of event instances.
    fn deref(&self) -> &Self::Target {
        &self.events
    }
}

impl<E: Event> std::ops::DerefMut for EventSequence<E> {
    /// 可变解引用到事件实例列表。
    ///
    /// Mutably dereferences to the list of event instances.
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.events
    }
}

/// 事件实例：事件数据与 id 的组合。
///
/// 事件数据是普通字段，互斥访问由资源锁（`Res`/`ResMut` 守卫）提供。
///
/// Event instance: an event payload together with its id. Exclusive access
/// is provided by the resource lock (`Res`/`ResMut` guards).
#[derive(Debug)]
pub struct EventInstance<E: Event> {
    /// 该实例的事件 id。
    ///
    /// The event id of this instance.
    pub event_id: EventId,
    /// 事件负载。
    ///
    /// The event payload.
    pub event: E,
}

impl<E: Event> EventInstance<E> {
    /// 返回事件负载的不可变引用。
    ///
    /// Returns an immutable reference to the event payload.
    pub fn get_event(&self) -> &E {
        &self.event
    }

    /// 返回事件负载的可变引用。
    ///
    /// Returns a mutable reference to the event payload.
    pub fn get_event_mut(&mut self) -> &mut E {
        &mut self.event
    }
}

/// 事件 trait：所有可发送事件的标记接口。
///
/// Event trait: the marker interface of every sendable event.
pub trait Event {}

/// 保存类型擦除的闭包，每个闭包对一种已注册事件类型调用
/// `Events::<T>::update()`。
///
/// `App::update()` 在每个 tick 开头调用 `EventUpdaters::run_all()`，
/// 交换全部事件缓冲区并丢弃旧事件。
///
/// Stores type-erased closures that call `Events::<T>::update()` for every
/// registered event type. `App::update()` calls
/// `EventUpdaters::run_all()` at the start of every tick, swapping all event
/// buffers and dropping old events.
#[derive(Resource)]
pub struct EventUpdaters {
    /// 已注册的类型擦除更新闭包。
    ///
    /// The registered type-erased update closures.
    updaters: Vec<Box<dyn Fn(&World) + Send + Sync>>,
}

impl EventUpdaters {
    /// 创建空的更新器集合。
    ///
    /// Creates an empty updater set.
    pub fn new() -> Self {
        Self {
            updaters: Vec::new(),
        }
    }

    /// 注册一个类型擦除闭包，在 `run_all` 被调用时执行
    /// `Events::<T>::update()`。
    ///
    /// Registers a type-erased closure that calls `Events::<T>::update()`
    /// when `run_all` is invoked.
    ///
    /// - `T`: 要更新事件流的事件类型
    pub fn register<T: Event + 'static + Send + Sync>(&mut self) {
        self.updaters.push(Box::new(|world: &World| {
            if let Some(mut events) = world.get_resource_mut::<Events<T>>() {
                events.update();
            }
        }));
    }

    /// 对每个已注册的 `Events<T>` 资源调用 `update()`；事件保留量约在
    /// 两个 tick 之内。
    ///
    /// Calls `update()` on every registered `Events<T>` resource, bounding
    /// retained events to roughly two ticks.
    ///
    /// - `world`: 事件流所在的世界
    pub fn run_all(&self, world: &World) {
        for updater in &self.updaters {
            updater(world);
        }
    }
}

/// 事件流资源：保存某一事件类型的旧事件与新事件。
///
/// Event stream resource: holds the old and new events of one event type.
#[derive(Resource)]
pub struct Events<E: Event + 'static> {
    /// 上一个 tick 的事件：仍可被落后一个 tick 的读取者读到。
    ///
    /// The events of the previous round: still readable by a reader lagging
    /// one tick.
    pub old_events: EventSequence<E>,
    /// 当前缓冲中新产生的事件。
    ///
    /// The events produced in the current round.
    pub new_events: EventSequence<E>,
    /// 已发送事件的累计计数。
    ///
    /// The cumulative count of sent events.
    pub event_count: usize,
}

/// 自动交换新旧缓冲区之前保留的最大事件数量；`old_events` 与
/// `new_events` 合计超过该阈值时，`send()` 内部调用 `update()`。
///
/// Maximum number of events to retain before automatically swapping the
/// old/new buffers. When the combined size of `old_events` + `new_events`
/// exceeds this threshold, `send()` calls `update()` internally.
const MAX_EVENTS_BEFORE_UPDATE: usize = 1024;

impl<E: Event> Events<E> {
    /// 创建空的事件流。
    ///
    /// Creates an empty event stream.
    pub fn new() -> Self {
        Events {
            old_events: EventSequence::default(),
            new_events: EventSequence::default(),
            event_count: 0,
        }
    }

    /// 发送一个事件并返回其 id。
    ///
    /// Sends an event and returns its id.
    ///
    /// - `event`: 要发送的事件负载
    pub fn send(&mut self, event: E) -> EventId {
        // Swap old/new buffers when the retained count exceeds the limit.
        if self.old_events.len() + self.new_events.len() > MAX_EVENTS_BEFORE_UPDATE {
            self.update();
        }

        let event_id = EventId {
            id: self.event_count,
        };
        self.new_events.push(EventInstance { event_id, event });
        self.event_count += 1;
        event_id
    }

    /// 把 `new_events` 缓冲区换入 `old_events` 并清空 `new_events`：
    /// 当前缓冲的事件变为旧事件（落后一 tick 的读取者仍可读取），更早的事件被丢弃。
    ///
    /// Swaps the `new_events` buffer into `old_events` and clears
    /// `new_events`: current-buffer events become "old" (still readable by
    /// lagging readers) and older events are dropped.
    pub fn update(&mut self) {
        std::mem::swap(&mut self.old_events, &mut self.new_events);
        self.new_events.clear();
        self.old_events.start_event_count = self.event_count - self.old_events.len();
        self.new_events.start_event_count = self.event_count;
    }

    /// 当前保留的事件总数。
    ///
    /// The total number of retained events.
    #[inline]
    pub fn len(&self) -> usize {
        self.old_events.len() + self.new_events.len()
    }

    /// 当前是否没有保留任何事件。
    ///
    /// Whether no event is currently retained.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 当前保留的最早事件的序号。
    ///
    /// The sequence number of the oldest retained event.
    pub fn oldest_event_count(&self) -> usize {
        self.old_events
            .start_event_count
            .min(self.new_events.start_event_count)
    }
}
