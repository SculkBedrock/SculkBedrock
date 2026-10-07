//! Block and biome surface-material mappings needed at generation time
//! (generation subset of the block-state/biome registries).
//!
//! Layout notes:
//! - This table is built once at startup from
//!   `BlockStateDictionary::global().first_hash_of("minecraft:xxx")`;
//!   unknown blocks fall back to `minecraft:stone` + `warn!`.
//! - Surface data needs per-biome materials (top/mid/seaFloor/seaFloorDepth
//!   from the version-pack biome definitions). They land as a **hardcoded
//!   const table** giving defaults per biome category, convertible to
//!   data-driven later. Values follow the biome definitions plus the
//!   community surface-material reference.

use crate::worldgen::biome::biome_id::*;
use sc_block::block_json::BlockJsonSnapshot;
use sc_log::t_log;
use sc_world::block_dictionary::BlockStateDictionary;
use sc_world::chunk::BlockRuntimeId;

// ---------------------------------------------------------------------------
// WorldgenBlockTable (generation subset of the block-state registry)
// ---------------------------------------------------------------------------

/// All block runtime ids needed by the generation stages, built once at
/// startup from the core palette.
///
/// Unknown blocks fall back to `stone` + `warn!`.
#[derive(Clone, Debug)]
pub struct WorldgenBlockTable {
    pub air: BlockRuntimeId,
    pub stone: BlockRuntimeId,
    pub deepslate: BlockRuntimeId,
    pub bedrock: BlockRuntimeId,
    pub water: BlockRuntimeId,
    pub flowing_water: BlockRuntimeId,
    pub dirt: BlockRuntimeId,
    pub coarse_dirt: BlockRuntimeId,
    pub grass_block: BlockRuntimeId,
    pub sand: BlockRuntimeId,
    pub sandstone: BlockRuntimeId,
    pub gravel: BlockRuntimeId,
    pub hardened_clay: BlockRuntimeId,
    pub orange_terracotta: BlockRuntimeId,
    pub white_terracotta: BlockRuntimeId,
    pub yellow_terracotta: BlockRuntimeId,
    pub brown_terracotta: BlockRuntimeId,
    pub red_terracotta: BlockRuntimeId,
    pub light_gray_terracotta: BlockRuntimeId,
    pub snow_layer: BlockRuntimeId,
    pub snow_block: BlockRuntimeId,
    pub ice: BlockRuntimeId,
    pub packed_ice: BlockRuntimeId,
    pub mycelium: BlockRuntimeId,
    pub podzol: BlockRuntimeId,
    pub red_sand: BlockRuntimeId,
    pub red_sandstone: BlockRuntimeId,
    pub terracotta: BlockRuntimeId,
}

impl WorldgenBlockTable {
    /// Builds from the core palette. Unknown blocks fall back to `stone` + `warn!`.
    pub fn from_core_palette() -> Self {
        Self::build(None)
    }

    /// Builds from a block bundle snapshot: defaults come from the
    /// authoritative declarations rather than palette-order first-seen
    /// hashes; unknown blocks still fall back to `stone` + `warn!`.
    pub fn from_block_snapshot(snapshot: &BlockJsonSnapshot) -> Self {
        Self::build(Some(snapshot))
    }

