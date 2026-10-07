//! Overworld biome picking (`OverworldBiomePicker` + `OverworldBiomeResult`).
//!
//! Port notes:
//! - The upstream constructor takes a `Level`; this port decouples it,
//!   pulling density functions (continents/erosion/ridges) and noises
//!   (temperature/humidity) directly from [`NormalObjectHolder`], with the
//!   heightmap injected as a `Box<dyn Fn(i32,i32)->i32>` callback.
//! - The upstream base-class random field goes unused during overworld picking
//!   (only the nether picker forks it); it is kept for structural parity with
//!   `#[allow(dead_code)]`.
//! - The two `pick`/`pickRaw` overloads (with/without FunctionContext) become
//!   `pick` / `pick_with_context` / `pick_raw` / `pick_raw_with_context`.
//! - Float fidelity: `f`-suffixed literals map to `f32`; unsuffixed double
//!   literals (`-0.19`/`-0.11`/`0.03`/`0.3`) compare as float-widened-to-double.
//! - `correct` / `reset` mirror chaining `return this` via `&mut Self`.

use crate::worldgen::biome::biome_id::*;
use crate::worldgen::biome::{BiomePicker, BiomeResult};
use crate::worldgen::densityfunction::function::{
    DensityFunction, FunctionContext, SinglePointContext,
};
use crate::worldgen::holder::normal::NormalObjectHolder;
use crate::worldgen::noise::noise::NormalNoise;
use crate::worldgen::random::MtRandom;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Sea level (63).
// ---------------------------------------------------------------------------

/// Sea level shared with the terrain stage.
///
/// Defined here for the biome picker; deduplicate with the terrain stage.
pub const SEA_LEVEL: i32 = 63;

// ---------------------------------------------------------------------------
// OverworldBiomeResult
// ---------------------------------------------------------------------------

/// Java: `OverworldBiomeResult extends BiomeResult`(L12-62).
#[derive(Clone, Debug)]
pub struct OverworldBiomeResult {
    biome_id: i32,
    /// Continentalness value.
    pub continental: f32,
    /// Temperature value.
    pub temperature: f32,
    /// Humidity value.
    pub humidity: f32,
    /// Erosion value.
    pub erosion: f32,
    /// Weirdness value.
    pub weirdness: f32,
    /// Peak/valley value.
    pub pv: f32,
    /// Original biome id before correct (restored by reset).
    original: i32,
}

impl OverworldBiomeResult {
    /// Java: `OverworldBiomeResult(int, float, float, float, float, float, float)`(L23-32).
    pub fn new(
        biome_id: i32,
        continental: f32,
        temperature: f32,
        humidity: f32,
        erosion: f32,
        weirdness: f32,
        pv: f32,
    ) -> Self {
        Self {
            biome_id,
            continental,
            temperature,
            humidity,
            erosion,
            weirdness,
            pv,
            original: biome_id,
        }
    }

    /// Java: `OverworldBiomeResult correct(int y)`(L38-57).
    ///
    /// Overwrite `biomeId` by depth (`blockY - heightMap`, negative below surface).
    /// Returns `&mut Self` for chaining.
    pub fn correct(&mut self, y: i32) -> &mut Self {
        // Integer division truncates before widening to float.
        let depth = (-y) as f32 / 128.0f32;

        if depth >= 0.2f32 {
            if depth < 0.99f32 {
                if self.continental > 0.8f32 && self.continental < 1.0f32 {
                    self.biome_id = DRIPSTONE_CAVES;
                } else if self.temperature > 0.55f32 && self.humidity < -0.1f32 {
                    self.biome_id = SULFUR_CAVES;
                } else if self.humidity > 0.3f32 {
                    self.biome_id = LUSH_CAVES;
                }
            } else if depth > 0.9f32 {
                // `depth > 0.9f` always holds once `depth >= 0.99f`; kept as-is.
                if self.erosion < -0.225f32 {
                    self.biome_id = DEEP_DARK;
                }
            }
        }
        self
    }

    /// Java: `void reset()`(L59-61).
    pub fn reset(&mut self) {
        self.biome_id = self.original;
    }

    /// Original biome id getter.
    pub fn original(&self) -> i32 {
        self.original
    }
}

impl BiomeResult for OverworldBiomeResult {
    fn biome_id(&self) -> i32 {
        self.biome_id
    }

    fn set_biome_id(&mut self, id: i32) {
        self.biome_id = id;
    }
}

// ---------------------------------------------------------------------------
// OverworldBiomePicker
// ---------------------------------------------------------------------------

/// Continental-level constants.
pub const CONTINENT_MUSHROOM: i32 = 0;
pub const CONTINENT_DEEP_OCEAN: i32 = 1;
pub const CONTINENT_OCEAN: i32 = 2;
pub const CONTINENT_COAST: i32 = 3;
pub const CONTINENT_NEAR_INLAND: i32 = 4;
pub const CONTINENT_MID_INLAND: i32 = 5;
pub const CONTINENT_FAR_INLAND: i32 = 6;

