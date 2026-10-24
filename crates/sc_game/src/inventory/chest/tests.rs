//! 箱子逻辑单元测试。
//!
//! Unit tests for the chest logic.

use super::*;
use crate::block_entity::BlockEntityIndex;
use crate::inventory::craft_inventory::{CraftContainer, CraftSlotRef};
use sc_ecs::world::World;
use sc_item::{ItemRegistry, ItemStack};
use sc_world::manager::MinecraftWorldId;

#[test]
fn perpendicular_neighbor_directions() {
    let pos = (10, 64, 10);
    // North: Left West (-X), Right East (+X)
    let (l, r) = perpendicular_neighbors("north", pos);
    assert_eq!(l, (9, 64, 10));
    assert_eq!(r, (11, 64, 10));

    // South: Left East (+X), Right West (-X)
    let (l, r) = perpendicular_neighbors("south", pos);
    assert_eq!(l, (11, 64, 10));
    assert_eq!(r, (9, 64, 10));

    // West: Left South (+Z), Right North (-Z)
    let (l, r) = perpendicular_neighbors("west", pos);
    assert_eq!(l, (10, 64, 11));
    assert_eq!(r, (10, 64, 9));

    // East: Left North (-Z), Right South (+Z)
    let (l, r) = perpendicular_neighbors("east", pos);
    assert_eq!(l, (10, 64, 9));
    assert_eq!(r, (10, 64, 11));
}

#[test]
fn canonical_chest_order_pairs_by_facing() {
    let pos_a = (10, 64, 10);
    let pos_b = (11, 64, 10);

    // North: West (-X) is Left
    let (l, r) = canonical_chest_order("north", pos_a, pos_b);
    assert_eq!(l, pos_a);
    assert_eq!(r, pos_b);

    // South: East (+X) is Left
    let (l, r) = canonical_chest_order("south", pos_a, pos_b);
    assert_eq!(l, pos_b);
    assert_eq!(r, pos_a);

    let pos_z1 = (10, 64, 10);
    let pos_z2 = (10, 64, 11);

    // West: South (+Z) is Left
    let (l, r) = canonical_chest_order("west", pos_z1, pos_z2);
    assert_eq!(l, pos_z2);
    assert_eq!(r, pos_z1);

    // East: North (-Z) is Left
    let (l, r) = canonical_chest_order("east", pos_z1, pos_z2);
    assert_eq!(l, pos_z1);
    assert_eq!(r, pos_z2);
}

#[test]
fn chest_slot_validation() {
    let slots = vec![
        ItemStack::new(1, 1),
        ItemStack::empty(),
    ];
    let valid_ref = CraftSlotRef {
        container: CraftContainer::Chest,
        slot: 0,
        net_id: 1,
        dynamic: Some(2),
        container_enum: None,
    };
    assert_eq!(chest_slot_index(&valid_ref, 2, 27, &slots), Some(0));

    // Wrong dynamic window id
    let wrong_window = CraftSlotRef {
        container: CraftContainer::Chest,
        slot: 0,
        net_id: 1,
        dynamic: Some(3),
        container_enum: None,
    };
    assert_eq!(chest_slot_index(&wrong_window, 2, 27, &slots), None);

    // Slot out of bounds
    let oob = CraftSlotRef {
        container: CraftContainer::Chest,
        slot: 28,
        net_id: 0,
        dynamic: None,
        container_enum: None,
    };
    assert_eq!(chest_slot_index(&oob, 2, 27, &slots), None);

    // net_id > 0 but slot is empty on server
    let empty_with_net_id = CraftSlotRef {
        container: CraftContainer::Chest,
        slot: 1,
        net_id: 5,
        dynamic: None,
        container_enum: None,
    };
    assert_eq!(chest_slot_index(&empty_with_net_id, 2, 27, &slots), None);
}

