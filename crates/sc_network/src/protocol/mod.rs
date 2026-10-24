//! Bedrock 协议层：包 ID 定义、协议版本协商与客户端包接收事件。
//!
//! Bedrock protocol layer: packet id definitions, protocol version
//! negotiation and client packet reception events.

pub mod client;
pub mod recv;
pub mod server;
pub mod version;

use crate::protocol::client::action::PlayerAction;
use crate::protocol::client::command::*;
use crate::protocol::client::container::ContainerClose;
use crate::protocol::client::crafting_request::ItemStackRequest;
use crate::protocol::client::handshake::*;
use crate::protocol::client::interact::*;
use crate::protocol::client::login::*;
use crate::protocol::client::movement::*;
use crate::protocol::client::resource_packs::*;
use crate::protocol::client::transaction::InventoryTransaction;
use crate::protocol::client::world::*;
use crate::protocol::client::*;
use crate::protocol::server::block::*;
use crate::protocol::server::chunk::*;
use crate::protocol::server::command::*;
use crate::protocol::server::crafting_response::ItemStackResponse;
use crate::protocol::server::creative::*;
use crate::protocol::server::effects::*;
use crate::protocol::server::entity::*;
use crate::protocol::server::game::*;
use crate::protocol::server::handshake::*;
use crate::protocol::server::inventory::*;
use crate::protocol::server::item::*;
use crate::protocol::server::item_entity::*;
use crate::protocol::server::login::*;
use crate::protocol::server::misc::*;
use crate::protocol::server::modern::*;
use crate::protocol::server::movement::*;
use crate::protocol::server::player::*;
use crate::protocol::server::resource_packs::*;
use crate::protocol::server::text::TextPacket;
use crate::protocol::server::world::*;
use sc_binary::BinaryIo;
use sc_ecs::entity::EntityId;
use sc_ecs::world::World;
use sc_network_macros::MinecraftPackets;
use sc_utils::game::structs::minecraft_version::MinecraftVersions;
use sc_utils::game::structs::protocol_versions::MinecraftProtocolVersions;
use std::sync::{Arc, OnceLock};

static GLOBAL_PROTOCOL_INFO: OnceLock<Arc<ProtocolInfo>> = OnceLock::new();

/// 进程级协议元信息：支持的协议版本、Minecraft 版本与网络版本字符串。
///
/// Process-wide protocol metadata: supported protocol versions, Minecraft
/// versions and the network version string.
#[derive(Clone, Debug)]
pub struct ProtocolInfo {
    /// 本服务端接受的协议版本集合。
    ///
    /// The set of protocol versions accepted by this server.
    pub protocol_versions: MinecraftProtocolVersions,
    /// 本服务端接受的 Minecraft 游戏版本集合。
    ///
    /// The set of Minecraft game versions accepted by this server.
    pub minecraft_versions: MinecraftVersions,
    /// 网络协议版本字符串（例如 "1.26.30"），直接写入 StartGame 等在线字段
    /// 的 vanillaVersion。它与 `minecraft_versions`（已解析的
    /// MinecraftVersion）保持同步，同时保留原始字符串以避免往返解析丢失。
    ///
    /// Network protocol version string (e.g. "1.26.30"), written directly to
    /// online fields such as StartGame vanillaVersion. Kept in sync with
    /// minecraft_versions (the parsed MinecraftVersion) while preserving the
    /// original string to avoid round-trip loss.
    pub minecraft_network_version: String,
}

impl ProtocolInfo {
    /// 初始化全局协议元信息（每进程仅一次；重复调用返回 `Err`）。
    ///
    /// Initializes the global protocol info (once per process; repeats return
    /// `Err`).
    ///
    /// - `protocol_versions`: 本服务端支持的协议版本集合
    /// - `minecraft_versions`: 本服务端支持的 Minecraft 版本集合
    /// - `minecraft_network_version`: 网络协议版本原始字符串
    pub fn new_global(
        protocol_versions: MinecraftProtocolVersions,
        minecraft_versions: MinecraftVersions,
        minecraft_network_version: String,
    ) -> Result<(), Arc<ProtocolInfo>> {
        GLOBAL_PROTOCOL_INFO.set(Arc::new(ProtocolInfo {
            protocol_versions,
            minecraft_versions,
            minecraft_network_version,
        }))
    }

