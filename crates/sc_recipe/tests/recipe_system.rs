//! Integration: compile + match + transaction over MS-docs fixtures.

use std::collections::{HashMap, HashSet};

use sc_recipe::{
    compile_ordered, decide_commit, execute_craft, find_best_instant, match_process_input,
    CommitChecks, CompileBudgets, CraftActor, CraftContext, CraftIntent, CraftQueue, CraftReceipt,
    CraftReject, ExpectedSlot, MapTagResolver, MatchInput, PlayerRecipeBook,
    RecipeRegistrySnapshot, SourceRecipe, StationKind, TxnDecision, UnlockPolicy,
};

fn src(pack: &str, order: usize, path: &str, raw: serde_json::Value) -> SourceRecipe {
    let bytes = serde_json::to_vec(&raw).unwrap();
    SourceRecipe::new(pack, order, path, "1.12", raw, &bytes)
}

fn budgets() -> CompileBudgets {
    CompileBudgets::default()
}

#[test]
fn furnace_brewing_smithing_material_reducer_fixtures_compile() {
    let sources = vec![
        src(
            "p",
            0,
            "furnace.json",
            serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_furnace": {
                    "description": {"identifier": "minecraft:furnace_beef"},
                    "tags": ["furnace", "smoker", "campfire", "soul_campfire"],
                    "input": {"item": "minecraft:beef", "data": 0, "count": 1},
                    "output": "minecraft:cooked_beef"
                }
            }),
        ),
        src(
            "p",
            0,
            "fuel.json",
            serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_furnace_material": {
                    "description": {"identifier": "minecraft:fuel_coal"},
                    "tags": ["furnace"],
                    "input": "minecraft:coal",
                    "output": "minecraft:coal"
                }
            }),
        ),
        src(
            "p",
            0,
            "brew_mix.json",
            serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_brewing_mix": {
                    "description": {"identifier": "minecraft:brew_awkward_blaze"},
                    "tags": ["brewing_stand"],
                    "input": "minecraft:potion_type:awkward",
                    "reagent": "minecraft:blaze_powder",
                    "output": "minecraft:potion_type:strength"
                }
            }),
        ),
        src(
            "p",
            0,
            "brew_container.json",
            serde_json::json!({
                "format_version": "1.17",
                "minecraft:recipe_brewing_container": {
                    "description": {"identifier": "minecraft:brew_splash"},
                    "tags": ["brewing_stand"],
                    "input": "minecraft:potion",
                    "reagent": "minecraft:gunpowder",
                    "output": "minecraft:splash_potion"
                }
            }),
        ),
        src(
            "p",
            0,
            "smith_transform.json",
            serde_json::json!({
                "format_version": "1.20.10",
                "minecraft:recipe_smithing_transform": {
                    "description": {"identifier": "minecraft:smithing_netherite_boots"},
                    "tags": ["smithing_table"],
                    "template": "minecraft:netherite_upgrade_smithing_template",
                    "base": "minecraft:diamond_boots",
                    "addition": "minecraft:netherite_ingot",
                    "result": "minecraft:netherite_boots"
                }
            }),
        ),
        src(
            "p",
            0,
            "smith_trim.json",
            serde_json::json!({
                "format_version": "1.20.10",
                "minecraft:recipe_smithing_trim": {
                    "description": {"identifier": "minecraft:smithing_armor_trim"},
                    "tags": ["smithing_table"],
                    "template": {"tag": "minecraft:trim_templates"},
                    "base": {"tag": "minecraft:trimmable_armors"},
                    "addition": {"tag": "minecraft:trim_materials"}
                }
            }),
        ),
        src(
            "p",
            0,
            "reducer.json",
            serde_json::json!({
                "format_version": "1.14",
                "minecraft:recipe_material_reduction": {
                    "description": {"identifier": "minecraft:reducer_stone"},
                    "tags": "material_reducer",
                    "input": "minecraft:stone",
                    "output": [
                        {"item": "minecraft:element_14", "count": 2},
                        {"item": "minecraft:element_8", "count": 1}
                    ]
                }
            }),
        ),
    ];
    let out = compile_ordered(&sources, &budgets(), false).unwrap();
    assert_eq!(out.recipes.len(), 7, "all MS-docs kinds compile");
    assert!(out.disabled.is_empty());
    let (snapshot, _) = RecipeRegistrySnapshot::from_output(out);
    // Stations are independent of kinds.
    let furnace: Vec<_> = snapshot.recipes_for_station(StationKind::Furnace);
    assert!(!furnace.is_empty());
    let brewing: Vec<_> = snapshot.recipes_for_station(StationKind::BrewingStand);
    assert_eq!(brewing.len(), 2);
}

