//! 跨世界事件总线。
//!
//! Cross-world event bus.
//!
//! 各世界不共享任何引用，只通过带 `from/to` 的事件交换值：
//! `CrossWorldBus<T>`（根世界资源）把事件放入 outbox（发后不管；顺序按
//! 来源-目标 FIFO 保持），每 tick outbox 把事件投递到目标实例的
//! `WorldMailbox<T>`，供下一 tick 的游戏命令消费。
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

/// 跨世界事件：来源世界到目标世界（只传值）。
///
/// Cross-world event: source world to target world (values only).
#[derive(Clone, Debug)]
pub struct CrossWorldEvent<T> {
    /// 来源世界 id。
    ///
    /// Source world id.
    pub from: GameWorldId,
    /// 目标世界 id。
    ///
    /// Target world id.
    pub to: GameWorldId,
    /// 事件负载。
    ///
    /// Event payload.
    pub payload: T,
}

/// 跨世界事件总线（每种负载类型 T 一个资源）。
///
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
    /// 指定 outbox 容量构造（小于 1 按 1 处理）。
    ///
    /// Constructs with the given outbox capacity (values below 1 clamp to 1).
    ///
    /// - `max_events`: 最大待发事件数
    pub fn with_capacity(max_events: usize) -> Self {
        Self {
            outbox: VecDeque::new(),
            max_events: max_events.max(1),
        }
    }

    /// 放入 outbox（下一 tick 投递）。
    ///
    /// Send into the outbox (dispatched next tick).
    ///
    /// - `from`: 来源世界 id
    /// - `to`: 目标世界 id
    /// - `payload`: 事件负载
    ///
    /// 队列满时丢弃最旧事件并记录警告。
    /// When full, the oldest event is dropped and a warning is logged.
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

    /// 取走全部待发事件（保持来源-目标 FIFO 顺序）。
    ///
    /// Drain pending events (per-source-target FIFO preserved).
    pub fn drain_outbox(&mut self) -> Vec<CrossWorldEvent<T>> {
        self.outbox.drain(..).collect()
    }

    /// 待发事件数量。
    ///
    /// Number of pending events.
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

/// 世界内邮箱：`dispatch` 把本世界的事件投进来供本地命令消费。
///
/// In-world mailbox: `dispatch` delivers this world's events for local
/// commands.
#[derive(Clone, Debug)]
pub struct WorldMailbox<T> {
    /// 已投递、待本地命令消费的事件。
    ///
    /// Delivered events awaiting local commands.
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
    /// 指定容量构造（小于 1 按 1 处理）。
    ///
    /// Constructs with the given capacity (values below 1 clamp to 1).
    ///
    /// - `max_events`: 最大缓冲事件数
    pub fn with_capacity(max_events: usize) -> Self {
        Self {
            events: VecDeque::new(),
            max_events: max_events.max(1),
        }
    }

    /// 投递一个事件；队列满时丢弃最旧事件并记录警告。
    ///
    /// Pushes an event; when full the oldest event is dropped and a warning
    /// is logged.
    ///
    /// - `event`: 要投递的事件
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

    /// 取走全部已投递事件。
    ///
    /// Drains all delivered events.
    pub fn drain(&mut self) -> Vec<CrossWorldEvent<T>> {
        self.events.drain(..).collect()
    }
}

/// Ping 负载：用于世界间值交换。
///
/// Ping payload for world-to-world value exchange.
#[derive(Clone, Debug)]
pub struct CrossWorldPing {
    /// 发起 ping 的世界 id。
    ///
    /// World that originated the ping.
    pub origin: GameWorldId,
    /// 发起时的 tick。
    ///
    /// Tick at which the ping was sent.
    pub from_tick: u64,
}

/// 根世界上的 ping 总线。
///
/// Ping bus on the root world.
pub type PingBus = CrossWorldBus<CrossWorldPing>;

/// 每世界的 ping 邮箱（由 `spawn_world` 挂载）。
///
/// Per-world ping mailboxes (attached by `spawn_world`).
pub type PingMailbox = WorldMailbox<CrossWorldPing>;

/// 区域 ping 负载（区域间值交换）。
///
/// Zone ping payload (zone-to-zone value exchange).
#[derive(Clone, Debug)]
pub struct RegionPing {
    /// 发起时的 tick。
    ///
    /// Tick at which the ping was sent.
    pub from_tick: u64,
}

/// 区域事件总线（分区世界上区域间的值传递）。
///
/// Zone event bus (inter-zone values on the partitioned world).
pub type RegionPingBus = sc_ecs::region::RegionBus<crate::region::RegionId, RegionPing>;

/// 跨世界传送请求。
///
/// Cross-world teleport request.
///
/// 从来源世界到目标世界；根世界上的 `process_teleports` 负责应用
/// 所有权、位置与区块订阅更新。
/// Source to target world; `process_teleports` on the root world applies
/// ownership, position, and chunk subscription updates.
#[derive(Clone, Debug)]
pub struct TeleportRequest {
    /// 来源世界 id。
    ///
    /// Source world id.
    pub from: GameWorldId,
    /// 目标世界 id。
    ///
    /// Target world id.
    pub to: GameWorldId,
    /// 目标实体（当前是根世界上的玩家实体）。
    ///
    /// Target entity (currently a root-world player entity).
    pub entity: EntityId,
    /// 目标位置 X。
    ///
    /// Target position X.
    pub x: f32,
    /// 目标位置 Y。
    ///
    /// Target position Y.
    pub y: f32,
    /// 目标位置 Z。
    ///
    /// Target position Z.
    pub z: f32,
}

/// 跨世界传送总线（根世界资源）。
///
/// Cross-world teleport bus (root-world resource).
pub type TeleportBus = CrossWorldBus<TeleportRequest>;

/// 跨世界公告（世界间广播）。
///
/// Cross-world announcement (inter-world broadcast).
#[derive(Clone, Debug)]
pub struct WorldAnnouncement {
    /// 发起公告的世界 id。
    ///
    /// World that issued the announcement.
    pub from: GameWorldId,
    /// 公告文本。
    ///
    /// Announcement text.
    pub message: String,
}

/// 公告总线（根世界资源）。
///
/// Announcement bus (root-world resource).
pub type AnnouncementBus = CrossWorldBus<WorldAnnouncement>;
