//! Game logic domain, physically isolated from the network domain.
//!
//! Responsibilities:
//! - multi-world hub (`world::WorldHub`): holds one ECS `World` per
//!   dimension/sub-world, running the same game commands over Running
//!   instances each tick;
//! - cross-world bus (`world::CrossWorldBus`): worlds exchange values only
//!   (events with from/to), never shared references or locks;
//! - network boundary (`net::NetworkOutbox`): the game domain emits intents
//!   (no protocol details) for the sc_network schedule plugin to encode.
//!
//! Dependency direction: `sc_game -> (sc_world, sc_entity, sc_block)` with
//! no dependency on sc_network.
//!
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
pub mod plugin;
pub mod region;
pub mod respawn;
pub mod world;

pub use block::{
    block_changed_to_outbox, harvested_for_hand, roll_break_drops, DropReceipts,
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
pub use movement::{PlayerMovement, PlayerMovementState};
pub use net::{
    dispatch_intent_hooks, intent_fault_runtime_id, BlockBreakProgressCue, HurtEntity,
    IntentReliability, IntentSendEvent, IntentSendHook, IntentSendHooks, NetworkIntent,
    NetworkOutbox, OutboxAdmissionStats, OutboxPushError, ParticleCue, PlayerInput, PlayerMoveMode,
    SoundCue, SyncHealth,
};
pub use net::{
    escalate_block_fact, escalate_inventory_fact, flush_pending_inventory_resync,
    request_column_refresh, FactBackpressure, PendingInventoryResync,
};
pub use net::{escalate_fact_to_connection_fault, ConnectionFault, PendingConnectionFaults};
pub use plugin::SCGamePlugin;
pub use region::{
    region_tick, EntityRegion, OperationId, Region, RegionCommands, RegionDriver, RegionGrid,
    RegionHub, RegionId, RegionPhysicsCommand, TransactionCoordinator,
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

/// Former `sc_game::bus` (now [`world::bus`]).
pub mod bus {
    pub use crate::world::bus::*;
}

/// Former `sc_game::interaction` (now [`block::interaction`]).
pub mod interaction {
    pub use crate::block::interaction::*;
}

/// Former `sc_game::crafting` (now [`inventory::crafting`]).
pub mod crafting {
    pub use crate::inventory::crafting::*;
}

/// Former `sc_game::craft_inventory` (now [`inventory::craft_inventory`]).
pub mod craft_inventory {
    pub use crate::inventory::craft_inventory::*;
}

/// Former `sc_game::container` (now [`inventory::container`]).
pub mod container {
    pub use crate::inventory::container::*;
}

/// Former `sc_game::item_drop` (now [`item::drop`]).
pub mod item_drop {
    pub use crate::item::drop::*;
}

/// Former `sc_game::net_backpressure` (now [`net::backpressure`]).
pub mod net_backpressure {
    pub use crate::net::backpressure::*;
}

/// Former `sc_game::net_faults` (now [`net::faults`]).
pub mod net_faults {
    pub use crate::net::faults::*;
}
