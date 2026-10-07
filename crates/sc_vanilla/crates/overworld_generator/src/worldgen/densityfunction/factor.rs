//! Overworld factor density combining continents, erosion, and ridges.

use crate::worldgen::densityfunction::common::{
    add, blend_alpha, cache_2d, constant, flat_cache, mul, p, p_fn, spline_from_points,
};
use crate::worldgen::densityfunction::function::DensityFunction;
use std::sync::Arc;

/// Builds the overworld factor function from the four inputs.
pub fn overworld_factor(
    continents: Arc<dyn DensityFunction>,
    erosion: Arc<dyn DensityFunction>,
    ridges: Arc<dyn DensityFunction>,
    ridges_folded: Arc<dyn DensityFunction>,
) -> Arc<dyn DensityFunction> {
    flat_cache(cache_2d(add(
        constant(10.0),
        mul(
            blend_alpha(),
            add(
                constant(-10.0),
                factor_spline(continents, erosion, ridges, ridges_folded),
            ),
        ),
    )))
}

/// Builds the nested factor spline.
fn factor_spline(
    continents: Arc<dyn DensityFunction>,
    erosion: Arc<dyn DensityFunction>,
    ridges: Arc<dyn DensityFunction>,
    ridges_folded: Arc<dyn DensityFunction>,
) -> Arc<dyn DensityFunction> {
    let r = |points| spline_from_points(Arc::clone(&ridges), points);

    let ridges_a = r(vec![p(-0.2, 6.3, 0.0), p(0.2, 6.25, 0.0)]);
    let ridges_b = r(vec![p(-0.05, 6.3, 0.0), p(0.05, 2.67, 0.0)]);
    let ridges_c = r(vec![p(-0.05, 2.67, 0.0), p(0.05, 6.3, 0.0)]);
    let ridges_d = r(vec![p(-0.2, 6.3, 0.0), p(0.2, 5.47, 0.0)]);
    let ridges_e = r(vec![p(-0.2, 6.3, 0.0), p(0.2, 5.08, 0.0)]);
    let ridges_f = r(vec![p(-0.2, 6.3, 0.0), p(0.2, 4.69, 0.0)]);
    let ridges_g = r(vec![p(0.0, 6.25, 0.0), p(0.1, 0.625, 0.0)]);
    let ridges_h = r(vec![p(0.0, 5.47, 0.0), p(0.1, 0.625, 0.0)]);
    let ridges_i = r(vec![p(0.0, 5.08, 0.0), p(0.1, 0.625, 0.0)]);

    let rf = |points| spline_from_points(Arc::clone(&ridges_folded), points);

    let ridges_folded_a = rf(vec![
        p(-0.9, 6.25, 0.0),
        p_fn(-0.69, Arc::clone(&ridges_g), 0.0),
    ]);
    let ridges_folded_b = rf(vec![
        p(-0.9, 5.47, 0.0),
        p_fn(-0.69, Arc::clone(&ridges_h), 0.0),
    ]);
    let ridges_folded_c = rf(vec![
        p(-0.9, 5.08, 0.0),
        p_fn(-0.69, Arc::clone(&ridges_i), 0.0),
    ]);
    let ridges_folded_d = rf(vec![
        p_fn(0.45, Arc::clone(&ridges_f), 0.0),
        p(0.7, 1.56, 0.0),
    ]);
    let ridges_folded_e = rf(vec![
        p_fn(-0.7, Arc::clone(&ridges_f), 0.0),
        p(-0.15, 1.37, 0.0),
    ]);

    let er = |points| spline_from_points(Arc::clone(&erosion), points);

    let erosion1 = er(vec![
        p_fn(-0.6, Arc::clone(&ridges_a), 0.0),
        p_fn(-0.5, Arc::clone(&ridges_b), 0.0),
        p_fn(-0.35, Arc::clone(&ridges_a), 0.0),
        p_fn(-0.25, Arc::clone(&ridges_a), 0.0),
        p_fn(-0.1, Arc::clone(&ridges_c), 0.0),
        p_fn(0.03, Arc::clone(&ridges_a), 0.0),
        p(0.35, 6.25, 0.0),
        p_fn(0.45, Arc::clone(&ridges_folded_a), 0.0),
        p_fn(0.55, Arc::clone(&ridges_folded_a), 0.0),
        p(0.62, 6.25, 0.0),
    ]);
    let erosion2 = er(vec![
        p_fn(-0.6, Arc::clone(&ridges_d), 0.0),
        p_fn(-0.5, Arc::clone(&ridges_b), 0.0),
        p_fn(-0.35, Arc::clone(&ridges_d), 0.0),
        p_fn(-0.25, Arc::clone(&ridges_d), 0.0),
        p_fn(-0.1, Arc::clone(&ridges_c), 0.0),
        p_fn(0.03, Arc::clone(&ridges_d), 0.0),
        p(0.35, 5.47, 0.0),
        p_fn(0.45, Arc::clone(&ridges_folded_b), 0.0),
        p_fn(0.55, Arc::clone(&ridges_folded_b), 0.0),
        p(0.62, 5.47, 0.0),
    ]);
    let erosion3 = er(vec![
        p_fn(-0.6, Arc::clone(&ridges_e), 0.0),
        p_fn(-0.5, Arc::clone(&ridges_b), 0.0),
        p_fn(-0.35, Arc::clone(&ridges_e), 0.0),
        p_fn(-0.25, Arc::clone(&ridges_e), 0.0),
        p_fn(-0.1, Arc::clone(&ridges_c), 0.0),
        p_fn(0.03, Arc::clone(&ridges_e), 0.0),
        p(0.35, 5.08, 0.0),
        p_fn(0.45, Arc::clone(&ridges_folded_c), 0.0),
        p_fn(0.55, Arc::clone(&ridges_folded_c), 0.0),
        p(0.62, 5.08, 0.0),
    ]);
    let erosion4 = er(vec![
        p_fn(-0.6, Arc::clone(&ridges_f), 0.0),
        p_fn(-0.5, ridges_b, 0.0),
        p_fn(-0.35, Arc::clone(&ridges_f), 0.0),
        p_fn(-0.25, Arc::clone(&ridges_f), 0.0),
        p_fn(-0.1, Arc::clone(&ridges_c), 0.0),
        p_fn(0.03, Arc::clone(&ridges_f), 0.0),
        p_fn(0.05, Arc::clone(&ridges_folded_d), 0.0),
        p_fn(0.4, ridges_folded_d, 0.0),
        p_fn(0.45, Arc::clone(&ridges_folded_e), 0.0),
        p_fn(0.55, ridges_folded_e, 0.0),
        p(0.58, 4.69, 0.0),
    ]);

    spline_from_points(
        continents,
        vec![
            p(-0.19, 3.95, 0.0),
            p_fn(-0.15, erosion1, 0.0),
            p_fn(-0.1, erosion2, 0.0),
            p_fn(0.03, erosion3, 0.0),
            p_fn(0.06, erosion4, 0.0),
        ],
    )
}
