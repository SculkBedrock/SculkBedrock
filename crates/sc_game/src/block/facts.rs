//! 权威方块事实到游戏/网络 intent 的转换。
//!
//! Authoritative block facts to gameplay/network intents.
//!
//! 掉落与工具损耗：被引用的战利品表（`minecraft:loot`）编译为只读 profile；
//! 破坏提交后，方块所有者在自己的 tick 中掷骰并扣除手持工具耐久。没有战利品
//! 引用（或没有可解析的表）则无掉落；创造模式无掉落、不磨损。
//!
//! Drops and tool wear: the referenced loot table (`minecraft:loot`) compiles to
//! read-only profiles; after a break commits, the block owner rolls dice and
//! charges the held tool's durability in its own tick. Without a loot reference
//! (or without a resolvable table) nothing drops; creative mode neither drops
//! nor wears.

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

use crate::item::ItemDropStore;
use crate::net::{
    IntentPublisher, IntentReliability, NetworkOutbox, ParticleCue,
    PendingConnectionFaults, PendingInventoryResync, PlayParticle, PlaySound, SoundCue,
    SyncHeldItem, UpdateBlock, UpdateInventorySlot,
};

/// 一次破坏结算的回执（内存内幂等）。
///
/// Break-settlement receipt (in-memory idempotency).
///
/// 结算覆盖工具耐久损耗与掉落掷骰：一次运行对每个操作最多结算一次，
/// 重试复用同一个 `OperationId`，不会重复扣耐久、也不会重新掷骰或重新排队掉落。
/// The settlement covers tool wear and the drop roll: one run settles an
/// operation at most once, and retries reuse the same `OperationId` without
/// double-charging wear, re-rolling or requeueing drops.
#[derive(Resource, Default)]
pub struct BreakSettlementReceipts {
    seen: HashSet<u64>,
    // Rolled values are retained across retries; independent worlds may
    // recover even while another world's item storage remains full.
    pending: VecDeque<PendingBreakDrops>,
    /// 有界回执数；清理会保留等待生成的操作。
    /// 回执只在一次运行内去重。
    ///
    /// Bounded receipt count; cleanup keeps operations awaiting generation.
    /// Receipts only dedup within a run.
    dropped_cleanups: u64,
}

/// 待生成的破坏掉落：一次方块变更加其掷出的物品栈。
///
/// Pending break drops: one block change plus its rolled item stacks.
struct PendingBreakDrops {
    /// 触发掉落的方块变更事实。
    ///
    /// Block-change fact that produced the drops.
    change: sc_block::write::BlockChanged,
    /// 等待生成的物品栈。
    ///
    /// Item stacks awaiting spawn.
    stacks: Vec<ItemStack>,
}

/// 生成待定掉落物中的物品；全部完成返回 `true`，被准入/存储拒绝时返回
/// `false`（调用方保留剩余部分稍后重试）。
///
/// Spawns the pending stacks; returns `true` when fully drained, `false`
/// when admission or storage refused (the caller retains the remainder for
/// a later retry).
///
/// - `pending`: 待定掉落（会被就地修改）
/// - `drops`: 物品掉落存储
/// - `registry`: 物品注册表
/// - `publisher`: intent 发布者
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

impl BreakSettlementReceipts {
    /// 回执表容量上限。
    ///
    /// Receipt table capacity.
    const MAX_RECEIPTS: usize = 8192;
    /// 待生成操作数上限。
    ///
    /// Maximum pending operations.
    const MAX_PENDING_OPERATIONS: usize = 4096;

    /// 认领一个操作；已见过时返回 `false`（调用方跳过生成）。
    ///
    /// Returns `false` when already seen (caller skips generation).
    ///
    /// - `operation_id`: 操作 id
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

    /// 是否已见过该操作。
    ///
    /// Whether the operation was already seen.
    ///
    /// - `operation_id`: 操作 id
    pub fn contains(&self, operation_id: u64) -> bool {
        self.seen.contains(&operation_id)
    }

