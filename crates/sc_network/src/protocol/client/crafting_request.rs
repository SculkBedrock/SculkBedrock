//! `ItemStackRequest`（0x93 = 147）边界类型（2168 协议档案）。
//!
//! `ItemStackRequest` (0x93 = 147) boundary type (2168 profile).
//!
//! 支持的动作子集：
//! - `Take`（0）/ `Place`（1）：数量 + 源槽位 + 目标槽位。
//! - `Swap`（2）：源槽位 + 目标槽位。
//! - `CraftRecipe`（10）/ `CraftRecipeAuto`（11）：配方网络 ID + 请求合成次数。
//! - 其他任何动作类型：载荷形态未实现，整个包以有界错误被拒绝（不猜测、
//!   绝不 panic），记录后跳过。
//!
//! 畸形缓冲区返回 `Err`，绝不 panic。
//!
//! Supported action subset:
//! - `Take` (0) / `Place` (1): amount + source slot + destination slot.
//! - `Swap` (2): source slot + destination slot.
//! - `CraftRecipe` (10) / `CraftRecipeAuto` (11): recipe network id +
//!   requested craft count.
//! - Any other action type: the payload shape is unimplemented, so the whole
//!   packet is rejected with a bounded error (never guessed, never panics);
//!   such requests are logged and skipped.
//!
//! Malformed buffers return `Err`, never panic.

use std::io::{Error, ErrorKind};

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;

/// 物品栏请求包的线上协议 ID。
///
/// The on-the-wire protocol id of the item stack request packet.
pub const ITEM_STACK_REQUEST_PACKET_ID: u16 = 147;

/// 物品栏请求动作类型常量。
///
/// Item stack request action type constants.
#[allow(non_snake_case)]
pub mod ItemStackRequestActionType {
    pub const TAKE: u8 = 0;
    pub const PLACE: u8 = 1;
    pub const SWAP: u8 = 2;
    pub const DROP: u8 = 3;
    pub const DESTROY: u8 = 4;
    pub const CONSUME: u8 = 5;
    pub const CREATE: u8 = 6;
    // Action type ids: 7 LAB_COMBINE, 8 BEACON_PAYMENT, 9 MINE_BLOCK.
    pub const LAB_TABLE_COMBINE: u8 = 7;
    pub const BEACON_PAYMENT: u8 = 8;
    pub const MINE_BLOCK: u8 = 9;
    pub const CRAFT_RECIPE: u8 = 10;
    pub const CRAFT_RECIPE_AUTO: u8 = 11;
    pub const CRAFT_CREATIVE: u8 = 12;
    pub const CRAFT_RECIPE_OPTIONAL: u8 = 13;
    /// 索引 13 在协议 2225 及之后承载 CraftReservedAction。
    ///
    /// Index 13 carries CraftReservedAction on protocol 2225 and later.
    pub const CRAFT_RESERVED_ACTION: u8 = 13;
    pub const CRAFT_REPAIR: u8 = 14;
    pub const CRAFT_LOOM: u8 = 15;
    pub const CRAFT_NON_IMPLEMENTED: u8 = 16;
    pub const CRAFT_RESULTS: u8 = 17;
}

/// 槽位中的容器引用（对合成逻辑不透明，仅用于保持流对齐——服务端绝不
/// 信任客户端的槽位路由）。
///
/// Container reference inside a slot (opaque to crafting; only consumed
/// for stream alignment — the server never trusts client slot routing).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestSlotRef {
    /// 容器枚举值。
    ///
    /// The container enumeration value.
    pub container_enum: u8,
    /// 动态槽位编号，仅当容器使用动态槽位时存在。
    ///
    /// The dynamic slot index; present only for containers using dynamic
    /// slots.
    pub dynamic_slot: Option<u32>,
    /// 静态槽位编号。
    ///
    /// The static slot index.
    pub slot: u8,
    /// 槽位的网络 ID。
    ///
    /// The network id of the slot.
    pub net_id: i32,
}

