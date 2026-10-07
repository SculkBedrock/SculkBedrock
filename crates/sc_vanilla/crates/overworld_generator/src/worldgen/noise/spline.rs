//! Port of the vanilla noise-spline package.
//!
//! - `SplineGenerator.java` → [`IEvaluator`] / [`Point`] / [`StaticValue`] / [`Spline`]
//! - `FactorSpline.java` → [`factor_spline`](Java `FactorSpline.CACHED_SPLINE`)
//! - `JaggednessSpline.java` → [`jaggedness_spline`](Java `JaggednessSpline.CACHED_SPLINE`)
//! - `OffsetSpline.java` → [`offset_spline`](Java `OffsetSpline.CACHED_SPLINE`)
//!
//! Reference sources live under `.fetch/`.
//!
//! Static initializers become `OnceLock` lazy singletons
//! (built once on first access, then shared).
//! Inner classes lift to module-level types;
//! shared child-spline references use `Arc<dyn IEvaluator>`.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

// ---------------------------------------------------------------------------
// IEvaluator interface as a trait.
// ---------------------------------------------------------------------------

/// Java: `SplineGenerator.IEvaluator`(L8-10).
pub trait IEvaluator: Send + Sync {
    /// Java: `double evaluate(Map<String, Double> parameters)`.
    fn evaluate(&self, parameters: &HashMap<String, f64>) -> f64;
}

// ---------------------------------------------------------------------------
// Point
// ---------------------------------------------------------------------------

/// Java: `SplineGenerator.Point`(L12-26).
pub struct Point {
    /// Java: `public final double location`.
    pub location: f64,
    /// Java: `public final IEvaluator value`.
    ///
    /// Both point constructor forms share one field type.
    pub value: Arc<dyn IEvaluator>,
    /// Java: `public final double derivative`.
    pub derivative: f64,
}

impl Point {
    /// Java: `Point(double location, IEvaluator value, double derivative)`(L17-21).
    ///
    /// Concrete types coerce to `Arc<dyn IEvaluator>` automatically.
    pub fn new(location: f64, value: Arc<dyn IEvaluator>, derivative: f64) -> Self {
        Self {
            location,
            value,
            derivative,
        }
    }

    /// Java: `Point(double location, double value, double derivative)`(L23-25)
    /// Delegates with a constant-value wrapper.
    pub fn new_static(location: f64, value: f64, derivative: f64) -> Self {
        Self::new(location, Arc::new(StaticValue { value }), derivative)
    }
}

// ---------------------------------------------------------------------------
// StaticValue
// ---------------------------------------------------------------------------

/// Java: `SplineGenerator.StaticValue`(L28-38).
pub struct StaticValue {
    /// Java: `private final double value`.
    pub(crate) value: f64,
}

impl IEvaluator for StaticValue {
    /// Ignores its argument and returns the constant.
    fn evaluate(&self, _parameters: &HashMap<String, f64>) -> f64 {
        self.value
    }
}

// ---------------------------------------------------------------------------
// Spline
// ---------------------------------------------------------------------------

/// Java: `SplineGenerator.Spline`(L40-86).
pub struct Spline {
    /// Java: `private final List<Point> points`.
    points: Vec<Point>,
    /// Java: `private final String coordinate`.
    coordinate: String,
}

impl Spline {
    /// Java: `Spline(String coordinate, List<Point> points)`(L44-50).
    ///
    /// Fewer than 2 points panics.
    pub fn new(coordinate: &str, points: Vec<Point>) -> Self {
        if points.len() < 2 {
            panic!("Spline needs at least two points");
        }
        Self {
            points,
            coordinate: coordinate.to_owned(),
        }
    }

