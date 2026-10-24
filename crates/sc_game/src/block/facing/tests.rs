//! 朝向系统单元测试（涵盖几何数学、视角分区与真实调色板放置测试）。
//!
//! Facing-system unit tests (geometry math, view quadrants and real-palette placement).

use super::*;
use sc_block::position::BlockPosition;
use sc_block::registry::BlockStateRegistry;
use sc_block::state::BlockPropertyValue;
use sc_entity::motion::Position;
use std::sync::Arc;

#[test]
fn look_follows_yaw_quadrants() {
    // Yaw quadrants: SOUTH on [315,45), WEST on [45,135),
    // NORTH on [135,225), EAST on [225,315).
    assert_eq!(BlockFace::from_yaw(0.0), BlockFace::South);
    assert_eq!(BlockFace::from_yaw(44.9), BlockFace::South);
    assert_eq!(BlockFace::from_yaw(45.0), BlockFace::West);
    assert_eq!(BlockFace::from_yaw(90.0), BlockFace::West);
    assert_eq!(BlockFace::from_yaw(180.0), BlockFace::North);
    assert_eq!(BlockFace::from_yaw(270.0), BlockFace::East);
    assert_eq!(BlockFace::from_yaw(315.0), BlockFace::South);
    assert_eq!(BlockFace::from_yaw(-90.0), BlockFace::East);
    assert_eq!(BlockFace::from_yaw(f32::NAN), BlockFace::South);
}

#[test]
fn face_math_helpers() {
    assert_eq!(BlockFace::Down.opposite(), BlockFace::Up);
    assert_eq!(BlockFace::Up.opposite(), BlockFace::Down);
    assert_eq!(BlockFace::North.opposite(), BlockFace::South);
    assert_eq!(BlockFace::West.opposite(), BlockFace::East);

    assert_eq!(BlockFace::Down.axis().as_str(), "y");
    assert_eq!(BlockFace::Up.axis().as_str(), "y");
    assert_eq!(BlockFace::North.axis().as_str(), "z");
    assert_eq!(BlockFace::East.axis().as_str(), "x");

    // weirdo: W=0/E=1/N=2/S=3
    assert_eq!(BlockFace::West.as_weirdo_int(), 0);
    assert_eq!(BlockFace::East.as_weirdo_int(), 1);
    assert_eq!(BlockFace::North.as_weirdo_int(), 2);
    assert_eq!(BlockFace::South.as_weirdo_int(), 3);

    assert_eq!(BlockFace::North.as_cardinal_str(), "north");
    assert_eq!(BlockFace::Down.as_observer_str(), "down");
    assert_eq!(BlockFace::Up.as_torch_str(), Some("top"));
    assert_eq!(BlockFace::Down.as_torch_str(), None);

    // Trapdoor EWSN (E=0/W=1/S=2/N=3) stores opposite of look
    assert_eq!(BlockFace::South.opposite().as_trapdoor_int(), 3); // North=3
    assert_eq!(BlockFace::North.opposite().as_trapdoor_int(), 2); // South=2
    assert_eq!(BlockFace::West.opposite().as_trapdoor_int(), 0);  // East=0
    assert_eq!(BlockFace::East.opposite().as_trapdoor_int(), 1);  // West=1
}

#[test]
fn eye_rule_needs_nearness_for_vertical_override() {
    let far_ctx = PlacementContext::new(
        BlockFace::Up,
        Position::new(100.5, 70.0, 100.5),
        BlockPosition::new(0, 65, 0),
        1.62,
        0.0,
        None,
    );
    // Far player: horizontal passes through even when high above.
    assert_eq!(far_ctx.eye_facing(BlockFace::North), BlockFace::North);

    let near_high_ctx = PlacementContext::new(
        BlockFace::Up,
        Position::new(0.5, 70.0, 0.5),
        BlockPosition::new(0, 65, 0),
        1.62,
        0.0,
        None,
    );
    // Near + eyes >2 above target: UP
    assert_eq!(near_high_ctx.eye_facing(BlockFace::North), BlockFace::Up);

    let near_low_ctx = PlacementContext::new(
        BlockFace::Up,
        Position::new(0.5, 60.0, 0.5),
        BlockPosition::new(0, 65, 0),
        1.62,
        0.0,
        None,
    );
    // Near + target above eyes: DOWN
    assert_eq!(near_low_ctx.eye_facing(BlockFace::North), BlockFace::Down);

    let near_level_ctx = PlacementContext::new(
        BlockFace::Up,
        Position::new(0.5, 65.0, 0.5),
        BlockPosition::new(0, 65, 0),
        1.62,
        0.0,
        None,
    );
    // Near + level: horizontal
    assert_eq!(near_level_ctx.eye_facing(BlockFace::West), BlockFace::West);
}