    /// 已记录的回执数。
    ///
    /// Number of recorded receipts.
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// 是否没有任何回执。
    ///
    /// Whether no receipt is recorded.
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

/// 创造模式破坏无掉落（声音照旧）。
///
/// Creative-mode breaks drop nothing (and sound no differently).
///
/// - `world`: ECS 世界
/// - `cause`: 方块变更原因
fn is_creative_player(world: &World, cause: &BlockChangeCause) -> bool {
    matches!(cause, BlockChangeCause::Player(entity) if world
        .get_component::<MinecraftClient>(entity)
        .map(|client| client.data.read().gamemode.is_creative())
        .unwrap_or(false))
}

/// 消费可靠的方块写入事实并发布全部后果。
///
/// Consume reliable block-write facts and publish all consequences.
///
/// `sc_block` 也会为插件发出旧的 `BlockChanged` ECS 事件，但本核心路径
/// 使用显式 FIFO，避免高频编辑丢失。
/// `sc_block` also emits the legacy `BlockChanged` ECS event for plugins,
/// but this core path uses the explicit FIFO so high-frequency edits are not
/// lost.
///
/// 无法准入的 `UpdateBlock` intent **不会**被静默丢弃：受影响的列会登记
/// 一次权威刷新，使观看者收敛到最新已提交的世代而不是丢失该增量。
/// An `UpdateBlock` intent that cannot be admitted is **not** dropped
/// silently: the affected column is registered for an authoritative refresh,
/// so viewers converge on the newest committed generation instead of
/// missing the delta.
///
/// 掉落语义：
/// Drop semantics:
/// - 只在破坏提交后掷骰，绝不预生成；
///   rolls happen only after the break commits, never pre-generated;
/// - 所有 region 在自己的 tick 中掷骰，无需跨 region 锁；
///   the owning region rolls in its own tick without cross-region locks;
/// - 一个 `OperationId` 最多计算一次，重试绝不重复掉落；
///   one `OperationId` computes at most once, retries never re-drop;
/// - 没有战利品引用（或没有可解析的表）即无掉落；
///   no loot reference (or no resolvable table) means no drops;
/// - 队列过载时保留掷出的值，按每栈上限重试。
///   overloaded queues keep rolled values and retry per stack cap.
pub fn block_changed_to_outbox(
    world: World,
    mut changed_queue: ResMut<BlockChangedQueue>,
    item_registry: Res<ItemRegistry>,
    mut drops: ResMut<ItemDropStore>,
    mut outbox: ResMut<NetworkOutbox>,
    block_registry: Res<BlockStateRegistry>,
    json_registry: Res<BlockJsonRegistry>,
    mut receipts: ResMut<BreakSettlementReceipts>,
) {
    let mut pending_resync = world.get_resource_mut::<PendingInventoryResync>();
    let mut faults = world.get_resource_mut::<PendingConnectionFaults>();
    let mut publisher = IntentPublisher::new(&mut outbox)
        .with_world(&world)
        .maybe_with_pending_resync(pending_resync.as_deref_mut())
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
        if receipts.pending.len() >= BreakSettlementReceipts::MAX_PENDING_OPERATIONS {
            break;
        }
        let Some(change) = changed_queue.pop_front() else {
            break;
        };
        if change.flags & update_flags::NETWORK != 0 {
            publisher.publish(UpdateBlock {
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
            // Placement success broadcasts the place sound event; break sounds stay
            // client-predicted. Player placements only.
            if change.current.0 != air
                && change.previous != change.current
                && change.flags & update_flags::NO_GRAPHIC == 0
                && matches!(change.cause, BlockChangeCause::Player(_))
            {
                publisher.publish(PlaySound {
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
            // Only the destroy-block particle broadcasts; break sounds stay
            // client-predicted (server echo would double-play).
            publisher.publish(PlayParticle {
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
            // Identity still goes (the block is gone); contents drop nothing.
            crate::block_entity::destroy_block_entity(
                &world,
                &change.world_id,
                (change.position.x, change.position.y, change.position.z),
            );
            continue;
        }
        // Idempotent: one OperationId settles (wear + drops) at most once.
        if !receipts.claim(change.request_id) {
            log::debug!(
                "break already settled (operation {}), skipping re-settlement",
                change.request_id
            );
            continue;
        }
        // Tool wear settles here (single convergence point for every break path);
        // drops below share this operation's idempotency guard.
        settle_tool_wear(
            &world,
            &change,
            &item_registry,
            &block_registry,
            &json_registry,
            &mut publisher,
        );
        // Tracked process contents (furnace slots) join the break drops
        // (identity is destroyed whether or not anything was inside).
        let process_contents = crate::block_entity::take_break_contents(
            &world,
            &change.world_id,
            (change.position.x, change.position.y, change.position.z),
        )
        .unwrap_or_default();
        // Fortune level: limited implementation (no enchant table).
        // Harvest flag comes from the break request snapshot.
        let hand = if let Some(context) = &change.break_context {
            context.hand.clone()
        } else {
            match &change.cause {
                BlockChangeCause::Player(entity) => {
                    crate::block::hand_snapshot(&world, entity)
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
        let mut stacks = roll_break_drops(
            &block_registry,
            &json_registry,
            &item_registry,
            change.previous.0,
            fortune_level,
            harvested,
            hand.item.as_deref(),
            &change,
        )
        .unwrap_or_default();
        // Container contents are not block drops (they roll no table), but they
        // share the pending/backpressure path below.
        stacks.extend(process_contents);
        if !stacks.is_empty() {
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

/// 一次破坏的收获检查（掉落门禁输入；纯函数，无锁）。
///
/// Harvest check for a break (drop gate input; pure, lock-free).
///
/// - 没有 `sc:mining`：视为已收获；
///   Without `sc:mining`: counts as harvested;
/// - 有 `sc:mining`：对手持物品运行 `mining_decision`；
///   With `sc:mining`: run `mining_decision` on the held item;
/// - 非玩家原因直接传 `harvested=true`。
///   Non-player causes pass `harvested=true` directly.
///
/// - `block_registry`: 方块状态注册表
/// - `json_registry`: 方块 JSON 注册表
/// - `previous_runtime`: 破坏前的运行时 id
/// - `hand`: 手持物快照
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

/// 方块破坏的手持工具耐久结算。
///
/// Tool wear settlement for one landed block break.
///
/// 只处理玩家原因；创造模式在调用点之前已跳过。空手、无耐久声明的物品、
/// 以及“手持物已不是开始挖掘时那件”的情况都不扣耐久（防错扣）。
///
/// Player causes only; creative reaches here already filtered out. Empty hands,
/// items without a wear declaration, and breaks whose held item is no longer the
/// one that earned them are never charged (wrong-charge guard).
///
/// - `world`: 世界（读玩家库存与耐久表）
/// - `change`: 已落地的方块变更事实
/// - `item_registry`: 物品注册表（runtime id → 标识符）
/// - `block_registry`: 方块状态注册表（重算工具匹配）
/// - `json_registry`: 方块 JSON 注册表（`sc:mining` 快照）
/// - `publisher`: intent 发布器（可靠事实 + 可覆盖表现）
fn settle_tool_wear(
    world: &World,
    change: &sc_block::write::BlockChanged,
    item_registry: &ItemRegistry,
    block_registry: &BlockStateRegistry,
    json_registry: &BlockJsonRegistry,
    publisher: &mut IntentPublisher<'_>,
) {
    use sc_block::item_wear::{wear_decision, WearCause, WearInput, WearOutcome};

    let BlockChangeCause::Player(entity) = &change.cause else {
        return;
    };
    // The landed break may predate a mode switch: re-check creative here.
    if is_creative_player(world, &change.cause) {
        return;
    }
    let Some(inventory) = world.get_component::<sc_item::PlayerInventory>(entity) else {
        return;
    };
    let slot = inventory.selected_slot() as usize;
    let Some(stack) = inventory.get(slot) else {
        return;
    };
    if stack.is_empty() {
        return;
    }
    // Wrong-charge guard: the held item must be the one this break was earned with.
    let expected = change
        .break_context
        .as_ref()
        .and_then(|context| context.hand.item.as_deref());
    let live = item_registry
        .get(stack.runtime_id)
        .map(|definition| definition.name.to_string());
    if expected.is_none() || live.as_deref() != expected {
        log::debug!(
            "[item-wear] skipped: held item changed during mining (expected {expected:?}, live {live:?})"
        );
        return;
    }
    let Some(wear_table) = world.get_resource::<sc_packloader::item::WearTable>() else {
        return;
    };
    let Some(profile) = wear_table.resolve(stack.runtime_id) else {
        return;
    };
    let cause = match correct_tool_for_hand(
        block_registry,
        json_registry,
        change.previous.0,
        change
            .break_context
            .as_ref()
            .map(|context| context.hand.clone())
            .unwrap_or_default(),
    ) {
        // No `sc:mining` data: the block has no tool requirement, so charge the
        // cheap tier rather than accuse the player of using the wrong tool.
        ToolMatch::Matched | ToolMatch::Unknown => WearCause::MineCorrect,
        ToolMatch::Unmatched => WearCause::MineIncorrect,
    };
    let world_u128 = change.world_id.world_id.as_u128();
    let mut rng = sc_block::mining_drops::SplitMix64::new(sc_block::mining_drops::drop_seed(
        (world_u128 & 0xFFFF_FFFF_FFFF_FFFF) as u64,
        ((world_u128 >> 64) & 0xFFFF_FFFF_FFFF_FFFF) as u64,
        change.position.x,
        change.position.y,
        change.position.z,
        change.generation,
        change.request_id,
        slot as u64,
        cause.roll_kind(),
    ));
    let initial_damage = change
        .break_context
        .as_ref()
        .map(|context| context.initial_damage)
        .unwrap_or(stack.damage);
    let input = WearInput {
        profile,
        cause,
        current_damage: initial_damage,
    };
    let outcome = wear_decision(&input, &mut || rng.next_u64());
    let next = match outcome {
        // Chance miss: log it, otherwise a mis-authored `damage_chance` looks like
        // "durability never drops" with no server-side trace.
        WearOutcome::Skip => {
            log::debug!(
                "[item-wear] {} skipped (damage_chance {:?})",
                live.as_deref().unwrap_or("<unknown>"),
                profile.damage_chance
            );
            return;
        }
        WearOutcome::Wear { damage } => {
            if stack.damage >= damage {
                return;
            }
            log::debug!(
                "[item-wear] {} wore to {damage}/{} ({cause:?})",
                live.as_deref().unwrap_or("<unknown>"),
                profile.max_durability
            );
            let mut next = ItemStack { damage, ..stack };
            if next.tag.is_some() {
                next.set_facet(&sc_item::DamageFacet { damage: damage as i32 });
            }
            next
        }
        WearOutcome::Break { .. } => {
            log::debug!(
                "[item-wear] {} broke at {} ({cause:?})",
                live.as_deref().unwrap_or("<unknown>"),
                profile.max_durability
            );
            ItemStack::EMPTY
        }
    };
    let broke = next.is_empty();
    inventory.set(slot, next.clone());
    let Some(actor) = world.get_component::<sc_entity::MinecraftEntityId>(entity) else {
        return;
    };
    // Reliable facts: the inventory copy plus the held-item copy. Clients keep the
    // in-hand item separate from the inventory container, so a state change such as
    // wear must refresh both or the durability bar never moves.
    publisher.publish(UpdateInventorySlot {
        entity_id: actor.0,
        slot: slot as u8,
        stack: next.clone(),
    });
    publisher.publish(SyncHeldItem {
        entity_id: actor.0,
        slot: slot as u8,
        stack: next,
    });
    if broke {
        // An in-progress break cancels itself: `same_mining_target` compares the hand
        // snapshot, and the now-empty slot no longer matches.
        // Presentation only; a dropped cue is harmless (client predicts its own).
        // `data` carries the sound's extra data: block runtime ids are meaningless
        // for `random.break`, so send 0.
        publisher.publish(PlaySound {
            world_id: change.world_id.clone(),
            cue: SoundCue::ItemBreak,
            data: 0,
            x: change.position.x as f32 + 0.5,
            y: change.position.y as f32 + 0.5,
            z: change.position.z as f32 + 0.5,
        });
    }
}

/// 手持物品相对当前方块 `tools[]` 的匹配结论。
///
/// How the held item matches the current block's `tools[]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolMatch {
    /// 命中 `tools[]` 条目（正确工具）。
    ///
    /// Matched a `tools[]` entry (correct tool).
    Matched,
    /// 未命中任何条目（走 `default` 规则）。
    ///
    /// Matched no entry (took the `default` rule).
    Unmatched,
    /// 该方块没有 `sc:mining` 声明，无工具要求。
    ///
    /// The block declares no `sc:mining`, so it has no tool requirement.
    Unknown,
}

/// 重算手持物品的工具匹配（纯查询，不取可变锁）。
///
/// Recomputes the held item's tool match (pure query, no mutable locks).
fn correct_tool_for_hand(
    block_registry: &BlockStateRegistry,
    json_registry: &BlockJsonRegistry,
    previous_runtime: u32,
    hand: sc_block::mining_drops::HandSnapshot,
) -> ToolMatch {
    let Some(snapshot) = json_registry.get() else {
        return ToolMatch::Unknown;
    };
    let Some(state_id) =
        block_registry.by_runtime_id(sc_world::chunk::BlockRuntimeId(previous_runtime))
    else {
        return ToolMatch::Unknown;
    };
    let Some(compiled) = snapshot.mining_of(state_id.0) else {
        return ToolMatch::Unknown;
    };
    match sc_block::mining_drops::mining_decision(compiled, &hand) {
        sc_block::mining_drops::MineDecision::Mine { correct_tool, .. } => {
            if correct_tool {
                ToolMatch::Matched
            } else {
                ToolMatch::Unmatched
            }
        }
        // Denied at survival: no break should have landed; charge the cheap tier.
        sc_block::mining_drops::MineDecision::Deny => ToolMatch::Unknown,
    }
}

/// 确定性的破坏掉落掷骰（纯函数；无锁、无 IO、无网络）。
///
/// Deterministic break drop roll (pure; no locks, IO, or network).
///
/// 无方块掉落返回 `None`；掷骰结果为空返回 `Some(vec![])`；
/// 有待生成的物品栈返回 `Some(stacks)`。
/// Returns `None` for no block drops; `Some(vec![])` for an empty roll;
/// `Some(stacks)` for item stacks awaiting spawn.
///
/// - `block_registry`: 方块状态注册表
/// - `json_registry`: 方块 JSON 注册表
/// - `item_registry`: 物品注册表
/// - `previous_runtime`: 破坏前的运行时 id
/// - `fortune_level`: 时运等级
/// - `harvested`: 是否视为已收获
/// - `hand_item`: 手持物品名（如有）
/// - `change`: 方块变更事实（提供种子输入）
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
        let block_runtime_id = item_registry.block_runtime_id(runtime_id).unwrap_or(0);
        out.push(ItemStack::with_block(runtime_id, block_runtime_id, count));
    }
    Some(out)
}

/// 确定性的散射抖动（`x/z` 在 [-0.1, 0.1) 内）。
///
/// Deterministic scatter jitter (`x/z` in [-0.1, 0.1)).
///
/// - `change`: 方块变更事实（提供种子输入）
/// - `stack`: 物品栈（参与种子）
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
        0x7665_6C6F_6369_74, // truncated "velocit" seed component (frozen value)
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
    use crate::net::NetworkIntent;
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
                    "minecraft:loot": "loot_tables/blocks/ore.json"
                },
                "sc:protocol_runtime_ids": [32000]
            }
        }"#;
        let plain = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "example:plain", "states": {}},
                "components": {"minecraft:destructible_by_mining": {"value": 1.0}},
                "sc:protocol_runtime_ids": [32001]
            }
        }"#;
        let raws: Vec<(&str, &str)> = vec![
            ("blocks/minecraft/air.json", air),
            ("blocks/example/ore.json", ore),
            ("blocks/example/plain.json", plain),
        ];
        let budgets = BlockBundleBudgets::default();
        let mut refs: Vec<(&str, &[u8])> = raws.iter().map(|(p, b)| (*p, b.as_bytes())).collect();
        refs.sort_by(|a, b| a.0.cmp(b.0));
        let files = refs
            .iter()
            .map(|(p, b)| parse_block_file("test", p, b).unwrap())
            .collect();
        let bundle = BlockJsonBundle {
            schema_version: 1,
            network_id_mode: "hashed".to_string(),
            fingerprint: fingerprint_bundle(&refs, "hashed"),
            files,
            loot_tables: std::iter::once((
                "loot_tables/blocks/ore.json".to_string(),
                serde_json::json!({
                    "pools": [
                        {"rolls": 1, "entries": [
                            {"type": "item", "name": "minecraft:cobblestone", "count": 1}
                        ]},
                        {"rolls": 1, "entries": [
                            {"type": "item", "name": "minecraft:coal", "chance": 0.25, "count": [
                                {"value": 1, "chance": 0.75}, {"value": 2, "chance": 0.25}
                            ]}
                        ]}
                    ]
                }),
            ))
            .collect(),
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
        let item_registry = sc_item::ItemRegistry::new();
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
        world.insert_resource(BreakSettlementReceipts::default());
        let mut system = block_changed_to_outbox.into_system();
        system.run(&world);
        let receipts = world.get_resource::<BreakSettlementReceipts>().unwrap();
        assert_eq!(receipts.pending.len(), 1);
        assert!(receipts.contains(100));
        assert!(receipts.contains(101));
        assert_eq!(receipts.pending[0].stacks, expected);
        drop(receipts);
        let sent = world.get_resource_mut::<NetworkOutbox>().unwrap().drain();
        assert!(sent.iter().any(|intent| matches!(intent, NetworkIntent::SpawnItemEntity(p) if p.world_id == other.world_id)));
        assert!(!sent.iter().any(|intent| matches!(intent, NetworkIntent::SpawnItemEntity(p) if p.world_id == change.world_id)));
        system.run(&world);
        assert_eq!(
            world.get_resource::<BreakSettlementReceipts>().unwrap().pending[0].stacks,
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
            initial_damage: 0,
        });
        let mut facts = BlockChangedQueue::default();
        facts.push(change);
        world.insert_resource(json);
        world.insert_resource(blocks);
        world.insert_resource(items);
        world.insert_resource(facts);
        world.insert_resource(ItemDropStore::default());
        world.insert_resource(NetworkOutbox::default());
        world.insert_resource(BreakSettlementReceipts::default());
        block_changed_to_outbox.into_system().run(&world);
        assert!(world.get_resource_mut::<NetworkOutbox>().unwrap().drain().iter().any(|intent|
            matches!(intent, NetworkIntent::SpawnItemEntity(p) if p.stack.runtime_id == 1)));
    }

    /// Breaking a tracked furnace drops its container contents through the
    /// same pending path as block drops, destroys the identity, and never
    /// double-drops on retry; creative breaks destroy without drops.
    #[test]
    fn tracked_break_drops_process_contents_once() {
        use sc_block::workstation::SlotRole;
        use sc_ecs::system::{IntoSystem, System};
        let (json, blocks, items) = compile_test_bundle();
        let runtime = blocks
            .state(blocks.state_of("example:ore", &[]).unwrap())
            .unwrap()
            .hash;
        fn track(
            world: &World,
            world_id: &sc_world::manager::MinecraftWorldId,
            input: (u16, u16),
            fuel: (u16, u16),
        ) {
            let entity = world.spawn((
                crate::block_entity::BlockEntityRef {
                    world: world_id.clone(),
                    pos: (3, 64, 5),
                    kind: crate::block_entity::BlockEntityKind::Furnace(
                        sc_recipe::StationKind::Furnace,
                    ),
                },
                crate::inventory::crafting::BlockProcessState::new(
                    sc_recipe::StationKind::Furnace,
                    vec![SlotRole::Input, SlotRole::Fuel, SlotRole::Result],
                    200,
                ),
            ));
            let process = world
                .get_component::<crate::inventory::crafting::BlockProcessState>(&entity)
                .expect("刚挂载");
            process.set_slot(SlotRole::Input, ItemStack::new(input.0, input.1));
            process.set_slot(SlotRole::Fuel, ItemStack::new(fuel.0, fuel.1));
            // Track it (the hook is index-driven; spawning alone is not enough).
            world
                .get_resource_mut::<crate::block_entity::BlockEntityIndex>()
                .expect("index")
                .insert(world_id.clone(), (3, 64, 5), entity);
        }
        fn coal_spawns(world: &World) -> Vec<(u16, u16)> {
            world
                .get_resource_mut::<NetworkOutbox>()
                .unwrap()
                .drain()
                .into_iter()
                .filter_map(|intent| match intent {
                    NetworkIntent::SpawnItemEntity(payload) => (payload.stack.runtime_id == 2)
                        .then_some((payload.stack.runtime_id, payload.stack.count)),
                    _ => None,
                })
                .collect()
        }
        let world = World::new();
        world.insert_resource(json);
        world.insert_resource(blocks);
        world.insert_resource(items);
        world.insert_resource(ItemDropStore::default());
        world.insert_resource(NetworkOutbox::default());
        world.insert_resource(BlockChangedQueue::default());
        world.insert_resource(BreakSettlementReceipts::default());
        world.insert_resource(crate::block_entity::BlockEntityIndex::default());
        // Survival break: coal input x3 + coal fuel x1 join the ore drops.
        let change = fake_change(runtime, 200);
        let survival_world = change.world_id.clone();
        track(&world, &change.world_id, (2, 3), (2, 1));
        world
            .get_resource_mut::<BlockChangedQueue>()
            .unwrap()
            .push(change);
        block_changed_to_outbox.into_system().run(&world);
        let spawns = coal_spawns(&world);
        assert!(spawns.contains(&(2, 3)), "input coal drops: {spawns:?}");
        assert!(spawns.contains(&(2, 1)), "fuel coal drops: {spawns:?}");
        // Identity destroyed with the block.
        let index = world
            .get_resource::<crate::block_entity::BlockEntityIndex>()
            .unwrap();
        assert!(index.get(&survival_world, (3, 64, 5)).is_none());
        drop(index);
        // Retry (same operation) never re-drops: claim dedups and the
        // entity is already gone.
        block_changed_to_outbox.into_system().run(&world);
        assert!(coal_spawns(&world).is_empty(), "no double drops");
        // Creative break: destroys without dropping.
        let mut creative = fake_change(runtime, 201);
        track(&world, &creative.world_id, (2, 3), (2, 1));
        creative.break_context = Some(sc_block::write::BlockBreakContext {
            hand: sc_block::mining_drops::HandSnapshot {
                item: None,
                efficiency_level: 0,
                fortune_level: 0,
            },
            creative: true,
            initial_damage: 0,
        });
        let creative_world = creative.world_id.clone();
        world
            .get_resource_mut::<BlockChangedQueue>()
            .unwrap()
            .push(creative);
        block_changed_to_outbox.into_system().run(&world);
        assert!(coal_spawns(&world).is_empty(), "creative drops nothing");
        let index = world
            .get_resource::<crate::block_entity::BlockEntityIndex>()
            .unwrap();
        assert!(index.get(&creative_world, (3, 64, 5)).is_none());
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
                if let NetworkIntent::SpawnItemEntity(payload) = intent {
                    total += payload.stack.count;
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
                .filter(|intent| matches!(intent, NetworkIntent::SpawnItemEntity(_)))
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
            let _idx = json_registry
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
        let mut receipts = BreakSettlementReceipts::default();
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
            &mut crate::net::IntentPublisher::new(&mut outbox),
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
                    NetworkIntent::SpawnItemEntity(payload) => Some(payload.stack.count),
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
                &mut crate::net::IntentPublisher::new(&mut outbox),
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
            &mut crate::net::IntentPublisher::new(&mut outbox),
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

    #[test]
    fn lit_furnace_break_drops_stateless_furnace() {
        let air = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        let furnace = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:furnace", "states": {}},
                "components": {
                    "minecraft:destructible_by_mining": {"value": 3.5},
                    "sc:mining": {
                        "formula_version": 1,
                        "base_time_seconds": 3.5,
                        "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0}
                    },
                    "minecraft:loot": "loot_tables/blocks/furnace.json"
                },
                "sc:protocol_runtime_ids": [15100]
            }
        }"#;
        let lit_furnace = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:lit_furnace", "states": {}},
                "components": {
                    "minecraft:destructible_by_mining": {"value": 3.5},
                    "sc:mining": {
                        "formula_version": 1,
                        "base_time_seconds": 3.5,
                        "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0}
                    },
                    "sc:furnace": {
                        "kind": "furnace",
                        "cook_ticks": 200,
                        "fuel": true,
                        "lit": true,
                        "lit_variant": "minecraft:furnace"
                    },
                    "minecraft:loot": "loot_tables/blocks/furnace.json"
                },
                "sc:protocol_runtime_ids": [15101]
            }
        }"#;
        let raws: Vec<(&str, &str)> = vec![
            ("blocks/minecraft/air.json", air),
            ("blocks/minecraft/furnace.json", furnace),
            ("blocks/minecraft/lit_furnace.json", lit_furnace),
        ];
        let budgets = BlockBundleBudgets::default();
        let mut refs: Vec<(&str, &[u8])> = raws.iter().map(|(p, b)| (*p, b.as_bytes())).collect();
        refs.sort_by(|a, b| a.0.cmp(b.0));
        let files = refs
            .iter()
            .map(|(p, b)| parse_block_file("test", p, b).unwrap())
            .collect();
        let bundle = BlockJsonBundle {
            schema_version: 1,
            network_id_mode: "hashed".to_string(),
            fingerprint: fingerprint_bundle(&refs, "hashed"),
            files,
            loot_tables: [(
                "loot_tables/blocks/furnace.json".to_string(),
                serde_json::json!({
                    "pools": [{"rolls": 1, "entries": [
                        {"type": "item", "name": "minecraft:furnace", "count": 1}
                    ]}]
                }),
            )]
            .into_iter()
            .collect(),
        };
        let allow = |id: &str| matches!(id, "minecraft:furnace" | "minecraft:lit_furnace");
        let (snapshot, _) =
            compile_bundle(&bundle, "test", &budgets, &|_| None, &allow).expect("应编译");
        let block_registry = BlockStateRegistry::from_block_snapshot(&snapshot);
        let json_registry = BlockJsonRegistry::new();
        json_registry.publish(std::sync::Arc::new(snapshot));
        let snap = json_registry.get().unwrap();
        let furnace_state_idx = snap.default_state_idx("minecraft:furnace").unwrap();
        let furnace_hash = block_registry
            .runtime_id(sc_block::state::BlockStateId(furnace_state_idx))
            .unwrap()
            .0;
        let lit_state_idx = snap.default_state_idx("minecraft:lit_furnace").unwrap();
        let lit_hash = block_registry
            .runtime_id(sc_block::state::BlockStateId(lit_state_idx))
            .unwrap()
            .0;

        let item_registry = sc_item::ItemRegistry::new();
        item_registry.upsert(
            sc_item::ItemDefinition::new(61, "minecraft:furnace")
                .max_stack_size(64)
                .block(furnace_hash),
        );
        item_registry.upsert(
            sc_item::ItemDefinition::new(62, "minecraft:lit_furnace")
                .max_stack_size(64)
                .block(lit_hash),
        );

        let change = fake_change(lit_hash, 10);
        let drops = roll_break_drops(
            &block_registry,
            &json_registry,
            &item_registry,
            lit_hash,
            0,
            true,
            None,
            &change,
        )
        .expect("应产生掉落");

        assert_eq!(drops.len(), 1);
        let stack = &drops[0];
        // Key assertion: the dropped runtime_id is 61 (minecraft:furnace), never
        // 62 (minecraft:lit_furnace).
        assert_eq!(stack.runtime_id, 61, "正在燃烧的熔炉破坏后必须掉落无状态的基础熔炉物品");
        assert_eq!(stack.count, 1);
        assert_eq!(stack.block_runtime_id, furnace_hash, "掉落物必须携带基础熔炉默认方块运行时 ID");
        assert_ne!(stack.block_runtime_id, lit_hash, "掉落物绝不能携带正在燃烧的熔炉方块状态");
    }

