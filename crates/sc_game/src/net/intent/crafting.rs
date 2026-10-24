//! 合成工作站 intent。
//!
//! Crafting workstation intents.

use sc_game_macros::GameIntent;

/// 打开合成站点（可靠事实）。
///
/// Opens a crafting station (reliable fact).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct OpenCraftingStation {
    pub entity_id: u64,
    pub station: sc_recipe::StationKind,
    pub window_id: u8,
    /// Bedrock `ContainerType` 字节，打开时从工作站声明解析
    /// （游戏侧拥有映射来源，网络侧只负责编码）。
    ///
    /// Bedrock `ContainerType` byte, resolved from the workstation decl at
    /// open time (game owns the mapping source, network only encodes).
    pub container_type: i8,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// 合成请求的响应（槽位更新与成败）。
///
/// Response to a crafting request (slot updates and success flag).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct CraftResponse {
    pub slots: Vec<crate::inventory::craft_inventory::CraftSlotUpdate>,
    pub entity_id: u64,
    pub request_id: i32,
    pub success: bool,
}
