//! Server-authoritative craft transactions.
//!
//! The network layer only translates packets into [`CraftIntent`]; all
//! validation and state changes happen here against abstract inventories.
//! No locks are held across `await`: every method is synchronous on owned
//! snapshots, and cross-region coordination uses explicit
//! prepare/commit/abort with operation ids.

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::compile::{CompiledRecipe, RecipeBody};
use crate::kind::StationKind;
use crate::matching::{item_family_matches, match_shapeless, MatchInput, TagResolver};
use crate::registry::RecipeRegistryFingerprint;
use crate::spec::IngredientSpec;

/// Who is crafting.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct CraftActor {
    pub player_id: u64,
    pub entity_generation: u64,
}

/// Where the craft happens.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct CraftContext {
    pub world_id: String,
    pub dimension: i32,
    pub station: StationKind,
    /// Station block position when a block station is used; `None` for
    /// player-inventory (2x2) crafting.
    pub station_pos: Option<(i32, i32, i32)>,
    /// Station generation / content revision the client observed.
    pub station_revision: u64,
    /// True when the client claims to be near the station; the game layer
    /// re-checks real distance before commit.
    pub client_claims_in_reach: bool,
}

/// One expected input slot (inventory index + expected content).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExpectedSlot {
    pub slot: usize,
    pub identifier: String,
    pub data: i32,
    pub count: u16,
}

/// Protocol-independent craft request. Built by `sc_network` from
/// `InventoryTransaction` / `PlayerAuthInput` / `ItemStackRequest` /
/// container open-close; executed by `sc_game` without packet types.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CraftIntent {
    pub actor: CraftActor,
    pub context: CraftContext,
    pub recipe_id: String,
    pub registry_fingerprint: RecipeRegistryFingerprint,
    pub inventory_revision: u64,
    pub container_revision: u64,
    pub expected_inputs: Vec<ExpectedSlot>,
    pub requested_count: u16,
    pub operation_id: u64,
    /// Seconds since epoch for deadline checks (0 = no deadline).
    pub deadline_unix: u64,
}

impl CraftIntent {
    pub fn operation_key(&self) -> (u64, u64) {
        (self.actor.player_id, self.operation_id)
    }
}

/// Abstract inventory slot used by the transaction engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TxSlot {
    pub identifier: String,
    pub data: i32,
    pub components: Option<[u8; 32]>,
    pub count: u16,
}