#[test]
fn unknown_fields_are_preserved_for_diagnostics() {
    let source = src(
        "p",
        0,
        "r.json",
        serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shapeless": {
                "description": {"identifier": "minecraft:x", "extra_desc": 1},
                "tags": ["crafting_table"],
                "ingredients": [{"item": "minecraft:stone", "weird": true}],
                "result": {"item": "minecraft:out"},
                "future_field": "kept"
            }
        }),
    );
    let out = compile_ordered(&[source], &budgets(), false).unwrap();
    assert!(!out.recipes[0].unknown_fields.is_empty());
}

#[test]
fn tag_expansion_limit_is_enforced() {
    // 3-member tag with limit 1: direct match helpers reject expansion.
    let source = src(
        "p",
        0,
        "r.json",
        serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shapeless": {
                "description": {"identifier": "minecraft:tag_test"},
                "tags": ["crafting_table"],
                "ingredients": [{"tag": "minecraft:planks"}],
                "result": {"item": "minecraft:out"}
            }
        }),
    );
    let out = compile_ordered(&[source], &budgets(), false).unwrap();
    let recipe = &out.recipes[0];
    let mut map = HashMap::new();
    map.insert(
        "minecraft:planks".to_string(),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
    );
    let tags = MapTagResolver::new(map);
    // Limit 1 < 3 members → no match (fail-closed, never partial).
    assert!(!sc_recipe::match_shapeless(
        recipe,
        &[MatchInput::new("a", 0, 1)],
        StationKind::CraftingTable,
        &tags,
        1,
    ));
    // Generous limit matches.
    assert!(sc_recipe::match_shapeless(
        recipe,
        &[MatchInput::new("a", 0, 1)],
        StationKind::CraftingTable,
        &tags,
        1024,
    ));
}

#[test]
fn components_requirement_is_disabled_not_ignored() {
    let source = src(
        "p",
        0,
        "r.json",
        serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shapeless": {
                "description": {"identifier": "minecraft:nbt_test"},
                "tags": ["crafting_table"],
                "ingredients": [{"item": "minecraft:stone", "components": {"x": 1}}],
                "result": {"item": "minecraft:out"}
            }
        }),
    );
    let out = compile_ordered(&[source], &budgets(), false).unwrap();
    assert!(out.recipes.is_empty());
    assert_eq!(out.disabled.len(), 1);
    assert!(out.disabled[0].reason.contains("components"));
}

#[test]
fn cross_region_reservation_commit_abort() {
    // Prepare: two owners reserve; both ready → commit.
    assert_eq!(decide_commit(&[true, true], "no"), TxnDecision::Commit);
    // One refuses → abort with reason, no state change.
    assert!(matches!(
        decide_commit(&[true, false], "station gone"),
        TxnDecision::Abort(_)
    ));
    // Reservations carry owner/epoch/revision for fencing.
    let reservation = sc_recipe::RegionReservation {
        operation_id: 5,
        owner: "region-a".to_string(),
        epoch: 3,
        revision: 9,
        ttl_ticks: 100,
    };
    assert_eq!(reservation.owner, "region-a");
}