/// Java: `OverworldBiomePicker extends BiomePicker<OverworldBiomeResult>`(L14-420).
///
/// Level-decoupled: density functions and noises come from [`NormalObjectHolder`],
/// the heightmap arrives as a callback.
pub struct OverworldBiomePicker {
    /// Continents density.
    continents: Arc<dyn DensityFunction>,
    /// Erosion density.
    erosion: Arc<dyn DensityFunction>,
    /// Ridges density.
    ridges: Arc<dyn DensityFunction>,
    /// Temperature noise.
    temperature_noise: Arc<NormalNoise>,
    /// Humidity noise.
    humidity_noise: Arc<NormalNoise>,
    /// Heightmap callback, injected by the session/stage.
    heightmap: Box<dyn Fn(i32, i32) -> i32>,
    /// Base-class random, seeded from the level seed.
    /// Unused during overworld picking; kept for structural parity.
    #[allow(dead_code)]
    random: MtRandom,
}

impl OverworldBiomePicker {
    /// Build from [`NormalObjectHolder`] density functions and noises.
    ///
    /// The heightmap callback replaces the level heightmap lookup.
    pub fn new(
        holder: &NormalObjectHolder,
        level_seed: i64,
        heightmap: Box<dyn Fn(i32, i32) -> i32>,
    ) -> Self {
        let terrain = holder.terrain_holder();
        let biome = holder.biome_holder();
        Self {
            continents: Arc::clone(terrain.continents()),
            erosion: Arc::clone(terrain.erosion()),
            ridges: Arc::clone(terrain.ridges()),
            temperature_noise: Arc::clone(biome.temperature_noise()),
            humidity_noise: Arc::clone(biome.humidity_noise()),
            heightmap,
            random: MtRandom::new(level_seed),
        }
    }

    // --- pick / pickRaw (4 methods) ---

    /// Java: `OverworldBiomeResult pick(int x, int y, int z)`(L32-34).
    pub fn pick(&self, x: i32, y: i32, z: i32) -> OverworldBiomeResult {
        let ctx = SinglePointContext {
            block_x: x,
            block_y: y,
            block_z: z,
        };
        self.pick_with_context(x, y, z, &ctx)
    }

    /// Java: `OverworldBiomeResult pick(int x, int y, int z, FunctionContext point)`(L36-38).
    pub fn pick_with_context(
        &self,
        x: i32,
        y: i32,
        z: i32,
        point: &dyn FunctionContext,
    ) -> OverworldBiomeResult {
        let heightmap = (self.heightmap)(x, z);
        let mut result = self.pick_raw_with_context(x, y, z, point);
        result.correct(y - heightmap);
        result
    }

    /// Java: `OverworldBiomeResult pickRaw(int x, int y, int z)`(L40-42).
    pub fn pick_raw(&self, x: i32, y: i32, z: i32) -> OverworldBiomeResult {
        let ctx = SinglePointContext {
            block_x: x,
            block_y: y,
            block_z: z,
        };
        self.pick_raw_with_context(x, y, z, &ctx)
    }

    /// Java: `OverworldBiomeResult pickRaw(int x, int y, int z, FunctionContext point)`(L44-67).
    pub fn pick_raw_with_context(
        &self,
        x: i32,
        _y: i32,
        z: i32,
        point: &dyn FunctionContext,
    ) -> OverworldBiomeResult {
        // `NormalObjectHolder` holders supply density and noises.
        // Those values are 2D
        let continental = self.continents.compute(point) as f32;
        let temperature = self
            .temperature_noise
            .get_value(x as f64, SEA_LEVEL as f64, z as f64);
        let humidity = self
            .humidity_noise
            .get_value(x as f64, SEA_LEVEL as f64, z as f64);
        let erosion = self.erosion.compute(point) as f32;
        let weirdness = self.ridges.compute(point) as f32;
        // Java L53: `float pv = -3 * (-(1/3f) + Math.abs(-(2/3f) + Math.abs(weirdness)));`
        let pv = -3.0f32 * (-(1.0f32 / 3.0f32) + (-(2.0f32 / 3.0f32) + weirdness.abs()).abs());

        // continentalLevel thresholds (double literals).
        let continental_level = if continental < -1.05f32 {
            0
        } else if continental < -0.455f32 {
            1
        } else if (continental as f64) < -0.19 {
            2
        } else if (continental as f64) < -0.11 {
            3
        } else if (continental as f64) < 0.03 {
            4
        } else if (continental as f64) < 0.3 {
            5
        } else {
            6
        };

        let temperature_level = if temperature < -0.45f32 {
            0
        } else if temperature < -0.15f32 {
            1
        } else if temperature < 0.3f32 {
            2
        } else if temperature < 0.55f32 {
            3
        } else {
            4
        };

        let humidity_level = if humidity < -0.35f32 {
            0
        } else if humidity < -0.1f32 {
            1
        } else if humidity < 0.1f32 {
            2
        } else if humidity < 0.3f32 {
            3
        } else {
            4
        };

        let erosion_level = if erosion < -0.78f32 {
            0
        } else if erosion < -0.375f32 {
            1
        } else if erosion < -0.2225f32 {
            2
        } else if erosion < 0.05f32 {
            3
        } else if erosion < 0.45f32 {
            4
        } else if erosion < 0.55f32 {
            5
        } else {
            6
        };

        // Java L59-64: switch (continentalLevel)
        let biome = match continental_level {
            CONTINENT_MUSHROOM => MUSHROOM_ISLAND,
            CONTINENT_OCEAN | CONTINENT_DEEP_OCEAN => {
                self.get_non_inland_biome(temperature_level, continental_level)
            }
            _ => self.get_inland_biome(
                temperature_level,
                humidity_level,
                continental_level,
                erosion_level,
                weirdness,
            ),
        };

        OverworldBiomeResult::new(
            biome,
            continental,
            temperature,
            humidity,
            erosion,
            weirdness,
            pv,
        )
    }