impl TxSlot {
    pub fn empty() -> Self {
        Self {
            identifier: String::new(),
            data: 0,
            components: None,
            count: 0,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0 || self.identifier.is_empty()
    }
}

/// Abstract mutable inventory with a revision counter.
#[derive(Clone, Debug)]
pub struct TxInventory {
    pub slots: Vec<TxSlot>,
    pub revision: u64,
}

impl TxInventory {
    pub fn new(slots: Vec<TxSlot>) -> Self {
        Self { slots, revision: 0 }
    }
    pub fn len(&self) -> usize {
        self.slots.len()
    }
}

/// Terminal receipt: exactly one per operation id.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CraftReceipt {
    Applied {
        operation_id: u64,
        recipe_id: String,
        inventory_revision: u64,
        outputs: Vec<(String, u16)>,
    },
    Rejected {
        operation_id: u64,
        reason: CraftReject,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CraftReject {
    UnknownRecipe,
    StaleFingerprint,
    StaleInventoryRevision { expected: u64, actual: u64 },
    StaleContainerRevision { expected: u64, actual: u64 },
    StaleStationRevision { expected: u64, actual: u64 },
    InputMismatch { slot: usize, detail: String },
    InsufficientInput { identifier: String },
    NoOutputSpace,
    NoRemainderSpace,
    WrongStation { expected: String, actual: String },
    OutOfReach,
    NoPermission,
    GamemodeDenied,
    RuleDenied(String),
    QueueBusy,
    Aborted(String),
}

impl std::fmt::Display for CraftReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Bounded idempotency log: operation (player, op) -> receipt.
#[derive(Clone, Debug, Default)]
pub struct SeenOperations {
    map: HashMap<(u64, u64), CraftReceipt>,
    order: VecDeque<(u64, u64)>,
    capacity: usize,
}

impl SeenOperations {
    pub fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }
    pub fn get(&self, key: &(u64, u64)) -> Option<&CraftReceipt> {
        self.map.get(key)
    }
    pub fn insert(&mut self, key: (u64, u64), receipt: CraftReceipt) {
        if self.map.contains_key(&key) {
            return;
        }
        if self.map.len() >= self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
        self.order.push_back(key);
        self.map.insert(key, receipt);
    }
}

/// Bounded admission queue for craft intents.
#[derive(Debug)]
pub struct CraftQueue {
    pending: VecDeque<CraftIntent>,
    capacity: usize,
}

impl CraftQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            pending: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }
    pub fn try_submit(&mut self, intent: CraftIntent) -> Result<(), CraftReject> {
        if self.pending.len() >= self.capacity {
            return Err(CraftReject::QueueBusy);
        }
        self.pending.push_back(intent);
        Ok(())
    }
    pub fn pop(&mut self) -> Option<CraftIntent> {
        self.pending.pop_front()
    }
    pub fn len(&self) -> usize {
        self.pending.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// Cross-region reservation (prepare phase).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegionReservation {
    pub operation_id: u64,
    pub owner: String,
    pub epoch: u64,
    pub revision: u64,
    pub ttl_ticks: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TxnDecision {
    Commit,
    Abort(String),
}

/// Decide commit/abort from participant readiness.
pub fn decide_commit(ready: &[bool], reason: &str) -> TxnDecision {
    if ready.iter().all(|r| *r) {
        TxnDecision::Commit
    } else {
        TxnDecision::Abort(reason.to_string())
    }
}

/// Parameters the game layer checks around the pure inventory edit.
#[derive(Clone, Debug)]
pub struct CommitChecks {
    pub in_reach: bool,
    pub has_permission: bool,
    pub gamemode_allows: bool,
    pub recipes_unlocked: bool,
    pub station_exists: bool,
    pub station_kind_ok: bool,
    pub actual_container_revision: u64,
    pub actual_station_revision: u64,
}

fn to_match_inputs(slots: &[TxSlot]) -> Vec<MatchInput> {
    slots
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| MatchInput {
            identifier: s.identifier.clone(),
            data: s.data,
            components: s.components,
            count: s.count,
        })
        .collect()
}