    fn build(snapshot: Option<&BlockJsonSnapshot>) -> Self {
        let air = BlockRuntimeId(sc_world::block_dictionary::air_runtime_id());
        let stone = lookup_snap(snapshot, "minecraft:stone", air);
        let lookup_or_stone = |name: &str| lookup_snap(snapshot, name, stone);

        Self {
            air,
            stone,
            deepslate: lookup_or_stone("minecraft:deepslate"),
            bedrock: lookup_or_stone("minecraft:bedrock"),
            water: lookup_or_stone("minecraft:water"),
            flowing_water: lookup_or_stone("minecraft:flowing_water"),
            dirt: lookup_or_stone("minecraft:dirt"),
            coarse_dirt: lookup_or_stone("minecraft:coarse_dirt"),
            grass_block: lookup_or_stone("minecraft:grass_block"),
            sand: lookup_or_stone("minecraft:sand"),
            sandstone: lookup_or_stone("minecraft:sandstone"),
            gravel: lookup_or_stone("minecraft:gravel"),
            hardened_clay: lookup_or_stone("minecraft:hardened_clay"),
            orange_terracotta: lookup_or_stone("minecraft:orange_terracotta"),
            white_terracotta: lookup_or_stone("minecraft:white_terracotta"),
            yellow_terracotta: lookup_or_stone("minecraft:yellow_terracotta"),
            brown_terracotta: lookup_or_stone("minecraft:brown_terracotta"),
            red_terracotta: lookup_or_stone("minecraft:red_terracotta"),
            light_gray_terracotta: lookup_or_stone("minecraft:light_gray_terracotta"),
            snow_layer: lookup_or_stone("minecraft:snow_layer"),
            snow_block: lookup_or_stone("minecraft:snow_block"),
            ice: lookup_or_stone("minecraft:ice"),
            packed_ice: lookup_or_stone("minecraft:packed_ice"),
            mycelium: lookup_or_stone("minecraft:mycelium"),
            podzol: lookup_or_stone("minecraft:podzol"),
            red_sand: lookup_or_stone("minecraft:red_sand"),
            red_sandstone: lookup_or_stone("minecraft:red_sandstone"),
            terracotta: lookup_or_stone("minecraft:terracotta"),
        }
    }

    /// Returns whether a runtime id is water (still or flowing).
    pub fn is_water(&self, id: BlockRuntimeId) -> bool {
        id == self.water || id == self.flowing_water
    }
}

// ---------------------------------------------------------------------------
// OreBlockTable (block ids needed by ore features)
// ---------------------------------------------------------------------------

/// All block runtime ids needed by ore features (stone variants +
/// deepslate variants). Unknown blocks fall back to `stone` + `warn!`.
#[derive(Clone, Debug)]
pub struct OreBlockTable {
    // Base blocks (for replaceability checks)
    pub stone: BlockRuntimeId,
    pub deepslate: BlockRuntimeId,
    // Ore stone variants
    pub coal_ore: BlockRuntimeId,
    pub iron_ore: BlockRuntimeId,
    pub copper_ore: BlockRuntimeId,
    pub gold_ore: BlockRuntimeId,
    pub redstone_ore: BlockRuntimeId,
    pub diamond_ore: BlockRuntimeId,
    pub lapis_ore: BlockRuntimeId,
    pub emerald_ore: BlockRuntimeId,
    // Ore deepslate variants
    pub deepslate_coal_ore: BlockRuntimeId,
    pub deepslate_iron_ore: BlockRuntimeId,
    pub deepslate_copper_ore: BlockRuntimeId,
    pub deepslate_gold_ore: BlockRuntimeId,
    pub deepslate_redstone_ore: BlockRuntimeId,
    pub deepslate_diamond_ore: BlockRuntimeId,
    pub deepslate_lapis_ore: BlockRuntimeId,
    pub deepslate_emerald_ore: BlockRuntimeId,
    // Non-ore replacement blocks (dirt/gravel/andesite/diorite/granite/tuff)
    pub dirt: BlockRuntimeId,
    pub gravel: BlockRuntimeId,
    pub andesite: BlockRuntimeId,
    pub diorite: BlockRuntimeId,
    pub granite: BlockRuntimeId,
    pub tuff: BlockRuntimeId,
    // Special ores (infested stone/deepslate)
    pub infested_stone: BlockRuntimeId,
    pub infested_deepslate: BlockRuntimeId,
}

impl OreBlockTable {
    /// Builds from the core palette. Unknown blocks fall back to `stone` + `warn!`.
    pub fn from_core_palette() -> Self {
        Self::build(None)
    }

    /// Builds from a block bundle snapshot (authoritative defaults first, same semantics).
    pub fn from_block_snapshot(snapshot: &BlockJsonSnapshot) -> Self {
        Self::build(Some(snapshot))
    }

