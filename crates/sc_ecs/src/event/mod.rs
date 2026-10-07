use crate::app::App;
use crate::world::World;
use sc_ecs_macros::Resource;
pub use sc_ecs_macros::{EnumEvents, Event};

pub mod iterator;
pub mod priority;
pub mod reader;

pub trait EnumEvents {
    fn send_event(self, world: &World) -> Option<EventId>;
    fn add_events(app: &App);
}

#[derive(Clone, Copy, Debug)]
pub struct EventId {
    pub id: usize,
}

pub struct EventSequence<E: Event> {
    events: Vec<EventInstance<E>>,
    pub start_event_count: usize,
}

// Derived Default impl would incorrectly require E: Default
impl<E: Event> Default for EventSequence<E> {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            start_event_count: Default::default(),
        }
    }
}

impl<E: Event> std::ops::Deref for EventSequence<E> {
    type Target = Vec<EventInstance<E>>;
    fn deref(&self) -> &Self::Target {
        &self.events
    }
}

impl<E: Event> std::ops::DerefMut for EventSequence<E> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.events
    }
}

/// Event instance. Event data is a plain field: the read path borrows `&E`
/// through `Res<Events<E>>` (read guard) and the write path borrows `&mut E`
/// through `ResMut<Events<E>>` (write guard); the resource lock provides
/// mutual exclusion.
#[derive(Debug)]
pub struct EventInstance<E: Event> {
    pub event_id: EventId,
    pub event: E,
}

impl<E: Event> EventInstance<E> {
    pub fn get_event(&self) -> &E {
        &self.event
    }

    pub fn get_event_mut(&mut self) -> &mut E {
        &mut self.event
    }
}

pub trait Event {}

/// Stores type-erased closures that call `Events::<T>::update()` for every
/// registered event type. Without periodic `update()` calls, the `new_events`
/// Vec inside `Events<E>` grows **monotonically and without bound** — every
/// `send()` pushes to it, but nothing ever moves events to `old_events` or
/// clears `old_events`.
///
/// `App::update()` calls `EventUpdaters::run_all()` at the start of every
/// tick, ensuring all event buffers are swapped and old events are dropped.
#[derive(Resource)]
pub struct EventUpdaters {
    updaters: Vec<Box<dyn Fn(&World) + Send + Sync>>,
}

impl EventUpdaters {
    pub fn new() -> Self {
        Self {
            updaters: Vec::new(),
        }
    }

    /// Register a type-erased closure that will call `Events::<T>::update()`
    /// when `run_all` is invoked.
    pub fn register<T: Event + 'static + Send + Sync>(&mut self) {
        self.updaters.push(Box::new(|world: &World| {
            if let Some(mut events) = world.get_resource_mut::<Events<T>>() {
                events.update();
            }
        }));
    }

    /// Call `update()` on every registered `Events<T>` resource. This swaps
    /// `new_events` into `old_events` and drops the previous `old_events`,
    /// bounding memory usage to roughly two ticks of events.
    pub fn run_all(&self, world: &World) {
        for updater in &self.updaters {
            updater(world);
        }
    }
}

#[derive(Resource)]
pub struct Events<E: Event + 'static> {
    pub old_events: EventSequence<E>,
    pub new_events: EventSequence<E>,
    pub event_count: usize,
}

/// Maximum number of events to retain before automatically swapping
/// old/new buffers. When the combined size of old_events + new_events
/// exceeds this threshold, `update()` is called from within `send()`.
/// This prevents unbounded memory growth when events are produced faster
/// than they are consumed (e.g. Minecraft packets during a busy session).
const MAX_EVENTS_BEFORE_UPDATE: usize = 1024;

impl<E: Event> Events<E> {
    pub fn new() -> Self {
        Events {
            old_events: EventSequence::default(),
            new_events: EventSequence::default(),
            event_count: 0,
        }
    }

    pub fn send(&mut self, event: E) -> EventId {
        // Periodically swap old/new buffers to prevent unbounded memory
        // growth when events are produced faster than they are consumed.
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

    /// Swap the new_events buffer into old_events and clear the previous
    /// old_events. This implements the standard double-buffered event cleanup
    /// pattern: events from the current buffer become "old" (still readable
    /// by lagging readers), and events from two buffers ago are dropped.
    pub fn update(&mut self) {
        std::mem::swap(&mut self.old_events, &mut self.new_events);
        self.new_events.clear();
        self.old_events.start_event_count = self.event_count - self.old_events.len();
        self.new_events.start_event_count = self.event_count;
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.old_events.len() + self.new_events.len()
    }

    pub fn oldest_event_count(&self) -> usize {
        self.old_events
            .start_event_count
            .min(self.new_events.start_event_count)
    }
}
