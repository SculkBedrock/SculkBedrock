//! Port of the `object/legacytree/` series (`LegacyTreeGenerator` plus 13 kinds),
//! `feature/LegacyTreeGeneratorFeature.java`, and `object/BeeNestGenerator.java`
//! (block subset).
//!
//! | Rust item | Upstream source |
//! |---|---|
//! | [`LegacyTreeGenerator`] (kind dispatch) | `legacytree/LegacyTreeGenerator.java` |
//! | `LegacyTreeKind::Oak` | `legacytree/LegacyOakTree.java` |
//! | `LegacyTreeKind::Birch` | `legacytree/LegacyBirchTree.java` |
//! | `LegacyTreeKind::TallBirch` | `legacytree/LegacyTallBirchTree.java` |
//! | `LegacyTreeKind::Spruce` | `legacytree/LegacySpruceTree.java` |
//! | `LegacyTreeKind::Jungle` | `legacytree/LegacyJungleTree.java` |
//! | `LegacyTreeKind::DarkOak` | `legacytree/LegacyDarkOakTree.java` |
//! | `LegacyTreeKind::BigSpruce` | `legacytree/LegacyBigSpruceTree.java` |
//! | `LegacyTreeKind::Crimson`/`Warped` | `legacytree/LegacyNetherTree.java` + `LegacyCrimsonTree.java`/`LegacyWarpedTree.java` |
//! | `LegacyTreeKind::Chorus` | `legacytree/LegacyChorusTree.java` |
//! | [`grow_grass`] | `legacytree/LegacyTallGrass.java` |
//! | [`grow_tree`] | `LegacyTreeGenerator.growTree` (static dispatch) |
//! | [`place_bee_nest`] | `object/BeeNestGenerator.java` (bee populate needs entity infrastructure) |
//! | [`LegacyTreeGeneratorFeature`] | `feature/LegacyTreeGeneratorFeature.java` |
//!
//! The inheritance tree (mutable base state plus subclass overrides) becomes
//! one struct dispatched by [`LegacyTreeKind`]: match arms equal virtual dispatch,
//! and its fields carry the base instance state.
//!
//! Known upstream quirks (faithfully kept):
//! - `growTree` builds `LegacyDarkOakTree(6, 3)`: multiplier=6 keeps
//!   `topSize = h - 6h` negative, so `placeLeaves` never runs (2x2 trunk only);
//! - `LegacyDarkOakTree.placeTrunk` checks overridable at `(x, y+yy, z)`
//!   but places at `(x+xx, y+yy, z+zz)` (big spruce checks per cell).

use std::sync::Arc;

use sc_world::chunk::BlockRuntimeId;

use crate::worldgen::biome::biome_id::*;
use crate::worldgen::chunk::WorldgenChunk;
use crate::worldgen::context::{BlockManager, ChunkGenerateContext};
use crate::worldgen::feature::object::{ObjectGenerator, TreeBlockTable, WoodType};
use crate::worldgen::feature::tree::{
    add_vines_around_log, FancyOakTree, HorizontalFace, TREE_WITH_VINES_CHANCE,
};
use crate::worldgen::feature::GenerateFeature;
use crate::worldgen::math::{java_string_hashcode, random_range};
use crate::worldgen::random::{RandomSourceProvider, Xoroshiro128};
use crate::worldgen::stages::chunk_hash;

// ---------------------------------------------------------------------------
// LegacyTreeGenerator plus 13 subclasses.
// ---------------------------------------------------------------------------

/// Kind dispatch for the 13 legacy trees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LegacyTreeKind {
    /// `LegacyOakTree` (fancy brace diverts to `ObjectFancyOakTree`).
    Oak,
    /// `LegacyBirchTree`.
    Birch,
    /// `LegacyTallBirchTree`.
    TallBirch,
    /// `LegacySpruceTree`.
    Spruce,
    /// `LegacyJungleTree`.
    Jungle,
    /// `LegacyDarkOakTree(leafStartHeightMultiplier, baseLeafRadius)`.
    DarkOak {
        leaf_start_height_multiplier: f32,
        base_leaf_radius: i32,
    },
    /// `LegacyBigSpruceTree(leafStartHeightMultiplier, baseLeafRadius)`.
    BigSpruce {
        leaf_start_height_multiplier: f32,
        base_leaf_radius: i32,
    },
    /// `LegacyCrimsonTree` (nether height fixed at construction).
    Crimson { height: i32 },
    /// `LegacyWarpedTree`.
    Warped { height: i32 },
    /// `LegacyChorusTree`.
    Chorus,
}

/// Java: `abstract class LegacyTreeGenerator extends TreeGenerator`.
///
/// One struct carries the mutable base state; methods dispatch
/// by [`LegacyTreeKind`] like virtual overrides.
pub struct LegacyTreeGenerator {
    kind: LegacyTreeKind,
    table: Arc<TreeBlockTable>,
    /// Base tree height, default 7 (subclasses randomize before placeObject;
    /// nether trees fix it at construction).
    tree_height: i32,
    /// Java: `protected boolean treeWithVines`.
    tree_with_vines: bool,
}

impl LegacyTreeGenerator {
    /// Constructor: nether trees take the build height, others start at 7.
    pub fn new(kind: LegacyTreeKind, table: Arc<TreeBlockTable>) -> Self {
        let tree_height = match &kind {
            LegacyTreeKind::Crimson { height } | LegacyTreeKind::Warped { height } => *height,
            _ => 7,
        };
        Self {
            kind,
            table,
            tree_height,
            tree_with_vines: false,
        }
    }

    /// Random height of the parameterless nether constructor.
    ///(`RandomSourceProvider.create().nextInt(9) + 4` → 4-12).
    pub fn random_nether_height(rand: &mut Xoroshiro128) -> i32 {
        rand.next_int_max(9) + 4
    }

    /// Java: `getTreeHeight()`.
    pub fn tree_height(&self) -> i32 {
        self.tree_height
    }

    /// `getType()` skips the base trunk/leaf switch for crimson/warped/chorus;
    /// subclasses override trunk/leaf block states.
    fn get_type(&self) -> WoodType {
        match self.kind {
            LegacyTreeKind::Oak => WoodType::Oak,
            LegacyTreeKind::Birch | LegacyTreeKind::TallBirch => WoodType::Birch,
            LegacyTreeKind::Spruce | LegacyTreeKind::BigSpruce { .. } => WoodType::Spruce,
            LegacyTreeKind::Jungle => WoodType::Jungle,
            LegacyTreeKind::DarkOak { .. } => WoodType::DarkOak,
            LegacyTreeKind::Crimson { .. }
            | LegacyTreeKind::Warped { .. }
            | LegacyTreeKind::Chorus => WoodType::Oak,
        }
    }