    /// Java: `@Override evaluate(Map<String, Double>)`(L53-72).
    fn evaluate_impl(&self, parameters: &HashMap<String, f64>) -> f64 {
        // Java L54: parameters.getOrDefault(this.coordinate, 0.0)
        let input = parameters.get(&self.coordinate).copied().unwrap_or(0.0);

        // Linear scan for the bracketing pair, then Hermite interpolation.
        for i in 0..self.points.len() - 1 {
            let p0 = &self.points[i];
            let p1 = &self.points[i + 1];

            if input >= p0.location && input <= p1.location {
                let y0 = p0.value.evaluate(parameters);
                let y1 = p1.value.evaluate(parameters);
                return Self::hermite(
                    input,
                    p0.location,
                    y0,
                    p0.derivative,
                    p1.location,
                    y1,
                    p1.derivative,
                );
            }
        }

        // Out-of-range inputs clamp to the first/last value.
        if input < self.points[0].location {
            self.points[0].value.evaluate(parameters)
        } else {
            self.points[self.points.len() - 1]
                .value
                .evaluate(parameters)
        }
    }

    /// Java: `private double hermite(double x, double x0, double y0, double m0, double x1, double y1, double m1)`
    ///(L74-85).
    #[allow(clippy::too_many_arguments)]
    fn hermite(x: f64, x0: f64, y0: f64, m0: f64, x1: f64, y1: f64, m1: f64) -> f64 {
        let t = (x - x0) / (x1 - x0);
        let t2 = t * t;
        let t3 = t2 * t;

        // Integer arithmetic widens to double, same as the float form.
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;

        h00 * y0 + h10 * (x1 - x0) * m0 + h01 * y1 + h11 * (x1 - x0) * m1
    }

    /// Point count (test helper).
    #[cfg(test)]
    pub(crate) fn points_len(&self) -> usize {
        self.points.len()
    }
}

impl IEvaluator for Spline {
    fn evaluate(&self, parameters: &HashMap<String, f64>) -> f64 {
        self.evaluate_impl(parameters)
    }
}

// ---------------------------------------------------------------------------
// FactorSpline table (static initializer as a build function plus OnceLock).
// ---------------------------------------------------------------------------

/// Cached FactorSpline table.
///
/// Point constants match the upstream table value by value.
pub fn factor_spline() -> &'static Arc<Spline> {
    static CACHED: OnceLock<Arc<Spline>> = OnceLock::new();
    CACHED.get_or_init(|| Arc::new(build_factor_spline()))
}

