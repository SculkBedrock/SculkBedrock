//! Density function interfaces and per-chunk evaluation contexts.
//!
//! Notes:
//! - Interface default combinators (`clamp`/`abs`/`square`/...) live as free
//!   functions in [`super::common`]; tree nodes are `Arc<dyn DensityFunction>`,
//!   so free-function composition is used directly.
//! - Fill delegation is inlined in each implementation; there is no separate
//!   simple-function trait.
//! - The whole density-function tree assumes single-threaded use: interior
//!   state uses `Cell`/`RefCell`/`Rc`. Parallel chunk generation must use a
//!   separate tree per thread/chunk or external serialization.
//! - The chunk-cache context infrastructure lives in this file because the
//!   downcast hooks need it (see below).

use crate::worldgen::noise::noise::NormalNoise;
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// Density function interface.
pub trait DensityFunction {
    /// Samples the density value at the given context.
    fn compute(&self, context: &dyn FunctionContext) -> f64;

    /// Fills `output` by sampling each slot's context.
    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider);

    /// Minimum possible value.
    fn min_value(&self) -> f64;

    /// Maximum possible value.
    fn max_value(&self) -> f64;

    /// Downcast hook for dispatch by concrete type (e.g. constant detection
    /// in two-argument functions). All implementations return `self`.
    fn as_any(&self) -> &dyn Any;
}

/// Context provider interface.
pub trait ContextProvider {
    /// Returns the context for `index`.
    fn for_index(&self, index: usize) -> &dyn FunctionContext;

    /// Fills `output` by computing each slot in order.
    fn fill_all_directly(&self, output: &mut [f64], function: &dyn DensityFunction) {
        for (i, slot) in output.iter_mut().enumerate() {
            *slot = function.compute(self.for_index(i));
        }
    }
}

/// Evaluation context holding a block position.
pub trait FunctionContext {
    /// Block X coordinate.
    fn block_x(&self) -> i32;

    /// Block Y coordinate.
    fn block_y(&self) -> i32;

    /// Block Z coordinate.
    fn block_z(&self) -> i32;

    /// Downcast hook for chunk-cache contexts;
    /// chunk-cache context types override this with `Some(self)`.
    fn as_chunk_cache_context(&self) -> Option<&dyn ChunkCacheContext> {
        None
    }
}

/// Chunk-cache context: exposes the per-chunk density cache.
pub trait ChunkCacheContext: FunctionContext {
    /// Returns the per-chunk density cache.
    ///
    /// The cache is shared via `Rc` (marker state stores a clone in the
    /// reusable context), so this returns a clone.
    fn density_chunk_cache(&self) -> Rc<ChunkCache>;
}

/// Per-chunk density cache.
///
/// Caches per-chunk state by marker instance identity.
/// Key is the marker's globally unique id (allocated by the counter in
/// [`super::common`]); value is `Rc<dyn Any>` (concrete type
/// `Rc<RefCell<S>>`). The entry is cloned and the map borrow released
/// before borrowing the inner `RefCell`, so nested marker access to the
/// same map cannot deadlock.
pub struct ChunkCache {
    states: RefCell<HashMap<u64, Rc<dyn Any>>>,
}

impl ChunkCache {
    pub fn new() -> Self {
        Self {
            states: RefCell::new(HashMap::new()),
        }
    }

    /// Returns the state for a marker, creating it on first use.
    ///
    /// State is constructed on demand (first creation via `S::default()`).
    pub(crate) fn get_or_create<S: Default + 'static>(&self, marker_id: u64) -> Rc<RefCell<S>> {
        let existing: Rc<dyn Any> = {
            let mut map = self.states.borrow_mut();
            match map.get(&marker_id) {
                Some(rc) => Rc::clone(rc),
                None => {
                    let rc: Rc<dyn Any> = Rc::new(RefCell::new(S::default()));
                    map.insert(marker_id, Rc::clone(&rc));
                    rc
                }
            }
        };
        existing
            .downcast::<RefCell<S>>()
            .expect("density marker state type mismatch")
    }

    /// Clears all cached marker states.
    pub fn clear(&self) {
        self.states.borrow_mut().clear();
    }
}

impl Default for ChunkCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Noise sampler interface.
pub trait NoiseSampler {
    /// Samples the noise value at the given position.
    fn get_value(&self, x: f64, y: f64, z: f64) -> f64;

    /// Maximum sampled value.
    fn max_value(&self) -> f64;
}

