//! Cubic spline density function.
//!
//! The generic parameter is used with a single concrete context type here:
//! `Point` is a reusable `FunctionContext` wrapper. Lifetime invariance of
//! trait objects prevents storing `C = dyn FunctionContext` as a struct
//! generic parameter, so this implementation is specialized to that use:
//! coordinate/value both accept `&dyn FunctionContext` via HRTB.

use super::function::FunctionContext;
use std::rc::Rc;

/// Spline value interface (here `C` is fixed to `dyn FunctionContext`).
pub trait CubicValue {
    /// Java: `double apply(C input)`.
    fn apply(&self, input: &dyn FunctionContext) -> f64;

    /// Java: `double minValue()`.
    fn min_value(&self) -> f64;

    /// Java: `double maxValue()`.
    fn max_value(&self) -> f64;
}

/// Java: `record Point<C>(double location, Value<C> value, double derivative)`(L107-108).
pub struct CubicPoint {
    /// Java: `public final double location`.
    pub location: f64,
    /// Java: `public final Value<C> value`.
    pub value: Rc<dyn CubicValue>,
    /// Java: `public final double derivative`.
    pub derivative: f64,
}

/// Cubic spline density function (specialized to `dyn FunctionContext`).
pub struct CubicSpline {
    /// Java: `private final ToDoubleFunction<C> coordinate`(L10).
    coordinate: Rc<dyn Fn(&dyn FunctionContext) -> f64>,
    /// Java: `private final double[] locations`(L11).
    locations: Box<[f64]>,
    /// Java: `private final Value<C>[] values`(L12).
    values: Vec<Rc<dyn CubicValue>>,
    /// Java: `private final double[] derivatives`(L13).
    derivatives: Box<[f64]>,
    /// Java: `private final double minValue`(L14).
    min_value: f64,
    /// Java: `private final double maxValue`(L15).
    max_value: f64,
}

impl CubicSpline {
    /// Java: `private CubicSpline(ToDoubleFunction<C>, List<Point<C>>)`(L18-43).
    fn new(coordinate: Rc<dyn Fn(&dyn FunctionContext) -> f64>, points: Vec<CubicPoint>) -> Self {
        assert!(points.len() >= 2, "CubicSpline needs at least two points");

        // Sort by ascending location.
        // `total_cmp` matches the total order with NaN sorted last.
        let mut sorted_points = points;
        sorted_points.sort_by(|a, b| a.location.total_cmp(&b.location));

        let point_count = sorted_points.len();
        let mut locations = vec![0.0; point_count];
        let mut values: Vec<Rc<dyn CubicValue>> = Vec::with_capacity(point_count);
        let mut derivatives = vec![0.0; point_count];

        // Copy points and aggregate min/max over each value's range.
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for i in 0..point_count {
            let point = &sorted_points[i];
            locations[i] = point.location;
            derivatives[i] = point.derivative;
            min = min.min(point.value.min_value());
            max = max.max(point.value.max_value());
            values.push(Rc::clone(&point.value));
        }

        Self {
            coordinate,
            locations: locations.into_boxed_slice(),
            values,
            derivatives: derivatives.into_boxed_slice(),
            min_value: min,
            max_value: max,
        }
    }

    /// Java: `public static <C> Builder<C> builder(ToDoubleFunction<C> coordinate)`(L45-47).
    pub fn builder(coordinate: Rc<dyn Fn(&dyn FunctionContext) -> f64>) -> CubicSplineBuilder {
        CubicSplineBuilder {
            coordinate,
            points: Vec::new(),
        }
    }

    /// Java: `public double apply(C input)`(L49-71).
    pub fn apply(&self, input: &dyn FunctionContext) -> f64 {
        let x = (self.coordinate)(input);
        let range = Self::find_range_for_location(&self.locations, x);
        if range < 0 {
            return self.values[0].apply(input);
        }

        let last = self.locations.len() - 1;
        if range == last as i32 {
            return self.values[last].apply(input);
        }

        // Cubic Hermite interpolation.
        let loc0 = self.locations[range as usize];
        let loc1 = self.locations[range as usize + 1];
        let loc_dist = loc1 - loc0;
        let k = (x - loc0) / loc_dist;
        let y0 = self.values[range as usize].apply(input);
        let y1 = self.values[range as usize + 1].apply(input);
        let y_dist = y1 - y0;
        let p = self.derivatives[range as usize] * loc_dist - y_dist;
        let q = -self.derivatives[range as usize + 1] * loc_dist + y_dist;
        y0 + k * y_dist + k * (1.0 - k) * (p + k * (q - p))
    }

