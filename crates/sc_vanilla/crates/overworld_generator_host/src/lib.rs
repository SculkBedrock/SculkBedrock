use std::ffi::CString;
use std::os::raw::{c_char, c_void};
use std::sync::OnceLock;

use sc_plugin_api::{ChunkGenRequest, HostApiV1, HostPluginV1, SetBlockFn, HOST_PLUGIN_ABI_V1};

struct OverworldPluginState;

static BLOCKS: OnceLock<[u32; 3]> = OnceLock::new();

fn hash_block(api: &HostApiV1, name: &str, states: &str) -> u32 {
    let Ok(name) = CString::new(name) else {
        return 0;
    };
    let Ok(states) = CString::new(states) else {
        return 0;
    };
    (api.hash_block_state)(name.as_ptr(), states.as_ptr())
}

fn log_message(log: extern "C" fn(*const c_char), message: &str) {
    let Ok(message) = CString::new(message) else {
        return;
    };
    log(message.as_ptr());
}

unsafe extern "C" fn enable_plugin(context: *mut c_void, api: *const HostApiV1) {
    if context.is_null() || api.is_null() {
        return;
    }

    let api = unsafe { &*api };
    let blocks = [
        // 1.26.40 canonical states (checked against block_palette.nbt):
        // grass_block has no snowy property; bedrock adds infiniburn_bit (default false).
        hash_block(api, "minecraft:grass_block", ""),
        hash_block(api, "minecraft:dirt", ""),
        hash_block(api, "minecraft:bedrock", r#"{"infiniburn_bit":false}"#),
    ];
    let _ = BLOCKS.set(blocks);
    (api.register_world_generator)(0, generate_overworld);
    log_message(api.log_info, "sc_vanilla_overworld: HostPluginV1 enabled");
}

unsafe extern "C" fn disable_plugin(_context: *mut c_void) {}

unsafe extern "C" fn destroy_plugin(context: *mut c_void) {
    if !context.is_null() {
        drop(unsafe { Box::from_raw(context as *mut OverworldPluginState) });
    }
}

unsafe extern "C" fn generate_overworld(
    context: *mut c_void,
    request: *const ChunkGenRequest,
    set_block: SetBlockFn,
) {
    if context.is_null() || request.is_null() {
        return;
    }

    let request = unsafe { &*request };
    let [grass, dirt, bedrock] = *BLOCKS.get().unwrap_or(&[0, 0, 0]);
    for x in 0..16u8 {
        for z in 0..16u8 {
            unsafe {
                set_block(context, x, request.min_y, z, bedrock);
                set_block(context, x, request.min_y + 1, z, dirt);
                set_block(context, x, request.min_y + 2, z, dirt);
                set_block(context, x, request.min_y + 3, z, grass);
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn get_host_plugin() -> HostPluginV1 {
    HostPluginV1 {
        abi_version: HOST_PLUGIN_ABI_V1,
        context: Box::into_raw(Box::new(OverworldPluginState)) as *mut c_void,
        enable: enable_plugin,
        disable: disable_plugin,
        destroy: destroy_plugin,
    }
}
