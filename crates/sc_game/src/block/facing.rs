//! Placement orientation (facing): data-driven state selection.
//!
//! Rules (vanilla yaw/face semantics, wire-verified against real clients), applied in order, each gated on
//! the property actually existing on the block — unknown blocks keep their
//! default state, never a guessed orientation:
//!
//! 1. `pillar_axis` (x/y/z) ← clicked-face axis (logs, pillars, chains).
//! 2. `weirdo_direction` (stairs: N=2/E=1/S=3/W=0) ← look yaw, plus
//!    `upside_down_bit` ← clicked bottom face or upper-half side click.
//! 3. `facing_direction` (Int 0-5):
//!    - wall-mounted (ladder, `*_button`, end/lighting rods) ← clicked face;
//!    - hopper ← clicked-face opposite (DOWN on top/bottom faces);
//!    - pistons ← look direction (head away from player);
//!    - everything else (dispenser/dropper/barrel/...) ← eye rule
//!      (vertical override plus opposite horizontal).
//! 4. `minecraft:cardinal_direction` (furnace/chest/door) ← opposite look.
//! 5. `minecraft:facing_direction` (observer, names) ← eye rule, named.
//! 6. `torch_facing_direction` ← face (`top` on top face, face name on
//!    sides; ceiling clicks keep the default — no ceiling torch exists).
//! 7. `ground_sign_direction` (standing banner) ← yaw to 0-15.
//! 8. `direction` on trapdoors only (EWSN int) ← opposite look, plus the
//!    stairs upside-down rule; other `direction` blocks (bed, cocoa,
//!    bookshelf, bell, grindstone...) need neighbor/pattern reads and keep
//!    their default state.
//!
//! Deliberately out of scope (need multi-block transactions or neighbor
//! reads): door hinge/upper half, chest pairing, lever/bell/grindstone
//! `direction` families.

use sc_block::registry::BlockStateRegistry;
use sc_block::state::BlockPropertyValue;
use sc_entity::motion::Position;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use sc_world::chunk::BlockRuntimeId;

/// Face ordinals (match `face_offset`): DOWN=0, UP=1, NORTH=2, SOUTH=3,
/// WEST=4, EAST=5.
pub const FACE_DOWN: u8 = 0;
pub const FACE_UP: u8 = 1;

/// Look direction from yaw (yaw quadrants): SOUTH on
/// [315,45), WEST on [45,135), NORTH on [135,225), EAST on [225,315).
/// Returns a `facing_direction`-space value (2=N,3=S,4=W,5=E).
pub fn look_facing(yaw: f32) -> u8 {
    if !yaw.is_finite() {
        return 3;
    }
    let mut rotation = yaw % 360.0;
    if rotation < 0.0 {
        rotation += 360.0;
    }
    if rotation < 45.0 || rotation >= 315.0 {
        3
    } else if rotation < 135.0 {
        4
    } else if rotation < 225.0 {
        2
    } else {
        5
    }
}

/// Opposite face (furnace front faces the player who placed it).
pub fn opposite_face(face: u8) -> u8 {
    match face {
        0 => 1,
        1 => 0,
        2 => 3,
        3 => 2,
        4 => 5,
        5 => 4,
        other => other,
    }
}

/// Pillar axis for the clicked face.
pub fn face_axis(face: u8) -> &'static str {
    match face {
        0 | 1 => "y",
        2 | 3 => "z",
        _ => "x",
    }
}

/// Stairs direction from look facing (`weirdo_direction`: W=0/E=1/N=2/S=3).
pub fn weirdo_from_facing(face: u8) -> i32 {
    match face {
        2 => 2,
        3 => 3,
        4 => 0,
        5 => 1,
        _ => 2,
    }
}

/// Cardinal name for a horizontal facing value.
pub fn cardinal_name(face: u8) -> &'static str {
    match face {
        2 => "north",
        3 => "south",
        4 => "west",
        5 => "east",
        _ => "south",
    }
}

/// Observer-style direction name for a 6-way facing value.
pub fn observer_name(face: u8) -> &'static str {
    match face {
        0 => "down",
        1 => "up",
        2 => "north",
        3 => "south",
        4 => "west",
        5 => "east",
        _ => "south",
    }
}

/// Torch direction name for the clicked face.
pub fn torch_name(face: u8) -> Option<&'static str> {
    match face {
        1 => Some("top"),
        2 => Some("north"),
        3 => Some("south"),
        4 => Some("west"),
        5 => Some("east"),
        _ => None,
    }
}

