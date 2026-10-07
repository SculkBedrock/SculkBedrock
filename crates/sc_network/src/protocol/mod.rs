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

#[derive(Clone, Debug)]
pub struct ProtocolInfo {
    pub protocol_versions: MinecraftProtocolVersions,
    pub minecraft_versions: MinecraftVersions,
    /// Network protocol version string (e.g. "1.26.30"), written directly to
    /// online fields such as StartGame vanillaVersion. Kept in sync with
    /// minecraft_versions (the parsed MinecraftVersion) while preserving the
    /// original string to avoid round-trip loss.
    pub minecraft_network_version: String,
}

impl ProtocolInfo {
    /// Initialize the global protocol info (once per process; repeats return Err).
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

    pub fn global() -> Option<Arc<ProtocolInfo>> {
        GLOBAL_PROTOCOL_INFO.get().cloned()
    }
}

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
    PlayerArmorDamage(PlayerArmorDamage) = 0x95,
    UpdateAttributes(UpdateAttributes) = 0x1d,
    SetSpawnPosition(SetSpawnPosition) = 0x2b,
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

pub trait MinecraftPacket {
    fn send_event(self, world: &World, entity: EntityId, timestamp: u128);
    fn to_packets(self) -> MinecraftPackets;
}

impl MinecraftPackets {
    /// Packet id (e.g. 0x13): compile-time discriminant, no allocation.
    ///
    /// Generated by the `MinecraftPackets` derive macro from the explicit
    /// `#[repr(u16)]` discriminants (`packet_id()`). Used by packet
    /// intercept/observe (packet_hooks).
    ///
    /// Serializing a whole packet via `write_to_bytes()` just to read two
    /// bytes is expensive (a `LevelChunk` payload can reach 74KB). The send
    /// path also queries the id by concrete type via `packet_id_for_type`
    /// without building/cloning the large `MinecraftPackets` enum. Id/prefix
    /// equivalence is pinned by the cross-check test in `tests/packet_id.rs`.
    #[inline]
    pub fn id(&self) -> u16 {
        self.packet_id()
    }

    /// Variant name (e.g. `"LevelChunk"`): compile-time constant string.
    #[inline]
    pub fn variant_name(&self) -> &'static str {
        self.packet_name()
    }
}
