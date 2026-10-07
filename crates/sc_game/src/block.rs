//! Authoritative block facts to gameplay/network intents.
//!
//! Drop path (`sc:drops`): direct drop definitions compile to read-only
//! profiles; after a break commits, the block owner rolls dice in its tick.
//! Without `sc:drops` (and without a parseable loot table) nothing drops;
//! creative mode drops nothing.

use std::collections::{HashSet, VecDeque};

use sc_block::block_json::BlockJsonRegistry;
use sc_block::registry::BlockStateRegistry;
use sc_block::write::{update_flags, BlockChangeCause, BlockChangedQueue};
use sc_ecs::params::resource::{Res, ResMut};
use sc_ecs::resource::Resource;
use sc_ecs::world::World;
use sc_entity::motion::{Position, Velocity};
use sc_item::{ItemRegistry, ItemStack};
use sc_log::t_log;
use sc_utils::game::client::MinecraftClient;

use crate::item_drop::ItemDropStore;
use crate::net::{IntentReliability, NetworkIntent, NetworkOutbox, ParticleCue, SoundCue};
use crate::net_backpressure::IntentPublisher;
use crate::net_faults::PendingConnectionFaults;

/// Drop receipt for one `OperationId` (in-memory idempotency).
///
/// One run rolls dice for one operation at most; retries reuse the
/// same `OperationId` without re-rolling or requeueing drops.
#[derive(Resource, Default)]
pub struct DropReceipts {
    seen: HashSet<u64>,
    // Rolled values are retained across retries; independent worlds may recover
    // even while another world's item storage remains full.
    pending: VecDeque<PendingBreakDrops>,
    /// Bounded receipt count; cleanup keeps operations awaiting generation.
    /// Receipts only dedup within a run.
    dropped_cleanups: u64,
}

struct PendingBreakDrops {
    change: sc_block::write::BlockChanged,
    stacks: Vec<ItemStack>,
}

fn publish_break_drops(
    pending: &mut PendingBreakDrops,
    drops: &mut ItemDropStore,
    registry: &ItemRegistry,
    publisher: &mut IntentPublisher<'_>,
) -> bool {
    let change = &pending.change;
    while let Some(stack) = pending.stacks.last_mut() {
        if stack.is_empty() {
            pending.stacks.pop();
            continue;
        }
        let count = stack
            .count
            .min(registry.max_stack_size(stack.runtime_id).max(1));
        let part = ItemStack {
            count,
            ..stack.clone()
        };
        if !publisher.admits(IntentReliability::ReliableFact) {
            return false;
        }
        let (jx, jz) = deterministic_jitter(change, &part);
        let admitted = drops.spawn_checked(
            change.world_id.clone(),
            part,
            Position::new(
                change.position.x as f32 + 0.5,
                change.position.y as f32 + 0.5,
                change.position.z as f32 + 0.5,
            ),
            Velocity::new(jx, 0.2, jz),
            registry,
            publisher,
        );
        if !admitted {
            return false;
        }
        stack.count -= count;
    }
    true
}

impl DropReceipts {
    const MAX_RECEIPTS: usize = 8192;
    const MAX_PENDING_OPERATIONS: usize = 4096;

    /// Returns `false` when already seen (caller skips generation).
    pub fn claim(&mut self, operation_id: u64) -> bool {
        if self.seen.contains(&operation_id) {
            return false;
        }
        if self.seen.len() >= Self::MAX_RECEIPTS {
            // Bounded cleanup keeps half; receipts are dedup hints only.
            let to_remove = Self::MAX_RECEIPTS / 2;
            let pending: HashSet<u64> = self
                .pending
                .iter()
                .map(|entry| entry.change.request_id)
                .collect();
            let ids: Vec<u64> = self
                .seen
                .iter()
                .copied()
                .filter(|id| !pending.contains(id))
                .take(to_remove)
                .collect();
            for id in ids {
                self.seen.remove(&id);
            }
            self.dropped_cleanups += 1;
            log::warn!(
                "{}",
                t_log!(
                    "console.game.drop_receipts_full",
                    max = Self::MAX_RECEIPTS,
                    removed = to_remove,
                    total = self.dropped_cleanups
                )
            );
        }
        self.seen.insert(operation_id);
        true
    }