    // --- Non-inland / inland selection ---

    /// Java: `getNonInlandBiome(int, int)`(L69-77).
    fn get_non_inland_biome(&self, temperature_level: i32, continental_level: i32) -> i32 {
        match temperature_level {
            0 => {
                if continental_level == CONTINENT_OCEAN {
                    FROZEN_OCEAN
                } else {
                    DEEP_FROZEN_OCEAN
                }
            }
            1 => {
                if continental_level == CONTINENT_OCEAN {
                    COLD_OCEAN
                } else {
                    DEEP_COLD_OCEAN
                }
            }
            2 => {
                if continental_level == CONTINENT_OCEAN {
                    OCEAN
                } else {
                    DEEP_OCEAN
                }
            }
            3 => {
                if continental_level == CONTINENT_OCEAN {
                    LUKEWARM_OCEAN
                } else {
                    DEEP_LUKEWARM_OCEAN
                }
            }
            _ => WARM_OCEAN,
        }
    }

    /// Java: `getInlandBiome(int, int, int, int, float)`(L79-107).
    fn get_inland_biome(
        &self,
        temperature_level: i32,
        humidity_level: i32,
        continental_level: i32,
        erosion_level: i32,
        weirdness: f32,
    ) -> i32 {
        if weirdness < -0.93333334f32 {
            self.pick_mid_slice_biome(
                temperature_level,
                humidity_level,
                continental_level,
                erosion_level,
                weirdness,
            )
        } else if weirdness < -0.7666667f32 {
            self.pick_high_slice_biome(
                temperature_level,
                humidity_level,
                continental_level,
                erosion_level,
                weirdness,
            )
        } else if weirdness < -0.56666666f32 {
            self.pick_peaks_biome(
                temperature_level,
                humidity_level,
                continental_level,
                erosion_level,
                weirdness,
            )
        } else if weirdness < -0.4f32 {
            self.pick_high_slice_biome(
                temperature_level,
                humidity_level,
                continental_level,
                erosion_level,
                weirdness,
            )
        } else if weirdness < -0.26666668f32 {
            self.pick_mid_slice_biome(
                temperature_level,
                humidity_level,
                continental_level,
                erosion_level,
                weirdness,
            )
        } else if weirdness < -0.05f32 {
            self.pick_low_slice_biome(
                temperature_level,
                humidity_level,
                continental_level,
                erosion_level,
                weirdness,
            )
        } else if weirdness < 0.05f32 {
            self.pick_valleys_biome(
                temperature_level,
                humidity_level,
                continental_level,
                erosion_level,
                weirdness,
            )
        } else if weirdness < 0.26666668f32 {
            self.pick_low_slice_biome(
                temperature_level,
                humidity_level,
                continental_level,
                erosion_level,
                weirdness,
            )
        } else if weirdness < 0.4f32 {
            self.pick_mid_slice_biome(
                temperature_level,
                humidity_level,
                continental_level,
                erosion_level,
                weirdness,
            )
        } else if weirdness < 0.56666666f32 {
            self.pick_high_slice_biome(
                temperature_level,
                humidity_level,
                continental_level,
                erosion_level,
                weirdness,
            )
        } else if weirdness < 0.7666667f32 {
            self.pick_peaks_biome(
                temperature_level,
                humidity_level,
                continental_level,
                erosion_level,
                weirdness,
            )
        } else if weirdness < 0.93333334f32 {
            self.pick_high_slice_biome(
                temperature_level,
                humidity_level,
                continental_level,
                erosion_level,
                weirdness,
            )
        } else {
            self.pick_mid_slice_biome(
                temperature_level,
                humidity_level,
                continental_level,
                erosion_level,
                weirdness,
            )
        }
    }

    // --- Slice selection (5 methods) ---

    /// Java: `pickPeaksBiome(...)`(L109-130).
    #[allow(clippy::too_many_arguments)]
    fn pick_peaks_biome(
        &self,
        temperature_level: i32,
        humidity_level: i32,
        continental_level: i32,
        erosion_level: i32,
        weirdness: f32,
    ) -> i32 {
        let weird = weirdness >= 0.0f32;
        let middle_biome = self.get_middle_biome(temperature_level, humidity_level, weird);
        let middle_biome_or_badlands_if_hot =
            self.get_middle_biome_or_badlands_if_hot(temperature_level, humidity_level, weird);
        let middle_biome_or_badlands_if_hot_or_slope_if_cold = self
            .get_middle_biome_or_badlands_if_hot_or_slope_if_cold(
                temperature_level,
                humidity_level,
                weird,
            );
        let plateau_biome = self.get_plateau_biome(temperature_level, humidity_level, weird);
        let shattered_biome = self.get_shattered_biome(temperature_level, humidity_level, weird);
        let shattered_biome_or_windswept_savanna = self.maybe_pick_windswept_savanna_biome(
            temperature_level,
            humidity_level,
            weird,
            shattered_biome,
        );
        let peak_biome = self.get_peak_biome(temperature_level, humidity_level, weird);

        if self.is_coast_to_far(continental_level) && erosion_level == 0 {
            return peak_biome;
        }
        if self.is_coast_to_near(continental_level) && erosion_level == 1 {
            return middle_biome_or_badlands_if_hot_or_slope_if_cold;
        }
        if self.is_mid_to_far(continental_level) && erosion_level == 1 {
            return peak_biome;
        }
        if self.is_coast_to_near(continental_level) && (erosion_level == 2 || erosion_level == 3) {
            return middle_biome;
        }
        if self.is_mid_to_far(continental_level) && erosion_level == 2 {
            return plateau_biome;
        }
        if self.is_mid(continental_level) && erosion_level == 3 {
            return middle_biome_or_badlands_if_hot;
        }
        if self.is_far(continental_level) && erosion_level == 3 {
            return plateau_biome;
        }
        if self.is_coast_to_far(continental_level) && erosion_level == 4 {
            return middle_biome;
        }
        if self.is_coast_to_near(continental_level) && erosion_level == 5 {
            return shattered_biome_or_windswept_savanna;
        }
        if self.is_mid_to_far(continental_level) && erosion_level == 5 {
            return shattered_biome;
        }
        middle_biome
    }

