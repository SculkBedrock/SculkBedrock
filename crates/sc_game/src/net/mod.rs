//! Network boundary: the only boundary between the game and network domains.
//!
//! - Outbound: game commands never import protocol packets; everything
//!   outbound is an intent with game-side semantics only.
//! - Inbound: the network plugin decodes client packets into game-side
//!   input values buffered for game commands.
//!
//! Layout mirrors `sc_network::protocol` but without a `client` / `server`
//! split: `intent/` holds one topic file per intent family plus the
//! macro-generated [`intent::NetworkIntent`] enum; `outbox` owns the bounded
//! queue; `hooks` owns interception; `backpressure` / `faults` own the
//! terminal-state contracts.

pub mod backpressure;
pub mod faults;
pub mod hooks;
pub mod input;
pub mod intent;
pub mod outbox;

pub use backpressure::{
    escalate_block_fact, escalate_inventory_fact, flush_pending_inventory_resync,
    intent_fault_runtime_id, request_column_refresh, FactBackpressure, IntentPublisher,
    PendingInventoryResync,
};
pub use faults::{escalate_fact_to_connection_fault, ConnectionFault, PendingConnectionFaults};
pub use hooks::{dispatch_intent_hooks, IntentSendEvent, IntentSendHook, IntentSendHooks};
pub use input::PlayerInput;
pub use intent::{
    BlockBreakProgress, BlockBreakProgressCue, ClosePlayerInventory, CorrectBlockPrediction,
    CraftResponse, DespawnItemEntity, HurtEntity, IntentPayload, IntentReliability,
    MoveEntityAbsolute, MoveItemEntity, MovePlayer, NetworkIntent, OpenCraftingStation,
    OpenPlayerInventory, ParticleCue, PlayParticle, PlaySound, PlayerMoveMode, RemoveEntity,
    RespawnPlayer, ResyncInventory, SetEntityMotion, SoundCue, SpawnItemEntity,
    SyncFurnaceProgress, SyncFurnaceWindow, SyncHealth, SyncHunger, TakeItemEntity, UpdateBlock,
    UpdateInventorySlot, UpdateItemStackSize,
};
pub use outbox::{
    intent_estimated_bytes, intent_estimated_bytes_for, NetworkOutbox, OutboxAdmissionStats,
    OutboxPushError,
};
