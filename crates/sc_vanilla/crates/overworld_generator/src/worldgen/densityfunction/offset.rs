//! Overworld offset density combining continents, erosion, and folded ridges.

use crate::worldgen::densityfunction::common::{
    add, blend_alpha, blend_offset, cache_2d, cache_once, constant, flat_cache, mul, p, p_fn,
    spline_from_points,
};
use crate::worldgen::densityfunction::function::DensityFunction;
use std::sync::Arc;

/// Base offset constant.
const BASE_OFFSET: f64 = -0.5037500262260437;

/// Builds the overworld offset function from the three inputs.
pub fn overworld_offset(
    continents: Arc<dyn DensityFunction>,
    erosion: Arc<dyn DensityFunction>,
    ridges_folded: Arc<dyn DensityFunction>,
) -> Arc<dyn DensityFunction> {
    let blend_alpha_cached = cache_once(blend_alpha());
    let blend_term = mul(
        blend_offset(),
        add(
            constant(1.0),
            mul(constant(-1.0), Arc::clone(&blend_alpha_cached)),
        ),
    );

    let offset_term = mul(
        add(
            constant(BASE_OFFSET),
            offset_spline(continents, erosion, ridges_folded),
        ),
        blend_alpha_cached,
    );

    flat_cache(cache_2d(add(blend_term, offset_term)))
}

