//! Cave systems density (spaghetti, noodles, pillars, entrances).

use crate::worldgen::densityfunction::common::{
    abs, add, blend_density, cache_2d, cache_once, clamp, constant, cube, flat_cache, interpolated,
    invert, mapped_noise, mapped_noise_scaled, max, min, mul, noise_scaled_xy, quarter_negative,
    range_choice, square, squeeze, weird_scaled_sampler, y, y_clamped_gradient, RarityValueMapper,
};
use crate::worldgen::densityfunction::function::DensityFunction;
use crate::worldgen::noise::noise::NormalNoise;
use std::sync::Arc;

/// Java: `spaghettiRoughnessFunction(NormalNoise, NormalNoise)`(L33-45).
pub fn spaghetti_roughness_function(
    spaghetti_roughness: Arc<NormalNoise>,
    spaghetti_roughness_modulator: Arc<NormalNoise>,
) -> Arc<dyn DensityFunction> {
    let roughness = noise_scaled_xy(spaghetti_roughness, 1.0, 1.0);
    let modulator = mapped_noise(spaghetti_roughness_modulator, 0.0, -0.1);
    cache_once(mul(modulator, add(abs(roughness), constant(-0.4))))
}

/// Java: `spaghetti2dThicknessModulator(NormalNoise)`(L47-51).
pub fn spaghetti_2d_thickness_modulator(
    spaghetti_2d_thickness: Arc<NormalNoise>,
) -> Arc<dyn DensityFunction> {
    cache_once(mapped_noise_scaled(
        spaghetti_2d_thickness,
        2.0,
        1.0,
        -0.6,
        -1.3,
    ))
}

/// Java: `entrances(DensityFunction, NormalNoise x5)`(L53-85).
pub fn entrances(
    spaghetti_roughness_function: Arc<dyn DensityFunction>,
    spaghetti_3d_rarity: Arc<NormalNoise>,
    spaghetti_3d_thickness: Arc<NormalNoise>,
    spaghetti_3d_first: Arc<NormalNoise>,
    spaghetti_3d_second: Arc<NormalNoise>,
    cave_entrance: Arc<NormalNoise>,
) -> Arc<dyn DensityFunction> {
    let rarity = cache_once(noise_scaled_xy(spaghetti_3d_rarity, 2.0, 1.0));
    let thickness = mapped_noise(spaghetti_3d_thickness, -0.065, -0.088);
    let first = weird_scaled_sampler(
        Arc::clone(&rarity),
        spaghetti_3d_first,
        RarityValueMapper::Type1,
    );
    let second = weird_scaled_sampler(rarity, spaghetti_3d_second, RarityValueMapper::Type1);
    let spaghetti = clamp(add(max(first, second), thickness), -1.0, 1.0);
    let entrance = noise_scaled_xy(cave_entrance, 0.75, 0.5);
    let entrance_gradient = add(
        add(entrance, constant(0.37)),
        y_clamped_gradient(-10, 30, 0.3, 0.0),
    );
    cache_once(min(
        entrance_gradient,
        add(spaghetti_roughness_function, spaghetti),
    ))
}

/// Java: `pillars(NormalNoise x3)`(L87-100).
pub fn pillars(
    pillar: Arc<NormalNoise>,
    pillar_rareness: Arc<NormalNoise>,
    pillar_thickness: Arc<NormalNoise>,
) -> Arc<dyn DensityFunction> {
    let pillar_noise = noise_scaled_xy(pillar, 25.0, 0.3);
    let rarity = mapped_noise(pillar_rareness, 0.0, -2.0);
    let thickness = mapped_noise(pillar_thickness, 0.0, 1.1);
    let combined = add(mul(pillar_noise, constant(2.0)), rarity);
    cache_once(mul(combined, cube(thickness)))
}

/// Java: `spaghetti2d(DensityFunction, NormalNoise x4)`(L102-129).
pub fn spaghetti_2d(
    spaghetti_2d_thickness_modulator: Arc<dyn DensityFunction>,
    spaghetti_2d_modulator: Arc<NormalNoise>,
    spaghetti_2d: Arc<NormalNoise>,
    spaghetti_2d_elevation: Arc<NormalNoise>,
) -> Arc<dyn DensityFunction> {
    let modulator = noise_scaled_xy(spaghetti_2d_modulator, 2.0, 1.0);
    let sampled = weird_scaled_sampler(modulator, spaghetti_2d, RarityValueMapper::Type2);
    let elevation = flat_cache(mul(
        constant(8.0),
        noise_scaled_xy(spaghetti_2d_elevation, 1.0, 0.0),
    ));
    let elevation_gradient = cube(add(
        abs(add(elevation, y_clamped_gradient(-64, 320, 8.0, -40.0))),
        Arc::clone(&spaghetti_2d_thickness_modulator),
    ));
    let adjusted = add(
        sampled,
        mul(constant(0.083), spaghetti_2d_thickness_modulator),
    );
    clamp(max(adjusted, elevation_gradient), -1.0, 1.0)
}