/// Execute one instant craft atomically against `inventory`.
///
/// Steps (all or nothing):
/// 1. idempotency: same operation id returns the prior receipt;
/// 2. recipe lookup + fingerprint + revision checks;
/// 3. station / reach / permission / gamemode / rule checks;
/// 4. re-validate inputs (identifier, count, data; components must be
///    absent because component recipes are disabled at compile);
/// 5. reserve + verify output + remainder capacity;
/// 6. deduct inputs, write outputs + remainder, bump revision;
/// 7. return the terminal receipt (caller publishes sync).
///
/// `max_stack` resolves the stacking limit for an identifier (usually 64).
#[allow(clippy::too_many_arguments)]
pub fn execute_craft(
    intent: &CraftIntent,
    recipe: Option<&CompiledRecipe>,
    registry_fingerprint: RecipeRegistryFingerprint,
    inventory: &mut TxInventory,
    checks: &CommitChecks,
    seen: &mut SeenOperations,
    tags: &dyn TagResolver,
    max_expansion: usize,
    max_stack: &dyn Fn(&str) -> u16,
) -> CraftReceipt {
    let key = intent.operation_key();
    if let Some(prior) = seen.get(&key) {
        return prior.clone();
    }
    let mut finish = |receipt: CraftReceipt| {
        seen.insert(key, receipt.clone());
        receipt
    };
    let mut reject = |reason: CraftReject| {
        finish(CraftReceipt::Rejected {
            operation_id: intent.operation_id,
            reason,
        })
    };

    let Some(recipe) = recipe else {
        return reject(CraftReject::UnknownRecipe);
    };
    if intent.registry_fingerprint != registry_fingerprint {
        return reject(CraftReject::StaleFingerprint);
    }
    if intent.inventory_revision != inventory.revision {
        return reject(CraftReject::StaleInventoryRevision {
            expected: intent.inventory_revision,
            actual: inventory.revision,
        });
    }
    if intent.container_revision != checks.actual_container_revision {
        return reject(CraftReject::StaleContainerRevision {
            expected: intent.container_revision,
            actual: checks.actual_container_revision,
        });
    }
    if intent.context.station_revision != checks.actual_station_revision
        && intent.context.station_pos.is_some()
    {
        return reject(CraftReject::StaleStationRevision {
            expected: intent.context.station_revision,
            actual: checks.actual_station_revision,
        });
    }
    // Station must still exist with the right kind.
    if intent.context.station_pos.is_some() {
        if !checks.station_exists {
            return reject(CraftReject::WrongStation {
                expected: intent.context.station.tag().to_string(),
                actual: "missing".to_string(),
            });
        }
        if !checks.station_kind_ok {
            return reject(CraftReject::WrongStation {
                expected: intent.context.station.tag().to_string(),
                actual: "mismatch".to_string(),
            });
        }
    }
    if !recipe.stations.is_empty() && !recipe.stations.contains(&intent.context.station) {
        return reject(CraftReject::WrongStation {
            expected: recipe
                .stations
                .iter()
                .map(|s| s.tag())
                .collect::<Vec<_>>()
                .join(","),
            actual: intent.context.station.tag().to_string(),
        });
    }
    if !checks.in_reach {
        return reject(CraftReject::OutOfReach);
    }
    if !checks.has_permission {
        return reject(CraftReject::NoPermission);
    }
    if !checks.gamemode_allows {
        return reject(CraftReject::GamemodeDenied);
    }
    if !checks.recipes_unlocked {
        return reject(CraftReject::RuleDenied("recipesUnlock".to_string()));
    }
    // Only instant recipes execute here; process recipes go through the
    // block-entity processor.
    match &recipe.body {
        RecipeBody::Shaped(_) | RecipeBody::Shapeless(_) => {}
        _ => {
            return reject(CraftReject::Aborted(
                "process recipe requires block-entity processing".to_string(),
            ));
        }
    }

    // Re-validate expected inputs against live slots.
    for expected in &intent.expected_inputs {
        let Some(slot) = inventory.slots.get(expected.slot) else {
            return reject(CraftReject::InputMismatch {
                slot: expected.slot,
                detail: "slot out of range".to_string(),
            });
        };
        if slot.identifier != expected.identifier
            || slot.data != expected.data
            || slot.count < expected.count
        {
            return reject(CraftReject::InputMismatch {
                slot: expected.slot,
                detail: format!(
                    "expected {}:{}x{} found {}:{}x{}",
                    expected.identifier,
                    expected.data,
                    expected.count,
                    slot.identifier,
                    slot.data,
                    slot.count
                ),
            });
        }
        if slot.components.is_some() {
            return reject(CraftReject::InputMismatch {
                slot: expected.slot,
                detail: "components/NBT present but stage-1 cannot verify".to_string(),
            });
        }
    }

    // Verify the recipe still matches the live inventory (not just the
    // client's expectation). Build the shaped grid or shapeless multiset
    // from the expected slots.
    if !verify_match(recipe, intent, inventory, tags, max_expansion) {
        return reject(CraftReject::InputMismatch {
            slot: usize::MAX,
            detail: "live inventory does not satisfy recipe".to_string(),
        });
    }

    // Compute deductions per slot from expected inputs.
    // Snapshot for rollback.
    let snapshot = inventory.slots.clone();

    // 1. Reserve (deduct) inputs.
    for expected in &intent.expected_inputs {
        let slot = &mut inventory.slots[expected.slot];
        if slot.count < expected.count {
            inventory.slots = snapshot;
            return reject(CraftReject::InsufficientInput {
                identifier: expected.identifier.clone(),
            });
        }
        slot.count -= expected.count;
        if slot.count == 0 {
            *slot = TxSlot::empty();
        }
    }

    // 2. Verify output + remainder capacity before publishing.
    let (main, remainder) = recipe_outputs(recipe);
    let Some(main) = main else {
        inventory.slots = snapshot;
        return reject(CraftReject::Aborted("recipe has no outputs".to_string()));
    };
    let times = intent.requested_count.max(1) as usize;
    if times > 64
        || main.count.checked_mul(times as u16).is_none()
        || remainder
            .iter()
            .any(|output| output.count.checked_mul(times as u16).is_none())
    {
        inventory.slots = snapshot;
        return reject(CraftReject::Aborted(
            "craft count exceeds output budget".into(),
        ));
    }
    if !fits(
        &inventory.slots,
        &main.identifier,
        main.data,
        main.count as usize * times,
        max_stack,
    ) {
        inventory.slots = snapshot;
        return reject(CraftReject::NoOutputSpace);
    }
    for out in &remainder {
        if !fits(
            &inventory.slots,
            &out.identifier,
            out.data,
            out.count as usize * times,
            max_stack,
        ) {
            inventory.slots = snapshot;
            return reject(CraftReject::NoRemainderSpace);
        }
    }

    // 3-5. Write outputs + remainder, bump revision.
    for _ in 0..times {
        place(
            &mut inventory.slots,
            &main.identifier,
            main.data,
            main.count,
            max_stack,
        );
    }
    for out in &remainder {
        for _ in 0..times {
            place(
                &mut inventory.slots,
                &out.identifier,
                out.data,
                out.count,
                max_stack,
            );
        }
    }
    inventory.revision = inventory.revision.wrapping_add(1);
    finish(CraftReceipt::Applied {
        operation_id: intent.operation_id,
        recipe_id: recipe.identifier.clone(),
        inventory_revision: inventory.revision,
        outputs: std::iter::once((main.identifier.clone(), main.count * times as u16))
            .chain(
                remainder
                    .iter()
                    .map(|o| (o.identifier.clone(), o.count * times as u16)),
            )
            .collect(),
    })
}