/// Standing-banner direction 0-15 from yaw.
pub fn sign_direction(yaw: f32) -> i32 {
    if !yaw.is_finite() {
        return 0;
    }
    let mut rotation = yaw % 360.0;
    if rotation < 0.0 {
        rotation += 360.0;
    }
    ((rotation + 180.0 + 11.25) / 22.5).floor() as i32 & 15
}

/// Eye rule for 6-way blocks: vertical override near-above/below,
/// otherwise horizontal. `feet_y` is player feet, `eye` adds eye height,
/// `target` is the placement cell.
pub fn eye_facing(
    horizontal: u8,
    feet_y: f32,
    eye: f32,
    target_y: i32,
    player_x: f32,
    player_z: f32,
    target_x: i32,
    target_z: i32,
) -> u8 {
    if !feet_y.is_finite() || !eye.is_finite() {
        return horizontal;
    }
    let near = (player_x - (target_x as f32 + 0.5)).abs() < 2.0
        && (player_z - (target_z as f32 + 0.5)).abs() < 2.0;
    if !near {
        return horizontal;
    }
    let eye_y = feet_y + eye;
    if eye_y - target_y as f32 > 2.0 {
        1
    } else if target_y as f32 - eye_y > 0.0 {
        0
    } else {
        horizontal
    }
}

fn is_wall_mounted(name: &str) -> bool {
    name == "minecraft:ladder"
        || name == "minecraft:end_rod"
        || name == "minecraft:lightning_rod"
        || name.contains("wall_sign")
        || name.contains("hanging_sign")
        || name.ends_with("_button")
}

fn is_trapdoor(name: &str) -> bool {
    name.contains("trapdoor")
}

/// Trapdoor direction from look (EWSN compass order: E=0/W=1/S=2/N=3,
/// stored face is the opposite of the player direction).
pub fn trapdoor_dir_from_look(look: u8) -> i32 {
    match opposite_face(look) {
        2 => 3,
        3 => 2,
        4 => 1,
        5 => 0,
        _ => 2,
    }
}

fn is_hopper(name: &str) -> bool {
    name == "minecraft:hopper"
}

fn is_piston(name: &str) -> bool {
    name == "minecraft:piston" || name == "minecraft:sticky_piston"
}

fn states_of(
    registry: &BlockStateRegistry,
    hash: u32,
) -> Option<(sc_block::state::BlockStateId, String, Option<CompoundNbt>)> {
    let state_id = registry.by_runtime_id(BlockRuntimeId(hash))?;
    let entry = registry.state(state_id)?;
    let name = entry.name.to_string();
    Some((state_id, name, entry.states.clone()))
}

/// Build a value matching the palette's stored type: ints stay ints,
/// bytes stay bools. A type we do not recognize keeps the current state
/// (never a guessed encoding).
fn typed_value(current: Option<&NbtValue>, int_value: i32) -> Option<BlockPropertyValue> {
    match current {
        Some(NbtValue::Int(_)) => Some(BlockPropertyValue::Int(int_value)),
        Some(NbtValue::Byte(_)) => Some(BlockPropertyValue::Bool(int_value != 0)),
        _ => None,
    }
}

