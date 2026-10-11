//! Player inventory sync intents (authoritative slot state).

use sc_game_macros::GameIntent;
use sc_item::ItemStack;

/// Sync player main-inventory slots; clients see new items without
/// opening the inventory; routed to the player entity by id.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct UpdateInventorySlot {
    pub entity_id: u64,
    pub slot: u8,
    pub stack: ItemStack,
}

/// Authoritative player inventory resync (rolls back client prediction).
/// The game domain never touches protocol details.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct ResyncInventory {
    pub entity_id: u64,
}

#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct OpenPlayerInventory {
    pub entity_id: u64,
}

#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct ClosePlayerInventory {
    pub entity_id: u64,
    pub window_id: u8,
    pub container_type: i8,
}

/// Authoritative furnace window slots (input/fuel/result); routed to the
/// viewing player by id. The network layer expands it to per-slot packets
/// with the station's container enums.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct SyncFurnaceWindow {
    pub entity_id: u64,
    pub window_id: u8,
    pub station: sc_recipe::StationKind,
    pub input: ItemStack,
    pub fuel: ItemStack,
    pub result: ItemStack,
}

/// Furnace progress values (cook/lit) for one open window; routed to the
/// viewing player by id. Overwritable state: values regenerate every tick,
/// so a pressured outbox drops them (see backpressure) and the next tick
/// re-sends. The network layer expands it to three `ContainerSetData`
/// packets (properties 0/1/2).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "entity_id")]
pub struct SyncFurnaceProgress {
    pub entity_id: u64,
    pub window_id: u8,
    pub cook_ticks: u16,
    pub lit_ticks: u16,
    pub lit_duration: u16,
}
