//! Crafting workstation intents.

use sc_game_macros::GameIntent;

#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct OpenCraftingStation {
    pub entity_id: u64,
    pub station: sc_recipe::StationKind,
    pub window_id: u8,
    /// Bedrock `ContainerType` byte, resolved from the workstation decl at
    /// open time (game owns the mapping source, network only encodes).
    pub container_type: i8,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct CraftResponse {
    pub slots: Vec<crate::inventory::craft_inventory::CraftSlotUpdate>,
    pub entity_id: u64,
    pub request_id: i32,
    pub success: bool,
}