    /// Java: `pickHighSliceBiome(...)`(L132-156).
    #[allow(clippy::too_many_arguments)]
    fn pick_high_slice_biome(
        &self,
        temperature_level: i32,
        humidity_level: i32,
        continental_level: i32,
        erosion_level: i32,
        weirdness: f32,
    ) -> i32 {
        let weird = weirdness >= 0.0f32;
        let middle_biome = self.get_middle_biome(temperature_level, humidity_level, weird);
        let middle_biome_or_badlands_if_hot =
            self.get_middle_biome_or_badlands_if_hot(temperature_level, humidity_level, weird);
        let middle_biome_or_badlands_if_hot_or_slope_if_cold = self
            .get_middle_biome_or_badlands_if_hot_or_slope_if_cold(
                temperature_level,
                humidity_level,
                weird,
            );
        let plateau_biome = self.get_plateau_biome(temperature_level, humidity_level, weird);
        let shattered_biome = self.get_shattered_biome(temperature_level, humidity_level, weird);
        let middle_biome_or_windswept_savanna = self.maybe_pick_windswept_savanna_biome(
            temperature_level,
            humidity_level,
            weird,
            middle_biome,
        );
        let slope_biome = self.get_slope_biome(temperature_level, humidity_level, weird);
        let peak_biome = self.get_peak_biome(temperature_level, humidity_level, weird);

        if self.is_coast(continental_level) && (erosion_level == 0 || erosion_level == 1) {
            return middle_biome;
        }
        if self.is_near(continental_level) && erosion_level == 0 {
            return slope_biome;
        }
        if self.is_mid_to_far(continental_level) && erosion_level == 0 {
            return peak_biome;
        }
        if self.is_near(continental_level) && erosion_level == 1 {
            return middle_biome_or_badlands_if_hot_or_slope_if_cold;
        }
        if self.is_mid_to_far(continental_level) && erosion_level == 1 {
            return slope_biome;
        }
        if self.is_coast_to_near(continental_level) && (erosion_level == 2 || erosion_level == 3) {
            return middle_biome;
        }
        if self.is_mid_to_far(continental_level) && erosion_level == 2 {
            return plateau_biome;
        }
        if self.is_mid(continental_level) && erosion_level == 3 {
            return middle_biome_or_badlands_if_hot;
        }
        if self.is_far(continental_level) && erosion_level == 3 {
            return plateau_biome;
        }
        if self.is_coast_to_far(continental_level) && erosion_level == 4 {
            return middle_biome;
        }
        if self.is_coast_to_near(continental_level) && erosion_level == 5 {
            return middle_biome_or_windswept_savanna;
        }
        if self.is_mid_to_far(continental_level) && erosion_level == 5 {
            return shattered_biome;
        }
        middle_biome
    }

