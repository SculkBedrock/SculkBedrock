//! SCDB 侧车缓存：按源文件（大小 + 修改时间）失效的旁路缓存读写。
//!
//! SCDB sidecar cache: reads and writes a bypass cache invalidated by the
//! source file (size + modification time).

use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// 缓存文件魔数。
///
/// Cache file magic.
const MAGIC: &[u8; 8] = b"SCDB\0\0\x01\0";
/// 头部长度（魔数 + 格式版本 + 源长度 + 源修改时间，各 8 字节）。
///
/// Header length (magic + format version + source length + source mtime, 8 bytes each).
const HEADER_LEN: u64 = 32;
/// 缓存格式版本。
///
/// Cache format version.
const FORMAT_VERSION: u64 = 1;
/// 锁等待间隔。
///
/// Lock polling interval.
const LOCK_WAIT: Duration = Duration::from_millis(25);
/// 锁等待超时。
///
/// Lock wait timeout.
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// 临时文件序号计数器。
///
/// Temp file sequence counter.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 计算源文件对应的缓存路径（文件名 + 路径哈希）。
///
/// Computes the cache path for a source file (file name plus path hash).
///
/// - `cache_dir`: 缓存目录
/// - `source`: 源文件路径
fn cache_path(cache_dir: &Path, source: &Path) -> PathBuf {
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("version-pack")
        .replace(['\\', '/', ':'], "_");
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.to_string_lossy().hash(&mut hasher);
    cache_dir.join(format!("{name}-{:016x}.scdb", hasher.finish()))
}

/// 缓存文件锁：以独占创建 `.scdb.lock` 实现，超时报错。
///
/// Cache file lock: implemented via exclusive creation of `.scdb.lock`;
/// errors on timeout.
struct CacheLock {
    /// 锁文件路径。
    ///
    /// Lock file path.
    path: PathBuf,
}

