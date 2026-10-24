//! 方块实体的脏变更跟踪组件。
//!
//! Dirty change tracking component for block entities.

use sc_ecs::component::Component;

/// 挂在方块实体上、表示有待写回或网络同步的组件。
///
/// Component attached to a block entity indicating pending writeback or
/// network sync.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlockEntityDirty {
    /// 实体状态是否需要序列化并保存到 LevelDB。
    ///
    /// Whether the entity state needs to be serialized and saved to LevelDB.
    pub save_dirty: bool,
    /// 视觉/复制字段是否需要经 `BlockActorDataPacket` 广播。
    ///
    /// Whether visual/replicated fields need to be broadcast via
    /// `BlockActorDataPacket`.
    pub network_dirty: bool,
}

impl BlockEntityDirty {
    /// 无待写回、无待同步。
    ///
    /// Nothing to write back or sync.
    pub const CLEAN: Self = Self {
        save_dirty: false,
        network_dirty: false,
    };

    /// 既需要写存又需要同步。
    ///
    /// Both save writeback and network sync pending.
    pub const ALL: Self = Self {
        save_dirty: true,
        network_dirty: true,
    };

    /// 仅需要写存。
    ///
    /// Save writeback only.
    pub fn save_only() -> Self {
        Self {
            save_dirty: true,
            network_dirty: false,
        }
    }

    /// 仅需要同步。
    ///
    /// Network sync only.
    pub fn network_only() -> Self {
        Self {
            save_dirty: false,
            network_dirty: true,
        }
    }

    /// 是否完全干净。
    ///
    /// Whether nothing is pending.
    pub fn is_clean(&self) -> bool {
        !self.save_dirty && !self.network_dirty
    }

    /// 标记需要写存。
    ///
    /// Marks save writeback pending.
    pub fn mark_save(&mut self) {
        self.save_dirty = true;
    }

    /// 标记需要同步。
    ///
    /// Marks network sync pending.
    pub fn mark_network(&mut self) {
        self.network_dirty = true;
    }

    /// 同时标记写存与同步。
    ///
    /// Marks both save writeback and network sync pending.
    pub fn mark_all(&mut self) {
        self.save_dirty = true;
        self.network_dirty = true;
    }
}
