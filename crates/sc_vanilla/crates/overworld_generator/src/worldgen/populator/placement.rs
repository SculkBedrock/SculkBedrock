//! Structure placement grid: sparse region sampling for structure origins.
//!
//! Notes:
//! - [`PlacementSettings`] carries field defaults via its `Default` impl.
//! - The biome predicate is a boxed `Fn(i32) -> bool`.
//! - Callers sample the biome themselves, then call
//!   [`StructurePlacement::can_generate`].
//! - Nearest-generation lookup (locate command) is not covered.

use crate::worldgen::random::RandomSourceProvider;
use crate::worldgen::stages::chunk_hash;

// ---------------------------------------------------------------------------
// PlacementSettings: spacing/salt/biome predicate for one structure kind
// ---------------------------------------------------------------------------

/// Spacing/salt/biome predicate for one structure kind.
pub struct PlacementSettings {
    /// Salt mixed into the region RNG (default `0x76694565C616765`).
    pub salt: i64,
    /// Minimum chunk offset within a region (default 0).
    pub min_distance: i32,
    /// Region spacing in chunks (default 1).
    pub max_distance: i32,
    /// Biome sample height (defaults to sea level).
    pub biome_sample_y: i32,
    /// Biome validity predicate (default accepts every biome).
    pub is_biome_valid: Box<dyn Fn(i32) -> bool + Send + Sync>,
}

impl Default for PlacementSettings {
    fn default() -> Self {
        Self {
            salt: 0x7669_4565_C616_765,
            min_distance: 0,
            max_distance: 1,
            biome_sample_y: crate::worldgen::stages::terrain::SEA_LEVEL,
            is_biome_valid: Box::new(|_| true),
        }
    }
}

// ---------------------------------------------------------------------------
// StructurePlacement: region-grid origin check
// ---------------------------------------------------------------------------

/// Region-grid origin check: sparsely decides whether a chunk is a structure origin.
pub struct StructurePlacement {
    /// Placement settings.
    pub settings: PlacementSettings,
}

impl StructurePlacement {
    /// Builds a placement from settings.
    pub fn new(settings: PlacementSettings) -> Self {
        Self { settings }
    }

    /// Returns whether the chunk is the selected origin of its region.
    ///
    /// Each region derives its RNG from `(level_seed ^ salt) + region_hash`,
    /// then picks one chunk offset inside the region span.
    pub fn can_generate(
        &self,
        level_seed: i64,
        random: &mut dyn RandomSourceProvider,
        chunk_x: i32,
        chunk_z: i32,
        biome: i32,
    ) -> bool {
        let max_distance = self.settings.max_distance;
        let min_distance = self.settings.min_distance;
        let region_x = region_coord(chunk_x, max_distance);
        let region_z = region_coord(chunk_z, max_distance);
        // Region RNG seeded from the salted level seed plus the region hash.
        random.set_seed(
            (level_seed ^ self.settings.salt).wrapping_add(chunk_hash(region_x, region_z)),
        );
        self.is_valid_biome(biome)
            && region_x
                .wrapping_mul(max_distance)
                .wrapping_add(random.next_bounded_int(max_distance - min_distance))
                == chunk_x
            && region_z
                .wrapping_mul(max_distance)
                .wrapping_add(random.next_bounded_int(max_distance - min_distance))
                == chunk_z
    }

    /// Returns whether the biome passes the placement predicate.
    pub fn is_valid_biome(&self, biome: i32) -> bool {
        (self.settings.is_biome_valid)(biome)
    }
}

/// Maps a chunk coordinate to its region coordinate.
///
/// `chunk < 0 ? (chunk - spacing - 1) / spacing : chunk / spacing`
/// (integer division truncates toward zero, matching `/` here).
pub fn region_coord(chunk: i32, spacing: i32) -> i32 {
    if chunk < 0 {
        (chunk - spacing - 1) / spacing
    } else {
        chunk / spacing
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::random::Xoroshiro128;

    #[test]
    fn region_coord_matches_java() {
        // Non-negative inputs divide directly.
        assert_eq!(region_coord(0, 32), 0);
        assert_eq!(region_coord(31, 32), 0);
        assert_eq!(region_coord(32, 32), 1);
        // Negative inputs offset before truncating division (-65/32 = -2,
        // not floor -3 or ceil -1).
        assert_eq!(region_coord(-1, 32), -1);
        assert_eq!(region_coord(-32, 32), -2);
        assert_eq!(region_coord(-33, 32), -2);
        assert_eq!(region_coord(-64, 32), -3);
    }

    #[test]
    fn can_generate_spread_is_deterministic() {
        // One chunk offset per 4x4 region from the region seed. Offsets may land
        // in a neighboring region, so each region hits at most one chunk, deterministically.
        let placement = StructurePlacement::new(PlacementSettings {
            salt: 14357620,
            min_distance: 0,
            max_distance: 4,
            ..Default::default()
        });
        let seed = 12345i64;
        let mut total_hits = 0;
        for rx in 0..4 {
            for rz in 0..4 {
                let mut region_hits = 0;
                for cx in rx * 4..rx * 4 + 4 {
                    for cz in rz * 4..rz * 4 + 4 {
                        let mut random = Xoroshiro128::new(0);
                        let first = placement.can_generate(seed, &mut random, cx, cz, 0);
                        // Determinism: rerunning with the same inputs matches.
                        let mut random2 = Xoroshiro128::new(0);
                        assert_eq!(
                            first,
                            placement.can_generate(seed, &mut random2, cx, cz, 0),
                            "can_generate must be deterministic"
                        );
                        if first {
                            region_hits += 1;
                        }
                    }
                }
                assert!(
                    region_hits <= 1,
                    "each region should match at most one chunk, got {region_hits}"
                );
                total_hits += region_hits;
            }
        }
        assert!(total_hits > 0, "some chunks should be selected");
    }

    #[test]
    fn can_generate_respects_biome_filter() {
        // Zero spread pins the offset to 0, so the position check always passes
        // and only the biome filter decides.
        let placement = StructurePlacement::new(PlacementSettings {
            min_distance: 1,
            max_distance: 1,
            is_biome_valid: Box::new(|b| b == 6),
            ..Default::default()
        });
        let mut random = Xoroshiro128::new(0);
        assert!(placement.can_generate(42, &mut random, 3, 7, 6));
        let mut random = Xoroshiro128::new(0);
        assert!(!placement.can_generate(42, &mut random, 3, 7, 5));
    }
}
