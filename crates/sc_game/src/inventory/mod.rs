//! 玩家背包与合成域。
//!
//! Player inventory and crafting domain.
//!
//! - [`container`]：每个玩家单一打开窗口的事实；
//!   [`container`]: the single open-window fact per player;
//! - [`craft_inventory`]：所有者本地的网格/光标事务；
//!   [`craft_inventory`]: owner-local grid/cursor transactions;
//! - [`crafting`]：服务端权威的配方执行与工作站规则。
//!   [`crafting`]: server-authoritative recipe execution and workstation rules.

pub mod chest;
pub mod container;
pub mod craft_inventory;
pub mod crafting;

pub use chest::{chest_viewer_count, open_chest_container};
pub use container::{ContainerKind, ContainerOpen, PLAYER_INVENTORY_WINDOW_ID};
pub use craft_inventory::{
    CraftContainer, CraftInventory, CraftSlotRef, CraftSlotUpdate, InventoryAction,
};
pub use crafting::{
    bump_inventory_revision, expected_from_slots, open_crafting_station, snapshot_from_sources,
    submit_craft, BlockProcessState, ContainerWindowAllocator, CraftInbox, CraftOutcome,
    CraftRequestEvent, CraftingRevision, CraftingSession, RecipeBookState, SharedItemTags,
    SharedRecipeRegistry,
};