impl CacheLock {
    /// 获取缓存锁（带超时轮询）。
    ///
    /// Acquires the cache lock (polling with a timeout).
    ///
    /// - `cache`: 目标缓存文件路径（锁路径由其派生）
    fn acquire(cache: &Path) -> io::Result<Self> {
        let lock_path = cache.with_extension("scdb.lock");
        let started = SystemTime::now();
        loop {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(_) => return Ok(Self { path: lock_path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if started.elapsed().unwrap_or_default() >= LOCK_TIMEOUT {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("timed out waiting for SCDB lock {}", lock_path.display()),
                        ));
                    }
                    std::thread::sleep(LOCK_WAIT);
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for CacheLock {
    /// 释放时删除锁文件。
    ///
    /// Removes the lock file on drop.
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// 生成临时文件路径（进程 id + 序号保证唯一）。
///
/// Generates a temp file path (unique via process id plus sequence).
///
/// - `cache`: 目标缓存文件路径
fn temp_path(cache: &Path) -> PathBuf {
    let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    cache.with_extension(format!("scdb.tmp.{}.{}", std::process::id(), id))
}
/// 读取文件修改时间（纳秒，截断到 u64；不可用时为 0）。
///
/// Reads the file modification time in nanoseconds (saturating into u64; 0
/// when unavailable).
///
/// - `metadata`: 文件元数据
fn modified_nanos(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

/// 读取并校验缓存头部；魔数/版本不符或读取失败时为 `None`。
///
/// Reads and validates the cache header; `None` on a magic/version mismatch
/// or a read failure.
///
/// - `file`: 已打开的缓存文件
fn read_header(file: &mut File) -> io::Result<Option<(u64, u64)>> {
    let mut header = [0u8; HEADER_LEN as usize];
    if file.read_exact(&mut header).is_err() {
        return Ok(None);
    }
    if &header[..8] != MAGIC {
        return Ok(None);
    }
    let version = u64::from_le_bytes(header[8..16].try_into().unwrap());
    if version != FORMAT_VERSION {
        return Ok(None);
    }
    let source_len = u64::from_le_bytes(header[16..24].try_into().unwrap());
    let source_mtime = u64::from_le_bytes(header[24..32].try_into().unwrap());
    Ok(Some((source_len, source_mtime)))
}

/// 读取缓存内容；头部校验失败、源已变更或长度不符时为 `None`。
///
/// Reads cached bytes; `None` when the header check fails, the source has
/// changed, or lengths mismatch.
///
/// - `cache`: 缓存文件路径
/// - `source_metadata`: 源文件元数据
fn load_cached(cache: &Path, source_metadata: &fs::Metadata) -> io::Result<Option<Vec<u8>>> {
    let mut file = match File::open(cache) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some((source_len, source_mtime)) = read_header(&mut file)? else {
        return Ok(None);
    };
    if source_len != source_metadata.len() || source_mtime != modified_nanos(source_metadata) {
        return Ok(None);
    }
    let payload_len = file.metadata()?.len().saturating_sub(HEADER_LEN);
    if payload_len != source_len || payload_len > usize::MAX as u64 {
        return Ok(None);
    }
    let mut payload = Vec::with_capacity(payload_len as usize);
    file.read_to_end(&mut payload)?;
    if payload.len() as u64 != payload_len {
        return Ok(None);
    }
    Ok(Some(payload))
}

/// 原子写入缓存（临时文件 + rename 替换）；失败时清理临时文件。
///
/// Atomically writes the cache (temp file plus rename replace); cleans up
/// the temp file on failure.
///
/// - `cache`: 缓存文件路径
/// - `source_metadata`: 源文件元数据
/// - `payload`: 待缓存内容
fn write_cached(cache: &Path, source_metadata: &fs::Metadata, payload: &[u8]) -> io::Result<()> {
    let temp = temp_path(cache);
    let result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp)?;
        file.write_all(MAGIC)?;
        file.write_all(&FORMAT_VERSION.to_le_bytes())?;
        file.write_all(&source_metadata.len().to_le_bytes())?;
        file.write_all(&modified_nanos(source_metadata).to_le_bytes())?;
        file.write_all(payload)?;
        file.sync_all()?;
        replace_cache(&temp, cache)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// 用临时文件替换目标缓存（目标已存在时先删除再改名）。
///
/// Replaces the target cache with the temp file (removing the target first
/// when it already exists).
///
/// - `temp`: 临时文件路径
/// - `cache`: 目标缓存路径
fn replace_cache(temp: &Path, cache: &Path) -> io::Result<()> {
    match fs::rename(temp, cache) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            fs::remove_file(cache)?;
            fs::rename(temp, cache)
        }
        Err(error) => Err(error),
    }
}

/// 返回源文件的缓存路径；缓存无效时在锁保护下重建（流式拷贝）。
///
/// Returns the cache path for a source file; rebuilds it under the lock
/// (streaming copy) when the cache is invalid.
///
/// - `source`: 源文件路径
/// - `cache_dir`: 缓存目录
pub fn load_or_compile_path(source: &Path, cache_dir: &Path) -> io::Result<PathBuf> {
    let metadata = fs::metadata(source)?;
    let cache = cache_path(cache_dir, source);
    let valid = File::open(&cache)
        .ok()
        .and_then(|mut file| read_header(&mut file).ok().flatten())
        .map(|(source_len, source_mtime)| {
            source_len == metadata.len() && source_mtime == modified_nanos(&metadata)
        })
        .unwrap_or(false);
    if valid {
        return Ok(cache);
    }

    fs::create_dir_all(cache_dir)?;
    let _lock = CacheLock::acquire(&cache)?;
    let valid_after_lock = File::open(&cache)
        .ok()
        .and_then(|mut file| read_header(&mut file).ok().flatten())
        .map(|(source_len, source_mtime)| {
            source_len == metadata.len() && source_mtime == modified_nanos(&metadata)
        })
        .unwrap_or(false);
    if valid_after_lock {
        return Ok(cache);
    }
    let temp = temp_path(&cache);
    let result = (|| {
        let mut output = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp)?;
        output.write_all(MAGIC)?;
        output.write_all(&FORMAT_VERSION.to_le_bytes())?;
        output.write_all(&metadata.len().to_le_bytes())?;
        output.write_all(&modified_nanos(&metadata).to_le_bytes())?;
        let mut input = File::open(source)?;
        io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        replace_cache(&temp, &cache)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(cache)
}

/// 返回源文件内容；命中缓存时直接读缓存，否则读源并尽力持久化缓存。
///
/// Returns the source file contents; reads the cache on a hit, otherwise
/// reads the source and persists the cache best-effort.
///
/// - `source`: 源文件路径
/// - `cache_dir`: 缓存目录
pub fn load_or_compile(source: &Path, cache_dir: &Path) -> io::Result<Vec<u8>> {
    let metadata = fs::metadata(source)?;
    let cache = cache_path(cache_dir, source);
    if let Some(payload) = load_cached(&cache, &metadata)? {
        return Ok(payload);
    }

    let mut source_file = File::open(source)?;
    let mut payload = Vec::with_capacity(metadata.len().min(usize::MAX as u64) as usize);
    source_file.read_to_end(&mut payload)?;
    if let Err(error) =
        fs::create_dir_all(cache_dir).and_then(|_| write_cached(&cache, &metadata, &payload))
    {
        log::debug!(
            "could not persist SCDB cache {}: {}",
            cache.display(),
            error
        );
    }
    Ok(payload)
}

/// 清理缓存目录中残留的临时文件（`.scdb.tmp.`）。
///
/// Removes leftover temp files (`.scdb.tmp.`) from the cache directory.
///
/// - `cache_dir`: 缓存目录
pub fn clear_stale_cache(cache_dir: &Path) {
    let Ok(entries) = fs::read_dir(cache_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if file_name.contains(".scdb.tmp.") {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Per-process counter: parallel tests must never share a directory.
    fn temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "scdb-test-{}-{stamp}-{seq}",
            std::process::id()
        ))
    }

    #[test]
    fn creates_and_reuses_sidecar_cache() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("pack.scver");
        fs::write(&source, b"zip payload").unwrap();
        let cache_dir = dir.join(".scdb");

        let first = load_or_compile_path(&source, &cache_dir).unwrap();
        let second = load_or_compile_path(&source, &cache_dir).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            &fs::read(&first).unwrap()[HEADER_LEN as usize..],
            b"zip payload"
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_change_invalidates_cache() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("pack.scver");
        let cache_dir = dir.join(".scdb");
        fs::write(&source, b"old").unwrap();
        let cache = load_or_compile_path(&source, &cache_dir).unwrap();
        fs::write(&source, b"new payload").unwrap();
        let refreshed = load_or_compile_path(&source, &cache_dir).unwrap();
        assert_eq!(cache, refreshed);
        assert_eq!(
            &fs::read(&refreshed).unwrap()[HEADER_LEN as usize..],
            b"new payload"
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_cache_is_rebuilt() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("pack.scver");
        let cache_dir = dir.join(".scdb");
        fs::write(&source, b"payload").unwrap();
        let cache = load_or_compile_path(&source, &cache_dir).unwrap();
        let mut bytes = fs::read(&cache).unwrap();
        bytes[0] = b'X';
        fs::write(&cache, bytes).unwrap();
        let rebuilt = load_or_compile_path(&source, &cache_dir).unwrap();
        assert_eq!(
            &fs::read(&rebuilt).unwrap()[HEADER_LEN as usize..],
            b"payload"
        );

        fs::remove_dir_all(dir).unwrap();
    }
}
