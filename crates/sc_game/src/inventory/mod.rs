//! Player inventory and crafting domain.
//!
//! - [`container`]: the single open-window fact per player;
//! - [`craft_inventory`]: owner-local grid/cursor transactions;
//! - [`crafting`]: server-authoritative recipe execution and workstation rules.

pub mod container;
pub mod craft_inventory;
pub mod crafting;

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
