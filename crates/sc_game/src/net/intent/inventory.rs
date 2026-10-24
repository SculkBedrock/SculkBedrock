//! 玩家背包同步 intent（权威槽位状态）。
//!
//! Player inventory sync intents (authoritative slot state).

use sc_game_macros::GameIntent;
use sc_item::ItemStack;

/// 同步玩家主背包槽位；客户端无需打开背包即可看到新物品；按 id 路由到
/// 玩家实体。
///
/// Sync player main-inventory slots; clients see new items without
/// opening the inventory; routed to the player entity by id.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct UpdateInventorySlot {
    pub entity_id: u64,
    pub slot: u8,
    pub stack: ItemStack,
}

/// 同步玩家手持物品（权威手持副本）。
///
/// Sync the player's held item (authoritative held copy).
///
/// 背包槽位同步（[`UpdateInventorySlot`]）只更新背包容器；手持物品在客户端还有一份
/// 独立副本（登录时由 `MobEquipment` 建立）。耐久这类**同一物品状态变化**必须同时
/// 刷新这份副本，否则客户端手部与快捷栏的耐久条不会动。
///
/// [`UpdateInventorySlot`] only refreshes the inventory container. Clients keep a
/// separate copy of the held item (established by `MobEquipment` at login); an
/// in-place state change such as wear must refresh that copy too, or the
/// durability bar in hand and the hotbar never moves.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct SyncHeldItem {
    pub entity_id: u64,
    /// 物品栏槽位（主背包 0..=8 即快捷栏槽位）。
    ///
    /// Inventory slot (0..=8 of the main inventory are the hotbar slots).
    pub slot: u8,
    pub stack: ItemStack,
}

/// 权威玩家背包重同步（回滚客户端预测）。游戏域从不接触协议细节。
///
/// Authoritative player inventory resync (rolls back client prediction).
/// The game domain never touches protocol details.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct ResyncInventory {
    pub entity_id: u64,
}

/// 打开玩家背包窗口。
///
/// Opens the player inventory window.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct OpenPlayerInventory {
    pub entity_id: u64,
}

/// 关闭玩家背包窗口。
///
/// Closes the player inventory window.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct ClosePlayerInventory {
    pub entity_id: u64,
    pub window_id: u8,
    pub container_type: i8,
}

/// 权威熔炉窗口槽位（输入/燃料/产物）；按 id 路由给观看的玩家。
///
/// Authoritative furnace window slots (input/fuel/result); routed to the
/// viewing player by id.
///
/// 网络层会用工作站的容器枚举把它展开为逐槽位包。
/// The network layer expands it to per-slot packets with the station's
/// container enums.
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

/// 一个打开窗口的熔炉进度数值（烹饪/点燃）；按 id 路由给观看的玩家。
///
/// Furnace progress values (cook/lit) for one open window; routed to the
/// viewing player by id.
///
/// 可覆盖状态：数值每 tick 重新生成，因此 outbox 受压时会丢弃它们
/// （参见 backpressure），下一 tick 重发。网络层把它展开为三个
/// `ContainerSetData` 包（属性 0/1/2）。
/// Overwritable state: values regenerate every tick, so a pressured outbox
/// drops them (see backpressure) and the next tick re-sends. The network
/// layer expands it to three `ContainerSetData` packets (properties 0/1/2).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "entity_id")]
pub struct SyncFurnaceProgress {
    pub entity_id: u64,
    pub window_id: u8,
    pub cook_ticks: u16,
    pub lit_ticks: u16,
    pub lit_duration: u16,
}

/// 权威箱子窗口槽位（27 或 54 格）；按 id 路由给观看的玩家。
///
/// Authoritative chest window slots (27 or 54 items); routed to the viewing
/// player by id.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct SyncChestWindow {
    pub entity_id: u64,
    pub window_id: u8,
    pub slots: Vec<ItemStack>,
}

/// 打开容器窗口（箱子、木桶等）；按 id 路由给观看的玩家。
///
/// Open a container window (chest, barrel, etc.); routed to the viewing player by id.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct OpenContainerWindow {
    pub entity_id: u64,
    pub window_id: u8,
    pub container_type: i8,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