/// Builds a synthetic palette covering air, stone and one block per orientation family.
fn palette() -> BlockStateRegistry {
    use sc_block::block_json::compile_bundle;
    use sc_packloader::block::{parse_block_file, BlockBundleBudgets, BlockJsonBundle};

    fn block_bytes(identifier: &str, states: &[(&str, &[&str])], next_id: &mut u32) -> Vec<u8> {
        let state_count = states
            .iter()
            .map(|(_, values)| values.len())
            .product::<usize>()
            .max(1);
        let ids: Vec<String> = (0..state_count)
            .map(|_| {
                let id = *next_id;
                *next_id += 1;
                id.to_string()
            })
            .collect();
        let states_json = if states.is_empty() {
            "{}".to_string()
        } else {
            let entries: Vec<String> = states
                .iter()
                .map(|(name, values)| format!("\"{name}\": [{}]", values.join(", ")))
                .collect();
            format!("{{{}}}", entries.join(", "))
        };
        format!(
            r#"{{"format_version":"1.10.0","minecraft:block":{{"description":{{"identifier":"{identifier}","states":{states_json}}},"components":{{}},"sc:protocol_runtime_ids":[{ids}]}}}}"#,
            ids = ids.join(", ")
        )
        .into_bytes()
    }

    let mut files = Vec::new();
    let mut next_id = 1u32;
    {
        let mut push = |identifier: &str, states: &[(&str, &[&str])]| {
            let path = format!("blocks/test/{}.json", identifier.replace(':', "_"));
            let bytes = block_bytes(identifier, states, &mut next_id);
            files.push(parse_block_file("test", &path, &bytes).expect("synthetic block parses"));
        };
        push("minecraft:air", &[]);
        push(
            "minecraft:oak_log",
            &[("pillar_axis", &["\"x\"", "\"y\"", "\"z\""])],
        );
        push(
            "minecraft:oak_stairs",
            &[
                ("upside_down_bit", &["false", "true"]),
                ("weirdo_direction", &["0", "1", "2", "3"]),
            ],
        );
        push(
            "minecraft:furnace",
            &[(
                "minecraft:cardinal_direction",
                &["\"north\"", "\"south\"", "\"east\"", "\"west\""],
            )],
        );
        push(
            "minecraft:dispenser",
            &[
                ("facing_direction", &["0", "1", "2", "3", "4", "5"]),
                ("triggered_bit", &["false", "true"]),
            ],
        );
        push(
            "minecraft:hopper",
            &[
                ("facing_direction", &["0", "1", "2", "3", "4", "5"]),
                ("toggle_bit", &["false", "true"]),
            ],
        );
        push(
            "minecraft:piston",
            &[("facing_direction", &["0", "1", "2", "3", "4", "5"])],
        );
        push(
            "minecraft:observer",
            &[
                (
                    "minecraft:facing_direction",
                    &[
                        "\"down\"", "\"up\"", "\"north\"", "\"south\"", "\"east\"",
                        "\"west\"",
                    ],
                ),
                ("powered_bit", &["false", "true"]),
            ],
        );
        push(
            "minecraft:torch",
            &[(
                "torch_facing_direction",
                &[
                    "\"unknown\"", "\"west\"", "\"east\"", "\"north\"", "\"south\"",
                    "\"top\"",
                ],
            )],
        );
        push(
            "minecraft:trapdoor",
            &[
                ("direction", &["0", "1", "2", "3"]),
                ("open_bit", &["false", "true"]),
                ("upside_down_bit", &["false", "true"]),
            ],
        );
        push("minecraft:stone", &[]);
    }

    let bundle = BlockJsonBundle {
        schema_version: 1,
        network_id_mode: "hashed".to_string(),
        fingerprint: 0,
        files,
        loot_tables: Default::default(),
    };
    let (snapshot, warnings) = compile_bundle(
        &bundle,
        "test",
        &BlockBundleBudgets::default(),
        &|_| None,
        &|_| true,
    )
    .expect("synthetic palette compiles");
    assert!(warnings.is_empty(), "warnings: {warnings:?}");
    BlockStateRegistry::from_block_snapshot(&snapshot)
}

fn hash_of(
    registry: &BlockStateRegistry,
    name: &str,
    props: &[(&str, BlockPropertyValue)],
) -> u32 {
    registry
        .runtime_id(
            registry
                .state_of(name, props)
                .unwrap_or_else(|| panic!("{name} {props:?} 应在 palette 中")),
        )
        .expect("runtime id")
        .0
}