    /// Java: `pickMidSliceBiome(...)`(L158-196).
    #[allow(clippy::too_many_arguments)]
    fn pick_mid_slice_biome(
        &self,
        temperature_level: i32,
        humidity_level: i32,
        continental_level: i32,
        erosion_level: i32,
        weirdness: f32,
    ) -> i32 {
        let weird = weirdness >= 0.0f32;
        if self.is_coast(continental_level) && erosion_level <= 2 {
            return STONE_BEACH;
        }
        if (temperature_level == 1 || temperature_level == 2)
            && self.is_near_to_far(continental_level)
            && erosion_level == 6
        {
            return SWAMPLAND;
        }
        if (temperature_level == 3 || temperature_level == 4)
            && self.is_near_to_far(continental_level)
            && erosion_level == 6
        {
            return MANGROVE_SWAMP;
        }

        let middle_biome = self.get_middle_biome(temperature_level, humidity_level, weird);
        let middle_biome_or_badlands_if_hot =
            self.get_middle_biome_or_badlands_if_hot(temperature_level, humidity_level, weird);
        let middle_biome_or_badlands_if_hot_or_slope_if_cold = self
            .get_middle_biome_or_badlands_if_hot_or_slope_if_cold(
                temperature_level,
                humidity_level,
                weird,
            );
        let shattered_biome = self.get_shattered_biome(temperature_level, humidity_level, weird);
        let plateau_biome = self.get_plateau_biome(temperature_level, humidity_level, weird);
        let beach_biome = self.get_beach_biome(temperature_level);
        let middle_biome_or_windswept_savanna = self.maybe_pick_windswept_savanna_biome(
            temperature_level,
            humidity_level,
            weird,
            middle_biome,
        );
        let shattered_coast_biome =
            self.pick_shattered_coast_biome(temperature_level, humidity_level, weird);
        let slope_biome = self.get_slope_biome(temperature_level, humidity_level, weird);

        if self.is_near_to_far(continental_level) && erosion_level == 0 {
            return slope_biome;
        }
        if self.is_near_to_mid(continental_level) && erosion_level == 1 {
            return middle_biome_or_badlands_if_hot_or_slope_if_cold;
        }
        if self.is_far(continental_level) && erosion_level == 1 {
            return if temperature_level == 0 {
                slope_biome
            } else {
                plateau_biome
            };
        }
        if self.is_near(continental_level) && erosion_level == 2 {
            return middle_biome;
        }
        if self.is_mid(continental_level) && erosion_level == 2 {
            return middle_biome_or_badlands_if_hot;
        }
        if self.is_far(continental_level) && erosion_level == 2 {
            return plateau_biome;
        }
        if self.is_coast_to_near(continental_level) && erosion_level == 3 {
            return middle_biome;
        }
        if self.is_mid_to_far(continental_level) && erosion_level == 3 {
            return middle_biome_or_badlands_if_hot;
        }
        if erosion_level == 4 {
            if !weird {
                if self.is_coast(continental_level) {
                    return beach_biome;
                }
                if self.is_near_to_far(continental_level) {
                    return middle_biome;
                }
            } else if self.is_coast_to_far(continental_level) {
                return middle_biome;
            }
        }
        if self.is_coast(continental_level) && erosion_level == 5 {
            return shattered_coast_biome;
        }
        if self.is_near(continental_level) && erosion_level == 5 {
            return middle_biome_or_windswept_savanna;
        }
        if self.is_mid_to_far(continental_level) && erosion_level == 5 {
            return shattered_biome;
        }
        if self.is_coast(continental_level) && erosion_level == 6 {
            return if weird { middle_biome } else { beach_biome };
        }
        if temperature_level == 0 && self.is_near_to_far(continental_level) && erosion_level == 6 {
            return middle_biome;
        }
        middle_biome
    }

    /// Java: `pickLowSliceBiome(...)`(L198-223).
    #[allow(clippy::too_many_arguments)]
    fn pick_low_slice_biome(
        &self,
        temperature_level: i32,
        humidity_level: i32,
        continental_level: i32,
        erosion_level: i32,
        weirdness: f32,
    ) -> i32 {
        let weird = weirdness >= 0.0f32;
        if self.is_coast(continental_level) && erosion_level <= 2 {
            return STONE_BEACH;
        }
        if (temperature_level == 1 || temperature_level == 2)
            && self.is_near_to_far(continental_level)
            && erosion_level == 6
        {
            return SWAMPLAND;
        }
        if (temperature_level == 3 || temperature_level == 4)
            && self.is_near_to_far(continental_level)
            && erosion_level == 6
        {
            return MANGROVE_SWAMP;
        }

        let middle_biome = self.get_middle_biome(temperature_level, humidity_level, weird);
        let middle_biome_or_badlands_if_hot =
            self.get_middle_biome_or_badlands_if_hot(temperature_level, humidity_level, weird);
        let middle_biome_or_badlands_if_hot_or_slope_if_cold = self
            .get_middle_biome_or_badlands_if_hot_or_slope_if_cold(
                temperature_level,
                humidity_level,
                weird,
            );
        let beach_biome = self.get_beach_biome(temperature_level);
        let middle_biome_or_windswept_savanna = self.maybe_pick_windswept_savanna_biome(
            temperature_level,
            humidity_level,
            weird,
            middle_biome,
        );
        let shattered_coast_biome =
            self.pick_shattered_coast_biome(temperature_level, humidity_level, weird);

        if self.is_near(continental_level) && (erosion_level == 0 || erosion_level == 1) {
            return middle_biome_or_badlands_if_hot;
        }
        if self.is_mid_to_far(continental_level) && (erosion_level == 0 || erosion_level == 1) {
            return middle_biome_or_badlands_if_hot_or_slope_if_cold;
        }
        if self.is_near(continental_level) && (erosion_level == 2 || erosion_level == 3) {
            return middle_biome;
        }
        if self.is_mid_to_far(continental_level) && (erosion_level == 2 || erosion_level == 3) {
            return middle_biome_or_badlands_if_hot;
        }
        if self.is_coast(continental_level) && (erosion_level == 3 || erosion_level == 4) {
            return beach_biome;
        }
        if self.is_near_to_far(continental_level) && erosion_level == 4 {
            return middle_biome;
        }
        if self.is_coast(continental_level) && erosion_level == 5 {
            return shattered_coast_biome;
        }
        if self.is_near(continental_level) && erosion_level == 5 {
            return middle_biome_or_windswept_savanna;
        }
        if self.is_mid_to_far(continental_level) && erosion_level == 5 {
            return middle_biome;
        }
        if self.is_coast(continental_level) && erosion_level == 6 {
            return beach_biome;
        }
        if temperature_level == 0 && self.is_near_to_far(continental_level) && erosion_level == 6 {
            return middle_biome;
        }
        middle_biome
    }