/// Builds the nested offset spline.
fn offset_spline(
    continents: Arc<dyn DensityFunction>,
    erosion: Arc<dyn DensityFunction>,
    ridges_folded: Arc<dyn DensityFunction>,
) -> Arc<dyn DensityFunction> {
    let rf = |points| spline_from_points(Arc::clone(&ridges_folded), points);

    let ridges_folded1 = rf(vec![
        p(-1.0, -0.08880186, 0.38940096),
        p(1.0, 0.69000006, 0.38940096),
    ]);
    let ridges_folded2 = rf(vec![
        p(-1.0, -0.115760356, 0.37788022),
        p(1.0, 0.6400001, 0.37788022),
    ]);
    let ridges_folded3 = rf(vec![
        p(-1.0, -0.2222, 0.0),
        p(-0.75, -0.2222, 0.0),
        p(-0.65, 0.0, 0.0),
        p(0.5954547, 2.9802322e-8, 0.0),
        p(0.6054547, 2.9802322e-8, 0.2534563),
        p(1.0, 0.100000024, 0.2534563),
    ]);
    let ridges_folded4 = rf(vec![
        p(-1.0, -0.3, 0.5),
        p(-0.4, 0.05, 0.0),
        p(0.0, 0.05, 0.0),
        p(0.4, 0.05, 0.0),
        p(1.0, 0.060000002, 0.007000001),
    ]);
    let ridges_folded5 = rf(vec![
        p(-1.0, -0.15, 0.5),
        p(-0.4, 0.0, 0.0),
        p(0.0, 0.0, 0.0),
        p(0.4, 0.05, 0.1),
        p(1.0, 0.060000002, 0.007000001),
    ]);
    let ridges_folded6 = rf(vec![
        p(-1.0, -0.15, 0.5),
        p(-0.4, 0.0, 0.0),
        p(0.0, 0.0, 0.0),
        p(0.4, 0.0, 0.0),
        p(1.0, 0.0, 0.0),
    ]);
    let ridges_folded7 = rf(vec![
        p(-1.0, -0.02, 0.0),
        p(-0.4, -0.03, 0.0),
        p(0.0, -0.03, 0.0),
        p(0.4, 0.0, 0.06),
        p(1.0, 0.0, 0.0),
    ]);
    let ridges_folded8 = rf(vec![
        p(-1.0, -0.25, 0.5),
        p(-0.4, 0.05, 0.0),
        p(0.0, 0.05, 0.0),
        p(0.4, 0.05, 0.0),
        p(1.0, 0.060000002, 0.007000001),
    ]);
    let ridges_folded9 = rf(vec![
        p(-1.0, -0.1, 0.5),
        p(-0.4, 0.001, 0.01),
        p(0.0, 0.003, 0.01),
        p(0.4, 0.05, 0.094000004),
        p(1.0, 0.060000002, 0.007000001),
    ]);
    let ridges_folded10 = rf(vec![
        p(-1.0, -0.1, 0.5),
        p(-0.4, 0.01, 0.0),
        p(0.0, 0.01, 0.0),
        p(0.4, 0.03, 0.04),
        p(1.0, 0.1, 0.049),
    ]);
    let ridges_folded11 = rf(vec![
        p(-1.0, -0.02, 0.015),
        p(-0.4, 0.01, 0.0),
        p(0.0, 0.01, 0.0),
        p(0.4, 0.03, 0.04),
        p(1.0, 0.1, 0.049),
    ]);
    let ridges_folded12 = rf(vec![
        p(-1.0, 0.20235021, 0.0),
        p(0.0, 0.7161751, 0.5138249),
        p(1.0, 1.23, 0.5138249),
    ]);
    let ridges_folded13 = rf(vec![
        p(-1.0, 0.2, 0.0),
        p(0.0, 0.44682026, 0.43317974),
        p(1.0, 0.88, 0.43317974),
    ]);
    let ridges_folded14 = rf(vec![
        p(-1.0, 0.2, 0.0),
        p(0.0, 0.30829495, 0.3917051),
        p(1.0, 0.70000005, 0.3917051),
    ]);
    let ridges_folded15 = rf(vec![
        p(-1.0, -0.25, 0.5),
        p(-0.4, 0.35, 0.0),
        p(0.0, 0.35, 0.0),
        p(0.4, 0.35, 0.0),
        p(1.0, 0.42000002, 0.049000014),
    ]);
    let ridges_folded16 = rf(vec![
        p(-1.0, -0.1, 0.5),
        p(-0.4, 0.0069999998, 0.07),
        p(0.0, 0.021, 0.07),
        p(0.4, 0.35, 0.658),
        p(1.0, 0.42000002, 0.049000014),
    ]);
    let ridges_folded17 = rf(vec![
        p(-1.0, -0.1, 0.5),
        p(-0.4, 0.01, 0.0),
        p(0.0, 0.01, 0.0),
        p(0.4, 0.03, 0.04),
        p(1.0, 0.1, 0.049),
    ]);
    let ridges_folded18 = rf(vec![
        p(-1.0, -0.05, 0.5),
        p(-0.4, 0.01, 0.0),
        p(0.0, 0.01, 0.0),
        p(0.4, 0.03, 0.04),
        p(1.0, 0.1, 0.049),
    ]);
    let ridges_folded19 = rf(vec![
        p(-1.0, 0.2, 0.0),
        p(0.0, 0.5391705, 0.4608295),
        p(1.0, 1.0, 0.4608295),
    ]);
    let ridges_folded20 = rf(vec![
        p(-1.0, -0.2, 0.5),
        p(-0.4, 0.5, 0.0),
        p(0.0, 0.5, 0.0),
        p(0.4, 0.5, 0.0),
        p(1.0, 0.6, 0.070000015),
    ]);
    let ridges_folded21 = rf(vec![
        p(-1.0, -0.05, 0.5),
        p(-0.4, 0.01, 0.099999994),
        p(0.0, 0.03, 0.099999994),
        p(0.4, 0.5, 0.94),
        p(1.0, 0.6, 0.070000015),
    ]);
    let ridges_folded22 = rf(vec![
        p(-1.0, -0.05, 0.5),
        p(-0.4, 0.01, 0.0),
        p(0.0, 0.01, 0.0),
        p(0.4, 0.03, 0.04),
        p(1.0, 0.1, 0.049),
    ]);
    let ridges_folded23 = rf(vec![
        p(-1.0, -0.02, 0.015),
        p(-0.4, 0.01, 0.0),
        p(0.0, 0.01, 0.0),
        p(0.4, 0.03, 0.04),
        p(1.0, 0.1, 0.049),
    ]);
    let ridges_folded24 = rf(vec![
        p(-1.0, 0.34792626, 0.0),
        p(0.0, 0.9239631, 0.5760369),
        p(1.0, 1.5, 0.5760369),
    ]);
    let ridges_folded25 = rf(vec![
        p(-1.0, -0.1, 0.0),
        p(-0.4, 0.1, 0.0),
        p(0.0, 0.17, 0.0),
    ]);
    let ridges_folded26 = rf(vec![
        p(-1.0, -0.05, 0.0),
        p(-0.4, 0.1, 0.0),
        p(0.0, 0.17, 0.0),
    ]);

    let er = |points| spline_from_points(Arc::clone(&erosion), points);

    let erosion1 = er(vec![
        p_fn(-0.85, Arc::clone(&ridges_folded1), 0.0),
        p_fn(-0.7, Arc::clone(&ridges_folded2), 0.0),
        p_fn(-0.4, Arc::clone(&ridges_folded3), 0.0),
        p_fn(-0.35, Arc::clone(&ridges_folded4), 0.0),
        p_fn(-0.1, Arc::clone(&ridges_folded5), 0.0),
        p_fn(0.2, Arc::clone(&ridges_folded6), 0.0),
        p_fn(0.7, Arc::clone(&ridges_folded7), 0.0),
    ]);
    let erosion2 = er(vec![
        p_fn(-0.85, ridges_folded1, 0.0),
        p_fn(-0.7, ridges_folded2, 0.0),
        p_fn(-0.4, ridges_folded3, 0.0),
        p_fn(-0.35, ridges_folded8, 0.0),
        p_fn(-0.1, ridges_folded9, 0.0),
        p_fn(0.2, ridges_folded10, 0.0),
        p_fn(0.7, Arc::clone(&ridges_folded11), 0.0),
    ]);
    let erosion3 = er(vec![
        p_fn(-0.85, ridges_folded12, 0.0),
        p_fn(-0.7, ridges_folded13, 0.0),
        p_fn(-0.4, ridges_folded14, 0.0),
        p_fn(-0.35, ridges_folded15, 0.0),
        p_fn(-0.1, ridges_folded16, 0.0),
        p_fn(0.2, Arc::clone(&ridges_folded17), 0.0),
        p_fn(0.4, Arc::clone(&ridges_folded17), 0.0),
        p_fn(0.45, Arc::clone(&ridges_folded25), 0.0),
        p_fn(0.55, ridges_folded25, 0.0),
        p_fn(0.58, ridges_folded18, 0.0),
        p_fn(0.7, ridges_folded11, 0.0),
    ]);
    let erosion4 = er(vec![
        p_fn(-0.85, ridges_folded24, 0.0),
        p_fn(-0.7, Arc::clone(&ridges_folded19), 0.0),
        p_fn(-0.4, ridges_folded19, 0.0),
        p_fn(-0.35, ridges_folded20, 0.0),
        p_fn(-0.1, ridges_folded21, 0.0),
        p_fn(0.2, Arc::clone(&ridges_folded22), 0.0),
        p_fn(0.4, Arc::clone(&ridges_folded22), 0.0),
        p_fn(0.45, Arc::clone(&ridges_folded26), 0.0),
        p_fn(0.55, ridges_folded26, 0.0),
        p_fn(0.58, ridges_folded22, 0.0),
        p_fn(0.7, ridges_folded23, 0.0),
    ]);

    spline_from_points(
        continents,
        vec![
            p(-1.1, 0.044, 0.0),
            p(-1.02, -0.2222, 0.0),
            p(-0.51, -0.2222, 0.0),
            p(-0.44, -0.12, 0.0),
            p(-0.18, -0.12, 0.0),
            p_fn(-0.16, Arc::clone(&erosion1), 0.0),
            p_fn(-0.15, erosion1, 0.0),
            p_fn(-0.1, erosion2, 0.0),
            p_fn(0.25, erosion3, 0.0),
            p_fn(1.0, erosion4, 0.0),
        ],
    )
}