fn far_ctx(clicked_face: BlockFace, yaw: f32, click_frac_y: Option<f32>) -> PlacementContext {
    PlacementContext::new(
        clicked_face,
        Position::new(100.5, 65.0, 100.5),
        BlockPosition::new(0, 65, 0),
        1.62,
        yaw,
        click_frac_y,
    )
}

#[test]
fn orient_pillar_axis_from_clicked_face() {
    let registry = palette();
    let base = hash_of(
        &registry,
        "minecraft:oak_log",
        &[("pillar_axis", BlockPropertyValue::Enum(Arc::from("y")))],
    );
    // Side click (north face) lays the log along z.
    let ctx = far_ctx(BlockFace::North, 0.0, None);
    let out = orient_state(&registry, base, &ctx);
    assert_eq!(
        out,
        hash_of(
            &registry,
            "minecraft:oak_log",
            &[("pillar_axis", BlockPropertyValue::Enum(Arc::from("z")))]
        )
    );
    // Top click keeps y.
    let top_ctx = far_ctx(BlockFace::Up, 0.0, None);
    assert_eq!(orient_state(&registry, base, &top_ctx), base);
}

#[test]
fn orient_stairs_weirdo_and_upside() {
    let registry = palette();
    let bool_false = BlockPropertyValue::Bool(false);
    let base = hash_of(
        &registry,
        "minecraft:oak_stairs",
        &[
            ("upside_down_bit", bool_false.clone()),
            ("weirdo_direction", BlockPropertyValue::Int(0)),
        ],
    );
    // Look south (yaw 0), top-face click: weirdo S=3, bottom half.
    let ctx = far_ctx(BlockFace::Up, 0.0, None);
    let out = orient_state(&registry, base, &ctx);
    assert_eq!(
        out,
        hash_of(
            &registry,
            "minecraft:oak_stairs",
            &[
                ("upside_down_bit", bool_false.clone()),
                ("weirdo_direction", BlockPropertyValue::Int(3)),
            ]
        )
    );
    // Bottom-face click flips upside-down.
    let bot_ctx = far_ctx(BlockFace::Down, 0.0, None);
    let out = orient_state(&registry, base, &bot_ctx);
    assert_eq!(
        out,
        hash_of(
            &registry,
            "minecraft:oak_stairs",
            &[
                ("upside_down_bit", BlockPropertyValue::Bool(true)),
                ("weirdo_direction", BlockPropertyValue::Int(3)),
            ]
        )
    );
    // Side click on the upper half flips upside-down (click fraction).
    let side_up_ctx = far_ctx(BlockFace::North, 0.0, Some(0.7));
    let out = orient_state(&registry, base, &side_up_ctx);
    assert_eq!(
        out,
        hash_of(
            &registry,
            "minecraft:oak_stairs",
            &[
                ("upside_down_bit", BlockPropertyValue::Bool(true)),
                ("weirdo_direction", BlockPropertyValue::Int(3)),
            ]
        )
    );
}

#[test]
fn orient_cardinal_is_opposite_look() {
    let registry = palette();
    let base = hash_of(
        &registry,
        "minecraft:furnace",
        &[(
            "minecraft:cardinal_direction",
            BlockPropertyValue::Enum(Arc::from("south")),
        )],
    );
    // Look east (yaw 270): front faces west (toward the player).
    let ctx = far_ctx(BlockFace::Up, 270.0, None);
    let out = orient_state(&registry, base, &ctx);
    assert_eq!(
        out,
        hash_of(
            &registry,
            "minecraft:furnace",
            &[(
                "minecraft:cardinal_direction",
                BlockPropertyValue::Enum(Arc::from("west")),
            )]
        )
    );
}

