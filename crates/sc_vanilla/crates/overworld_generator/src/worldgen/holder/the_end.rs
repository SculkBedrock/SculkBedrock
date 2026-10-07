//! End holder with terrain noise.

use crate::worldgen::holder::ObjectHolder;
use crate::worldgen::noise::d::{NoiseGeneratorOctavesD, NoiseGeneratorSimplexD};
use crate::worldgen::random::{RandomSourceProvider, Xoroshiro128};

// ---------------------------------------------------------------------------
// TheEndObjectHolder
// ---------------------------------------------------------------------------

/// End object holder.
pub struct TheEndObjectHolder {
    random: Xoroshiro128,
    terrain_holder: TheEndTerrainHolder,
}

impl TheEndObjectHolder {
    /// Java: `TheEndObjectHolder(RandomSourceProvider)`(L12-15).
    pub fn new(mut random: Xoroshiro128) -> Self {
        let terrain_holder = TheEndTerrainHolder::new(&mut random);
        Self {
            random,
            terrain_holder,
        }
    }

    pub fn random(&self) -> Xoroshiro128 {
        self.random
    }
    pub fn terrain_holder(&self) -> &TheEndTerrainHolder {
        &self.terrain_holder
    }
}

impl ObjectHolder for TheEndObjectHolder {}

// ---------------------------------------------------------------------------
// TerrainHolder
// ---------------------------------------------------------------------------

/// End terrain holder with roughness, detail, and island noises.
pub struct TheEndTerrainHolder {
    random: Xoroshiro128,
    roughness_noise_octaves: NoiseGeneratorOctavesD,
    roughness_noise_octaves2: NoiseGeneratorOctavesD,
    detail_noise_octaves: NoiseGeneratorOctavesD,
    island_noise: NoiseGeneratorSimplexD,
}

impl TheEndTerrainHolder {
    /// Java: `TerrainHolder(RandomSourceProvider)`(L25-31).
    ///
    /// All identical() calls preserve RNG state.
    pub fn new(random: &mut Xoroshiro128) -> Self {
        let roughness_noise_octaves = {
            let mut id = random.identical();
            NoiseGeneratorOctavesD::new(&mut id, 16)
        };
        let roughness_noise_octaves2 = {
            let mut id = random.identical();
            NoiseGeneratorOctavesD::new(&mut id, 16)
        };
        let detail_noise_octaves = {
            let mut id = random.identical();
            NoiseGeneratorOctavesD::new(&mut id, 8)
        };
        let island_noise = {
            let mut id = random.identical();
            NoiseGeneratorSimplexD::new(&mut id)
        };
        Self {
            random: *random,
            roughness_noise_octaves,
            roughness_noise_octaves2,
            detail_noise_octaves,
            island_noise,
        }
    }

    pub fn random(&self) -> Xoroshiro128 {
        self.random
    }
    pub fn roughness_noise_octaves(&self) -> &NoiseGeneratorOctavesD {
        &self.roughness_noise_octaves
    }
    pub fn roughness_noise_octaves2(&self) -> &NoiseGeneratorOctavesD {
        &self.roughness_noise_octaves2
    }
    pub fn detail_noise_octaves(&self) -> &NoiseGeneratorOctavesD {
        &self.detail_noise_octaves
    }
    pub fn island_noise(&self) -> &NoiseGeneratorSimplexD {
        &self.island_noise
    }
}