    /// Java: `public double minValue()`(L73-75).
    pub fn min_value(&self) -> f64 {
        self.min_value
    }

    /// Java: `public double maxValue()`(L77-79).
    pub fn max_value(&self) -> f64 {
        self.max_value
    }

    /// Java: `private static int findRangeForLocation(double[], double)`(L81-97).
    ///
    /// Binary search: returns the last `i` with `locations[i] <= x` (`-1` if below the first).
    /// `length` is always non-negative, so the unsigned shift matches `>>` here.
    fn find_range_for_location(locations: &[f64], x: f64) -> i32 {
        let mut min: i32 = 0;
        let mut length: i32 = locations.len() as i32;

        while length > 0 {
            let half = ((length as u32) >> 1) as i32;
            let mid = min + half;
            if x < locations[mid as usize] {
                length = half;
            } else {
                min = mid + 1;
                length -= half + 1;
            }
        }

        min - 1
    }
}

/// Java: `public static final class Builder<C>`(L110-130).
pub struct CubicSplineBuilder {
    /// Java: `private final ToDoubleFunction<C> coordinate`(L111).
    coordinate: Rc<dyn Fn(&dyn FunctionContext) -> f64>,
    /// Java: `private final List<Point<C>> points`(L112).
    points: Vec<CubicPoint>,
}

impl CubicSplineBuilder {
    /// Java: `addPoint(double location, double value, double derivative)`(L118-120).
    pub fn add_point_const(&mut self, location: f64, value: f64, derivative: f64) -> &mut Self {
        self.add_point(location, Rc::new(ConstantValue { value }), derivative)
    }

    /// Java: `addPoint(double location, Value<C> value, double derivative)`(L122-125).
    pub fn add_point(
        &mut self,
        location: f64,
        value: Rc<dyn CubicValue>,
        derivative: f64,
    ) -> &mut Self {
        self.points.push(CubicPoint {
            location,
            value,
            derivative,
        });
        self
    }

    /// Java: `build()`(L127-129).
    pub fn build(self) -> CubicSpline {
        CubicSpline::new(self.coordinate, self.points)
    }
}

/// Java: `private record ConstantValue<C>(double value) implements Value<C>`(L136-151).
struct ConstantValue {
    value: f64,
}

impl CubicValue for ConstantValue {
    fn apply(&self, _input: &dyn FunctionContext) -> f64 {
        self.value
    }

    fn min_value(&self) -> f64 {
        self.value
    }

    fn max_value(&self) -> f64 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::super::function::SinglePointContext;
    use super::*;
    use std::cell::Cell;

    /// Coordinate function: reads the input `blockY`.
    fn identity_builder() -> CubicSplineBuilder {
        CubicSpline::builder(Rc::new(|ctx: &dyn FunctionContext| ctx.block_y() as f64))
    }

    /// Controllable coordinate: reads from a `Cell<f64>` for arbitrary sample points.
    fn cell_builder() -> (CubicSplineBuilder, Rc<Cell<f64>>) {
        let cell = Rc::new(Cell::new(0.0));
        let read = Rc::clone(&cell);
        let builder = CubicSpline::builder(Rc::new(move |_ctx: &dyn FunctionContext| read.get()));
        (builder, cell)
    }

    fn ctx(y: i32) -> SinglePointContext {
        SinglePointContext {
            block_x: 0,
            block_y: y,
            block_z: 0,
        }
    }

    /// Closed-form cubic Hermite basis (for cross-checking).
    fn hermite(k: f64, y0: f64, y1: f64, m0: f64, m1: f64, loc_dist: f64) -> f64 {
        let k2 = k * k;
        let k3 = k2 * k;
        let h00 = 2.0 * k3 - 3.0 * k2 + 1.0;
        let h01 = -2.0 * k3 + 3.0 * k2;
        let h10 = k3 - 2.0 * k2 + k;
        let h11 = k3 - k2;
        y0 * h00 + y1 * h01 + loc_dist * (m0 * h10 + m1 * h11)
    }