/// Resolve the placed state hash with orientation applied.
///
/// `yaw` is player body yaw (degrees), `face` the clicked face, `feet` the
/// player feet position, `target_y` the placement cell height and
/// `click_frac_y` the within-block click height when the protocol carries
/// it. Any unresolvable step keeps the running hash, so the worst case is
/// today's default-state placement.
#[allow(clippy::too_many_arguments)]
pub fn orient_state(
    registry: &BlockStateRegistry,
    base_hash: u32,
    face: u8,
    yaw: f32,
    feet: Position,
    target_x: i32,
    target_y: i32,
    target_z: i32,
    eye_height: f32,
    click_frac_y: Option<f32>,
) -> u32 {
    let Some((mut state_id, name, states)) = states_of(registry, base_hash) else {
        return base_hash;
    };
    let has = |key: &str| states.as_ref().is_some_and(|s| s.contains_key(key));

    // 1. Pillar axis from the clicked face.
    if has("pillar_axis") {
        if let Some(next) = registry.with_property(
            state_id,
            "pillar_axis",
            BlockPropertyValue::Enum(face_axis(face).into()),
        ) {
            state_id = next;
        }
        return commit(registry, state_id, base_hash);
    }

    let look = look_facing(yaw);

    // 2. Stairs: weirdo from look, upside-down from face/click height.
    if has("weirdo_direction") {
        if let Some(next) = registry.with_property(
            state_id,
            "weirdo_direction",
            BlockPropertyValue::Int(weirdo_from_facing(look)),
        ) {
            state_id = next;
        }
        if has("upside_down_bit") {
            state_id = apply_upside_down(registry, state_id, face, click_frac_y);
        }
        return commit(registry, state_id, base_hash);
    }

    // 3. Integer facing_direction (0-5).
    if has("facing_direction") {
        let facing = if is_wall_mounted(&name) {
            face.min(5)
        } else if is_hopper(&name) {
            // Hopper: opposite of the clicked face, DOWN on top/bottom.
            if face == FACE_DOWN || face == FACE_UP {
                0
            } else {
                opposite_face(face)
            }
        } else if is_piston(&name) {
            // Pistons point along the look direction (head away from the
            // player); vertical still follows the eye rule.
            eye_facing(
                look, feet.y, eye_height, target_y, feet.x, feet.z, target_x, target_z,
            )
        } else {
            eye_facing(
                opposite_face(look),
                feet.y,
                eye_height,
                target_y,
                feet.x,
                feet.z,
                target_x,
                target_z,
            )
        };
        if let Some(next) = registry.with_property(
            state_id,
            "facing_direction",
            BlockPropertyValue::Int(facing as i32),
        ) {
            state_id = next;
        }
        // Upside-down for blocks combining it with integer facing
        // (defensive: stairs/trapdoors return through their own rules).
        if has("upside_down_bit") {
            state_id = apply_upside_down(registry, state_id, face, click_frac_y);
        }
        return commit(registry, state_id, base_hash);
    }

    // 3b. Trapdoor direction (EWSN int) plus the stairs upside-down rule
    // (vanilla trapdoor placement). Other `direction` blocks (bed, cocoa,
    // bookshelf, bell, grindstone...) need neighbor/pattern reads and keep
    // their default state.
    if has("direction") && is_trapdoor(&name) {
        let states = registry
            .state(state_id)
            .and_then(|entry| entry.states.clone());
        if let Some(value) = typed_value(
            states.as_ref().and_then(|s| s.get("direction")),
            trapdoor_dir_from_look(look),
        ) {
            if let Some(next) = registry.with_property(state_id, "direction", value) {
                state_id = next;
            }
        }
        if has("upside_down_bit") {
            state_id = apply_upside_down(registry, state_id, face, click_frac_y);
        }
        return commit(registry, state_id, base_hash);
    }

    // 4. Cardinal facing (furnace/chest/door): opposite of look.
    if has("minecraft:cardinal_direction") {
        if let Some(next) = registry.with_property(
            state_id,
            "minecraft:cardinal_direction",
            BlockPropertyValue::Enum(cardinal_name(opposite_face(look)).into()),
        ) {
            state_id = next;
        }
        return commit(registry, state_id, base_hash);
    }

    // 5. Observer named facing: vanilla placement points along the look
    // direction (face away from the player); vertical follows the eye rule.
    if has("minecraft:facing_direction") {
        let facing = eye_facing(
            look, feet.y, eye_height, target_y, feet.x, feet.z, target_x, target_z,
        );
        if let Some(next) = registry.with_property(
            state_id,
            "minecraft:facing_direction",
            BlockPropertyValue::Enum(observer_name(facing).into()),
        ) {
            state_id = next;
        }
        return commit(registry, state_id, base_hash);
    }

    // 6. Torch facing: top on top face, face name on sides.
    if has("torch_facing_direction") {
        if let Some(name) = torch_name(face) {
            if let Some(next) = registry.with_property(
                state_id,
                "torch_facing_direction",
                BlockPropertyValue::Enum(name.into()),
            ) {
                state_id = next;
            }
        }
        return commit(registry, state_id, base_hash);
    }

    // 7. Standing banner direction from yaw.
    if has("ground_sign_direction") {
        let states = registry
            .state(state_id)
            .and_then(|entry| entry.states.clone());
        if let Some(value) = typed_value(
            states.as_ref().and_then(|s| s.get("ground_sign_direction")),
            sign_direction(yaw),
        ) {
            if let Some(next) = registry.with_property(state_id, "ground_sign_direction", value) {
                state_id = next;
            }
        }
        return commit(registry, state_id, base_hash);
    }

    commit(registry, state_id, base_hash)
}

fn commit(
    registry: &BlockStateRegistry,
    state_id: sc_block::state::BlockStateId,
    fallback: u32,
) -> u32 {
    registry
        .runtime_id(state_id)
        .map(|runtime| runtime.0)
        .unwrap_or(fallback)
}