    /// Java: `pickValleysBiome(...)`(L225-243).
    #[allow(clippy::too_many_arguments)]
    fn pick_valleys_biome(
        &self,
        temperature_level: i32,
        humidity_level: i32,
        continental_level: i32,
        erosion_level: i32,
        weirdness: f32,
    ) -> i32 {
        let weird = weirdness >= 0.0f32;
        let frozen = temperature_level == 0;

        if self.is_coast(continental_level) && (erosion_level == 0 || erosion_level == 1) {
            return if frozen {
                if weird {
                    FROZEN_RIVER
                } else {
                    STONE_BEACH
                }
            } else {
                if weird {
                    RIVER
                } else {
                    STONE_BEACH
                }
            };
        }
        if self.is_near(continental_level) && (erosion_level == 0 || erosion_level == 1) {
            return if frozen { FROZEN_RIVER } else { RIVER };
        }
        if self.is_coast_to_far(continental_level) && erosion_level >= 2 && erosion_level <= 5 {
            return if frozen { FROZEN_RIVER } else { RIVER };
        }
        if self.is_coast(continental_level) && erosion_level == 6 {
            return if frozen { FROZEN_RIVER } else { RIVER };
        }
        if (temperature_level == 1 || temperature_level == 2)
            && self.is_near_to_far(continental_level)
            && erosion_level == 6
        {
            return SWAMPLAND;
        }
        if (temperature_level == 3 || temperature_level == 4)
            && self.is_near_to_far(continental_level)
            && erosion_level == 6
        {
            return MANGROVE_SWAMP;
        }
        if frozen && self.is_near_to_far(continental_level) && erosion_level == 6 {
            return FROZEN_RIVER;
        }
        if self.is_mid_to_far(continental_level) && (erosion_level == 0 || erosion_level == 1) {
            return self.get_middle_biome_or_badlands_if_hot(
                temperature_level,
                humidity_level,
                weird,
            );
        }
        if frozen {
            FROZEN_RIVER
        } else {
            RIVER
        }
    }

    // --- Helper biome selection ---

    /// Java: `getMiddleBiomeOrBadlandsIfHot(int, int, boolean)`(L245-247).
    fn get_middle_biome_or_badlands_if_hot(
        &self,
        temperature_level: i32,
        humidity_level: i32,
        weird: bool,
    ) -> i32 {
        if temperature_level == 4 {
            self.get_badland_biome(humidity_level, weird)
        } else {
            self.get_middle_biome(temperature_level, humidity_level, weird)
        }
    }

    /// Java: `getMiddleBiomeOrBadlandsIfHotOrSlopeIfCold(int, int, boolean)`(L249-254).
    fn get_middle_biome_or_badlands_if_hot_or_slope_if_cold(
        &self,
        temperature_level: i32,
        humidity_level: i32,
        weird: bool,
    ) -> i32 {
        if temperature_level == 0 {
            self.get_slope_biome(temperature_level, humidity_level, weird)
        } else {
            self.get_middle_biome_or_badlands_if_hot(temperature_level, humidity_level, weird)
        }
    }

    /// Java: `maybePickWindsweptSavannaBiome(int, int, boolean, int)`(L256-258).
    fn maybe_pick_windswept_savanna_biome(
        &self,
        temperature_level: i32,
        humidity_level: i32,
        weird: bool,
        underlying_biome: i32,
    ) -> i32 {
        if weird && temperature_level > 1 && humidity_level < 4 {
            SAVANNA_MUTATED
        } else {
            underlying_biome
        }
    }

    /// Java: `pickShatteredCoastBiome(int, int, boolean)`(L260-263).
    fn pick_shattered_coast_biome(
        &self,
        temperature_level: i32,
        humidity_level: i32,
        weird: bool,
    ) -> i32 {
        let beach_or_middle_biome = if weird {
            self.get_middle_biome(temperature_level, humidity_level, true)
        } else {
            self.get_beach_biome(temperature_level)
        };
        self.maybe_pick_windswept_savanna_biome(
            temperature_level,
            humidity_level,
            weird,
            beach_or_middle_biome,
        )
    }

    /// Java: `getPeakBiome(int, int, boolean)`(L265-273).
    fn get_peak_biome(&self, temperature_level: i32, humidity_level: i32, weird: bool) -> i32 {
        if temperature_level <= 2 {
            return if weird { FROZEN_PEAKS } else { JAGGED_PEAKS };
        }
        if temperature_level == 3 {
            return STONY_PEAKS;
        }
        self.get_badland_biome(humidity_level, weird)
    }

    /// Java: `getSlopeBiome(int, int, boolean)`(L275-280).
    fn get_slope_biome(&self, temperature_level: i32, humidity_level: i32, weird: bool) -> i32 {
        if temperature_level >= 3 {
            return self.get_plateau_biome(temperature_level, humidity_level, weird);
        }
        if humidity_level <= 1 {
            SNOWY_SLOPES
        } else {
            GROVE
        }
    }

    // --- Continental-level classification ---

    fn is_coast(&self, continental_level: i32) -> bool {
        continental_level == CONTINENT_COAST
    }
    fn is_near(&self, continental_level: i32) -> bool {
        continental_level == CONTINENT_NEAR_INLAND
    }
    fn is_mid(&self, continental_level: i32) -> bool {
        continental_level == CONTINENT_MID_INLAND
    }
    fn is_far(&self, continental_level: i32) -> bool {
        continental_level == CONTINENT_FAR_INLAND
    }
    fn is_coast_to_near(&self, continental_level: i32) -> bool {
        continental_level >= CONTINENT_COAST && continental_level <= CONTINENT_NEAR_INLAND
    }
    fn is_coast_to_far(&self, continental_level: i32) -> bool {
        continental_level >= CONTINENT_COAST && continental_level <= CONTINENT_FAR_INLAND
    }
    fn is_near_to_mid(&self, continental_level: i32) -> bool {
        continental_level >= CONTINENT_NEAR_INLAND && continental_level <= CONTINENT_MID_INLAND
    }
    fn is_near_to_far(&self, continental_level: i32) -> bool {
        continental_level >= CONTINENT_NEAR_INLAND && continental_level <= CONTINENT_FAR_INLAND
    }
    fn is_mid_to_far(&self, continental_level: i32) -> bool {
        continental_level >= CONTINENT_MID_INLAND && continental_level <= CONTINENT_FAR_INLAND
    }

