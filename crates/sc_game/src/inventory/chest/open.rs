//! 箱子容器开启与会话创建。
//!
//! 处理右键交互、距离判定、权限检查、阻挡检查、双箱合并与规范定序、
//! 挂载 `ContainerOpen` 组件并下发网络同步数据包。
//!
//! Chest window opening and session management.
//!
//! Handles the right-click interaction, distance and permission checks,
//! obstruction checks, double-chest pairing with canonical ordering, and
//! attaches `ContainerOpen` before emitting the network sync packets.

use sc_block::position::BlockPosition;
use sc_ecs::entity::EntityId;
use sc_ecs::world::World;
use sc_world::manager::MinecraftWorldId;

use crate::inventory::chest::double_chest::{canonical_chest_order, chest_facing_at, find_paired_chest};
use crate::inventory::chest::obstruction::is_chest_obstructed;
use crate::inventory::chest::tx::load_chest_slots;
use crate::inventory::container::ContainerOpen;
use crate::inventory::crafting::ContainerWindowAllocator;
use crate::net::{
    BlockAnimationCue, IntentPublisher, NetworkOutbox, OpenContainerWindow, PendingConnectionFaults,
    PlayBlockAnimation, PlaySound, SoundCue, SyncChestWindow,
};

/// 尝试打开玩家交互的箱子方块。
///
/// 若成功打开或该方块确为箱子但条件不满足（如正上方阻挡、超出交互距离），返回 `true`（吞掉交互，不再触发放置方块）；
/// 若该方块非箱子，返回 `false`（允许后续系统处理放置或其他交互）。
///
/// Tries to open the chest block the player is interacting with. Returns
/// `true` when the chest opens or the block is a chest whose open conditions
/// fail (obstructed lid, out of interaction range), swallowing the interaction
/// so placement does not run; returns `false` when the block is not a chest so
/// later systems can handle placement or other interactions.
pub fn open_chest_container(
    world: &World,
    entity: EntityId,
    position: BlockPosition,
    outbox: &mut NetworkOutbox,
) -> bool {
    // A player may hold at most one container window at a time: return early
    // when one is already open.
    if let Some(open) = world.get_component::<ContainerOpen>(&entity) {
        log::debug!(
            "[chest] 窗口保持关闭：玩家已有打开的窗口 (window={} kind={})",
            open.window_id,
            open.kind_name()
        );
        return true;
    }

    let Some(world_id) = world.get_component::<MinecraftWorldId>(&entity) else {
        return false;
    };
    let pos = (position.x, position.y, position.z);

    // Look up the sc:chest declaration for this block.
    let Some(decl) = crate::inventory::crafting::chest_decl_at(world, &world_id, pos) else {
        return false;
    };

    // Check the player's OpenContainers permission.
    if !world
        .get_component::<sc_entity::player::adventure_settings::AdventureSettings>(&entity)
        .is_some_and(|settings| {
            settings.get(
                &sc_entity::player::adventure_settings::AdventureSettingsType::OpenContainers,
            )
        })
    {
        return false;
    }

    // Check the player's interaction distance (squared distance <= 36.0).
    let Some(transform) = world.get_component::<sc_entity::motion::Transform>(&entity) else {
        return true;
    };
    let feet = world
        .get_component::<sc_utils::game::client::MinecraftClient>(&entity)
        .map(|client| {
            let data = client.data.read();
            (data.position.x, data.position.y, data.position.z)
        })
        .unwrap_or_else(|| {
            let transform = transform.read();
            (
                transform.position.x,
                transform.position.y,
                transform.position.z,
            )
        });

    let dist_sq = (feet.0 - position.x as f32 - 0.5).powi(2)
        + (feet.1 - position.y as f32 - 0.5).powi(2)
        + (feet.2 - position.z as f32 - 0.5).powi(2);
    if dist_sq > 36.0 {
        log::debug!(
            "[chest] 箱子保持关闭：超出交互距离 {dist_sq:.1} (feet={feet:?}, block={pos:?})"
        );
        return true;
    }

    // Obstruction check.
    if decl.check_obstruction && is_chest_obstructed(world, &world_id, pos) {
        log::debug!("[chest] 箱子正上方被实心方块阻挡，无法打开: {pos:?}");
        return true;
    }

    // Double-chest pairing.
    let (left_pos, right_pos, total_slots) = if decl.pairable {
        if let Some(paired) = find_paired_chest(world, &world_id, pos) {
            // The paired half must also be unobstructed when obstruction
            // checks are enabled.
            let paired_check_obstruction = crate::inventory::crafting::chest_decl_at(world, &world_id, paired)
                .map(|d| d.check_obstruction)
                .unwrap_or(decl.check_obstruction);
            if paired_check_obstruction && is_chest_obstructed(world, &world_id, paired) {
                log::debug!("[chest] 配对箱子正上方被实心方块阻挡，无法打开: {paired:?}");
                return true;
            }
            let facing = chest_facing_at(world, &world_id, pos).unwrap_or("north");
            let (left, right) = canonical_chest_order(facing, pos, paired);
            (left, Some(right), 54u16)
        } else {
            (pos, None, decl.slots)
        }
    } else {
        (pos, None, decl.slots)
    };

    let Some(actor) = world.get_component::<sc_entity::MinecraftEntityId>(&entity) else {
        return true;
    };

    // Allocate a window_id.
    if world
        .get_component::<ContainerWindowAllocator>(&entity)
        .is_none()
    {
        world.add_component(&entity, ContainerWindowAllocator::default());
    }
    let window_id = world
        .get_component::<ContainerWindowAllocator>(&entity)
        .map(|allocator| allocator.allocate())
        .unwrap_or(1);

    let viewers_before = chest_viewer_count(world, world_id.as_ref(), left_pos, right_pos, None);

    // Attach the ContainerOpen component.
    world.add_component(
        &entity,
        ContainerOpen::chest(
            window_id,
            world_id.as_ref().clone(),
            left_pos,
            right_pos,
            total_slots,
        ),
    );

    // Load slot data.
    let slots = load_chest_slots(world, world_id.as_ref(), left_pos, right_pos);

    let mut faults = world.get_resource_mut::<PendingConnectionFaults>();
    let mut publisher = IntentPublisher::new(outbox)
        .with_world(world)
        .maybe_with_faults(faults.as_deref_mut());

    // 1. Emit the open-window intent: whichever half of a double chest is
    // clicked, the network open coordinates always use the canonical first
    // cell (left_pos).
    publisher.publish(OpenContainerWindow {
        entity_id: actor.0,
        window_id,
        container_type: decl.container_type,
        x: left_pos.0,
        y: left_pos.1,
        z: left_pos.2,
    });

    // 2. Emit the slot sync intent.
    publisher.publish(SyncChestWindow {
        entity_id: actor.0,
        window_id,
        slots,
    });

    // 3. When the first viewer, play the chest open sound and broadcast the
    // lid animation (BlockEvent: type=1, data=1).
    if viewers_before == 0 {
        let (sound_x, sound_y, sound_z) = if let Some(right) = right_pos {
            (
                (left_pos.0 as f32 + right.0 as f32) / 2.0 + 0.5,
                left_pos.1 as f32 + 0.5,
                (left_pos.2 as f32 + right.2 as f32) / 2.0 + 0.5,
            )
        } else {
            (
                left_pos.0 as f32 + 0.5,
                left_pos.1 as f32 + 0.5,
                left_pos.2 as f32 + 0.5,
            )
        };
        publisher.publish(PlaySound {
            cue: SoundCue::ChestOpen,
            world_id: world_id.as_ref().clone(),
            data: -1,
            x: sound_x,
            y: sound_y,
            z: sound_z,
        });
        publisher.publish(PlayBlockAnimation {
            world_id: world_id.as_ref().clone(),
            cue: BlockAnimationCue::ChestLid,
            data: 1,
            x: left_pos.0,
            y: left_pos.1,
            z: left_pos.2,
        });
        if let Some(right) = right_pos {
            publisher.publish(PlayBlockAnimation {
                world_id: world_id.as_ref().clone(),
                cue: BlockAnimationCue::ChestLid,
                data: 1,
                x: right.0,
                y: right.1,
                z: right.2,
            });
        }
    }

    log::debug!(
        "[chest] 成功打开箱子 window_id={window_id} left={left_pos:?} right={right_pos:?} slots={total_slots} viewers_before={viewers_before}"
    );

    true
}

/// 计算当前正在查看指定箱子（包含大箱子配对位置）的玩家数量。
///
/// 可通过 `exclude` 排除特定实体（如正在关闭窗口的当前玩家）。
///
/// Counts the players currently viewing the given chest, including the paired
/// position for a double chest. `exclude` drops a specific entity (e.g. the
/// player closing the window).
pub fn chest_viewer_count(
    world: &World,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
    paired: Option<(i32, i32, i32)>,
    exclude: Option<EntityId>,
) -> usize {
    let mut count = 0;
    for entity in world.entities_with_component::<ContainerOpen>() {
        if let Some(exc) = exclude {
            if entity == exc {
                continue;
            }
        }
        if let Some(open) = world.get_component::<ContainerOpen>(&entity) {
            if let Some((w_id, c_pos, c_paired)) = open.chest_info() {
                if &w_id == world_id {
                    let matches_primary = c_pos == pos || paired.map_or(false, |p| c_pos == p);
                    let matches_paired = c_paired.map_or(false, |cp| cp == pos || paired.map_or(false, |p| cp == p));
                    if matches_primary || matches_paired {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}