    /// Java: `canGenerateWithVines()`——OAK/SPRUCE/JUNGLE/DARK_OAK.
    fn can_generate_with_vines(&self) -> bool {
        matches!(
            self.get_type(),
            WoodType::Oak | WoodType::Spruce | WoodType::Jungle | WoodType::DarkOak
        )
    }

    /// Java: `setRandomTreeWithVines(random)`.
    fn set_random_tree_with_vines(&mut self, rand: &mut Xoroshiro128) {
        self.tree_with_vines =
            self.can_generate_with_vines() && rand.next_int_max(TREE_WITH_VINES_CHANCE) == 0;
    }

    /// Trunk block state (crimson/warped subclasses override).
    fn trunk_block_state(&self) -> BlockRuntimeId {
        match self.kind {
            LegacyTreeKind::Crimson { .. } => self.table.crimson_stem,
            LegacyTreeKind::Warped { .. } => self.table.warped_stem,
            _ => self.table.log_of(self.get_type()),
        }
    }

    /// Leaf block state (crimson/warped subclasses override).
    fn leaf_block_state(&self) -> BlockRuntimeId {
        match self.kind {
            LegacyTreeKind::Crimson { .. } => self.table.nether_wart_block,
            LegacyTreeKind::Warped { .. } => self.table.warped_wart_block,
            _ => self.table.leaves_of(self.get_type()),
        }
    }

    /// Java: `overridable(Block)`(L40-74)→ `TreeBlockTable::is_overridable`.
    fn overridable(&self, block: BlockRuntimeId) -> bool {
        self.table.is_overridable(block)
    }

    /// Fancy-brace check: non-overridable neighbors (e.g. stone) switch
    /// to the fancy oak.
    fn has_fancy_brace(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) -> bool {
        for xx in -1..=1 {
            for zz in -1..=1 {
                if xx == 0 && zz == 0 {
                    continue;
                }
                if !self.overridable(level.get_block_if_cached_or_loaded(x + xx, y + 1, z + zz)) {
                    return true;
                }
            }
        }
        false
    }

