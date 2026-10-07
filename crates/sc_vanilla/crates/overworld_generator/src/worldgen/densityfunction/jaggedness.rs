//! Overworld jaggedness density combining continents, erosion, and ridges.

use crate::worldgen::densityfunction::common::{
    add, blend_alpha, cache_2d, constant, flat_cache, mul, p, p_fn, spline_from_points,
};
use crate::worldgen::densityfunction::function::DensityFunction;
use std::sync::Arc;

/// Builds the overworld jaggedness function from the four inputs.
pub fn overworld_jaggedness(
    continents: Arc<dyn DensityFunction>,
    erosion: Arc<dyn DensityFunction>,
    ridges: Arc<dyn DensityFunction>,
    ridges_folded: Arc<dyn DensityFunction>,
) -> Arc<dyn DensityFunction> {
    flat_cache(cache_2d(add(
        constant(0.0),
        mul(
            blend_alpha(),
            add(
                constant(-0.0),
                jaggedness_spline(continents, erosion, ridges, ridges_folded),
            ),
        ),
    )))
}

/// Builds the nested jaggedness spline.
fn jaggedness_spline(
    continents: Arc<dyn DensityFunction>,
    erosion: Arc<dyn DensityFunction>,
    ridges: Arc<dyn DensityFunction>,
    ridges_folded: Arc<dyn DensityFunction>,
) -> Arc<dyn DensityFunction> {
    let r = |points| spline_from_points(Arc::clone(&ridges), points);

    let ridges1 = r(vec![p(-0.01, 0.63, 0.0), p(0.01, 0.3, 0.0)]);
    let ridges2 = r(vec![p(-0.01, 0.315, 0.0), p(0.01, 0.15, 0.0)]);
    let ridges3 = r(vec![p(-0.01, 0.315, 0.0), p(0.01, 0.15, 0.0)]);
    let ridges4 = r(vec![p(-0.01, 0.63, 0.0), p(0.01, 0.3, 0.0)]);
    let ridges5 = r(vec![p(-0.01, 0.63, 0.0), p(0.01, 0.3, 0.0)]);
    let ridges6 = r(vec![p(-0.01, 0.63, 0.0), p(0.01, 0.3, 0.0)]);

    let rf = |points| spline_from_points(Arc::clone(&ridges_folded), points);

    let ridges_folded1 = rf(vec![
        p(0.19999999, 0.0, 0.0),
        p(0.44999996, 0.0, 0.0),
        p_fn(1.0, ridges1, 0.0),
    ]);
    let ridges_folded2 = rf(vec![
        p(0.19999999, 0.0, 0.0),
        p(0.44999996, 0.0, 0.0),
        p_fn(1.0, ridges2, 0.0),
    ]);
    let ridges_folded3 = rf(vec![
        p(0.19999999, 0.0, 0.0),
        p(0.44999996, 0.0, 0.0),
        p_fn(1.0, ridges3, 0.0),
    ]);
    let ridges_folded4 = rf(vec![
        p(0.19999999, 0.0, 0.0),
        p_fn(0.44999996, ridges4, 0.0),
        p_fn(1.0, ridges5, 0.0),
    ]);
    let ridges_folded5 = rf(vec![
        p(0.19999999, 0.0, 0.0),
        p(0.44999996, 0.0, 0.0),
        p_fn(1.0, ridges6, 0.0),
    ]);

    let er = |points| spline_from_points(Arc::clone(&erosion), points);

    let erosion1 = er(vec![
        p_fn(-1.0, ridges_folded1, 0.0),
        p_fn(-0.78, ridges_folded2, 0.0),
        p_fn(-0.5775, ridges_folded3, 0.0),
        p(-0.375, 0.0, 0.0),
    ]);
    let erosion2 = er(vec![
        p_fn(-1.0, ridges_folded4, 0.0),
        p_fn(-0.78, Arc::clone(&ridges_folded5), 0.0),
        p_fn(-0.5775, ridges_folded5, 0.0),
        p(-0.375, 0.0, 0.0),
    ]);

    spline_from_points(
        continents,
        vec![
            p(-0.11, 0.0, 0.0),
            p_fn(0.03, erosion1, 0.0),
            p_fn(0.65, erosion2, 0.0),
        ],
    )
}
