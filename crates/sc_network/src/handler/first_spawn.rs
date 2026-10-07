//! First-spawn network phase.
//!
//! Once chunks reach `spawn_threshold`, sending `PLAYER_SPAWN` alone is not enough.
//! This phase also completes player state, spawn point, player list, and the
//! spawn teleport packet. Packet construction/sending stays independent of the
//! chunk pipeline, which only triggers this phase.

use log::debug;
use sc_ecs::entity::EntityId;
use sc_ecs::world::World;
use sc_entity::motion::{PhysicsBody, Transform};
use sc_entity::MinecraftEntityId;
use sc_item::PlayerInventory;
use sc_utils::components::DisplayName;
use sc_utils::game::client::MinecraftClient;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use std::sync::Arc;

use crate::client::MinecraftClientNetwork;
use crate::player_connection::{PlayerConnection, PlayerConnectionError, PlayerConnectionStatus};
use crate::protocol::client::transaction::ItemData;
use crate::protocol::server::inventory::{
    InventoryContent, InventorySlot, MobEquipment, PlayerArmorDamage,
};
use crate::protocol::server::login::PlayStatus;
use crate::protocol::server::misc::{
    CraftingData, PlayerList, PlayerListEntry, SetCommandsEnabled, SetSpawnPosition, SetTime,
};
use crate::protocol::server::movement::{MovePlayer, MovePlayerMode};

const OFFHAND_EQUIPMENT_SLOT: u8 = 1;

struct InventorySyncContext {
    connection: Arc<PlayerConnection>,
    runtime_entity_id: u64,
    inventory_items: Vec<ItemData>,
    selected_slot: u8,
    selected_item: ItemData,
}