/// 读取一个槽位引用。
///
/// Reads one slot reference.
///
/// - `buf`: 输入字节读取器
///
/// # Errors
/// 缓冲区不足时返回 IO 错误。
fn read_slot_ref(buf: &mut ByteReader) -> Result<RequestSlotRef, Error> {
    let container_enum = buf.read_u8()?;
    let dynamic_slot = if buf.read_bool()? {
        Some(buf.read_u32_le()?)
    } else {
        None
    };
    let slot = buf.read_u8()?;
    let net_id = buf.read_i32_le()?;
    Ok(RequestSlotRef {
        container_enum,
        dynamic_slot,
        slot,
        net_id,
    })
}

/// 写入一个槽位引用。
///
/// Writes one slot reference.
///
/// - `buf`: 输出字节写入器
/// - `slot`: 待写入的槽位引用
///
/// # Errors
/// 写入失败时返回 IO 错误。
fn write_slot_ref(buf: &mut ByteWriter, slot: &RequestSlotRef) -> Result<(), Error> {
    buf.write_u8(slot.container_enum)?;
    match slot.dynamic_slot {
        Some(dynamic) => {
            buf.write_bool(true)?;
            buf.write_u32_le(dynamic)?;
        }
        None => buf.write_bool(false)?,
    }
    buf.write_u8(slot.slot)?;
    buf.write_i32_le(slot.net_id)?;
    Ok(())
}

/// 一个已解析的请求动作。
///
/// One parsed request action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemStackRequestAction {
    /// 合成结果动作。
    ///
    /// A craft results action.
    CraftResults {
        /// 合成次数。
        ///
        /// The number of crafts.
        times: u8,
    },
    /// 消耗动作：从源槽位扣除指定数量。
    ///
    /// A consume action: removes the given amount from the source slot.
    Consume {
        /// 消耗数量。
        ///
        /// The consumed amount.
        amount: u8,
        /// 源槽位引用。
        ///
        /// The source slot reference.
        source: RequestSlotRef,
    },
    /// 创建动作：在结果槽位生成物品。
    ///
    /// A create action: produces the item in the result slot.
    Create {
        /// 结果槽位编号。
        ///
        /// The result slot index.
        result_slot: u8,
    },
    /// 客户端侧移动（整理）。仅为对齐而解析；游戏依据权威多重集合成。
    ///
    /// Client-side move (arrange). Parsed for alignment only; the game
    /// crafts from the authoritative multiset instead.
    Move {
        /// 移动动作类型编号。
        ///
        /// The move action type ordinal.
        action_type: u8,
        /// 移动数量；`Swap` 动作为 `None`。
        ///
        /// The moved amount; `None` for the `Swap` action.
        amount: Option<u8>,
        /// 源槽位引用。
        ///
        /// The source slot reference.
        source: RequestSlotRef,
        /// 目标槽位引用。
        ///
        /// The destination slot reference.
        destination: RequestSlotRef,
    },
    /// 按配方合成动作。
    ///
    /// A craft-by-recipe action.
    CraftRecipe {
        /// 配方网络 ID。
        ///
        /// The recipe network id.
        recipe_network_id: u32,
        /// 合成次数。
        ///
        /// The number of crafts.
        times: u8,
    },
    /// 自动合成动作（客户端自动选择材料）。
    ///
    /// An auto craft action (the client selects the ingredients).
    CraftRecipeAuto {
        /// 配方网络 ID。
        ///
        /// The recipe network id.
        recipe_network_id: u32,
        /// 合成次数。
        ///
        /// The number of crafts.
        times: u8,
    },
    /// 创造模式取物（动作类型 12）：`creative_id` 是本服务端创造模式物品
    /// 列表中从 1 开始的条目 ID。末尾的载荷字节是不透明的（并非取出的
    /// 数量：整组取物仍携带 1）；它仅用于保持流对齐，实际发放的数量总是
    /// 来自该请求自身中消耗输出槽的移动动作。
    ///
    /// Creative-mode take (action type 12): `creative_id` is the 1-based
    /// entry id from this server's CreativeContent listing. The trailing
    /// payload byte is opaque (not the taken count: full-stack takes still
    /// carry 1); it is consumed for stream alignment only, and the granted
    /// amount always comes from the request's own Output-consuming Moves.
    CraftCreative {
        /// 创造模式条目 ID。
        ///
        /// The creative content entry id.
        creative_id: u32,
    },
    /// 挖掘方块消耗手持物品耐久（动作类型 9：SCREEN_HUD_MINE_BLOCK）。
    ///
    /// Breaking a block with tool durability loss (action type 9: SCREEN_HUD_MINE_BLOCK).
    MineBlock {
        hotbar_slot: i32,
        predicted_durability: i32,
        stack_network_id: i32,
    },
    /// 其他任何动作类型。其载荷形态未实现，因此无法安全对齐，整个包会被
    /// 拒绝。
    ///
    /// Any other action type. The payload shape is not implemented, so the
    /// request cannot be aligned safely and the packet is rejected.
    Unknown {
        /// 未知动作的类型编号。
        ///
        /// The type ordinal of the unknown action.
        action_type: u8,
    },
}