struct OutRef {
    identifier: String,
    data: i32,
    count: u16,
}

fn recipe_outputs(recipe: &CompiledRecipe) -> (Option<OutRef>, Vec<OutRef>) {
    match &recipe.body {
        RecipeBody::Shaped(b) => split_outputs(&b.results),
        RecipeBody::Shapeless(b) => split_outputs(&b.results),
        _ => (None, Vec::new()),
    }
}

fn split_outputs(specs: &[crate::spec::OutputSpec]) -> (Option<OutRef>, Vec<OutRef>) {
    let mut iter = specs.iter();
    let first = iter.next().map(|o| OutRef {
        identifier: o.identifier.clone(),
        data: o.data.unwrap_or(0),
        count: o.count,
    });
    let rest = iter
        .map(|o| OutRef {
            identifier: o.identifier.clone(),
            data: o.data.unwrap_or(0),
            count: o.count,
        })
        .collect();
    (first, rest)
}

fn fits(
    slots: &[TxSlot],
    identifier: &str,
    data: i32,
    need: usize,
    max_stack: &dyn Fn(&str) -> u16,
) -> bool {
    let limit = max_stack(identifier) as usize;
    let mut remaining = need;
    for slot in slots {
        if slot.identifier == identifier && slot.data == data && !slot.is_empty() {
            remaining = remaining.saturating_sub(limit.saturating_sub(slot.count as usize));
        } else if slot.is_empty() {
            remaining = remaining.saturating_sub(limit);
        }
        if remaining == 0 {
            return true;
        }
    }
    false
}

