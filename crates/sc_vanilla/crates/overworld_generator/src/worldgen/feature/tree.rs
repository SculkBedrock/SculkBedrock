//! Port of the `object/` tree generator series.
//!
//! | Rust item | Upstream source |
//! |---|---|
//! | [`FallenTree`] | `ObjectFallenTree.java` |
//! | [`SmallSpruceTree`] | `ObjectSmallSpruceTree.java` |
//! | [`SwampOakTree`] | `ObjectSwampOakTree.java` |
//! | [`SavannaTree`] | `ObjectSavannaTree.java` |
//! | [`JungleTree`] | `ObjectJungleTree.java` |
//! | [`JungleBush`] | `ObjectJungleBush.java` |
//! | [`BigSpruceTree`] | `ObjectBigSpruceTree.java` |
//! | [`FancyOakTree`] | `ObjectFancyOakTree.java` |
//! | [`DarkOakTree`] | `ObjectDarkOakTree.java` |
//! | [`CherryTree`] | `ObjectCherryTree.java` |
//! | [`AzaleaTree`] | `ObjectAzaleaTree.java` |
//! | [`JungleBigTree`] | `ObjectJungleBigTree.java` |
//! | [`PaleOakTree`] | `ObjectPaleOakTree.java` |
//! | [`SmallPaleOakTree`] | `ObjectSmallPaleOakTree.java` |
//! | [`MangroveTree`] | `ObjectMangroveTree.java` |
//!
//! Shared helpers (from `TreeGenerator.java`): `add_vine`/`add_vines_around_log`
//! /`set_dirt_at`.

use std::sync::Arc;

use sc_log::t_log;
use sc_world::chunk::BlockRuntimeId;

use crate::worldgen::context::BlockManager;
use crate::worldgen::feature::object::{Axis, ObjectGenerator, TreeBlockTable, WoodType};
use crate::worldgen::math::random_range;
use crate::worldgen::random::{MtRandom, RandomSourceProvider, Xoroshiro128};
use crate::worldgen::stages::terrain::SEA_LEVEL;

/// Java: `TreeGenerator.TREE_WITH_VINES_CHANCE`(L14).
pub const TREE_WITH_VINES_CHANCE: i32 = 20;

/// Horizontal directions (N/E/S/W order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HorizontalFace {
    North,
    East,
    South,
    West,
}

impl HorizontalFace {
    /// Java: `Plane.HORIZONTAL.random(rand)`(L425-427)——
    /// `faces[rand.nextInt(4)]`,faces = {NORTH, EAST, SOUTH, WEST}.
    pub fn random(rand: &mut Xoroshiro128) -> Self {
        match rand.next_int_max(4) {
            0 => HorizontalFace::North,
            1 => HorizontalFace::East,
            2 => HorizontalFace::South,
            _ => HorizontalFace::West,
        }
    }

    /// Java: `BlockFace.getOffset()`(NORTH (0,0,-1) / EAST (1,0,0) /
    /// SOUTH (0,0,1) / WEST (-1,0,0)).
    pub fn offset(self) -> (i32, i32) {
        match self {
            HorizontalFace::North => (0, -1),
            HorizontalFace::East => (1, 0),
            HorizontalFace::South => (0, 1),
            HorizontalFace::West => (-1, 0),
        }
    }

    /// Java: `BlockFace.getAxis()`(N/S → Z,E/W → X).
    pub fn axis(self) -> Axis {
        match self {
            HorizontalFace::North | HorizontalFace::South => Axis::Z,
            HorizontalFace::East | HorizontalFace::West => Axis::X,
        }
    }

    /// Iteration order is NORTH, EAST, SOUTH, WEST.
    pub const ALL: [HorizontalFace; 4] = [
        HorizontalFace::North,
        HorizontalFace::East,
        HorizontalFace::South,
        HorizontalFace::West,
    ];

    /// Java: `BlockFace.getHorizontalIndex()`——S=0, W=1, N=2, E=3
    /// (Third constructor parameter.)
    pub fn horizontal_index(self) -> usize {
        match self {
            HorizontalFace::South => 0,
            HorizontalFace::West => 1,
            HorizontalFace::North => 2,
            HorizontalFace::East => 3,
        }
    }

    /// Java: `BlockFace.getOpposite()`(N↔S,E↔W).
    pub fn opposite(self) -> Self {
        match self {
            HorizontalFace::North => HorizontalFace::South,
            HorizontalFace::East => HorizontalFace::West,
            HorizontalFace::South => HorizontalFace::North,
            HorizontalFace::West => HorizontalFace::East,
        }
    }
}

// ---------------------------------------------------------------------------
// TreeGenerator shared helpers.
// ---------------------------------------------------------------------------

/// Java: `TreeGenerator.addVinesAroundLog`(L73-78).
pub fn add_vines_around_log(
    table: &TreeBlockTable,
    level: &mut BlockManager<'_>,
    x: i32,
    y: i32,
    z: i32,
) {
    add_vine(table, level, x - 1, y, z, 8);
    add_vine(table, level, x + 1, y, z, 2);
    add_vine(table, level, x, y, z - 1, 1);
    add_vine(table, level, x, y, z + 1, 4);
}

/// addVine: vines only on air.
fn add_vine(
    table: &TreeBlockTable,
    level: &mut BlockManager<'_>,
    x: i32,
    y: i32,
    z: i32,
    meta: u8,
) {
    if level.get_block_if_cached_or_loaded(x, y, z) == table.air {
        level.set_block_state_at(x, y, z, 0, table.vine_state(meta));
    }
}

/// Java: `TreeGenerator.setDirtAt`(L69-71).
fn set_dirt_at(table: &TreeBlockTable, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) {
    level.set_block_state_at(x, y, z, 0, table.dirt);
}

// ---------------------------------------------------------------------------
// ObjectFallenTree
// ---------------------------------------------------------------------------

/// Java: `ObjectFallenTree.java`.
pub struct FallenTree {
    wood: WoodType,
    min_log_length: i32,
    max_log_length: i32,
    table: Arc<TreeBlockTable>,
}

/// Java: `FALLEN_LOG_MAX_GROUND_GAP`(L15).
const FALLEN_LOG_MAX_GROUND_GAP: i32 = 2;
/// Java: `MAX_MUSHROOMS`(L16).
const MAX_MUSHROOMS: i32 = 2;

impl FallenTree {
    /// Java: `ObjectFallenTree()`(L22-24)——OAK 3-7.
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self {
            wood: WoodType::Oak,
            min_log_length: 3,
            max_log_length: 7,
            table,
        }
    }

    /// Java: `ObjectFallenTree(WoodType)`(L26-28).
    pub fn of_wood(table: Arc<TreeBlockTable>, wood: WoodType) -> Self {
        Self {
            wood,
            min_log_length: 3,
            max_log_length: 7,
            table,
        }
    }

    /// Java: `ObjectFallenTree(WoodType, int, int)`(L30-34).
    pub fn with_lengths(
        table: Arc<TreeBlockTable>,
        wood: WoodType,
        min_log_length: i32,
        max_log_length: i32,
    ) -> Self {
        Self {
            wood,
            min_log_length,
            max_log_length,
            table,
        }
    }

    /// Java: `sampleLogLength`(L66-72).
    fn sample_log_length(&self, rand: &mut Xoroshiro128) -> i32 {
        if self.max_log_length <= self.min_log_length {
            return self.min_log_length.max(1);
        }
        self.min_log_length + rand.next_int_max(self.max_log_length - self.min_log_length + 1)
    }

    /// Java: `setGroundHeightForFallenLogStartPos`(L74-84)——
    /// Up to 6 downward tries after y++.
    fn set_ground_height_for_fallen_log_start_pos(
        &self,
        level: &mut BlockManager<'_>,
        pos: &mut (i32, i32, i32),
    ) {
        pos.1 += 1;
        for _ in 0..6 {
            if self.may_place_on(level, *pos) {
                return;
            }
            pos.1 -= 1;
        }
    }

    /// Java: `canPlaceEntireFallenLog`(L86-107).
    fn can_place_entire_fallen_log(
        &self,
        level: &mut BlockManager<'_>,
        log_length: i32,
        start: (i32, i32, i32),
        direction: HorizontalFace,
    ) -> bool {
        let mut gap_in_ground = 0;
        let (dx, dz) = direction.offset();
        let mut current = start;

        for _ in 0..log_length {
            if !self.valid_fallen_log_pos(level, current) {
                return false;
            }
            if !self.is_over_solid_ground(level, current) {
                gap_in_ground += 1;
                if gap_in_ground > FALLEN_LOG_MAX_GROUND_GAP {
                    return false;
                }
            } else {
                gap_in_ground = 0;
            }
            current.0 += dx;
            current.2 += dz;
        }
        true
    }

    /// placeFallenLog returns the fallen-log coordinates.
    fn place_fallen_log(
        &self,
        level: &mut BlockManager<'_>,
        log_length: i32,
        start: (i32, i32, i32),
        direction: HorizontalFace,
    ) -> Vec<(i32, i32, i32)> {
        let mut fallen_log = Vec::with_capacity(log_length as usize);
        let (dx, dz) = direction.offset();
        let mut current = start;
        let axis = direction.axis();
        for _ in 0..log_length {
            self.place_log_block(level, current, axis);
            fallen_log.push(current);
            current.0 += dx;
            current.2 += dz;
        }
        fallen_log
    }

    /// decorateStump: stump vines for OAK/JUNGLE.
    fn decorate_stump(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        stump: (i32, i32, i32),
    ) {
        if self.wood != WoodType::Oak && self.wood != WoodType::Jungle {
            return;
        }
        let (x, y, z) = stump;
        self.place_stump_vine(level, rand, (x - 1, y, z), 8);
        self.place_stump_vine(level, rand, (x + 1, y, z), 2);
        self.place_stump_vine(level, rand, (x, y, z - 1), 1);
        self.place_stump_vine(level, rand, (x, y, z + 1), 4);
    }

    /// Java: `placeStumpVine`(L132-138).
    fn place_stump_vine(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        pos: (i32, i32, i32),
        meta: u8,
    ) {
        if rand.next_int_max(4) == 0
            || level.get_block_if_cached_or_loaded(pos.0, pos.1, pos.2) != self.table.air
        {
            return;
        }
        level.set_block_state_at(pos.0, pos.1, pos.2, 0, self.table.vine_state(meta));
    }

    /// decorateFallenLog: mushroom dressing.
    fn decorate_fallen_log(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        fallen_log: &[(i32, i32, i32)],
    ) {
        if fallen_log.is_empty() || rand.next_int_max(4) != 0 {
            return;
        }
        let mushrooms = 1 + rand.next_int_max(MAX_MUSHROOMS);
        for _ in 0..mushrooms {
            let log_pos = fallen_log[rand.next_int_max(fallen_log.len() as i32) as usize];
            let mushroom_pos = (log_pos.0, log_pos.1 + 1, log_pos.2);
            if level.get_block_if_cached_or_loaded(mushroom_pos.0, mushroom_pos.1, mushroom_pos.2)
                == self.table.air
            {
                let mushroom = if rand.next_boolean() {
                    self.table.red_mushroom
                } else {
                    self.table.brown_mushroom
                };
                level.set_block_state_at(
                    mushroom_pos.0,
                    mushroom_pos.1,
                    mushroom_pos.2,
                    0,
                    mushroom,
                );
            }
        }
    }

    /// Java: `mayPlaceOn`(L161-163).
    fn may_place_on(&self, level: &mut BlockManager<'_>, pos: (i32, i32, i32)) -> bool {
        self.valid_fallen_log_pos(level, pos) && self.is_over_solid_ground(level, pos)
    }

    /// validFallenLogPos: y range plus non-solid.
    fn valid_fallen_log_pos(&self, level: &mut BlockManager<'_>, pos: (i32, i32, i32)) -> bool {
        if pos.1 < level.min_height() || pos.1 >= level.max_height() {
            return false;
        }
        let block = level.get_block_if_cached_or_loaded(pos.0, pos.1, pos.2);
        !self.table.is_solid(block)
    }

    /// Java: `isOverSolidGround`(L174-176).
    fn is_over_solid_ground(&self, level: &mut BlockManager<'_>, pos: (i32, i32, i32)) -> bool {
        let below = level.get_block_if_cached_or_loaded(pos.0, pos.1 - 1, pos.2);
        self.table.is_solid(below)
    }

    /// Java: `placeLogBlock`(L178-180).
    fn place_log_block(&self, level: &mut BlockManager<'_>, pos: (i32, i32, i32), axis: Axis) {
        level.set_block_state_at(
            pos.0,
            pos.1,
            pos.2,
            0,
            self.table.log_state(self.wood, axis),
        );
    }
}

impl ObjectGenerator for FallenTree {
    /// generate plus hook cleanup of unplantable flowers.
    /// No block-property system here; the approximation only plants flowers
    /// on solid blocks, so removals stay rare.
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let origin = (x, y, z);
        if !self.may_place_on(level, origin) {
            return false;
        }

        self.place_log_block(level, origin, Axis::Y);
        self.decorate_stump(level, rand, origin);

        let direction = HorizontalFace::random(rand);
        let log_length = self.sample_log_length(rand);
        let (dx, dz) = direction.offset();
        let step = 2 + rand.next_int_max(2);
        let mut log_start_pos = (x + dx * step, y, z + dz * step);
        self.set_ground_height_for_fallen_log_start_pos(level, &mut log_start_pos);

        if self.can_place_entire_fallen_log(level, log_length, log_start_pos, direction) {
            let fallen_log = self.place_fallen_log(level, log_length, log_start_pos, direction);
            self.decorate_fallen_log(level, rand, &fallen_log);
        }
        true
    }
}

// ---------------------------------------------------------------------------
// ObjectSmallSpruceTree
// ---------------------------------------------------------------------------

/// Java: `ObjectSmallSpruceTree.java`.
pub struct SmallSpruceTree {
    table: Arc<TreeBlockTable>,
}