/// Runs the first-spawn network phase (player state, lists, spawn teleport).
///
/// The function is called from the chunk sender after the threshold chunk has
/// been queued successfully. Every packet is sent sequentially through the
/// normal packet encoder and RakNet path; no raw packet bytes are constructed.
pub(crate) async fn send_first_spawn(
    world: &World,
    entity: EntityId,
    epoch: u64,
) -> Result<(), PlayerConnectionError> {
    let Some(connection) = world.get_component::<PlayerConnection>(&entity) else {
        return Err(PlayerConnectionError::RecvError(
            sc_raknet::connection::RecvError::Closed,
        ));
    };

    // The chunk pipeline owns the one-shot threshold transition. This guard
    // protects against an accidental second invocation from another system.
    if connection.get_status() != PlayerConnectionStatus::Initializing {
        return Ok(());
    }

    let Some(client) = world.get_component::<MinecraftClient>(&entity) else {
        return Ok(());
    };
    let Some(entity_id) = world.get_component::<MinecraftEntityId>(&entity) else {
        return Ok(());
    };
    let Some(world_id) = world.get_component::<MinecraftWorldId>(&entity) else {
        return Ok(());
    };
    let world_data = {
        let Some(world_manager) = world.get_resource::<MinecraftWorldManager>() else {
            return Ok(());
        };
        let Some(minecraft_world) = world_manager.get_world(world_id.as_ref()) else {
            return Ok(());
        };
        minecraft_world.world_data.clone()
    };
    let client_data = client.data.read().clone();

    // The recipe registry is sent from first spawn, after the client has
    // received the spawn chunks. Keep this packet out of BEFORE_SPAWN so its
    // ordering matches the chunk/first-spawn state machine.
    // Stage-1: send compiled, wire-expressible recipes from the shared
    // snapshot; fall back to the trim-only empty table when no registry is
    // loaded. Unexpressible recipes are never sent (see from_snapshot).
    connection
        .send_packet(crafting_data_for_connection(world), true)
        .await?;
    send_full_player_inventory(world, entity).await?;

    // Enables commands and synchronizes world time during first spawn.
    connection
        .send_packet(
            SetCommandsEnabled {
                enabled: world_data.commands_enabled,
            },
            true,
        )
        .await?;
    connection
        .send_packet(
            SetTime {
                time: world_data.daylight_cycle,
            },
            true,
        )
        .await?;

    // Update the client's compass/spawn position immediately before
    // PLAYER_SPAWN, keeping spawn-time packets ordered after it.
    let (spawn_x, spawn_y, spawn_z) = (
        client_data.position.x.floor() as i32,
        client_data.position.y.floor() as i32,
        client_data.position.z.floor() as i32,
    );
    connection
        .send_packet(
            SetSpawnPosition {
                spawn_type: SetSpawnPosition::TYPE_PLAYER_SPAWN,
                x: spawn_x,
                y: spawn_y,
                z: spawn_z,
                dimension: world_data.get_dimension(),
                spawn_block_position: None,
            },
            true,
        )
        .await?;

    // This is the one-shot state transition in the protocol handshake.
    let Some(view) = world.get_component::<sc_world::chunk_view::ChunkView>(&entity) else {
        return Err(PlayerConnectionError::PacketNotQueued(
            crate::player_connection::PacketSendOutcome::StaleContext,
        ));
    };
    let spawn_outcome = connection
        .send_packet_with_checked_outcome(
            PlayStatus {
                status: PlayStatus::PLAYER_SPAWN,
            },
            true,
            |plain, permit, trace| {
                let data = view.read();
                if data.epoch != epoch
                    || &data.world_id != world_id.as_ref()
                    || !data.has_spawn_chunks
                {
                    return Ok(crate::player_connection::PacketSendOutcome::StaleContext);
                }
                connection.admit_prepared(plain, true, permit, trace)?;
                connection.set_status(PlayerConnectionStatus::AwaitingClientInitialization);
                Ok(crate::player_connection::PacketSendOutcome::Queued)
            },
        )
        .await?;
    if spawn_outcome != crate::player_connection::PacketSendOutcome::Queued {
        return Err(PlayerConnectionError::PacketNotQueued(spawn_outcome));
    }

    // The login sequence already sent the full server-wide player list (including
    // the joiner, with full skins); resending it would be redundant. Only players
    // who joined after the login sequence are patched in, avoiding a repeated
    // 300KB+ skin dump; the packet is skipped when there is nothing new.
    let player_list = build_late_joining_player_list(world, entity);
    if player_list.entries.is_empty() {
        debug!(
            "Player({}) >> First spawn: player list unchanged, skip re-send",
            client_data.display_name
        );
    } else {
        connection.send_packet(player_list, true).await?;
    }

    // Teleport packets carry eye-height-adjusted coordinates.
    let (yaw, pitch, head_yaw) = world
        .get_component::<Transform>(&entity)
        .map(|transform| {
            let transform = transform.inner.read();
            (
                transform.rotation.yaw,
                transform.rotation.pitch,
                transform.rotation.yaw,
            )
        })
        .unwrap_or((0.0, 0.0, 0.0));
    let on_ground = world
        .get_component::<PhysicsBody>(&entity)
        .map(|body| body.on_ground)
        .unwrap_or(false);
    let base_offset = world
        .get_component::<PhysicsBody>(&entity)
        .map(|body| body.base_offset)
        .unwrap_or(1.62);

    let teleport_outcome = connection
        .send_packet_with_checked_outcome(
            MovePlayer {
                entity_id: entity_id.0,
                x: client_data.position.x,
                y: client_data.position.y + base_offset,
                z: client_data.position.z,
                pitch,
                yaw,
                head_yaw,
                mode: MovePlayerMode::Teleport,
                on_ground,
                riding_entity_id: 0,
                tick: 0,
            },
            true,
            |plain, permit, trace| {
                let data = view.read();
                if data.epoch != epoch || &data.world_id != world_id.as_ref() {
                    return Ok(crate::player_connection::PacketSendOutcome::StaleContext);
                }
                connection.admit_prepared(plain, true, permit, trace)?;
                Ok(crate::player_connection::PacketSendOutcome::Queued)
            },
        )
        .await?;
    if teleport_outcome != crate::player_connection::PacketSendOutcome::Queued {
        return Err(PlayerConnectionError::PacketNotQueued(teleport_outcome));
    }

    debug!(
        "Player({}) >> First spawn sequence complete; waiting for SetLocalPlayerAsInitialized",
        client_data.display_name
    );
    Ok(())
}