fn build_factor_spline() -> Spline {
    // Ridges Splines
    let ridges1 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.2, 6.3, 0.0),
            Point::new_static(0.2, 6.25, 0.0),
        ],
    ));

    let ridges2 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.05, 6.3, 0.0),
            Point::new_static(0.05, 2.67, 0.0),
        ],
    ));

    let ridges3 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.2, 6.3, 0.0),
            Point::new_static(0.2, 6.25, 0.0),
        ],
    ));

    let ridges4 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.2, 6.3, 0.0),
            Point::new_static(0.2, 6.25, 0.0),
        ],
    ));

    let ridges5 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.05, 2.67, 0.0),
            Point::new_static(0.05, 6.3, 0.0),
        ],
    ));

    let ridges6 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.2, 6.3, 0.0),
            Point::new_static(0.2, 6.25, 0.0),
        ],
    ));

    let ridges7 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(0.0, 6.25, 0.0),
            Point::new_static(0.1, 0.625, 0.0),
        ],
    ));

    let ridges8 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.2, 6.3, 0.0),
            Point::new_static(0.2, 5.47, 0.0),
        ],
    ));

    let ridges9 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.2, 6.3, 0.0),
            Point::new_static(0.2, 5.08, 0.0),
        ],
    ));

    let ridges10 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.2, 6.3, 0.0),
            Point::new_static(0.2, 4.69, 0.0),
        ],
    ));

    let ridges11 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(0.0, 5.47, 0.0),
            Point::new_static(0.1, 0.625, 0.0),
        ],
    ));

    let ridges12 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(0.0, 5.08, 0.0),
            Point::new_static(0.1, 0.625, 0.0),
        ],
    ));

    // `ridges13` is defined but unreferenced upstream; kept for table parity.
    let _ridges13 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(0.0, 4.69, 0.0),
            Point::new_static(0.1, 0.625, 0.0),
        ],
    ));

    // Ridges Folded Splines
    let ridges_folded1 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-0.9, 6.25, 0.0),
            Point::new(-0.69, ridges7, 0.0),
        ],
    ));

    let ridges_folded2 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-0.9, 5.47, 0.0),
            Point::new(-0.69, ridges11, 0.0),
        ],
    ));

    let ridges_folded3 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-0.9, 5.08, 0.0),
            Point::new(-0.69, ridges12, 0.0),
        ],
    ));

    let ridges_folded4 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new(0.45, ridges10.clone(), 0.0),
            Point::new_static(0.7, 1.56, 0.0),
        ],
    ));

    let ridges_folded5 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new(-0.7, ridges10.clone(), 0.0),
            Point::new_static(-0.15, 1.37, 0.0),
        ],
    ));

    // Erosion Splines
    let erosion1 = Arc::new(Spline::new(
        "minecraft:overworld/erosion",
        vec![
            Point::new(-0.6, ridges1.clone(), 0.0),
            Point::new(-0.5, ridges2.clone(), 0.0),
            Point::new(-0.35, ridges3.clone(), 0.0),
            Point::new(-0.25, ridges4.clone(), 0.0),
            Point::new(-0.1, ridges5.clone(), 0.0),
            Point::new(0.03, ridges6.clone(), 0.0),
            Point::new_static(0.35, 6.25, 0.0),
            Point::new(0.45, ridges_folded1.clone(), 0.0),
            Point::new(0.55, ridges_folded1.clone(), 0.0),
            Point::new_static(0.62, 6.25, 0.0),
        ],
    ));

    let erosion2 = Arc::new(Spline::new(
        "minecraft:overworld/erosion",
        vec![
            Point::new(-0.6, ridges8.clone(), 0.0),
            Point::new(-0.5, ridges2.clone(), 0.0),
            Point::new(-0.35, ridges8.clone(), 0.0),
            Point::new(-0.25, ridges8.clone(), 0.0),
            Point::new(-0.1, ridges5.clone(), 0.0),
            Point::new(0.03, ridges8.clone(), 0.0),
            Point::new_static(0.35, 5.47, 0.0),
            Point::new(0.45, ridges_folded2.clone(), 0.0),
            Point::new(0.55, ridges_folded2.clone(), 0.0),
            Point::new_static(0.62, 5.47, 0.0),
        ],
    ));

    let erosion3 = Arc::new(Spline::new(
        "minecraft:overworld/erosion",
        vec![
            Point::new(-0.6, ridges9.clone(), 0.0),
            Point::new(-0.5, ridges2.clone(), 0.0),
            Point::new(-0.35, ridges9.clone(), 0.0),
            Point::new(-0.25, ridges9.clone(), 0.0),
            Point::new(-0.1, ridges5.clone(), 0.0),
            Point::new(0.03, ridges9.clone(), 0.0),
            Point::new_static(0.35, 5.08, 0.0),
            Point::new(0.45, ridges_folded3.clone(), 0.0),
            Point::new(0.55, ridges_folded3.clone(), 0.0),
            Point::new_static(0.62, 5.08, 0.0),
        ],
    ));

    let erosion4 = Arc::new(Spline::new(
        "minecraft:overworld/erosion",
        vec![
            Point::new(-0.6, ridges10.clone(), 0.0),
            Point::new(-0.5, ridges2, 0.0),
            Point::new(-0.35, ridges10.clone(), 0.0),
            Point::new(-0.25, ridges10.clone(), 0.0),
            Point::new(-0.1, ridges5, 0.0),
            Point::new(0.03, ridges10, 0.0),
            Point::new(0.05, ridges_folded4.clone(), 0.0),
            Point::new(0.4, ridges_folded4, 0.0),
            Point::new(0.45, ridges_folded5.clone(), 0.0),
            Point::new(0.55, ridges_folded5, 0.0),
            Point::new_static(0.58, 4.69, 0.0),
        ],
    ));

    Spline::new(
        "minecraft:overworld/continents",
        vec![
            Point::new_static(-0.19, 3.95, 0.0),
            Point::new(-0.15, erosion1, 0.0),
            Point::new(-0.1, erosion2, 0.0),
            Point::new(0.03, erosion3, 0.0),
            Point::new(0.06, erosion4, 0.0),
        ],
    )
}

// ---------------------------------------------------------------------------
// JaggednessSpline
// ---------------------------------------------------------------------------