impl SmallSpruceTree {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }

    /// placeTreeOfHeight checks the whole volume for grow-through.
    fn place_tree_of_height(
        &self,
        level: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        height: i32,
    ) -> bool {
        for dy in 0..=height + 1 {
            let r = if dy == 0 {
                0
            } else if dy >= height - 1 {
                2
            } else {
                1
            };
            for dx in -r..=r {
                for dz in -r..=r {
                    let block = level.get_block_if_cached_or_loaded(x + dx, y + dy, z + dz);
                    if !self.table.can_grow_into(block) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Java: `placeLogAt`(L105-109).
    fn place_log_at(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) {
        let block = level.get_block_if_cached_or_loaded(x, y, z);
        if self.table.can_grow_into(block) {
            level.set_block_state_at(x, y, z, 0, self.table.log_state(WoodType::Spruce, Axis::Y));
        }
    }

    /// placeLeafAt: leaves only on air/snow_layer/vine.
    fn place_leaf_at(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) {
        let block = level.get_block_if_cached_or_loaded(x, y, z);
        if block == self.table.air || block == self.table.snow_layer || block == self.table.vine {
            level.set_block_state_at(
                x,
                y,
                z,
                0,
                self.table.leaves[WoodType::ALL
                    .iter()
                    .position(|w| *w == WoodType::Spruce)
                    .unwrap_or(0)],
            );
        }
    }
}

impl ObjectGenerator for SmallSpruceTree {
    /// Java: `generate`(L20-83).
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let height = 6 + rand.next_int_max(4);
        let tree_with_vines = rand.next_int_max(TREE_WITH_VINES_CHANCE) == 0;
        let base_x = x;
        let base_y = y;
        let base_z = z;

        // baseY bounds with the hardcoded 256 cap.
        if base_y < 1 || base_y + height + 2 >= 256 {
            return false;
        }

        // Ground must be grass_block/dirt/podzol.
        let ground = level.get_block_if_cached_or_loaded(base_x, base_y - 1, base_z);
        if ground != self.table.grass_block
            && ground != self.table.dirt
            && ground != self.table.podzol
        {
            return false;
        }

        if !self.place_tree_of_height(level, base_x, base_y, base_z, height) {
            return false;
        }

        // Trunk.
        let trunk_height = height - rand.next_int_max(3);
        for dy in 0..trunk_height {
            self.place_log_at(level, base_x, base_y + dy, base_z);
            if tree_with_vines {
                add_vines_around_log(&self.table, level, base_x, base_y + dy, base_z);
            }
        }

        // Top conical leaves.
        let top_size = height - (1 + rand.next_int_max(2));
        let l_radius = 2 + rand.next_int_max(2);
        let mut radius = rand.next_int_max(2);
        let mut max_r = 1;
        let mut min_r = 0;

        for yy in 0..=top_size {
            let yyy = base_y + height - yy;
            for xx in base_x - radius..=base_x + radius {
                let x_off = (xx - base_x).abs();
                for zz in base_z - radius..=base_z + radius {
                    let z_off = (zz - base_z).abs();
                    if x_off == radius && z_off == radius && radius > 0 {
                        continue;
                    }
                    let block = level.get_block_if_cached_or_loaded(xx, yyy, zz);
                    if !self.table.can_grow_into(block) {
                        continue;
                    }
                    self.place_leaf_at(level, xx, yyy, zz);
                }
            }
            if radius >= max_r {
                radius = min_r;
                min_r = 1;
                max_r += 1;
                if max_r > l_radius {
                    max_r = l_radius;
                }
            } else {
                radius += 1;
            }
        }

        true
    }
}

// ---------------------------------------------------------------------------
// ObjectSwampOakTree
// ---------------------------------------------------------------------------

/// Java: `ObjectSwampOakTree.java`.
pub struct SwampOakTree {
    min_tree_height: i32,
    max_tree_height: i32,
    table: Arc<TreeBlockTable>,
}

impl SwampOakTree {
    /// Java: `ObjectSwampOakTree(int, int)`(L23-26).
    pub fn new(table: Arc<TreeBlockTable>, min_tree_height: i32, max_tree_height: i32) -> Self {
        Self {
            min_tree_height,
            max_tree_height,
            table,
        }
    }

    /// addHangingVine: vines hang 3-5 below leaves.
    ///
    /// Note: upstream draws the length from unseeded randomness; this port
    /// uses a coordinate-derived hash (deterministic).
    fn add_hanging_vine(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32, meta: u8) {
        let mut dy = y - 1;
        let length = 3 + rand_len_3(x, z);
        for _ in 0..length {
            if level.get_block_if_cached_or_loaded(x, dy, z) == self.table.air && dy > SEA_LEVEL {
                level.set_block_state_at(x, dy, z, 0, self.table.vine_state(meta));
                dy -= 1;
            } else {
                break;
            }
        }
    }

    /// Java: `randomVineMeta`(L107-110)——`metas[nextInt(len-1)]`,
    /// Upstream quirk kept: `nextInt(3)` only hits {1, 2, 4}, never 8.
    fn random_vine_meta(&self, rand: &mut Xoroshiro128) -> u8 {
        const METAS: [u8; 4] = [1, 2, 4, 8];
        METAS[rand.next_int_max(METAS.len() as i32 - 1) as usize]
    }
}

/// Deterministic length sampling for `addHangingVine`.
fn rand_len_3(x: i32, z: i32) -> i32 {
    ((x.wrapping_mul(31).wrapping_add(z)) & 0x7FFF) % 3
}

impl ObjectGenerator for SwampOakTree {
    /// Java: `generate`(L29-87).
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let mut height = rand.next_int_max(self.max_tree_height - self.min_tree_height + 1)
            + self.min_tree_height;

        if y < level.min_height() || y + height + 1 >= level.max_height() {
            return false;
        }

        // Ground needs the DIRT tag.
        let ground = level.get_block_if_cached_or_loaded(x, y - 1, z);
        if !self.table.is_support_dirt(ground) {
            return false;
        }

        set_dirt_at(&self.table, level, x, y - 1, z);

        // Trunk (grows taller through water).
        for dy in 0..height {
            let block = level.get_block_if_cached_or_loaded(x, y + dy, z);
            if block == self.table.air
                || block == self.table.leaves[WoodType::Oak.index()]
                || block == self.table.vine
                || self.table.is_water(block)
            {
                level.set_block_state_at(
                    x,
                    y + dy,
                    z,
                    0,
                    self.table.log_state(WoodType::Oak, Axis::Y),
                );
                if self.table.is_water(block) {
                    height += 1;
                }
            }
        }

        // Top canopy plus hanging vines.
        for yy in y - 3 + height..=y + height {
            let y_off = (yy - (y + height)) as f64;
            let mid = (1.0 - y_off / 2.0) as i32;

            for xx in x - mid..=x + mid {
                let x_off = (xx - x).abs();
                for zz in z - mid..=z + mid {
                    let z_off = (zz - z).abs();

                    // Corner trim (yOff==0 or random skip).
                    if x_off == mid && z_off == mid && (y_off == 0.0 || rand.next_int_max(2) == 0) {
                        continue;
                    }

                    let block_at = level.get_block_if_cached_or_loaded(xx, yy, zz);
                    if !self.table.is_solid(block_at) {
                        level.set_block_state_at(
                            xx,
                            yy,
                            zz,
                            0,
                            self.table.leaves[WoodType::Oak.index()],
                        );
                        if rand.next_int_max(4) == 0 {
                            let meta = self.random_vine_meta(rand);
                            self.add_hanging_vine(level, xx, yy, zz, meta);
                        }
                    }
                }
            }
        }

        true
    }
}

// ---------------------------------------------------------------------------
// ObjectSavannaTree
// ---------------------------------------------------------------------------

/// Acacia trees.
pub struct SavannaTree {
    table: Arc<TreeBlockTable>,
}

impl SavannaTree {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }

    /// placeLogAt places unconditionally.
    fn place_log_at(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) {
        level.set_block_state_at(x, y, z, 0, self.table.log_state(WoodType::Acacia, Axis::Y));
    }

    /// placeLeafAt: leaves only on air/leaves.
    fn place_leaf_at(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) {
        let block = level.get_block_if_cached_or_loaded(x, y, z);
        if block == self.table.air || self.table.is_leaves(block) {
            level.set_block_state_at(x, y, z, 0, self.table.leaves[WoodType::Acacia.index()]);
        }
    }
}

impl ObjectGenerator for SavannaTree {
    /// generate keeps the hardcoded pre-1.18 256 world height.
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let i = rand.next_int_max(3) + rand.next_int_max(3) + 5;
        let mut flag = true;