/// Equipment container state sent during the chunk phase.
///
/// Sent after a small number of chunks have reached the client. It
/// does not include the main inventory content; that belongs to the later full
/// sync at the spawn threshold.
pub(crate) async fn send_equipment_container_inventory(
    world: &World,
    entity: EntityId,
) -> Result<(), PlayerConnectionError> {
    let Some(ctx) = inventory_sync_context(world, entity)? else {
        return Ok(());
    };
    debug!("Player inventory sync: equipment containers");
    send_selected_hotbar(&ctx).await?;
    send_armor_cursor_offhand(&ctx).await
}

/// Full player inventory sync used immediately before PLAYER_SPAWN.
pub(crate) async fn send_full_player_inventory(
    world: &World,
    entity: EntityId,
) -> Result<(), PlayerConnectionError> {
    let Some(ctx) = inventory_sync_context(world, entity)? else {
        return Ok(());
    };
    debug!(
        "Player inventory sync: full inventory slots={} selected={}",
        ctx.inventory_items.len(),
        ctx.selected_slot
    );
    send_selected_hotbar(&ctx).await?;
    send_armor_cursor_offhand(&ctx).await?;
    ctx.connection
        .send_packet(
            InventoryContent::new(0, normalize_slots(ctx.inventory_items.clone(), 36)),
            true,
        )
        .await
}

fn inventory_sync_context(
    world: &World,
    entity: EntityId,
) -> Result<Option<InventorySyncContext>, PlayerConnectionError> {
    let Some(connection) = world.get_component::<PlayerConnection>(&entity) else {
        return Err(PlayerConnectionError::RecvError(
            sc_raknet::connection::RecvError::Closed,
        ));
    };
    let Some(entity_id) = world.get_component::<MinecraftEntityId>(&entity) else {
        return Ok(None);
    };
    let (inventory_items, selected_slot, selected_item) = world
        .get_component::<PlayerInventory>(&entity)
        .map(|inventory| {
            let items = inventory
                .snapshot()
                .into_iter()
                .map(item_stack_to_item_data)
                .collect::<Vec<_>>();
            let selected_slot = inventory.selected_slot();
            let selected_item = inventory
                .get(selected_slot as usize)
                .map(item_stack_to_item_data)
                .unwrap_or_default();
            (items, selected_slot, selected_item)
        })
        .unwrap_or_else(|| (vec![ItemData::default(); 36], 0, ItemData::default()));

    Ok(Some(InventorySyncContext {
        connection,
        runtime_entity_id: entity_id.0,
        inventory_items,
        selected_slot,
        selected_item,
    }))
}

async fn send_selected_hotbar(ctx: &InventorySyncContext) -> Result<(), PlayerConnectionError> {
    ctx.connection
        .send_packet(
            InventorySlot {
                full_container_name_id: None,
                full_container_dynamic_id: None,
                container_id: InventoryContent::SPECIAL_INVENTORY,
                slot: ctx.selected_slot,
                item: ctx.selected_item.clone(),
            },
            true,
        )
        .await?;

    ctx.connection
        .send_packet(
            MobEquipment {
                runtime_entity_id: ctx.runtime_entity_id,
                item: ctx.selected_item.clone(),
                hotbar_slot: ctx.selected_slot,
                inventory_slot: ctx.selected_slot,
                window_id: 0,
            },
            true,
        )
        .await
}

async fn send_armor_cursor_offhand(
    ctx: &InventorySyncContext,
) -> Result<(), PlayerConnectionError> {
    ctx.connection
        .send_packet(
            InventoryContent::new(
                InventoryContent::SPECIAL_ARMOR,
                vec![ItemData::default(); 4],
            ),
            true,
        )
        .await?;
    ctx.connection
        .send_packet(PlayerArmorDamage::empty(), true)
        .await?;
    ctx.connection
        .send_packet(
            InventorySlot {
                full_container_name_id: None,
                full_container_dynamic_id: None,
                container_id: InventoryContent::SPECIAL_CURSOR,
                slot: 0,
                item: ItemData::default(),
            },
            true,
        )
        .await?;
    ctx.connection
        .send_packet(
            InventoryContent::new(InventoryContent::SPECIAL_OFFHAND, vec![ItemData::default()]),
            true,
        )
        .await?;
    // Equipment is repeated after offhand sync using the offhand container
    // id. The slot value is the offhand equipment slot, not the selected
    // hotbar slot.
    ctx.connection
        .send_packet(
            MobEquipment {
                runtime_entity_id: ctx.runtime_entity_id,
                item: ItemData::default(),
                hotbar_slot: OFFHAND_EQUIPMENT_SLOT,
                inventory_slot: OFFHAND_EQUIPMENT_SLOT,
                window_id: InventoryContent::SPECIAL_OFFHAND,
            },
            true,
        )
        .await
}

