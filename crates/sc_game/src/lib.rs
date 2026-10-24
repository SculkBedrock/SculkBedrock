//! 游戏逻辑域，与网络域物理隔离。
//!
//! Game logic domain, physically isolated from the network domain.
//!
//! 职责：
//! Responsibilities:
//! - 多世界枢纽（`world::WorldHub`）：每个维度/子世界持有一个 ECS `World`，
//!   每 tick 对所有 Running 实例执行同一套游戏命令；
//!   multi-world hub (`world::WorldHub`): holds one ECS `World` per
//!   dimension/sub-world, running the same game commands over Running
//!   instances each tick;
//! - 跨世界总线（`world::CrossWorldBus`）：世界之间只交换值（带 from/to 的
//!   事件），从不共享引用或锁；
//!   cross-world bus (`world::CrossWorldBus`): worlds exchange values only
//!   (events with from/to), never shared references or locks;
//! - 网络边界（`net::NetworkOutbox`）：游戏域只发出 intent（不含协议细节），
//!   由 sc_network 的 schedule 插件负责编码。
//!   network boundary (`net::NetworkOutbox`): the game domain emits intents
//!   (no protocol details) for the sc_network schedule plugin to encode.
//!
//! 依赖方向：`sc_game -> (sc_world, sc_entity, sc_block)`，不依赖 sc_network。
//! Dependency direction: `sc_game -> (sc_world, sc_entity, sc_block)` with
//! no dependency on sc_network.
//!
//! 布局：每个游戏域一个目录（`world`、`region`、`block`、`inventory`、`item`、
//! `movement`、`net`）。`net::intent` 镜像 `sc_network::protocol`（每个 intent
//! 族一个主题文件、宏生成的枚举），但不区分 `client` / `server`。
//! Layout: one folder per game domain (`world`, `region`, `block`,
//! `inventory`, `item`, `movement`, `net`). `net::intent` mirrors
//! `sc_network::protocol` (one topic file per intent family, macro-generated
//! enum) but without a `client` / `server` split.

pub mod block;
pub mod block_entity;
pub mod damage;
pub mod furnace;
pub mod hunger;
pub mod inventory;
pub mod item;
pub mod movement;
pub mod net;
pub mod oxygen;
pub mod plugin;
pub mod region;
pub mod respawn;
pub mod world;

pub use block::{
    block_changed_to_outbox, harvested_for_hand, roll_break_drops, BreakSettlementReceipts,
};
pub use damage::{
    apply_fall_damage, calc_fall_damage, decide_fall_damage, Dead, FallDamageInput,
    FallDamageOutcome, FallSettlement, LandingMaterial, FALL_DAMAGE_OFFSET, FALL_LANDING_THRESHOLD,
};
pub use inventory::{
    bump_inventory_revision, ContainerKind, ContainerOpen, CraftingRevision, CraftingSession,
    PLAYER_INVENTORY_WINDOW_ID,
};
pub use inventory::crafting::{
    BlockProcessState, CraftOutcome, ProcessSnapshot, RecipeBookState, SharedItemTags,
    SharedRecipeRegistry,
};
pub use item::{DropHandle, ItemDrop, ItemDropStore, RegionItemDropStore};
pub use movement::{
    get_or_create_swimming, is_player_sneaking, PlayerActionFlags, PlayerActionState, PlayerMovement,
    PlayerMovementState, Swimming, SwimmingState,
};
pub use net::{
    dispatch_intent_hooks, intent_fault_runtime_id, BlockAnimationCue, BlockBreakProgressCue,
    HurtEntity, IntentReliability, IntentSendEvent, IntentSendHook, IntentSendHooks, NetworkIntent,
    NetworkOutbox, OutboxAdmissionStats, OutboxPushError, ParticleCue, PlayBlockAnimation,
    PlayerInput, PlayerMoveMode, SoundCue, SyncAirSupply, SyncHealth, SyncPlayerSwimming,
};
pub use net::{
    escalate_block_fact, escalate_inventory_fact, flush_pending_inventory_resync,
    request_column_refresh, FactBackpressure, PendingInventoryResync,
};
pub use oxygen::{oxygen_tick, OxygenClock, OxygenClockData};
pub use net::{escalate_fact_to_connection_fault, ConnectionFault, PendingConnectionFaults};
pub use plugin::SCGamePlugin;
pub use region::{
    region_tick, BoundedRegionScheduler, EntityRegion, OperationId, Region, RegionCommands,
    RegionDriver, RegionGrid, RegionHub, RegionId, RegionPhysicsCommand, TransactionCoordinator,
};
pub use world::{
    AnnouncementBus, CrossWorldBus, CrossWorldEvent, CrossWorldPing, GameWorld, GameWorldEntry,
    GameWorldId, GameWorldMap, PingBus, PingMailbox, PlayerGameWorld, RegionEpoch, RegionShardId,
    RegionTick, TeleportBus, TeleportRequest, TickCounter, WorldAnnouncement, WorldHub,
    WorldInstanceId, WorldMailbox, WorldStatus,
};

// ---------------------------------------------------------------------------
// Compatibility shims: pre-reorganisation paths keep resolving so downstream
// crates and plugins migrate on their own schedule. New code uses the folder
// paths above (`world::bus`, `block::interaction`, `inventory::*`, ...).
// ---------------------------------------------------------------------------

/// 原 `sc_game::bus`（现为 [`world::bus`]）。
///
/// Former `sc_game::bus` (now [`world::bus`]).
pub mod bus {
    pub use crate::world::bus::*;
}

/// 原 `sc_game::interaction`（现为 [`block::interaction`]）。
///
/// Former `sc_game::interaction` (now [`block::interaction`]).
pub mod interaction {
    pub use crate::block::interaction::*;
}

/// 原 `sc_game::crafting`（现为 [`inventory::crafting`]）。
///
/// Former `sc_game::crafting` (now [`inventory::crafting`]).
pub mod crafting {
    pub use crate::inventory::crafting::*;
}

/// 原 `sc_game::craft_inventory`（现为 [`inventory::craft_inventory`]）。
///
/// Former `sc_game::craft_inventory` (now [`inventory::craft_inventory`]).
pub mod craft_inventory {
    pub use crate::inventory::craft_inventory::*;
}

/// 原 `sc_game::container`（现为 [`inventory::container`]）。
///
/// Former `sc_game::container` (now [`inventory::container`]).
pub mod container {
    pub use crate::inventory::container::*;
}

/// 原 `sc_game::item_drop`（现为 [`item::drop`]）。
///
/// Former `sc_game::item_drop` (now [`item::drop`]).
pub mod item_drop {
    pub use crate::item::drop::*;
}

/// 原 `sc_game::net_backpressure`（现为 [`net::backpressure`]）。
///
/// Former `sc_game::net_backpressure` (now [`net::backpressure`]).
pub mod net_backpressure {
    pub use crate::net::backpressure::*;
}

/// 原 `sc_game::net_faults`（现为 [`net::faults`]）。
///
/// Former `sc_game::net_faults` (now [`net::faults`]).
pub mod net_faults {
    pub use crate::net::faults::*;
}