        if y >= 1 && y + i + 1 <= 256 {
            // Whole-volume grow-through check in three bands.
            for j in y..=y + 1 + i {
                let k = if j == y {
                    0
                } else if j >= y + 1 + i - 2 {
                    2
                } else {
                    1
                };

                for l in x - k..=x + k {
                    if !flag {
                        break;
                    }
                    for i1 in z - k..=z + k {
                        if !flag {
                            break;
                        }
                        if j >= 0 && j < 256 {
                            if !self
                                .table
                                .can_grow_into(level.get_block_if_cached_or_loaded(l, j, i1))
                            {
                                flag = false;
                            }
                        } else {
                            flag = false;
                        }
                    }
                }
            }

            if !flag {
                return false;
            }

            // Ground is grass/dirt.
            let ground = level.get_block_if_cached_or_loaded(x, y - 1, z);
            if (ground == self.table.grass_block || ground == self.table.dirt) && y < 256 - i - 1 {
                set_dirt_at(&self.table, level, x, y - 1, z);
                let face = HorizontalFace::random(rand);
                let (fx, fz) = face.offset();
                let k2 = i - rand.next_int_max(4) - 1;
                let mut l2 = 3 - rand.next_int_max(3);
                let mut i3 = x;
                let mut j1 = z;
                let mut k1 = 0;

                // Main trunk (bent top).
                for l1 in 0..i {
                    let i2 = y + l1;
                    if l1 >= k2 && l2 > 0 {
                        i3 += fx;
                        j1 += fz;
                        l2 -= 1;
                    }
                    let block = level.get_block_if_cached_or_loaded(i3, i2, j1);
                    if block == self.table.air || self.table.is_leaves(block) {
                        self.place_log_at(level, i3, i2, j1);
                        k1 = i2;
                    }
                }

                // Main 7x7 de-cornered canopy.
                for j3 in -3i32..=3 {
                    for i4 in -3i32..=3 {
                        if j3.abs() != 3 || i4.abs() != 3 {
                            self.place_leaf_at(level, i3 + j3, k1, j1 + i4);
                        }
                    }
                }
                // Upper 3x3 plus four 2-block arms.
                for k3 in -1..=1 {
                    for j4 in -1..=1 {
                        self.place_leaf_at(level, i3 + k3, k1 + 1, j1 + j4);
                    }
                }
                self.place_leaf_at(level, i3 + 2, k1 + 1, j1);
                self.place_leaf_at(level, i3 - 2, k1 + 1, j1);
                self.place_leaf_at(level, i3, k1 + 1, j1 + 2);
                self.place_leaf_at(level, i3, k1 + 1, j1 - 2);

                // Secondary trunk (when face1 differs from face).
                i3 = x;
                j1 = z;
                let face1 = HorizontalFace::random(rand);

                if face1 != face {
                    let (f1x, f1z) = face1.offset();
                    let l3 = k2 - rand.next_int_max(2) - 1;
                    let mut k4 = 1 + rand.next_int_max(3);
                    k1 = 0;

                    let mut l4 = l3;
                    while l4 < i && k4 > 0 {
                        if l4 >= 1 {
                            let j2 = y + l4;
                            i3 += f1x;
                            j1 += f1z;
                            let block = level.get_block_if_cached_or_loaded(i3, j2, j1);
                            if block == self.table.air || self.table.is_leaves(block) {
                                self.place_log_at(level, i3, j2, j1);
                                k1 = j2;
                            }
                        }
                        l4 += 1;
                        k4 -= 1;
                    }

                    if k1 > 0 {
                        for i5 in -2i32..=2 {
                            for k5 in -2i32..=2 {
                                if i5.abs() != 2 || k5.abs() != 2 {
                                    self.place_leaf_at(level, i3 + i5, k1, j1 + k5);
                                }
                            }
                        }
                        for j5 in -1..=1 {
                            for l5 in -1..=1 {
                                self.place_leaf_at(level, i3 + j5, k1 + 1, j1 + l5);
                            }
                        }
                    }
                }

                true
            } else {
                false
            }
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// ObjectJungleTree
// ---------------------------------------------------------------------------

/// Java: `ObjectJungleTree.java`.
pub struct JungleTree {
    min_tree_height: i32,
    max_tree_height: i32,
    table: Arc<TreeBlockTable>,
}

impl JungleTree {
    /// Java: `ObjectJungleTree(int, int)`(L40-43).
    pub fn new(table: Arc<TreeBlockTable>, min_tree_height: i32, max_tree_height: i32) -> Self {
        Self {
            min_tree_height,
            max_tree_height,
            table,
        }
    }

    /// placeCocoa: direction is the side horizontal index.
    fn place_cocoa(
        &self,
        level: &mut BlockManager<'_>,
        age: i32,
        x: i32,
        y: i32,
        z: i32,
        side: HorizontalFace,
    ) {
        let state = self.table.cocoa_states[side.horizontal_index()][age.clamp(0, 2) as usize];
        level.set_block_state_at(x, y, z, 0, state);
    }

    /// addVine places unconditionally.
    fn add_vine_direct(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32, meta: u8) {
        level.set_block_state_at(x, y, z, 0, self.table.vine_state(meta));
    }

    /// addHangingVine: first cell plus up to 4 hanging.
    fn add_hanging_vine(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32, meta: u8) {
        self.add_vine_direct(level, x, y, z, meta);
        let mut i = 4;
        let mut dy = y - 1;
        while i > 0 && level.get_block_if_cached_or_loaded(x, dy, z) == self.table.air {
            self.add_vine_direct(level, x, dy, z, meta);
            dy -= 1;
            i -= 1;
        }
    }
}

impl ObjectGenerator for JungleTree {
    /// Java: `generate`(L46-202).
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        // Asymmetric range written as nextInt(max) + min.
        let i = rand.next_int_max(self.max_tree_height) + self.min_tree_height;
        let tree_with_vines = rand.next_int_max(TREE_WITH_VINES_CHANCE) == 0;
        let mut flag = true;

        if y >= level.min_height() && y + i + 1 < level.max_height() {
            // Whole-volume grow-through check.
            for j in y..=y + 1 + i {
                let k = if j == y {
                    0
                } else if j >= y + 1 + i - 2 {
                    2
                } else {
                    1
                };

                for l in x - k..=x + k {
                    if !flag {
                        break;
                    }
                    for i1 in z - k..=z + k {
                        if !flag {
                            break;
                        }
                        if j >= level.min_height() && j < level.max_height() {
                            if !self
                                .table
                                .can_grow_into(level.get_block_if_cached_or_loaded(l, j, i1))
                            {
                                flag = false;
                            }
                        } else {
                            flag = false;
                        }
                    }
                }
            }

            if !flag {
                return false;
            }

            // Ground is grass/dirt/farmland.
            let ground = level.get_block_if_cached_or_loaded(x, y - 1, z);
            if (ground == self.table.grass_block
                || ground == self.table.dirt
                || ground == self.table.farmland)
                && y < level.max_height() - i - 1
            {
                set_dirt_at(&self.table, level, x, y - 1, z);

                // Canopy in 4 top-down layers.
                for i3 in y - 3 + i..=y + i {
                    let i4 = i3 - (y + i);
                    let j1 = 1 - i4 / 2;

                    for k1 in x - j1..=x + j1 {
                        let l1 = k1 - x;
                        for i2 in z - j1..=z + j1 {
                            let j2 = i2 - z;

                            // Corner trim (`&&` binds tighter than `||`).
                            if l1.abs() != j1
                                || j2.abs() != j1
                                || (rand.next_int_max(2) != 0 && i4 != 0)
                            {
                                let block = level.get_block_if_cached_or_loaded(k1, i3, i2);
                                if block == self.table.air
                                    || self.table.is_leaves(block)
                                    || block == self.table.vine
                                {
                                    level.set_block_state_at(
                                        k1,
                                        i3,
                                        i2,
                                        0,
                                        self.table.leaves[WoodType::Jungle.index()],
                                    );
                                }
                            }
                        }
                    }
                }

                // Trunk plus vines.
                for j3 in 0..i {
                    let block = level.get_block_if_cached_or_loaded(x, y + j3, z);
                    if block == self.table.air
                        || self.table.is_leaves(block)
                        || block == self.table.vine
                    {
                        level.set_block_state_at(
                            x,
                            y + j3,
                            z,
                            0,
                            self.table.log_state(WoodType::Jungle, Axis::Y),
                        );
                        if j3 > 0 {
                            if tree_with_vines {
                                add_vines_around_log(&self.table, level, x, y + j3, z);
                            } else {
                                // Sparse four-way vines.
                                if rand.next_int_max(3) > 0
                                    && level.get_block_if_cached_or_loaded(x - 1, y + j3, z)
                                        == self.table.air
                                {
                                    self.add_vine_direct(level, x - 1, y + j3, z, 8);
                                }
                                if rand.next_int_max(3) > 0
                                    && level.get_block_if_cached_or_loaded(x + 1, y + j3, z)
                                        == self.table.air
                                {
                                    self.add_vine_direct(level, x + 1, y + j3, z, 2);
                                }
                                if rand.next_int_max(3) > 0
                                    && level.get_block_if_cached_or_loaded(x, y + j3, z - 1)
                                        == self.table.air
                                {
                                    self.add_vine_direct(level, x, y + j3, z - 1, 1);
                                }
                                if rand.next_int_max(3) > 0
                                    && level.get_block_if_cached_or_loaded(x, y + j3, z + 1)
                                        == self.table.air
                                {
                                    self.add_vine_direct(level, x, y + j3, z + 1, 4);
                                }
                            }
                        }
                    }
                }

                // Canopy-edge hanging vines.
                for k3 in y - 3 + i..=y + i {
                    let j4 = k3 - (y + i);
                    let k4 = 2 - j4 / 2;

                    for l4 in x - k4..=x + k4 {
                        for i5 in z - k4..=z + k4 {
                            if self
                                .table
                                .is_leaves(level.get_block_if_cached_or_loaded(l4, k3, i5))
                            {
                                if rand.next_int_max(4) == 0
                                    && level.get_block_if_cached_or_loaded(l4 - 1, k3, i5)
                                        == self.table.air
                                {
                                    self.add_hanging_vine(level, l4 - 1, k3, i5, 8);
                                }
                                if rand.next_int_max(4) == 0
                                    && level.get_block_if_cached_or_loaded(l4 + 1, k3, i5)
                                        == self.table.air
                                {
                                    self.add_hanging_vine(level, l4 + 1, k3, i5, 2);
                                }
                                if rand.next_int_max(4) == 0
                                    && level.get_block_if_cached_or_loaded(l4, k3, i5 - 1)
                                        == self.table.air
                                {
                                    self.add_hanging_vine(level, l4, k3, i5 - 1, 1);
                                }
                                if rand.next_int_max(4) == 0
                                    && level.get_block_if_cached_or_loaded(l4, k3, i5 + 1)
                                        == self.table.air
                                {
                                    self.add_hanging_vine(level, l4, k3, i5 + 1, 4);
                                }
                            }
                        }
                    }
                }

                // Cocoa beans (i > 5 at 1/5 chance).
                if rand.next_int_max(5) == 0 && i > 5 {
                    for l3 in 0..2 {
                        for enumfacing in HorizontalFace::ALL {
                            if rand.next_int_max(4 - l3) == 0 {
                                let opposite = enumfacing.opposite();
                                let (ox, oz) = opposite.offset();
                                self.place_cocoa(
                                    level,
                                    rand.next_int_max(2),
                                    x + ox,
                                    y + i - 5 + l3,
                                    z + oz,
                                    enumfacing,
                                );
                            }
                        }
                    }
                }

                true
            } else {
                false
            }
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// ObjectJungleBush
// ---------------------------------------------------------------------------

/// Java: `ObjectJungleBush.java`.
pub struct JungleBush {
    table: Arc<TreeBlockTable>,
}

impl JungleBush {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl ObjectGenerator for JungleBush {
    /// Java: `generate`(L22-43).
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        _rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        level.set_block_state_at(x, y, z, 0, self.table.log_state(WoodType::Jungle, Axis::Y));

        // Circular leaf blobs for y=-2..=1.
        for dy in -2..=1 {
            let radius = 2 - dy;
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if dx * dx + dz * dz <= radius * radius {
                        let existing = level.get_block_if_cached_or_loaded(x + dx, y + dy, z + dz);
                        // Java L34: isAir || canBeReplaced || instanceof BlockLeaves
                        if existing == self.table.air
                            || self.table.check_block(existing)
                            || self.table.is_leaves(existing)
                        {
                            level.set_block_state_at(
                                x + dx,
                                y + dy,
                                z + dz,
                                0,
                                self.table.leaves[WoodType::Jungle.index()],
                            );
                        }
                    }
                }
            }
        }

        true
    }
}

// ---------------------------------------------------------------------------
// ObjectBigSpruceTree
// ---------------------------------------------------------------------------

/// Java: `ObjectBigSpruceTree.foliages`(L18-23).
const BIG_SPRUCE_FOLIAGES: [&[i32]; 4] = [
    &[1, 0, 0, 1, 2, 1, 1, 2, 3, 2, 2, 3, 4, 3],
    &[1, 0, 1, 2, 1, 2, 1, 1, 2, 3, 2, 2, 3, 4, 3],
    &[1, 2, 3],
    &[1, 2, 1, 3, 2, 4, 3],
];

/// Big spruce (2x2 trunk).
pub struct BigSpruceTree {
    table: Arc<TreeBlockTable>,
}

impl BigSpruceTree {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }

    /// placeLogAt needs grow-through.
    fn place_log_at(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) {
        if self
            .table
            .can_grow_into(level.get_block_if_cached_or_loaded(x, y, z))
        {
            level.set_block_state_at(x, y, z, 0, self.table.log_state(WoodType::Spruce, Axis::Y));
        }
    }

    /// placeLeafAt: leaves only on air/snow_layer.
    fn place_leaf_at(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) {
        let block = level.get_block_if_cached_or_loaded(x, y, z);
        if block == self.table.air || block == self.table.snow_layer {
            level.set_block_state_at(x, y, z, 0, self.table.leaves[WoodType::Spruce.index()]);
        }
    }

    /// placePodzolAt turns dirt tags to podzol.
    fn place_podzol_at(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) {
        if self
            .table
            .is_support_dirt(level.get_block_if_cached_or_loaded(x, y, z))
        {
            level.set_block_state_at(x, y, z, 0, self.table.podzol);
        }
    }
}

impl ObjectGenerator for BigSpruceTree {
    /// generate keeps the hardcoded 256.
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let height = 24 + rand.next_int_max(8);
        let base_x = x;
        let base_y = y;
        let base_z = z;
        let mid_x = base_x + 1;
        let mid_z = base_z + 1;

        if base_y < 1 || base_y + height + 5 >= 256 {
            return false;
        }

        // Top podzol disc (radius 6).
        let rad = 6;
        for dx in -rad - 1..=rad {
            for dz in -rad - 1..=rad {
                let calc_x = dx as f32 + 0.5;
                let calc_z = dz as f32 + 0.5;
                let calc_rad = rad as f32 + 0.8;
                let px = mid_x + dx;
                let pz = mid_z + dz;
                if calc_x * calc_x + calc_z * calc_z < calc_rad * calc_rad {
                    // Cross-chunk heightmap reads skip unloaded chunks
                    // (single-chunk approximation).
                    if let Some(hm) = level.height_map(px, pz) {
                        self.place_podzol_at(level, px, hm, pz);
                    }
                }
            }
        }

        // Ground is grass/dirt/podzol.
        let ground = level.get_block_if_cached_or_loaded(base_x, base_y - 1, base_z);
        if ground != self.table.grass_block
            && ground != self.table.dirt
            && ground != self.table.podzol
        {
            return false;
        }

        // Quirk kept: the foliage index never hits the last set.
        // (The last foliage set is never picked.)
        let leaf_radii =
            BIG_SPRUCE_FOLIAGES[rand.next_int_max(BIG_SPRUCE_FOLIAGES.len() as i32 - 1) as usize];

        // Treetop 2x2 leaf cap.
        self.place_leaf_at(level, base_x, base_y + height + 1, base_z);
        self.place_leaf_at(level, base_x + 1, base_y + height + 1, base_z);
        self.place_leaf_at(level, base_x, base_y + height + 1, base_z + 1);
        self.place_leaf_at(level, base_x + 1, base_y + height + 1, base_z + 1);

        // 2x2 trunk with per-layer leaf rings.
        for y_off in (0..=height).rev() {
            self.place_log_at(level, base_x, base_y + y_off, base_z);
            self.place_log_at(level, base_x + 1, base_y + y_off, base_z);
            self.place_log_at(level, base_x, base_y + y_off, base_z + 1);
            self.place_log_at(level, base_x + 1, base_y + y_off, base_z + 1);
            let index = height - y_off;
            if (index as usize) < leaf_radii.len() {
                let radius = leaf_radii[index as usize];
                for dx in -radius - 1..=radius {
                    for dz in -radius - 1..=radius {
                        let calc_x = dx as f32 + 0.5;
                        let calc_z = dz as f32 + 0.5;
                        let calc_rad = radius as f32 + 0.7;
                        if calc_x * calc_x + calc_z * calc_z < calc_rad * calc_rad {
                            self.place_leaf_at(level, mid_x + dx, base_y + y_off, mid_z + dz);
                        }
                    }
                }
            }
        }

        true
    }
}

// ---------------------------------------------------------------------------
// ObjectFancyOakTree
// ---------------------------------------------------------------------------

/// Fancy oak constants.
const FANCY_OAK_TRUNK_SCALE: f64 = 0.618;
const FANCY_OAK_CLUSTER_DENSITY: f64 = 1.382;
const FANCY_OAK_BRANCH_SLOPE: f64 = 0.381;
const FANCY_OAK_BRANCH_LENGTH: f64 = 0.328;
const FANCY_OAK_FOLIAGE_HEIGHT: i32 = 4;
const FANCY_OAK_FOLIAGE_RADIUS: i32 = 2;
const FANCY_OAK_FOLIAGE_OFFSET: i32 = 4;

/// Java: `ObjectFancyOakTree.FoliageCoords`(L235-243).
#[derive(Clone, Copy)]
struct FoliageCoords {
    x: i32,
    y: i32,
    z: i32,
    branch_base: i32,
}

/// Java: `ObjectFancyOakTree.java`.
pub struct FancyOakTree {
    base_height: i32,
    height_rand_a: i32,
    height_rand_b: i32,
    table: Arc<TreeBlockTable>,
}