    /// Placement check (LegacyOakTree overrides
    /// `hasFancyBrace || super`).
    pub fn can_place_object(
        &mut self,
        level: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        if matches!(self.kind, LegacyTreeKind::Oak) && self.has_fancy_brace(level, x, y, z) {
            return true;
        }
        let mut radius_to_check = 0i32;
        for yy in 0..(self.tree_height + 3) {
            if yy == 1 || yy == self.tree_height {
                radius_to_check += 1;
            }
            for xx in -radius_to_check..=radius_to_check {
                for zz in -radius_to_check..=radius_to_check {
                    if !self.overridable(level.get_block_if_cached_or_loaded(
                        x + xx,
                        y + yy,
                        z + zz,
                    )) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// placeObject: base plus per-subclass dispatch.
    pub fn place_object(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) {
        let kind = self.kind;
        match kind {
            LegacyTreeKind::Oak => {
                // Fancy-brace diversion.
                if self.has_fancy_brace(level, x, y, z) {
                    let mut fancy = FancyOakTree::new(self.table.clone());
                    fancy.generate(level, rand, x, y, z);
                    return;
                }
                self.tree_height = rand.next_int_max(3) + 4;
                self.place_object_default(level, rand, x, y, z);
            }
            LegacyTreeKind::Birch => {
                // LegacyBirchTree.placeObject:nextInt(2) + 5.
                self.tree_height = rand.next_int_max(2) + 5;
                self.place_object_default(level, rand, x, y, z);
            }
            LegacyTreeKind::TallBirch => {
                // LegacyTallBirchTree.placeObject:nextInt(3) + 10.
                self.tree_height = rand.next_int_max(3) + 10;
                self.place_object_default(level, rand, x, y, z);
            }
            LegacyTreeKind::Jungle => {
                // LegacyJungleTree.placeObject:nextInt(6) + 4.
                self.tree_height = rand.next_int_max(6) + 4;
                self.place_object_default(level, rand, x, y, z);
            }
            LegacyTreeKind::Spruce => {
                // LegacySpruceTree.placeObject(L19-29).
                self.tree_height = rand.next_int_max(4) + 6;
                self.set_random_tree_with_vines(rand);
                let top_size = self.tree_height - (1 + rand.next_int_max(2));
                let l_radius = 2 + rand.next_int_max(2);
                self.place_trunk(level, x, y, z, self.tree_height - rand.next_int_max(3));
                self.place_conical_leaves(level, top_size, l_radius, x, y, z, rand, true);
            }
            LegacyTreeKind::DarkOak {
                leaf_start_height_multiplier,
                base_leaf_radius,
            } => {
                // LegacyDarkOakTree.placeObject(L26-39).
                if self.tree_height == 0 {
                    self.tree_height = rand.next_int_max(15) + 20;
                }
                let top_size = self.tree_height
                    - (self.tree_height as f32 * leaf_start_height_multiplier) as i32;
                let l_radius = base_leaf_radius + rand.next_int_max(2);
                self.set_random_tree_with_vines(rand);
                self.place_trunk_dark_oak(level, x, y, z, self.tree_height - rand.next_int_max(3));
                self.place_conical_leaves(level, top_size, l_radius, x, y, z, rand, false);
            }
            LegacyTreeKind::BigSpruce {
                leaf_start_height_multiplier,
                base_leaf_radius,
            } => {
                // No vines on big spruce.
                if self.tree_height == 7 {
                    self.tree_height = rand.next_int_max(15) + 20;
                }
                let top_size = self.tree_height
                    - (self.tree_height as f32 * leaf_start_height_multiplier) as i32;
                let l_radius = base_leaf_radius + rand.next_int_max(2);
                self.place_trunk_big_spruce(
                    level,
                    x,
                    y,
                    z,
                    self.tree_height - rand.next_int_max(3),
                );
                self.place_conical_leaves(level, top_size, l_radius, x, y, z, rand, false);
            }
            LegacyTreeKind::Crimson { .. } | LegacyTreeKind::Warped { .. } => {
                self.place_nether_object(level, rand, x, y, z);
            }
            LegacyTreeKind::Chorus => {
                let _ = self.generate_chorus(level, rand, x, y, z, 8);
            }
        }
    }

    /// Default placeObject shared by oak/birch/jungle,
    /// L119-141).
    fn place_object_default(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) {
        self.set_random_tree_with_vines(rand);
        self.place_trunk(level, x, y, z, self.tree_height - 1);

        let h = self.tree_height;
        for yy in (y - 3 + h)..=(y + h) {
            let y_off = (yy - (y + h)) as f64;
            let mid = (1.0 - y_off / 2.0) as i32;
            for xx in (x - mid)..=(x + mid) {
                let x_off = (xx - x).abs();
                for zz in (z - mid)..=(z + mid) {
                    let z_off = (zz - z).abs();
                    if x_off == mid && z_off == mid && (y_off == 0.0 || rand.next_int_max(2) == 0) {
                        continue;
                    }
                    let block_at = level.get_block_if_cached_or_loaded(xx, yy, zz);
                    if !self.table.is_solid(block_at) {
                        level.set_block_state_at(xx, yy, zz, 0, self.leaf_block_state());
                    }
                }
            }
        }
    }

    /// Base placeTrunk.
    fn place_trunk(
        &mut self,
        level: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        trunk_height: i32,
    ) {
        // The base dirt block
        level.set_block_state_at(x, y - 1, z, 0, self.table.dirt);

        for yy in 0..trunk_height {
            let b = level.get_block_if_cached_or_loaded(x, y + yy, z);
            if self.overridable(b) {
                level.set_block_state_at(x, y + yy, z, 0, self.trunk_block_state());
                if self.tree_with_vines {
                    add_vines_around_log(&self.table, level, x, y + yy, z);
                }
            }
        }
    }

    /// Dark oak placeTrunk override: 2x2 trunk.
    ///
    /// Note: upstream checks overridable at `(x, y+yy, z)` but places at
    /// `(x+xx, y+yy, z+zz)` (quirk kept).
    fn place_trunk_dark_oak(
        &mut self,
        level: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        trunk_height: i32,
    ) {
        // The base dirt block
        level.set_block_state_at(x, y - 1, z, 0, self.table.dirt);
        let radius = 2;

        for yy in 0..trunk_height {
            for xx in 0..radius {
                for zz in 0..radius {
                    let b = level.get_block_if_cached_or_loaded(x, y + yy, z);
                    if self.overridable(b) {
                        level.set_block_state_at(
                            x + xx,
                            y + yy,
                            z + zz,
                            0,
                            self.trunk_block_state(),
                        );
                        if self.tree_with_vines {
                            add_vines_around_log(&self.table, level, x + xx, y + yy, z + zz);
                        }
                    }
                }
            }
        }
    }

    /// Big spruce placeTrunk override: 2x2 trunk, per-cell checks, no vines.
    fn place_trunk_big_spruce(
        &mut self,
        level: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        trunk_height: i32,
    ) {
        // The base dirt block
        level.set_block_state_at(x, y - 1, z, 0, self.table.dirt);
        let radius = 2;

        for yy in 0..trunk_height {
            for xx in 0..radius {
                for zz in 0..radius {
                    let b = level.get_block_if_cached_or_loaded(x + xx, y + yy, z + zz);
                    if self.overridable(b) {
                        level.set_block_state_at(
                            x + xx,
                            y + yy,
                            z + zz,
                            0,
                            self.trunk_block_state(),
                        );
                    }
                }
            }
        }
    }

    /// Java: `LegacySpruceTree.placeLeaves` / `LegacyDarkOakTree.placeLeaves`
    /// Big spruce leaves share the conical algorithm, plus a dirt pedestal.
    fn place_conical_leaves(
        &mut self,
        level: &mut BlockManager<'_>,
        top_size: i32,
        l_radius: i32,
        x: i32,
        y: i32,
        z: i32,
        rand: &mut Xoroshiro128,
        dirt_base: bool,
    ) {
        let mut radius = rand.next_int_max(2);
        let mut max_r = 1;
        let mut min_r = 0;
        if dirt_base {
            level.set_block_state_at(x, y - 1, z, 0, self.table.dirt);
        }
        for yy in 0..=top_size {
            let yyy = y + self.tree_height - yy;

            for xx in (x - radius)..=(x + radius) {
                let x_off = (xx - x).abs();
                for zz in (z - radius)..=(z + radius) {
                    let z_off = (zz - z).abs();
                    if x_off == radius && z_off == radius && radius > 0 {
                        continue;
                    }
                    let block_at = level.get_block_if_cached_or_loaded(xx, yyy, zz);
                    if !self.table.is_solid(block_at) {
                        level.set_block_state_at(xx, yyy, zz, 0, self.leaf_block_state());
                    }
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
    }

    /// Nether checkY keeps trees out of the top bedrock.
    /// Only overworld runs in this pipeline; nether/end generators are unported.
    fn check_y(&self, y: i32) -> bool {
        y > 318
    }

    /// Nether placeObject shared by crimson/warped.
    fn place_nether_object(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) {
        if self.check_y(y) {
            // prevent growing into the top bedrock layer
            return;
        }

        self.place_trunk_nether(level, x, y, z, self.tree_height);

        // Java L43-44: blankArea = -3 → mid = (int)(1 - (-3)/2) = 2.
        let mid = 2i32;
        let h = self.tree_height;

        // Three ring layers (corners skipped 50%; 1/20 shroomlight).
        for yy in (y - 3 + h)..=(y + h - 1) {
            if self.check_y(yy) {
                continue;
            }
            // Row sweep (xx full range, zz steps by mid*2).
            let mut xx = x - mid;
            while xx <= x + mid {
                let mut zz = z - mid;
                while zz <= z + mid {
                    self.place_nether_leaf(level, rand, x, z, xx, yy, zz, mid);
                    zz += mid * 2;
                }
                xx += 1;
            }
            // Column sweep (zz full range, xx steps by mid*2).
            let mut zz = z - mid;
            while zz <= z + mid {
                let mut xx = x - mid;
                while xx <= x + mid {
                    self.place_nether_leaf(level, rand, x, z, xx, yy, zz, mid);
                    xx += mid * 2;
                }
                zz += 1;
            }
        }

        // Two drooping layers below.
        for yy in (y - 4 + h)..=(y + h - 3) {
            if self.check_y(yy) {
                continue;
            }
            let mut xx = x - mid;
            while xx <= x + mid {
                let mut zz = z - mid;
                while zz <= z + mid {
                    self.place_nether_hanging_leaf(level, rand, xx, yy, zz, 5);
                    zz += mid * 2;
                }
                xx += 1;
            }
            let mut zz = z - mid;
            while zz <= z + mid {
                let mut xx = x - mid;
                while xx <= x + mid {
                    self.place_nether_hanging_leaf(level, rand, xx, yy, zz, 4);
                    xx += mid * 2;
                }
                zz += 1;
            }
        }

        // Top 3x3 cap.
        for x_canopy in (x - mid + 1)..=(x + mid - 1) {
            for z_canopy in (z - mid + 1)..=(z + mid - 1) {
                let block = level.get_block_if_cached_or_loaded(x_canopy, y + h, z_canopy);
                if !self.table.is_solid(block) {
                    level.set_block_state_at(x_canopy, y + h, z_canopy, 0, self.leaf_block_state());
                }
            }
        }
    }

    /// Single-cell nether leaves (corners skipped 50% plus shroomlight).
    fn place_nether_leaf(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        z: i32,
        xx: i32,
        yy: i32,
        zz: i32,
        mid: i32,
    ) {
        let x_off = (xx - x).abs();
        let z_off = (zz - z).abs();
        if x_off == mid && z_off == mid && rand.next_int_max(2) == 0 {
            return;
        }
        let block = level.get_block_if_cached_or_loaded(xx, yy, zz);
        if !self.table.is_solid(block) {
            if rand.next_int_max(20) == 0 {
                level.set_block_state_at(xx, yy, zz, 0, self.table.shroomlight);
            } else {
                level.set_block_state_at(xx, yy, zz, 0, self.leaf_block_state());
            }
        }
    }

    /// Single-cell drooping nether leaves.
    fn place_nether_hanging_leaf(
        &self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        xx: i32,
        yy: i32,
        zz: i32,
        next_bound: i32,
    ) {
        let block = level.get_block_if_cached_or_loaded(xx, yy, zz);
        if !self.table.is_solid(block) {
            if rand.next_int_max(3) == 0 {
                let drops = rand.next_int_max(next_bound);
                for i in 0..drops {
                    let block2 = level.get_block_if_cached_or_loaded(xx, yy - i, zz);
                    if !self.table.is_solid(block2) {
                        level.set_block_state_at(xx, yy - i, zz, 0, self.leaf_block_state());
                    }
                }
            }
        }
    }

    /// Nether placeTrunk override: no dirt pedestal,
    /// places `(x, y, z)` first.
    fn place_trunk_nether(
        &mut self,
        level: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        trunk_height: i32,
    ) {
        level.set_block_state_at(x, y, z, 0, self.trunk_block_state());
        for yy in 0..trunk_height {
            if self.check_y(y + yy) {
                // prevent growing into the top bedrock layer
                continue;
            }
            let b = level.get_block_if_cached_or_loaded(x, y + yy, z);
            if self.overridable(b) {
                level.set_block_state_at(x, y + yy, z, 0, self.trunk_block_state());
            }
        }
    }

    /// Java: `LegacyChorusTree.generate(level, rand, position, maxSize)`
    ///(L25-29).
    pub fn generate_chorus(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
        max_size: i32,
    ) -> bool {
        level.set_block_state_at(x, y, z, 0, self.table.chorus_plant);
        self.grow_immediately(level, rand, x, y, z, max_size, 0);
        true
    }

    /// Java: `LegacyChorusTree.growImmediately`(L31-66).
    fn grow_immediately(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
        max_size: i32,
        age: i32,
    ) {
        // Random height
        let mut height = 1 + rand.next_int_max(4);
        if age == 0 {
            height += 1;
        }

        // Grow upward
        for yy in 1..=height {
            if !self.is_horizontal_air(level, x, y + yy, z) {
                return;
            }
            level.set_block_state_at(x, y + yy, z, 0, self.table.chorus_plant);
        }

        if age < 4 {
            // Grow horizontally
            let mut attempt = rand.next_int_max(4);
            if age == 0 {
                attempt += 1;
            }

            for _ in 0..attempt {
                let face = HorizontalFace::random(rand);
                let (ox, oz) = face.offset();
                let cx = x + ox;
                let cy = y + height;
                let cz = z + oz;
                if level.get_block_if_cached_or_loaded(cx, cy, cz) == self.table.air
                    && level.get_block_if_cached_or_loaded(cx, cy - 1, cz) == self.table.air
                {
                    if (cx - x).abs() < max_size
                        && (cz - z).abs() < max_size
                        && self.is_horizontal_air_except(level, cx, cy, cz, face.opposite())
                    {
                        level.set_block_state_at(cx, cy, cz, 0, self.table.chorus_plant);
                        self.grow_immediately(level, rand, cx, cy, cz, max_size, age + 1);
                    }
                }
            }
        } else {
            // Death
            level.set_block_state_at(x, y + height, z, 0, self.table.chorus_flower_fully_aged);
        }
    }

    /// Java: `LegacyChorusTree.isHorizontalAir`(L68-76).
    fn is_horizontal_air(&self, level: &mut BlockManager<'_>, x: i32, y: i32, z: i32) -> bool {
        for face in HorizontalFace::ALL {
            let (ox, oz) = face.offset();
            if level.get_block_if_cached_or_loaded(x + ox, y, z + oz) != self.table.air {
                return false;
            }
        }
        true
    }

    /// Java: `LegacyChorusTree.isHorizontalAirExcept`(L78-88).
    fn is_horizontal_air_except(
        &self,
        level: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        except: HorizontalFace,
    ) -> bool {
        for face in HorizontalFace::ALL {
            if face != except {
                let (ox, oz) = face.offset();
                if level.get_block_if_cached_or_loaded(x + ox, y, z + oz) != self.table.air {
                    return false;
                }
            }
        }
        true
    }
}

impl ObjectGenerator for LegacyTreeGenerator {
    /// Java: `LegacyTreeGenerator.generate`(placeObject + true);
    /// `LegacyChorusTree.generate`(maxSize=8).
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        if matches!(self.kind, LegacyTreeKind::Chorus) {
            self.generate_chorus(level, rand, x, y, z, 8)
        } else {
            self.place_object(level, rand, x, y, z);
            true
        }
    }

    /// Java: `generator instanceof LegacyOakTree || instanceof LegacyBirchTree`
    /// (Beehive eligibility.)
    fn is_bee_nest_eligible(&self) -> bool {
        matches!(self.kind, LegacyTreeKind::Oak | LegacyTreeKind::Birch)
    }
}

/// Java: `LegacyTreeGenerator.growTree(level, x, y, z, random, type, tall)`
/// (Static dispatch for sapling growth.)
///
/// Note `new LegacyDarkOakTree(6, 3)` keeps topSize negative,
/// so placeLeaves never runs.
pub fn grow_tree(
    level: &mut BlockManager<'_>,
    x: i32,
    y: i32,
    z: i32,
    rand: &mut Xoroshiro128,
    wood: WoodType,
    tall: bool,
    table: &Arc<TreeBlockTable>,
) {
    let kind = match wood {
        WoodType::Spruce => LegacyTreeKind::Spruce,
        WoodType::Birch => {
            if tall {
                LegacyTreeKind::TallBirch
            } else {
                LegacyTreeKind::Birch
            }
        }
        WoodType::DarkOak => LegacyTreeKind::DarkOak {
            leaf_start_height_multiplier: 6.0,
            base_leaf_radius: 3,
        },
        WoodType::Jungle => LegacyTreeKind::Jungle,
        // TODO: more complex trees
        _ => LegacyTreeKind::Oak,
    };

    let mut tree = LegacyTreeGenerator::new(kind, table.clone());
    if tree.can_place_object(level, x, y, z) {
        tree.place_object(level, rand, x, y, z);
    }
}

// ---------------------------------------------------------------------------
// LegacyTallGrass(legacytree/LegacyTallGrass.java)
// ---------------------------------------------------------------------------

/// Java: `LegacyTallGrass.growGrass(level, pos, random)`(L28-77)——
/// Scatter one of 14 surface plants above grass in a 5x5 area.
pub fn grow_grass(
    table: &TreeBlockTable,
    level: &mut BlockManager<'_>,
    x: i32,
    y: i32,
    z: i32,
    rand: &mut Xoroshiro128,
) {
    let base_y = y + 1;
    for gx in (x - 2)..=(x + 2) {
        for gz in (z - 2)..=(z + 2) {
            // Java L36: newY = y + nextInt(2) * (nextBoolean() ? -1 : 1)
            let new_y = base_y + rand.next_int_max(2) * if rand.next_boolean() { -1 } else { 1 };
            if rand.next_boolean()
                && level.get_block_if_cached_or_loaded(gx, new_y, gz) == table.air
                && level.get_block_if_cached_or_loaded(gx, new_y - 1, gz) == table.grass_block
            {
                // Java L39: (int) Math.round(nextGaussian() * 1000)
                let ran_number = (rand.next_gaussian() * 1000.0).round() as i32;
                let abs_rn = ran_number.abs();
                let places = &table.tall_grass_places;
                if (-300..=300).contains(&ran_number) {
                    level.set_block_state_at(gx, new_y, gz, 0, places[0]);
                } else if (300..=500).contains(&abs_rn) {
                    // -300 ~ -500 + 300 ~ 500
                    level.set_block_state_at(gx, new_y, gz, 0, places[1]);
                    // Upper plant half.
                    level.set_block_state_at(gx, new_y + 1, gz, 0, table.tall_grass_upper);
                } else if (500..600).contains(&ran_number) {
                    level.set_block_state_at(gx, new_y, gz, 0, places[2]);
                } else if (-600..=-500).contains(&ran_number) {
                    level.set_block_state_at(gx, new_y, gz, 0, places[3]);
                } else if (600..700).contains(&ran_number) {
                    level.set_block_state_at(gx, new_y, gz, 0, places[4]);
                } else if (-700..-600).contains(&ran_number) {
                    level.set_block_state_at(gx, new_y, gz, 0, places[5]);
                } else if (-750..-700).contains(&ran_number) {
                    level.set_block_state_at(gx, new_y, gz, 0, places[6]);
                } else if (-800..-750).contains(&ran_number) {
                    level.set_block_state_at(gx, new_y, gz, 0, places[7]);
                } else if (-850..-800).contains(&ran_number) {
                    level.set_block_state_at(gx, new_y, gz, 0, places[8]);
                } else if (-900..-850).contains(&ran_number) {
                    level.set_block_state_at(gx, new_y, gz, 0, places[9]);
                } else if (-1000..-900).contains(&ran_number) {
                    level.set_block_state_at(gx, new_y, gz, 0, places[10]);
                } else if (700..800).contains(&ran_number) {
                    level.set_block_state_at(gx, new_y, gz, 0, places[11]);
                } else if (800..900).contains(&ran_number) {
                    level.set_block_state_at(gx, new_y, gz, 0, places[12]);
                } else if (900..1000).contains(&ran_number) {
                    level.set_block_state_at(gx, new_y, gz, 0, places[13]);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// BeeNestGenerator (block subset).
// ---------------------------------------------------------------------------

/// Java: `BeeNestGenerator.place(level, random, treePosition)`(L20-22 → L24-39).
///
/// Walk up the trunk for the first spot with solid wood, air for the nest,
/// and solid above; bee populate needs entity infrastructure, so only
/// the `nextInt(2, 4)` beeCount draw runs to keep the random stream aligned.
pub fn place_bee_nest(
    table: &TreeBlockTable,
    level: &mut BlockManager<'_>,
    rand: &mut Xoroshiro128,
    x: i32,
    y: i32,
    z: i32,
) -> bool {
    // beeCount only feeds entity populate.
    let _bee_count = rand.next_int_range(2, 4);
    for leaf_y in (y + 1)..=(y + 32) {
        // nestPosition = (x, leafY - 1, z + 1)
        if level.get_block_if_cached_or_loaded(x, leaf_y - 1, z) == table.air
            || level.get_block_if_cached_or_loaded(x, leaf_y - 1, z + 1) != table.air
            || level.get_block_if_cached_or_loaded(x, leaf_y, z + 1) == table.air
        {
            continue;
        }
        // Java placeAt: bee_nest{direction=SOUTH(0), honey_level=0}
        level.set_block_state_at(x, leaf_y - 1, z + 1, 0, table.bee_nest);
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// LegacyObjectWrapper(object/ObjectLegacyObjectWrapper.java)
// ---------------------------------------------------------------------------

/// Java: `class ObjectLegacyObjectWrapper extends TreeGenerator`.
///
/// Adapt [`LegacyTreeGenerator`] as an [`ObjectGenerator`]
/// (always true); SavannaTreeFeature mixes in legacy oaks through it.
pub struct LegacyObjectWrapper {
    generator: LegacyTreeGenerator,
}

impl LegacyObjectWrapper {
    pub fn new(generator: LegacyTreeGenerator) -> Self {
        Self { generator }
    }
}

impl ObjectGenerator for LegacyObjectWrapper {
    /// Returns true after placeObject.
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        self.generator.place_object(level, rand, x, y, z);
        true
    }
}

// ---------------------------------------------------------------------------
// LegacyTreeGeneratorFeature(feature/LegacyTreeGeneratorFeature.java)
// ---------------------------------------------------------------------------

/// Java: `abstract class LegacyTreeGeneratorFeature extends GenerateFeature
/// implements Supportable`(L21-74).
///
/// Differences from `ObjectGeneratorFeature`:
/// - No checkBlock downward scan (heightmap top decides support);
/// - Biome filtering uses `getRequiredTag`;
///   tag ids come from the hardcoded [`biome_id_matches_tag`] set;
/// - One BlockManager per tree, merged after generation;
/// - Legacy oak/birch carry beehive chances.
pub trait LegacyTreeGeneratorFeature: GenerateFeature {
    /// Java: `abstract TreeGenerator getGenerator(RandomSourceProvider)`(L23).
    ///
    /// A null generator aborts the whole feature inside apply,
    /// discarding already-merged trees (quirk kept).
    fn get_generator(&self, random: &mut Xoroshiro128) -> Option<Box<dyn ObjectGenerator>>;

    /// Java: `getMin()`(L25-27).
    fn get_min(&self) -> i32 {
        5
    }

    /// Java: `getMax()`(L29-31).
    fn get_max(&self) -> i32 {
        6
    }

    /// Java: `getRequiredTag()`(L33-35)——`BiomeTags.OVERWORLD`.
    fn get_required_tag(&self) -> &'static str {
        "overworld"
    }

    /// Java: `getBeeNestChance()`(L37-39).
    fn get_bee_nest_chance(&self) -> f32 {
        0.0
    }

    /// Tree block table (support-dirt checks).
    fn tree_table(&self) -> &TreeBlockTable;

    /// Java: `final void apply(ChunkGenerateContext)`(L42-74).
    ///
    /// Seed formula (`^` chain):
    /// `levelSeed ^ chunkHash(chunkX, chunkZ) ^ name().hashCode()`.
    fn legacy_apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        let sx = chunk_x << 4;
        let sz = chunk_z << 4;

        let name_hash = java_string_hashcode(self.name()) as i64;
        let seed = ctx.level_seed() ^ chunk_hash(chunk_x, chunk_z) ^ name_hash;
        let mut random = Xoroshiro128::new(seed);

        let amount = random_range(&mut random, self.get_min(), self.get_max());

        // Fresh BlockManager as the merge buffer.
        let chunk_ref: &WorldgenChunk = ctx.chunk;
        let mut manager = BlockManager::with_chunk_and_seed(chunk_ref, ctx.level_seed());

        for _ in 0..amount {
            // Java L52-54: random.nextInt(15) × 2
            let x = random.next_int_max(15);
            let z = random.next_int_max(15);
            // Java L54: chunk.getHeightMap(x, z)
            let y = ctx.chunk.height_map(x as u8, z as u8);
            // Java L55-57: y < level.getMinHeight() → continue
            if y < ctx.min_y() {
                continue;
            }
            let wx = x + sx;
            let wz = z + sz;

            // Java L60: Registries.BIOME.containsTag(getRequiredTag(), biomeId)
            if !biome_id_matches_tag(
                self.get_required_tag(),
                ctx.chunk.biome_id(x as u8, y, z as u8),
            ) {
                continue;
            }

            // Java L61: isSupportDirt(level.getBlock(v))
            if self
                .tree_table()
                .is_support_dirt(ctx.chunk.block_state(x as u8, y, z as u8, 0))
            {
                // Merge each per-tree BlockManager after generation.
                let mut object = BlockManager::with_chunk_and_seed(chunk_ref, ctx.level_seed());
                // A null generator returns early: already-merged trees drop
                // with the feature (quirk kept).
                // (queueObject never runs).
                let Some(mut generator) = self.get_generator(&mut random) else {
                    return;
                };
                // Java L64: treePosition = v.up(1)
                let tree_y = y + 1;
                let generated = generator.generate(&mut object, &mut random, wx, tree_y, wz);
                // Legacy oak/birch plus chance yields beehives.
                if generated
                    && generator.is_bee_nest_eligible()
                    && random.next_float() < self.get_bee_nest_chance()
                {
                    place_bee_nest(self.tree_table(), &mut object, &mut random, wx, tree_y, wz);
                }
                // Java L70: manager.merge(object)
                manager.merge(object);
            }
        }

        // Java L73: queueObject(chunk, manager) → root.merge——
        // Buffer until stage end, then submit once.
        let places = manager.into_places();
        ctx.queue_object(places);
    }
}

// ---------------------------------------------------------------------------
// Hardcoded biome tags.
// ---------------------------------------------------------------------------

/// Hardcoded approximation of tag containment checks.
///
/// Biome tag data ships without this repo, so Bedrock biome ids
/// and stock biome groupings are hardcoded; unknown tags are
/// always false.
pub fn biome_id_matches_tag(tag: &str, biome_id: i32) -> bool {
    match tag {
        // OVERWORLD: all overworld ids (classic 0-49 minus HELL/THE_END,
        // variants 129-167, 1.18+ mountain and cave biomes 182-194).
        "overworld" => {
            ((0..=49).contains(&biome_id) && biome_id != HELL && biome_id != THE_END)
                || (129..=167).contains(&biome_id)
                || (182..=194).contains(&biome_id)
        }
        "forest" => [FOREST, FOREST_HILLS, FLOWER_FOREST].contains(&biome_id),
        "flower_forest" => biome_id == FLOWER_FOREST,
        "plains" => [PLAINS, SUNFLOWER_PLAINS].contains(&biome_id),
        "birch" => [
            BIRCH_FOREST,
            BIRCH_FOREST_HILLS,
            BIRCH_FOREST_MUTATED,
            BIRCH_FOREST_HILLS_MUTATED,
        ]
        .contains(&biome_id),
        "taiga" => [
            TAIGA,
            TAIGA_HILLS,
            COLD_TAIGA,
            COLD_TAIGA_HILLS,
            MEGA_TAIGA,
            MEGA_TAIGA_HILLS,
            TAIGA_MUTATED,
            COLD_TAIGA_MUTATED,
            REDWOOD_TAIGA_MUTATED,
            REDWOOD_TAIGA_HILLS_MUTATED,
        ]
        .contains(&biome_id),
        "jungle" => [JUNGLE, JUNGLE_HILLS, JUNGLE_MUTATED].contains(&biome_id),
        "edge" => [JUNGLE_EDGE, JUNGLE_EDGE_MUTATED].contains(&biome_id),
        "bamboo" => [BAMBOO_JUNGLE, BAMBOO_JUNGLE_HILLS].contains(&biome_id),
        "savanna" => [
            SAVANNA,
            SAVANNA_PLATEAU,
            SAVANNA_MUTATED,
            SAVANNA_PLATEAU_MUTATED,
        ]
        .contains(&biome_id),
        "swamp" => [SWAMPLAND, SWAMPLAND_MUTATED].contains(&biome_id),
        // DESERT: desert and variants (used by the structure populator).
        "desert" => [DESERT, DESERT_HILLS, DESERT_MUTATED].contains(&biome_id),
        "mangrove_swamp" => biome_id == MANGROVE_SWAMP,
        "cherry_grove" => biome_id == CHERRY_GROVE,
        "pale_garden" => biome_id == PALE_GARDEN,
        "meadow" => biome_id == MEADOW,
        "grove" => biome_id == GROVE,
        "mesa" => [
            MESA,
            MESA_PLATEAU_STONE,
            MESA_PLATEAU,
            MESA_BRYCE,
            MESA_PLATEAU_STONE_MUTATED,
            MESA_PLATEAU_MUTATED,
        ]
        .contains(&biome_id),
        "roofed" => [ROOFED_FOREST, ROOFED_FOREST_MUTATED].contains(&biome_id),
        "mooshroom_island" => [MUSHROOM_ISLAND, MUSHROOM_ISLAND_SHORE].contains(&biome_id),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sc_world::chunk::{Chunk, ChunkPosition};

    fn make_chunk() -> WorldgenChunk {
        WorldgenChunk::new(Chunk::empty_overworld(ChunkPosition::new(0, 0)))
    }

    #[test]
    fn biome_tags_match_hardcoded_ids() {
        // overworld: all overworld ids, nether/end excluded.
        assert!(biome_id_matches_tag("overworld", PLAINS));
        assert!(biome_id_matches_tag("overworld", FOREST));
        assert!(biome_id_matches_tag("overworld", CHERRY_GROVE));
        assert!(biome_id_matches_tag("overworld", JAGGED_PEAKS));
        assert!(!biome_id_matches_tag("overworld", HELL));
        assert!(!biome_id_matches_tag("overworld", THE_END));
        assert!(!biome_id_matches_tag("overworld", CRIMSON_FOREST));
        // Per-tree feature tags.
        assert!(biome_id_matches_tag("forest", FOREST));
        assert!(biome_id_matches_tag("forest", FLOWER_FOREST));
        assert!(!biome_id_matches_tag("forest", PLAINS));
        assert!(biome_id_matches_tag("taiga", MEGA_TAIGA));
        assert!(!biome_id_matches_tag("taiga", FOREST));
        assert!(biome_id_matches_tag("jungle", JUNGLE_HILLS));
        assert!(!biome_id_matches_tag("jungle", BAMBOO_JUNGLE));
        assert!(biome_id_matches_tag("bamboo", BAMBOO_JUNGLE));
        assert!(biome_id_matches_tag("edge", JUNGLE_EDGE));
        assert!(biome_id_matches_tag("savanna", SAVANNA_PLATEAU));
        assert!(biome_id_matches_tag("swamp", SWAMPLAND_MUTATED));
        assert!(biome_id_matches_tag("mangrove_swamp", MANGROVE_SWAMP));
        assert!(biome_id_matches_tag("roofed", ROOFED_FOREST));
        assert!(biome_id_matches_tag("mesa", MESA_BRYCE));
        assert!(biome_id_matches_tag("cherry_grove", CHERRY_GROVE));
        assert!(biome_id_matches_tag("pale_garden", PALE_GARDEN));
        assert!(biome_id_matches_tag("meadow", MEADOW));
        assert!(biome_id_matches_tag("grove", GROVE));
        assert!(biome_id_matches_tag("mooshroom_island", MUSHROOM_ISLAND));
        // Unknown tags map to false.
        assert!(!biome_id_matches_tag("nonexistent_tag", FOREST));
    }

    #[test]
    fn legacy_oak_places_trunk_and_dirt_base() {
        let table = Arc::new(TreeBlockTable::from_core_palette());
        let chunk = make_chunk();
        let mut level = BlockManager::with_chunk(&chunk);
        let mut rand = Xoroshiro128::new(0x5EED_CAFE);
        let mut tree = LegacyTreeGenerator::new(LegacyTreeKind::Oak, table.clone());
        assert!(tree.generate(&mut level, &mut rand, 8, 64, 8));
        // Java: treeHeight = nextInt(3) + 4 → 4-6
        assert!((4..=6).contains(&tree.tree_height()));
        if table.dirt != table.air {
            // Trunk (visible in buffer).
            assert_eq!(
                level.get_block_if_cached_or_loaded(8, 64, 8),
                table.log_of(WoodType::Oak)
            );
            // Dirt pedestal.
            assert_eq!(level.get_block_if_cached_or_loaded(8, 63, 8), table.dirt);
            // Leafy top center.
            let h = tree.tree_height();
            assert_eq!(
                level.get_block_if_cached_or_loaded(8, 64 + h, 8),
                table.leaves_of(WoodType::Oak)
            );
        }
    }

    #[test]
    fn legacy_birch_spruce_jungle_height_ranges() {
        let table = Arc::new(TreeBlockTable::from_core_palette());
        let chunk = make_chunk();
        let mut level = BlockManager::with_chunk(&chunk);
        let mut rand = Xoroshiro128::new(42);

        let mut birch = LegacyTreeGenerator::new(LegacyTreeKind::Birch, table.clone());
        birch.generate(&mut level, &mut rand, 4, 64, 4);
        assert!((5..=6).contains(&birch.tree_height()));

        let mut tall_birch = LegacyTreeGenerator::new(LegacyTreeKind::TallBirch, table.clone());
        tall_birch.generate(&mut level, &mut rand, 4, 64, 12);
        assert!((10..=12).contains(&tall_birch.tree_height()));

        let mut spruce = LegacyTreeGenerator::new(LegacyTreeKind::Spruce, table.clone());
        spruce.generate(&mut level, &mut rand, 12, 64, 4);
        assert!((6..=9).contains(&spruce.tree_height()));

        let mut jungle = LegacyTreeGenerator::new(LegacyTreeKind::Jungle, table.clone());
        jungle.generate(&mut level, &mut rand, 12, 64, 12);
        assert!((4..=9).contains(&jungle.tree_height()));
    }

    #[test]
    fn legacy_dark_oak_places_2x2_trunk() {
        let table = Arc::new(TreeBlockTable::from_core_palette());
        let chunk = make_chunk();
        let mut level = BlockManager::with_chunk(&chunk);
        let mut rand = Xoroshiro128::new(7);
        let mut tree = LegacyTreeGenerator::new(
            LegacyTreeKind::DarkOak {
                leaf_start_height_multiplier: 0.3,
                base_leaf_radius: 3,
            },
            table.clone(),
        );
        tree.generate(&mut level, &mut rand, 8, 64, 8);
        if table.dirt != table.air {
            // 2x2 trunk.
            for xx in 0..2 {
                for zz in 0..2 {
                    assert_eq!(
                        level.get_block_if_cached_or_loaded(8 + xx, 64, 8 + zz),
                        table.log_of(WoodType::DarkOak)
                    );
                }
            }
        }
    }

    #[test]
    fn legacy_big_spruce_randomizes_height_from_default() {
        let table = Arc::new(TreeBlockTable::from_core_palette());
        let chunk = make_chunk();
        let mut level = BlockManager::with_chunk(&chunk);
        let mut rand = Xoroshiro128::new(11);
        let mut tree = LegacyTreeGenerator::new(
            LegacyTreeKind::BigSpruce {
                leaf_start_height_multiplier: 0.3,
                base_leaf_radius: 3,
            },
            table.clone(),
        );
        // Default height 7 rolls nextInt(15) + 20, giving 20-34.
        tree.generate(&mut level, &mut rand, 8, 64, 8);
        assert!((20..=34).contains(&tree.tree_height()));
    }

    #[test]
    fn legacy_nether_tree_places_trunk_and_leaves() {
        let table = Arc::new(TreeBlockTable::from_core_palette());
        let chunk = make_chunk();
        let mut level = BlockManager::with_chunk(&chunk);
        let mut rand = Xoroshiro128::new(13);
        let mut tree =
            LegacyTreeGenerator::new(LegacyTreeKind::Crimson { height: 5 }, table.clone());
        assert_eq!(tree.tree_height(), 5);
        tree.generate(&mut level, &mut rand, 8, 64, 8);
        if table.crimson_stem != table.air {
            // Trunk first at (x,y,z), then 0..height.
            assert_eq!(
                level.get_block_if_cached_or_loaded(8, 64, 8),
                table.crimson_stem
            );
            // Top 3x3 cap (leafy center, no trunk).
            let top = level.get_block_if_cached_or_loaded(8, 64 + 5, 8);
            assert!(
                top == table.nether_wart_block || top == table.crimson_stem,
                "顶部应为疣块叶或干，实际 {top:?}"
            );
        }
    }

    #[test]
    fn legacy_chorus_grows_from_base() {
        let table = Arc::new(TreeBlockTable::from_core_palette());
        let chunk = make_chunk();
        let mut level = BlockManager::with_chunk(&chunk);
        let mut rand = Xoroshiro128::new(17);
        let mut tree = LegacyTreeGenerator::new(LegacyTreeKind::Chorus, table.clone());
        assert!(tree.generate(&mut level, &mut rand, 8, 64, 8));
        if table.chorus_plant != table.air {
            // Base plant.
            assert_eq!(
                level.get_block_if_cached_or_loaded(8, 64, 8),
                table.chorus_plant
            );
            // Upward growth (age=0 with height >= 2 always plants y+1).
            assert_eq!(
                level.get_block_if_cached_or_loaded(8, 65, 8),
                table.chorus_plant
            );
        }
    }

    #[test]
    fn bee_nest_places_adjacent_to_trunk() {
        let table = Arc::new(TreeBlockTable::from_core_palette());
        let chunk = make_chunk();
        let mut level = BlockManager::with_chunk(&chunk);
        // Hand-built tree: trunk plus leaves above the nest spot.
        for y in 64..=70 {
            level.set_block_state_at(8, y, 8, 0, table.log_of(WoodType::Oak));
        }
        level.set_block_state_at(8, 71, 9, 0, table.leaves_of(WoodType::Oak));
        let mut rand = Xoroshiro128::new(3);
        let placed = place_bee_nest(&table, &mut level, &mut rand, 8, 64, 8);
        if table.bee_nest != table.air {
            assert!(placed);
            // First valid spot puts the nest at (8, 70, 9).
            assert_eq!(
                level.get_block_if_cached_or_loaded(8, 70, 9),
                table.bee_nest
            );
        }
    }

    #[test]
    fn grow_grass_places_plant_on_grass_block() {
        let table = Arc::new(TreeBlockTable::from_core_palette());
        if table.grass_block == table.air {
            // Palette unloaded (all air fallback): smoke test only.
            return;
        }
        let mut chunk = make_chunk();
        // 5x5 grass patch scan range.
        for gx in 6..=10 {
            for gz in 6..=10 {
                chunk.set_block_state(gx as u8, 63, gz as u8, 0, table.grass_block);
            }
        }
        chunk.set_height_map(8, 8, 63);
        // Multi-seed attempts (total failure is ~1e-12).
        let mut placed_any = false;
        for seed in 0..16u64 {
            let mut level = BlockManager::with_chunk(&chunk);
            let mut rand = Xoroshiro128::new(seed as i64);
            grow_grass(&table, &mut level, 8, 62, 8, &mut rand);
            // Check the buffer for non-air placements.
            for gx in 6..=10 {
                for gz in 6..=10 {
                    let b = level.get_block_if_cached_or_loaded(gx, 64, gz);
                    if b != table.air {
                        placed_any = true;
                    }
                }
            }
            if placed_any {
                break;
            }
        }
        assert!(placed_any, "5×5 草地 + 16 种子应至少放置一株植物");
    }

    #[test]
    fn grow_tree_dispatches_by_wood_type() {
        let table = Arc::new(TreeBlockTable::from_core_palette());
        let chunk = make_chunk();
        let mut level = BlockManager::with_chunk(&chunk);
        let mut rand = Xoroshiro128::new(23);
        // Air chunk passes canPlaceObject into placeObject.
        grow_tree(
            &mut level,
            8,
            64,
            8,
            &mut rand,
            WoodType::Spruce,
            false,
            &table,
        );
        if table.dirt != table.air {
            assert_eq!(level.get_block_if_cached_or_loaded(8, 63, 8), table.dirt);
            assert_eq!(
                level.get_block_if_cached_or_loaded(8, 64, 8),
                table.log_of(WoodType::Spruce)
            );
        }
    }
}