fn build_late_joining_player_list(world: &World, joining_entity: EntityId) -> PlayerList {
    // The player list is server-wide (no per-world filtering;
    // AddPlayer is the same-world broadcast). The login sequence already sent the
    // full list, so the joiner is excluded here and only later joiners are collected.
    let mut entries = Vec::new();

    for entity in world.entities_with_component::<PlayerConnection>() {
        if entity == joining_entity {
            continue;
        }
        let Some(other_connection) = world.get_component::<PlayerConnection>(&entity) else {
            continue;
        };
        let status = other_connection.get_status();
        if !matches!(
            status,
            PlayerConnectionStatus::Initializing
                | PlayerConnectionStatus::AwaitingClientInitialization
                | PlayerConnectionStatus::InGame
                | PlayerConnectionStatus::Spawned
        ) {
            continue;
        }
        let Some(other_id) = world.get_component::<MinecraftEntityId>(&entity) else {
            continue;
        };
        let Some(other_name) = world.get_component::<DisplayName>(&entity) else {
            continue;
        };
        let data = other_connection.get_data();
        entries.push(PlayerListEntry {
            uuid: data.uuid,
            entity_id: other_id.0 as i64,
            name: other_name.0.clone(),
            xuid: data.xuid,
            platform_chat_id: String::new(),
            build_platform: data.device_os.index(),
            skin: data.skin,
            ..Default::default()
        });
    }

    PlayerList {
        list_type: PlayerList::TYPE_ADD,
        entries,
    }
}

fn item_stack_to_item_data(stack: sc_item::ItemStack) -> ItemData {
    ItemData {
        runtime_id: stack.runtime_id,
        count: stack.count,
        damage: stack.damage,
        // No fake ids before the net-id allocator lands (empty/untracked slots carry no net id).
        has_net_id: false,
        net_id: 0,
        block_runtime_id: stack.block_runtime_id,
        user_data: None,
    }
}

fn normalize_slots(mut items: Vec<ItemData>, slot_count: usize) -> Vec<ItemData> {
    items.truncate(slot_count);
    items.resize(slot_count, ItemData::default());
    items
}

/// Build the `CraftingData` packet from the shared recipe snapshot.
///
/// Falls back to the trim-only empty table when no registry or item table
/// is loaded. Brewing numeric ids have no stage-1 resolver, so brewing /
/// container / material-reducer recipes are reported disabled and skipped
/// (never sent with guessed ids).
pub(crate) fn crafting_data_for_connection(world: &World) -> CraftingData {
    let (Some(registry), Some(items)) = (
        world.get_resource::<sc_game::SharedRecipeRegistry>(),
        world.get_resource::<sc_item::ItemRegistry>(),
    ) else {
        return CraftingData::empty();
    };
    let snapshot = registry.snapshot();
    if snapshot.is_empty() {
        return CraftingData::empty();
    }
    let item_runtime = |id: &str| {
        let runtime = items.runtime_id_by_name(id)?;
        let def = items.get(runtime)?;
        Some((runtime, def.default_block_runtime_id.unwrap_or(0)))
    };
    // No numeric brewing-id table in stage-1: disable brewing loudly.
    let brewing_id = |_: &str| None;
    let (data, disabled) = CraftingData::from_snapshot(&snapshot, &item_runtime, &brewing_id);
    if !disabled.is_empty() {
        log::debug!(
            "CraftingData: {} recipes sent, {} disabled (first: {:?})",
            data.shaped.len() + data.shapeless.len() + data.smithing_transform.len(),
            disabled.len(),
            disabled.first()
        );
    }
    data
}