/// Cached JaggednessSpline table.
///
/// Point constants match the upstream table value by value.
pub fn jaggedness_spline() -> &'static Arc<Spline> {
    static CACHED: OnceLock<Arc<Spline>> = OnceLock::new();
    CACHED.get_or_init(|| Arc::new(build_jaggedness_spline()))
}

fn build_jaggedness_spline() -> Spline {
    let ridges1 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.01, 0.63, 0.0),
            Point::new_static(0.01, 0.3, 0.0),
        ],
    ));

    let ridges2 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.01, 0.315, 0.0),
            Point::new_static(0.01, 0.15, 0.0),
        ],
    ));

    let ridges3 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.01, 0.315, 0.0),
            Point::new_static(0.01, 0.15, 0.0),
        ],
    ));

    let ridges4 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.01, 0.63, 0.0),
            Point::new_static(0.01, 0.3, 0.0),
        ],
    ));

    let ridges5 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.01, 0.63, 0.0),
            Point::new_static(0.01, 0.3, 0.0),
        ],
    ));

    let ridges6 = Arc::new(Spline::new(
        "minecraft:overworld/ridges",
        vec![
            Point::new_static(-0.01, 0.63, 0.0),
            Point::new_static(0.01, 0.3, 0.0),
        ],
    ));

    let ridges_folded1 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(0.19999999, 0.0, 0.0),
            Point::new_static(0.44999996, 0.0, 0.0),
            Point::new(1.0, ridges1, 0.0),
        ],
    ));

    let ridges_folded2 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(0.19999999, 0.0, 0.0),
            Point::new_static(0.44999996, 0.0, 0.0),
            Point::new(1.0, ridges2, 0.0),
        ],
    ));

    let ridges_folded3 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(0.19999999, 0.0, 0.0),
            Point::new_static(0.44999996, 0.0, 0.0),
            Point::new(1.0, ridges3, 0.0),
        ],
    ));

    let ridges_folded4 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(0.19999999, 0.0, 0.0),
            Point::new(0.44999996, ridges4, 0.0),
            Point::new(1.0, ridges5, 0.0),
        ],
    ));

    let ridges_folded5 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(0.19999999, 0.0, 0.0),
            Point::new_static(0.44999996, 0.0, 0.0),
            Point::new(1.0, ridges6, 0.0),
        ],
    ));

    let erosion1 = Arc::new(Spline::new(
        "minecraft:overworld/erosion",
        vec![
            Point::new(-1.0, ridges_folded1, 0.0),
            Point::new(-0.78, ridges_folded2.clone(), 0.0),
            Point::new(-0.5775, ridges_folded3, 0.0),
            Point::new_static(-0.375, 0.0, 0.0),
        ],
    ));

    let erosion2 = Arc::new(Spline::new(
        "minecraft:overworld/erosion",
        vec![
            Point::new(-1.0, ridges_folded4, 0.0),
            Point::new(-0.78, ridges_folded5.clone(), 0.0),
            Point::new(-0.5775, ridges_folded5, 0.0),
            Point::new_static(-0.375, 0.0, 0.0),
        ],
    ));

    Spline::new(
        "minecraft:overworld/continents",
        vec![
            Point::new_static(-0.11, 0.0, 0.0),
            Point::new(0.03, erosion1, 0.0),
            Point::new(0.65, erosion2, 0.0),
        ],
    )
}

// ---------------------------------------------------------------------------
// OffsetSpline
// ---------------------------------------------------------------------------

/// Cached OffsetSpline table.
///
/// Point constants match the upstream table value by value.
pub fn offset_spline() -> &'static Arc<Spline> {
    static CACHED: OnceLock<Arc<Spline>> = OnceLock::new();
    CACHED.get_or_init(|| Arc::new(build_offset_spline()))
}