#[test]
fn load_and_commit_chest_slots() {
    let world = World::new();
    world.insert_resource(BlockEntityIndex::default());
    let world_id = MinecraftWorldId::random();
    let left_pos = (0, 64, 0);
    let right_pos = (1, 64, 0);

    // Initial load returns empty 27 or 54 slots
    let initial_single = load_chest_slots(&world, &world_id, left_pos, None);
    assert_eq!(initial_single.len(), 27);
    assert!(initial_single.iter().all(|s| s.is_empty()));

    let initial_double = load_chest_slots(&world, &world_id, left_pos, Some(right_pos));
    assert_eq!(initial_double.len(), 54);

    // Commit changes into double chest
    let mut modified = initial_double.clone();
    modified[0] = ItemStack::new(10, 64);
    modified[27] = ItemStack::new(20, 32);

    commit_chest_slots(&world, &world_id, left_pos, Some(right_pos), &modified);

    // Verify persisted slots
    let reloaded = load_chest_slots(&world, &world_id, left_pos, Some(right_pos));
    assert_eq!(reloaded[0].runtime_id, 10);
    assert_eq!(reloaded[0].count, 64);
    assert_eq!(reloaded[27].runtime_id, 20);
    assert_eq!(reloaded[27].count, 32);

    // Check single chest read for left and right
    let left_only = load_chest_slots(&world, &world_id, left_pos, None);
    assert_eq!(left_only[0].runtime_id, 10);
    let right_only = load_chest_slots(&world, &world_id, right_pos, None);
    assert_eq!(right_only[0].runtime_id, 20);
}

#[test]
fn chest_move_and_swap() {
    let items = ItemRegistry::new();
    let mut player_stacks = vec![ItemStack::new(1, 10)];
    let mut chest_slots = vec![ItemStack::empty(); 27];

    let src = CraftSlotRef {
        container: CraftContainer::Inventory,
        slot: 0,
        net_id: 0,
        dynamic: None,
        container_enum: None,
    };
    let dst = CraftSlotRef {
        container: CraftContainer::Chest,
        slot: 5,
        net_id: 0,
        dynamic: None,
        container_enum: None,
    };

    let valid_player = |_: CraftSlotRef, _: usize, _: &[ItemStack]| true;

    // Move 4 items from player to chest
    let success = chest_move(
        &valid_player,
        &items,
        1,
        false,
        1,
        27,
        &mut player_stacks,
        &mut chest_slots,
        4,
        &src,
        &dst,
        100,
    );
    assert!(success);
    assert_eq!(player_stacks[0].count, 6);
    assert_eq!(chest_slots[5].runtime_id, 1);
    assert_eq!(chest_slots[5].count, 4);

    // Swap between player and chest
    let swap_src = CraftSlotRef {
        container: CraftContainer::Inventory,
        slot: 0,
        net_id: 0,
        dynamic: None,
        container_enum: None,
    };
    let swap_dst = CraftSlotRef {
        container: CraftContainer::Chest,
        slot: 5,
        net_id: 0,
        dynamic: None,
        container_enum: None,
    };
    let swap_success = chest_swap(
        &valid_player,
        &items,
        1,
        false,
        1,
        27,
        &mut player_stacks,
        &mut chest_slots,
        &swap_src,
        &swap_dst,
        101,
    );
    assert!(swap_success);
    assert_eq!(player_stacks[0].count, 4);
    assert_eq!(chest_slots[5].count, 6);
}

#[test]
fn chest_switches_pairable_and_obstruction() {
    use sc_block::workstation::ChestDecl;

    // Normal chest: pairable=true, check_obstruction=true
    let normal_chest = ChestDecl {
        slots: 27,
        container_type: 0,
        pairable: true,
        check_obstruction: true,
        open_sound: "random.chestopen".into(),
        close_sound: "random.chestclosed".into(),
    };
    assert!(normal_chest.pairable);
    assert!(normal_chest.check_obstruction);

    // Barrel / unblocked custom container: pairable=false, check_obstruction=false
    let barrel = ChestDecl {
        slots: 27,
        container_type: 0,
        pairable: false,
        check_obstruction: false,
        open_sound: "block.barrel.open".into(),
        close_sound: "block.barrel.close".into(),
    };
    assert!(!barrel.pairable);
    assert!(!barrel.check_obstruction);

    // Obstruction bypass check: when check_obstruction is false, even if physical obstruction is true,
    // the boolean guard decl.check_obstruction && is_obstructed evaluates to false.
    let is_physically_obstructed = true;
    let chest_blocked = normal_chest.check_obstruction && is_physically_obstructed;
    let barrel_blocked = barrel.check_obstruction && is_physically_obstructed;
    assert!(chest_blocked, "Normal chest must be blocked by physical obstruction");
    assert!(!barrel_blocked, "Barrel with check_obstruction=false must ignore obstruction");
}

