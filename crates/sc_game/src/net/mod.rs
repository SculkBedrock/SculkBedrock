//! 网络边界：游戏域与网络域之间的唯一边界。
//!
//! Network boundary: the only boundary between the game and network domains.
//!
//! - 出站：游戏命令从不导入协议包；一切出站内容都是只含游戏侧语义的 intent。
//!   Outbound: game commands never import protocol packets; everything
//!   outbound is an intent with game-side semantics only.
//! - 入站：网络插件把客户端包解码为游戏侧输入值，缓冲给游戏命令使用。
//!   Inbound: the network plugin decodes client packets into game-side
//!   input values buffered for game commands.
//!
//! 布局镜像 `sc_network::protocol`，但不区分 `client` / `server`：`intent/`
//! 每个 intent 族一个主题文件，外加宏生成的 [`intent::NetworkIntent`] 枚举；
//! `outbox` 拥有有界队列；`hooks` 拥有拦截；`backpressure` / `faults` 拥有
//! 终态契约。
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
    BlockAnimationCue, BlockBreakProgress, BlockBreakProgressCue, ClosePlayerInventory,
    CorrectBlockPrediction, CraftResponse, DespawnItemEntity, HurtEntity, IntentPayload,
    IntentReliability, MoveEntityAbsolute, MoveItemEntity, MovePlayer, NetworkIntent,
    OpenContainerWindow, OpenCraftingStation, OpenPlayerInventory, ParticleCue, PlayBlockAnimation,
    PlayParticle, PlaySound, PlayerMoveMode, RemoveEntity, RespawnPlayer, ResyncInventory,
    SetEntityMotion, SoundCue, SpawnItemEntity, SyncAirSupply, SyncChestWindow, SyncFurnaceProgress,
    SyncFurnaceWindow, SyncHealth, SyncHunger, SyncPlayerSwimming, TakeItemEntity, UpdateBlock,
    SyncHeldItem, UpdateInventorySlot, UpdateItemStackSize,
};
pub use outbox::{
    intent_estimated_bytes, intent_estimated_bytes_for, NetworkOutbox, OutboxAdmissionStats,
    OutboxPushError,
};