    #[test]
    fn lit_furnace_full_break_flow_publishes_stateless_item_and_contents() {
        use sc_block::workstation::SlotRole;
        use sc_ecs::system::{IntoSystem, System};

        let air = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        let furnace = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:furnace", "states": {}},
                "components": {
                    "minecraft:destructible_by_mining": {"value": 3.5},
                    "sc:mining": {
                        "formula_version": 1,
                        "base_time_seconds": 3.5,
                        "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0}
                    },
                    "minecraft:loot": "loot_tables/blocks/furnace.json"
                },
                "sc:protocol_runtime_ids": [15100]
            }
        }"#;
        let lit_furnace = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:lit_furnace", "states": {}},
                "components": {
                    "minecraft:destructible_by_mining": {"value": 3.5},
                    "sc:mining": {
                        "formula_version": 1,
                        "base_time_seconds": 3.5,
                        "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0}
                    },
                    "sc:furnace": {
                        "kind": "furnace",
                        "cook_ticks": 200,
                        "fuel": true,
                        "lit": true,
                        "lit_variant": "minecraft:furnace"
                    },
                    "minecraft:loot": "loot_tables/blocks/furnace.json"
                },
                "sc:protocol_runtime_ids": [15101]
            }
        }"#;
        let raws: Vec<(&str, &str)> = vec![
            ("blocks/minecraft/air.json", air),
            ("blocks/minecraft/furnace.json", furnace),
            ("blocks/minecraft/lit_furnace.json", lit_furnace),
        ];
        let budgets = BlockBundleBudgets::default();
        let mut refs: Vec<(&str, &[u8])> = raws.iter().map(|(p, b)| (*p, b.as_bytes())).collect();
        refs.sort_by(|a, b| a.0.cmp(b.0));
        let files = refs
            .iter()
            .map(|(p, b)| parse_block_file("test", p, b).unwrap())
            .collect();
        let bundle = BlockJsonBundle {
            schema_version: 1,
            network_id_mode: "hashed".to_string(),
            fingerprint: fingerprint_bundle(&refs, "hashed"),
            files,
            loot_tables: [(
                "loot_tables/blocks/furnace.json".to_string(),
                serde_json::json!({
                    "pools": [{"rolls": 1, "entries": [
                        {"type": "item", "name": "minecraft:furnace", "count": 1}
                    ]}]
                }),
            )]
            .into_iter()
            .collect(),
        };
        let allow = |id: &str| matches!(id, "minecraft:furnace" | "minecraft:lit_furnace" | "minecraft:coal");
        let (snapshot, _) =
            compile_bundle(&bundle, "test", &budgets, &|_| None, &allow).expect("应编译");
        let block_registry = BlockStateRegistry::from_block_snapshot(&snapshot);
        let json_registry = BlockJsonRegistry::new();
        json_registry.publish(std::sync::Arc::new(snapshot));
        let snap = json_registry.get().unwrap();
        let furnace_state_idx = snap.default_state_idx("minecraft:furnace").unwrap();
        let furnace_hash = block_registry
            .runtime_id(sc_block::state::BlockStateId(furnace_state_idx))
            .unwrap()
            .0;
        let lit_state_idx = snap.default_state_idx("minecraft:lit_furnace").unwrap();
        let lit_hash = block_registry
            .runtime_id(sc_block::state::BlockStateId(lit_state_idx))
            .unwrap()
            .0;

        let item_registry = sc_item::ItemRegistry::new();
        item_registry.upsert(
            sc_item::ItemDefinition::new(61, "minecraft:furnace")
                .max_stack_size(64)
                .block(furnace_hash),
        );
        item_registry.upsert(
            sc_item::ItemDefinition::new(62, "minecraft:lit_furnace")
                .max_stack_size(64)
                .block(lit_hash),
        );
        item_registry.upsert(sc_item::ItemDefinition::new(263, "minecraft:coal").max_stack_size(64));

        let world = World::new();
        world.insert_resource(json_registry);
        world.insert_resource(block_registry);
        world.insert_resource(item_registry);
        world.insert_resource(ItemDropStore::default());
        world.insert_resource(NetworkOutbox::default());
        world.insert_resource(BlockChangedQueue::default());
        world.insert_resource(BreakSettlementReceipts::default());
        world.insert_resource(crate::block_entity::BlockEntityIndex::default());

        let change = fake_change(lit_hash, 300);
        let pos = (change.position.x, change.position.y, change.position.z);
        // Attach furnace slot data (the fuel slot holds 5 coal).
        let entity = world.spawn((
            crate::block_entity::BlockEntityRef {
                world: change.world_id.clone(),
                pos,
                kind: crate::block_entity::BlockEntityKind::Furnace(sc_recipe::StationKind::Furnace),
            },
            crate::inventory::crafting::BlockProcessState::new(
                sc_recipe::StationKind::Furnace,
                vec![SlotRole::Input, SlotRole::Fuel, SlotRole::Result],
                200,
            ),
        ));
        let process = world
            .get_component::<crate::inventory::crafting::BlockProcessState>(&entity)
            .unwrap();
        process.set_slot(SlotRole::Fuel, ItemStack::new(263, 5));
        world
            .get_resource_mut::<crate::block_entity::BlockEntityIndex>()
            .unwrap()
            .insert(change.world_id.clone(), pos, entity);

        world
            .get_resource_mut::<BlockChangedQueue>()
            .unwrap()
            .push(change);
        block_changed_to_outbox.into_system().run(&world);

        let outbox_intents = world.get_resource_mut::<NetworkOutbox>().unwrap().drain();
        let spawned: Vec<(u16, u16)> = outbox_intents
            .into_iter()
            .filter_map(|intent| match intent {
                NetworkIntent::SpawnItemEntity(payload) => {
                    assert_ne!(
                        payload.stack.block_runtime_id, lit_hash,
                        "掉落物绝不能携带正在燃烧的熔炉状态"
                    );
                    if payload.stack.runtime_id == 61 {
                        assert_eq!(
                            payload.stack.block_runtime_id, furnace_hash,
                            "熔炉掉落物必须携带基础方块默认状态"
                        );
                    }
                    if payload.stack.runtime_id == 263 {
                        assert_eq!(
                            payload.stack.block_runtime_id, 0,
                            "非方块物品 block_runtime_id 应为 0"
                        );
                    }
                    Some((payload.stack.runtime_id, payload.stack.count))
                }
                _ => None,
            })
            .collect();

        // Must drop 1 stateless furnace (61) and 5 fuel coal (263), never lit_furnace (62).
        assert!(spawned.contains(&(61, 1)), "掉落中必须包含基础无状态熔炉: {spawned:?}");
        assert!(spawned.contains(&(263, 5)), "掉落中必须包含槽位残留燃料: {spawned:?}");
        assert!(!spawned.iter().any(|(id, _)| *id == 62), "掉落物中绝不能包含带状态的 lit_furnace");
    }

    /// 工具磨损测试世界：玩家实体 + 库存 + 耐久表 + 全部结算资源。
    ///
    /// Tool-wear test world: player entity, inventory, wear table and every
    /// settlement resource.
    fn wear_world(
        durability: u32,
        costs: sc_packloader::item::DurabilityCosts,
        stack_damage: u32,
    ) -> (
        World,
        sc_ecs::entity::EntityId,
        sc_block::write::BlockChanged,
    ) {
        let (json_registry, block_registry, item_registry) = compile_test_bundle();
        item_registry.upsert(
            sc_item::ItemDefinition::new(3, "minecraft:iron_pickaxe").max_stack_size(1),
        );
        let runtime = block_registry
            .state(block_registry.state_of("example:ore", &[]).unwrap())
            .unwrap()
            .hash;

        let world = World::new();
        world.insert_resource(json_registry);
        world.insert_resource(block_registry);
        world.insert_resource(item_registry);
        world.insert_resource(ItemDropStore::default());
        world.insert_resource(NetworkOutbox::default());
        world.insert_resource(BlockChangedQueue::default());
        world.insert_resource(BreakSettlementReceipts::default());
        world.insert_resource(crate::block_entity::BlockEntityIndex::default());
        world.insert_resource(sc_packloader::item::WearTable::default());
        world.get_resource::<sc_packloader::item::WearTable>()
            .unwrap()
            .insert(
                3,
                sc_packloader::item::WearProfile {
                    max_durability: durability,
                    damage_chance: sc_packloader::item::wear::DEFAULT_WEAR_CHANCE,
                    costs,
                },
            );

        let inventory = sc_item::PlayerInventory::new(36);
        let _ = inventory.set(
            0,
            ItemStack {
                damage: stack_damage,
                ..ItemStack::new(3, 1)
            },
        );
        let entity = world.spawn((inventory, sc_entity::MinecraftEntityId(7)));

        let mut change = fake_change(runtime, 300);
        change.cause = sc_block::write::BlockChangeCause::Player(entity);
        change.break_context = Some(sc_block::write::BlockBreakContext {
            hand: sc_block::mining_drops::HandSnapshot {
                item: Some("minecraft:iron_pickaxe".to_string()),
                efficiency_level: 0,
                fortune_level: 0,
            },
            creative: false,
            initial_damage: stack_damage,
        });
        (world, entity, change)
    }

    fn run_settlement(world: &World, change: sc_block::write::BlockChanged) {
        use sc_ecs::system::{IntoSystem, System};
        world
            .get_resource_mut::<BlockChangedQueue>()
            .unwrap()
            .push(change);
        block_changed_to_outbox.into_system().run(world);
    }

    fn slot_damage(world: &World, entity: &sc_ecs::entity::EntityId, slot: usize) -> Option<u32> {
        let inventory = world.get_component::<sc_item::PlayerInventory>(entity)?;
        inventory.get(slot).map(|stack| stack.damage)
    }

    #[test]
    fn survival_break_charges_correct_tool_wear_once() {
        // Ore's `tools[]` lists the iron pickaxe: correct tool costs 1 point.
        let (world, entity, change) =
            wear_world(60, sc_packloader::item::DurabilityCosts::default(), 0);
        run_settlement(&world, change.clone());
        assert_eq!(slot_damage(&world, &entity, 0), Some(1), "correct tool wears 1");
        // Same OperationId never charges twice.
        run_settlement(&world, change);
        assert_eq!(slot_damage(&world, &entity, 0), Some(1), "idempotent");
        let intents = world.get_resource_mut::<NetworkOutbox>().unwrap().drain();
        let synced = intents
            .iter()
            .filter_map(|intent| match intent {
                crate::net::NetworkIntent::UpdateInventorySlot(payload) if payload.slot == 0 => {
                    Some(payload.stack.damage)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(synced, vec![1], "one slot fact with the new damage");
        // The client keeps a separate held-item copy: it must be refreshed too,
        // otherwise the durability bar does not move.
        let held = intents
            .iter()
            .filter_map(|intent| match intent {
                crate::net::NetworkIntent::SyncHeldItem(payload) if payload.slot == 0 => {
                    Some(payload.stack.damage)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(held, vec![1], "held-item fact carries the same damage");
    }

    #[test]
    fn tool_break_clears_slot_and_cues_item_break() {
        // 59 + 2 (incorrect tier is irrelevant here: correct costs 2 in this fixture)
        // reaching the cap breaks the tool and clears the slot.
        let costs = sc_packloader::item::DurabilityCosts {
            mine_correct: 2,
            ..sc_packloader::item::DurabilityCosts::default()
        };
        let (world, entity, change) = wear_world(60, costs, 59);
        run_settlement(&world, change);
        let inventory = world.get_component::<sc_item::PlayerInventory>(&entity).unwrap();
        assert!(inventory.get(0).unwrap().is_empty(), "broken tool is cleared");
        drop(inventory);
        let intents = world.get_resource_mut::<NetworkOutbox>().unwrap().drain();
        assert!(
            intents.iter().any(|intent| matches!(
                intent,
                crate::net::NetworkIntent::UpdateInventorySlot(payload)
                    if payload.slot == 0 && payload.stack.is_empty()
            )),
            "authoritative empty slot is synced"
        );
        assert!(
            intents.iter().any(|intent| matches!(
                intent,
                crate::net::NetworkIntent::PlaySound(payload)
                    if payload.cue == SoundCue::ItemBreak
            )),
            "break cue is published"
        );
    }

    #[test]
    fn real_tool_declaration_charges_every_break() {
        // Regression: the shipped tool JSON must actually wear on every break.
        // `damage_chance {0,100}` used to halve the rate (a probability range, not
        // "always"), which looked like "durability never drops".
        use sc_block::item_wear::{wear_decision, WearCause, WearInput, WearOutcome};
        use sc_block::mining_drops::SplitMix64;

        let raw = include_str!(
            "../../../../version_packs/[SC原版版本包] Vanilla-1.26.40/behavior_packs/vanilla/items/iron_pickaxe.json"
        );
        let parsed: serde_json::Value = serde_json::from_str(raw).expect("json");
        let durability = &parsed["minecraft:item"]["components"]["minecraft:durability"];
        assert!(
            durability.get("damage_chance").is_none(),
            "tools must rely on the default 100% chance"
        );
        let profile = sc_packloader::item::WearProfile {
            max_durability: durability["max_durability"].as_u64().unwrap() as u32,
            damage_chance: sc_packloader::item::wear::DEFAULT_WEAR_CHANCE,
            costs: sc_packloader::item::DurabilityCosts::default(),
        };
        let input = WearInput {
            profile,
            cause: WearCause::MineCorrect,
            current_damage: 0,
        };
        for seed in 0..64u64 {
            let mut rng = SplitMix64::new(seed);
            let outcome = wear_decision(&input, &mut || rng.next_u64());
            assert!(
                matches!(outcome, WearOutcome::Wear { damage: 1 }),
                "seed {seed} skipped wear: {outcome:?}"
            );
        }
    }

    #[test]
    fn wrong_held_item_is_never_charged() {
        // The break was earned with an iron pickaxe but the player now holds coal.
        let (world, entity, change) = wear_world(60, sc_packloader::item::DurabilityCosts::default(), 0);
        let inventory = world.get_component::<sc_item::PlayerInventory>(&entity).unwrap();
        let _ = inventory.set(0, ItemStack::new(2, 5));
        drop(inventory);
        run_settlement(&world, change);
        assert_eq!(
            slot_damage(&world, &entity, 0),
            Some(0),
            "the wrong item is not charged"
        );
    }
}