/// 一个客户端请求：请求 ID + 有界的动作列表。
///
/// One client request: id plus a bounded action list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemStackRequestEntry {
    /// 请求 ID。
    ///
    /// The request id.
    pub request_id: i32,
    /// 该请求携带的动作列表。
    ///
    /// The list of actions carried by this request.
    pub actions: Vec<ItemStackRequestAction>,
}

/// `ItemStackRequest` 包（客户端 → 服务端）。
///
/// `ItemStackRequest` packet (client → server).
#[derive(Clone, Debug, Eq, PartialEq, MinecraftPacket)]
pub struct ItemStackRequest {
    /// 本包携带的请求列表。
    ///
    /// The list of requests carried by this packet.
    pub requests: Vec<ItemStackRequestEntry>,
}

impl ItemStackRequest {
    /// 单个包允许的最大请求数。
    ///
    /// The maximum number of requests allowed in one packet.
    pub const MAX_REQUESTS: usize = 64;
    /// 单个请求允许的最大动作数。
    ///
    /// The maximum number of actions allowed in one request.
    pub const MAX_ACTIONS: usize = 64;

    /// 仅提取合成动作（配方网络 ID、次数）。供合成翻译器使用；客户端侧
    /// 移动会被跳过（服务端依据权威多重集合成，而不是重放客户端的槽位
    /// 移动）。
    ///
    /// Craft actions only (recipe network id, times). Used by the crafting
    /// translator; client-side moves are skipped (the server crafts from
    /// the authoritative multiset instead of replaying client slot moves).
    pub fn craft_actions(&self) -> Vec<(i32, u32, u8)> {
        let mut out = Vec::new();
        for request in &self.requests {
            for action in &request.actions {
                match action {
                    ItemStackRequestAction::CraftRecipe {
                        recipe_network_id,
                        times,
                    }
                    | ItemStackRequestAction::CraftRecipeAuto {
                        recipe_network_id,
                        times,
                    } => out.push((request.request_id, *recipe_network_id, *times)),
                    ItemStackRequestAction::Move { .. }
                    | ItemStackRequestAction::Consume { .. }
                    | ItemStackRequestAction::Create { .. }
                    | ItemStackRequestAction::CraftResults { .. }
                    | ItemStackRequestAction::CraftCreative { .. }
                    | ItemStackRequestAction::MineBlock { .. }
                    | ItemStackRequestAction::Unknown { .. } => {}
                }
            }
        }
        out
    }

    /// 是否存在携带合成动作的请求。
    ///
    /// Whether any request carries a craft action.
    pub fn has_craft(&self) -> bool {
        self.requests.iter().any(|request| {
            request.actions.iter().any(|action| {
                matches!(
                    action,
                    ItemStackRequestAction::CraftRecipe { .. }
                        | ItemStackRequestAction::CraftRecipeAuto { .. }
                )
            })
        })
    }
}

/// 读取一个移动类动作。
///
/// Reads one move-style action.
///
/// - `buf`: 输入字节读取器
/// - `action_type`: 该动作的类型编号
/// - `with_amount`: 该动作是否携带数量字段
///
/// # Errors
/// 缓冲区不足时返回 IO 错误。
fn read_move(
    buf: &mut ByteReader,
    action_type: u8,
    with_amount: bool,
) -> Result<ItemStackRequestAction, Error> {
    let amount = if with_amount {
        Some(buf.read_u8()?)
    } else {
        None
    };
    let source = read_slot_ref(buf)?;
    let destination = read_slot_ref(buf)?;
    Ok(ItemStackRequestAction::Move {
        action_type,
        amount,
        source,
        destination,
    })
}