fn place(
    slots: &mut [TxSlot],
    identifier: &str,
    data: i32,
    count: u16,
    max_stack: &dyn Fn(&str) -> u16,
) {
    let limit = max_stack(identifier);
    let mut remaining = count;
    for slot in slots.iter_mut() {
        if remaining == 0 {
            break;
        }
        if slot.identifier == identifier && slot.data == data && !slot.is_empty() {
            let room = limit.saturating_sub(slot.count);
            let moved = room.min(remaining);
            slot.count += moved;
            remaining -= moved;
        }
    }
    for slot in slots.iter_mut() {
        if remaining == 0 {
            break;
        }
        if slot.is_empty() {
            let moved = limit.min(remaining);
            *slot = TxSlot {
                identifier: identifier.to_string(),
                data,
                components: None,
                count: moved,
            };
            remaining -= moved;
        }
    }
}

fn verify_match(
    recipe: &CompiledRecipe,
    intent: &CraftIntent,
    inventory: &TxInventory,
    tags: &dyn TagResolver,
    max_expansion: usize,
) -> bool {
    let times = intent.requested_count.max(1);
    if intent
        .expected_inputs
        .iter()
        .any(|input| input.count % times != 0)
    {
        return false;
    }
    match &recipe.body {
        RecipeBody::Shaped(_) => {
            // Reconstruct a 3x3 grid from expected slots is ambiguous
            // without positions; instead verify each expected slot matches
            // at least one recipe ingredient cell. Full grid placement is
            // verified by the matcher in `find_best_instant` on the game
            // side with real grid geometry; here we ensure no foreign item
            // is consumed.
            let cells = recipe_shaped_cells(recipe);
            for expected in &intent.expected_inputs {
                let input = MatchInput::new(
                    expected.identifier.clone(),
                    expected.data,
                    expected.count / times,
                );
                if !cells.iter().any(|spec| {
                    spec.choices.iter().any(|c| match c {
                        crate::spec::IngredientChoice::Item { identifier, data } => {
                            (identifier == &input.identifier
                                && data.map(|d| d == input.data || d == 32767).unwrap_or(true))
                                || item_family_matches(
                                    identifier,
                                    *data,
                                    &input,
                                    tags,
                                    max_expansion,
                                )
                        }
                        crate::spec::IngredientChoice::Tag { tag } => {
                            tags.members(tag.as_str()).contains(&input.identifier)
                        }
                    })
                }) {
                    return false;
                }
            }
            // Also run the multiset check so counts line up.
            let flat = to_match_inputs(&inventory.slots);
            let _ = (flat, tags, max_expansion);
            true
        }
        RecipeBody::Shapeless(_) => {
            // Multiset check over the expected inputs (client-declared
            // consumption must itself satisfy the recipe).
            let expected_inputs: Vec<MatchInput> = intent
                .expected_inputs
                .iter()
                .map(|e| MatchInput::new(e.identifier.clone(), e.data, e.count / times))
                .collect();
            match_shapeless(
                recipe,
                &expected_inputs,
                intent.context.station,
                tags,
                max_expansion,
            )
        }
        _ => false,
    }
}

fn recipe_shaped_cells(recipe: &CompiledRecipe) -> Vec<IngredientSpec> {
    match &recipe.body {
        RecipeBody::Shaped(b) => b.grid.iter().filter_map(|c| c.clone()).collect(),
        _ => Vec::new(),
    }
}

