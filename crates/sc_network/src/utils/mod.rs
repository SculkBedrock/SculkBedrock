pub mod adventure_settings;
pub mod auth;
pub mod compression_algorithm;
pub mod encryption;
mod server;

use sc_ecs::entity::EntityId;
use sc_ecs::resource::Resource;
use std::collections::HashMap;
use tokio::task::JoinHandle;

/// 跟踪每个玩家派生的异步任务句柄，以便玩家断开时统一中止。
///
/// Tracks async task handles spawned per player so they can all be aborted
/// when the player disconnects.
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

    /// 为玩家实体登记一个异步任务句柄；对该实体调用 `drop_thread` 时会中止它。
    ///
    /// Registers an async task handle for a player entity; aborted when
    /// `drop_thread` is called for that entity.
    pub fn insert(&mut self, entity: EntityId, handle: JoinHandle<()>) {
        self.threads.entry(entity).or_default().push(handle);
    }

    /// 中止并丢弃指定实体登记的全部任务句柄，并收缩内部 HashMap。
    ///
    /// Aborts and drops all task handles registered for the given entity, then
    /// shrinks the internal HashMap to release the freed capacity.
    pub fn drop_thread(&mut self, entity: &EntityId) -> Option<()> {
        let result = self.threads.remove(entity).map(|handles| {
            for handle in handles {
                handle.abort();
            }
        });
        // Release excess capacity so the HashMap does not keep its high-water mark.
        self.threads.shrink_to_fit();
        result
    }

    /// 回收已完成的句柄：移除正常结束后滞留的句柄，以及为已移除实体迟到登记的句柄。
    /// 每 tick 调用一次，成本是一次线性 `is_finished()` 检查。
    ///
    /// Reclaims finished task handles: those left in the Vec after normal
    /// completion and late registrations for already-removed entities. Called
    /// once per tick; costs one linear `is_finished()` check.
    pub fn sweep_finished(&mut self) {
        self.threads.retain(|_, handles| {
            handles.retain(|handle| !handle.is_finished());
            !handles.is_empty()
        });
    }
}
