//! Density-function combinators (`DensityCommon`).
//!
//! Port notes:
//! - Static factories become free functions (`add`/`mul`/`constant`/...); interface
//!   defaults (`clamp`/`abs`/...) are free functions too.
//! - Per-chunk `Marker` state keys off the marker global id
//!   inside [`ChunkCache`] (see function.rs).
//! - Fallback state (non-ChunkCacheContext) lives in the marker
//!   instance `RefCell` (single-threaded semantics).
//! - Java `ChunkCacheContext`/`CellFunctionContext`/`MutableChunkCacheContext`
//!   [`ChunkCache`]/[`ChunkCacheContext`] live in function.rs;
//!   this module implements them.
//! - `chunkCache`/`releaseChunkCache` couple to chunk infrastructure and land
//!   with the chunk adapter layer.

use super::cubic_spline::{CubicSpline, CubicValue};
use super::function::{
    ChunkCache, ChunkCacheContext, ContextProvider, DensityFunction, FunctionContext,
    MutableFunctionContext, NoiseHolder,
};
use crate::worldgen::math::clamp_f64;
use crate::worldgen::noise::noise::NormalNoise;
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Marker identity and state access.
// ---------------------------------------------------------------------------

/// Globally unique marker instance id (explicit numbering avoids
/// false hits from address reuse).
static MARKER_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