    fn build(snapshot: Option<&BlockJsonSnapshot>) -> Self {
        let stone = BlockRuntimeId(lookup_snap(snapshot, "minecraft:stone", BlockRuntimeId(0)).0);
        let lookup_or_stone = |name: &str| lookup_snap(snapshot, name, stone);

        Self {
            stone,
            deepslate: lookup_or_stone("minecraft:deepslate"),
            coal_ore: lookup_or_stone("minecraft:coal_ore"),
            iron_ore: lookup_or_stone("minecraft:iron_ore"),
            copper_ore: lookup_or_stone("minecraft:copper_ore"),
            gold_ore: lookup_or_stone("minecraft:gold_ore"),
            redstone_ore: lookup_or_stone("minecraft:redstone_ore"),
            diamond_ore: lookup_or_stone("minecraft:diamond_ore"),
            lapis_ore: lookup_or_stone("minecraft:lapis_ore"),
            emerald_ore: lookup_or_stone("minecraft:emerald_ore"),
            deepslate_coal_ore: lookup_or_stone("minecraft:deepslate_coal_ore"),
            deepslate_iron_ore: lookup_or_stone("minecraft:deepslate_iron_ore"),
            deepslate_copper_ore: lookup_or_stone("minecraft:deepslate_copper_ore"),
            deepslate_gold_ore: lookup_or_stone("minecraft:deepslate_gold_ore"),
            deepslate_redstone_ore: lookup_or_stone("minecraft:deepslate_redstone_ore"),
            deepslate_diamond_ore: lookup_or_stone("minecraft:deepslate_diamond_ore"),
            deepslate_lapis_ore: lookup_or_stone("minecraft:deepslate_lapis_ore"),
            deepslate_emerald_ore: lookup_or_stone("minecraft:deepslate_emerald_ore"),
            dirt: lookup_or_stone("minecraft:dirt"),
            gravel: lookup_or_stone("minecraft:gravel"),
            andesite: lookup_or_stone("minecraft:andesite"),
            diorite: lookup_or_stone("minecraft:diorite"),
            granite: lookup_or_stone("minecraft:granite"),
            tuff: lookup_or_stone("minecraft:tuff"),
            infested_stone: lookup_or_stone("minecraft:infested_stone"),
            infested_deepslate: lookup_or_stone("minecraft:infested_deepslate"),
        }
    }
}

/// Snapshot-first default-state resolution: bundle authoritative default →
/// alias → legacy first-seen-hash fallback.
///
/// Shared entry point for all worldgen block tables; falls back to the
/// legacy behavior when no snapshot is present.
pub fn resolve_block_default(snapshot: Option<&BlockJsonSnapshot>, name: &str) -> Option<u32> {
    if let Some(snap) = snapshot {
        if let Some(hash) = snap.default_hash(name) {
            return Some(hash);
        }
        if let Some(alias) = bedrock_alias(name) {
            if let Some(hash) = snap.default_hash(alias) {
                return Some(hash);
            }
        }
    }
    first_hash_of_aliased(BlockStateDictionary::global(), name)
}

fn lookup_snap(
    snapshot: Option<&BlockJsonSnapshot>,
    name: &str,
    fallback: BlockRuntimeId,
) -> BlockRuntimeId {
    match resolve_block_default(snapshot, name) {
        Some(hash) => BlockRuntimeId(hash),
        None => {
            log::warn!("{}", t_log!("console.worldgen.palette_missing", name = name, fallback = format!("{fallback:?}")));
            fallback
        }
    }
}

/// Block-name alias table for palette naming differences.
///
/// The bundled palette uses Bedrock names while worldgen uses Java-edition
/// names. Known differences:
/// - `dead_bush` → `deadbush` (legacy Bedrock name without underscore)
/// - `snow_block` → `snow` (Bedrock snow-block name)
/// - `terracotta` → `hardened_clay` (Bedrock terracotta name)
pub fn bedrock_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "minecraft:dead_bush" => "minecraft:deadbush",
        "minecraft:snow_block" => "minecraft:snow",
        "minecraft:terracotta" => "minecraft:hardened_clay",
        _ => return None,
    })
}

/// `first_hash_of` with Bedrock-alias fallback: shared entry point for all
/// worldgen block tables.
pub fn first_hash_of_aliased(dictionary: &BlockStateDictionary, name: &str) -> Option<u32> {
    dictionary
        .first_hash_of(name)
        .or_else(|| bedrock_alias(name).and_then(|alias| dictionary.first_hash_of(alias)))
}