/// 读取一个请求动作，按动作类型分派到对应的载荷解析。
///
/// Reads one request action, dispatching to the matching payload parser by
/// action type.
///
/// - `buf`: 输入字节读取器
///
/// # Errors
/// 缓冲区不足、动作类型非法、条目数量超限或遇到未实现的动作类型时返回 IO
/// 错误。
fn read_action(buf: &mut ByteReader) -> Result<ItemStackRequestAction, Error> {
    let action_type = u8::try_from(buf.read_var_u32()?)
        .map_err(|_| Error::new(ErrorKind::InvalidData, "invalid action type"))?;
    let _legacy_type = buf.read_u8()?;
    match action_type {
        ItemStackRequestActionType::TAKE | ItemStackRequestActionType::PLACE => {
            read_move(buf, action_type, true)
        }
        ItemStackRequestActionType::SWAP => read_move(buf, action_type, false),
        ItemStackRequestActionType::CONSUME => Ok(ItemStackRequestAction::Consume {
            amount: buf.read_u8()?,
            source: read_slot_ref(buf)?,
        }),
        ItemStackRequestActionType::CREATE => Ok(ItemStackRequestAction::Create {
            result_slot: buf.read_u8()?,
        }),
        // Unsupported actions consume their payload first, then classify as
        // Unknown (keeping the stream aligned); the caller replies ERROR so the
        // client rolls back its prediction instead of stalling.
        ItemStackRequestActionType::DROP => {
            buf.read_u8()?; // amount
            read_slot_ref(buf)?;
            buf.read_bool()?;
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::DESTROY => {
            buf.read_u8()?; // amount
            read_slot_ref(buf)?;
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::LAB_TABLE_COMBINE => {
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::BEACON_PAYMENT => {
            buf.read_var_i32()?;
            buf.read_var_i32()?;
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::MINE_BLOCK => {
            let hotbar_slot = buf.read_var_i32()?;
            let predicted_durability = buf.read_var_i32()?;
            let stack_network_id = buf.read_i32_le()?;
            Ok(ItemStackRequestAction::MineBlock {
                hotbar_slot,
                predicted_durability,
                stack_network_id,
            })
        }
        ItemStackRequestActionType::CRAFT_CREATIVE => {
            let creative_id = buf.read_var_u32()?;
            buf.read_u8()?; // opaque trailing byte, alignment only
            Ok(ItemStackRequestAction::CraftCreative { creative_id })
        }
        ItemStackRequestActionType::CRAFT_RECIPE_OPTIONAL => {
            if crate::protocol::version::protocol_at_least(
                crate::protocol::version::PROTOCOL_VERSION_1_26_60,
            ) {
                // Protocol 2225 and later: CraftReservedAction.
                buf.read_string()?;
                buf.read_u8()?;
            } else {
                buf.read_var_u32()?;
                buf.read_i32_le()?;
            }
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::CRAFT_REPAIR => {
            buf.read_i32_le()?; // recipe network id (intLE)
            buf.read_u8()?;
            buf.read_var_i32()?; // repair cost
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::CRAFT_LOOM => {
            buf.read_string()?;
            buf.read_u8()?;
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::CRAFT_NON_IMPLEMENTED => {
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::CRAFT_RESULTS => {
            let count = buf.read_var_u32()?;
            if count > 9 {
                return Err(Error::new(ErrorKind::InvalidData, "too many craft results"));
            }
            for _ in 0..count {
                read_request_ingredient(buf)?;
                buf.read_var_u32()?; // block runtime id
                let bytes = buf.read_var_u32()? as usize;
                if bytes > 65536 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "craft userdata too large",
                    ));
                }
                buf.read_bytes(bytes)?;
            }
            Ok(ItemStackRequestAction::CraftResults {
                times: buf.read_u8()?,
            })
        }
        ItemStackRequestActionType::CRAFT_RECIPE => {
            let recipe_network_id = buf.read_var_u32()?;
            let times = buf.read_u8()?;
            Ok(ItemStackRequestAction::CraftRecipe {
                recipe_network_id,
                times,
            })
        }
        ItemStackRequestActionType::CRAFT_RECIPE_AUTO => {
            let recipe_network_id = buf.read_var_u32()?;
            let times = buf.read_u8()?;
            let count = buf.read_var_u32()?;
            if count > 9 {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "too many auto craft ingredients",
                ));
            }
            for _ in 0..count {
                read_request_ingredient(buf)?;
            }
            Ok(ItemStackRequestAction::CraftRecipeAuto {
                recipe_network_id,
                times,
            })
        }
        other => {
            // Payload shape unimplemented: the request cannot be safely aligned,
            // so the whole packet is rejected (caller logs and skips). Never
            // silent, never panics.
            Err(Error::new(
                ErrorKind::Unsupported,
                format!("unsupported item stack action {other}"),
            ))
        }
    }
}

impl Reader<ItemStackRequest> for ItemStackRequest {
    /// 从字节流中解码一个物品栏请求包。
    ///
    /// Decodes an item stack request packet from a byte stream.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足或请求数量超过上限时返回 IO 错误。
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let count = buf.read_var_u32()? as usize;
        if count > Self::MAX_REQUESTS {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "too many item stack requests",
            ));
        }
        let mut requests = Vec::with_capacity(count);
        for _ in 0..count {
            requests.push(ItemStackRequestEntry::read_entry(buf)?);
        }
        Ok(Self { requests })
    }
}

