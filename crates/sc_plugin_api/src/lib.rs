//! Host ABI for version-pack dynamic plugins (stable C-ABI).
//!
//! Rationale: a plugin statically linking its own `sc_ecs/sc_world` copy would operate on host
//! memory with plugin code after dlopen (TypeId/registries belong to different crate
//! instances), causing segfaults. **Correct boundary**: plugins depend **only on this crate** (zero
//! deps, pure C-ABI function pointers); the host exports [`HostApiV1`] registration functions and the
//! plugin receives the API in the `HostPluginV1` `enable` callback to register callbacks.
//!
//! All data exchange uses C-compatible types (ints/pointers/C strings); no Rust structs cross FFI.

use std::os::raw::{c_char, c_void};

/// Chunk generation request (passed by value).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ChunkGenRequest {
    pub x: i32,
    pub z: i32,
    pub dimension: i32,
    pub min_y: i32,
    pub max_y: i32,
}

/// Host-provided set_block callback: plugins write blocks with it (ctx passed through from the gen callback).
pub type SetBlockFn = unsafe extern "C" fn(ctx: *mut c_void, x: u8, y: i32, z: u8, runtime_id: u32);

/// Plugin-provided generation callback (registered via [`HostApiV1::register_world_generator`]).
pub type GenerateFn =
    unsafe extern "C" fn(ctx: *mut c_void, req: *const ChunkGenRequest, set_block: SetBlockFn);

/// Host API v1: function-pointer table received at plugin enable time.
#[repr(C)]
pub struct HostApiV1 {
    /// Registers a world generator: world_kind 0=Overworld 1=Nether 2=End.
    pub register_world_generator: extern "C" fn(world_kind: i32, generate: GenerateFn),
    /// Computes a block runtime id: `FNV1a-32(name + states_json)`.
    /// Empty states_json means stateless; otherwise a JSON object like `{"snowy":false}`.
    pub hash_block_state: extern "C" fn(name: *const c_char, states_json: *const c_char) -> u32,
    pub log_info: extern "C" fn(msg: *const c_char),
    pub log_warn: extern "C" fn(msg: *const c_char),
}

/// Host plugin ABI version implemented by [`HostPluginV1`].
pub const HOST_PLUGIN_ABI_V1: u32 = 1;

/// Dynamically loaded host plugin instance.
///
/// The `context` pointer is owned by the plugin. The host must call `disable`
/// and then `destroy` before releasing the dynamic library that supplied these
/// function pointers. The plugin implementation must keep the context
/// thread-safe because registered generators can run on chunk worker threads.
#[repr(C)]
pub struct HostPluginV1 {
    pub abi_version: u32,
    pub context: *mut c_void,
    pub enable: unsafe extern "C" fn(context: *mut c_void, api: *const HostApiV1),
    pub disable: unsafe extern "C" fn(context: *mut c_void),
    pub destroy: unsafe extern "C" fn(context: *mut c_void),
}

// The ABI contract requires the plugin-owned context to be Send + Sync.
unsafe impl Send for HostPluginV1 {}
unsafe impl Sync for HostPluginV1 {}

/// Exported factory signature for a host plugin dynamic library.
pub type CreateHostPluginFn = unsafe extern "C" fn() -> HostPluginV1;
