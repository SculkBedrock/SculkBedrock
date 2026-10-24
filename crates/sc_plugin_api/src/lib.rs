//! 版本包动态插件的宿主 ABI（稳定 C-ABI）。
//!
//! 插件只依赖本 crate（纯 C-ABI 函数指针），不得静态链接自己的 `sc_ecs`/`sc_world`
//! 副本去操作宿主内存。宿主导出 [`HostApiV1`] 注册函数，插件在 [`HostPluginV1`]
//! 的 `enable` 回调中取得该 API 并注册回调。
//!
//! 所有数据交换使用 C 兼容类型（整数/指针/C 字符串），Rust 结构体不跨 FFI。
//!
//! Host ABI for version-pack dynamic plugins (stable C-ABI).
//!
//! Plugins depend only on this crate (pure C-ABI function pointers) and must not
//! statically link their own `sc_ecs`/`sc_world` copy to operate on host memory.
//! The host exports [`HostApiV1`] registration functions; the plugin receives that
//! API in the [`HostPluginV1`] `enable` callback and registers its callbacks there.
//!
//! All data exchange uses C-compatible types (ints/pointers/C strings); no Rust
//! structs cross FFI.

use std::os::raw::{c_char, c_void};

/// 方块生成请求（按值传递）。
///
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

/// 宿主提供的 set_block 回调：插件用它写入方块（ctx 由生成回调原样透传）。
///
/// Host-provided set_block callback: plugins write blocks with it (ctx passed through from the gen callback).
pub type SetBlockFn = unsafe extern "C" fn(ctx: *mut c_void, x: u8, y: i32, z: u8, runtime_id: u32);

/// 插件提供的生成回调（通过 [`HostApiV1::register_world_generator`] 注册）。
///
/// Plugin-provided generation callback (registered via [`HostApiV1::register_world_generator`]).
pub type GenerateFn =
    unsafe extern "C" fn(ctx: *mut c_void, req: *const ChunkGenRequest, set_block: SetBlockFn);

/// 宿主 API v1：插件在 enable 时收到的函数指针表。
///
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

/// 动态加载的宿主插件实例。
///
/// `context` 指针由插件拥有。宿主必须先调用 `disable`，再调用 `destroy`，
/// 之后才能释放提供这些函数指针的动态库。插件实现必须保证 context 线程安全，
/// 因为已注册的生成器可能在 chunk 工作线程上运行。
///
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

/// 宿主插件动态库导出的工厂函数签名。
///
/// Exported factory signature for a host plugin dynamic library.
pub type CreateHostPluginFn = unsafe extern "C" fn() -> HostPluginV1;
