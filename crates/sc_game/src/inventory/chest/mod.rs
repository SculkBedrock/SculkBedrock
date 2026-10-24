//! 箱子方块核心游戏逻辑。
//!
//! 包含：
//! - `obstruction`: 正上方实体方块阻挡检测
//! - `double_chest`: 双箱合并与规范朝向定序
//! - `open`: 容器窗口打开与网络意图下发
//! - `tx`: 槽位读取、提交、Move 与 Swap 事务
//!
//! Chest block core game logic.
//!
//! Contains:
//! - `obstruction`: solid-block obstruction check above the chest;
//! - `double_chest`: double-chest merging and canonical facing ordering;
//! - `open`: container window opening and network intent dispatch;
//! - `tx`: slot reads, commits and Move/Swap transactions.

pub mod double_chest;
pub mod obstruction;
pub mod open;
pub mod tx;

#[cfg(test)]
mod tests;

pub use double_chest::{
    canonical_chest_order, chest_facing_at, find_paired_chest, perpendicular_neighbors,
};
pub use obstruction::is_chest_obstructed;
pub use open::{chest_viewer_count, open_chest_container};
pub use tx::{
    chest_move, chest_slot_index, chest_swap, commit_chest_slots, is_chest_slot, load_chest_slots,
};