    /// 读取已初始化的全局协议元信息；尚未初始化时返回 `None`。
    ///
    /// Reads the initialized global protocol info; returns `None` when it has
    /// not been initialized yet.
    pub fn global() -> Option<Arc<ProtocolInfo>> {
        GLOBAL_PROTOCOL_INFO.get().cloned()
    }
}

/// 全部 Bedrock 包 ID 枚举：枚举项的判别值即线上协议包头 ID。
///
/// The Bedrock packet id enum: each discriminant is the on-the-wire packet
/// header id.
#[derive(Clone, Debug, BinaryIo, MinecraftPackets)]
#[repr(u16)]
pub enum MinecraftPackets {
    Login(Login) = 0x01,
    PlayStatus(PlayStatus) = 0x02,
    ServerToClientHandshake(ServerToClientHandshake) = 0x03,
    ClientToServerHandshake(ClientToServerHandshake) = 0x04,
    Disconnect(Disconnect) = 0x05,
    TextPacket(TextPacket) = 0x09,
    ResourcePackInfo(ResourcePackInfo) = 0x06,
    ResourcePackStack(ResourcePackStack) = 0x07,
    ResourcePackClientResponse(ResourcePackClientResponse) = 0x08,
    ResourcePackDataInfo(ResourcePackDataInfo) = 0x52,
    ResourcePackChunkData(ResourcePackChunkData) = 0x53,
    ResourcePackChunkRequest(ResourcePackChunkRequest) = 0x54,
    StartGame(StartGame) = 0x0b,
    SetTime(SetTime) = 0x0a,
    SetEntityData(SetEntityData) = 0x27,
    CraftingData(CraftingData) = 0x34,
    AvailableCommands(AvailableCommands) = 0x4c,
    CommandRequest(CommandRequest) = 0x4d,
    CommandOutput(CommandOutput) = 0x4f,
    UpdateBlock(UpdateBlock) = 0x15,
    BlockEvent(BlockEvent) = 0x1a,
    PlayerAuthInput(PlayerAuthInput) = 0x90,
    PlayerAction(PlayerAction) = 0x24,
    InventoryTransaction(InventoryTransaction) = 0x1e,
    ItemStackRequest(ItemStackRequest) = 0x93,
    ItemStackResponse(ItemStackResponse) = 0x94,
    MobEquipment(MobEquipment) = 0x1f,
    Interact(Interact) = 0x21,
    ContainerOpen(ContainerOpen) = 0x2e,
    ContainerClose(ContainerClose) = 0x2f,
    InventoryContent(InventoryContent) = 0x31,
    InventorySlot(InventorySlot) = 0x32,
    ContainerSetData(ContainerSetData) = 0x33,
    PlayerArmorDamage(PlayerArmorDamage) = 0x95,
    UpdateAttributes(UpdateAttributes) = 0x1d,
    SetSpawnPosition(SetSpawnPosition) = 0x2b,
    Respawn(Respawn) = 0x2d,
    SetCommandsEnabled(SetCommandsEnabled) = 0x3b,
    SetDifficulty(SetDifficulty) = 0x3c,
    PlayerList(PlayerList) = 0x3f,
    AvailableEntityIdentifiers(AvailableEntityIdentifiers) = 0x77,
    GameRulesChanged(GameRulesChanged) = 0x48,
    BiomeDefinitionList(BiomeDefinitionList) = 0x7a,
    ClientCacheStatus(ClientCacheStatus) = 0x81,
    NetworkSettings(NetworkSettings) = 0x8f,
    CreativeContent(CreativeContent) = 0x91,
    ItemComponent(ItemComponent) = 0xa2,
    UpdateAbilities(UpdateAbilities) = 0xbb,
    UpdateAdventureSettings(UpdateAdventureSettings) = 0xbc,
    // SetPlayerGameType (0x3e); GameType ordinals are defined in game.rs.
    SetPlayerGameType(SetPlayerGameType) = 0x3e,
    MovePlayer(MovePlayer) = 0x13,
    MoveEntityAbsolute(MoveEntityAbsolute) = 0x12,
    SetEntityMotion(SetEntityMotion) = 0x28,
    RemoveEntity(RemoveEntity) = 0x0e,
    AddItemEntity(AddItemEntity) = 0x0f,
    // 0x10 is a deprecated slot in 2168; TakeItemEntity is 0x11.
    TakeItemEntity(TakeItemEntity) = 0x11,
    // ActorEvent (0x1b): syncs the stack count after dropped items merge.
    EntityEvent(EntityEvent) = 0x1b,
    LevelEvent(LevelEvent) = 0x19,
    // 0x3d is ChangeDimensionPacket in 2168; the string-sound LevelSoundEvent is 0x7b.
    LevelSoundEvent(LevelSoundEvent) = 0x7b,
    AddPlayer(AddPlayer) = 0x0c,
    RequestNetworkSettings(RequestNetworkSettings) = 0xc1,
    SetLocalPlayerAsInitialized(SetLocalPlayerAsInitialized) = 0x71,
    RequestChunkRadius(RequestChunkRadius) = 0x45,
    ServerboundLoadingScreen(ServerboundLoadingScreen) = 0x138,
    ChunkRadiusUpdated(ChunkRadiusUpdated) = 0x46,
    LevelChunk(LevelChunk) = 0x3a,
    NetworkChunkPublisherUpdate(NetworkChunkPublisherUpdate) = 0x79,
    SyncActorProperty(SyncActorProperty) = 0xa5,
    PlayerFog(PlayerFog) = 0xa0,
    CameraPresets(CameraPresets) = 0xc6,
    TrimData(TrimData) = 0x12e,
    CameraAimAssistPresets(CameraAimAssistPresets) = 0x140,
    ClientCameraAimAssist(ClientCameraAimAssist) = 0x141,
    VoxelShapes(VoxelShapes) = 0x151,
    SetPlayerFurnaceOptions(SetPlayerFurnaceOptions) = 0x15f,
    RecordStarted(RecordStarted) = 0x160,
    ClientboundMatchmakingState(ClientboundMatchmakingState) = 0x161,
    ServerboundStonecutterSetRecipe(ServerboundStonecutterSetRecipe) = 0x162,
    ClientboundStonecutterSetRecipe(ClientboundStonecutterSetRecipe) = 0x163,
    ServerboundMatchmakingCancel(ServerboundMatchmakingCancel) = 0x164,
    SetPassengerOfBlock(SetPassengerOfBlock) = 0x165,
    ServerboundCursorItemDrag(ServerboundCursorItemDrag) = 0x166,
    ClientboundPlayAudioContent(ClientboundPlayAudioContent) = 0x167,
    ServerboundRegisterAudioContent(ServerboundRegisterAudioContent) = 0x168,
}