    pub fn contains(&self, operation_id: u64) -> bool {
        self.seen.contains(&operation_id)
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }
}

/// Creative-mode breaks drop nothing (and sound no differently).
fn is_creative_player(world: &World, cause: &BlockChangeCause) -> bool {
    matches!(cause, BlockChangeCause::Player(entity) if world
        .get_component::<MinecraftClient>(entity)
        .map(|client| client.data.read().gamemode.is_creative())
        .unwrap_or(false))
}

/// Consume reliable block-write facts and publish all consequences.
///
/// `sc_block` also emits the legacy `BlockChanged` ECS event for plugins, but
/// this core path uses the explicit FIFO so high-frequency edits are not lost.
///
/// An `UpdateBlock` intent that cannot be admitted is **not** dropped silently:
/// the affected column is registered for an authoritative refresh, so viewers
/// converge on the newest committed generation instead of missing the delta.
///
/// Drop semantics:
/// - rolls happen only after the break commits, never pre-generated;
/// - the owning region rolls in its own tick without cross-region locks;
/// - one `OperationId` computes at most once, retries never re-drop;
/// - no `sc:drops` (and no parseable loot table) means no drops;
/// - overloaded queues keep rolled values and retry per stack cap.
pub fn block_changed_to_outbox(
    world: World,
    mut changed_queue: ResMut<BlockChangedQueue>,
    item_registry: Res<ItemRegistry>,
    mut drops: ResMut<ItemDropStore>,
    mut outbox: ResMut<NetworkOutbox>,
    block_registry: Res<BlockStateRegistry>,
    json_registry: Res<BlockJsonRegistry>,
    mut receipts: ResMut<DropReceipts>,
) {
    let mut faults = world.get_resource_mut::<PendingConnectionFaults>();
    let mut publisher = IntentPublisher::new(&mut outbox)
        .with_world(&world)
        .maybe_with_faults(faults.as_deref_mut());
    let mut blocked_worlds = HashSet::new();
    for _ in 0..receipts.pending.len() {
        let mut pending = receipts.pending.pop_front().expect("pending retry window");
        if blocked_worlds.contains(&pending.change.world_id)
            || !publish_break_drops(&mut pending, &mut drops, &item_registry, &mut publisher)
        {
            blocked_worlds.insert(pending.change.world_id.clone());
            receipts.pending.push_back(pending);
        }
    }
    loop {
        if receipts.pending.len() >= DropReceipts::MAX_PENDING_OPERATIONS {
            break;
        }
        let Some(change) = changed_queue.pop_front() else {
            break;
        };
        if change.flags & update_flags::NETWORK != 0 {
            publisher.publish(NetworkIntent::UpdateBlock {
                world_id: change.world_id.clone(),
                dimension: change.dimension,
                incarnation: change.incarnation,
                generation: change.generation,
                x: change.position.x,
                y: change.position.y,
                z: change.position.z,
                runtime_id: change.current.0,
                flags: change.flags,
                layer: change.layer as u32,
            });
        }

        let air = sc_world::block_dictionary::air_runtime_id();
        let is_break = change.previous.0 != air && change.current.0 == air;
        if !is_break {
            // Placement success broadcasts the place sound event;
            // break sounds stay client-predicted. Player placements only.
            if change.current.0 != air
                && change.previous != change.current
                && change.flags & update_flags::NO_GRAPHIC == 0
                && matches!(change.cause, BlockChangeCause::Player(_))
            {
                publisher.publish(NetworkIntent::PlaySound {
                    world_id: change.world_id.clone(),
                    cue: SoundCue::BlockPlace,
                    data: change.current.0 as i32,
                    x: change.position.x as f32 + 0.5,
                    y: change.position.y as f32 + 0.5,
                    z: change.position.z as f32 + 0.5,
                });
            }
            continue;
        }

        let x = change.position.x as f32 + 0.5;
        let y = change.position.y as f32 + 0.5;
        let z = change.position.z as f32 + 0.5;
        if change.flags & update_flags::NO_GRAPHIC == 0 {
            // Only the destroy-block particle broadcasts; break sounds
            // stay client-predicted (server echo would double-play).
            publisher.publish(NetworkIntent::PlayParticle {
                world_id: change.world_id.clone(),
                cue: ParticleCue::BlockBreak,
                data: change.previous.0 as i32,
                x,
                y,
                z,
            });
        }

        // Creative-mode breaks drop nothing.
        if change
            .break_context
            .as_ref()
            .map(|context| context.creative)
            .unwrap_or_else(|| is_creative_player(&world, &change.cause))
        {
            continue;
        }
        // Idempotent: one OperationId rolls dice at most once.
        if !receipts.claim(change.request_id) {
            log::debug!(
                "drop already rolled (operation {}), skipping re-roll",
                change.request_id
            );
            continue;
        }
        // Fortune level: limited implementation (no enchant table).
        // Harvest flag comes from the break request snapshot.
        let hand = if let Some(context) = &change.break_context {
            context.hand.clone()
        } else {
            match &change.cause {
                BlockChangeCause::Player(entity) => {
                    crate::interaction::hand_snapshot(&world, entity)
                }
                _ => sc_block::mining_drops::HandSnapshot {
                    item: None,
                    efficiency_level: 0,
                    fortune_level: 0,
                },
            }
        };
        // Non-player causes have no hand concept: count as harvested.
        let non_player = !matches!(change.cause, BlockChangeCause::Player(_));
        let fortune_level = hand.fortune_level;
        let harvested = non_player
            || harvested_for_hand(&block_registry, &json_registry, change.previous.0, &hand);
        if let Some(stacks) = roll_break_drops(
            &block_registry,
            &json_registry,
            &item_registry,
            change.previous.0,
            fortune_level,
            harvested,
            hand.item.as_deref(),
            &change,
        ) {
            let mut pending = PendingBreakDrops { change, stacks };
            if blocked_worlds.contains(&pending.change.world_id)
                || !publish_break_drops(&mut pending, &mut drops, &item_registry, &mut publisher)
            {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.game.drop_deferred",
                        request = pending.change.request_id
                    )
                );
                blocked_worlds.insert(pending.change.world_id.clone());
                receipts.pending.push_back(pending);
            }
        }
    }
}