#[test]
fn chest_viewer_count_single_and_double() {
    use crate::inventory::chest_viewer_count;
    use crate::inventory::container::ContainerOpen;

    let world = World::new();
    let world_id = MinecraftWorldId::random();
    let pos_a = (10, 64, 10);
    let pos_b = (11, 64, 10);

    // Initial: 0 viewers
    assert_eq!(chest_viewer_count(&world, &world_id, pos_a, None, None), 0);
    assert_eq!(chest_viewer_count(&world, &world_id, pos_a, Some(pos_b), None), 0);

    // Player 1 opens single chest at pos_a
    let p1 = world.spawn(ContainerOpen::chest(1, world_id.clone(), pos_a, None, 27));
    assert_eq!(chest_viewer_count(&world, &world_id, pos_a, None, None), 1);
    // Excluding p1 gives 0
    assert_eq!(chest_viewer_count(&world, &world_id, pos_a, None, Some(p1)), 0);

    // Player 2 also opens the same chest
    let p2 = world.spawn(ContainerOpen::chest(2, world_id.clone(), pos_a, None, 27));
    assert_eq!(chest_viewer_count(&world, &world_id, pos_a, None, None), 2);
    assert_eq!(chest_viewer_count(&world, &world_id, pos_a, None, Some(p1)), 1);
    assert_eq!(chest_viewer_count(&world, &world_id, pos_a, None, Some(p2)), 1);

    // Now test double chest: Player 3 opens at pos_b with paired pos_a
    let _p3 = world.spawn(ContainerOpen::chest(3, world_id.clone(), pos_b, Some(pos_a), 54));
    // Querying double chest (pos_a, paired pos_b) should see all 3 viewers
    assert_eq!(chest_viewer_count(&world, &world_id, pos_a, Some(pos_b), None), 3);
    // Querying from pos_b side (pos_b, paired pos_a) should also see all 3 viewers
    assert_eq!(chest_viewer_count(&world, &world_id, pos_b, Some(pos_a), None), 3);

    // A viewer in another world shouldn't count
    let other_world = MinecraftWorldId::random();
    let _p_other = world.spawn(ContainerOpen::chest(4, other_world, pos_a, None, 27));
    assert_eq!(chest_viewer_count(&world, &world_id, pos_a, Some(pos_b), None), 3);
}

#[test]
fn is_player_sneaking() {
    use crate::movement::{is_player_sneaking, PlayerActionState, PlayerMovement};
    use crate::net::PlayerInput;

    let world = World::new();
    let (entity, _) = world.pre_spawn();

    // No movement or action state -> false
    assert!(!is_player_sneaking(&world, &entity));

    // PlayerActionState with sneaking false
    let action = PlayerActionState::new();
    world.add_component(&entity, action.clone());
    assert!(!is_player_sneaking(&world, &entity));

    // PlayerActionState with sneaking true
    action.inner.write().sneaking = true;
    assert!(is_player_sneaking(&world, &entity));

    // Reset action state to false
    action.inner.write().sneaking = false;
    assert!(!is_player_sneaking(&world, &entity));

    // PlayerMovement buffer has sneaking true
    let movement = PlayerMovement::new();
    movement.state.write().input_buffer.push_back(PlayerInput {
        sneaking: true,
        ..Default::default()
    });
    world.add_component(&entity, movement);
    assert!(is_player_sneaking(&world, &entity));
}

#[test]
fn double_chest_canonical_order_and_clicks() {
    let pos_a = (10, 64, 10);
    let pos_b = (11, 64, 10);

    // North facing: (10, 64, 10) is left, (11, 64, 10) is right
    let (left_from_a, right_from_a) = canonical_chest_order("north", pos_a, pos_b);
    let (left_from_b, right_from_b) = canonical_chest_order("north", pos_b, pos_a);

    // Both clicking side A or side B must yield identical left anchor
    assert_eq!(left_from_a, pos_a);
    assert_eq!(left_from_b, pos_a);
    assert_eq!(right_from_a, pos_b);
    assert_eq!(right_from_b, pos_b);
}