/// 所有可收发协议包的统一接口。
///
/// The common interface implemented by every protocol packet.
pub trait MinecraftPacket {
    /// 将该包投递为事件：触发对应的事件处理流程。
    ///
    /// Dispatches the packet as an event, triggering the matching event
    /// handling flow.
    ///
    /// - `world`: 事件所属的世界
    /// - `entity`: 触发事件的实体 ID
    /// - `timestamp`: 事件产生的时间戳
    fn send_event(self, world: &World, entity: EntityId, timestamp: u128);
    /// 将具体包转换为统一的包枚举，便于统一分发。
    ///
    /// Converts the concrete packet into the unified packet enum for uniform
    /// dispatch.
    fn to_packets(self) -> MinecraftPackets;
}

impl MinecraftPackets {
    /// 包 ID（例如 0x13）：编译期判别值，无额外分配。
    ///
    /// 包 ID（例如 0x13）：编译期判别值，无内存分配。
    ///
    /// Packet id (e.g. 0x13): compile-time discriminant, no allocation.
    ///
    /// 由 `MinecraftPackets` derive 宏依据 `#[repr(u16)]` 显式判别值生成，
    /// 供包拦截/观察（packet_hooks）使用。
    ///
    /// Generated by the `MinecraftPackets` derive macro from the explicit
    /// `#[repr(u16)]` discriminants (`packet_id()`); used by packet
    /// intercept/observe (packet_hooks).
    #[inline]
    pub fn id(&self) -> u16 {
        self.packet_id()
    }

    /// 枚举项名称（例如 `"LevelChunk"`）：编译期常量字符串。
    ///
    /// Variant name (e.g. `"LevelChunk"`): compile-time constant string.
    #[inline]
    pub fn variant_name(&self) -> &'static str {
        self.packet_name()
    }
}