impl FancyOakTree {
    /// Java: `ObjectFancyOakTree()`(L38-40)——(3, 11, 0).
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self::with_params(table, 3, 11, 0)
    }

    /// Java: `ObjectFancyOakTree(int, int, int)`(L42-46).
    pub fn with_params(
        table: Arc<TreeBlockTable>,
        base_height: i32,
        height_rand_a: i32,
        height_rand_b: i32,
    ) -> Self {
        Self {
            base_height,
            height_rand_a,
            height_rand_b,
            table,
        }
    }

    /// makeLimb links start to end (checking or placing).
    fn make_limb(
        &self,
        level: &mut BlockManager<'_>,
        sx: i32,
        sy: i32,
        sz: i32,
        ex: i32,
        ey: i32,
        ez: i32,
        do_place: bool,
    ) -> bool {
        if !do_place && (sx, sy, sz) == (ex, ey, ez) {
            return true;
        }

        let (dx, dy, dz) = (ex - sx, ey - sy, ez - sz);
        let steps = Self::get_steps(dx, dy, dz);
        if steps == 0 {
            return true;
        }

        let fx = dx as f32 / steps as f32;
        let fy = dy as f32 / steps as f32;
        let fz = dz as f32 / steps as f32;

        for i in 0..=steps {
            let bx = sx + math_floor(0.5 + i as f32 * fx);
            let by = sy + math_floor(0.5 + i as f32 * fy);
            let bz = sz + math_floor(0.5 + i as f32 * fz);

            if do_place {
                let axis = Self::get_log_axis(sx, sz, bx, bz);
                self.place_log_at(level, bx, by, bz, axis);
            } else if !self
                .table
                .can_grow_into(level.get_block_if_cached_or_loaded(bx, by, bz))
            {
                return false;
            }
        }

        true
    }

    /// Java: `getSteps`(L142-147).
    fn get_steps(dx: i32, dy: i32, dz: i32) -> i32 {
        dx.abs().max(dy.abs().max(dz.abs()))
    }

    /// Java: `getLogAxis`(L149-159).
    fn get_log_axis(sx: i32, sz: i32, bx: i32, bz: i32) -> Axis {
        let xdiff = (bx - sx).abs();
        let zdiff = (bz - sz).abs();
        let maxdiff = xdiff.max(zdiff);
        if maxdiff > 0 {
            if xdiff == maxdiff {
                Axis::X
            } else {
                Axis::Z
            }
        } else {
            Axis::Y
        }
    }

    /// Java: `trimBranches`(L161-163).
    fn trim_branches(height: i32, local_y: i32) -> bool {
        (local_y as f64) >= height as f64 * 0.2
    }

    /// Java: `makeBranches`(L165-173).
    fn make_branches(
        &self,
        level: &mut BlockManager<'_>,
        height: i32,
        origin_x: i32,
        origin_y: i32,
        origin_z: i32,
        foliage_coords: &[FoliageCoords],
    ) {
        for end_coord in foliage_coords {
            let base_coord = (origin_x, end_coord.branch_base, origin_z);
            if base_coord != (end_coord.x, end_coord.y, end_coord.z)
                && Self::trim_branches(height, end_coord.branch_base - origin_y)
            {
                self.make_limb(
                    level,
                    origin_x,
                    end_coord.branch_base,
                    origin_z,
                    end_coord.x,
                    end_coord.y,
                    end_coord.z,
                    true,
                );
            }
        }
    }

    /// Java: `createFoliage`(L175-180).
    fn create_foliage(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) {
        for yo in
            (FANCY_OAK_FOLIAGE_OFFSET - FANCY_OAK_FOLIAGE_HEIGHT..=FANCY_OAK_FOLIAGE_OFFSET).rev()
        {
            let current_radius = FANCY_OAK_FOLIAGE_RADIUS
                + if yo != FANCY_OAK_FOLIAGE_OFFSET
                    && yo != FANCY_OAK_FOLIAGE_OFFSET - FANCY_OAK_FOLIAGE_HEIGHT
                {
                    1
                } else {
                    0
                };
            self.place_leaves_row(level, x, y, z, current_radius, yo);
        }
    }

    /// Java: `placeLeavesRow`(L182-191).
    fn place_leaves_row(
        &self,
        level: &mut BlockManager<'_>,
        cx: i32,
        cy: i32,
        cz: i32,
        radius: i32,
        y_offset: i32,
    ) {
        let y = cy + y_offset;
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if !Self::should_skip_location(dx, dz, radius) {
                    self.place_leaf_at(level, cx + dx, y, cz + dz);
                }
            }
        }
    }

    /// Java: `shouldSkipLocation`(L193-195).
    fn should_skip_location(dx: i32, dz: i32, current_radius: i32) -> bool {
        let dx = dx as f32 + 0.5;
        let dz = dz as f32 + 0.5;
        dx * dx + dz * dz > (current_radius * current_radius) as f32
    }

    /// Java: `treeShape`(L197-212).
    fn tree_shape(height: i32, y: i32) -> f32 {
        if (y as f32) < height as f32 * 0.3 {
            return -1.0;
        }

        let radius = height as f32 / 2.0;
        let adjacent = radius - y as f32;
        let mut distance = (radius * radius - adjacent * adjacent).sqrt();
        if adjacent == 0.0 {
            distance = radius;
        } else if adjacent.abs() >= radius {
            return 0.0;
        }

        distance * 0.5
    }

    /// placeLogAt needs a free cell (axis included).
    fn place_log_at(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32, axis: Axis) {
        if self
            .table
            .can_grow_into(level.get_block_if_cached_or_loaded(x, y, z))
        {
            level.set_block_state_at(x, y, z, 0, self.table.log_state(WoodType::Oak, axis));
        }
    }

    /// placeLeafAt: leaves only on air/leaves.
    fn place_leaf_at(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) {
        let block = level.get_block_if_cached_or_loaded(x, y, z);
        if block == self.table.air || self.table.is_leaves(block) {
            level.set_block_state_at(x, y, z, 0, self.table.leaves[WoodType::Oak.index()]);
        }
    }
}

/// Truncating floor helper.
fn math_floor(v: f32) -> i32 {
    v.floor() as i32
}

impl ObjectGenerator for FancyOakTree {
    /// Java: `generate`(L49-108).
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let tree_height = self.base_height
            + rand.next_int_max(self.height_rand_a + 1)
            + rand.next_int_max(self.height_rand_b + 1);
        let height = tree_height + 2;

        if y < level.min_height() + 1 || y + height + 1 >= level.max_height() {
            return false;
        }

        let ground = level.get_block_if_cached_or_loaded(x, y - 1, z);
        if ground != self.table.grass_block && ground != self.table.dirt {
            return false;
        }

        let trunk_height = (height as f64 * FANCY_OAK_TRUNK_SCALE).floor() as i32;
        // min(1, floor(...)) never exceeds 1.
        let clusters_per_y =
            1.min((FANCY_OAK_CLUSTER_DENSITY + (height as f64 / 13.0).powi(2)).floor() as i32);
        let trunk_top = y + trunk_height;
        let mut relative_y = height - 5;
        let mut foliage_coords: Vec<FoliageCoords> = Vec::new();
        foliage_coords.push(FoliageCoords {
            x,
            y: y + relative_y,
            z,
            branch_base: trunk_top,
        });

        while relative_y >= 0 {
            let tree_shape = Self::tree_shape(height, relative_y);
            if tree_shape < 0.0 {
                relative_y -= 1;
                continue;
            }

            for _ in 0..clusters_per_y {
                let radius =
                    tree_shape as f64 * (rand.next_float() as f64 + FANCY_OAK_BRANCH_LENGTH);
                let angle = rand.next_float() as f64 * 2.0 * std::f64::consts::PI;
                let bx = radius * angle.sin() + 0.5;
                let bz = radius * angle.cos() + 0.5;
                let check_start = (
                    x + math_floor(bx as f32),
                    y + relative_y - 1,
                    z + math_floor(bz as f32),
                );
                let check_end = (check_start.0, check_start.1 + 5, check_start.2);

                if self.make_limb(
                    level,
                    check_start.0,
                    check_start.1,
                    check_start.2,
                    check_end.0,
                    check_end.1,
                    check_end.2,
                    false,
                ) {
                    let dx = x - check_start.0;
                    let dz = z - check_start.2;
                    let branch_height = check_start.1 as f64
                        - ((dx * dx + dz * dz) as f64).sqrt() * FANCY_OAK_BRANCH_SLOPE;
                    let branch_top = if branch_height > trunk_top as f64 {
                        trunk_top
                    } else {
                        branch_height as i32
                    };
                    if self.make_limb(
                        level,
                        x,
                        branch_top,
                        z,
                        check_start.0,
                        check_start.1,
                        check_start.2,
                        false,
                    ) {
                        foliage_coords.push(FoliageCoords {
                            x: check_start.0,
                            y: check_start.1,
                            z: check_start.2,
                            branch_base: branch_top,
                        });
                    }
                }
            }
            relative_y -= 1;
        }

        set_dirt_at(&self.table, level, x, y - 1, z);
        self.make_limb(level, x, y, z, x, y + trunk_height, z, true);
        self.make_branches(level, height, x, y, z, &foliage_coords);

        for foliage_coord in &foliage_coords {
            if Self::trim_branches(height, foliage_coord.branch_base - y) {
                self.create_foliage(level, foliage_coord.x, foliage_coord.y, foliage_coord.z);
            }
        }

        true
    }
}

// ---------------------------------------------------------------------------
// ObjectDarkOakTree
// ---------------------------------------------------------------------------

/// Dark oak (2x2 trunk).
pub struct DarkOakTree {
    table: Arc<TreeBlockTable>,
}

