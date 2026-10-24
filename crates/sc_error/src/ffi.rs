//! 跨 C-ABI 插件边界的错误 FFI 映射。
//!
//! Rust 错误绝不越过 `extern "C"` 边界；调用方将其转换为稳定的 `i32`，
//! 并在宿主侧记录完整错误。
//!
//! FFI mapping for errors crossing the C-ABI plugin boundary.
//!
//! Rust errors never cross `extern "C"`; callers convert to a stable
//! `i32` and log the full error on the host side.

use crate::ScCode;

/// Generic failure when no specific code applies.
pub const FFI_UNKNOWN: i32 = -1;

/// 把稳定错误码映射为 FFI 整数。已知码得到较小正整数
///
/// Map a stable code to an FFI integer. Known codes get small positive
/// values; unknown codes map to [`FFI_UNKNOWN`].
pub fn code_to_int(code: ScCode) -> i32 {
    match code.0 {
        "world.storage.io" => 1001,
        "world.storage.corrupt" => 1002,
        "world.storage.unsupported" => 1003,
        "world.storage.capacity" => 1004,
        "world.storage.backend" => 1005,
        "world.reader.invalid_world_data" => 1006,
        "world.reader.io" => 1007,
        "world.reader.serde_json" => 1008,
        "world.executor.busy" => 1009,
        "world.executor.closed" => 1010,
        "world.executor.too_many_waiters" => 1011,
        "recipe.compile" => 2001,
        "raknet.outbound.busy" => 3001,
        "raknet.outbound.closed" => 3002,
        "raknet.recv.closed" => 3010,
        "raknet.recv.old_seq" => 3011,
        "raknet.recv.reliable_window_exhausted" => 3012,
        "raknet.recv.order_channel_exhausted" => 3013,
        "raknet.send.packet_too_large" => 3020,
        "raknet.send.parse_error" => 3021,
        "raknet.send.fragment_error" => 3022,
        "raknet.send.send_error" => 3023,
        "raknet.send.recovery_full" => 3024,
        "raknet.fragment.frame_exists" => 3030,
        "raknet.fragment.frame_not_fragmented" => 3031,
        "raknet.fragment.does_not_need_split" => 3032,
        "raknet.fragment.fragment_invalid" => 3033,
        "raknet.fragment.fragments_missing" => 3034,
        "raknet.fragment.frame_index_out_of_bounds" => 3035,
        "raknet.fragment.too_many_fragments" => 3036,
        "raknet.fragment.queue_full" => 3037,
        "raknet.queue.invalid_insertion" => 3040,
        "raknet.queue.invalid_insertion_known" => 3041,
        "raknet.queue.item_deletion_fail" => 3042,
        "raknet.queue.invalid_item" => 3043,
        "raknet.queue.empty" => 3044,
        "raknet.queue.other" => 3045,
        "raknet.listener.invalid_address" => 3050,
        "raknet.listener.udp_socket_error" => 3051,
        "raknet.server.already_online" => 3060,
        "raknet.server.not_listening" => 3061,
        "raknet.server.killed" => 3062,
        "network.packet.unsupported_compressor" => 4001,
        "network.packet.io" => 4002,
        "network.packet.read" => 4003,
        "network.encryption.invalid_key" => 4010,
        "network.chunk.stale_generation" => 4020,
        "network.chunk.invalid_block_mapping" => 4021,
        "network.chunk.worker_panicked" => 4022,
        "network.chunk.submit_busy" => 4023,
        "network.chunk.submit_closed" => 4024,
        "network.chunk.submit_too_many_waiters" => 4025,
        "network.chunk.wait_closed" => 4026,
        "network.connection.send_busy" => 4030,
        "network.connection.send" => 4031,
        "network.connection.encryption_init" => 4032,
        "network.connection.recv" => 4033,
        "network.connection.encryption" => 4034,
        "network.connection.send_queue" => 4035,
        "network.connection.not_queued" => 4036,
        "network.connection.stale_context" => 4037,
        "plugin.loader.zip" => 5001,
        "plugin.loader.io" => 5002,
        "plugin.loader.lib" => 5003,
        "plugin.loader.serde" => 5004,
        "plugin.loader.invalid_abi" => 5005,
        "plugin.deps.version_mismatch" => 5010,
        "plugin.deps.missing" => 5011,
        "plugin.deps.circular" => 5012,
        "plugin.deps.failed" => 5013,
        "plugin.deps.loader" => 5014,
        "packloader.version_pack_not_found" => 5101,
        "packloader.version.zip" => 5102,
        "packloader.version.json" => 5103,
        "packloader.version.io" => 5104,
        "packloader.version.not_found" => 5105,
        "packloader.version.deserialization" => 5106,
        "packloader.split.invalid" => 5111,
        "command.duplicate_name" => 5201,
        "command.duplicate_alias" => 5202,
        "ecs.resource.not_found" => 6001,
        "game.outbox.state_budget" => 6010,
        "game.outbox.fact_budget" => 6011,
        "utils.version.unknown_version_string" => 6020,
        "utils.version.unknown_version_vec" => 6021,
        "utils.version.parse_int" => 6022,
        _ => FFI_UNKNOWN,
    }
}