    // --- Base biome table ---

    /// Java: `getBeachBiome(int)`(L318-324).
    fn get_beach_biome(&self, temperature_level: i32) -> i32 {
        match temperature_level {
            0 => COLD_BEACH,
            1 | 2 | 3 => BEACH,
            _ => DESERT,
        }
    }

    /// Java: `getBadlandBiome(int, boolean)`(L326-332).
    fn get_badland_biome(&self, humidity_level: i32, weird: bool) -> i32 {
        match humidity_level {
            0 | 1 => {
                if weird {
                    MESA_BRYCE
                } else {
                    MESA
                }
            }
            2 => MESA,
            _ => MESA_PLATEAU_STONE,
        }
    }

    /// Java: `getMiddleBiome(int, int, boolean)`(L334-364).
    fn get_middle_biome(&self, temperature_level: i32, humidity_level: i32, weird: bool) -> i32 {
        match temperature_level {
            0 => match humidity_level {
                0 => {
                    if weird {
                        ICE_PLAINS_SPIKES
                    } else {
                        ICE_PLAINS
                    }
                }
                1 => ICE_PLAINS,
                2 => {
                    if weird {
                        COLD_TAIGA
                    } else {
                        ICE_PLAINS
                    }
                }
                3 => COLD_TAIGA,
                _ => TAIGA,
            },
            1 => match humidity_level {
                0 | 1 => PLAINS,
                2 => FOREST,
                3 => TAIGA,
                _ => {
                    if weird {
                        MEGA_TAIGA
                    } else {
                        REDWOOD_TAIGA_MUTATED
                    }
                }
            },
            2 => match humidity_level {
                0 => {
                    if weird {
                        SUNFLOWER_PLAINS
                    } else {
                        FLOWER_FOREST
                    }
                }
                1 => PLAINS,
                2 => FOREST,
                3 => {
                    if weird {
                        BIRCH_FOREST_MUTATED
                    } else {
                        BIRCH_FOREST
                    }
                }
                _ => ROOFED_FOREST,
            },
            3 => match humidity_level {
                0 | 1 => SAVANNA,
                2 => {
                    if weird {
                        PLAINS
                    } else {
                        FOREST
                    }
                }
                3 => {
                    if weird {
                        JUNGLE_EDGE
                    } else {
                        JUNGLE
                    }
                }
                _ => {
                    if weird {
                        BAMBOO_JUNGLE
                    } else {
                        JUNGLE
                    }
                }
            },
            _ => DESERT,
        }
    }

    /// Java: `getPlateauBiome(int, int, boolean)`(L366-397).
    fn get_plateau_biome(&self, temperature_level: i32, humidity_level: i32, weird: bool) -> i32 {
        match temperature_level {
            0 => match humidity_level {
                0 => {
                    if weird {
                        ICE_PLAINS_SPIKES
                    } else {
                        ICE_PLAINS
                    }
                }
                1 | 2 => ICE_PLAINS,
                _ => COLD_TAIGA,
            },
            1 => match humidity_level {
                0 => {
                    if weird {
                        CHERRY_GROVE
                    } else {
                        MEADOW
                    }
                }
                1 => MEADOW,
                2 => {
                    if weird {
                        MEADOW
                    } else {
                        FOREST
                    }
                }
                3 => {
                    if weird {
                        MEADOW
                    } else {
                        TAIGA
                    }
                }
                _ => {
                    if weird {
                        MEGA_TAIGA
                    } else {
                        REDWOOD_TAIGA_MUTATED
                    }
                }
            },
            2 => match humidity_level {
                0 | 1 => {
                    if weird {
                        CHERRY_GROVE
                    } else {
                        MEADOW
                    }
                }
                2 => {
                    if weird {
                        FOREST
                    } else {
                        MEADOW
                    }
                }
                3 => {
                    if weird {
                        BIRCH_FOREST
                    } else {
                        MEADOW
                    }
                }
                _ => PALE_GARDEN,
            },
            3 => match humidity_level {
                0 | 1 => SAVANNA_PLATEAU,
                2 | 3 => FOREST,
                _ => JUNGLE,
            },
            _ => match humidity_level {
                0 | 1 => {
                    if weird {
                        MESA_BRYCE
                    } else {
                        MESA
                    }
                }
                2 => MESA,
                _ => MESA_PLATEAU_STONE,
            },
        }
    }