impl DarkOakTree {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }

    /// placeTreeOfHeight checks the whole volume for grow-through.
    fn place_tree_of_height(
        &self,
        level: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        height: i32,
    ) -> bool {
        for l in 0..=height + 1 {
            let i1 = if l == 0 {
                0
            } else if l >= height - 1 {
                2
            } else {
                1
            };

            for j1 in -i1..=i1 {
                for k1 in -i1..=i1 {
                    if !self
                        .table
                        .can_grow_into(level.get_block_if_cached_or_loaded(x + j1, y + l, z + k1))
                    {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// placeLogAt needs grow-through (vines allowed).
    fn place_log_at(
        &self,
        level: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        tree_with_vines: bool,
    ) {
        if self
            .table
            .can_grow_into(level.get_block_if_cached_or_loaded(x, y, z))
        {
            level.set_block_state_at(x, y, z, 0, self.table.log_state(WoodType::DarkOak, Axis::Y));
            if tree_with_vines {
                add_vines_around_log(&self.table, level, x, y, z);
            }
        }
    }

    /// placeLeafAt: leaves only on air/vine.
    fn place_leaf_at(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) {
        let block = level.get_block_if_cached_or_loaded(x, y, z);
        if block == self.table.air || block == self.table.vine {
            level.set_block_state_at(x, y, z, 0, self.table.leaves[WoodType::DarkOak.index()]);
        }
    }
}

impl ObjectGenerator for DarkOakTree {
    /// generate keeps the hardcoded 256.
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let i = rand.next_int_max(3) + rand.next_int_max(2) + 6;
        let tree_with_vines = rand.next_int_max(TREE_WITH_VINES_CHANCE) == 0;
        let j = x;
        let k = y;
        let l = z;

        if k >= 1 && k + i + 1 < 256 {
            let ground = level.get_block_if_cached_or_loaded(j, k - 1, l);

            if !self.table.can_grow_into(ground) {
                return false;
            }
            if !self.place_tree_of_height(level, j, k, l, i) {
                return false;
            }

            set_dirt_at(&self.table, level, j, k - 1, l);
            set_dirt_at(&self.table, level, j + 1, k - 1, l);
            set_dirt_at(&self.table, level, j, k - 1, l + 1);
            set_dirt_at(&self.table, level, j + 1, k - 1, l + 1);

            let enumfacing = HorizontalFace::random(rand);
            let (fx, fz) = enumfacing.offset();
            let i1 = i - rand.next_int_max(4);
            let mut j1 = 2 - rand.next_int_max(3);
            let mut k1 = j;
            let mut l1 = l;
            let i2 = k + i - 1;

            // 2x2 trunk (bent top).
            for j2 in 0..i {
                if j2 >= i1 && j1 > 0 {
                    k1 += fx;
                    l1 += fz;
                    j1 -= 1;
                }

                let k2 = k + j2;
                let material = level.get_block_if_cached_or_loaded(k1, k2, l1);
                if self.table.can_grow_into(material) || self.table.is_leaves(material) {
                    self.place_log_at(level, k1, k2, l1, tree_with_vines);
                    self.place_log_at(level, k1 + 1, k2, l1, tree_with_vines);
                    self.place_log_at(level, k1, k2, l1 + 1, tree_with_vines);
                    self.place_log_at(level, k1 + 1, k2, l1 + 1, tree_with_vines);
                }
            }

            // Top double-layer de-cornered 2x2 leaf cap.
            for i3 in -2..=0 {
                for l3 in -2..=0 {
                    let mut k4 = -1;
                    self.place_leaf_at(level, k1 + i3, i2 + k4, l1 + l3);
                    self.place_leaf_at(level, 1 + k1 - i3, i2 + k4, l1 + l3);
                    self.place_leaf_at(level, k1 + i3, i2 + k4, 1 + l1 - l3);
                    self.place_leaf_at(level, 1 + k1 - i3, i2 + k4, 1 + l1 - l3);

                    if (i3 > -2 || l3 > -1) && (i3 != -1 || l3 != -2) {
                        k4 = 1;
                        self.place_leaf_at(level, k1 + i3, i2 + k4, l1 + l3);
                        self.place_leaf_at(level, 1 + k1 - i3, i2 + k4, l1 + l3);
                        self.place_leaf_at(level, k1 + i3, i2 + k4, 1 + l1 - l3);
                        self.place_leaf_at(level, 1 + k1 - i3, i2 + k4, 1 + l1 - l3);
                    }
                }
            }

            // Random top hat.
            if rand.next_boolean() {
                self.place_leaf_at(level, k1, i2 + 2, l1);
                self.place_leaf_at(level, k1 + 1, i2 + 2, l1);
                self.place_leaf_at(level, k1 + 1, i2 + 2, l1 + 1);
                self.place_leaf_at(level, k1, i2 + 2, l1 + 1);
            }

            // 8x8 de-cornered canopy rim.
            for j3 in -3i32..=4 {
                for i4 in -3i32..=4 {
                    if (j3 != -3 || i4 != -3)
                        && (j3 != -3 || i4 != 4)
                        && (j3 != 4 || i4 != -3)
                        && (j3 != 4 || i4 != 4)
                        && (j3.abs() < 3 || i4.abs() < 3)
                    {
                        self.place_leaf_at(level, k1 + j3, i2, l1 + i4);
                    }
                }
            }

            // Bottom branches (with hanging trunks).
            for k3 in -1..=2 {
                for j4 in -1..=2 {
                    if (k3 < 0 || k3 > 1 || j4 < 0 || j4 > 1) && rand.next_int_max(3) <= 0 {
                        let l4 = rand.next_int_max(3) + 2;

                        for i5 in 0..l4 {
                            self.place_log_at(level, j + k3, i2 - i5 - 1, l + j4, false);
                        }

                        for j5 in -1..=1 {
                            for l2 in -1..=1 {
                                self.place_leaf_at(level, k1 + k3 + j5, i2, l1 + j4 + l2);
                            }
                        }
                        for k5 in -2i32..=2 {
                            for l5 in -2i32..=2 {
                                if k5.abs() != 2 || l5.abs() != 2 {
                                    self.place_leaf_at(level, k1 + k3 + k5, i2 - 1, l1 + j4 + l5);
                                }
                            }
                        }
                    }
                }
            }

            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// ObjectCherryTree
// ---------------------------------------------------------------------------

/// Java: `ObjectCherryTree.LEAVES_RADIUS`(L234).
const CHERRY_LEAVES_RADIUS: i32 = 4;

/// Java: `ObjectCherryTree.java`.
pub struct CherryTree {
    table: Arc<TreeBlockTable>,
}

impl CherryTree {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }

    /// Java: `generateBigTree`(L50-164).
    fn generate_big_tree(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let main_trunk_height = rand.next_int_max(2) + 10;

        if !self.can_place_object(level, main_trunk_height, x, y, z) {
            return false;
        }

        let mut grow_on_x_axis = rand.next_boolean();
        let mut x_multiplier = i32::from(grow_on_x_axis);
        let mut z_multiplier = i32::from(!grow_on_x_axis);

        let left_side_trunk_length = rand.next_int_range(2, 4);
        let left_side_trunk_height = rand.next_int_range(3, 5);
        let left_side_trunk_start_y = rand.next_int_range(4, 5);

        if !self.can_place_object(
            level,
            left_side_trunk_height,
            x - left_side_trunk_length * x_multiplier,
            y + left_side_trunk_start_y,
            z - left_side_trunk_length * z_multiplier,
        ) {
            grow_on_x_axis = !grow_on_x_axis;
            x_multiplier = i32::from(grow_on_x_axis);
            z_multiplier = i32::from(!grow_on_x_axis);
            if !self.can_place_object(
                level,
                left_side_trunk_height,
                x - left_side_trunk_length * x_multiplier,
                y + left_side_trunk_start_y,
                z - left_side_trunk_length * z_multiplier,
            ) {
                return false;
            }
        }

        let right_side_trunk_length = rand.next_int_range(2, 4);
        let right_side_trunk_height = rand.next_int_range(3, 5);
        let right_side_trunk_start_y = rand.next_int_range(4, 5);

        if !self.can_place_object(
            level,
            right_side_trunk_height,
            x + right_side_trunk_length * x_multiplier,
            y + right_side_trunk_start_y,
            z + right_side_trunk_length * z_multiplier,
        ) {
            return false;
        }

        set_dirt_at(&self.table, level, x, y - 1, z);

        // Main trunk.
        for yy in 0..main_trunk_height {
            level.set_block_state_at(
                x,
                y + yy,
                z,
                0,
                self.table.log_state(WoodType::Cherry, Axis::Y),
            );
        }
        let side_state = if grow_on_x_axis {
            self.table.log_state(WoodType::Cherry, Axis::X)
        } else {
            self.table.log_state(WoodType::Cherry, Axis::Z)
        };

        // Left branch (horizontal run).
        for xx in 1..=left_side_trunk_length {
            if self
                .table
                .can_grow_into(level.get_block_if_cached_or_loaded(
                    x - xx * x_multiplier,
                    y + left_side_trunk_start_y,
                    z - xx * z_multiplier,
                ))
            {
                level.set_block_state_at(
                    x - xx * x_multiplier,
                    y + left_side_trunk_start_y,
                    z - xx * z_multiplier,
                    0,
                    side_state,
                );
            }
        }
        // Left branch (vertical run).
        for yy in 1..left_side_trunk_height {
            if self
                .table
                .can_grow_into(level.get_block_if_cached_or_loaded(
                    x - left_side_trunk_length * x_multiplier,
                    y + left_side_trunk_start_y + yy,
                    z - left_side_trunk_length * z_multiplier,
                ))
            {
                level.set_block_state_at(
                    x - left_side_trunk_length * x_multiplier,
                    y + left_side_trunk_start_y + yy,
                    z - left_side_trunk_length * z_multiplier,
                    0,
                    self.table.log_state(WoodType::Cherry, Axis::Y),
                );
            }
        }
        // Left-branch corner fix (when startY == 4).
        if left_side_trunk_start_y == 4 {
            let mut tmp_x = x - left_side_trunk_length * x_multiplier;
            let mut tmp_y = y + left_side_trunk_start_y;
            let mut tmp_z = z - left_side_trunk_length * z_multiplier;
            level.set_block_state_at(tmp_x, tmp_y, tmp_z, 0, self.table.air);
            tmp_x += x_multiplier;
            tmp_y += 1;
            tmp_z += z_multiplier;
            if self
                .table
                .can_grow_into(level.get_block_if_cached_or_loaded(tmp_x, tmp_y, tmp_z))
            {
                level.set_block_state_at(
                    tmp_x,
                    tmp_y,
                    tmp_z,
                    0,
                    self.table.log_state(WoodType::Cherry, Axis::Y),
                );
            }
            tmp_x -= x_multiplier;
            tmp_z -= z_multiplier;
            if self
                .table
                .can_grow_into(level.get_block_if_cached_or_loaded(tmp_x, tmp_y, tmp_z))
            {
                level.set_block_state_at(tmp_x, tmp_y, tmp_z, 0, side_state);
            }
        }
        // Right branch (horizontal run).
        for xx in 1..=right_side_trunk_length {
            if self
                .table
                .can_grow_into(level.get_block_if_cached_or_loaded(
                    x + xx * x_multiplier,
                    y + right_side_trunk_start_y,
                    z + xx * z_multiplier,
                ))
            {
                level.set_block_state_at(
                    x + xx * x_multiplier,
                    y + right_side_trunk_start_y,
                    z + xx * z_multiplier,
                    0,
                    side_state,
                );
            }
        }
        // Right branch (vertical run).
        for yy in 1..right_side_trunk_height {
            if self
                .table
                .can_grow_into(level.get_block_if_cached_or_loaded(
                    x + right_side_trunk_length * x_multiplier,
                    y + right_side_trunk_start_y + yy,
                    z + right_side_trunk_length * z_multiplier,
                ))
            {
                level.set_block_state_at(
                    x + right_side_trunk_length * x_multiplier,
                    y + right_side_trunk_start_y + yy,
                    z + right_side_trunk_length * z_multiplier,
                    0,
                    self.table.log_state(WoodType::Cherry, Axis::Y),
                );
            }
        }
        // Right-branch corner fix.
        if right_side_trunk_start_y == 4 {
            let mut tmp_x = x + right_side_trunk_length * x_multiplier;
            let mut tmp_y = y + right_side_trunk_start_y;
            let mut tmp_z = z + right_side_trunk_length * z_multiplier;
            level.set_block_state_at(tmp_x, tmp_y, tmp_z, 0, self.table.air);
            tmp_x -= x_multiplier;
            tmp_y += 1;
            tmp_z -= z_multiplier;
            if self
                .table
                .can_grow_into(level.get_block_if_cached_or_loaded(tmp_x, tmp_y, tmp_z))
            {
                level.set_block_state_at(
                    tmp_x,
                    tmp_y,
                    tmp_z,
                    0,
                    self.table.log_state(WoodType::Cherry, Axis::Y),
                );
            }
            tmp_x += x_multiplier;
            tmp_z += z_multiplier;
            if self
                .table
                .can_grow_into(level.get_block_if_cached_or_loaded(tmp_x, tmp_y, tmp_z))
            {
                level.set_block_state_at(tmp_x, tmp_y, tmp_z, 0, side_state);
            }
        }

        // Three leaf blobs.
        self.generate_leaves(level, rand, x, y + main_trunk_height + 1, z);
        self.generate_leaves(
            level,
            rand,
            x - left_side_trunk_length * x_multiplier,
            y + left_side_trunk_start_y + left_side_trunk_height + 1,
            z - left_side_trunk_length * z_multiplier,
        );
        self.generate_leaves(
            level,
            rand,
            x + right_side_trunk_length * x_multiplier,
            y + right_side_trunk_start_y + right_side_trunk_height + 1,
            z + right_side_trunk_length * z_multiplier,
        );
        true
    }

    /// Java: `generateSmallTree`(L166-232).
    fn generate_small_tree(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let main_trunk_height = rand.next_int_max(2) + 4;
        let side_trunk_height = rand.next_int_range(3, 5);

        if !self.can_place_object(level, main_trunk_height + 1, x, y, z) {
            return false;
        }

        let mut grow_direction = rand.next_int_range(0, 3);
        let mut x_multiplier = 0;
        let mut z_multiplier = 0;
        let mut can_place = false;
        for _ in 0..4 {
            grow_direction = (grow_direction + 1) % 4;
            x_multiplier = match grow_direction {
                0 => -1,
                1 => 1,
                _ => 0,
            };
            z_multiplier = match grow_direction {
                2 => -1,
                3 => 1,
                _ => 0,
            };
            if self.can_place_object(
                level,
                side_trunk_height,
                x + x_multiplier * side_trunk_height,
                y,
                z + z_multiplier * side_trunk_height,
            ) {
                can_place = true;
                break;
            }
        }
        if !can_place {
            return false;
        }

        let side_state = if x_multiplier == 0 {
            self.table.log_state(WoodType::Cherry, Axis::Z)
        } else {
            self.table.log_state(WoodType::Cherry, Axis::X)
        };
        // Main trunk.
        for yy in 0..main_trunk_height {
            if self
                .table
                .can_grow_into(level.get_block_if_cached_or_loaded(x, y + yy, z))
            {
                level.set_block_state_at(
                    x,
                    y + yy,
                    z,
                    0,
                    self.table.log_state(WoodType::Cherry, Axis::Y),
                );
            }
        }
        // Side trunk (stepped).
        for yy in 1..=side_trunk_height {
            let tmp_x = x + yy * x_multiplier;
            let mut tmp_y = y + main_trunk_height + yy - 2;
            let tmp_z = z + yy * z_multiplier;
            if self
                .table
                .can_grow_into(level.get_block_if_cached_or_loaded(tmp_x, tmp_y, tmp_z))
            {
                level.set_block_state_at(tmp_x, tmp_y, tmp_z, 0, side_state);
            }
            // Side trunks 4+ tall skip their last segment.
            if yy == side_trunk_height - 1 && side_trunk_height > 3 {
                continue;
            }
            tmp_y += 1;
            if self
                .table
                .can_grow_into(level.get_block_if_cached_or_loaded(tmp_x, tmp_y, tmp_z))
            {
                level.set_block_state_at(
                    tmp_x,
                    tmp_y,
                    tmp_z,
                    0,
                    self.table.log_state(WoodType::Cherry, Axis::Y),
                );
            }
        }

        self.generate_leaves(
            level,
            rand,
            x + side_trunk_height * x_multiplier,
            y + main_trunk_height + side_trunk_height,
            z + side_trunk_height * z_multiplier,
        );
        true
    }

    /// generateLeaves: radius-4 leaf ball with random bottom fringe.
    fn generate_leaves(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) {
        for dy in -2i32..=2 {
            for dx in -CHERRY_LEAVES_RADIUS..=CHERRY_LEAVES_RADIUS {
                for dz in -CHERRY_LEAVES_RADIUS..=CHERRY_LEAVES_RADIUS {
                    let current_radius = CHERRY_LEAVES_RADIUS - dy.abs().max(1);
                    if dx * dx + dz * dz > current_radius * current_radius {
                        continue;
                    }
                    let block = level.get_block_if_cached_or_loaded(x + dx, y + dy, z + dz);
                    if block == self.table.air
                        || self.table.is_leaves(block)
                        || block == self.table.azalea_leaves_flowered
                    {
                        level.set_block_state_at(
                            x + dx,
                            y + dy,
                            z + dz,
                            0,
                            self.table.leaves[WoodType::Cherry.index()],
                        );
                    }
                    if dy == -2 && rand.next_int_range(0, 2) == 0 {
                        let block = level.get_block_if_cached_or_loaded(x + dx, y + dy - 1, z + dz);
                        if block == self.table.air
                            || self.table.is_leaves(block)
                            || block == self.table.azalea_leaves_flowered
                        {
                            level.set_block_state_at(
                                x + dx,
                                y + dy - 1,
                                z + dz,
                                0,
                                self.table.leaves[WoodType::Cherry.index()],
                            );
                        }
                    }
                }
            }
        }
    }

    /// canPlaceObject checks the conical volume for grow-through.
    fn can_place_object(
        &self,
        level: &mut BlockManager<'_>,
        tree_height: i32,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let mut radius_to_check = 0;
        for yy in 0..tree_height + 3 {
            if yy == 1 || yy == tree_height {
                radius_to_check += 1;
            }
            for xx in -radius_to_check..radius_to_check + 1 {
                for zz in -radius_to_check..radius_to_check + 1 {
                    if !self
                        .table
                        .can_grow_into(level.get_block_if_cached_or_loaded(x + xx, y + yy, z + zz))
                    {
                        return false;
                    }
                }
            }
        }
        true
    }
}

impl ObjectGenerator for CherryTree {
    /// generate picks big or small (falling back to small on failure).
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let is_big_tree = rand.next_boolean();
        if is_big_tree && self.generate_big_tree(level, rand, x, y, z) {
            return true;
        }
        self.generate_small_tree(level, rand, x, y, z)
    }
}

// ---------------------------------------------------------------------------
// ObjectAzaleaTree
// ---------------------------------------------------------------------------

/// Java: `ObjectAzaleaTree.java`.
pub struct AzaleaTree {
    table: Arc<TreeBlockTable>,
}

impl AzaleaTree {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }

    /// placeLogAt: logs only on air/azalea leaves.
    fn place_log_at(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) {
        let block = level.get_block_if_cached_or_loaded(x, y, z);
        if block == self.table.air
            || block == self.table.azalea_leaves
            || block == self.table.azalea_leaves_flowered
        {
            level.set_block_state_at(x, y, z, 0, self.table.log_state(WoodType::Oak, Axis::Y));
        }
    }

    /// placeLeafAt: leaves only on air (1/3 flowering).
    fn place_leaf_at(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) {
        let block = level.get_block_if_cached_or_loaded(x, y, z);
        if block == self.table.air {
            let state = if rand.next_int_max(3) == 1 {
                self.table.azalea_leaves_flowered
            } else {
                self.table.azalea_leaves
            };
            level.set_block_state_at(x, y, z, 0, state);
        }
    }
}

impl ObjectGenerator for AzaleaTree {
    /// generate keeps the hardcoded -63/320 bounds.
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let i = rand.next_int_max(2) + 2;
        let j = x;
        let k = y;
        let l = z;
        let i2 = k + i;

        if k >= -63 && k + i + 2 < 320 {
            // Trunk.
            for il in 0..i + 1 {
                self.place_log_at(level, j, il + k, l);
            }
            // setDirtAt overridden to dirt_with_roots.
            level.set_block_state_at(j, k - 1, l, 0, self.table.dirt_with_roots);

            // Three leaf blobs (random offsets).
            for i3 in -2..=1 {
                for l3 in -2..=1 {
                    let mut k4 = 1;
                    let offset_x = rand.next_int_range(0, 1);
                    let offset_y = rand.next_int_range(0, 1);
                    let offset_z = rand.next_int_range(0, 1);
                    self.place_leaf_at(
                        level,
                        rand,
                        j + i3 + offset_x,
                        i2 + k4 + offset_y,
                        l + l3 + offset_z,
                    );
                    self.place_leaf_at(
                        level,
                        rand,
                        j - i3 + offset_x,
                        i2 + k4 + offset_y,
                        l + l3 + offset_z,
                    );
                    self.place_leaf_at(
                        level,
                        rand,
                        j + i3 + offset_x,
                        i2 + k4 + offset_y,
                        l - l3 + offset_z,
                    );
                    self.place_leaf_at(
                        level,
                        rand,
                        j - i3 + offset_x,
                        i2 + k4 + offset_y,
                        l - l3 + offset_z,
                    );

                    k4 = 0;
                    self.place_leaf_at(level, rand, j + i3, i2 + k4, l + l3);
                    self.place_leaf_at(level, rand, j - i3, i2 + k4, l + l3);
                    self.place_leaf_at(level, rand, j + i3, i2 + k4, l - l3);
                    self.place_leaf_at(level, rand, j - i3, i2 + k4, l - l3);

                    k4 = 1;
                    self.place_leaf_at(level, rand, j + i3, i2 + k4, l + l3);
                    self.place_leaf_at(level, rand, j - i3, i2 + k4, l + l3);
                    self.place_leaf_at(level, rand, j + i3, i2 + k4, l - l3);
                    self.place_leaf_at(level, rand, j - i3, i2 + k4, l - l3);

                    k4 = 2;
                    let offset_x = rand.next_int_range(-1, 0);
                    let offset_y = rand.next_int_range(-1, 0);
                    let offset_z = rand.next_int_range(-1, 0);

                    self.place_leaf_at(
                        level,
                        rand,
                        j + i3 + offset_x,
                        i2 + k4 + offset_y,
                        l + l3 + offset_z,
                    );
                    self.place_leaf_at(
                        level,
                        rand,
                        j - i3 + offset_x,
                        i2 + k4 + offset_y,
                        l + l3 + offset_z,
                    );
                    self.place_leaf_at(
                        level,
                        rand,
                        j + i3 + offset_x,
                        i2 + k4 + offset_y,
                        l - l3 + offset_z,
                    );
                    self.place_leaf_at(
                        level,
                        rand,
                        j - i3 + offset_x,
                        i2 + k4 + offset_y,
                        l - l3 + offset_z,
                    );
                }
            }
            return true;
        }

        false
    }
}

// ---------------------------------------------------------------------------
// ObjectJungleBigTree (HugeTreesGenerator helpers inlined).
// ---------------------------------------------------------------------------

/// Jungle big tree (`HugeTreesGenerator` subclass; base helpers
/// inlined as private methods).
pub struct JungleBigTree {
    /// Java: `HugeTreesGenerator.baseHeight`.
    base_height: i32,
    /// Java: `HugeTreesGenerator.extraRandomHeight`.
    extra_random_height: i32,
    table: Arc<TreeBlockTable>,
}

impl JungleBigTree {
    pub fn new(base_height: i32, extra_random_height: i32, table: Arc<TreeBlockTable>) -> Self {
        Self {
            base_height,
            extra_random_height,
            table,
        }
    }

    /// Java: `HugeTreesGenerator.getHeight`(L38-46).
    fn get_height(&self, rand: &mut Xoroshiro128) -> i32 {
        let mut i = rand.next_int_max(3) + self.base_height;
        if self.extra_random_height > 1 {
            i += rand.next_int_max(self.extra_random_height);
        }
        i
    }

    /// Space check with the hardcoded 256 cap.
    fn is_space_at(
        &self,
        level: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        height: i32,
    ) -> bool {
        if !(y >= 1 && y + height + 1 <= 256) {
            return false;
        }
        for i in 0..=1 + height {
            let j = if i == 0 { 1 } else { 2 };
            for k in -j..=j {
                for l in -j..=j {
                    let by = y + i;
                    if by < 0
                        || by >= 256
                        || !self
                            .table
                            .can_grow_into(level.get_block_if_cached_or_loaded(x + k, by, z + l))
                    {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Java: `HugeTreesGenerator.ensureDirtsUnderneath`(L84-97).
    fn ensure_dirts_underneath(
        &self,
        level: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let block = level.get_block_if_cached_or_loaded(x, y - 1, z);
        if (block == self.table.grass_block || block == self.table.dirt) && y >= 2 {
            set_dirt_at(&self.table, level, x, y - 1, z);
            set_dirt_at(&self.table, level, x + 1, y - 1, z);
            set_dirt_at(&self.table, level, x, y - 1, z + 1);
            set_dirt_at(&self.table, level, x + 1, y - 1, z + 1);
            true
        } else {
            false
        }
    }

    /// Java: `HugeTreesGenerator.ensureGrowable`(L103-105).
    fn ensure_growable(
        &self,
        level: &mut BlockManager<'_>,
        _rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
        height: i32,
    ) -> bool {
        self.is_space_at(level, x, y, z, height) && self.ensure_dirts_underneath(level, x, y, z)
    }

    /// Java: `HugeTreesGenerator.growLeavesLayerStrict`(L110-128)——
    /// Leaves fill the disc, fringing past the edge; air/leaves only.
    fn grow_leaves_layer_strict(
        &self,
        level: &mut BlockManager<'_>,
        cx: i32,
        cy: i32,
        cz: i32,
        width: i32,
    ) {
        let i = width * width;
        for j in -width..=width + 1 {
            for k in -width..=width + 1 {
                let l = j - 1;
                let i1 = k - 1;
                if j * j + k * k <= i
                    || l * l + i1 * i1 <= i
                    || j * j + i1 * i1 <= i
                    || l * l + k * k <= i
                {
                    let block = level.get_block_if_cached_or_loaded(cx + j, cy, cz + k);
                    if block == self.table.air || self.table.is_leaves(block) {
                        level.set_block_state_at(
                            cx + j,
                            cy,
                            cz + k,
                            0,
                            self.table.leaves[WoodType::Jungle.index()],
                        );
                    }
                }
            }
        }
    }

    /// growLeavesLayer fills the disc with leaves.
    fn grow_leaves_layer(
        &self,
        level: &mut BlockManager<'_>,
        cx: i32,
        cy: i32,
        cz: i32,
        width: i32,
    ) {
        let i = width * width;
        for j in -width..=width {
            for k in -width..=width {
                if j * j + k * k <= i {
                    let block = level.get_block_if_cached_or_loaded(cx + j, cy, cz + k);
                    if block == self.table.air || self.table.is_leaves(block) {
                        level.set_block_state_at(
                            cx + j,
                            cy,
                            cz + k,
                            0,
                            self.table.leaves[WoodType::Jungle.index()],
                        );
                    }
                }
            }
        }
    }

    /// Java: `ObjectJungleBigTree.placeVine`(L107-112)——
    /// Vines need `nextInt(3) > 0` plus air.
    fn place_vine(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
        meta: u8,
    ) {
        if rand.next_int_max(3) > 0
            && level.get_block_if_cached_or_loaded(x, y, z) == self.table.air
        {
            level.set_block_state_at(x, y, z, 0, self.table.vine_state(meta));
        }
    }
}

impl ObjectGenerator for JungleBigTree {
    /// Java: `generate`(L30-105).
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let height = self.get_height(rand);
        if !self.ensure_growable(level, rand, x, y, z, height) {
            return false;
        }

        // Java L36: createCrown(level, position.up(height), 2)
        // —— j=-2..=0: growLeavesLayerStrict(pos.up(j), 2 + 1 - j).
        for j in -2..=0 {
            self.grow_leaves_layer_strict(level, x, y + height + j, z, 2 + 1 - j);
        }

        // Side branches (spiral plus small blobs).
        let mut j = y + height - 2 - rand.next_int_max(4);
        while (j as f64) > y as f64 + height as f64 / 2.0 {
            // Java: f = nextFloat() * (PI * 2F);
            // k/l truncate with float inner and double outer arithmetic.
            let f = rand.next_float() * (std::f32::consts::PI * 2.0);
            let mut k = (x as f64 + (0.5f32 + f.cos() * 4.0f32) as f64) as i32;
            let mut l = (z as f64 + (0.5f32 + f.sin() * 4.0f32) as f64) as i32;

            for i1 in 0..5 {
                k = (x as f64 + (1.5f32 + f.cos() * i1 as f32) as f64) as i32;
                l = (z as f64 + (1.5f32 + f.sin() * i1 as f32) as f64) as i32;
                level.set_block_state_at(
                    k,
                    j - 3 + i1 / 2,
                    l,
                    0,
                    self.table.log_state(WoodType::Jungle, Axis::Y),
                );
            }

            let j2 = 1 + rand.next_int_max(2);
            for k1 in j - j2..=j {
                let l1 = k1 - j;
                self.grow_leaves_layer(level, k, k1, l, 1 - l1);
            }

            j -= 2 + rand.next_int_max(4);
        }

        // 2x2 trunk with four-way vines.
        for i2 in 0..height {
            let by = y + i2;
            if self
                .table
                .can_grow_into(level.get_block_if_cached_or_loaded(x, by, z))
            {
                level.set_block_state_at(
                    x,
                    by,
                    z,
                    0,
                    self.table.log_state(WoodType::Jungle, Axis::Y),
                );
                if i2 > 0 {
                    self.place_vine(level, rand, x - 1, by, z, 8);
                    self.place_vine(level, rand, x, by, z - 1, 1);
                }
            }

            if i2 < height - 1 {
                // blockpos.east()
                if self
                    .table
                    .can_grow_into(level.get_block_if_cached_or_loaded(x + 1, by, z))
                {
                    level.set_block_state_at(
                        x + 1,
                        by,
                        z,
                        0,
                        self.table.log_state(WoodType::Jungle, Axis::Y),
                    );
                    if i2 > 0 {
                        self.place_vine(level, rand, x + 2, by, z, 2);
                        self.place_vine(level, rand, x + 1, by, z - 1, 1);
                    }
                }
                // blockpos.south().east()
                if self
                    .table
                    .can_grow_into(level.get_block_if_cached_or_loaded(x + 1, by, z + 1))
                {
                    level.set_block_state_at(
                        x + 1,
                        by,
                        z + 1,
                        0,
                        self.table.log_state(WoodType::Jungle, Axis::Y),
                    );
                    if i2 > 0 {
                        self.place_vine(level, rand, x + 2, by, z + 1, 2);
                        self.place_vine(level, rand, x + 1, by, z + 2, 4);
                    }
                }
                // blockpos.south()
                if self
                    .table
                    .can_grow_into(level.get_block_if_cached_or_loaded(x, by, z + 1))
                {
                    level.set_block_state_at(
                        x,
                        by,
                        z + 1,
                        0,
                        self.table.log_state(WoodType::Jungle, Axis::Y),
                    );
                    if i2 > 0 {
                        self.place_vine(level, rand, x - 1, by, z + 1, 8);
                        self.place_vine(level, rand, x, by, z + 2, 4);
                    }
                }
            }
        }

        true
    }
}

// ---------------------------------------------------------------------------
// ObjectPaleOakTree
// ---------------------------------------------------------------------------

/// Pale oak (2x2 trunk with creaking hearts and hanging moss).
pub struct PaleOakTree {
    table: Arc<TreeBlockTable>,
    /// tryCreakingHeart flips a mutable field after placing one
    /// creaking heart.
    pub try_creaking_heart: bool,
}

impl PaleOakTree {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self {
            table,
            try_creaking_heart: false,
        }
    }

    /// placeTreeOfHeight checks the conical volume for grow-through.
    fn place_tree_of_height(
        &self,
        level: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        height: i32,
    ) -> bool {
        for l in 0..=height + 1 {
            let i1 = if l == 0 {
                0
            } else if l >= height - 1 {
                2
            } else {
                1
            };
            for j1 in -i1..=i1 {
                for k1 in -i1..=i1 {
                    if !self
                        .table
                        .can_grow_into(level.get_block_if_cached_or_loaded(x + j1, y + l, z + k1))
                    {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// placeLogAt needs grow-through (or a creaking heart).
    fn place_log_at(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32, creaking: bool) {
        if self
            .table
            .can_grow_into(level.get_block_if_cached_or_loaded(x, y, z))
        {
            let state = if creaking {
                self.table.creaking_heart
            } else {
                self.table.log_state(WoodType::PaleOak, Axis::Y)
            };
            level.set_block_state_at(x, y, z, 0, state);
        }
    }

    /// placeLeafAt: leaves only on air, plus hanging moss.
    fn place_leaf_at(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) {
        if level.get_block_if_cached_or_loaded(x, y, z) == self.table.air {
            level.set_block_state_at(x, y, z, 0, self.table.leaves[WoodType::PaleOak.index()]);

            let mut random =
                MtRandom::new(level.seed().wrapping_add(x as i64 + y as i64 + z as i64));
            if random.next_int_max(2) == 0 {
                let depth = random.next_int_range(1, 6);
                for i in 1..depth {
                    let py = y - i;
                    // Termination guard: cross-chunk columns always read air,
                    // so the world floor bounds the loop instead.
                    if py < level.min_height() {
                        break;
                    }
                    if level.get_block_if_cached_or_loaded(x, py, z) == self.table.air {
                        let state = if i == depth - 1 {
                            self.table.pale_hanging_moss[1]
                        } else {
                            self.table.pale_hanging_moss[0]
                        };
                        level.set_block_state_at(x, py, z, 0, state);
                    } else {
                        break;
                    }
                }
            }
        }
    }
}

impl ObjectGenerator for PaleOakTree {
    /// generate keeps the hardcoded 1/256 chance.
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        // nextInt(1) is always 0.
        let i = rand.next_int_max(1) + rand.next_int_max(1) + 6;
        let j = x;
        let k = y;
        let l = z;

        if k >= 1 && k + i + 1 < 256 {
            let ground = level.get_block_if_cached_or_loaded(j, k - 1, l);
            if ground != self.table.grass_block && ground != self.table.dirt {
                return false;
            }
            if !self.place_tree_of_height(level, j, k, l, i) {
                return false;
            }

            set_dirt_at(&self.table, level, j, k - 1, l);
            set_dirt_at(&self.table, level, j + 1, k - 1, l);
            set_dirt_at(&self.table, level, j, k - 1, l + 1);
            set_dirt_at(&self.table, level, j + 1, k - 1, l + 1);

            let enumfacing = HorizontalFace::random(rand);
            let (fx, fz) = enumfacing.offset();
            let i1 = i - rand.next_int_max(4);
            let mut j1 = 2 - rand.next_int_max(3);
            let mut k1 = j;
            let mut l1 = l;
            let i2 = k + i - 1;

            // 2x2 trunk (bent top plus creaking heart).
            for j2 in 0..i {
                if j2 >= i1 && j1 > 0 {
                    k1 += fx;
                    l1 += fz;
                    j1 -= 1;
                }

                let k2 = k + j2;
                if self
                    .table
                    .can_grow_into(level.get_block_if_cached_or_loaded(k1, k2, l1))
                {
                    // Java L71-77: creaking = nextInt(3) ∈ 0-2——
                    // `creaking == 3` never holds (upstream quirk).
                    let mut creaking = -1;
                    if self.try_creaking_heart && k2 > k && rand.next_int_max(i) == 0 {
                        self.try_creaking_heart = false;
                        creaking = rand.next_int_max(3);
                    }
                    self.place_log_at(level, k1, k2, l1, creaking == 0);
                    self.place_log_at(level, k1 + 1, k2, l1, creaking == 1);
                    self.place_log_at(level, k1, k2, l1 + 1, creaking == 2);
                    self.place_log_at(level, k1 + 1, k2, l1 + 1, creaking == 3);
                }
            }

            // Top double-layer de-cornered 2x2 leaf cap.
            for i3 in -2..=0 {
                for l3 in -2..=0 {
                    let mut k4 = -1;
                    self.place_leaf_at(level, k1 + i3, i2 + k4, l1 + l3);
                    self.place_leaf_at(level, 1 + k1 - i3, i2 + k4, l1 + l3);
                    self.place_leaf_at(level, k1 + i3, i2 + k4, 1 + l1 - l3);
                    self.place_leaf_at(level, 1 + k1 - i3, i2 + k4, 1 + l1 - l3);

                    if (i3 > -2 || l3 > -1) && (i3 != -1 || l3 != -2) {
                        k4 = 1;
                        self.place_leaf_at(level, k1 + i3, i2 + k4, l1 + l3);
                        self.place_leaf_at(level, 1 + k1 - i3, i2 + k4, l1 + l3);
                        self.place_leaf_at(level, k1 + i3, i2 + k4, 1 + l1 - l3);
                        self.place_leaf_at(level, 1 + k1 - i3, i2 + k4, 1 + l1 - l3);
                    }
                }
            }

            // Random top hat.
            if rand.next_boolean() {
                self.place_leaf_at(level, k1, i2 + 2, l1);
                self.place_leaf_at(level, k1 + 1, i2 + 2, l1);
                self.place_leaf_at(level, k1 + 1, i2 + 2, l1 + 1);
                self.place_leaf_at(level, k1, i2 + 2, l1 + 1);
            }

            // 8x8 de-cornered canopy rim.
            for j3 in -3i32..=4 {
                for i4 in -3i32..=4 {
                    if (j3 != -3 || i4 != -3)
                        && (j3 != -3 || i4 != 4)
                        && (j3 != 4 || i4 != -3)
                        && (j3 != 4 || i4 != 4)
                        && (j3.abs() < 3 || i4.abs() < 3)
                    {
                        self.place_leaf_at(level, k1 + j3, i2, l1 + i4);
                    }
                }
            }

            // Bottom branches (with hanging trunks).
            for k3 in -1..=2 {
                for j4 in -1..=2 {
                    if (k3 < 0 || k3 > 1 || j4 < 0 || j4 > 1) && rand.next_int_max(3) <= 0 {
                        let l4 = rand.next_int_max(3) + 2;

                        for i5 in 0..l4 {
                            self.place_log_at(level, j + k3, i2 - i5 - 1, l + j4, false);
                        }
                        for j5 in -1..=1 {
                            for l2 in -1..=1 {
                                self.place_leaf_at(level, k1 + k3 + j5, i2, l1 + j4 + l2);
                            }
                        }
                        for k5 in -2i32..=2 {
                            for l5 in -2i32..=2 {
                                if k5.abs() != 2 || l5.abs() != 2 {
                                    self.place_leaf_at(level, k1 + k3 + k5, i2 - 1, l1 + j4 + l5);
                                }
                            }
                        }
                    }
                }
            }

            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// ObjectSmallPaleOakTree
// ---------------------------------------------------------------------------

/// Small pale oak (pale variant of the small oak).
pub struct SmallPaleOakTree {
    /// Java: `minTreeHeight`.
    min_tree_height: i32,
    /// Java: `maxTreeHeight`.
    max_tree_height: i32,
    table: Arc<TreeBlockTable>,
}

impl SmallPaleOakTree {
    pub fn new(min_tree_height: i32, max_tree_height: i32, table: Arc<TreeBlockTable>) -> Self {
        Self {
            min_tree_height,
            max_tree_height,
            table,
        }
    }

    /// Inlined placeLeafAt: leaves on air/leaves/moss spots plus
    /// hanging moss.
    ///
    /// Note the upstream quirk: the loop increments the tree height
    /// instead of the loop variable, so non-air below terminates it;
    /// moss starts about one tree-height under the leaves (plus a floor guard).
    fn place_leaf_with_moss(
        &self,
        level: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        tree_height: i32,
    ) {
        let block = level.get_block_if_cached_or_loaded(x, y, z);
        if block == self.table.air
            || self.table.is_leaves(block)
            || block == self.table.pale_hanging_moss[0]
            || block == self.table.pale_hanging_moss[1]
        {
            level.set_block_state_at(x, y, z, 0, self.table.leaves[WoodType::PaleOak.index()]);

            let mut random =
                MtRandom::new(level.seed().wrapping_add(x as i64 + y as i64 + z as i64));
            if random.next_int_max(2) == 0 {
                let depth = random.next_int_range(1, 6);
                let j = 1;
                let mut i = tree_height;
                while j < depth {
                    let py = y - i;
                    // Termination guard (same as pale oak).
                    if py < level.min_height() {
                        break;
                    }
                    if level.get_block_if_cached_or_loaded(x, py, z) == self.table.air {
                        let state = if i == depth - 1 {
                            self.table.pale_hanging_moss[1]
                        } else {
                            self.table.pale_hanging_moss[0]
                        };
                        level.set_block_state_at(x, py, z, 0, state);
                    } else {
                        break;
                    }
                    i += 1;
                }
            }
        }
    }
}

impl ObjectGenerator for SmallPaleOakTree {
    /// Java: `generate`(L41-127).
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let i = rand.next_int_max(self.max_tree_height) + self.min_tree_height;
        let mut flag = true;

        if y >= level.min_height() && y + i + 1 < level.max_height() {
            // Whole-volume grow-through check.
            for j in y..=y + 1 + i {
                let k = if j == y {
                    0
                } else if j >= y + 1 + i - 2 {
                    2
                } else {
                    1
                };
                for l in x - k..=x + k {
                    for i1 in z - k..=z + k {
                        if !(j >= level.min_height() && j < level.max_height())
                            || !self
                                .table
                                .can_grow_into(level.get_block_if_cached_or_loaded(l, j, i1))
                        {
                            flag = false;
                        }
                    }
                }
            }

            if !flag {
                return false;
            }

            let ground = level.get_block_if_cached_or_loaded(x, y - 1, z);
            if (ground == self.table.grass_block
                || ground == self.table.dirt
                || ground == self.table.farmland)
                && y < level.max_height() - i - 1
            {
                set_dirt_at(&self.table, level, x, y - 1, z);

                // Per-layer leaves (random de-cornering).
                for i3 in y - 3 + i..=y + i {
                    let i4 = i3 - (y + i);
                    let j1 = 1 - i4 / 2;

                    for k1 in x - j1..=x + j1 {
                        let l1 = k1 - x;
                        for i2 in z - j1..=z + j1 {
                            let j2 = i2 - z;
                            // Java L96: |l1| != j1 || |j2| != j1 || (nextInt(2) != 0 && i4 != 0)
                            if l1.abs() != j1
                                || j2.abs() != j1
                                || (rand.next_int_max(2) != 0 && i4 != 0)
                            {
                                self.place_leaf_with_moss(level, k1, i3, i2, i);
                            }
                        }
                    }
                }
                true
            } else {
                false
            }
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// ObjectMangroveTree
// ---------------------------------------------------------------------------

/// Mangrove tree constants.
const MANGROVE_ROOT_WIDTH_LIMIT: i32 = 8;
const MANGROVE_ROOT_LENGTH_LIMIT: usize = 15;
const MANGROVE_ROOT_RANDOM_SKEW_CHANCE: f32 = 0.2;
const MANGROVE_BRANCH_PER_LOG_PROBABILITY: f32 = 0.5;
const MANGROVE_LEAF_RADIUS: i32 = 3;
const MANGROVE_LEAF_HEIGHT: i32 = 2;
const MANGROVE_LEAF_PLACEMENT_ATTEMPTS: i32 = 70;
const MANGROVE_VINE_PROBABILITY: f32 = 0.125;
const MANGROVE_PROPAGULE_PROBABILITY: f32 = 0.14;

/// Java: `ObjectMangroveTree.MangroveProperties` record(L394-404).
struct MangroveProperties {
    base_height: i32,
    height_rand_a: i32,
    height_rand_b: i32,
    extra_branch_steps_min: i32,
    extra_branch_steps_max: i32,
    extra_branch_length_min: i32,
    extra_branch_length_max: i32,
    root_offset_min: i32,
    root_offset_max: i32,
}

/// Position tuples.
type Pos = (i32, i32, i32);

/// 48-bit LCG random, used only for shuffling.
struct JavaUtilRandom {
    seed: u64,
}

impl JavaUtilRandom {
    /// setSeed xors the scramble, then truncates to 48 bits.
    fn new(seed: i64) -> Self {
        Self {
            seed: (seed as u64).wrapping_add(0x5DEECE66D) & ((1u64 << 48) - 1),
        }
    }

    /// Java: `next(bits)`.
    fn next(&mut self, bits: u32) -> i32 {
        self.seed = self.seed.wrapping_mul(0x5DEECE66D).wrapping_add(0xB) & ((1u64 << 48) - 1);
        (self.seed >> (48 - bits)) as i32
    }

    /// Java: `nextInt(bound)`(bound > 0).
    fn next_int_bound(&mut self, bound: i32) -> i32 {
        let mut r = self.next(31);
        let m = bound - 1;
        if (bound & m) == 0 {
            // Powers of two shift directly.
            ((bound as i64 * r as i64) >> 31) as i32
        } else {
            loop {
                let u = r;
                r = u % bound;
                if u.wrapping_sub(r).wrapping_add(m) >= 0 {
                    break;
                }
                r = self.next(31);
            }
            r
        }
    }
}

/// Fisher-Yates shuffle (RandomAccess path).
fn java_shuffle<T>(list: &mut [T], rnd: &mut JavaUtilRandom) {
    for i in (1..list.len()).rev() {
        let j = rnd.next_int_bound(i as i32 + 1) as usize;
        list.swap(i, j);
    }
}

/// Java: `ObjectMangroveTree.getVineMeta`(L341-349).
fn mangrove_vine_meta(attached_to: HorizontalFace) -> u8 {
    match attached_to {
        HorizontalFace::South => 1,
        HorizontalFace::West => 2,
        HorizontalFace::North => 4,
        HorizontalFace::East => 8,
    }
}

/// Java: `ObjectMangroveTree.randomHorizontal`(L351-353)——
/// Horizontal pick order is S-W-N-E.
fn mangrove_random_horizontal(rand: &mut Xoroshiro128) -> HorizontalFace {
    match rand.next_bounded_int(3) {
        0 => HorizontalFace::South,
        1 => HorizontalFace::West,
        2 => HorizontalFace::North,
        _ => HorizontalFace::East,
    }
}

/// Mangrove trees (prop roots, branches, propagules, vines).
pub struct MangroveTree {
    table: Arc<TreeBlockTable>,
    /// withBeenest defaults false (nests need entity NBT, unbuilt).
    pub with_bee_nest: bool,
    /// Java: `beeCount`.
    pub bee_count: i32,
    /// Java: `tall`.
    tall: bool,
}

impl MangroveTree {
    /// Java: `ObjectMangroveTree(boolean tall)`
    /// (Parameterless construction uses nondeterministic randomness; unused here.)
    pub fn new(table: Arc<TreeBlockTable>, tall: bool) -> Self {
        Self {
            table,
            with_bee_nest: false,
            bee_count: 3,
            tall,
        }
    }

    /// Java: `canPlaceLogInto`(L355-363).
    fn can_place_log_into(&self, block: BlockRuntimeId) -> bool {
        block == self.table.air
            || self.table.is_water_or_flowing(block)
            || block == self.table.leaves[WoodType::Mangrove.index()]
            || self.table.is_vine(block)
            || self.table.is_mangrove_propagule(block)
    }

    /// Java: `canPlaceLeafInto`(L365-371).
    fn can_place_leaf_into(&self, block: BlockRuntimeId) -> bool {
        block == self.table.air
            || self.table.is_water_or_flowing(block)
            || self.table.is_vine(block)
            || self.table.is_mangrove_propagule(block)
    }

    /// Java: `canPlaceRoot`(L373-384).
    fn can_place_root(&self, block: BlockRuntimeId) -> bool {
        block == self.table.air
            || self.table.is_water_or_flowing(block)
            || block == self.table.mud
            || block == self.table.mangrove_roots
            || block == self.table.muddy_mangrove_roots_y
            || block == self.table.leaves[WoodType::Mangrove.index()]
            || self.table.is_mangrove_propagule(block)
            || self.table.is_vine(block)
    }

    /// placeWithWaterlogging tops up layer-1 still water where placed in water.
    fn place_with_waterlogging(
        &self,
        level: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        state: sc_world::chunk::BlockRuntimeId,
    ) {
        let previous = level.get_block_if_cached_or_loaded(x, y, z);
        level.set_block_state_at(x, y, z, 0, state);
        if self.table.is_water_or_flowing(previous) {
            level.set_block_state_at(x, y, z, 1, self.table.water);
        }
    }

    /// Java: `placeLog`(L319-326).
    fn place_log(&self, level: &mut BlockManager<'_>, pos: Pos) -> bool {
        let previous = level.get_block_if_cached_or_loaded(pos.0, pos.1, pos.2);
        if !self.can_place_log_into(previous) {
            return false;
        }
        self.place_with_waterlogging(
            level,
            pos.0,
            pos.1,
            pos.2,
            self.table.log_state(WoodType::Mangrove, Axis::Y),
        );
        true
    }

    /// Java: `placeLeaf`(L328-333).
    fn place_leaf(&self, level: &mut BlockManager<'_>, pos: Pos) {
        let previous = level.get_block_if_cached_or_loaded(pos.0, pos.1, pos.2);
        if self.can_place_leaf_into(previous) {
            self.place_with_waterlogging(
                level,
                pos.0,
                pos.1,
                pos.2,
                self.table.leaves[WoodType::Mangrove.index()],
            );
        }
    }

    /// Java: `placeVine`(L335-339).
    fn place_vine(&self, level: &mut BlockManager<'_>, pos: Pos, attached_to: HorizontalFace) {
        level.set_block_state_at(
            pos.0,
            pos.1,
            pos.2,
            0,
            self.table.vine_state(mangrove_vine_meta(attached_to)),
        );
    }

    /// placeTrunk: main trunk plus random side branches.
    fn place_trunk(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        tree_height: i32,
        origin: Pos,
        props: &MangroveProperties,
    ) -> Vec<Pos> {
        let mut attachments = Vec::new();

        for height_pos in 0..tree_height {
            let current_height = origin.1 + height_pos;
            let log_pos = (origin.0, current_height, origin.2);
            if self.place_log(level, log_pos)
                && height_pos < tree_height - 1
                && rand.next_float() < MANGROVE_BRANCH_PER_LOG_PROBABILITY
            {
                let branch_dir = mangrove_random_horizontal(rand);
                let (dx, dz) = branch_dir.offset();
                let branch_len = random_range(
                    rand,
                    props.extra_branch_length_min,
                    props.extra_branch_length_max,
                );
                let branch_pos = (branch_len
                    - random_range(
                        rand,
                        props.extra_branch_length_min,
                        props.extra_branch_length_max,
                    )
                    - 1)
                .max(0);
                let branch_steps = random_range(
                    rand,
                    props.extra_branch_steps_min,
                    props.extra_branch_steps_max,
                );
                self.place_branch(
                    level,
                    tree_height,
                    &mut attachments,
                    log_pos,
                    current_height,
                    dx,
                    dz,
                    branch_pos,
                    branch_steps,
                );
            }

            if height_pos == tree_height - 1 {
                attachments.push((origin.0, current_height + 1, origin.2));
            }
        }

        attachments
    }

    /// Java: `placeBranch`(L106-135).
    fn place_branch(
        &self,
        level: &mut BlockManager<'_>,
        tree_height: i32,
        attachments: &mut Vec<Pos>,
        log_pos: Pos,
        current_height: i32,
        dx: i32,
        dz: i32,
        branch_pos: i32,
        mut branch_steps: i32,
    ) {
        let mut height_along_branch = current_height + branch_pos;
        let mut log_x = log_pos.0;
        let mut log_z = log_pos.2;
        let mut branch_placement_index = branch_pos;

        while branch_placement_index < tree_height && branch_steps > 0 {
            if branch_placement_index >= 1 {
                let placement_height = current_height + branch_placement_index;
                log_x += dx;
                log_z += dz;
                let branch_log_pos = (log_x, placement_height, log_z);
                height_along_branch = placement_height;
                if self.place_log(level, branch_log_pos) {
                    height_along_branch = placement_height + 1;
                }
                attachments.push(branch_log_pos);
            }
            branch_placement_index += 1;
            branch_steps -= 1;
        }

        if height_along_branch - current_height > 1 {
            let foliage_pos = (log_x, height_along_branch, log_z);
            attachments.push(foliage_pos);
            attachments.push((foliage_pos.0, foliage_pos.1 - 2, foliage_pos.2));
        }
    }

    /// Java: `placeRoots`(L137-164).
    fn place_roots(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        origin: Pos,
        trunk_origin: Pos,
    ) -> bool {
        let mut root_positions: Vec<Pos> = Vec::new();
        let mut column_pos = origin;

        while column_pos.1 < trunk_origin.1 {
            if !self.can_place_root(level.get_block_if_cached_or_loaded(
                column_pos.0,
                column_pos.1,
                column_pos.2,
            )) {
                return false;
            }
            column_pos.1 += 1;
        }

        root_positions.push((trunk_origin.0, trunk_origin.1 - 1, trunk_origin.2));
        for direction in HorizontalFace::ALL {
            let (dx, dz) = direction.offset();
            let pos = (trunk_origin.0 + dx, trunk_origin.1, trunk_origin.2 + dz);
            let mut positions_in_direction = Vec::new();
            if !self.simulate_roots(
                level,
                rand,
                pos,
                direction,
                trunk_origin,
                &mut positions_in_direction,
                0,
            ) {
                return false;
            }
            root_positions.extend(positions_in_direction);
            root_positions.push(pos);
        }

        for root_pos in root_positions {
            self.place_root(level, rand, root_pos);
        }
        true
    }

    /// simulateRoots: recursive root simulation.
    fn simulate_roots(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        root_pos: Pos,
        dir: HorizontalFace,
        root_origin: Pos,
        root_positions: &mut Vec<Pos>,
        layer: i32,
    ) -> bool {
        if layer == MANGROVE_ROOT_LENGTH_LIMIT as i32
            || root_positions.len() > MANGROVE_ROOT_LENGTH_LIMIT
        {
            return false;
        }

        for pos in self.potential_root_positions(root_pos, dir, rand, root_origin) {
            if self.can_place_root(level.get_block_if_cached_or_loaded(pos.0, pos.1, pos.2)) {
                root_positions.push(pos);
                if !self.simulate_roots(
                    level,
                    rand,
                    pos,
                    dir,
                    root_origin,
                    root_positions,
                    layer + 1,
                ) {
                    return false;
                }
            }
        }

        true
    }

    /// Java: `potentialRootPositions`(L184-197).
    fn potential_root_positions(
        &self,
        pos: Pos,
        previous_dir: HorizontalFace,
        rand: &mut Xoroshiro128,
        root_origin: Pos,
    ) -> Vec<Pos> {
        let below = (pos.0, pos.1 - 1, pos.2);
        let (dx, dz) = previous_dir.offset();
        let next_to = (pos.0 + dx, pos.1, pos.2 + dz);
        let next_to_down = (next_to.0, next_to.1 - 1, next_to.2);
        let width = (pos.0 - root_origin.0).abs()
            + (pos.1 - root_origin.1).abs()
            + (pos.2 - root_origin.2).abs();

        if width > MANGROVE_ROOT_WIDTH_LIMIT - 3 && width <= MANGROVE_ROOT_WIDTH_LIMIT {
            if rand.next_float() < MANGROVE_ROOT_RANDOM_SKEW_CHANCE {
                vec![below, next_to_down]
            } else {
                vec![below]
            }
        } else if width > MANGROVE_ROOT_WIDTH_LIMIT {
            vec![below]
        } else if rand.next_float() < MANGROVE_ROOT_RANDOM_SKEW_CHANCE {
            vec![below]
        } else if rand.next_boolean() {
            vec![next_to]
        } else {
            vec![below]
        }
    }

    /// placeRoot: muddy roots on mud, plain roots elsewhere;
    /// moss carpets crown half the root tops.
    fn place_root(&self, level: &mut BlockManager<'_>, rand: &mut Xoroshiro128, pos: Pos) {
        let previous = level.get_block_if_cached_or_loaded(pos.0, pos.1, pos.2);
        // Id-based checks cover any pillar_axis state; terrain and placed
        // muddy roots both default to y axis, so one state compares equal.
        let state = if previous == self.table.mud || previous == self.table.muddy_mangrove_roots_y {
            self.table.muddy_mangrove_roots_y
        } else {
            self.table.mangrove_roots
        };
        self.place_with_waterlogging(level, pos.0, pos.1, pos.2, state);

        if rand.next_float() < 0.5 {
            let above = (pos.0, pos.1 + 1, pos.2);
            if level.get_block_if_cached_or_loaded(above.0, above.1, above.2) == self.table.air {
                level.set_block_state_at(above.0, above.1, above.2, 0, self.table.moss_carpet);
            }
        }
    }

    /// Java: `createRandomSpreadFoliage`(L214-221).
    fn create_random_spread_foliage(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        origin: Pos,
    ) {
        for _ in 0..MANGROVE_LEAF_PLACEMENT_ATTEMPTS {
            let x = origin.0 + rand.next_int_max(MANGROVE_LEAF_RADIUS)
                - rand.next_int_max(MANGROVE_LEAF_RADIUS);
            let y = origin.1 + rand.next_int_max(MANGROVE_LEAF_HEIGHT)
                - rand.next_int_max(MANGROVE_LEAF_HEIGHT);
            let z = origin.2 + rand.next_int_max(MANGROVE_LEAF_RADIUS)
                - rand.next_int_max(MANGROVE_LEAF_RADIUS);
            self.place_leaf(level, (x, y, z));
        }
    }

    /// collectPlacedLeaves gathers placed mangrove leaves by id.
    /// (Sorted for determinism; distribution stays equivalent.)
    fn collect_placed_leaves(
        &self,
        level: &mut BlockManager<'_>,
        foliage_attachments: &[Pos],
    ) -> Vec<Pos> {
        let mut leaves = std::collections::BTreeSet::new();
        for origin in foliage_attachments {
            for x in origin.0 - MANGROVE_LEAF_RADIUS + 1..=origin.0 + MANGROVE_LEAF_RADIUS - 1 {
                for y in origin.1 - MANGROVE_LEAF_HEIGHT + 1..=origin.1 + MANGROVE_LEAF_HEIGHT - 1 {
                    for z in
                        origin.2 - MANGROVE_LEAF_RADIUS + 1..=origin.2 + MANGROVE_LEAF_RADIUS - 1
                    {
                        if level.get_block_if_cached_or_loaded(x, y, z)
                            == self.table.leaves[WoodType::Mangrove.index()]
                        {
                            leaves.insert((x, y, z));
                        }
                    }
                }
            }
        }
        leaves.into_iter().collect()
    }

    /// Java: `placeLeafVines`(L223-231)+ `maybePlaceVine`(L233-237)+
    /// `addHangingVine`(L239-249).
    fn place_leaf_vines(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        foliage_attachments: &[Pos],
    ) {
        let leaves = self.collect_placed_leaves(level, foliage_attachments);
        for leaf_pos in leaves {
            // Four horizontal directions.
            self.maybe_place_vine(
                level,
                rand,
                (leaf_pos.0 - 1, leaf_pos.1, leaf_pos.2),
                HorizontalFace::East,
            );
            self.maybe_place_vine(
                level,
                rand,
                (leaf_pos.0 + 1, leaf_pos.1, leaf_pos.2),
                HorizontalFace::West,
            );
            self.maybe_place_vine(
                level,
                rand,
                (leaf_pos.0, leaf_pos.1, leaf_pos.2 - 1),
                HorizontalFace::South,
            );
            self.maybe_place_vine(
                level,
                rand,
                (leaf_pos.0, leaf_pos.1, leaf_pos.2 + 1),
                HorizontalFace::North,
            );
        }
    }

    /// Java: `maybePlaceVine`(L233-237).
    fn maybe_place_vine(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        pos: Pos,
        attached_to: HorizontalFace,
    ) {
        if rand.next_float() < MANGROVE_VINE_PROBABILITY
            && level.get_block_if_cached_or_loaded(pos.0, pos.1, pos.2) == self.table.air
        {
            self.add_hanging_vine(level, pos, attached_to);
        }
    }

    /// addHangingVine extends down at most 4.
    fn add_hanging_vine(
        &self,
        level: &mut BlockManager<'_>,
        pos: Pos,
        attached_to: HorizontalFace,
    ) {
        self.place_vine(level, pos, attached_to);
        let mut vine_pos = (pos.0, pos.1 - 1, pos.2);
        let mut max_length = 4;

        while level.get_block_if_cached_or_loaded(vine_pos.0, vine_pos.1, vine_pos.2)
            == self.table.air
            && max_length > 0
        {
            self.place_vine(level, vine_pos, attached_to);
            vine_pos.1 -= 1;
            max_length -= 1;
        }
    }

    /// placePropagules hangs propagules under shuffled leaves.
    fn place_propagules(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        foliage_attachments: &[Pos],
    ) {
        let mut blacklist = std::collections::HashSet::new();
        let mut leaves = self.collect_placed_leaves(level, foliage_attachments);
        let mut shuffle_random = JavaUtilRandom::new(rand.next_long());
        java_shuffle(&mut leaves, &mut shuffle_random);

        for leaf_pos in leaves {
            let placement_pos = (leaf_pos.0, leaf_pos.1 - 1, leaf_pos.2);
            if !blacklist.contains(&placement_pos)
                && rand.next_float() < MANGROVE_PROPAGULE_PROBABILITY
                && self.has_required_empty_blocks(level, leaf_pos, 2)
            {
                for x in placement_pos.0 - 1..=placement_pos.0 + 1 {
                    for z in placement_pos.2 - 1..=placement_pos.2 + 1 {
                        blacklist.insert((x, placement_pos.1, z));
                    }
                }

                let stage = random_range(rand, 0, 4);
                let propagule = self.table.mangrove_propagule[1][stage.clamp(0, 4) as usize];
                level.set_block_state_at(
                    placement_pos.0,
                    placement_pos.1,
                    placement_pos.2,
                    0,
                    propagule,
                );
            }
        }
    }

    /// hasRequiredEmptyBlocks needs count air cells below.
    fn has_required_empty_blocks(
        &self,
        level: &mut BlockManager<'_>,
        leaf_pos: Pos,
        count: i32,
    ) -> bool {
        for i in 1..=count {
            if level.get_block_if_cached_or_loaded(leaf_pos.0, leaf_pos.1 - i, leaf_pos.2)
                != self.table.air
            {
                return false;
            }
        }
        true
    }

    /// placeBeeNest needs `BeeNestGenerator` (blocks plus bee entity NBT);
    /// without entity NBT it stays a no-op when `with_bee_nest` is false
    /// and only logs when true.
    fn place_bee_nest(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        foliage_attachments: &[Pos],
    ) {
        if !self.with_bee_nest || foliage_attachments.is_empty() {
            return;
        }
        let attachment =
            foliage_attachments[rand.next_int_max(foliage_attachments.len() as i32) as usize];
        let nest_pos = (attachment.0, attachment.1 - 1, attachment.2 + 1);
        let nest_block = level.get_block_if_cached_or_loaded(nest_pos.0, nest_pos.1, nest_pos.2);
        let above = level.get_block_if_cached_or_loaded(nest_pos.0, nest_pos.1 + 1, nest_pos.2);
        if nest_block != self.table.air || above == self.table.air {
            return;
        }
        // TODO: BeeNestGenerator (bee_nest blocks plus bee entities) needs entity infrastructure.
        log::warn!("{}", t_log!("console.worldgen.bee_nest_pending"));
    }
}

impl ObjectGenerator for MangroveTree {
    /// Java: `generate`(L55-80).
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let props = if self.tall {
            MangroveProperties {
                base_height: 4,
                height_rand_a: 1,
                height_rand_b: 9,
                extra_branch_steps_min: 1,
                extra_branch_steps_max: 6,
                extra_branch_length_min: 0,
                extra_branch_length_max: 1,
                root_offset_min: 3,
                root_offset_max: 7,
            }
        } else {
            MangroveProperties {
                base_height: 2,
                height_rand_a: 1,
                height_rand_b: 4,
                extra_branch_steps_min: 1,
                extra_branch_steps_max: 4,
                extra_branch_length_min: 0,
                extra_branch_length_max: 1,
                root_offset_min: 1,
                root_offset_max: 3,
            }
        };

        let origin = (x, y, z);
        let trunk_offset_y = random_range(rand, props.root_offset_min, props.root_offset_max);
        let trunk_origin = (x, y + trunk_offset_y, z);
        let tree_height = props.base_height
            + rand.next_int_max(props.height_rand_a + 1)
            + rand.next_int_max(props.height_rand_b + 1);

        if !self.place_roots(level, rand, origin, trunk_origin) {
            return false;
        }

        let foliage_attachments = self.place_trunk(level, rand, tree_height, trunk_origin, &props);
        for attachment in &foliage_attachments {
            self.create_random_spread_foliage(level, rand, *attachment);
        }

        self.place_leaf_vines(level, rand, &foliage_attachments);
        self.place_propagules(level, rand, &foliage_attachments);
        self.place_bee_nest(level, rand, &foliage_attachments);
        true
    }
}