#[test]
fn orient_dispenser_eye_rule_and_hopper_face() {
    let registry = palette();
    let base = hash_of(
        &registry,
        "minecraft:dispenser",
        &[
            ("facing_direction", BlockPropertyValue::Int(3)),
            ("triggered_bit", BlockPropertyValue::Bool(false)),
        ],
    );
    // Look south, far: hole faces north (opposite of look).
    let ctx = far_ctx(BlockFace::Up, 0.0, None);
    let out = orient_state(&registry, base, &ctx);
    assert_eq!(
        out,
        hash_of(
            &registry,
            "minecraft:dispenser",
            &[
                ("facing_direction", BlockPropertyValue::Int(2)),
                ("triggered_bit", BlockPropertyValue::Bool(false)),
            ]
        )
    );
    // Near + eyes high above: fires upward.
    let near_high_ctx = PlacementContext::new(
        BlockFace::Up,
        Position::new(0.5, 70.0, 0.5),
        BlockPosition::new(0, 65, 0),
        1.62,
        0.0,
        None,
    );
    let out = orient_state(&registry, base, &near_high_ctx);
    assert_eq!(
        out,
        hash_of(
            &registry,
            "minecraft:dispenser",
            &[
                ("facing_direction", BlockPropertyValue::Int(1)),
                ("triggered_bit", BlockPropertyValue::Bool(false)),
            ]
        )
    );
    // Hopper: opposite of the clicked side face (west click -> east).
    let hopper_base = hash_of(
        &registry,
        "minecraft:hopper",
        &[
            ("facing_direction", BlockPropertyValue::Int(0)),
            ("toggle_bit", BlockPropertyValue::Bool(false)),
        ],
    );
    let west_ctx = far_ctx(BlockFace::West, 0.0, None);
    let out = orient_state(&registry, hopper_base, &west_ctx);
    assert_eq!(
        out,
        hash_of(
            &registry,
            "minecraft:hopper",
            &[
                ("facing_direction", BlockPropertyValue::Int(5)),
                ("toggle_bit", BlockPropertyValue::Bool(false)),
            ]
        )
    );
}

#[test]
fn orient_piston_and_observer_follow_look() {
    let registry = palette();
    // Piston head extends along look (west on yaw 90).
    let base = hash_of(
        &registry,
        "minecraft:piston",
        &[("facing_direction", BlockPropertyValue::Int(3))],
    );
    let ctx = far_ctx(BlockFace::Up, 90.0, None);
    assert_eq!(
        orient_state(&registry, base, &ctx),
        hash_of(
            &registry,
            "minecraft:piston",
            &[("facing_direction", BlockPropertyValue::Int(4))]
        )
    );
    // Observer face points along look (west on yaw 90).
    let obs_base = hash_of(
        &registry,
        "minecraft:observer",
        &[
            (
                "minecraft:facing_direction",
                BlockPropertyValue::Enum(Arc::from("south")),
            ),
            ("powered_bit", BlockPropertyValue::Bool(false)),
        ],
    );
    assert_eq!(
        orient_state(&registry, obs_base, &ctx),
        hash_of(
            &registry,
            "minecraft:observer",
            &[
                (
                    "minecraft:facing_direction",
                    BlockPropertyValue::Enum(Arc::from("west")),
                ),
                ("powered_bit", BlockPropertyValue::Bool(false)),
            ]
        )
    );
}

#[test]
fn orient_torch_trapdoor_and_passthrough() {
    let registry = palette();
    // Torch on top face stands up.
    let torch_base = hash_of(
        &registry,
        "minecraft:torch",
        &[(
            "torch_facing_direction",
            BlockPropertyValue::Enum(Arc::from("unknown")),
        )],
    );
    let ctx = far_ctx(BlockFace::Up, 0.0, None);
    assert_eq!(
        orient_state(&registry, torch_base, &ctx),
        hash_of(
            &registry,
            "minecraft:torch",
            &[(
                "torch_facing_direction",
                BlockPropertyValue::Enum(Arc::from("top")),
            )]
        )
    );
    // Trapdoor: EWSN opposite of look (south look -> north=3), bottom.
    let trap_base = hash_of(
        &registry,
        "minecraft:trapdoor",
        &[
            ("direction", BlockPropertyValue::Int(0)),
            ("open_bit", BlockPropertyValue::Bool(false)),
            ("upside_down_bit", BlockPropertyValue::Bool(false)),
        ],
    );
    assert_eq!(
        orient_state(&registry, trap_base, &ctx),
        hash_of(
            &registry,
            "minecraft:trapdoor",
            &[
                ("direction", BlockPropertyValue::Int(3)),
                ("open_bit", BlockPropertyValue::Bool(false)),
                ("upside_down_bit", BlockPropertyValue::Bool(false)),
            ]
        )
    );
    // Blocks without orientation props (stone) keep the base hash.
    let stone = registry
        .state_of("minecraft:stone", &[])
        .expect("stone 应在 palette 中");
    let stone_hash = registry.runtime_id(stone).expect("stone hash").0;
    let west_ctx = far_ctx(BlockFace::West, 90.0, None);
    assert_eq!(orient_state(&registry, stone_hash, &west_ctx), stone_hash);
    // Unknown hashes never panic: base passes through.
    assert_eq!(orient_state(&registry, 0xDEAD_BEEF, &west_ctx), 0xDEAD_BEEF);
}
