//! Cross-world event bus.
//!
//! Worlds share no references, exchanging values only through events with
//! `from/to`: `CrossWorldBus<T>` (root-world resource) enqueues into an
//! outbox (fire-and-forget; ordering is per-source-target FIFO), and each
//! tick the outbox dispatches into the target instance's `WorldMailbox<T>`
//! for next-tick game commands.

use sc_ecs::entity::EntityId;
use sc_ecs::resource::{cached_resource_id_for, Resource, ResourceId};
use std::collections::VecDeque;

use crate::world::GameWorldId;
use sc_log::t_log;

/// Cross-world event: source world to target world (values only).
#[derive(Clone, Debug)]
pub struct CrossWorldEvent<T> {
    pub from: GameWorldId,
    pub to: GameWorldId,
    pub payload: T,
}

/// Cross-world event bus (one resource per payload type T).
#[derive(Clone, Debug)]
pub struct CrossWorldBus<T> {
    outbox: VecDeque<CrossWorldEvent<T>>,
    max_events: usize,
}

// Manual Default: derive would bound generics with `T: Default`.
impl<T> Default for CrossWorldBus<T> {
    fn default() -> Self {
        Self {
            outbox: VecDeque::new(),
            max_events: 8192,
        }
    }
}

impl<T> CrossWorldBus<T> {
    pub fn with_capacity(max_events: usize) -> Self {
        Self {
            outbox: VecDeque::new(),
            max_events: max_events.max(1),
        }
    }

    /// Send into the outbox (dispatched next tick).
    pub fn send(&mut self, from: GameWorldId, to: GameWorldId, payload: T) {
        if self.outbox.len() >= self.max_events {
            self.outbox.pop_front();
            log::warn!(
                "{}",
                t_log!("console.game.bus_full", max = self.max_events)
            );
        }
        self.outbox.push_back(CrossWorldEvent { from, to, payload });
    }

    /// Drain pending events (per-source-target FIFO preserved).
    pub fn drain_outbox(&mut self) -> Vec<CrossWorldEvent<T>> {
        self.outbox.drain(..).collect()
    }

    pub fn pending(&self) -> usize {
        self.outbox.len()
    }
}

// Generic resource: manual Resource impl.
// Layout code must stay in sync with fields.
impl<T: Send + Sync + 'static> Resource for CrossWorldBus<T> {
    fn resource_id() -> ResourceId {
        cached_resource_id_for::<Self>(|| {
            format!(
                "CrossWorldBus{{max_events:usize,outbox:VecDeque<CrossWorldEvent<T>>}}<{}>",
                std::any::type_name::<T>()
            )
        })
    }

    fn name() -> String {
        format!("CrossWorldBus<{}>", std::any::type_name::<T>())
    }
}

/// In-world mailbox: `dispatch` delivers this world's events for local commands.
#[derive(Clone, Debug)]
pub struct WorldMailbox<T> {
    pub events: VecDeque<CrossWorldEvent<T>>,
    max_events: usize,
}

impl<T> Default for WorldMailbox<T> {
    fn default() -> Self {
        Self {
            events: VecDeque::new(),
            max_events: 8192,
        }
    }
}

impl<T: Send + Sync + 'static> Resource for WorldMailbox<T> {
    fn resource_id() -> ResourceId {
        cached_resource_id_for::<Self>(|| {
            format!(
                "WorldMailbox{{events:VecDeque<CrossWorldEvent<T>>,max_events:usize}}<{}>",
                std::any::type_name::<T>()
            )
        })
    }

    fn name() -> String {
        format!("WorldMailbox<{}>", std::any::type_name::<T>())
    }
}

impl<T> WorldMailbox<T> {
    pub fn with_capacity(max_events: usize) -> Self {
        Self {
            events: VecDeque::new(),
            max_events: max_events.max(1),
        }
    }

    pub fn push(&mut self, event: CrossWorldEvent<T>) {
        if self.events.len() >= self.max_events {
            self.events.pop_front();
            log::warn!(
                "{}",
                t_log!("console.game.mailbox_full", max = self.max_events)
            );
        }
        self.events.push_back(event);
    }

    pub fn drain(&mut self) -> Vec<CrossWorldEvent<T>> {
        self.events.drain(..).collect()
    }
}

// Skeleton demo payload: cross-world ping.

/// Ping payload proving world-to-world value exchange.
#[derive(Clone, Debug)]
pub struct CrossWorldPing {
    pub origin: GameWorldId,
    pub from_tick: u64,
}

/// Ping bus on the root world.
pub type PingBus = CrossWorldBus<CrossWorldPing>;

/// Per-world ping mailboxes (attached by `spawn_world`).
pub type PingMailbox = WorldMailbox<CrossWorldPing>;

/// Zone ping payload (zone-to-zone value exchange demo).
#[derive(Clone, Debug)]
pub struct RegionPing {
    pub from_tick: u64,
}

/// Zone event bus (inter-zone values on the partitioned world).
pub type RegionPingBus = sc_ecs::region::RegionBus<crate::region::RegionId, RegionPing>;

/// Cross-world teleport request.
///
/// Source to target world; `process_teleports` on the root world applies
/// ownership, position, and chunk subscription updates.
#[derive(Clone, Debug)]
pub struct TeleportRequest {
    pub from: GameWorldId,
    pub to: GameWorldId,
    /// Target entity (currently a root-world player entity).
    pub entity: EntityId,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Cross-world teleport bus (root-world resource).
pub type TeleportBus = CrossWorldBus<TeleportRequest>;

/// Cross-world announcement (inter-world broadcast).
#[derive(Clone, Debug)]
pub struct WorldAnnouncement {
    pub from: GameWorldId,
    pub message: String,
}

/// Announcement bus (root-world resource).
pub type AnnouncementBus = CrossWorldBus<WorldAnnouncement>;