impl ItemStackRequestEntry {
    /// 从字节流中读取一个请求条目。
    ///
    /// Reads one request entry from a byte stream.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足、动作数量或过滤字符串数量超限时返回 IO 错误。
    pub fn read_entry(buf: &mut ByteReader) -> Result<Self, Error> {
        let request_id = buf.read_var_i32()?;
        let action_count = buf.read_var_u32()? as usize;
        if action_count > ItemStackRequest::MAX_ACTIONS {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "too many item stack request actions",
            ));
        }
        let mut actions = Vec::with_capacity(action_count);
        for _ in 0..action_count {
            actions.push(read_action(buf)?);
            // Every known action (including those normalized to Unknown:
            // DROP/DESTROY/BEACON/MINE/CREATIVE/OPTIONAL/REPAIR/LOOM) consumes
            // its standard-length payload, so later actions stay aligned. Only
            // unknown action types error out (packet logged and skipped; the
            // connection is kept).
        }
        let filters = buf.read_var_u32()?;
        if filters > 16 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "too many filter strings",
            ));
        }
        for _ in 0..filters {
            let text = buf.read_string()?;
            if text.len() > 1024 {
                return Err(Error::new(ErrorKind::InvalidData, "filter string too long"));
            }
        }
        let _origin = buf.read_i32_le()?;
        Ok(Self {
            request_id,
            actions,
        })
    }
}

/// 读取一个请求材料描述符并跳过其载荷。
///
/// Reads one request ingredient descriptor and skips its payload.
///
/// - `buf`: 输入字节读取器
///
/// # Errors
/// 缓冲区不足或遇到不支持的材料描述符类型时返回 IO 错误。
fn read_request_ingredient(buf: &mut ByteReader) -> Result<(), Error> {
    let kind = buf.read_var_u32()?;
    let _legacy = buf.read_u8()?;
    match kind {
        0 => {}
        1 => {
            buf.read_string()?;
            buf.read_var_i32()?;
        }
        2 => {
            buf.read_string()?;
            buf.read_i16_le()?;
        }
        3 => {
            buf.read_string()?;
        }
        _ => {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "unsupported auto craft descriptor",
            ))
        }
    }
    buf.read_i16_le()?;
    Ok(())
}

/// 写入动作类型编号与对应的旧版类型字节。
///
/// Writes the action type ordinal and its legacy type byte.
///
/// - `buf`: 输出字节写入器
/// - `kind`: 待写入的动作类型编号
///
/// # Errors
/// 写入失败时返回 IO 错误。
fn write_action_type(buf: &mut ByteWriter, kind: u8) -> Result<(), Error> {
    // Legacy type mapping: TYPE 0..6 keep their values, TYPE 7..17 map to legacy
    // 9..19 (legacy 7/8 were deprecated container moves). This writer is test-only.
    buf.write_var_u32(kind as u32)?;
    buf.write_u8(if kind >= 7 { kind + 2 } else { kind })
}