/// Java: `noodle(NormalNoise x4)`(L131-172).
pub fn noodle(
    noodle: Arc<NormalNoise>,
    noodle_thickness: Arc<NormalNoise>,
    noodle_ridge_a: Arc<NormalNoise>,
    noodle_ridge_b: Arc<NormalNoise>,
) -> Arc<dyn DensityFunction> {
    let toggle = y_limited_interpolatable(noise_scaled_xy(noodle, 1.0, 1.0), -60, 320, -1.0);
    let thickness = y_limited_interpolatable(
        mapped_noise_scaled(noodle_thickness, 1.0, 1.0, -0.05, -0.1),
        -60,
        320,
        0.0,
    );
    let ridge_a = y_limited_interpolatable(
        noise_scaled_xy(noodle_ridge_a, 2.6666666666666665, 2.6666666666666665),
        -60,
        320,
        0.0,
    );
    let ridge_b = y_limited_interpolatable(
        noise_scaled_xy(noodle_ridge_b, 2.6666666666666665, 2.6666666666666665),
        -60,
        320,
        0.0,
    );
    let ridges = mul(constant(1.5), max(abs(ridge_a), abs(ridge_b)));
    range_choice(
        toggle,
        -1000000.0,
        0.0,
        constant(64.0),
        add(thickness, ridges),
    )
}

/// Java: `finalDensity(DensityFunction, NormalNoise x20)`(L174-260).
#[allow(clippy::too_many_arguments)]
pub fn final_density(
    sloped_cheese: Arc<dyn DensityFunction>,
    spaghetti_roughness: Arc<NormalNoise>,
    spaghetti_roughness_modulator: Arc<NormalNoise>,
    spaghetti_2d_thickness: Arc<NormalNoise>,
    spaghetti_2d_modulator: Arc<NormalNoise>,
    spaghetti_2d: Arc<NormalNoise>,
    spaghetti_2d_elevation: Arc<NormalNoise>,
    spaghetti_3d_rarity: Arc<NormalNoise>,
    spaghetti_3d_thickness: Arc<NormalNoise>,
    spaghetti_3d_first: Arc<NormalNoise>,
    spaghetti_3d_second: Arc<NormalNoise>,
    cave_entrance: Arc<NormalNoise>,
    cave_layer: Arc<NormalNoise>,
    cave_cheese: Arc<NormalNoise>,
    pillar: Arc<NormalNoise>,
    pillar_rareness: Arc<NormalNoise>,
    pillar_thickness: Arc<NormalNoise>,
    noodle: Arc<NormalNoise>,
    noodle_thickness: Arc<NormalNoise>,
    noodle_ridge_a: Arc<NormalNoise>,
    noodle_ridge_b: Arc<NormalNoise>,
) -> Arc<dyn DensityFunction> {
    let spaghetti_roughness_function =
        spaghetti_roughness_function(spaghetti_roughness, spaghetti_roughness_modulator);
    let spaghetti_2d_thickness_modulator = spaghetti_2d_thickness_modulator(spaghetti_2d_thickness);
    // Parameter shadows the function name; call via fully qualified path.
    let spaghetti_2d_function = self::spaghetti_2d(
        spaghetti_2d_thickness_modulator,
        spaghetti_2d_modulator,
        spaghetti_2d,
        spaghetti_2d_elevation,
    );
    let entrances = entrances(
        Arc::clone(&spaghetti_roughness_function),
        spaghetti_3d_rarity,
        spaghetti_3d_thickness,
        spaghetti_3d_first,
        spaghetti_3d_second,
        cave_entrance,
    );
    let pillars = pillars(pillar, pillar_rareness, pillar_thickness);
    let underground = underground(
        Arc::clone(&sloped_cheese),
        spaghetti_2d_function,
        spaghetti_roughness_function,
        Arc::clone(&entrances),
        pillars,
        cave_layer,
        cave_cheese,
    );
    let caves = range_choice(
        Arc::clone(&sloped_cheese),
        -1000000.0,
        1.5625,
        min(sloped_cheese, mul(constant(5.0), entrances)),
        underground,
    );
    let post_processed = squeeze(mul(
        constant(0.64),
        interpolated(blend_density(add(
            constant(0.1171875),
            mul(
                y_clamped_gradient(-64, -40, 0.0, 1.0),
                add(
                    constant(-0.1171875),
                    add(
                        constant(-0.078125),
                        mul(
                            y_clamped_gradient(240, 256, 1.0, 0.0),
                            add(constant(0.078125), caves),
                        ),
                    ),
                ),
            ),
        ))),
    ));
    min(
        post_processed,
        // Parameter shadows the function name; call via fully qualified path.
        self::noodle(noodle, noodle_thickness, noodle_ridge_a, noodle_ridge_b),
    )
}

