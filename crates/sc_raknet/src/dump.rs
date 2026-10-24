use crate::protocol::frame::FramePacket;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use sc_binary::interfaces::Reader;
use sc_log::t_log;

/// 抓包落盘状态：目录、索引文件与自增序号。
///
/// Packet-dump state: target directory, index file, and an incrementing
/// sequence number.
struct DumpState {
    /// 输出目录（由 `SC_PACKET_DUMP_DIR` 控制）。
    ///
    /// Output directory (controlled by `SC_PACKET_DUMP_DIR`).
    dir: PathBuf,
    /// 索引文件句柄：每行记录一个数据包。
    ///
    /// Index file handle: one line per recorded datagram.
    index: File,
    /// 已记录的数据包序号，从 1 开始。
    ///
    /// Number of recorded datagrams, starting from 1.
    sequence: u64,
}

/// 全局状态单例：仅在启用抓包时初始化。
///
/// Global state singleton: initialized only when dumping is enabled.
static STATE: OnceLock<Mutex<Option<DumpState>>> = OnceLock::new();
/// 抓包总开关的环境变量缓存。
///
/// Cached value of the master dump switch environment variable.
static ENABLED: OnceLock<bool> = OnceLock::new();
/// HEX 输出开关的环境变量缓存。
///
/// Cached value of the hex-output switch environment variable.
static HEX_ENABLED: OnceLock<bool> = OnceLock::new();

/// 抓包开关，每个进程只读取一次。
///
/// `record()` 在每个 RakNet 数据报上都会执行（比协议层更热），
/// 而 `std::env::var` 每次调用都会分配 `String`；
/// 环境变量在进程启动前就已固定，因此缓存一次即可。
///
/// Whether dumping is enabled; read once per process.
///
/// `record()` runs on every RakNet datagram (hotter than the protocol
/// layer), and `std::env::var` allocates a `String` per call.
/// Environment variables are fixed before process start.
fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var("SC_PACKET_DUMP")
            .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false)
    })
}

/// HEX 输出开关，每个进程只读取一次。
///
/// Whether hex output is enabled; read once per process.
fn hex_enabled() -> bool {
    *HEX_ENABLED.get_or_init(|| {
        std::env::var("SC_PACKET_DUMP_HEX")
            .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false)
    })
}

/// 获取（必要时初始化）全局抓包状态；未启用时返回 `None`。
///
/// Returns (initializing if needed) the global dump state; `None` when
/// dumping is disabled.
fn state() -> Option<&'static Mutex<Option<DumpState>>> {
    if !enabled() {
        return None;
    }

    Some(STATE.get_or_init(|| {
        let dir = std::env::var_os("SC_PACKET_DUMP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("diagnostics/sc-2168/logs/packet-dump"));

        if let Err(error) = create_dir_all(&dir) {
            log::warn!("{}", t_log!("console.raknet.dump_create_fail", dir = dir.display(), error = error));
            return Mutex::new(None);
        }

        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("sc-raknet.index"))
        {
            Ok(index) => Mutex::new(Some(DumpState {
                dir,
                index,
                sequence: 0,
            })),
            Err(error) => {
                log::warn!("{}", t_log!("console.raknet.dump_index_fail", error = error));
                Mutex::new(None)
            }
        }
    }))
}

/// 把字符串清洗为安全文件名片段（仅保留 ASCII 字母数字与 `-_.`）。
///
/// Sanitizes a string into a safe filename fragment (keeps only ASCII
/// alphanumerics plus `-_.`).
fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// 计算 FNV-1a 摘要，仅用于诊断定位首次分歧。
///
/// 保持此诊断用哈希零依赖；FNV 足以定位首次分歧，Minecraft 层另记录
/// SHA-256。
///
/// Computes an FNV-1a digest, used only to locate a first divergence in
/// diagnostics.
///
/// Keep this diagnostic-only hash dependency-free. FNV is sufficient for
/// locating a first divergence; the Minecraft layer records SHA-256.
fn sha256(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// 把字节数组完整格式化为小写十六进制字符串。
///
/// Formats a byte slice into a full lowercase hex string.
fn full_hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push_str(&format!("{byte:02x}"));
    }
    result
}

/// 解析数据包负载，生成一行人类可读的帧摘要。
///
/// Parses a datagram payload into a human-readable one-line frame summary.
fn frame_details(payload: &[u8]) -> String {
    let Some(packet_id) = payload.first().copied() else {
        return "empty".to_string();
    };

    if (0x80..=0x8d).contains(&packet_id) {
        if let Ok(packet) = FramePacket::read_from_slice(payload) {
            let mut details = format!("packet_id=0x{packet_id:02x} sequence={}", packet.sequence);
            for (index, frame) in packet.frames.iter().enumerate() {
                details.push_str(&format!(
                    " frame{index}={{reliability={:?},reliable={:?},order={:?},channel={:?},split={:?},body={}}}",
                    frame.reliability,
                    frame.reliable_index,
                    frame.order_index,
                    frame.order_channel,
                    frame.fragment_meta,
                    frame.body.len()
                ));
            }
            return details;
        }
    }

    match packet_id {
        0xc0 => format!("packet_id=0xc0 ack bytes={}", payload.len()),
        0xa0 => format!("packet_id=0xa0 nack bytes={}", payload.len()),
        _ => format!("packet_id=0x{packet_id:02x} bytes={}", payload.len()),
    }
}

/// 记录一个数据包：写 `.bin` 负载、（可选）`.hex` 文本与索引行。
///
/// Records one datagram: writes the `.bin` payload, optional `.hex` text,
/// and an index line.
///
/// - `direction`: 传输方向标识（如 `in`/`out`）
/// - `address`: 对端地址
/// - `payload`: 原始数据报负载
/// - `kind`: 调用方提供的数据包分类
pub fn record(direction: &str, address: SocketAddr, payload: &[u8], kind: &str) {
    let Some(state) = state() else {
        return;
    };
    let Ok(mut state) = state.lock() else {
        return;
    };
    let Some(state) = state.as_mut() else {
        return;
    };

    state.sequence = state.sequence.wrapping_add(1);
    let sequence = state.sequence;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let details = frame_details(payload);
    let base = format!(
        "{sequence:09}-{direction}-{}-{}",
        sanitize(kind),
        payload.first().copied().unwrap_or(0)
    );
    let path = state.dir.join(format!("{base}.bin"));
    let file = match File::create(&path).and_then(|mut file| file.write_all(payload).map(|_| file))
    {
        Ok(_) => {
            if hex_enabled() {
                if let Ok(mut hex_file) = File::create(state.dir.join(format!("{base}.hex"))) {
                    let _ = hex_file.write_all(full_hex(payload).as_bytes());
                }
            }
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("-")
                .to_string()
        }
        Err(error) => {
            log::warn!("{}", t_log!("console.raknet.dump_write_fail", path = path.display(), error = error));
            "-".to_string()
        }
    };

    let _ = writeln!(
        state.index,
        "seq={sequence} timestamp={timestamp} direction={direction} address={address} kind={} bytes={} hash={} file={} {}",
        sanitize(kind),
        payload.len(),
        sha256(payload),
        file,
        sanitize(&details),
    );
    let _ = state.index.flush();
}
