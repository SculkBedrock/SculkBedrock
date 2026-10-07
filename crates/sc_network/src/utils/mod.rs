pub mod adventure_settings;
pub mod auth;
pub mod compression_algorithm;
pub mod encryption;
mod server;

use sc_ecs::entity::EntityId;
use sc_ecs::resource::Resource;
use std::collections::HashMap;
use tokio::task::JoinHandle;

/// Tracks all async tasks spawned per-player so they can be aborted when
/// the player disconnects. Previously only the `receive_packet` task was
/// tracked; the 8 fire-and-forget tasks in `player.rs` and `network.rs`
/// had their JoinHandles dropped immediately, making them impossible to
/// cancel. If a player disconnected while such a task was still running
/// (e.g. waiting on `sem.acquire()` inside `resource_pack_chunk_request`),
/// the task continued to hold a `World` clone and associated Arc references
/// until it completed — leaking memory over many join/leave cycles.
#[derive(Resource)]
pub struct ConnectionThreadManager {
    threads: HashMap<EntityId, Vec<JoinHandle<()>>>,
}

impl ConnectionThreadManager {
    pub fn new() -> ConnectionThreadManager {
        Self {
            threads: HashMap::new(),
        }
    }

    /// Register an async task handle for a player entity. The handle will
    /// be aborted when `drop_thread` is called for that entity.
    pub fn insert(&mut self, entity: EntityId, handle: JoinHandle<()>) {
        self.threads.entry(entity).or_default().push(handle);
    }

    /// Abort and drop ALL task handles registered for the given entity.
    /// Called from `drop_connection` when a player disconnects. Also shrinks
    /// the internal HashMap to release capacity that was allocated for the
    /// removed entries, preventing the HashMap from retaining excess memory
    /// over many join/leave cycles.
    pub fn drop_thread(&mut self, entity: &EntityId) -> Option<()> {
        let result = self.threads.remove(entity).map(|handles| {
            for handle in handles {
                handle.abort();
            }
        });
        // Release excess capacity left over from the removed entry. Without
        // this, the HashMap retains its high-water-mark capacity permanently.
        self.threads.shrink_to_fit();
        result
    }

    /// Reclaim finished task handles. Prevents two leaks: tasks that completed
    /// during normal operation but whose handles sit in the Vec until logout,
    /// and handles inserted for an already-removed entity after DropConnection
    /// (e.g. late CommandFeedback) that drop_thread would never clean.
    /// Called once per tick; cost is a linear `is_finished()` check.
    pub fn sweep_finished(&mut self) {
        self.threads.retain(|_, handles| {
            handles.retain(|handle| !handle.is_finished());
            !handles.is_empty()
        });
    }
}