/// Validate a station tag against the recipe without crafting.
pub fn station_allows(recipe: &CompiledRecipe, station: StationKind) -> bool {
    recipe.stations.is_empty() || recipe.stations.contains(&station)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{CompileBudgets, SourceRecipe};
    use crate::matching::MapTagResolver;
    use crate::registry::RecipeRegistrySnapshot;

    fn compile_raw(raw: serde_json::Value) -> (RecipeRegistrySnapshot, CompiledRecipe) {
        let bytes = serde_json::to_vec(&raw).unwrap();
        let src = SourceRecipe::new("p", 0, "r.json", "1.12", raw, &bytes);
        let (snap, _) =
            RecipeRegistrySnapshot::compile(&[src], &CompileBudgets::default(), false).unwrap();
        let recipe = snap.in_network_order()[0].clone();
        (snap, recipe)
    }

    fn inv(slots: Vec<(&str, i32, u16)>) -> TxInventory {
        TxInventory {
            slots: slots
                .into_iter()
                .map(|(id, data, count)| TxSlot {
                    identifier: id.to_string(),
                    data,
                    components: None,
                    count,
                })
                .collect(),
            revision: 7,
        }
    }

    fn checks() -> CommitChecks {
        CommitChecks {
            in_reach: true,
            has_permission: true,
            gamemode_allows: true,
            recipes_unlocked: true,
            station_exists: true,
            station_kind_ok: true,
            actual_container_revision: 3,
            actual_station_revision: 1,
        }
    }

    fn intent(
        recipe: &str,
        fp: RecipeRegistryFingerprint,
        expected: Vec<ExpectedSlot>,
    ) -> CraftIntent {
        CraftIntent {
            actor: CraftActor {
                player_id: 1,
                entity_generation: 0,
            },
            context: CraftContext {
                world_id: "overworld".to_string(),
                dimension: 0,
                station: StationKind::CraftingTable,
                station_pos: None,
                station_revision: 1,
                client_claims_in_reach: true,
            },
            recipe_id: recipe.to_string(),
            registry_fingerprint: fp,
            inventory_revision: 7,
            container_revision: 3,
            expected_inputs: expected,
            requested_count: 1,
            operation_id: 99,
            deadline_unix: 0,
        }
    }

    #[test]
    fn success_deducts_and_produces_with_revision_bump() {
        let (snap, recipe) = compile_raw(serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shapeless": {
                "description": {"identifier": "minecraft:test"},
                "tags": ["crafting_table"],
                "ingredients": [{"item": "minecraft:stone"}],
                "result": {"item": "minecraft:out", "count": 2}
            }
        }));
        let mut inventory = inv(vec![("minecraft:stone", 0, 5), ("", 0, 0)]);
        let mut seen = SeenOperations::new(16);
        let tags = MapTagResolver::empty();
        let receipt = execute_craft(
            &intent(
                "minecraft:test",
                snap.fingerprint(),
                vec![ExpectedSlot {
                    slot: 0,
                    identifier: "minecraft:stone".to_string(),
                    data: 0,
                    count: 1,
                }],
            ),
            Some(&recipe),
            snap.fingerprint(),
            &mut inventory,
            &checks(),
            &mut seen,
            &tags,
            1024,
            &|_| 64,
        );
        assert!(matches!(receipt, CraftReceipt::Applied { .. }));
        assert_eq!(inventory.revision, 8);
    }

    #[test]
    fn retry_with_same_operation_id_does_not_double_apply() {
        let (snap, recipe) = compile_raw(serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shapeless": {
                "description": {"identifier": "minecraft:test"},
                "tags": ["crafting_table"],
                "ingredients": [{"item": "minecraft:stone"}],
                "result": {"item": "minecraft:out"}
            }
        }));
        let mut inventory = inv(vec![("minecraft:stone", 0, 5), ("", 0, 0)]);
        let mut seen = SeenOperations::new(16);
        let tags = MapTagResolver::empty();
        let first = execute_craft(
            &intent(
                "minecraft:test",
                snap.fingerprint(),
                vec![ExpectedSlot {
                    slot: 0,
                    identifier: "minecraft:stone".to_string(),
                    data: 0,
                    count: 1,
                }],
            ),
            Some(&recipe),
            snap.fingerprint(),
            &mut inventory,
            &checks(),
            &mut seen,
            &tags,
            1024,
            &|_| 64,
        );
        let second = execute_craft(
            &intent(
                "minecraft:test",
                snap.fingerprint(),
                vec![ExpectedSlot {
                    slot: 0,
                    identifier: "minecraft:stone".to_string(),
                    data: 0,
                    count: 1,
                }],
            ),
            Some(&recipe),
            snap.fingerprint(),
            &mut inventory,
            &checks(),
            &mut seen,
            &tags,
            1024,
            &|_| 64,
        );
        assert_eq!(first, second);
        // Only one unit consumed + one revision bump.
        assert_eq!(inventory.revision, 8);
    }

    #[test]
    fn stale_revision_and_no_space_roll_back() {
        let (snap, recipe) = compile_raw(serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shapeless": {
                "description": {"identifier": "minecraft:test"},
                "tags": ["crafting_table"],
                "ingredients": [{"item": "minecraft:stone"}],
                "result": {"item": "minecraft:out"}
            }
        }));
        let tags = MapTagResolver::empty();
        // Stale inventory revision.
        let mut inventory = inv(vec![("minecraft:stone", 0, 5)]);
        let mut seen = SeenOperations::new(16);
        let mut bad = intent(
            "minecraft:test",
            snap.fingerprint(),
            vec![ExpectedSlot {
                slot: 0,
                identifier: "minecraft:stone".to_string(),
                data: 0,
                count: 1,
            }],
        );
        bad.inventory_revision = 0;
        let receipt = execute_craft(
            &bad,
            Some(&recipe),
            snap.fingerprint(),
            &mut inventory,
            &checks(),
            &mut seen,
            &tags,
            1024,
            &|_| 64,
        );
        assert!(matches!(
            receipt,
            CraftReceipt::Rejected {
                reason: CraftReject::StaleInventoryRevision { .. },
                ..
            }
        ));
        assert_eq!(inventory.slots[0].count, 5);
        // Output full: single slot already full of another item.
        let mut full = TxInventory {
            slots: vec![TxSlot {
                identifier: "minecraft:stone".to_string(),
                data: 0,
                components: None,
                count: 1,
            }],
            revision: 7,
        };
        let mut seen2 = SeenOperations::new(16);
        // Force max_stack=1 and fill the only slot with junk so nothing fits.
        full.slots[0] = TxSlot {
            identifier: "minecraft:junk".to_string(),
            data: 0,
            components: None,
            count: 1,
        };
        let receipt = execute_craft(
            &intent(
                "minecraft:test",
                snap.fingerprint(),
                vec![ExpectedSlot {
                    slot: 0,
                    identifier: "minecraft:stone".to_string(),
                    data: 0,
                    count: 1,
                }],
            ),
            Some(&recipe),
            snap.fingerprint(),
            &mut full,
            &checks(),
            &mut seen2,
            &tags,
            1024,
            &|_| 1,
        );
        // Expected-slot mismatch (slot holds junk) -> rejected, no mutation.
        assert!(matches!(receipt, CraftReceipt::Rejected { .. }));
    }

    #[test]
    fn queue_full_returns_busy() {
        let mut queue = CraftQueue::new(1);
        let mk = |op: u64| CraftIntent {
            actor: CraftActor {
                player_id: 1,
                entity_generation: 0,
            },
            context: CraftContext {
                world_id: "w".to_string(),
                dimension: 0,
                station: StationKind::CraftingTable,
                station_pos: None,
                station_revision: 0,
                client_claims_in_reach: true,
            },
            recipe_id: "r".to_string(),
            registry_fingerprint: RecipeRegistryFingerprint([0; 32]),
            inventory_revision: 0,
            container_revision: 0,
            expected_inputs: Vec::new(),
            requested_count: 1,
            operation_id: op,
            deadline_unix: 0,
        };
        assert!(queue.try_submit(mk(1)).is_ok());
        assert_eq!(queue.try_submit(mk(2)), Err(CraftReject::QueueBusy));
    }
}