// ---------------------------------------------------------------------------
// BiomeSurfaceMaterial (generation subset of biome surface-material data)
// ---------------------------------------------------------------------------

/// Per-biome surface material (top/mid/seaFloor/seaFloorDepth), stored as
/// [`BlockRuntimeId`]s.
#[derive(Clone, Copy, Debug)]
pub struct BiomeSurfaceMaterial {
    pub top: BlockRuntimeId,
    pub mid: BlockRuntimeId,
    pub sea_floor: BlockRuntimeId,
    pub sea_floor_depth: i32,
}

impl BiomeSurfaceMaterial {
    const fn new(
        top: BlockRuntimeId,
        mid: BlockRuntimeId,
        sea_floor: BlockRuntimeId,
        sea_floor_depth: i32,
    ) -> Self {
        Self {
            top,
            mid,
            sea_floor,
            sea_floor_depth,
        }
    }
}

/// Looks up the surface material for a biome id.
///
/// Hardcoded table grouped by biome category; most overworld biomes share
/// grass/dirt/dirt. Unlisted biomes fall back to the default
/// (grass/dirt/dirt/0).
pub fn biome_surface_material(biome_id: i32, table: &WorldgenBlockTable) -> BiomeSurfaceMaterial {
    let grass = table.grass_block;
    let dirt = table.dirt;
    let sand = table.sand;
    let sandstone = table.sandstone;
    let gravel = table.gravel;
    let stone = table.stone;
    let snow = table.snow_block;
    let mycelium = table.mycelium;
    let podzol = table.podzol;
    let red_sand = table.red_sand;

    // Default: grass/dirt/dirt
    let default = BiomeSurfaceMaterial::new(grass, dirt, dirt, 0);

    match biome_id {
        // --- Deserts ---
        DESERT | DESERT_HILLS | DESERT_MUTATED => {
            BiomeSurfaceMaterial::new(sand, sand, sandstone, 0)
        }

        // --- Oceans (sandy sea floor over dirt/gravel) ---
        OCEAN | DEEP_OCEAN | WARM_OCEAN | DEEP_WARM_OCEAN | LUKEWARM_OCEAN
        | DEEP_LUKEWARM_OCEAN | COLD_OCEAN | DEEP_COLD_OCEAN => {
            BiomeSurfaceMaterial::new(sand, dirt, gravel, 0)
        }
        FROZEN_OCEAN | DEEP_FROZEN_OCEAN | LEGACY_FROZEN_OCEAN => {
            BiomeSurfaceMaterial::new(sand, dirt, gravel, 0)
        }

        // --- Beaches ---
        BEACH | COLD_BEACH => BiomeSurfaceMaterial::new(sand, sand, sandstone, 0),
        STONE_BEACH => BiomeSurfaceMaterial::new(stone, stone, gravel, 0),
        MUSHROOM_ISLAND | MUSHROOM_ISLAND_SHORE => {
            BiomeSurfaceMaterial::new(mycelium, dirt, dirt, 0)
        }

        // --- Snowy biomes ---
        ICE_PLAINS | ICE_PLAINS_SPIKES | ICE_MOUNTAINS | FROZEN_RIVER | COLD_TAIGA
        | COLD_TAIGA_HILLS | COLD_TAIGA_MUTATED | SNOWY_SLOPES | GROVE => {
            BiomeSurfaceMaterial::new(snow, dirt, dirt, 0)
        }

        // --- Mountains (stone top) ---
        EXTREME_HILLS
        | EXTREME_HILLS_EDGE
        | EXTREME_HILLS_MUTATED
        | EXTREME_HILLS_PLUS_TREES
        | EXTREME_HILLS_PLUS_TREES_MUTATED
        | JAGGED_PEAKS
        | FROZEN_PEAKS
        | STONY_PEAKS => BiomeSurfaceMaterial::new(stone, dirt, dirt, 0),

        // --- Badlands (overwritten by SurfaceOverwriteStage; base values here) ---
        MESA
        | MESA_BRYCE
        | MESA_PLATEAU
        | MESA_PLATEAU_STONE
        | MESA_PLATEAU_STONE_MUTATED
        | MESA_PLATEAU_MUTATED => BiomeSurfaceMaterial::new(red_sand, table.terracotta, stone, 0),

        // --- Giant tree taiga (podzol) ---
        MEGA_TAIGA | MEGA_TAIGA_HILLS | REDWOOD_TAIGA_MUTATED | REDWOOD_TAIGA_HILLS_MUTATED => {
            BiomeSurfaceMaterial::new(podzol, dirt, dirt, 0)
        }

        // --- Everything else (plains/forest/jungle/savanna/swamp/river/...) → grass default ---
        _ => default,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_table_builds_from_core_palette() {
        // The core palette may not have a version pack loaded, but
        // `air_runtime_id` is always available.
        let table = WorldgenBlockTable::from_core_palette();
        // Not panicking is enough (missing blocks fall back to stone, and
        // a missing stone falls back to air).
        let _ = table.deepslate;
        let _ = table.bedrock;
    }

    /// Snapshot-first: bundle defaults become the worldgen block-table source
    /// (independent of first-seen order).
    #[test]
    fn block_table_prefers_snapshot_defaults() {
        use sc_block::block_json::compile_bundle;
        use sc_packloader::block::{
            fingerprint_bundle, parse_block_file, BlockBundleBudgets, BlockJsonBundle,
        };
        let budgets = BlockBundleBudgets::default();
        let air_json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        let stone_json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:stone", "states": {}},
                "components": {},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let raws = [
            (
                "definitions/blocks/minecraft/air.block.json",
                air_json.as_bytes().to_vec(),
            ),
            (
                "definitions/blocks/minecraft/stone.block.json",
                stone_json.as_bytes().to_vec(),
            ),
        ];
        let mut refs: Vec<(&str, &[u8])> = raws.iter().map(|(p, b)| (*p, b.as_slice())).collect();
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
        let snapshot =
            compile_bundle(&bundle, "test", &budgets, &|_| None, &|_| true).expect("测试 bundle 应编译").0;
        // Snapshot default-state hash = canonical algorithm hash (not first-seen order).
        let expected = sc_world::leveldb::block_hash::block_state_hash("minecraft:stone", None);
        assert_eq!(snapshot.default_hash("minecraft:stone"), Some(expected));
        let table = WorldgenBlockTable::from_block_snapshot(&snapshot);
        assert_eq!(table.stone, sc_world::chunk::BlockRuntimeId(expected));
        assert_eq!(
            resolve_block_default(Some(&snapshot), "minecraft:stone"),
            Some(expected)
        );
        // No snapshot declaration → legacy fallback (None when the global
        // dictionary has no stone; never panics).
        let _ = resolve_block_default(None, "minecraft:definitely_not_a_block");
    }

    #[test]
    fn is_water_detects_both_water_variants() {
        let table = WorldgenBlockTable::from_core_palette();
        // Only assert strictly when a version pack is loaded (water differs from stone).
        if table.water != table.stone {
            assert!(table.is_water(table.water));
            assert!(table.is_water(table.flowing_water));
            assert!(!table.is_water(table.stone));
        }
    }

    #[test]
    fn biome_surface_material_default_is_grass_dirt() {
        let table = WorldgenBlockTable::from_core_palette();
        let m = biome_surface_material(PLAINS, &table);
        assert_eq!(m.top, table.grass_block);
        assert_eq!(m.mid, table.dirt);
    }

    #[test]
    fn biome_surface_material_desert_is_sand() {
        let table = WorldgenBlockTable::from_core_palette();
        let m = biome_surface_material(DESERT, &table);
        assert_eq!(m.top, table.sand);
        assert_eq!(m.sea_floor, table.sandstone);
    }

    #[test]
    fn biome_surface_material_ocean_has_gravel_floor() {
        let table = WorldgenBlockTable::from_core_palette();
        let m = biome_surface_material(OCEAN, &table);
        assert_eq!(m.sea_floor, table.gravel);
    }

    #[test]
    fn biome_surface_material_unknown_biome_falls_back() {
        let table = WorldgenBlockTable::from_core_palette();
        let m = biome_surface_material(9999, &table);
        assert_eq!(m.top, table.grass_block);
    }
}
