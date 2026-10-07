//! Game logic domain, physically isolated from the network domain.
//!
//! Responsibilities:
//! - multi-world hub (`world::WorldHub`): holds one ECS `World` per
//!   dimension/sub-world, running the same game commands over Running
//!   instances each tick;
//! - cross-world bus (`bus::CrossWorldBus`): worlds exchange values only
//!   (events with from/to), never shared references or locks;
//! - network boundary (`net::NetworkOutbox`): the game domain emits intents
//!   (no protocol details) for the sc_network schedule plugin to encode.
//!
//! Dependency direction: `sc_game -> (sc_world, sc_entity, sc_block)` with
//! no dependency on sc_network.

pub mod block;
pub mod bus;
pub mod container;
pub mod crafting;
pub mod craft_inventory;
pub mod interaction;
pub mod item_drop;
pub mod movement;
pub mod net;
pub mod net_backpressure;
pub mod net_faults;
pub mod plugin;
pub mod region;
pub mod world;

pub use bus::{
    AnnouncementBus, CrossWorldBus, CrossWorldEvent, CrossWorldPing, PingBus, PingMailbox,
    TeleportBus, TeleportRequest, WorldAnnouncement, WorldMailbox,
};
pub use crafting::{
    BlockProcessState, CraftOutcome, CraftingRevision, CraftingSession, RecipeBookState,
    SharedItemTags, SharedRecipeRegistry,
};
pub use container::{ContainerKind, ContainerOpen, PLAYER_INVENTORY_WINDOW_ID};
pub use item_drop::{DropHandle, ItemDrop, ItemDropStore, RegionItemDropStore};
pub use movement::{PlayerMovement, PlayerMovementState};
pub use net::{
    dispatch_intent_hooks, BlockBreakProgressCue, IntentReliability, IntentSendEvent,
    IntentSendHook, IntentSendHooks, NetworkIntent, NetworkOutbox, OutboxAdmissionStats,
    OutboxPushError, ParticleCue, PlayerInput, PlayerMoveMode, SoundCue,
};
pub use net_backpressure::{
    escalate_block_fact, escalate_inventory_fact, flush_pending_inventory_resync,
    request_column_refresh, FactBackpressure, PendingInventoryResync,
};
pub use net_faults::{escalate_fact_to_connection_fault, ConnectionFault, PendingConnectionFaults};
pub use plugin::SCGamePlugin;
pub use region::{
    region_tick, EntityRegion, Region, RegionCommands, RegionDriver, RegionGrid, RegionHub,
    RegionId, RegionPhysicsCommand,
};
pub use world::{
    GameWorld, GameWorldEntry, GameWorldId, GameWorldMap, PlayerGameWorld, TickCounter, WorldHub,
    WorldStatus,
};