/// Nullable noise-sampler holder.
pub struct NoiseHolder {
    /// Wrapped sampler (`None` means empty).
    noise: Option<Rc<dyn NoiseSampler>>,
}

impl NoiseHolder {
    /// Builds a holder from a normal noise sampler.
    pub fn from_normal_noise(noise: Arc<NormalNoise>) -> Self {
        Self {
            noise: Some(Rc::new(NormalNoiseAdapter(noise))),
        }
    }

    /// Builds a holder from a sampler.
    pub fn new(noise: Rc<dyn NoiseSampler>) -> Self {
        Self { noise: Some(noise) }
    }

    /// Builds an empty holder with no sampler.
    pub fn empty() -> Self {
        Self { noise: None }
    }

    /// Samples the value (empty holder returns 0.0).
    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        match &self.noise {
            None => 0.0,
            Some(noise) => noise.get_value(x, y, z),
        }
    }

    /// Returns the maximum value (empty holder returns 2.0).
    pub fn max_value(&self) -> f64 {
        match &self.noise {
            None => 2.0,
            Some(noise) => noise.max_value(),
        }
    }
}

/// Adapter from normal noise to the sampler interface.
struct NormalNoiseAdapter(Arc<NormalNoise>);

impl NoiseSampler for NormalNoiseAdapter {
    fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        // Normal noise samples `f32`; the adapter widens to `f64`.
        self.0.get_value(x, y, z) as f64
    }

    fn max_value(&self) -> f64 {
        self.0.get_max()
    }
}

/// Single-point evaluation context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SinglePointContext {
    pub block_x: i32,
    pub block_y: i32,
    pub block_z: i32,
}

impl FunctionContext for SinglePointContext {
    fn block_x(&self) -> i32 {
        self.block_x
    }

    fn block_y(&self) -> i32 {
        self.block_y
    }

    fn block_z(&self) -> i32 {
        self.block_z
    }
}

/// Reusable mutable evaluation context.
///
/// Reusable mutable context (`set` updates through `Cell` via `&self`).
#[derive(Debug)]
pub struct MutableFunctionContext {
    block_x: Cell<i32>,
    block_y: Cell<i32>,
    block_z: Cell<i32>,
}

impl MutableFunctionContext {
    pub fn new() -> Self {
        Self {
            block_x: Cell::new(0),
            block_y: Cell::new(0),
            block_z: Cell::new(0),
        }
    }

    /// Sets the position and returns `self` for chaining.
    pub fn set(&self, block_x: i32, block_y: i32, block_z: i32) -> &Self {
        self.block_x.set(block_x);
        self.block_y.set(block_y);
        self.block_z.set(block_z);
        self
    }
}

impl Default for MutableFunctionContext {
    fn default() -> Self {
        Self::new()
    }
}

impl FunctionContext for MutableFunctionContext {
    fn block_x(&self) -> i32 {
        self.block_x.get()
    }

    fn block_y(&self) -> i32 {
        self.block_y.get()
    }

    fn block_z(&self) -> i32 {
        self.block_z.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutable_context_set_and_read() {
        let ctx = MutableFunctionContext::new();
        ctx.set(3, -5, 7);
        assert_eq!((ctx.block_x(), ctx.block_y(), ctx.block_z()), (3, -5, 7));
        // Reuses the same context through chaining.
        ctx.set(1, 2, 3).set(4, 5, 6);
        assert_eq!((ctx.block_x(), ctx.block_y(), ctx.block_z()), (4, 5, 6));
    }

    #[test]
    fn empty_noise_holder_defaults() {
        // Java L88-94:null noise → getValue 0.0 / maxValue 2.0.
        let holder = NoiseHolder::empty();
        assert_eq!(holder.get_value(1.0, 2.0, 3.0), 0.0);
        assert_eq!(holder.max_value(), 2.0);
    }

    #[test]
    fn chunk_cache_get_or_create_is_stable() {
        // Same marker id returns the same state; rebuilt after clear.
        let cache = ChunkCache::new();
        let s1 = cache.get_or_create::<u32>(1);
        *s1.borrow_mut() = 42;
        let s2 = cache.get_or_create::<u32>(1);
        assert_eq!(*s2.borrow(), 42);
        assert!(Rc::ptr_eq(&s1, &s2));
        cache.clear();
        let s3 = cache.get_or_create::<u32>(1);
        assert_eq!(*s3.borrow(), 0);
    }
}