fn next_marker_id() -> u64 {
    MARKER_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Java: `Marker.state(FunctionContext, ThreadLocal<S>, Supplier<S>)`(L651-656).
///
/// Per-chunk state for ChunkCacheContext, marker-local state otherwise.
/// Borrowing: chunk state clones the `Rc` then releases the map borrow,
/// so nested markers never double-borrow one cache.
fn with_marker_state<S, R>(
    marker_id: u64,
    context: &dyn FunctionContext,
    local: &RefCell<S>,
    f: impl FnOnce(&mut S) -> R,
) -> R
where
    S: Default + 'static,
{
    if let Some(chunk_cache_context) = context.as_chunk_cache_context() {
        let cache = chunk_cache_context.density_chunk_cache();
        let state = cache.get_or_create::<S>(marker_id);
        let mut s = state.borrow_mut();
        f(&mut s)
    } else {
        let mut s = local.borrow_mut();
        f(&mut s)
    }
}

// ---------------------------------------------------------------------------
// CellFunctionContext / MutableChunkCacheContext(Java L36-72, L1024-1061)
// ---------------------------------------------------------------------------

/// Java: `final class CellFunctionContext implements ChunkCacheContext`(L36-72).
pub struct CellFunctionContext {
    cache: Rc<ChunkCache>,
    world_x: Cell<i32>,
    world_y: Cell<i32>,
    world_z: Cell<i32>,
}

impl CellFunctionContext {
    /// Java: `CellFunctionContext(ChunkCache chunkCache)`(L42-44).
    pub fn new(cache: Rc<ChunkCache>) -> Self {
        Self {
            cache,
            world_x: Cell::new(0),
            world_y: Cell::new(0),
            world_z: Cell::new(0),
        }
    }

    /// Set the cursor position (builder style, returns this).
    pub fn set(&self, world_x: i32, world_y: i32, world_z: i32) -> &Self {
        self.world_x.set(world_x);
        self.world_y.set(world_y);
        self.world_z.set(world_z);
        self
    }
}

impl FunctionContext for CellFunctionContext {
    fn block_x(&self) -> i32 {
        self.world_x.get()
    }

    fn block_y(&self) -> i32 {
        self.world_y.get()
    }

    fn block_z(&self) -> i32 {
        self.world_z.get()
    }

    fn as_chunk_cache_context(&self) -> Option<&dyn ChunkCacheContext> {
        Some(self)
    }
}

impl ChunkCacheContext for CellFunctionContext {
    fn density_chunk_cache(&self) -> Rc<ChunkCache> {
        Rc::clone(&self.cache)
    }
}

/// Java: `final class MutableChunkCacheContext implements ChunkCacheContext`(L1024-1061).
pub(crate) struct MutableChunkCacheContext {
    cache: RefCell<Option<Rc<ChunkCache>>>,
    block_x: Cell<i32>,
    block_y: Cell<i32>,
    block_z: Cell<i32>,
}

impl MutableChunkCacheContext {
    pub(crate) fn new() -> Self {
        Self {
            cache: RefCell::new(None),
            block_x: Cell::new(0),
            block_y: Cell::new(0),
            block_z: Cell::new(0),
        }
    }

    /// Attach a cache (builder style, returns this).
    fn with_cache(&self, cache: Rc<ChunkCache>) -> &Self {
        self.cache.replace(Some(cache));
        self
    }

    /// Set the cursor position (builder style, returns this).
    fn set(&self, block_x: i32, block_y: i32, block_z: i32) -> &Self {
        self.block_x.set(block_x);
        self.block_y.set(block_y);
        self.block_z.set(block_z);
        self
    }
}

impl FunctionContext for MutableChunkCacheContext {
    fn block_x(&self) -> i32 {
        self.block_x.get()
    }

    fn block_y(&self) -> i32 {
        self.block_y.get()
    }

    fn block_z(&self) -> i32 {
        self.block_z.get()
    }

    fn as_chunk_cache_context(&self) -> Option<&dyn ChunkCacheContext> {
        Some(self)
    }
}

impl ChunkCacheContext for MutableChunkCacheContext {
    fn density_chunk_cache(&self) -> Rc<ChunkCache> {
        self.cache
            .borrow()
            .clone()
            .expect("MutableChunkCacheContext used without cache")
    }
}

// ---------------------------------------------------------------------------
// Constant(Java L240-262)
// ---------------------------------------------------------------------------

/// Java: `record Constant(double value) implements SimpleFunction`(L240-262).
pub struct Constant {
    pub value: f64,
}

impl DensityFunction for Constant {
    fn compute(&self, _context: &dyn FunctionContext) -> f64 {
        self.value
    }

    fn fill_array(&self, output: &mut [f64], _context_provider: &dyn ContextProvider) {
        // Java L249-251:Arrays.fill.
        output.fill(self.value);
    }

    fn min_value(&self) -> f64 {
        self.value
    }

    fn max_value(&self) -> f64 {
        self.value
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Zero constant singleton.
///
/// Static singletons cannot cache globally here (this tree is single-threaded
/// `Rc`/`RefCell`, not `Send`/`Sync`); `Constant` allocates directly instead.
pub fn zero() -> Arc<dyn DensityFunction> {
    Arc::new(Constant { value: 0.0 })
}

/// Java: `constant(double)`(L199-201).
pub fn constant(value: f64) -> Arc<dyn DensityFunction> {
    Arc::new(Constant { value })
}

// ---------------------------------------------------------------------------
// Shared Y implementation (two identical private upstream SimpleFunctions
// merge into one here).
// ---------------------------------------------------------------------------

/// Java: `private static final DensityFunction Y = new SimpleFunction() {...}`.
pub struct YFunction;

impl DensityFunction for YFunction {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        context.block_y() as f64
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        context_provider.fill_all_directly(output, self);
    }

    fn min_value(&self) -> f64 {
        f64::NEG_INFINITY
    }

    fn max_value(&self) -> f64 {
        f64::INFINITY
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Matches the private static `Y` instances (fresh each time, same semantics).
pub fn y() -> Arc<dyn DensityFunction> {
    Arc::new(YFunction)
}

// ---------------------------------------------------------------------------
// Clamp.
// ---------------------------------------------------------------------------

/// Java: `record Clamp(DensityFunction input, double min, double max)`(L264-279).
pub struct Clamp {
    pub input: Arc<dyn DensityFunction>,
    pub min: f64,
    pub max: f64,
}

impl DensityFunction for Clamp {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        clamp_f64(self.input.compute(context), self.min, self.max)
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        // PureTransformer: fill input first, then transform elementwise.
        self.input.fill_array(output, context_provider);
        for value in output.iter_mut() {
            *value = clamp_f64(*value, self.min, self.max);
        }
    }

    fn min_value(&self) -> f64 {
        self.min
    }

    fn max_value(&self) -> f64 {
        self.max
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Java: `DensityFunction.clamp(double, double)`(DensityFunction.java L20-22).
pub fn clamp(input: Arc<dyn DensityFunction>, min: f64, max: f64) -> Arc<dyn DensityFunction> {
    Arc::new(Clamp { input, min, max })
}

// ---------------------------------------------------------------------------
// Noise(Java L281-301)
// ---------------------------------------------------------------------------

/// Java: `record Noise(NoiseHolder noise, double xzScale, double yScale)`(L281-301).
pub struct Noise {
    pub noise: NoiseHolder,
    pub xz_scale: f64,
    pub y_scale: f64,
}

impl DensityFunction for Noise {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        self.noise.get_value(
            context.block_x() as f64 * self.xz_scale,
            context.block_y() as f64 * self.y_scale,
            context.block_z() as f64 * self.xz_scale,
        )
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        context_provider.fill_all_directly(output, self);
    }

    fn min_value(&self) -> f64 {
        -self.max_value()
    }

    fn max_value(&self) -> f64 {
        self.noise.max_value()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Java: `noise(NormalNoise)`(L111-113).
pub fn noise(noise: Arc<NormalNoise>) -> Arc<dyn DensityFunction> {
    noise_scaled_xy(noise, 1.0, 1.0)
}

/// Java: `noise(NormalNoise, double xzScale, double yScale)`(L115-117).
pub fn noise_scaled_xy(
    noise: Arc<NormalNoise>,
    xz_scale: f64,
    y_scale: f64,
) -> Arc<dyn DensityFunction> {
    noise_holder(NoiseHolder::from_normal_noise(noise), xz_scale, y_scale)
}

/// Java: `noise(NoiseHolder, double xzScale, double yScale)`(L119-121).
pub fn noise_holder(noise: NoiseHolder, xz_scale: f64, y_scale: f64) -> Arc<dyn DensityFunction> {
    Arc::new(Noise {
        noise,
        xz_scale,
        y_scale,
    })
}

/// Java: `private static mapFromUnitTo(DensityFunction, double, double)`(L163-167).
fn map_from_unit_to(
    input: Arc<dyn DensityFunction>,
    min: f64,
    max: f64,
) -> Arc<dyn DensityFunction> {
    let midpoint = (min + max) * 0.5;
    let scale = (max - min) * 0.5;
    add(constant(midpoint), mul(constant(scale), input))
}

/// Java: `mappedNoise(NormalNoise, double min, double max)`(L123-125).
pub fn mapped_noise(
    normal_noise: Arc<NormalNoise>,
    min: f64,
    max: f64,
) -> Arc<dyn DensityFunction> {
    map_from_unit_to(noise(normal_noise), min, max)
}

/// Java: `mappedNoise(NormalNoise, double xzScale, double yScale, double min, double max)`(L127-129).
pub fn mapped_noise_scaled(
    noise: Arc<NormalNoise>,
    xz_scale: f64,
    y_scale: f64,
    min: f64,
    max: f64,
) -> Arc<dyn DensityFunction> {
    map_from_unit_to(noise_scaled_xy(noise, xz_scale, y_scale), min, max)
}

// ---------------------------------------------------------------------------
// RangeChoice(Java L303-338)
// ---------------------------------------------------------------------------

/// Java: `record RangeChoice(...)`(L303-338).
pub struct RangeChoice {
    pub input: Arc<dyn DensityFunction>,
    pub min_inclusive: f64,
    pub max_exclusive: f64,
    pub when_in_range: Arc<dyn DensityFunction>,
    pub when_out_of_range: Arc<dyn DensityFunction>,
}

impl DensityFunction for RangeChoice {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        let input_value = self.input.compute(context);
        if input_value >= self.min_inclusive && input_value < self.max_exclusive {
            self.when_in_range.compute(context)
        } else {
            self.when_out_of_range.compute(context)
        }
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        // Fill input first, then branch per slot value.
        self.input.fill_array(output, context_provider);
        for i in 0..output.len() {
            let value = output[i];
            output[i] = if value >= self.min_inclusive && value < self.max_exclusive {
                self.when_in_range.compute(context_provider.for_index(i))
            } else {
                self.when_out_of_range
                    .compute(context_provider.for_index(i))
            };
        }
    }

    fn min_value(&self) -> f64 {
        self.when_in_range
            .min_value()
            .min(self.when_out_of_range.min_value())
    }

    fn max_value(&self) -> f64 {
        self.when_in_range
            .max_value()
            .max(self.when_out_of_range.max_value())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Java: `rangeChoice(...)`(L169-177).
pub fn range_choice(
    input: Arc<dyn DensityFunction>,
    min_inclusive: f64,
    max_exclusive: f64,
    when_in_range: Arc<dyn DensityFunction>,
    when_out_of_range: Arc<dyn DensityFunction>,
) -> Arc<dyn DensityFunction> {
    Arc::new(RangeChoice {
        input,
        min_inclusive,
        max_exclusive,
        when_in_range,
        when_out_of_range,
    })
}

// ---------------------------------------------------------------------------
// WeirdScaledSampler(Java L340-408)
// ---------------------------------------------------------------------------

/// Java: `enum RarityValueMapper`(L365-376).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RarityValueMapper {
    /// Java: `TYPE1(getSpaghettiRarity3D, 2.0)`.
    Type1,
    /// Java: `TYPE2(getSpaghettiRarity2D, 3.0)`.
    Type2,
}

impl RarityValueMapper {
    /// Java: `private final Mapper mapper`.
    fn apply(self, value: f64) -> f64 {
        match self {
            RarityValueMapper::Type1 => Self::get_spaghetti_rarity_3d(value),
            RarityValueMapper::Type2 => Self::get_spaghetti_rarity_2d(value),
        }
    }

    /// Java: `private final double maxRarity`.
    fn max_rarity(self) -> f64 {
        match self {
            RarityValueMapper::Type1 => 2.0,
            RarityValueMapper::Type2 => 3.0,
        }
    }

    /// Java: `getSpaghettiRarity2D(double)`(L383-395).
    fn get_spaghetti_rarity_2d(value: f64) -> f64 {
        if value < -0.75 {
            0.5
        } else if value < -0.5 {
            0.75
        } else if value < 0.5 {
            1.0
        } else if value < 0.75 {
            2.0
        } else {
            3.0
        }
    }

    /// Java: `getSpaghettiRarity3D(double)`(L397-407).
    fn get_spaghetti_rarity_3d(value: f64) -> f64 {
        if value < -0.5 {
            0.75
        } else if value < 0.0 {
            1.0
        } else if value < 0.5 {
            1.5
        } else {
            2.0
        }
    }
}

/// Java: `record WeirdScaledSampler(...)`(L340-408).
pub struct WeirdScaledSampler {
    pub input: Arc<dyn DensityFunction>,
    pub noise: NoiseHolder,
    pub rarity_value_mapper: RarityValueMapper,
}

impl DensityFunction for WeirdScaledSampler {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        // TransformerWithContext(L1270-1287).
        self.transform(context, self.input.compute(context))
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        for i in 0..output.len() {
            let context = context_provider.for_index(i);
            output[i] = self.transform(context, self.input.compute(context));
        }
    }

    fn min_value(&self) -> f64 {
        0.0
    }

    fn max_value(&self) -> f64 {
        self.rarity_value_mapper.max_rarity() * self.noise.max_value()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl WeirdScaledSampler {
    /// Java: `transform(FunctionContext, double)`(L346-353).
    fn transform(&self, context: &dyn FunctionContext, input_value: f64) -> f64 {
        let rarity = self.rarity_value_mapper.apply(input_value);
        rarity
            * self
                .noise
                .get_value(
                    context.block_x() as f64 / rarity,
                    context.block_y() as f64 / rarity,
                    context.block_z() as f64 / rarity,
                )
                .abs()
    }
}

/// Java: `weirdScaledSampler(...)`(L131-137).
pub fn weird_scaled_sampler(
    input: Arc<dyn DensityFunction>,
    noise: Arc<NormalNoise>,
    rarity_value_mapper: RarityValueMapper,
) -> Arc<dyn DensityFunction> {
    Arc::new(WeirdScaledSampler {
        input,
        noise: NoiseHolder::from_normal_noise(noise),
        rarity_value_mapper,
    })
}

// ---------------------------------------------------------------------------
// YClampedGradient(Java L410-425)
// ---------------------------------------------------------------------------

/// Java: `record YClampedGradient(int fromY, int toY, double fromValue, double toValue)`(L410-425).
pub struct YClampedGradient {
    pub from_y: i32,
    pub to_y: i32,
    pub from_value: f64,
    pub to_value: f64,
}

impl DensityFunction for YClampedGradient {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        clamped_map(
            context.block_y() as f64,
            self.from_y as f64,
            self.to_y as f64,
            self.from_value,
            self.to_value,
        )
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        context_provider.fill_all_directly(output, self);
    }

    fn min_value(&self) -> f64 {
        self.from_value.min(self.to_value)
    }

    fn max_value(&self) -> f64 {
        self.from_value.max(self.to_value)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Java: `yClampedGradient(int, int, double, double)`(L203-205).
pub fn y_clamped_gradient(
    from_y: i32,
    to_y: i32,
    from_value: f64,
    to_value: f64,
) -> Arc<dyn DensityFunction> {
    Arc::new(YClampedGradient {
        from_y,
        to_y,
        from_value,
        to_value,
    })
}

// ---------------------------------------------------------------------------
// BlendAlpha / BlendOffset(Java L427-473)
// ---------------------------------------------------------------------------

/// `BlendAlpha`: constant 1.0 singleton.
pub struct BlendAlpha;

impl DensityFunction for BlendAlpha {
    fn compute(&self, _context: &dyn FunctionContext) -> f64 {
        1.0
    }

    fn fill_array(&self, output: &mut [f64], _context_provider: &dyn ContextProvider) {
        output.fill(1.0);
    }

    fn min_value(&self) -> f64 {
        1.0
    }

    fn max_value(&self) -> f64 {
        1.0
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `BlendOffset`: constant 0.0 singleton.
pub struct BlendOffset;

impl DensityFunction for BlendOffset {
    fn compute(&self, _context: &dyn FunctionContext) -> f64 {
        0.0
    }

    fn fill_array(&self, output: &mut [f64], _context_provider: &dyn ContextProvider) {
        output.fill(0.0);
    }

    fn min_value(&self) -> f64 {
        0.0
    }

    fn max_value(&self) -> f64 {
        0.0
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Java: `blendAlpha()`(L151-153).
pub fn blend_alpha() -> Arc<dyn DensityFunction> {
    Arc::new(BlendAlpha)
}

/// Java: `blendOffset()`(L155-157).
pub fn blend_offset() -> Arc<dyn DensityFunction> {
    Arc::new(BlendOffset)
}

/// `blendDensity` returns its input unchanged.
pub fn blend_density(input: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    input
}

// ---------------------------------------------------------------------------
// Spline(Java L475-532)
// ---------------------------------------------------------------------------

/// Java: `record SplinePoint(double location, Object value, double derivative)`(L531-532).
///
/// `Object value` (Number or DensityFunction) as an explicit enum.
pub struct SplinePoint {
    pub location: f64,
    pub value: SplineValue,
    pub derivative: f64,
}

/// The two shapes of a spline point value.
pub enum SplineValue {
    Constant(f64),
    Function(Arc<dyn DensityFunction>),
}

/// Java: `p(double location, double value, double derivative)`(L232-234).
pub fn p(location: f64, value: f64, derivative: f64) -> SplinePoint {
    SplinePoint {
        location,
        value: SplineValue::Constant(value),
        derivative,
    }
}

/// Java: `p(double location, DensityFunction value, double derivative)`(L236-238).
pub fn p_fn(location: f64, value: Arc<dyn DensityFunction>, derivative: f64) -> SplinePoint {
    SplinePoint {
        location,
        value: SplineValue::Function(value),
        derivative,
    }
}

/// Java: `record Spline(CubicSpline<Point> spline) implements DensityFunction`(L475-529).
///
/// Context references pass directly to CubicSpline; no Point wrapper needed.
pub struct Spline {
    pub(crate) spline: CubicSpline,
}

impl DensityFunction for Spline {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        self.spline.apply(context)
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        context_provider.fill_all_directly(output, self);
    }

    fn min_value(&self) -> f64 {
        self.spline.min_value()
    }

    fn max_value(&self) -> f64 {
        self.spline.max_value()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Adapts a DensityFunction as a CubicSpline value.
struct SplineCoordinate {
    function: Arc<dyn DensityFunction>,
}

impl CubicValue for SplineCoordinate {
    fn apply(&self, input: &dyn FunctionContext) -> f64 {
        self.function.compute(input)
    }

    fn min_value(&self) -> f64 {
        self.function.min_value()
    }

    fn max_value(&self) -> f64 {
        self.function.max_value()
    }
}

/// Java: `spline(CubicSpline<Spline.Point>)`(L215-217).
pub fn spline(spline: CubicSpline) -> Arc<dyn DensityFunction> {
    Arc::new(Spline { spline })
}

/// Java: `spline(DensityFunction coordinate, SplinePoint... points)`(L219-230).
pub fn spline_from_points(
    coordinate: Arc<dyn DensityFunction>,
    points: Vec<SplinePoint>,
) -> Arc<dyn DensityFunction> {
    let coordinate_fn: Rc<dyn Fn(&dyn FunctionContext) -> f64> = {
        let coordinate = Arc::clone(&coordinate);
        Rc::new(move |context: &dyn FunctionContext| coordinate.compute(context))
    };
    let mut builder = CubicSpline::builder(coordinate_fn);
    for point in points {
        match point.value {
            SplineValue::Function(function) => {
                builder.add_point(
                    point.location,
                    Rc::new(SplineCoordinate { function }),
                    point.derivative,
                );
            }
            SplineValue::Constant(value) => {
                builder.add_point_const(point.location, value, point.derivative);
            }
        }
    }
    spline(builder.build())
}

// ---------------------------------------------------------------------------
// Mapped wrappers.
// ---------------------------------------------------------------------------

/// Java: `enum Mapped.Type`(L564-610).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappedType {
    Abs,
    Square,
    Cube,
    HalfNegative,
    QuarterNegative,
    Invert,
    Squeeze,
}

impl MappedType {
    /// Java: `abstract double apply(double input)`.
    fn apply(self, input: f64) -> f64 {
        match self {
            MappedType::Abs => input.abs(),
            MappedType::Square => input * input,
            MappedType::Cube => input * input * input,
            MappedType::HalfNegative => {
                if input > 0.0 {
                    input
                } else {
                    input * 0.5
                }
            }
            MappedType::QuarterNegative => {
                if input > 0.0 {
                    input
                } else {
                    input * 0.25
                }
            }
            MappedType::Invert => {
                if input == 0.0 {
                    f64::INFINITY
                } else {
                    1.0 / input
                }
            }
            MappedType::Squeeze => {
                let clamped = clamp_f64(input, -1.0, 1.0);
                clamped / 2.0 - clamped * clamped * clamped / 24.0
            }
        }
    }
}

/// Java: `record Mapped(Type type, DensityFunction input, double minValue, double maxValue)`(L534-611).
pub struct Mapped {
    pub type_: MappedType,
    pub input: Arc<dyn DensityFunction>,
    pub min_value: f64,
    pub max_value: f64,
}

impl DensityFunction for Mapped {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        self.type_.apply(self.input.compute(context))
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        // PureTransformer: fill input first, then transform elementwise.
        self.input.fill_array(output, context_provider);
        for value in output.iter_mut() {
            *value = self.type_.apply(*value);
        }
    }

    fn min_value(&self) -> f64 {
        self.min_value
    }

    fn max_value(&self) -> f64 {
        self.max_value
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Java: `Mapped.create(Type, DensityFunction)`(L535-553).
pub(crate) fn create_mapped(
    type_: MappedType,
    input: Arc<dyn DensityFunction>,
) -> Arc<dyn DensityFunction> {
    let min = input.min_value();
    let max = input.max_value();
    let transformed_min = type_.apply(min);
    let transformed_max = type_.apply(max);

    let (min_value, max_value) = if type_ == MappedType::Invert {
        // Reciprocals spanning 0 produce ±inf.
        if min < 0.0 && max > 0.0 {
            (f64::NEG_INFINITY, f64::INFINITY)
        } else {
            (
                transformed_min.min(transformed_max),
                transformed_min.max(transformed_max),
            )
        }
    } else if type_ == MappedType::Abs || type_ == MappedType::Square {
        // Lower bound raised to 0.
        (
            0.0f64.max(transformed_min.min(transformed_max)),
            transformed_min.max(transformed_max),
        )
    } else {
        (
            transformed_min.min(transformed_max),
            transformed_min.max(transformed_max),
        )
    };

    Arc::new(Mapped {
        type_,
        input,
        min_value,
        max_value,
    })
}

/// Java: `map(DensityFunction, Mapped.Type)`(L207-209).
pub fn map(function: Arc<dyn DensityFunction>, type_: MappedType) -> Arc<dyn DensityFunction> {
    create_mapped(type_, function)
}

/// Java: `DensityFunction.abs()`(DensityFunction.java L24-26).
pub fn abs(function: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    map(function, MappedType::Abs)
}

/// Java: `DensityFunction.square()`(L28-30).
pub fn square(function: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    map(function, MappedType::Square)
}

/// Java: `DensityFunction.cube()`(L32-34).
pub fn cube(function: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    map(function, MappedType::Cube)
}

/// Java: `DensityFunction.halfNegative()`(L36-38).
pub fn half_negative(function: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    map(function, MappedType::HalfNegative)
}

/// Java: `DensityFunction.quarterNegative()`(L40-42).
pub fn quarter_negative(function: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    map(function, MappedType::QuarterNegative)
}

/// Java: `DensityFunction.invert()`(L44-46).
pub fn invert(function: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    map(function, MappedType::Invert)
}

/// Java: `DensityFunction.squeeze()`(L48-50).
pub fn squeeze(function: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    map(function, MappedType::Squeeze)
}

// ---------------------------------------------------------------------------
// Marker family.
// ---------------------------------------------------------------------------

/// Java: `Marker.Type`(L614-620).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerType {
    Interpolated,
    FlatCache,
    Cache2d,
    CacheOnce,
    CacheAllInCell,
}

/// Java: `interpolated(DensityFunction)`(L91-93).
pub fn interpolated(function: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    Arc::new(InterpolatedMarker::new(function))
}

/// Java: `flatCache(DensityFunction)`(L95-97).
pub fn flat_cache(function: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    Arc::new(FlatCacheMarker::new(function))
}

/// Java: `cache2d(DensityFunction)`(L99-101).
pub fn cache_2d(function: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    Arc::new(Cache2DMarker::new(function))
}

/// Java: `cacheOnce(DensityFunction)`(L103-105).
pub fn cache_once(function: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    Arc::new(CacheOnceMarker::new(function))
}

/// Java: `cacheAllInCell(DensityFunction)`(L107-109).
pub fn cache_all_in_cell(function: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    Arc::new(CacheAllInCellMarker::new(function))
}

// --- Cache2DMarker(Java L659-684, L1012-1015) ------------------------------

/// Java: `private static final class Cache2DState`(L1012-1015).
#[derive(Default)]
struct Cache2DState {
    last_pos_2d: i64,
    last_value: f64,
}

/// Java: `private static final class Cache2DMarker extends Marker`(L659-684).
struct Cache2DMarker {
    id: u64,
    wrapped: Arc<dyn DensityFunction>,
    local: RefCell<Cache2DState>,
}

impl Cache2DMarker {
    fn new(wrapped: Arc<dyn DensityFunction>) -> Self {
        Self {
            id: next_marker_id(),
            wrapped,
            local: RefCell::new(Cache2DState::default()),
        }
    }
}

impl DensityFunction for Cache2DMarker {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        let block_x = context.block_x();
        let block_z = context.block_z();
        with_marker_state(self.id, context, &self.local, |s| {
            // Java L671:`(((long) blockX) << 32) ^ (blockZ & 0xFFFFFFFFL)`.
            let pos_2d = ((block_x as i64) << 32) ^ ((block_z as i64) & 0xFFFF_FFFF);
            if s.last_pos_2d == pos_2d {
                return s.last_value;
            }
            s.last_pos_2d = pos_2d;
            s.last_value = self.wrapped.compute(context);
            s.last_value
        })
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        // Passes straight through to wrapped.
        self.wrapped.fill_array(output, context_provider);
    }

    fn min_value(&self) -> f64 {
        self.wrapped.min_value()
    }

    fn max_value(&self) -> f64 {
        self.wrapped.max_value()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// --- FlatCacheMarker(Java L686-757) -----------------------------------------

/// Java: `private static final int CHUNK_SIZE_BLOCKS = 16`(L687).
const CHUNK_SIZE_BLOCKS: i32 = 16;

/// Java: `private record FlatCacheCell(...)`(L756-757).
struct FlatCacheCell {
    first_block_x: i32,
    first_block_z: i32,
    values: Box<[f64; 256]>,
    filled_bits: Box<[u64; 4]>,
}

impl FlatCacheCell {
    fn new(first_block_x: i32, first_block_z: i32) -> Self {
        Self {
            first_block_x,
            first_block_z,
            values: Box::new([0.0; 256]),
            filled_bits: Box::new([0u64; 4]),
        }
    }
}

/// Java: `private static final class FlatCacheState`(L727-754).
///
/// The recent-cell fast path is a pure optimization; HashMap lookup
/// is equivalent, so the fast path is omitted.
struct FlatCacheState {
    context: MutableFunctionContext,
    cells: HashMap<i64, FlatCacheCell>,
}

impl Default for FlatCacheState {
    fn default() -> Self {
        Self {
            context: MutableFunctionContext::new(),
            cells: HashMap::new(),
        }
    }
}

/// Java: `private static final class FlatCacheMarker extends Marker`(L686-725).
struct FlatCacheMarker {
    id: u64,
    wrapped: Arc<dyn DensityFunction>,
    local: RefCell<FlatCacheState>,
}

impl FlatCacheMarker {
    fn new(wrapped: Arc<dyn DensityFunction>) -> Self {
        Self {
            id: next_marker_id(),
            wrapped,
            local: RefCell::new(FlatCacheState::default()),
        }
    }
}

impl DensityFunction for FlatCacheMarker {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        let block_x = context.block_x();
        let block_z = context.block_z();
        with_marker_state(self.id, context, &self.local, |s| {
            let chunk_x = block_x >> 4;
            let chunk_z = block_z >> 4;
            // Java L735:`(((long) chunkX) << 32) ^ (chunkZ & 0xFFFFFFFFL)`.
            let key = ((chunk_x as i64) << 32) ^ ((chunk_z as i64) & 0xFFFF_FFFF);
            let cell = s
                .cells
                .entry(key)
                .or_insert_with(|| FlatCacheCell::new(chunk_x << 4, chunk_z << 4));

            let local_x = block_x - cell.first_block_x;
            let local_z = block_z - cell.first_block_z;
            if (0..CHUNK_SIZE_BLOCKS).contains(&local_x)
                && (0..CHUNK_SIZE_BLOCKS).contains(&local_z)
            {
                let index = (local_x + local_z * CHUNK_SIZE_BLOCKS) as usize;
                let word = index >> 6;
                let mask = 1u64 << (index & 63);
                if cell.filled_bits[word] & mask != 0 {
                    return cell.values[index];
                }

                // Wrapped receives (blockX, 0, blockZ): y is always 0,
                // using the state-owned context.
                let computed = self.wrapped.compute(s.context.set(block_x, 0, block_z));
                cell.values[index] = computed;
                cell.filled_bits[word] |= mask;
                computed
            } else {
                // Defensive branch (unreachable without key collision).
                self.wrapped.compute(context)
            }
        })
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        context_provider.fill_all_directly(output, self);
    }

    fn min_value(&self) -> f64 {
        self.wrapped.min_value()
    }

    fn max_value(&self) -> f64 {
        self.wrapped.max_value()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// --- CacheOnceMarker(Java L759-786, L1017-1022) -----------------------------

/// Java: `private static final class Cache3DState`(L1017-1022).
///
/// Fields default to 0 (a first query at (0,0,0) hits all-zero state).
#[derive(Default)]
struct Cache3DState {
    block_x: i32,
    block_y: i32,
    block_z: i32,
    value: f64,
}

/// Java: `private static final class CacheOnceMarker extends Marker`(L759-786).
struct CacheOnceMarker {
    id: u64,
    wrapped: Arc<dyn DensityFunction>,
    local: RefCell<Cache3DState>,
}

impl CacheOnceMarker {
    fn new(wrapped: Arc<dyn DensityFunction>) -> Self {
        Self {
            id: next_marker_id(),
            wrapped,
            local: RefCell::new(Cache3DState::default()),
        }
    }
}

impl DensityFunction for CacheOnceMarker {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        let block_x = context.block_x();
        let block_y = context.block_y();
        let block_z = context.block_z();
        with_marker_state(self.id, context, &self.local, |s| {
            if s.block_x == block_x && s.block_y == block_y && s.block_z == block_z {
                return s.value;
            }
            s.block_x = block_x;
            s.block_y = block_y;
            s.block_z = block_z;
            s.value = self.wrapped.compute(context);
            s.value
        })
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        self.wrapped.fill_array(output, context_provider);
    }

    fn min_value(&self) -> f64 {
        self.wrapped.min_value()
    }

    fn max_value(&self) -> f64 {
        self.wrapped.max_value()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// Shared constants for cell markers.

const CELL_SIZE_XZ: i32 = 4;
const CELL_SIZE_Y: i32 = 8;
const CELL_XZ_MASK: i32 = CELL_SIZE_XZ - 1;
const CELL_Y_MASK: i32 = CELL_SIZE_Y - 1;
const CELL_VALUE_COUNT: usize = (CELL_SIZE_XZ * CELL_SIZE_Y * CELL_SIZE_XZ) as usize;
const INV_CELL_SIZE_XZ: f64 = 1.0 / CELL_SIZE_XZ as f64;
const INV_CELL_SIZE_Y: f64 = 1.0 / CELL_SIZE_Y as f64;

// --- CacheAllInCellMarker(Java L788-877) ------------------------------------

/// Java: `CacheAllInCellMarker.CacheAllInCellState`(L822-875).
struct CacheAllInCellState {
    values: Box<[f64; CELL_VALUE_COUNT]>,
    context: MutableFunctionContext,
    chunk_cache_context: MutableChunkCacheContext,
    last_key: i64,
}

impl Default for CacheAllInCellState {
    fn default() -> Self {
        Self {
            values: Box::new([0.0; CELL_VALUE_COUNT]),
            context: MutableFunctionContext::new(),
            chunk_cache_context: MutableChunkCacheContext::new(),
            // Java L827:`private long lastKey = Long.MAX_VALUE`.
            last_key: i64::MAX,
        }
    }
}

impl CacheAllInCellState {
    /// Java: `getOrCreateCell(int, int, int, DensityFunction, FunctionContext)`(L829-840).
    fn get_or_create_cell(
        &mut self,
        cell_x: i32,
        cell_y: i32,
        cell_z: i32,
        wrapped: &Arc<dyn DensityFunction>,
        source_context: &dyn FunctionContext,
    ) -> &[f64] {
        // 21-bit truncated packed key.
        let key = (((cell_x as i64) & 0x1F_FFFF) << 42)
            | (((cell_y as i64) & 0x1F_FFFF) << 21)
            | ((cell_z as i64) & 0x1F_FFFF);
        if key == self.last_key {
            return self.values.as_slice();
        }

        self.fill_cell(
            cell_x << 2,
            cell_y << 3,
            cell_z << 2,
            wrapped,
            source_context,
        );
        self.last_key = key;
        self.values.as_slice()
    }

    /// Java: `fillCell(int, int, int, DensityFunction, FunctionContext)`(L842-848):
    /// Reusable fill context chosen by the source context kind.
    fn fill_cell(
        &mut self,
        start_x: i32,
        start_y: i32,
        start_z: i32,
        wrapped: &Arc<dyn DensityFunction>,
        source_context: &dyn FunctionContext,
    ) {
        if let Some(chunk_cache_source) = source_context.as_chunk_cache_context() {
            // Cached context (cache reference propagates to wrapped).
            let cache = chunk_cache_source.density_chunk_cache();
            let context = self.chunk_cache_context.with_cache(cache);
            let mut index = 0usize;
            for local_y in 0..CELL_SIZE_Y {
                let y = start_y + local_y;
                for local_z in 0..CELL_SIZE_XZ {
                    let z = start_z + local_z;
                    for local_x in 0..CELL_SIZE_XZ {
                        let value = wrapped.compute(context.set(start_x + local_x, y, z));
                        self.values[index] = value;
                        index += 1;
                    }
                }
            }
        } else {
            // Plain reusable context.
            let context = &self.context;
            let mut index = 0usize;
            for local_y in 0..CELL_SIZE_Y {
                let y = start_y + local_y;
                for local_z in 0..CELL_SIZE_XZ {
                    let z = start_z + local_z;
                    for local_x in 0..CELL_SIZE_XZ {
                        let value = wrapped.compute(context.set(start_x + local_x, y, z));
                        self.values[index] = value;
                        index += 1;
                    }
                }
            }
        }
    }
}

/// Java: `private static final class CacheAllInCellMarker extends Marker`(L788-877).
struct CacheAllInCellMarker {
    id: u64,
    wrapped: Arc<dyn DensityFunction>,
    local: RefCell<CacheAllInCellState>,
}

impl CacheAllInCellMarker {
    fn new(wrapped: Arc<dyn DensityFunction>) -> Self {
        Self {
            id: next_marker_id(),
            wrapped,
            local: RefCell::new(CacheAllInCellState::default()),
        }
    }
}

impl DensityFunction for CacheAllInCellMarker {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        let block_x = context.block_x();
        let block_y = context.block_y();
        let block_z = context.block_z();
        let cell_x = block_x >> 2;
        let cell_y = block_y >> 3;
        let cell_z = block_z >> 2;
        with_marker_state(self.id, context, &self.local, |s| {
            let values = s.get_or_create_cell(cell_x, cell_y, cell_z, &self.wrapped, context);
            // Java L811-813:`((blockY & CELL_Y_MASK) << 4) | ((blockZ & CELL_XZ_MASK) << 2) | (blockX & CELL_XZ_MASK)`.
            let index = (((block_y & CELL_Y_MASK) << 4)
                | ((block_z & CELL_XZ_MASK) << 2)
                | (block_x & CELL_XZ_MASK)) as usize;
            values[index]
        })
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        context_provider.fill_all_directly(output, self);
    }

    fn min_value(&self) -> f64 {
        self.wrapped.min_value()
    }

    fn max_value(&self) -> f64 {
        self.wrapped.max_value()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// --- InterpolatedMarker(Java L879-1010) --------------------------------------

/// Java: `InterpolatedMarker.InterpolatedState`(L914-1008).
struct InterpolatedState {
    values: Box<[f64; CELL_VALUE_COUNT]>,
    context: MutableFunctionContext,
    chunk_cache_context: MutableChunkCacheContext,
    last_key: i64,
}

impl Default for InterpolatedState {
    fn default() -> Self {
        Self {
            values: Box::new([0.0; CELL_VALUE_COUNT]),
            context: MutableFunctionContext::new(),
            chunk_cache_context: MutableChunkCacheContext::new(),
            last_key: i64::MAX,
        }
    }
}

impl InterpolatedState {
    /// Java: `getOrCreateCell(int, int, int, DensityFunction, FunctionContext)`(L921-930).
    ///
    /// Note: cellX/Y/Z arrive pre-aligned by the caller.
    fn get_or_create_cell(
        &mut self,
        cell_x: i32,
        cell_y: i32,
        cell_z: i32,
        wrapped: &Arc<dyn DensityFunction>,
        source_context: &dyn FunctionContext,
    ) -> &[f64] {
        let key = (((cell_x as i64) & 0x1F_FFFF) << 42)
            | (((cell_y as i64) & 0x1F_FFFF) << 21)
            | ((cell_z as i64) & 0x1F_FFFF);
        if key == self.last_key {
            return self.values.as_slice();
        }

        self.fill_cell(cell_x, cell_y, cell_z, wrapped, source_context);
        self.last_key = key;
        self.values.as_slice()
    }

    /// `fillCell`: 8-corner sampling plus trilinear expansion.
    fn fill_cell(
        &mut self,
        cell_x: i32,
        cell_y: i32,
        cell_z: i32,
        wrapped: &Arc<dyn DensityFunction>,
        source_context: &dyn FunctionContext,
    ) {
        let next_x = cell_x + CELL_SIZE_XZ;
        let next_y = cell_y + CELL_SIZE_Y;
        let next_z = cell_z + CELL_SIZE_XZ;

        let corners: [f64; 8] =
            if let Some(chunk_cache_source) = source_context.as_chunk_cache_context() {
                // Cached-context path.
                let cache = chunk_cache_source.density_chunk_cache();
                let context = self.chunk_cache_context.with_cache(cache);
                [
                    wrapped.compute(context.set(cell_x, cell_y, cell_z)),
                    wrapped.compute(context.set(next_x, cell_y, cell_z)),
                    wrapped.compute(context.set(cell_x, next_y, cell_z)),
                    wrapped.compute(context.set(next_x, next_y, cell_z)),
                    wrapped.compute(context.set(cell_x, cell_y, next_z)),
                    wrapped.compute(context.set(next_x, cell_y, next_z)),
                    wrapped.compute(context.set(cell_x, next_y, next_z)),
                    wrapped.compute(context.set(next_x, next_y, next_z)),
                ]
            } else {
                // Plain-context path.
                let context = &self.context;
                [
                    wrapped.compute(context.set(cell_x, cell_y, cell_z)),
                    wrapped.compute(context.set(next_x, cell_y, cell_z)),
                    wrapped.compute(context.set(cell_x, next_y, cell_z)),
                    wrapped.compute(context.set(next_x, next_y, cell_z)),
                    wrapped.compute(context.set(cell_x, cell_y, next_z)),
                    wrapped.compute(context.set(next_x, cell_y, next_z)),
                    wrapped.compute(context.set(cell_x, next_y, next_z)),
                    wrapped.compute(context.set(next_x, next_y, next_z)),
                ]
            };

        self.fill_values(
            corners[0], corners[1], corners[2], corners[3], corners[4], corners[5], corners[6],
            corners[7],
        );
    }

    /// Java: `fillValues(double×8)`(L972-1007).
    fn fill_values(
        &mut self,
        d000: f64,
        d100: f64,
        d010: f64,
        d110: f64,
        d001: f64,
        d101: f64,
        d011: f64,
        d111: f64,
    ) {
        let c000 = d000;
        let c100 = d100 - d000;
        let c010 = d010 - d000;
        let c001 = d001 - d000;
        let c110 = d110 - d100 - d010 + d000;
        let c101 = d101 - d100 - d001 + d000;
        let c011 = d011 - d010 - d001 + d000;
        let c111 = d111 - d110 - d101 - d011 + d100 + d010 + d001 - d000;

        let mut index = 0usize;
        for local_y in 0..CELL_SIZE_Y {
            let y_alpha = local_y as f64 * INV_CELL_SIZE_Y;
            for local_z in 0..CELL_SIZE_XZ {
                let z_alpha = local_z as f64 * INV_CELL_SIZE_XZ;
                let yz = y_alpha * z_alpha;
                let z_term = z_alpha * (c001 + y_alpha * c011);
                for local_x in 0..CELL_SIZE_XZ {
                    let x_alpha = local_x as f64 * INV_CELL_SIZE_XZ;
                    self.values[index] = c000
                        + x_alpha * (c100 + y_alpha * c110 + z_alpha * c101 + yz * c111)
                        + y_alpha * c010
                        + z_term;
                    index += 1;
                }
            }
        }
    }
}

/// Java: `private static final class InterpolatedMarker extends Marker`(L879-1010).
struct InterpolatedMarker {
    id: u64,
    wrapped: Arc<dyn DensityFunction>,
    local: RefCell<InterpolatedState>,
}

impl InterpolatedMarker {
    fn new(wrapped: Arc<dyn DensityFunction>) -> Self {
        Self {
            id: next_marker_id(),
            wrapped,
            local: RefCell::new(InterpolatedState::default()),
        }
    }
}

impl DensityFunction for InterpolatedMarker {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        let block_x = context.block_x();
        let block_y = context.block_y();
        let block_z = context.block_z();
        // Cell-aligned coordinates.
        let cell_x = (block_x >> 2) << 2;
        let cell_y = (block_y >> 3) << 3;
        let cell_z = (block_z >> 2) << 2;
        with_marker_state(self.id, context, &self.local, |s| {
            let values = s.get_or_create_cell(cell_x, cell_y, cell_z, &self.wrapped, context);
            let index = (((block_y & CELL_Y_MASK) << 4)
                | ((block_z & CELL_XZ_MASK) << 2)
                | (block_x & CELL_XZ_MASK)) as usize;
            values[index]
        })
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        context_provider.fill_all_directly(output, self);
    }

    fn min_value(&self) -> f64 {
        self.wrapped.min_value()
    }

    fn max_value(&self) -> f64 {
        self.wrapped.max_value()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------
// Shift family.
// ---------------------------------------------------------------------------

/// Java: `ShiftNoise.computeShift(double, double, double)`(L1066-1068).
fn compute_shift(offset_noise: &NoiseHolder, x: f64, y: f64, z: f64) -> f64 {
    offset_noise.get_value(x * 0.25, y * 0.25, z * 0.25) * 4.0
}

/// Java: `record Shift(NoiseHolder offsetNoise) implements ShiftNoise`(L1086-1092).
pub struct Shift {
    pub offset_noise: NoiseHolder,
}

impl DensityFunction for Shift {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        compute_shift(
            &self.offset_noise,
            context.block_x() as f64,
            context.block_y() as f64,
            context.block_z() as f64,
        )
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        context_provider.fill_all_directly(output, self);
    }

    fn min_value(&self) -> f64 {
        -self.max_value()
    }

    fn max_value(&self) -> f64 {
        self.offset_noise.max_value() * 4.0
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Java: `record ShiftA(NoiseHolder offsetNoise) implements ShiftNoise`(L1094-1100).
pub struct ShiftA {
    pub offset_noise: NoiseHolder,
}

impl DensityFunction for ShiftA {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        compute_shift(
            &self.offset_noise,
            context.block_x() as f64,
            0.0,
            context.block_z() as f64,
        )
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        context_provider.fill_all_directly(output, self);
    }

    fn min_value(&self) -> f64 {
        -self.max_value()
    }

    fn max_value(&self) -> f64 {
        self.offset_noise.max_value() * 4.0
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Java: `record ShiftB(NoiseHolder offsetNoise) implements ShiftNoise`(L1102-1108).
pub struct ShiftB {
    pub offset_noise: NoiseHolder,
}

impl DensityFunction for ShiftB {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        compute_shift(
            &self.offset_noise,
            context.block_z() as f64,
            context.block_x() as f64,
            0.0,
        )
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        context_provider.fill_all_directly(output, self);
    }

    fn min_value(&self) -> f64 {
        -self.max_value()
    }

    fn max_value(&self) -> f64 {
        self.offset_noise.max_value() * 4.0
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Java: `shiftA(NormalNoise)`(L139-141).
pub fn shift_a(noise: Arc<NormalNoise>) -> Arc<dyn DensityFunction> {
    Arc::new(ShiftA {
        offset_noise: NoiseHolder::from_normal_noise(noise),
    })
}

/// Java: `shiftB(NormalNoise)`(L143-145).
pub fn shift_b(noise: Arc<NormalNoise>) -> Arc<dyn DensityFunction> {
    Arc::new(ShiftB {
        offset_noise: NoiseHolder::from_normal_noise(noise),
    })
}

/// Java: `shift(NormalNoise)`(L147-149).
pub fn shift(noise: Arc<NormalNoise>) -> Arc<dyn DensityFunction> {
    Arc::new(Shift {
        offset_noise: NoiseHolder::from_normal_noise(noise),
    })
}

// ---------------------------------------------------------------------------
// ShiftedNoise(Java L1110-1140)
// ---------------------------------------------------------------------------

/// Java: `record ShiftedNoise(...)`(L1110-1140).
pub struct ShiftedNoise {
    pub shift_x: Arc<dyn DensityFunction>,
    pub shift_y: Arc<dyn DensityFunction>,
    pub shift_z: Arc<dyn DensityFunction>,
    pub xz_scale: f64,
    pub y_scale: f64,
    pub noise: NoiseHolder,
}

impl DensityFunction for ShiftedNoise {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        let x = context.block_x() as f64 * self.xz_scale + self.shift_x.compute(context);
        let y = context.block_y() as f64 * self.y_scale + self.shift_y.compute(context);
        let z = context.block_z() as f64 * self.xz_scale + self.shift_z.compute(context);
        self.noise.get_value(x, y, z)
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        context_provider.fill_all_directly(output, self);
    }

    fn min_value(&self) -> f64 {
        -self.max_value()
    }

    fn max_value(&self) -> f64 {
        self.noise.max_value()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------
// TwoArgumentSimpleFunction / Ap2 / MulOrAdd(Java L1142-1249)
// ---------------------------------------------------------------------------

/// Java: `TwoArgumentSimpleFunction.Type`(L1178-1183).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TwoArgumentType {
    Add,
    Mul,
    Min,
    Max,
}

/// Java: `record Ap2(...)`(L1186-1233).
pub struct Ap2 {
    pub type_: TwoArgumentType,
    pub argument1: Arc<dyn DensityFunction>,
    pub argument2: Arc<dyn DensityFunction>,
    pub min_value: f64,
    pub max_value: f64,
}

impl DensityFunction for Ap2 {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        let v1 = self.argument1.compute(context);
        match self.type_ {
            TwoArgumentType::Add => v1 + self.argument2.compute(context),
            // MUL short-circuits on v1 == 0.0.
            TwoArgumentType::Mul => {
                if v1 == 0.0 {
                    0.0
                } else {
                    v1 * self.argument2.compute(context)
                }
            }
            TwoArgumentType::Min => {
                if v1 < self.argument2.min_value() {
                    v1
                } else {
                    v1.min(self.argument2.compute(context))
                }
            }
            TwoArgumentType::Max => {
                if v1 > self.argument2.max_value() {
                    v1
                } else {
                    v1.max(self.argument2.compute(context))
                }
            }
        }
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        self.argument1.fill_array(output, context_provider);
        match self.type_ {
            TwoArgumentType::Add => {
                // Scratch array fills argument2, then adds elementwise.
                let mut other = vec![0.0; output.len()];
                self.argument2.fill_array(&mut other, context_provider);
                for i in 0..output.len() {
                    output[i] += other[i];
                }
            }
            TwoArgumentType::Mul => {
                for i in 0..output.len() {
                    let value = output[i];
                    output[i] = if value == 0.0 {
                        0.0
                    } else {
                        value * self.argument2.compute(context_provider.for_index(i))
                    };
                }
            }
            TwoArgumentType::Min => {
                let min = self.argument2.min_value();
                for i in 0..output.len() {
                    let value = output[i];
                    output[i] = if value < min {
                        value
                    } else {
                        value.min(self.argument2.compute(context_provider.for_index(i)))
                    };
                }
            }
            TwoArgumentType::Max => {
                let max = self.argument2.max_value();
                for i in 0..output.len() {
                    let value = output[i];
                    output[i] = if value > max {
                        value
                    } else {
                        value.max(self.argument2.compute(context_provider.for_index(i)))
                    };
                }
            }
        }
    }

    fn min_value(&self) -> f64 {
        self.min_value
    }

    fn max_value(&self) -> f64 {
        self.max_value
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Java: `record MulOrAdd(...)`(L1235-1249).
pub struct MulOrAdd {
    pub specific_type: TwoArgumentType,
    pub input: Arc<dyn DensityFunction>,
    pub min_value: f64,
    pub max_value: f64,
    pub argument: f64,
}

impl DensityFunction for MulOrAdd {
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        self.transform(self.input.compute(context))
    }

    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        // PureTransformer.
        self.input.fill_array(output, context_provider);
        for value in output.iter_mut() {
            *value = self.transform(*value);
        }
    }

    fn min_value(&self) -> f64 {
        self.min_value
    }

    fn max_value(&self) -> f64 {
        self.max_value
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl MulOrAdd {
    /// Java: `transform(double)`(L1238-1243).
    fn transform(&self, input_value: f64) -> f64 {
        match self.specific_type {
            TwoArgumentType::Mul => input_value * self.argument,
            TwoArgumentType::Add => input_value + self.argument,
            _ => unreachable!("MulOrAdd is only built by ADD/MUL"),
        }
    }
}

/// Java: `TwoArgumentSimpleFunction.create(Type, DensityFunction, DensityFunction)`(L1143-1176).
fn create_two_argument(
    type_: TwoArgumentType,
    argument1: Arc<dyn DensityFunction>,
    argument2: Arc<dyn DensityFunction>,
) -> Arc<dyn DensityFunction> {
    let min1 = argument1.min_value();
    let min2 = argument2.min_value();
    let max1 = argument1.max_value();
    let max2 = argument2.max_value();

    let min_value = match type_ {
        TwoArgumentType::Add => min1 + min2,
        // MUL lower bound (same-sign products, cross minima otherwise).
        TwoArgumentType::Mul => {
            if min1 > 0.0 && min2 > 0.0 {
                min1 * min2
            } else if max1 < 0.0 && max2 < 0.0 {
                max1 * max2
            } else {
                (min1 * max2).min(max1 * min2)
            }
        }
        TwoArgumentType::Min => min1.min(min2),
        TwoArgumentType::Max => min1.max(min2),
    };

    let max_value = match type_ {
        TwoArgumentType::Add => max1 + max2,
        // MUL upper bound.
        TwoArgumentType::Mul => {
            if min1 > 0.0 && min2 > 0.0 {
                max1 * max2
            } else if max1 < 0.0 && max2 < 0.0 {
                min1 * min2
            } else {
                (min1 * min2).max(max1 * max2)
            }
        }
        TwoArgumentType::Min => max1.min(max2),
        TwoArgumentType::Max => max1.max(max2),
    };

    // Constant arguments fold into MulOrAdd.
    if matches!(type_, TwoArgumentType::Add | TwoArgumentType::Mul) {
        if let Some(constant) = argument1.as_any().downcast_ref::<Constant>() {
            return Arc::new(MulOrAdd {
                specific_type: type_,
                input: argument2,
                min_value,
                max_value,
                argument: constant.value,
            });
        }
        if let Some(constant) = argument2.as_any().downcast_ref::<Constant>() {
            return Arc::new(MulOrAdd {
                specific_type: type_,
                input: argument1,
                min_value,
                max_value,
                argument: constant.value,
            });
        }
    }

    Arc::new(Ap2 {
        type_,
        argument1,
        argument2,
        min_value,
        max_value,
    })
}

/// Java: `add(DensityFunction, DensityFunction)`(L179-181).
pub fn add(f1: Arc<dyn DensityFunction>, f2: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    create_two_argument(TwoArgumentType::Add, f1, f2)
}

/// Java: `mul(DensityFunction, DensityFunction)`(L183-185).
pub fn mul(f1: Arc<dyn DensityFunction>, f2: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    create_two_argument(TwoArgumentType::Mul, f1, f2)
}

/// Java: `min(DensityFunction, DensityFunction)`(L187-189).
pub fn min(f1: Arc<dyn DensityFunction>, f2: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    create_two_argument(TwoArgumentType::Min, f1, f2)
}

/// Java: `max(DensityFunction, DensityFunction)`(L191-193).
pub fn max(f1: Arc<dyn DensityFunction>, f2: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    create_two_argument(TwoArgumentType::Max, f1, f2)
}

// ---------------------------------------------------------------------------
// lerp / clampedMap(Java L211-213, L1289-1295)
// ---------------------------------------------------------------------------

/// Java: `lerp(DensityFunction factor, double first, DensityFunction second)`(L211-213).
pub fn lerp(
    factor: Arc<dyn DensityFunction>,
    first: f64,
    second: Arc<dyn DensityFunction>,
) -> Arc<dyn DensityFunction> {
    add(mul(factor, add(second, constant(-first))), constant(first))
}

/// Java: `private static clampedMap(double, double, double, double, double)`(L1289-1295).
fn clamped_map(value: f64, from_y: f64, to_y: f64, from_value: f64, to_value: f64) -> f64 {
    if from_y == to_y {
        return if value < from_y { from_value } else { to_value };
    }
    let t = clamp_f64((value - from_y) / (to_y - from_y), 0.0, 1.0);
    from_value + t * (to_value - from_value)
}

#[cfg(test)]
mod tests {
    use super::super::function::NoiseSampler;
    use super::*;
    use crate::worldgen::densityfunction::function::SinglePointContext;

    fn ctx(x: i32, y: i32, z: i32) -> SinglePointContext {
        SinglePointContext {
            block_x: x,
            block_y: y,
            block_z: z,
        }
    }

    fn compute(f: &Arc<dyn DensityFunction>, x: i32, y: i32, z: i32) -> f64 {
        f.compute(&ctx(x, y, z))
    }

    /// Call-counting wrapper (cache-semantics tests).
    struct Counting {
        inner: Arc<dyn DensityFunction>,
        calls: std::rc::Rc<Cell<u32>>,
    }

    impl DensityFunction for Counting {
        fn compute(&self, context: &dyn FunctionContext) -> f64 {
            self.calls.set(self.calls.get() + 1);
            self.inner.compute(context)
        }
        fn fill_array(&self, output: &mut [f64], cp: &dyn ContextProvider) {
            self.inner.fill_array(output, cp)
        }
        fn min_value(&self) -> f64 {
            self.inner.min_value()
        }
        fn max_value(&self) -> f64 {
            self.inner.max_value()
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn counting(
        inner: Arc<dyn DensityFunction>,
        calls: &std::rc::Rc<Cell<u32>>,
    ) -> Arc<dyn DensityFunction> {
        Arc::new(Counting {
            inner,
            calls: std::rc::Rc::clone(calls),
        })
    }

    /// Records received coordinates (flatCache y=0 checks).
    struct Recorder {
        last: std::rc::Rc<Cell<(i32, i32, i32)>>,
    }

    impl DensityFunction for Recorder {
        fn compute(&self, context: &dyn FunctionContext) -> f64 {
            self.last
                .set((context.block_x(), context.block_y(), context.block_z()));
            1.0
        }
        fn fill_array(&self, _output: &mut [f64], _cp: &dyn ContextProvider) {
            unimplemented!()
        }
        fn min_value(&self) -> f64 {
            0.0
        }
        fn max_value(&self) -> f64 {
            2.0
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    // --- Constant / basic combinations ---

    #[test]
    fn constant_zero_and_value() {
        assert_eq!(compute(&zero(), 1, 2, 3), 0.0);
        assert_eq!(compute(&constant(3.5), 1, 2, 3), 3.5);
        assert_eq!(zero().min_value(), 0.0);
        assert_eq!(constant(3.5).max_value(), 3.5);
    }

    #[test]
    fn add_mul_min_max_compute_and_bounds() {
        let a = constant(2.0);
        let b = constant(-3.0);

        let sum = add(Arc::clone(&a), Arc::clone(&b));
        assert_eq!(compute(&sum, 0, 0, 0), -1.0);
        assert_eq!((sum.min_value(), sum.max_value()), (-1.0, -1.0));

        let product = mul(Arc::clone(&a), Arc::clone(&b));
        assert_eq!(compute(&product, 0, 0, 0), -6.0);

        let mn = min(Arc::clone(&a), Arc::clone(&b));
        let mx = max(Arc::clone(&a), Arc::clone(&b));
        assert_eq!(compute(&mn, 0, 0, 0), -3.0);
        assert_eq!(compute(&mx, 0, 0, 0), 2.0);

        // add(constant, f) folds into MulOrAdd.
        let folded = add(constant(10.0), Arc::clone(&b));
        assert!(folded.as_any().downcast_ref::<MulOrAdd>().is_some());
        assert_eq!(compute(&folded, 0, 0, 0), 7.0);
        let folded_mul = mul(Arc::clone(&b), constant(0.5));
        assert!(folded_mul.as_any().downcast_ref::<MulOrAdd>().is_some());
        assert_eq!(compute(&folded_mul, 0, 0, 0), -1.5);
    }

    #[test]
    fn mul_bounds_crossing_signs() {
        // Covers positive/positive, negative/negative, and mixed signs.
        let pos = y_clamped_gradient(0, 1, 1.0, 2.0); // range [1, 2]
        let neg = y_clamped_gradient(0, 1, -4.0, -2.0); // range [-4, -2]
        let m = mul(Arc::clone(&pos), Arc::clone(&neg));
        // Mixed signs: min = min(1*-2, 2*-4) = -8.
        assert_eq!(m.min_value(), -8.0);
        // max = max(min1*min2, max1*max2) = max(-4, -4) = -4
        assert_eq!(m.max_value(), -4.0);

        let m2 = mul(Arc::clone(&pos), Arc::clone(&pos));
        assert_eq!((m2.min_value(), m2.max_value()), (1.0, 4.0));
    }

    #[test]
    fn ap2_mul_zero_short_circuit() {
        // v1 == 0.0 skips argument2 (non-constant path uses Ap2).
        let y = y_clamped_gradient(0, 1, 0.0, 1.0); // y=0 → 0.0
        let calls = std::rc::Rc::new(Cell::new(0u32));
        let counted = counting(Arc::clone(&y) as Arc<dyn DensityFunction>, &calls);
        let ap2 = mul(Arc::clone(&counted), Arc::clone(&y));
        assert!(ap2.as_any().downcast_ref::<Ap2>().is_some());
        assert_eq!(compute(&ap2, 5, 0, 5), 0.0);
        assert_eq!(calls.get(), 1, "v1==0 must not evaluate argument2");
    }

    // --- Clamp / Mapped ------------------------------------------------------

    #[test]
    fn clamp_transform() {
        let y = y_clamped_gradient(0, 10, -5.0, 5.0); // y linear in [-5, 5]
        let c = clamp(Arc::clone(&y), -2.0, 3.0);
        assert_eq!(compute(&c, 0, 0, 0), -2.0);
        assert_eq!(compute(&c, 0, 5, 0), 0.0);
        assert_eq!(compute(&c, 0, 10, 0), 3.0);
        assert_eq!((c.min_value(), c.max_value()), (-2.0, 3.0));
    }

    #[test]
    fn mapped_types_apply_and_bounds() {
        let y = y_clamped_gradient(0, 1, -2.0, 3.0); // range [-2, 3]

        let a = abs(Arc::clone(&y));
        assert_eq!(compute(&a, 0, 0, 0), 2.0);
        // ABS bounds transform endpoints only (no contains-zero check):
        // max(0, min(|-2|, |3|)) = 2.
        assert_eq!((a.min_value(), a.max_value()), (2.0, 3.0));

        let s = square(Arc::clone(&y));
        assert_eq!(compute(&s, 0, 0, 0), 4.0);
        // Java L548-550:max(0, min(4, 9)) = 4.
        assert_eq!((s.min_value(), s.max_value()), (4.0, 9.0));

        let c = cube(Arc::clone(&y));
        assert_eq!(compute(&c, 0, 0, 0), -8.0);
        assert_eq!((c.min_value(), c.max_value()), (-8.0, 27.0));

        let hn = half_negative(Arc::clone(&y));
        assert_eq!(compute(&hn, 0, 0, 0), -1.0); // -2 * 0.5
        assert_eq!(compute(&hn, 0, 1, 0), 3.0); // positives unchanged

        let qn = quarter_negative(Arc::clone(&y));
        assert_eq!(compute(&qn, 0, 0, 0), -0.5);

        // INVERT across 0 yields ±inf.
        let inv = invert(Arc::clone(&y));
        assert!(inv.min_value() == f64::NEG_INFINITY);
        assert!(inv.max_value() == f64::INFINITY);
        assert_eq!(compute(&inv, 0, 0, 0), -0.5);

        let sq = squeeze(Arc::clone(&y));
        // squeeze(-2) = clamp(-2,-1,1)/2 - (-1)^3/24 = -0.5 + 1/24
        assert!((compute(&sq, 0, 0, 0) - (-0.5 + 1.0 / 24.0)).abs() < 1e-12);
    }

    #[test]
    fn y_clamped_gradient_compute() {
        let g = y_clamped_gradient(0, 10, 100.0, 200.0);
        assert_eq!(compute(&g, 0, -5, 0), 100.0); // below fromY
        assert_eq!(compute(&g, 0, 20, 0), 200.0); // above toY
        assert_eq!(compute(&g, 0, 5, 0), 150.0);
        // fromY == toY branch.
        let g2 = y_clamped_gradient(4, 4, 1.0, 2.0);
        assert_eq!(compute(&g2, 0, 3, 0), 1.0);
        assert_eq!(compute(&g2, 0, 4, 0), 2.0);
    }

    // --- RangeChoice ----------------------------------------------------------

    #[test]
    fn range_choice_branches() {
        let y = y_clamped_gradient(0, 10, 0.0, 10.0);
        let rc = range_choice(Arc::clone(&y), 3.0, 7.0, constant(100.0), constant(-100.0));
        assert_eq!(compute(&rc, 0, 2, 0), -100.0); // < minInclusive
        assert_eq!(compute(&rc, 0, 3, 0), 100.0); // == minInclusive (inclusive)
        assert_eq!(compute(&rc, 0, 6, 0), 100.0);
        assert_eq!(compute(&rc, 0, 7, 0), -100.0); // == maxExclusive (exclusive)
        assert_eq!((rc.min_value(), rc.max_value()), (-100.0, 100.0));
    }

    // --- blend / lerp ----------------------------------------------------------

    #[test]
    fn blend_alpha_offset_and_lerp() {
        assert_eq!(compute(&blend_alpha(), 0, 0, 0), 1.0);
        assert_eq!(compute(&blend_offset(), 0, 0, 0), 0.0);
        assert_eq!(blend_alpha().max_value(), 1.0);
        assert_eq!(blend_offset().min_value(), 0.0);

        // lerp(factor, 0, second):factor=0.5 → second*0.5
        let f = constant(0.5);
        let l = lerp(f, 0.0, constant(10.0));
        assert_eq!(compute(&l, 0, 0, 0), 5.0);
    }

    // --- CacheOnce / Cache2D ----------------------------------------------------

    #[test]
    fn cache_once_reuses_same_point() {
        let calls = std::rc::Rc::new(Cell::new(0u32));
        // Gradient tracking y exactly (no clamp interference).
        let inner = counting(y_clamped_gradient(0, 100, 0.0, 100.0), &calls);
        let f = cache_once(inner);
        assert_eq!(compute(&f, 5, 5, 5), 5.0);
        assert_eq!(compute(&f, 5, 5, 5), 5.0);
        assert_eq!(calls.get(), 1, "second visit to one point must hit cache");
        assert_eq!(compute(&f, 5, 6, 5), 6.0);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn cache_2d_ignores_y() {
        let calls = std::rc::Rc::new(Cell::new(0u32));
        let inner = counting(constant(7.0), &calls);
        let f = cache_2d(inner);
        assert_eq!(compute(&f, 3, 0, 4), 7.0);
        assert_eq!(compute(&f, 3, 100, 4), 7.0); // same (x,z), different y
        assert_eq!(calls.get(), 1, "cache_2d must ignore y");
        assert_eq!(compute(&f, 4, 100, 4), 7.0);
        assert_eq!(calls.get(), 2);
    }

    // --- FlatCache ----------------------------------------------------------------

    #[test]
    fn flat_cache_caches_per_chunk_and_zeroes_y() {
        let last = std::rc::Rc::new(Cell::new((0, 0, 0)));
        let recorder: Arc<dyn DensityFunction> = Arc::new(Recorder {
            last: std::rc::Rc::clone(&last),
        });
        let calls = std::rc::Rc::new(Cell::new(0u32));
        let counted = counting(recorder, &calls);
        let f = flat_cache(counted);

        // Same (x,z), different y: cache hit with y replaced by 0.
        assert_eq!(compute(&f, 3, 50, 4), 1.0);
        assert_eq!(last.get(), (3, 0, 4), "wrapped must receive y=0");
        assert_eq!(compute(&f, 3, 60, 4), 1.0);
        assert_eq!(calls.get(), 1, "second visit to one (x,z) must hit cache");

        // New (x,z) in one chunk: new slot.
        assert_eq!(compute(&f, 3, 60, 5), 1.0);
        assert_eq!(calls.get(), 2);

        // Across chunks: different cell.
        assert_eq!(compute(&f, 19, 0, 4), 1.0);
        assert_eq!(calls.get(), 3);
    }

    // --- ChunkCacheContext path ---

    #[test]
    fn marker_state_follows_chunk_cache() {
        // cache_2d state hangs off ChunkCache: recompute after swap or clear.
        let calls = std::rc::Rc::new(Cell::new(0u32));
        let inner = counting(constant(7.0), &calls);
        let f = cache_2d(inner);

        let cache1 = Rc::new(ChunkCache::new());
        let ctx1 = CellFunctionContext::new(Rc::clone(&cache1));
        ctx1.set(3, 0, 4);
        assert_eq!(f.compute(&ctx1), 7.0);
        assert_eq!(calls.get(), 1);
        ctx1.set(3, 99, 4);
        assert_eq!(f.compute(&ctx1), 7.0);
        assert_eq!(calls.get(), 1, "same (x,z) hits inside the chunk cache");

        // New chunk cache: independent state.
        let cache2 = Rc::new(ChunkCache::new());
        let ctx2 = CellFunctionContext::new(Rc::clone(&cache2));
        ctx2.set(3, 0, 4);
        assert_eq!(f.compute(&ctx2), 7.0);
        assert_eq!(calls.get(), 2, "a different ChunkCache must recompute");

        // Recompute after clear.
        cache2.clear();
        ctx2.set(3, 0, 4);
        assert_eq!(f.compute(&ctx2), 7.0);
        assert_eq!(calls.get(), 3, "must recompute after clear");
    }

    // --- CacheAllInCell ---------------------------------------------------------------

    #[test]
    fn cache_all_in_cell_fills_4x8x4_once() {
        struct Xyz;
        impl DensityFunction for Xyz {
            fn compute(&self, c: &dyn FunctionContext) -> f64 {
                (c.block_x() + c.block_y() + c.block_z()) as f64
            }
            fn fill_array(&self, _o: &mut [f64], _p: &dyn ContextProvider) {
                unimplemented!()
            }
            fn min_value(&self) -> f64 {
                f64::NEG_INFINITY
            }
            fn max_value(&self) -> f64 {
                f64::INFINITY
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
        }

        let calls = std::rc::Rc::new(Cell::new(0u32));
        // wrapped = x + y + z (validates indexing).
        let inner = counting(Arc::new(Xyz), &calls);
        let f = cache_all_in_cell(inner);

        // Visit all 128 positions of cell (0..4, 0..8, 0..4).
        for y in 0..8 {
            for z in 0..4 {
                for x in 0..4 {
                    assert_eq!(compute(&f, x, y, z), (x + y + z) as f64, "({x},{y},{z})");
                }
            }
        }
        assert_eq!(calls.get(), 128, "one cell must fill exactly 128 times");

        // Re-reads in one cell: no more evaluation.
        assert_eq!(compute(&f, 1, 1, 1), 3.0);
        assert_eq!(calls.get(), 128);

        // Neighbor cell (x direction) triggers refill.
        assert_eq!(compute(&f, 4, 0, 0), 4.0);
        assert_eq!(calls.get(), 256);

        // Negative alignment: block -1 starts a cell at -4.
        let before = calls.get();
        assert_eq!(compute(&f, -1, 0, 0), -1.0);
        assert_eq!(calls.get(), before + 128, "negative coords must start a new cell");
    }

    // --- Interpolated --------------------------------------------------------------------

    #[test]
    fn interpolated_matches_linear_function() {
        // Trilinear interpolation is exact for linear functions of y.
        let wrapped = y_clamped_gradient(0, 8, 0.0, 8.0);
        let f = interpolated(wrapped);
        assert!((compute(&f, 0, 4, 0) - 4.0).abs() < 1e-9);
        assert!((compute(&f, 3, 4, 3) - 4.0).abs() < 1e-9);
        assert!((compute(&f, 0, 1, 0) - 1.0).abs() < 1e-9);
        assert!((compute(&f, 0, 7, 0) - 7.0).abs() < 1e-9);
    }

    #[test]
    fn interpolated_reuses_cell() {
        let calls = std::rc::Rc::new(Cell::new(0u32));
        let inner = counting(y_clamped_gradient(0, 8, 0.0, 8.0), &calls);
        let f = interpolated(inner);
        // Cell fill evaluates 8 corners.
        assert_eq!(compute(&f, 1, 1, 1), 1.0);
        assert_eq!(calls.get(), 8, "filling one cell evaluates 8 corners");
        assert_eq!(compute(&f, 2, 2, 2), 2.0);
        assert_eq!(calls.get(), 8, "one cell never refills");
    }

    // --- Shift / ShiftedNoise ------------------------------------------------------------

    struct ConstNoise {
        value: f64,
        max: f64,
    }

    impl NoiseSampler for ConstNoise {
        fn get_value(&self, _x: f64, _y: f64, _z: f64) -> f64 {
            self.value
        }
        fn max_value(&self) -> f64 {
            self.max
        }
    }

    #[test]
    fn shift_family_formula() {
        // Constant noise 2.0: computeShift = 2.0 * 4.0 = 8.0.
        let holder = NoiseHolder::new(Rc::new(ConstNoise {
            value: 2.0,
            max: 2.0,
        }));
        let shift_fn: Arc<dyn DensityFunction> = Arc::new(Shift {
            offset_noise: holder,
        });
        assert_eq!(compute(&shift_fn, 1, 2, 3), 8.0);
        assert_eq!(shift_fn.max_value(), 8.0);
        assert_eq!(shift_fn.min_value(), -8.0);

        // empty NoiseHolder → 0.
        let shift_a_empty: Arc<dyn DensityFunction> = Arc::new(ShiftA {
            offset_noise: NoiseHolder::empty(),
        });
        assert_eq!(compute(&shift_a_empty, 1, 2, 3), 0.0);
    }

    #[test]
    fn shifted_noise_combines_shifts() {
        // Zero shifts with constant noise 3.0 stay 3.0.
        let f: Arc<dyn DensityFunction> = Arc::new(ShiftedNoise {
            shift_x: Arc::new(ShiftA {
                offset_noise: NoiseHolder::empty(),
            }),
            shift_y: Arc::new(Shift {
                offset_noise: NoiseHolder::empty(),
            }),
            shift_z: Arc::new(ShiftB {
                offset_noise: NoiseHolder::empty(),
            }),
            xz_scale: 0.25,
            y_scale: 0.0,
            noise: NoiseHolder::new(Rc::new(ConstNoise {
                value: 3.0,
                max: 1.5,
            })),
        });
        assert_eq!(compute(&f, 100, 50, -30), 3.0);
        assert_eq!(f.max_value(), 1.5);
    }

    // --- Spline ----------------------------------------------------------------------------

    #[test]
    fn spline_from_points_two_level() {
        // Inner: linear ridges spline mapping 0→10.
        let ridges = y_clamped_gradient(0, 1, 0.0, 1.0);
        let inner = spline_from_points(
            Arc::clone(&ridges),
            vec![p(0.0, 0.0, 0.0), p(1.0, 10.0, 0.0)],
        );
        // Outer: linear spline mapping 0→100.
        let outer = spline_from_points(
            Arc::clone(&inner),
            vec![p(0.0, 0.0, 0.0), p(10.0, 100.0, 0.0)],
        );
        // y=0.5 → inner=5 → outer=50.
        assert!((compute(&outer, 0, 0, 0) - 0.0).abs() < 1e-9);
        assert!((compute(&outer, 0, 1, 0) - 100.0).abs() < 1e-9);
        // min/max come from leaf ranges.
        assert_eq!(outer.min_value(), 0.0);
        assert_eq!(outer.max_value(), 100.0);
    }

    #[test]
    fn spline_p_fn_uses_function_values() {
        // Function-valued points evaluate in the same context.
        let y = y_clamped_gradient(0, 1, 0.0, 1.0);
        let f = spline_from_points(
            Arc::clone(&y),
            vec![
                p_fn(0.0, constant(0.0), 0.0),
                p_fn(1.0, Arc::clone(&y), 0.0),
            ],
        );
        // y=0 yields 0; y=1 yields 1.
        assert!((compute(&f, 0, 0, 0) - 0.0).abs() < 1e-9);
        assert!((compute(&f, 0, 1, 0) - 1.0).abs() < 1e-9);
    }

    // --- fill_array ---------------------------------------------------------------------------

    struct VecProvider {
        contexts: Vec<SinglePointContext>,
    }

    impl ContextProvider for VecProvider {
        fn for_index(&self, index: usize) -> &dyn FunctionContext {
            &self.contexts[index]
        }
    }

    #[test]
    fn fill_array_matches_compute() {
        let y = y_clamped_gradient(0, 10, 0.0, 10.0);
        let f = add(Arc::clone(&y), constant(1.0));
        let contexts: Vec<SinglePointContext> = (0..4).map(|i| ctx(i, i * 2, 0)).collect();
        let mut output = vec![0.0; contexts.len()];
        let provider = VecProvider { contexts };
        f.fill_array(&mut output, &provider);
        for (i, value) in output.iter().enumerate() {
            let expected = (i * 2) as f64 + 1.0;
            assert_eq!(*value, expected, "index {i}");
        }

        // Constant.fillArray fast path.
        let mut output2 = vec![0.0; 3];
        constant(9.0).fill_array(&mut output2, &provider);
        assert!(output2.iter().all(|&v| v == 9.0));
    }
}