/// Java: `preliminarySurfaceLevel(DensityFunction, DensityFunction)`(L262-294).
pub fn preliminary_surface_level(
    offset: Arc<dyn DensityFunction>,
    factor: Arc<dyn DensityFunction>,
) -> Arc<dyn DensityFunction> {
    let base = add(y_clamped_gradient(-64, 320, 1.5, -1.5), cache_2d(offset));
    let scaled = quarter_negative(mul(base, cache_2d(factor)));
    let clamped = clamp(
        add(constant(-0.703125), mul(constant(4.0), scaled)),
        -64.0,
        64.0,
    );
    add(
        constant(-0.390625),
        add(
            constant(0.1171875),
            mul(
                y_clamped_gradient(-64, -40, 0.0, 1.0),
                add(
                    constant(-0.1171875),
                    add(
                        constant(-0.078125),
                        mul(
                            y_clamped_gradient(240, 256, 1.0, 0.0),
                            add(constant(0.078125), clamped),
                        ),
                    ),
                ),
            ),
        ),
    )
}

/// Java: `preliminarySurfaceLevelUpperBound(DensityFunction, DensityFunction)`(L296-313).
pub fn preliminary_surface_level_upper_bound(
    offset: Arc<dyn DensityFunction>,
    factor: Arc<dyn DensityFunction>,
) -> Arc<dyn DensityFunction> {
    clamp(
        add(
            constant(128.0),
            mul(
                constant(-128.0),
                add(
                    mul(constant(0.2734375), invert(cache_2d(factor))),
                    mul(constant(-1.0), cache_2d(offset)),
                ),
            ),
        ),
        -40.0,
        320.0,
    )
}

/// Java: `underground(DensityFunction x5, NormalNoise x2)`(L315-351).
fn underground(
    sloped_cheese: Arc<dyn DensityFunction>,
    spaghetti_2d: Arc<dyn DensityFunction>,
    spaghetti_roughness_function: Arc<dyn DensityFunction>,
    entrances: Arc<dyn DensityFunction>,
    pillars: Arc<dyn DensityFunction>,
    cave_layer: Arc<NormalNoise>,
    cave_cheese: Arc<NormalNoise>,
) -> Arc<dyn DensityFunction> {
    let layerized_caverns = mul(constant(4.0), square(noise_scaled_xy(cave_layer, 1.0, 8.0)));
    let cave_cheese_function = add(
        clamp(
            add(
                constant(0.27),
                noise_scaled_xy(cave_cheese, 1.0, 0.6666666666666666),
            ),
            -1.0,
            1.0,
        ),
        clamp(
            add(constant(1.5), mul(constant(-0.64), sloped_cheese)),
            0.0,
            0.5,
        ),
    );
    let cave_density = add(layerized_caverns, cave_cheese_function);
    let passages = min(
        min(cave_density, entrances),
        add(spaghetti_2d, spaghetti_roughness_function),
    );
    let pillar_filter = range_choice(
        Arc::clone(&pillars),
        -1000000.0,
        0.03,
        constant(-1000000.0),
        pillars,
    );
    max(passages, pillar_filter)
}

/// Java: `yLimitedInterpolatable(DensityFunction, int, int, double)`(L353-363).
fn y_limited_interpolatable(
    density: Arc<dyn DensityFunction>,
    min_y: i32,
    max_y: i32,
    when_out_of_range: f64,
) -> Arc<dyn DensityFunction> {
    interpolated(range_choice(
        y(),
        min_y as f64,
        (max_y + 1) as f64,
        density,
        constant(when_out_of_range),
    ))
}