fn build_offset_spline() -> Spline {
    let ridges_folded1 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.08880186, 0.38940096),
            Point::new_static(1.0, 0.69000006, 0.38940096),
        ],
    ));
    let ridges_folded2 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.115760356, 0.37788022),
            Point::new_static(1.0, 0.6400001, 0.37788022),
        ],
    ));
    let ridges_folded3 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.2222, 0.0),
            Point::new_static(-0.75, -0.2222, 0.0),
            Point::new_static(-0.65, 0.0, 0.0),
            Point::new_static(0.5954547, 2.9802322E-8, 0.0),
            Point::new_static(0.6054547, 2.9802322E-8, 0.2534563),
            Point::new_static(1.0, 0.100000024, 0.2534563),
        ],
    ));
    let ridges_folded4 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.3, 0.5),
            Point::new_static(-0.4, 0.05, 0.0),
            Point::new_static(0.0, 0.05, 0.0),
            Point::new_static(0.4, 0.05, 0.0),
            Point::new_static(1.0, 0.060000002, 0.007000001),
        ],
    ));
    let ridges_folded5 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.15, 0.5),
            Point::new_static(-0.4, 0.0, 0.0),
            Point::new_static(0.0, 0.0, 0.0),
            Point::new_static(0.4, 0.05, 0.1),
            Point::new_static(1.0, 0.060000002, 0.007000001),
        ],
    ));
    let ridges_folded6 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.15, 0.5),
            Point::new_static(-0.4, 0.0, 0.0),
            Point::new_static(0.0, 0.0, 0.0),
            Point::new_static(0.4, 0.0, 0.0),
            Point::new_static(1.0, 0.0, 0.0),
        ],
    ));
    let ridges_folded7 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.02, 0.0),
            Point::new_static(-0.4, -0.03, 0.0),
            Point::new_static(0.0, -0.03, 0.0),
            Point::new_static(0.4, 0.0, 0.06),
            Point::new_static(1.0, 0.0, 0.0),
        ],
    ));
    let ridges_folded8 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.25, 0.5),
            Point::new_static(-0.4, 0.05, 0.0),
            Point::new_static(0.0, 0.05, 0.0),
            Point::new_static(0.4, 0.05, 0.0),
            Point::new_static(1.0, 0.060000002, 0.007000001),
        ],
    ));
    let ridges_folded9 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.1, 0.5),
            Point::new_static(-0.4, 0.001, 0.01),
            Point::new_static(0.0, 0.003, 0.01),
            Point::new_static(0.4, 0.05, 0.094000004),
            Point::new_static(1.0, 0.060000002, 0.007000001),
        ],
    ));
    let ridges_folded10 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.1, 0.5),
            Point::new_static(-0.4, 0.01, 0.0),
            Point::new_static(0.0, 0.01, 0.0),
            Point::new_static(0.4, 0.03, 0.04),
            Point::new_static(1.0, 0.1, 0.049),
        ],
    ));
    let ridges_folded11 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.02, 0.015),
            Point::new_static(-0.4, 0.01, 0.0),
            Point::new_static(0.0, 0.01, 0.0),
            Point::new_static(0.4, 0.03, 0.04),
            Point::new_static(1.0, 0.1, 0.049),
        ],
    ));
    let ridges_folded12 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, 0.20235021, 0.0),
            Point::new_static(0.0, 0.7161751, 0.5138249),
            Point::new_static(1.0, 1.23, 0.5138249),
        ],
    ));
    let ridges_folded13 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, 0.2, 0.0),
            Point::new_static(0.0, 0.44682026, 0.43317974),
            Point::new_static(1.0, 0.88, 0.43317974),
        ],
    ));
    let ridges_folded14 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, 0.2, 0.0),
            Point::new_static(0.0, 0.30829495, 0.3917051),
            Point::new_static(1.0, 0.70000005, 0.3917051),
        ],
    ));
    let ridges_folded15 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.25, 0.5),
            Point::new_static(-0.4, 0.35, 0.0),
            Point::new_static(0.0, 0.35, 0.0),
            Point::new_static(0.4, 0.35, 0.0),
            Point::new_static(1.0, 0.42000002, 0.049000014),
        ],
    ));
    let ridges_folded16 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.1, 0.5),
            Point::new_static(-0.4, 0.0069999998, 0.07),
            Point::new_static(0.0, 0.021, 0.07),
            Point::new_static(0.4, 0.35, 0.658),
            Point::new_static(1.0, 0.42000002, 0.049000014),
        ],
    ));
    let ridges_folded17 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.1, 0.5),
            Point::new_static(-0.4, 0.01, 0.0),
            Point::new_static(0.0, 0.01, 0.0),
            Point::new_static(0.4, 0.03, 0.04),
            Point::new_static(1.0, 0.1, 0.049),
        ],
    ));
    let ridges_folded18 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.05, 0.5),
            Point::new_static(-0.4, 0.01, 0.0),
            Point::new_static(0.0, 0.01, 0.0),
            Point::new_static(0.4, 0.03, 0.04),
            Point::new_static(1.0, 0.1, 0.049),
        ],
    ));
    let ridges_folded19 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, 0.2, 0.0),
            Point::new_static(0.0, 0.5391705, 0.4608295),
            Point::new_static(1.0, 1.0, 0.4608295),
        ],
    ));
    let ridges_folded20 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.2, 0.5),
            Point::new_static(-0.4, 0.5, 0.0),
            Point::new_static(0.0, 0.5, 0.0),
            Point::new_static(0.4, 0.5, 0.0),
            Point::new_static(1.0, 0.6, 0.070000015),
        ],
    ));
    let ridges_folded21 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.05, 0.5),
            Point::new_static(-0.4, 0.01, 0.099999994),
            Point::new_static(0.0, 0.03, 0.099999994),
            Point::new_static(0.4, 0.5, 0.94),
            Point::new_static(1.0, 0.6, 0.070000015),
        ],
    ));
    let ridges_folded22 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.05, 0.5),
            Point::new_static(-0.4, 0.01, 0.0),
            Point::new_static(0.0, 0.01, 0.0),
            Point::new_static(0.4, 0.03, 0.04),
            Point::new_static(1.0, 0.1, 0.049),
        ],
    ));
    let ridges_folded23 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.02, 0.015),
            Point::new_static(-0.4, 0.01, 0.0),
            Point::new_static(0.0, 0.01, 0.0),
            Point::new_static(0.4, 0.03, 0.04),
            Point::new_static(1.0, 0.1, 0.049),
        ],
    ));
    let ridges_folded24 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, 0.34792626, 0.0),
            Point::new_static(0.0, 0.9239631, 0.5760369),
            Point::new_static(1.0, 1.5, 0.5760369),
        ],
    ));
    let ridges_folded25 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.1, 0.0),
            Point::new_static(-0.4, 0.1, 0.0),
            Point::new_static(0.0, 0.17, 0.0),
        ],
    ));
    let ridges_folded26 = Arc::new(Spline::new(
        "minecraft:overworld/ridges_folded",
        vec![
            Point::new_static(-1.0, -0.05, 0.0),
            Point::new_static(-0.4, 0.1, 0.0),
            Point::new_static(0.0, 0.17, 0.0),
        ],
    ));

    let erosion1 = Arc::new(Spline::new(
        "minecraft:overworld/erosion",
        vec![
            Point::new(-0.85, ridges_folded1.clone(), 0.0),
            Point::new(-0.7, ridges_folded2.clone(), 0.0),
            Point::new(-0.4, ridges_folded3.clone(), 0.0),
            Point::new(-0.35, ridges_folded4.clone(), 0.0),
            Point::new(-0.1, ridges_folded5.clone(), 0.0),
            Point::new(0.2, ridges_folded6.clone(), 0.0),
            Point::new(0.7, ridges_folded7, 0.0),
        ],
    ));
    let erosion2 = Arc::new(Spline::new(
        "minecraft:overworld/erosion",
        vec![
            Point::new(-0.85, ridges_folded1, 0.0),
            Point::new(-0.7, ridges_folded2, 0.0),
            Point::new(-0.4, ridges_folded3, 0.0),
            Point::new(-0.35, ridges_folded8.clone(), 0.0),
            Point::new(-0.1, ridges_folded9.clone(), 0.0),
            Point::new(0.2, ridges_folded10.clone(), 0.0),
            Point::new(0.7, ridges_folded11.clone(), 0.0),
        ],
    ));
    let erosion3 = Arc::new(Spline::new(
        "minecraft:overworld/erosion",
        vec![
            Point::new(-0.85, ridges_folded12, 0.0),
            Point::new(-0.7, ridges_folded13, 0.0),
            Point::new(-0.4, ridges_folded14, 0.0),
            Point::new(-0.35, ridges_folded15, 0.0),
            Point::new(-0.1, ridges_folded16, 0.0),
            Point::new(0.2, ridges_folded17.clone(), 0.0),
            Point::new(0.4, ridges_folded17, 0.0),
            Point::new(0.45, ridges_folded25.clone(), 0.0),
            Point::new(0.55, ridges_folded25, 0.0),
            Point::new(0.58, ridges_folded18, 0.0),
            Point::new(0.7, ridges_folded11, 0.0),
        ],
    ));
    let erosion4 = Arc::new(Spline::new(
        "minecraft:overworld/erosion",
        vec![
            Point::new(-0.85, ridges_folded24, 0.0),
            Point::new(-0.7, ridges_folded19.clone(), 0.0),
            Point::new(-0.4, ridges_folded19, 0.0),
            Point::new(-0.35, ridges_folded20, 0.0),
            Point::new(-0.1, ridges_folded21, 0.0),
            Point::new(0.2, ridges_folded22.clone(), 0.0),
            Point::new(0.4, ridges_folded22.clone(), 0.0),
            Point::new(0.45, ridges_folded26.clone(), 0.0),
            Point::new(0.55, ridges_folded26, 0.0),
            Point::new(0.58, ridges_folded22, 0.0),
            Point::new(0.7, ridges_folded23, 0.0),
        ],
    ));

    Spline::new(
        "minecraft:overworld/continents",
        vec![
            Point::new_static(-1.1, 0.044, 0.0),
            Point::new_static(-1.02, -0.2222, 0.0),
            Point::new_static(-0.51, -0.2222, 0.0),
            Point::new_static(-0.44, -0.12, 0.0),
            Point::new_static(-0.18, -0.12, 0.0),
            Point::new(-0.16, erosion1.clone(), 0.0),
            Point::new(-0.15, erosion1, 0.0),
            Point::new(-0.1, erosion2, 0.0),
            Point::new(0.25, erosion3, 0.0),
            Point::new(1.0, erosion4, 0.0),
        ],
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn params(entries: &[(&str, f64)]) -> HashMap<String, f64> {
        entries.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
    }

    #[test]
    fn static_value_returns_constant() {
        let sv = StaticValue { value: 1.25 };
        assert_eq!(sv.evaluate(&params(&[])), 1.25);
        assert_eq!(sv.evaluate(&params(&[("anything", 9.0)])), 1.25);
    }

    #[test]
    fn spline_endpoint_hit_returns_static_values() {
        // Inputs exactly on endpoints yield endpoint values.
        let spline = Spline::new(
            "c",
            vec![
                Point::new_static(0.0, 3.0, 0.0),
                Point::new_static(1.0, 7.0, 0.0),
            ],
        );
        assert_eq!(spline.evaluate_impl(&params(&[("c", 0.0)])), 3.0);
        assert_eq!(spline.evaluate_impl(&params(&[("c", 1.0)])), 7.0);
    }

    #[test]
    fn spline_out_of_range_returns_end_values() {
        let spline = Spline::new(
            "c",
            vec![
                Point::new_static(1.0, 5.0, 0.0),
                Point::new_static(2.0, 6.0, 0.0),
            ],
        );
        // Below the first point yields the first value.
        assert_eq!(spline.evaluate_impl(&params(&[("c", -100.0)])), 5.0);
        // Above the last point yields the last value.
        assert_eq!(spline.evaluate_impl(&params(&[("c", 100.0)])), 6.0);
    }

    #[test]
    fn spline_missing_coordinate_defaults_to_zero() {
        // Empty maps default to 0.0 (below the first point, so first value).
        let spline = Spline::new(
            "c",
            vec![
                Point::new_static(1.0, 5.0, 0.0),
                Point::new_static(2.0, 6.0, 0.0),
            ],
        );
        assert_eq!(spline.evaluate_impl(&params(&[])), 5.0);
    }

    #[test]
    fn spline_midpoint_hermite_value() {
        // Flat tangents give the midpoint 5.0; m0=1 shifts it to 5.125.
        let flat = Spline::new(
            "c",
            vec![
                Point::new_static(0.0, 0.0, 0.0),
                Point::new_static(1.0, 10.0, 0.0),
            ],
        );
        assert_eq!(flat.evaluate_impl(&params(&[("c", 0.5)])), 5.0);

        let sloped = Spline::new(
            "c",
            vec![
                Point::new_static(0.0, 0.0, 1.0),
                Point::new_static(1.0, 10.0, 0.0),
            ],
        );
        assert_eq!(sloped.evaluate_impl(&params(&[("c", 0.5)])), 5.125);
    }

    #[test]
    fn nested_spline_composition() {
        // Nested splines evaluate inner first, then outer.
        // input(outer)=1.0 selects y1 = inner result (100.0 at input 0.0).
        let inner = Arc::new(Spline::new(
            "inner",
            vec![
                Point::new_static(0.0, 100.0, 0.0),
                Point::new_static(1.0, 200.0, 0.0),
            ],
        ));
        let outer = Spline::new(
            "outer",
            vec![
                Point::new_static(0.0, 3.0, 0.0),
                Point::new(1.0, inner, 0.0),
            ],
        );
        assert_eq!(outer.evaluate_impl(&params(&[("outer", 1.0)])), 100.0);
    }

    #[test]
    fn spline_needs_two_points() {
        let result = std::panic::catch_unwind(|| {
            let _ = Spline::new("c", vec![Point::new_static(0.0, 1.0, 0.0)]);
        });
        assert!(result.is_err(), "Spline with 1 point must panic");
    }

    #[test]
    fn cached_splines_point_counts() {
        // Cached tables hold 5, 3, and 10 points respectively.
        assert_eq!(factor_spline().points_len(), 5);
        assert_eq!(jaggedness_spline().points_len(), 3);
        assert_eq!(offset_spline().points_len(), 10);
    }

    #[test]
    fn cached_splines_endpoint_values() {
        let mut p = HashMap::new();
        // FactorSpline first point is a static value.
        p.insert("minecraft:overworld/continents".to_owned(), -0.19);
        assert_eq!(factor_spline().evaluate_impl(&p), 3.95);

        // OffsetSpline clamps below its first point.
        p.insert("minecraft:overworld/continents".to_owned(), -1.1);
        assert_eq!(offset_spline().evaluate_impl(&p), 0.044);
        p.insert("minecraft:overworld/continents".to_owned(), -2.0);
        assert_eq!(offset_spline().evaluate_impl(&p), 0.044);

        // JaggednessSpline first point is a static value.
        p.insert("minecraft:overworld/continents".to_owned(), -0.11);
        assert_eq!(jaggedness_spline().evaluate_impl(&p), 0.0);
    }

    #[test]
    fn jaggedness_missing_params_cascade_zero() {
        // Empty maps evaluate the full tree to 0.0 through zero segments.
        assert_eq!(jaggedness_spline().evaluate_impl(&params(&[])), 0.0);
    }

    #[test]
    fn cached_splines_evaluate_finite() {
        // Whole-tree evaluation stays finite across parameter sets.
        for &c in &[-1.5, -0.9, -0.5, -0.05, 0.0, 0.2, 0.5, 0.99, 1.5] {
            for &e in &[-1.0, -0.6, -0.3, 0.0, 0.4, 0.8] {
                for &r in &[-1.0, -0.5, 0.0, 0.5, 1.0] {
                    for &rf in &[-1.0, 0.0, 1.0] {
                        let p = params(&[
                            ("minecraft:overworld/continents", c),
                            ("minecraft:overworld/erosion", e),
                            ("minecraft:overworld/ridges", r),
                            ("minecraft:overworld/ridges_folded", rf),
                        ]);
                        for spline in [factor_spline(), jaggedness_spline(), offset_spline()] {
                            let v = spline.evaluate_impl(&p);
                            assert!(
                                v.is_finite(),
                                "non-finite at c={c} e={e} r={r} rf={rf}: {v}"
                            );
                        }
                    }
                }
            }
        }
    }
}