/// Harvest check for a break (drop gate input; pure, lock-free).
///
/// - Without `sc:mining`: counts as harvested;
/// - With `sc:mining`: run `mining_decision` on the held item;
/// - Non-player causes pass `harvested=true` directly.
pub fn harvested_for_hand(
    block_registry: &BlockStateRegistry,
    json_registry: &BlockJsonRegistry,
    previous_runtime: u32,
    hand: &sc_block::mining_drops::HandSnapshot,
) -> bool {
    let Some(snapshot) = json_registry.get() else {
        return true;
    };
    let Some(state_id) =
        block_registry.by_runtime_id(sc_world::chunk::BlockRuntimeId(previous_runtime))
    else {
        return true;
    };
    let Some(compiled) = snapshot.mining_of(state_id.0) else {
        return true;
    };
    match sc_block::mining_drops::mining_decision(compiled, hand) {
        sc_block::mining_drops::MineDecision::Mine { harvested, .. } => harvested,
        sc_block::mining_drops::MineDecision::Deny => false,
    }
}

/// Deterministic break drop roll (pure; no locks, IO, or network).
///
/// Returns `None` for no block drops; `Some(vec![])` for an empty roll;
/// `Some(stacks)` for item stacks awaiting spawn.
pub fn roll_break_drops(
    block_registry: &BlockStateRegistry,
    json_registry: &BlockJsonRegistry,
    item_registry: &ItemRegistry,
    previous_runtime: u32,
    fortune_level: u32,
    harvested: bool,
    hand_item: Option<&str>,
    change: &sc_block::write::BlockChanged,
) -> Option<Vec<ItemStack>> {
    use sc_block::mining_drops::{drop_seed, roll_drops};
    let snapshot = json_registry.get()?;
    let state_id =
        block_registry.by_runtime_id(sc_world::chunk::BlockRuntimeId(previous_runtime))?;
    let profile = snapshot.drops_of(state_id.0)?;
    if !profile.enabled {
        return None;
    }
    // Loot-path references are a separate unresolved data domain.
    let world_u128 = change.world_id.world_id.as_u128();
    let world_lo = (world_u128 & 0xFFFF_FFFF_FFFF_FFFF) as u64;
    let world_hi = ((world_u128 >> 64) & 0xFFFF_FFFF_FFFF_FFFF) as u64;
    let rolled = roll_drops(
        profile,
        fortune_level,
        harvested,
        hand_item,
        |entry_index, kind| {
            let seed = drop_seed(
                world_lo,
                world_hi,
                change.position.x,
                change.position.y,
                change.position.z,
                change.generation,
                change.request_id,
                entry_index,
                kind,
            );
            // Uniform [0,1) (53 bits).
            const DIV: f64 = (1u64 << 53) as f64;
            (((seed >> 11) as f64) / DIV).clamp(0.0, 0.999_999_999)
        },
    );
    // Empty `one_of` and full-miss `independent` yield empty vecs.
    if rolled.is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::with_capacity(rolled.len());
    for r in rolled {
        let Some(runtime_id) = item_registry.runtime_id_by_name(&r.item) else {
            log::warn!(
                "{}",
                t_log!("console.game.drop_not_in_registry", item = format!("{:?}", r.item))
            );
            continue;
        };
        // `count` is bounded at compile time; `ItemStack.count` is u16.
        let count = r.count.min(u16::MAX as u32) as u16;
        if count == 0 {
            continue;
        }
        out.push(ItemStack::new(runtime_id, count));
    }
    Some(out)
}