/// Stairs/trapdoor upside-down bit (vanilla placement): bottom-face click (or
/// upper-half side click) goes on top. Re-reads the state after any prior
/// change so the stored value type (byte) is mirrored.
fn apply_upside_down(
    registry: &BlockStateRegistry,
    state_id: sc_block::state::BlockStateId,
    face: u8,
    click_frac_y: Option<f32>,
) -> sc_block::state::BlockStateId {
    let upside = face == FACE_DOWN
        || (face != FACE_UP && click_frac_y.is_some_and(|frac| frac.is_finite() && frac > 0.5));
    let states = registry
        .state(state_id)
        .and_then(|entry| entry.states.clone());
    if let Some(value) = typed_value(
        states.as_ref().and_then(|s| s.get("upside_down_bit")),
        if upside { 1 } else { 0 },
    ) {
        registry
            .with_property(state_id, "upside_down_bit", value)
            .unwrap_or(state_id)
    } else {
        state_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_block::state::BlockPropertyValue;
    use std::sync::Arc;

    #[test]
    fn look_follows_yaw_quadrants() {
        // Yaw quadrants: SOUTH on [315,45), WEST on [45,135),
        // NORTH on [135,225), EAST on [225,315).
        assert_eq!(look_facing(0.0), 3);
        assert_eq!(look_facing(44.9), 3);
        assert_eq!(look_facing(45.0), 4);
        assert_eq!(look_facing(90.0), 4);
        assert_eq!(look_facing(180.0), 2);
        assert_eq!(look_facing(270.0), 5);
        assert_eq!(look_facing(315.0), 3);
        assert_eq!(look_facing(-90.0), 5);
        assert_eq!(look_facing(f32::NAN), 3);
    }

    #[test]
    fn face_math_helpers() {
        assert_eq!(opposite_face(0), 1);
        assert_eq!(opposite_face(1), 0);
        assert_eq!(opposite_face(2), 3);
        assert_eq!(opposite_face(4), 5);
        assert_eq!(face_axis(0), "y");
        assert_eq!(face_axis(1), "y");
        assert_eq!(face_axis(2), "z");
        assert_eq!(face_axis(5), "x");
        // weirdo: W=0/E=1/N=2/S=3.
        assert_eq!(weirdo_from_facing(4), 0);
        assert_eq!(weirdo_from_facing(5), 1);
        assert_eq!(weirdo_from_facing(2), 2);
        assert_eq!(weirdo_from_facing(3), 3);
        assert_eq!(cardinal_name(2), "north");
        assert_eq!(observer_name(0), "down");
        assert_eq!(torch_name(1), Some("top"));
        assert_eq!(torch_name(0), None);
        // Trapdoor EWSN (E=0/W=1/S=2/N=3) stores the opposite of look.
        assert_eq!(trapdoor_dir_from_look(3), 3);
        assert_eq!(trapdoor_dir_from_look(2), 2);
        assert_eq!(trapdoor_dir_from_look(4), 0);
        assert_eq!(trapdoor_dir_from_look(5), 1);
    }

    #[test]
    fn eye_rule_needs_nearness_for_vertical_override() {
        // Far player: horizontal passes through even when high above.
        assert_eq!(eye_facing(2, 70.0, 1.62, 65, 100.5, 100.5, 0, 0), 2);
        // Near + eyes >2 above target: UP; near + target above eyes: DOWN.
        assert_eq!(eye_facing(2, 70.0, 1.62, 65, 0.5, 0.5, 0, 0), 1);
        assert_eq!(eye_facing(2, 60.0, 1.62, 65, 0.5, 0.5, 0, 0), 0);
        // Near + level: horizontal.
        assert_eq!(eye_facing(4, 65.0, 1.62, 65, 0.5, 0.5, 0, 0), 4);
        // Non-finite inputs keep horizontal (never a guessed direction).
        assert_eq!(eye_facing(2, f32::NAN, 1.62, 65, 0.5, 0.5, 0, 0), 2);
    }

    /// Real palette (auto-selected version-pack fixture; skips when absent).
    fn load_palette() -> Option<BlockStateRegistry> {
        let bytes =
            match sc_packloader::test_pack::read_test_pack_file("definitions/block_palette.nbt") {
                Some(bytes) => bytes,
                None => {
                    eprintln!("[skip] 版本包 palette 缺失，跳过朝向测试");
                    return None;
                }
            };
        let registry = BlockStateRegistry::new();
        registry
            .load_palette_from_bytes(&bytes)
            .expect("palette 解析");
        Some(registry)
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

    fn far_feet() -> Position {
        Position::new(100.5, 65.0, 100.5)
    }

    #[test]
    fn orient_pillar_axis_from_clicked_face() {
        let Some(registry) = load_palette() else {
            return;
        };
        let base = hash_of(
            &registry,
            "minecraft:oak_log",
            &[("pillar_axis", BlockPropertyValue::Enum(Arc::from("y")))],
        );
        // Side click (north face) lays the log along z.
        let out = orient_state(&registry, base, 2, 0.0, far_feet(), 0, 65, 0, 1.62, None);
        assert_eq!(
            out,
            hash_of(
                &registry,
                "minecraft:oak_log",
                &[("pillar_axis", BlockPropertyValue::Enum(Arc::from("z")))]
            )
        );
        // Top click keeps y.
        assert_eq!(
            orient_state(&registry, base, 1, 0.0, far_feet(), 0, 65, 0, 1.62, None),
            base
        );
    }

    #[test]
    fn orient_stairs_weirdo_and_upside() {
        let Some(registry) = load_palette() else {
            return;
        };
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
        let out = orient_state(&registry, base, 1, 0.0, far_feet(), 0, 65, 0, 1.62, None);
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
        let out = orient_state(&registry, base, 0, 0.0, far_feet(), 0, 65, 0, 1.62, None);
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
        let out = orient_state(
            &registry,
            base,
            2,
            0.0,
            far_feet(),
            0,
            65,
            0,
            1.62,
            Some(0.7),
        );
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
        let Some(registry) = load_palette() else {
            return;
        };
        let base = hash_of(
            &registry,
            "minecraft:furnace",
            &[(
                "minecraft:cardinal_direction",
                BlockPropertyValue::Enum(Arc::from("south")),
            )],
        );
        // Look east (yaw 270): front faces west (toward the player).
        let out = orient_state(&registry, base, 1, 270.0, far_feet(), 0, 65, 0, 1.62, None);
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
        let Some(registry) = load_palette() else {
            return;
        };
        let base = hash_of(
            &registry,
            "minecraft:dispenser",
            &[
                ("facing_direction", BlockPropertyValue::Int(3)),
                ("triggered_bit", BlockPropertyValue::Bool(false)),
            ],
        );
        // Look south, far: hole faces north (opposite of look).
        let out = orient_state(&registry, base, 1, 0.0, far_feet(), 0, 65, 0, 1.62, None);
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
        let out = orient_state(
            &registry,
            base,
            1,
            0.0,
            Position::new(0.5, 70.0, 0.5),
            0,
            65,
            0,
            1.62,
            None,
        );
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
        let out = orient_state(
            &registry,
            hopper_base,
            4,
            0.0,
            far_feet(),
            0,
            65,
            0,
            1.62,
            None,
        );
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
        let Some(registry) = load_palette() else {
            return;
        };
        // Piston head extends along look (west on yaw 90).
        let base = hash_of(
            &registry,
            "minecraft:piston",
            &[("facing_direction", BlockPropertyValue::Int(3))],
        );
        assert_eq!(
            orient_state(&registry, base, 1, 90.0, far_feet(), 0, 65, 0, 1.62, None),
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
            orient_state(
                &registry,
                obs_base,
                1,
                90.0,
                far_feet(),
                0,
                65,
                0,
                1.62,
                None
            ),
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
        let Some(registry) = load_palette() else {
            return;
        };
        // Torch on top face stands up.
        let torch_base = hash_of(
            &registry,
            "minecraft:torch",
            &[(
                "torch_facing_direction",
                BlockPropertyValue::Enum(Arc::from("unknown")),
            )],
        );
        assert_eq!(
            orient_state(
                &registry,
                torch_base,
                1,
                0.0,
                far_feet(),
                0,
                65,
                0,
                1.62,
                None
            ),
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
            orient_state(
                &registry,
                trap_base,
                1,
                0.0,
                far_feet(),
                0,
                65,
                0,
                1.62,
                None
            ),
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
        assert_eq!(
            orient_state(
                &registry,
                stone_hash,
                4,
                90.0,
                far_feet(),
                0,
                65,
                0,
                1.62,
                None
            ),
            stone_hash
        );
        // Unknown hashes never panic: base passes through.
        assert_eq!(
            orient_state(
                &registry,
                0xDEAD_BEEF,
                4,
                90.0,
                far_feet(),
                0,
                65,
                0,
                1.62,
                None
            ),
            0xDEAD_BEEF
        );
    }
}
