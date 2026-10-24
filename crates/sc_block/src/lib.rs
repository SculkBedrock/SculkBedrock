//! SC 方块系统核心（机制层）。
//!
//! 提供：
//! - [`registry::BlockStateRegistry`]（引导模式：由 LevelDB 解析路径上的引导字典喂入，
//!   零数据文件即可回答 hash 到方块的映射）；
//! - [`access::BlockRead`]（基于现有区块缓存的只读 get_block API）；
//! - `/blockat` / `/setblock` 调试命令。
//!
//! 稠密模型（调色板数据就绪后启用）：
//! - [`registry::BlockStateRegistry`] 持有稠密的 [`state::BlockStateId`] 表，附带
//!   hash / network id 双向映射，提供 O(1) 的 `state_of` / `with_property`；
//! - [`state::BlockComponentFlags`] 是以稠密 id 为下标的能力位掩码表。
//!
//! 分层约束：本 crate 不得指名任何具体方块（门/作物/箱子等）；
//! 唯一例外是协议哨兵 `minecraft:air` / `minecraft:unknown`（定义于 sc_world 侧）。
//! 所有方块行为均由版本包插件实现。
//!
//! SC block system core (mechanics layer).
//!
//! Provides:
//! - [`registry::BlockStateRegistry`] (bootstrap mode: fed by the bootstrap dictionary
//!   on the LevelDB parsing path, answers hash-to-block mapping with zero data files);
//! - [`access::BlockRead`] (read-only get_block API over the existing chunk cache);
//! - `/blockat` / `/setblock` debug commands.
//!
//! Dense model (active once the palette lands):
//! - [`registry::BlockStateRegistry`] holds the dense [`state::BlockStateId`] table with
//!   bidirectional hash / network-id mappings, offering O(1) `state_of` / `with_property`;
//! - [`state::BlockComponentFlags`] is a capability bitmask table indexed by dense id.
//!
//! Layering constraint: this crate must not name any concrete block (door/crop/chest, etc.);
//! the only exception is the protocol sentinels `minecraft:air` / `minecraft:unknown` (defined
//! on the sc_world side). All block behavior is implemented by version-pack plugins.

pub mod access;
pub mod block_json;
pub mod commands;
pub mod item_wear;
pub mod mining_drops;
pub mod position;
pub mod registry;
pub mod state;
pub mod workstation;
pub mod write;

use log::warn;
use sc_command::registry::CommandRegistry;
use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_ecs::schedule::PostUpdate;
use sc_log::t_log;
use sc_utils::schedule::SCEventUpdate;

use crate::registry::BlockStateRegistry;
use crate::state::BlockComponentFlags;
use crate::write::{
    BlockChangeQueue, BlockChangeResult, BlockChanged, BlockChangedQueue, PendingBlockChangeLoads,
};

/// 方块系统 ECS 插件：注册状态表、事件队列与调试命令，并挂载单写者方块变更系统。
///
/// Block system ECS plugin: registers the state table, event queues and debug
/// commands, and mounts the single-writer block change systems.
pub struct SCBlockPlugin;

impl Plugin for SCBlockPlugin {
    /// 构建插件：插入资源、注册事件与调试命令，并把方块变更系统挂到调度上。
    ///
    /// Builds the plugin: inserts resources, registers events and debug commands,
    /// and schedules the block change systems.
    ///
    /// - `app`: ECS 应用，插件在此挂载资源与系统
    fn build(&self, app: &App) {
        app.insert_resource(BlockStateRegistry::new())
            .insert_resource(BlockComponentFlags::default())
            .insert_resource(crate::block_json::BlockJsonRegistry::new())
            .insert_resource(BlockChangeQueue::default())
            .insert_resource(BlockChangedQueue::default())
            .insert_resource(PendingBlockChangeLoads::default())
            .add_event::<BlockChanged>()
            .add_event::<BlockChangeResult>();
        // Registers debug commands into the command registry. Requires SCCommandPlugin
        // to be added before SCBlockPlugin in bootstrap (matches the command core build-time convention).
        if let Some(mut command_registry) = app.world().get_resource_mut::<CommandRegistry>() {
            commands::register_block_commands(&mut command_registry);
        } else {
            warn!("{}", t_log!("console.block.registry_missing"));
        }
        app.add_systems(
            SCEventUpdate,
            (commands::blockat_command, commands::setblock_command),
        );
        // Single-writer apply: lands uniformly at end of tick, after builtin/plugin command systems.
        // The network broadcast system (sc_network) also runs in PostUpdate and is assembled after
        // this plugin, so the same tick can broadcast.
        app.add_systems(
            PostUpdate,
            (write::poll_block_change_loads, write::apply_block_changes),
        );
    }
}