/// Deterministic scatter jitter (`x/z` in [-0.1, 0.1)).
fn deterministic_jitter(change: &sc_block::write::BlockChanged, stack: &ItemStack) -> (f32, f32) {
    use sc_block::mining_drops::drop_seed;
    let world_u128 = change.world_id.world_id.as_u128();
    let world_lo = (world_u128 & 0xFFFF_FFFF_FFFF_FFFF) as u64;
    let world_hi = ((world_u128 >> 64) & 0xFFFF_FFFF_FFFF_FFFF) as u64;
    let item_hash = {
        let mut h = 0u64;
        for b in stack
            .runtime_id
            .to_le_bytes()
            .iter()
            .chain((stack.count as u32).to_le_bytes().iter())
        {
            h = h.wrapping_mul(0x1000_0000_01B3).wrapping_add(*b as u64);
        }
        h
    };
    let jx_seed = drop_seed(
        world_lo,
        world_hi,
        change.position.x,
        change.position.y,
        change.position.z,
        change.generation,
        change.request_id ^ item_hash,
        0x6A69_7474_6572_58, // "jitterX"
        0x7665_6C6F_6369_74, // "velocit" 截断（实现冻结，golden 锁定）
    );
    let jz_seed = drop_seed(
        world_lo,
        world_hi,
        change.position.x,
        change.position.y,
        change.position.z,
        change.generation,
        change.request_id ^ item_hash.rotate_left(32),
        0x6A69_7474_6572_5A, // "jitterZ"
        0x7665_6C6F_6369_74,
    );
    const DIV: f64 = (1u64 << 53) as f64;
    let jx = ((jx_seed >> 11) as f64) / DIV * 0.2 - 0.1;
    let jz = ((jz_seed >> 11) as f64) / DIV * 0.2 - 0.1;
    (jx as f32, jz as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_block::block_json::{compile_bundle, BlockJsonRegistry};
    use sc_block::registry::BlockStateRegistry;
    use sc_packloader::block::{
        fingerprint_bundle, parse_block_file, BlockBundleBudgets, BlockJsonBundle,
    };

    fn compile_test_bundle() -> (BlockJsonRegistry, BlockStateRegistry, sc_item::ItemRegistry) {
        let air = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        let ore = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "example:ore", "states": {}},
                "components": {
                    "minecraft:destructible_by_mining": {"value": 3.0},
                    "sc:mining": {
                        "formula_version": 1,
                        "base_time_seconds": 3.0,
                        "default": {"can_mine": true, "harvest": false, "speed_multiplier": 1.0},
                        "tools": [{"items": ["minecraft:iron_pickaxe"], "can_mine": true, "harvest": true, "speed_multiplier": 4.0}]
                    },
                    "sc:drops": {
                        "enabled": true, "mode": "independent",
                        "entries": [
                            {"item": "minecraft:cobblestone", "chance": 1.0, "count": [{"value": 1, "chance": 1.0}]},
                            {"item": "minecraft:coal", "chance": 0.25, "count": [{"value": 1, "chance": 0.75}, {"value": 2, "chance": 0.25}]}
                        ]
                    }
                },
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [32000]
            }
        }"#;
        let plain = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "example:plain", "states": {}},
                "components": {"minecraft:destructible_by_mining": {"value": 1.0}},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [32001]
            }
        }"#;
        let raws: Vec<(&str, &str)> = vec![
            ("definitions/blocks/minecraft/air.block.json", air),
            ("definitions/blocks/example/ore.block.json", ore),
            ("definitions/blocks/example/plain.block.json", plain),
        ];
        let budgets = BlockBundleBudgets::default();
        let mut refs: Vec<(&str, &[u8])> = raws.iter().map(|(p, b)| (*p, b.as_bytes())).collect();
        refs.sort_by(|a, b| a.0.cmp(b.0));
        let files = refs
            .iter()
            .map(|(p, b)| parse_block_file("test", p, b, &budgets).unwrap())
            .collect();
        let bundle = BlockJsonBundle {
            schema_version: 1,
            network_id_mode: "hashed".to_string(),
            fingerprint: fingerprint_bundle(&refs, "hashed"),
            files,
        };
        let allow = |id: &str| {
            matches!(
                id,
                "minecraft:cobblestone" | "minecraft:coal" | "minecraft:iron_pickaxe"
            )
        };
        let (snapshot, _) =
            compile_bundle(&bundle, "test", &budgets, &|_| None, &allow).expect("应编译");
        let block_registry = BlockStateRegistry::from_block_snapshot(&snapshot);
        let json_registry = BlockJsonRegistry::new();
        json_registry.publish(std::sync::Arc::new(snapshot));
        let mut item_registry = sc_item::ItemRegistry::new();
        item_registry
            .upsert(sc_item::ItemDefinition::new(1, "minecraft:cobblestone").max_stack_size(64));
        item_registry.upsert(sc_item::ItemDefinition::new(2, "minecraft:coal").max_stack_size(64));
        (json_registry, block_registry, item_registry)
    }

    #[test]
    fn full_world_defers_rolled_fact_without_blocking_other_worlds_or_rerolling() {
        use sc_ecs::system::{IntoSystem, System};
        let (json, blocks, items) = compile_test_bundle();
        let runtime = blocks
            .state(blocks.state_of("example:ore", &[]).unwrap())
            .unwrap()
            .hash;
        let change = fake_change(runtime, 100);
        let mut other = fake_change(runtime, 101);
        other.world_id = sc_world::manager::MinecraftWorldId::random();
        let expected =
            roll_break_drops(&blocks, &json, &items, runtime, 0, true, None, &change).unwrap();
        let mut store = ItemDropStore::default();
        let mut outbox = NetworkOutbox::default();
        for _ in 0..ItemDropStore::MAX_ACTIVE_DROPS_PER_WORLD {
            assert!(store.spawn_checked(
                change.world_id.clone(),
                ItemStack::new(1, 1),
                Position::new(0.0, 64.0, 0.0),
                Velocity::default(),
                &items,
                &mut IntentPublisher::new(&mut outbox)
            ));
        }
        outbox.drain();
        let mut facts = BlockChangedQueue::default();
        facts.push(change.clone());
        facts.push(other.clone());
        facts.push(change.clone());
        let world = World::new();
        world.insert_resource(json);
        world.insert_resource(blocks);
        world.insert_resource(items);
        world.insert_resource(store);
        world.insert_resource(outbox);
        world.insert_resource(facts);
        world.insert_resource(DropReceipts::default());
        let mut system = block_changed_to_outbox.into_system();
        system.run(&world);
        let receipts = world.get_resource::<DropReceipts>().unwrap();
        assert_eq!(receipts.pending.len(), 1);
        assert!(receipts.contains(100));
        assert!(receipts.contains(101));
        assert_eq!(receipts.pending[0].stacks, expected);
        drop(receipts);
        let sent = world.get_resource_mut::<NetworkOutbox>().unwrap().drain();
        assert!(sent.iter().any(|intent| matches!(intent, NetworkIntent::SpawnItemEntity { world_id, .. } if world_id == &other.world_id)));
        assert!(!sent.iter().any(|intent| matches!(intent, NetworkIntent::SpawnItemEntity { world_id, .. } if world_id == &change.world_id)));
        system.run(&world);
        assert_eq!(
            world.get_resource::<DropReceipts>().unwrap().pending[0].stacks,
            expected
        );
        assert!(world.get_resource::<NetworkOutbox>().unwrap().is_empty());
    }

    #[test]
    fn committed_break_uses_captured_hand_after_player_switches_tools() {
        use sc_ecs::system::{IntoSystem, System};
        let (json, blocks, items) = compile_test_bundle();
        let runtime = blocks.state_of("example:ore", &[]).unwrap();
        let world = World::new();
        let player = world.spawn(sc_item::PlayerInventory::new(36));
        let mut change = fake_change(blocks.state(runtime).unwrap().hash, 99);
        change.cause = BlockChangeCause::Player(player);
        change.break_context = Some(sc_block::write::BlockBreakContext {
            hand: sc_block::mining_drops::HandSnapshot {
                item: Some("minecraft:iron_pickaxe".into()),
                ..Default::default()
            },
            creative: false,
        });
        let mut facts = BlockChangedQueue::default();
        facts.push(change);
        world.insert_resource(json);
        world.insert_resource(blocks);
        world.insert_resource(items);
        world.insert_resource(facts);
        world.insert_resource(ItemDropStore::default());
        world.insert_resource(NetworkOutbox::default());
        world.insert_resource(DropReceipts::default());
        block_changed_to_outbox.into_system().run(&world);
        assert!(world.get_resource_mut::<NetworkOutbox>().unwrap().drain().iter().any(|intent|
            matches!(intent, NetworkIntent::SpawnItemEntity { stack, .. } if stack.runtime_id == 1)));
    }

    #[test]
    fn pending_drops_resume_without_rerolling_or_duplicating_split_stacks() {
        let registry = ItemRegistry::new();
        registry.upsert(sc_item::ItemDefinition::new(1, "minecraft:cobblestone").max_stack_size(2));
        let mut pending = PendingBreakDrops {
            change: fake_change(7, 42),
            stacks: vec![ItemStack::new(1, 5), ItemStack::new(1, 1)],
        };
        let mut store = ItemDropStore::default();
        let mut outbox = NetworkOutbox::with_limits(1, 1, 1024);
        let mut total = 0;
        let mut packets = 0;
        for _ in 0..4 {
            let finished = publish_break_drops(
                &mut pending,
                &mut store,
                &registry,
                &mut IntentPublisher::new(&mut outbox),
            );
            for intent in outbox.drain() {
                if let NetworkIntent::SpawnItemEntity { stack, .. } = intent {
                    total += stack.count;
                    packets += 1;
                }
            }
            if finished {
                break;
            }
        }
        assert_eq!(total, 6);
        assert_eq!(packets, 4);
        assert!(pending.stacks.is_empty());
        assert!(publish_break_drops(
            &mut pending,
            &mut store,
            &registry,
            &mut IntentPublisher::new(&mut outbox)
        ));
        assert!(outbox.is_empty());
    }

    #[test]
    fn full_item_store_retains_pending_drops_until_capacity_recovers() {
        let registry = ItemRegistry::new();
        registry.upsert(sc_item::ItemDefinition::new(1, "minecraft:cobblestone"));
        let mut pending = PendingBreakDrops {
            change: fake_change(7, 42),
            stacks: vec![ItemStack::new(1, 1)],
        };
        let mut store = ItemDropStore::default();
        let mut outbox = NetworkOutbox::default();
        for _ in 0..ItemDropStore::MAX_ACTIVE_DROPS_PER_WORLD {
            assert!(store.spawn_checked(
                pending.change.world_id.clone(),
                ItemStack::new(1, 1),
                Position::new(0.0, 64.0, 0.0),
                Velocity::default(),
                &registry,
                &mut IntentPublisher::new(&mut outbox)
            ));
        }
        outbox.drain();
        assert!(!publish_break_drops(
            &mut pending,
            &mut store,
            &registry,
            &mut IntentPublisher::new(&mut outbox)
        ));
        assert_eq!(pending.stacks[0].count, 1);
        assert!(outbox.is_empty());
        for _ in 0..10 {
            store.tick(None, &registry, &mut IntentPublisher::new(&mut outbox));
            outbox.drain();
        }
        store.try_pickup(
            &pending.change.world_id,
            1,
            &sc_entity::motion::Aabb {
                min_x: -10.0,
                min_y: 0.0,
                min_z: -10.0,
                max_x: 10.0,
                max_y: 100.0,
                max_z: 10.0,
            },
            &sc_item::PlayerInventory::new(1),
            false,
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );
        outbox.drain();
        assert!(publish_break_drops(
            &mut pending,
            &mut store,
            &registry,
            &mut IntentPublisher::new(&mut outbox)
        ));
        assert!(pending.stacks.is_empty());
        assert_eq!(
            outbox
                .drain()
                .iter()
                .filter(|intent| matches!(intent, NetworkIntent::SpawnItemEntity { .. }))
                .count(),
            1
        );
    }

    fn fake_change(previous: u32, request_id: u64) -> sc_block::write::BlockChanged {
        sc_block::write::BlockChanged {
            break_context: None,
            request_id,
            world_id: sc_world::manager::MinecraftWorldId::random(),
            dimension: 0,
            incarnation: 1,
            generation: 7,
            position: sc_block::position::BlockPosition::new(3, 64, 5),
            layer: 0,
            previous: sc_world::chunk::BlockRuntimeId(previous),
            current: sc_world::chunk::BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()),
            cause: sc_block::write::BlockChangeCause::Command,
            flags: sc_block::write::update_flags::DEFAULT,
        }
    }

    #[test]
    fn no_drops_without_profile_is_not_guessed() {
        let (json_registry, block_registry, item_registry) = compile_test_bundle();
        // Plain blocks without drops yield None (never guess by name).
        let plain_hash = {
            let idx = json_registry
                .get()
                .unwrap()
                .default_state_idx("example:plain")
                .unwrap();
            // Look up the registry runtime id.
            // Simplified to a direct reverse lookup.
            let snap = json_registry.get().unwrap();
            let state_idx = snap.default_state_idx("example:plain").unwrap();
            block_registry
                .runtime_id(sc_block::state::BlockStateId(state_idx))
                .unwrap()
                .0
        };
        let change = fake_change(plain_hash, 1);
        let result = roll_break_drops(
            &block_registry,
            &json_registry,
            &item_registry,
            plain_hash,
            0,
            true,
            None,
            &change,
        );
        assert!(
            result.is_none(),
            "无 drops 时不得猜测掉落，实际：{result:?}"
        );
    }

    #[test]
    fn unharvested_break_yields_default_empty_drop() {
        use sc_block::mining_drops::HandSnapshot;
        let (json_registry, block_registry, item_registry) = compile_test_bundle();
        let snap = json_registry.get().unwrap();
        let state_idx = snap.default_state_idx("example:ore").unwrap();
        let ore_hash = block_registry
            .runtime_id(sc_block::state::BlockStateId(state_idx))
            .unwrap()
            .0;
        // Ore mining defaults to unharvestable: bare hands fail.
        assert!(!harvested_for_hand(
            &block_registry,
            &json_registry,
            ore_hash,
            &HandSnapshot::default()
        ));
        let change = fake_change(ore_hash, 7);
        // Unharvested means default empty drops without rolling.
        let empty = roll_break_drops(
            &block_registry,
            &json_registry,
            &item_registry,
            ore_hash,
            0,
            false,
            None,
            &change,
        )
        .expect("应为 Some 空");
        assert!(empty.is_empty());
        // Harvested means normal drops.
        let drops = roll_break_drops(
            &block_registry,
            &json_registry,
            &item_registry,
            ore_hash,
            0,
            true,
            None,
            &change,
        )
        .expect("应为 Some");
        assert!(
            drops.iter().any(|s| s.runtime_id == 1),
            "收获时应掉 cobblestone，实际：{drops:?}"
        );
    }

    #[test]
    fn retry_with_same_operation_does_not_reroll() {
        let mut receipts = DropReceipts::default();
        assert!(receipts.claim(42));
        assert!(!receipts.claim(42), "同一 OperationId 重试不得重复掷骰");
        assert!(receipts.claim(43));
        assert_eq!(receipts.len(), 2);
    }

    #[test]
    fn quantity_split_keeps_total() {
        use sc_item::{ItemDefinition, ItemRegistry};
        let registry = ItemRegistry::new();
        registry.upsert(ItemDefinition::new(1, "minecraft:coal").max_stack_size(2));
        let mut store = ItemDropStore::default();
        let mut outbox = NetworkOutbox::default();
        let world_id = sc_world::manager::MinecraftWorldId::random();
        let admitted = store.spawn_checked(
            world_id,
            sc_item::ItemStack::new(1, 5),
            sc_entity::motion::Position::new(0.0, 64.0, 0.0),
            sc_entity::motion::Velocity::default(),
            &registry,
            &mut crate::net_backpressure::IntentPublisher::new(&mut outbox),
        );
        assert!(admitted);
        // max 2: 5 -> 2+2+1, total unchanged.
        assert_eq!(store.active_count(), 3);
        let total: u16 = {
            // Sum stack counts over network intents.
            let intents = outbox.drain();
            intents
                .iter()
                .filter_map(|i| match i {
                    NetworkIntent::SpawnItemEntity { stack, .. } => Some(stack.count),
                    _ => None,
                })
                .sum()
        };
        assert_eq!(total, 5);
    }

    #[test]
    fn drop_queue_bound_returns_busy_without_silent_loss() {
        use sc_item::{ItemDefinition, ItemRegistry};
        let registry = ItemRegistry::new();
        registry.upsert(ItemDefinition::new(1, "minecraft:coal").max_stack_size(64));
        let mut store = ItemDropStore::default();
        let mut outbox = NetworkOutbox::default();
        let world_id = sc_world::manager::MinecraftWorldId::random();
        // Fill to the cap.
        for _ in 0..ItemDropStore::MAX_ACTIVE_DROPS_PER_WORLD {
            let admitted = store.spawn_checked(
                world_id.clone(),
                sc_item::ItemStack::new(1, 1),
                sc_entity::motion::Position::new(0.0, 64.0, 0.0),
                sc_entity::motion::Velocity::default(),
                &registry,
                &mut crate::net_backpressure::IntentPublisher::new(&mut outbox),
            );
            assert!(admitted);
        }
        // Further spawns report busy explicitly, never silently.
        let busy = store.spawn_checked(
            world_id,
            sc_item::ItemStack::new(1, 1),
            sc_entity::motion::Position::new(0.0, 64.0, 0.0),
            sc_entity::motion::Velocity::default(),
            &registry,
            &mut crate::net_backpressure::IntentPublisher::new(&mut outbox),
        );
        assert!(!busy, "满载必须返回 busy");
    }

    #[test]
    fn deterministic_jitter_is_stable() {
        let change = fake_change(123, 99);
        let a = deterministic_jitter(&change, &sc_item::ItemStack::new(1, 1));
        let b = deterministic_jitter(&change, &sc_item::ItemStack::new(1, 1));
        assert_eq!(a, b);
        let c = deterministic_jitter(&fake_change(123, 100), &sc_item::ItemStack::new(1, 1));
        assert_ne!(a, c);
        // Range [-0.1, 0.1).
        assert!((-0.1..0.1).contains(&a.0));
        assert!((-0.1..0.1).contains(&a.1));
    }
}