#[test]
fn transaction_rolls_back_on_remainder_no_space() {
    use sc_recipe::{SeenOperations, TxInventory, TxSlot};
    // Cake-style recipe: main + remainder (3 buckets).
    let raw = serde_json::json!({
        "format_version": "1.12",
        "minecraft:recipe_shaped": {
            "description": {"identifier": "minecraft:cake"},
            "tags": ["crafting_table"],
            "pattern": ["A"],
            "key": {"A": {"item": "minecraft:sugar"}},
            "result": [
                {"item": "minecraft:cake"},
                {"item": "minecraft:bucket", "count": 3}
            ]
        }
    });
    let source = src("p", 0, "r.json", raw);
    let (snapshot, _) = RecipeRegistrySnapshot::compile(&[source], &budgets(), false).unwrap();
    let recipe = snapshot.get("minecraft:cake").unwrap().clone();
    // One-slot inventory: input present but no room for remainder.
    let mut inv = TxInventory {
        slots: vec![TxSlot {
            identifier: "minecraft:sugar".to_string(),
            data: 0,
            components: None,
            count: 1,
        }],
        revision: 0,
    };
    let mut seen = SeenOperations::new(8);
    let tags = MapTagResolver::empty();
    let intent = CraftIntent {
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
        recipe_id: "minecraft:cake".to_string(),
        registry_fingerprint: snapshot.fingerprint(),
        inventory_revision: 0,
        container_revision: 0,
        expected_inputs: vec![ExpectedSlot {
            slot: 0,
            identifier: "minecraft:sugar".to_string(),
            data: 0,
            count: 1,
        }],
        requested_count: 1,
        operation_id: 1,
        deadline_unix: 0,
    };
    let checks = CommitChecks {
        in_reach: true,
        has_permission: true,
        gamemode_allows: true,
        recipes_unlocked: true,
        station_exists: true,
        station_kind_ok: true,
        actual_container_revision: 0,
        actual_station_revision: 0,
    };
    let receipt = execute_craft(
        &intent,
        Some(&recipe),
        snapshot.fingerprint(),
        &mut inv,
        &checks,
        &mut seen,
        &tags,
        1024,
        &|_| 1,
    );
    assert!(
        matches!(
            receipt,
            CraftReceipt::Rejected {
                reason: CraftReject::NoOutputSpace | CraftReject::NoRemainderSpace,
                ..
            }
        ),
        "got {receipt:?}"
    );
    // Rollback: input restored.
    assert_eq!(inv.slots[0].count, 1);
    assert_eq!(inv.slots[0].identifier, "minecraft:sugar");
}

#[test]
fn unlock_migration_and_queue_busy() {
    let mut book = PlayerRecipeBook::default();
    book.unlock("minecraft:kept");
    book.unlock("minecraft:gone");
    let live: HashSet<String> = ["minecraft:kept".to_string()].into_iter().collect();
    let dropped = book.migrate(sc_recipe::RecipeRegistryFingerprint([9; 32]), &live);
    assert_eq!(dropped, vec!["minecraft:gone".to_string()]);
    assert!(book.is_unlocked("minecraft:kept"));

    let policy = UnlockPolicy {
        recipes_unlock_rule: false,
    };
    assert!(!policy.may_craft(&book, "minecraft:locked2", true));

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
        registry_fingerprint: sc_recipe::RecipeRegistryFingerprint([0; 32]),
        inventory_revision: 0,
        container_revision: 0,
        expected_inputs: Vec::new(),
        requested_count: 1,
        operation_id: op,
        deadline_unix: 0,
    };
    assert!(queue.try_submit(mk(1)).is_ok());
    assert!(matches!(
        queue.try_submit(mk(2)),
        Err(CraftReject::QueueBusy)
    ));
}

#[test]
fn instant_finder_prefers_priority_then_identifier() {
    let mk = |id: &str, priority: i64| {
        src(
            "p",
            0,
            &format!("{id}.json"),
            serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_shapeless": {
                    "description": {"identifier": id},
                    "tags": ["crafting_table"],
                    "priority": priority,
                    "ingredients": [{"item": "minecraft:stone"}],
                    "result": {"item": "minecraft:out"}
                }
            }),
        )
    };
    let sources = vec![mk("minecraft:b", 0), mk("minecraft:a", 0)];
    let (snapshot, _) = RecipeRegistrySnapshot::compile(&sources, &budgets(), false).unwrap();
    let tags = MapTagResolver::empty();
    let grid = vec![Some(MatchInput::new("minecraft:stone", 0, 1))];
    let best = find_best_instant(
        snapshot.in_network_order(),
        &grid,
        1,
        1,
        StationKind::CraftingTable,
        &tags,
        1024,
    )
    .unwrap();
    assert_eq!(best.identifier, "minecraft:a");
}

#[test]
fn process_input_matches_single_slot() {
    use sc_recipe::IngredientSpec;
    let spec = IngredientSpec::single_item("minecraft:beef", Some(0), 1);
    let tags = MapTagResolver::empty();
    assert!(match_process_input(
        &spec,
        &MatchInput::new("minecraft:beef", 0, 1),
        &tags,
        1024
    ));
    assert!(!match_process_input(
        &spec,
        &MatchInput::new("minecraft:porkchop", 0, 1),
        &tags,
        1024
    ));
}