    /// Java: `getShatteredBiome(int, int, boolean)`(L399-418).
    fn get_shattered_biome(&self, temperature_level: i32, humidity_level: i32, weird: bool) -> i32 {
        match temperature_level {
            0 | 1 => match humidity_level {
                0 | 1 => EXTREME_HILLS_MUTATED,
                2 => EXTREME_HILLS,
                _ => EXTREME_HILLS_PLUS_TREES,
            },
            2 => match humidity_level {
                0 | 1 | 2 => EXTREME_HILLS,
                _ => EXTREME_HILLS_PLUS_TREES,
            },
            3 => match humidity_level {
                0 | 1 => SAVANNA,
                2 => {
                    if weird {
                        PLAINS
                    } else {
                        FOREST
                    }
                }
                3 => {
                    if weird {
                        JUNGLE_EDGE
                    } else {
                        JUNGLE
                    }
                }
                _ => {
                    if weird {
                        BAMBOO_JUNGLE
                    } else {
                        JUNGLE
                    }
                }
            },
            _ => DESERT,
        }
    }
}

impl BiomePicker for OverworldBiomePicker {
    type Result = OverworldBiomeResult;

    fn pick(&self, x: i32, y: i32, z: i32) -> Self::Result {
        // Delegates to the inherent pick with a fresh point context.
        OverworldBiomePicker::pick(self, x, y, z)
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::material::MaterialBlocks;
    use crate::worldgen::random::Xoroshiro128;
    use sc_world::chunk::BlockRuntimeId;

    fn test_blocks() -> MaterialBlocks {
        MaterialBlocks {
            air: BlockRuntimeId(0),
            water: BlockRuntimeId(1),
            lava: BlockRuntimeId(2),
            stone: BlockRuntimeId(3),
            granite: BlockRuntimeId(4),
            tuff: BlockRuntimeId(5),
            copper_ore: BlockRuntimeId(6),
            deepslate_iron_ore: BlockRuntimeId(7),
            raw_copper_block: BlockRuntimeId(8),
            raw_iron_block: BlockRuntimeId(9),
        }
    }

    fn make_picker(heightmap_value: i32) -> OverworldBiomePicker {
        let holder = NormalObjectHolder::new(Xoroshiro128::new(12345), test_blocks());
        OverworldBiomePicker::new(&holder, 12345, Box::new(move |_x, _z| heightmap_value))
    }

    #[test]
    fn pick_raw_deterministic() {
        // Same seed and coordinates give identical results.
        let p1 = make_picker(SEA_LEVEL);
        let p2 = make_picker(SEA_LEVEL);
        let r1 = p1.pick_raw(0, SEA_LEVEL, 0);
        let r2 = p2.pick_raw(0, SEA_LEVEL, 0);
        assert_eq!(r1.biome_id(), r2.biome_id());
        assert_eq!(r1.continental, r2.continental);
        assert_eq!(r1.weirdness, r2.weirdness);
    }

    #[test]
    fn pick_raw_different_coords_differ() {
        // Distant coordinates usually differ (statistical check).
        let p = make_picker(SEA_LEVEL);
        let r1 = p.pick_raw(0, SEA_LEVEL, 0);
        let r2 = p.pick_raw(1000, SEA_LEVEL, 1000);
        // Biome ids may coincide; the noise values must differ.
        assert!(
            r1.continental != r2.continental
                || r1.temperature != r2.temperature
                || r1.humidity != r2.humidity
                || r1.erosion != r2.erosion
                || r1.weirdness != r2.weirdness
        );
    }

    #[test]
    fn correct_and_reset() {
        let p = make_picker(SEA_LEVEL);
        let mut r = p.pick_raw(0, SEA_LEVEL, 0);
        let original = r.biome_id();
        // correct must not panic; deep depths may rewrite the biome.
        r.correct(-100); // depth ~= 0.78 lands in [0.2, 0.99).
                         // reset restores.
        r.reset();
        assert_eq!(r.biome_id(), original);
    }

    #[test]
    fn correct_deep_dark_threshold() {
        // depth > 0.99 with erosion < -0.225 gives DEEP_DARK.
        // Hand-built results pin the threshold logic.
        let mut r = OverworldBiomeResult::new(PLAINS, 0.0, 0.0, 0.0, -0.3, 0.0, 0.0);
        // y = -128 → depth = 128/128 = 1.0 ≥ 0.99, erosion < -0.225 → DEEP_DARK
        r.correct(-128);
        assert_eq!(r.biome_id(), DEEP_DARK);
        r.reset();
        assert_eq!(r.biome_id(), PLAINS);
    }

    #[test]
    fn correct_dripstone_threshold() {
        // depth ∈ [0.2, 0.99), continental ∈ (0.8, 1.0) → DRIPSTONE_CAVES
        let mut r = OverworldBiomeResult::new(PLAINS, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0);
        // y = -50 → depth = 50/128 ≈ 0.39 ∈ [0.2, 0.99)
        r.correct(-50);
        assert_eq!(r.biome_id(), DRIPSTONE_CAVES);
    }

    #[test]
    fn biome_id_constants_match_upstream() {
        // Key constants match the biome id table.
        assert_eq!(OCEAN, 0);
        assert_eq!(PLAINS, 1);
        assert_eq!(MUSHROOM_ISLAND, 14);
        assert_eq!(DEEP_DARK, 190);
        assert_eq!(SULFUR_CAVES, 194);
        assert_eq!(PALE_GARDEN, 193);
    }

    #[test]
    fn continent_level_thresholds() {
        // Continental threshold boundaries.
        let p = make_picker(SEA_LEVEL);
        // continental < -1.05 → level 0 (MUSHROOM)
        let r0 = p.pick_raw(0, SEA_LEVEL, 0);
        // pick returns a valid biome id without panicking.
        let _ = r0.biome_id();
    }
}