    #[test]
    fn needs_two_points() {
        let (mut b, _cell) = cell_builder();
        b.add_point_const(0.0, 1.0, 0.0);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            b.build();
        }));
        assert!(
            result.is_err(),
            "fewer than 2 points must panic (Java throws IllegalArgumentException)"
        );
    }

    #[test]
    fn zero_derivative_matches_hermite_basis() {
        // Two points, zero derivatives: cubic Hermite degrades to smoothstep.
        let (mut b, cell) = cell_builder();
        b.add_point_const(0.0, 0.0, 0.0);
        b.add_point_const(10.0, 2.0, 0.0);
        let s = b.build();
        for x in [2.5f64, 5.0, 7.5] {
            cell.set(x);
            let expected = hermite(x / 10.0, 0.0, 2.0, 0.0, 0.0, 10.0);
            assert!(
                (s.apply(&ctx(0)) - expected).abs() < 1e-12,
                "x={x}: got {} want {expected}",
                s.apply(&ctx(0))
            );
        }
        // At k=0.5 the correction term is exactly 0: value equals the midpoint.
        cell.set(5.0);
        assert!((s.apply(&ctx(0)) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn hermite_interpolation_with_derivatives() {
        // Two points with nonzero derivatives: H(k)=k(1-k), H'(0)=+1, H'(1)=-1.
        let (mut b, cell) = cell_builder();
        b.add_point_const(0.0, 0.0, 1.0);
        b.add_point_const(1.0, 0.0, -1.0);
        let s = b.build();
        cell.set(0.0);
        assert!((s.apply(&ctx(0))).abs() < 1e-12);
        cell.set(1.0);
        assert!((s.apply(&ctx(0))).abs() < 1e-12);
        cell.set(0.5);
        assert!((s.apply(&ctx(0)) - 0.25).abs() < 1e-12);
        // Finite-difference check of endpoint slopes: H'(0)=+1 (forward), H'(1)=-1 (backward).
        let h = 1e-7;
        cell.set(h);
        let d0 = (s.apply(&ctx(0)) - hermite(0.0, 0.0, 0.0, 1.0, -1.0, 1.0)) / h;
        cell.set(1.0 - h);
        let d1 = (hermite(1.0, 0.0, 0.0, 1.0, -1.0, 1.0) - s.apply(&ctx(0))) / h;
        assert!((d0 - 1.0).abs() < 1e-5, "d0={d0}");
        assert!((d1 + 1.0).abs() < 1e-5, "d1={d1}");
    }

    #[test]
    fn out_of_range_clamps_to_endpoint_values() {
        // Out-of-range inputs clamp to the endpoint values.
        let (mut b, cell) = cell_builder();
        b.add_point_const(0.0, 5.0, 0.0);
        b.add_point_const(1.0, 7.0, 0.0);
        let s = b.build();
        cell.set(-10.0);
        assert_eq!(s.apply(&ctx(0)), 5.0);
        cell.set(10.0);
        assert_eq!(s.apply(&ctx(0)), 7.0);
        // The boundary point itself takes the last value.
        cell.set(1.0);
        assert_eq!(s.apply(&ctx(0)), 7.0);
    }

    #[test]
    fn points_are_sorted_by_location() {
        // Construction sorts by location, independent of insertion order.
        let (mut b, cell) = cell_builder();
        b.add_point_const(1.0, 2.0, 0.0);
        b.add_point_const(-1.0, -2.0, 0.0);
        b.add_point_const(0.0, 0.0, 0.0);
        let s = b.build();
        cell.set(-1.0);
        assert_eq!(s.apply(&ctx(0)), -2.0);
        cell.set(1.0);
        assert_eq!(s.apply(&ctx(0)), 2.0);
        // Midpoint lands near 0 (symmetric setup).
        cell.set(0.0);
        assert!((s.apply(&ctx(0))).abs() < 1e-12);
    }

    #[test]
    fn nested_value_computes_on_same_input() {
        // Nested value evaluated on the same input.
        // Setup: location 0 maps DoubleY (=2*y), location 2 maps constant 4;
        // at ctx(y=1), k=0.5, y0=2, y1=4, zero derivatives give the smoothstep midpoint 3.
        struct DoubleY;
        impl CubicValue for DoubleY {
            fn apply(&self, input: &dyn FunctionContext) -> f64 {
                input.block_y() as f64 * 2.0
            }
            fn min_value(&self) -> f64 {
                -10.0
            }
            fn max_value(&self) -> f64 {
                10.0
            }
        }
        let mut b = identity_builder();
        b.add_point(0.0, Rc::new(DoubleY), 0.0);
        b.add_point_const(2.0, 4.0, 0.0);
        let s = b.build();
        assert!((s.apply(&ctx(1)) - 3.0).abs() < 1e-12);
        // Endpoints take their own values: y=0 gives DoubleY(0)=0.
        assert!((s.apply(&ctx(0))).abs() < 1e-12);
        // Min/max aggregate over each value's range.
        assert_eq!(s.min_value(), -10.0);
        assert_eq!(s.max_value(), 10.0);
    }
}
