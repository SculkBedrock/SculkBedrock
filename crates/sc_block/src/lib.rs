//! SC block system core (mechanics layer).
//!
//! Design doc: docs/block_system_design.md. Provides:
//! - [`registry::BlockStateRegistry`] (bootstrap mode: fed by the bootstrap dictionary
//!   on the LevelDB parse path, answers hash-to-block with zero data files);
//! - [`access::BlockRead`] (read-only get_block API over the existing chunk cache);
//! - `/blockat` / `/setblock` debug commands (acceptance observability).
//!
//! Dense model (active once the palette lands):
//! - [`registry::BlockStateRegistry`] holds the dense [`state::BlockStateId`] table with a
//!   two-way hash/network-id map, offering O(1) `state_of` / `with_property`;
//! - [`state::BlockComponentFlags`] is a capability bitmask table indexed by dense id
//!   (design doc section 2 flags enum).
//!
//! Layering constraint: this crate must not name any concrete block (door/crop/chest, etc.);
//! the only exception is the protocol sentinels `minecraft:air` / `minecraft:unknown` (defined
//! on the sc_world side). All block behavior is implemented by version-pack plugins.

pub mod access;
pub mod block_json;
pub mod commands;
pub mod mining_drops;
pub mod position;
pub mod registry;
pub mod state;
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

pub struct SCBlockPlugin;

impl Plugin for SCBlockPlugin {
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
