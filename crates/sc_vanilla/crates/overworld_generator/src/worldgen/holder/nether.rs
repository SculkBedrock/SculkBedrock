//! Nether holder with terrain and basalt-delta noises.

use crate::worldgen::densityfunction::base3d;
use crate::worldgen::densityfunction::function::DensityFunction;
use crate::worldgen::densityfunction::nether;
use crate::worldgen::holder::ObjectHolder;
use crate::worldgen::noise::noise::NormalNoise;
use crate::worldgen::random::{RandomSourceProvider, Xoroshiro128};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// NetherObjectHolder
// ---------------------------------------------------------------------------

/// Nether object holder.
pub struct NetherObjectHolder {
    random: Xoroshiro128,
    terrain_holder: NetherTerrainHolder,
    basalt_deltas_holder: BasaltDeltaHolder,
}

impl NetherObjectHolder {
    /// Java: `NetherObjectHolder(RandomSourceProvider)`(L16-20).
    pub fn new(mut random: Xoroshiro128) -> Self {
        let terrain_holder = NetherTerrainHolder::new(&mut random);
        let basalt_deltas_holder = BasaltDeltaHolder::new(&mut random);
        Self {
            random,
            terrain_holder,
            basalt_deltas_holder,
        }
    }

    pub fn random(&self) -> Xoroshiro128 {
        self.random
    }
    pub fn terrain_holder(&self) -> &NetherTerrainHolder {
        &self.terrain_holder
    }
    pub fn basalt_deltas_holder(&self) -> &BasaltDeltaHolder {
        &self.basalt_deltas_holder
    }
}

impl ObjectHolder for NetherObjectHolder {}

// ---------------------------------------------------------------------------
// TerrainHolder
// ---------------------------------------------------------------------------

/// Nether terrain holder.
pub struct NetherTerrainHolder {
    random: Xoroshiro128,
    surface_noise: Arc<NormalNoise>,
    patch_noise: Arc<NormalNoise>,
    soulsand_noise: Arc<NormalNoise>,
    nether_state_noise: Arc<NormalNoise>,
    netherwart_noise: Arc<NormalNoise>,
    base3d_noise: Arc<dyn DensityFunction>,
    density_function: Arc<dyn DensityFunction>,
}

impl NetherTerrainHolder {
    /// Java: `TerrainHolder(RandomSourceProvider)`(L33-42).
    ///
    /// All identical() calls preserve RNG state.
    pub fn new(random: &mut Xoroshiro128) -> Self {
        let surface_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -6, &[1.0, 1.0, 1.0]))
        };
        let patch_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(
                &mut id,
                -5,
                &[1.0, 0.0, 0.0, 0.0, 0.0, 0.013333333333333334],
            ))
        };
        let soulsand_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(
                &mut id,
                -8,
                &[
                    1.0,
                    1.0,
                    1.0,
                    1.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.013333333333333334,
                ],
            ))
        };
        let nether_state_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -4, &[1.0]))
        };
        let netherwart_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -3, &[1.0, 0.0, 0.0, 0.9]))
        };
        let base3d_noise = {
            let mut id = random.identical();
            base3d::nether(&mut id)
        };
        let density_function = nether::final_density(Arc::clone(&base3d_noise));
        Self {
            random: *random,
            surface_noise,
            patch_noise,
            soulsand_noise,
            nether_state_noise,
            netherwart_noise,
            base3d_noise,
            density_function,
        }
    }

    pub fn random(&self) -> Xoroshiro128 {
        self.random
    }
    pub fn surface_noise(&self) -> &Arc<NormalNoise> {
        &self.surface_noise
    }
    pub fn patch_noise(&self) -> &Arc<NormalNoise> {
        &self.patch_noise
    }
    pub fn soulsand_noise(&self) -> &Arc<NormalNoise> {
        &self.soulsand_noise
    }
    pub fn nether_state_noise(&self) -> &Arc<NormalNoise> {
        &self.nether_state_noise
    }
    pub fn netherwart_noise(&self) -> &Arc<NormalNoise> {
        &self.netherwart_noise
    }
    pub fn base3d_noise(&self) -> &Arc<dyn DensityFunction> {
        &self.base3d_noise
    }
    pub fn density_function(&self) -> &Arc<dyn DensityFunction> {
        &self.density_function
    }
}

// ---------------------------------------------------------------------------
// BasaltDeltaHolder
// ---------------------------------------------------------------------------

/// Basalt delta holder.
pub struct BasaltDeltaHolder {
    random: Xoroshiro128,
    surface_noise: Arc<NormalNoise>,
    surface_sec_noise: Arc<NormalNoise>,
}

impl BasaltDeltaHolder {
    /// Java: `BasaltDeltaHolder(RandomSourceProvider)`(L51-55).
    ///
    /// All identical() calls preserve RNG state.
    pub fn new(random: &mut Xoroshiro128) -> Self {
        let surface_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -6, &[1.0, 1.0, 1.0]))
        };
        let surface_sec_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -6, &[1.0, 0.0, 1.0, 1.0]))
        };
        Self {
            random: *random,
            surface_noise,
            surface_sec_noise,
        }
    }

    pub fn random(&self) -> Xoroshiro128 {
        self.random
    }
    pub fn surface_noise(&self) -> &Arc<NormalNoise> {
        &self.surface_noise
    }
    pub fn surface_sec_noise(&self) -> &Arc<NormalNoise> {
        &self.surface_sec_noise
    }
}