impl Writer for ItemStackRequest {
    /// 将物品栏请求包编码写入字节流。
    ///
    /// Encodes the item stack request packet into a byte stream.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 缓冲区不足或遇到无法编码的未知动作时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(self.requests.len() as u32)?;
        for request in &self.requests {
            buf.write_var_i32(request.request_id)?;
            buf.write_var_u32(request.actions.len() as u32)?;
            for action in &request.actions {
                match action {
                    ItemStackRequestAction::CraftResults { times } => {
                        write_action_type(buf, 17)?;
                        buf.write_var_u32(0)?;
                        buf.write_u8(*times)?;
                    }
                    ItemStackRequestAction::Consume { amount, source } => {
                        write_action_type(buf, ItemStackRequestActionType::CONSUME)?;
                        buf.write_u8(*amount)?;
                        write_slot_ref(buf, source)?;
                    }
                    ItemStackRequestAction::Create { result_slot } => {
                        write_action_type(buf, ItemStackRequestActionType::CREATE)?;
                        buf.write_u8(*result_slot)?;
                    }
                    ItemStackRequestAction::Move {
                        action_type,
                        amount,
                        source,
                        destination,
                    } => {
                        write_action_type(buf, *action_type)?;
                        if *action_type == ItemStackRequestActionType::TAKE
                            || *action_type == ItemStackRequestActionType::PLACE
                        {
                            buf.write_u8(amount.unwrap_or(0))?;
                        }
                        write_slot_ref(buf, source)?;
                        write_slot_ref(buf, destination)?;
                    }
                    ItemStackRequestAction::CraftRecipe {
                        recipe_network_id,
                        times,
                    } => {
                        write_action_type(buf, ItemStackRequestActionType::CRAFT_RECIPE)?;
                        buf.write_var_u32(*recipe_network_id)?;
                        buf.write_u8(*times)?;
                    }
                    ItemStackRequestAction::CraftRecipeAuto {
                        recipe_network_id,
                        times,
                    } => {
                        write_action_type(buf, ItemStackRequestActionType::CRAFT_RECIPE_AUTO)?;
                        buf.write_var_u32(*recipe_network_id)?;
                        buf.write_u8(*times)?;
                        buf.write_var_u32(0)?; // auto-ingredient selection, no descriptor prompt
                    }
                    ItemStackRequestAction::Unknown { action_type } => {
                        if *action_type != 16 {
                            return Err(Error::new(
                                ErrorKind::Unsupported,
                                "unknown action cannot be encoded",
                            ));
                        }
                        write_action_type(buf, 16)?;
                    }
                    ItemStackRequestAction::CraftCreative { creative_id } => {
                        write_action_type(buf, ItemStackRequestActionType::CRAFT_CREATIVE)?;
                        buf.write_var_u32(*creative_id)?;
                        buf.write_u8(0)?;
                    }
                    ItemStackRequestAction::MineBlock {
                        hotbar_slot,
                        predicted_durability,
                        stack_network_id,
                    } => {
                        write_action_type(buf, ItemStackRequestActionType::MINE_BLOCK)?;
                        buf.write_var_i32(*hotbar_slot)?;
                        buf.write_var_i32(*predicted_durability)?;
                        buf.write_i32_le(*stack_network_id)?;
                    }
                }
            }
            buf.write_var_u32(0)?; // filter string count
            buf.write_i32_le(-1)?; // unknown text origin
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_binary::interfaces::{Reader, Writer};
    use sc_binary::{ByteReader, ByteWriter};

    #[test]
    fn vanilla_take_bytes_decode_to_move() {
        // Hand-built wire bytes (not via this Writer): single request -1,
        // Take{1, (29/slot10/net0) -> (59/slot0/net0)}, 25 bytes total.
        // request(-1)=01 actions(1)=01 type(00)+legacy(00) count(01)
        // src(1D 00 0A 00000000) dst(3B 00 00 00000000) filters(00) origin(FFFFFFFF).
        let bytes: [u8; 25] = [
            1, 1, 1, 0, 0, 1, 0x1d, 0, 10, 0, 0, 0, 0, 0x3b, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff,
            0xff,
        ];
        let mut reader = ByteReader::from(&bytes[..]);
        let request = ItemStackRequest::read(&mut reader).unwrap();
        assert!(reader.as_slice().is_empty());
        assert_eq!(request.requests.len(), 1);
        assert_eq!(request.requests[0].request_id, -1);
        assert!(matches!(
            &request.requests[0].actions[..],
            [ItemStackRequestAction::Move {
                action_type: 0,
                amount: Some(1),
                source,
                destination,
            }] if source.container_enum == 29
                && source.slot == 10
                && source.net_id == 0
                && destination.container_enum == 59
                && destination.slot == 0
        ));
        assert!(!request.has_craft());
    }

    #[test]
    fn codec_2168_craft_consume_golden_and_every_truncated_prefix() {
        // One signed request (-1), two actions: new type + legacy type,
        // recipe id=2, count=2, then consume from hotbar (container 28).
        let bytes = [
            1, 1, 2, 10, 12, 2, 2, 5, 5, 2, 28, 0, 3, 0, 0, 0, 0, 0, 255, 255, 255, 255,
        ];
        let mut reader = ByteReader::from(&bytes[..]);
        let request = ItemStackRequest::read(&mut reader).unwrap();
        assert_eq!(request.craft_actions(), vec![(-1, 2, 2)]);
        assert!(
            matches!(&request.requests[0].actions[1], ItemStackRequestAction::Consume { source, amount: 2 } if source.slot == 3 && source.net_id == 0)
        );
        for end in 0..bytes.len() {
            assert!(
                ItemStackRequest::read(&mut ByteReader::from(&bytes[..end])).is_err(),
                "prefix {end}"
            );
        }
        let mut encoded = ByteWriter::new();
        request.write(&mut encoded).unwrap();
        assert_eq!(encoded.as_slice(), bytes);
    }

    #[test]
    fn auto_craft_descriptors_and_filter_tail_keep_multiple_requests_aligned() {
        let mut bytes = ByteWriter::new();
        bytes.write_var_u32(2).unwrap();
        bytes.write_var_i32(-4).unwrap();
        bytes.write_var_u32(1).unwrap();
        write_action_type(&mut bytes, 11).unwrap();
        bytes.write_var_u32(2).unwrap();
        bytes.write_u8(3).unwrap();
        bytes.write_var_u32(1).unwrap();
        bytes.write_var_u32(1).unwrap();
        bytes.write_u8(1).unwrap();
        bytes.write_string("minecraft:oak_log").unwrap();
        bytes.write_var_i32(32767).unwrap();
        bytes.write_i16_le(1).unwrap();
        bytes.write_var_u32(1).unwrap();
        bytes.write_string("filter").unwrap();
        bytes.write_i32_le(-1).unwrap();
        bytes.write_var_i32(-5).unwrap();
        bytes.write_var_u32(1).unwrap();
        write_action_type(&mut bytes, 10).unwrap();
        bytes.write_var_u32(3).unwrap();
        bytes.write_u8(1).unwrap();
        bytes.write_var_u32(0).unwrap();
        bytes.write_i32_le(-1).unwrap();
        let parsed = ItemStackRequest::read(&mut ByteReader::from(bytes.as_slice())).unwrap();
        assert_eq!(parsed.craft_actions(), vec![(-4, 2, 3), (-5, 3, 1)]);
    }

    #[test]
    fn craft_recipe_round_trip() {
        let packet = ItemStackRequest {
            requests: vec![ItemStackRequestEntry {
                request_id: 5,
                actions: vec![
                    ItemStackRequestAction::CraftRecipe {
                        recipe_network_id: 9,
                        times: 2,
                    },
                    ItemStackRequestAction::CraftRecipeAuto {
                        recipe_network_id: 10,
                        times: 1,
                    },
                ],
            }],
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = ItemStackRequest::read(&mut reader).unwrap();
        assert_eq!(decoded, packet);
        assert_eq!(decoded.craft_actions().len(), 2);
    }

    #[test]
    fn malformed_request_returns_error_without_panic() {
        // Truncated varint.
        let mut reader = ByteReader::from(&[0xFF, 0xFF, 0xFF][..]);
        assert!(ItemStackRequest::read(&mut reader).is_err());
        // Declared count exceeds the bound.
        let mut writer = ByteWriter::new();
        writer.write_var_u32(10_000).unwrap();
        let mut reader = ByteReader::from(writer.as_slice());
        assert!(ItemStackRequest::read(&mut reader).is_err());
        // Empty buffer.
        let mut reader = ByteReader::from(&[][..]);
        assert!(ItemStackRequest::read(&mut reader).is_err());
    }

    #[test]
    fn unknown_action_does_not_panic_and_yields_no_craft() {
        let mut writer = ByteWriter::new();
        writer.write_var_u32(1).unwrap();
        writer.write_var_i32(1).unwrap();
        writer.write_var_u32(1).unwrap();
        writer.write_u8(3).unwrap(); // Drop (unsupported payload shape here)
        let mut reader = ByteReader::from(writer.as_slice());
        assert!(ItemStackRequest::read(&mut reader).is_err());
    }

    #[test]
    fn mixed_take_place_then_craft_stays_aligned() {
        // Real clients arrange (Take/Place) then execute (CraftRecipe) in
        // one request. Moves are parsed for alignment; only the craft
        // yields an intent.
        let slot = |container: u8, slot: u8| RequestSlotRef {
            container_enum: container,
            dynamic_slot: None,
            slot,
            net_id: 0,
        };
        let packet = ItemStackRequest {
            requests: vec![ItemStackRequestEntry {
                request_id: 7,
                actions: vec![
                    ItemStackRequestAction::Move {
                        action_type: ItemStackRequestActionType::TAKE,
                        amount: Some(2),
                        source: slot(0, 5),
                        destination: slot(1, 0),
                    },
                    ItemStackRequestAction::Move {
                        action_type: ItemStackRequestActionType::PLACE,
                        amount: Some(2),
                        source: slot(1, 0),
                        destination: slot(0, 6),
                    },
                    ItemStackRequestAction::CraftRecipeAuto {
                        recipe_network_id: 12,
                        times: 1,
                    },
                ],
            }],
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = ItemStackRequest::read(&mut reader).unwrap();
        assert_eq!(decoded, packet);
        assert!(decoded.has_craft());
        assert_eq!(decoded.craft_actions(), vec![(7, 12, 1)]);
    }

    #[test]
    fn live_creative_take_bytes_decode_to_grant_flow() {
        // Golden bytes from a real creative-mode take (oak planks x64):
        // [CraftCreative(id=1) + CraftResults + Move{64, Output(60/slot50)
        // -> Hotbar(28/slot0)}]. Previously decoded as Unknown and the
        // whole request was rejected, so creative takes never landed.
        let bytes: [u8; 75] = [
            0x01, 0x05, 0x03, 0x0c, 0x0e, 0x01, 0x01, 0x11, 0x13, 0x01, 0x01, 0x01, 0x14, 0x6d,
            0x69, 0x6e, 0x65, 0x63, 0x72, 0x61, 0x66, 0x74, 0x3a, 0x6f, 0x61, 0x6b, 0x5f, 0x70,
            0x6c, 0x61, 0x6e, 0x6b, 0x73, 0x00, 0x01, 0x00, 0xb6, 0xb5, 0xac, 0x94, 0x07, 0x0a,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x40,
            0x3c, 0x00, 0x32, 0xfd, 0xff, 0xff, 0xff, 0x1c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0xff, 0xff, 0xff, 0xff,
        ];
        let mut reader = ByteReader::from(&bytes[..]);
        let request = ItemStackRequest::read(&mut reader).unwrap();
        assert!(reader.as_slice().is_empty());
        assert_eq!(request.requests.len(), 1);
        assert_eq!(request.requests[0].request_id, -3);
        assert!(matches!(
            &request.requests[0].actions[..],
            [
                ItemStackRequestAction::CraftCreative { creative_id: 1 },
                ItemStackRequestAction::CraftResults { times: 1 },
                ItemStackRequestAction::Move {
                    action_type: 1,
                    amount: Some(64),
                    source,
                    destination,
                }
            ] if source.container_enum == 60
                && source.slot == 50
                && destination.container_enum == 28
                && destination.slot == 0
        ));
        assert!(!request.has_craft());
    }
    #[test]
    fn mine_block_action_round_trip() {
        let request = ItemStackRequest {
            requests: vec![ItemStackRequestEntry {
                request_id: -42,
                actions: vec![ItemStackRequestAction::MineBlock {
                    hotbar_slot: 3,
                    predicted_durability: 12,
                    stack_network_id: 100,
                }],
            }],
        };
        let mut writer = ByteWriter::new();
        request.write(&mut writer).unwrap();
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = ItemStackRequest::read(&mut reader).unwrap();
        assert_eq!(request, decoded);
    }
}
