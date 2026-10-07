//! Durable disk environment for the Bedrock LevelDB backend.
//!
//! `rusty-leveldb 3.0.3` routes a *synced* write through
//! `DB::write(batch, true)` → `LogWriter::flush()` → `std::io::Write::flush()`.
//! On a `std::fs::File` that call is a no-op: it pushes the bytes out of the
//! process but never asks the OS to write them to stable storage. A power loss
//! can therefore drop writes this backend already reported as persisted.
//!
//! This module wraps [`PosixDiskEnv`] and only changes the write-ahead log
//! writer: every `flush()` first issues `File::sync_data()`, so a synced batch
//! really is on the device before the storage reply is sent. Everything else
//! (sstable writes, MANIFEST handling, locking, directory listing) keeps the
//! upstream behaviour on purpose — sstable builders flush per block and paying
//! an fsync there would throttle compaction.
//!
//! Scope: this makes the *acknowledged* WAL durable. It is not a full
//! power-loss audit — the database directory itself is fsynced by
//! [`fsync_dir`] after close, and MANIFEST/CURRENT renames are only as durable
//! as the previous checkpoint (LevelDB tolerates replaying an older CURRENT).

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rusty_leveldb::env::{Env, FileLock, Logger, RandomAccess};
use rusty_leveldb::{PosixDiskEnv, Result, Status, StatusCode};
use sc_log::t_log;

/// Wrap a writer so that `flush()` also issues `sync_data()`.
struct DurableLogWriter {
    file: File,
}

impl DurableLogWriter {
    fn new(path: &Path, append: bool) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(path)
            .map_err(|error| {
                Status::from(error).annotate(format!("open (durable): {}", path.display()))
            })?;
        Ok(Self { file })
    }
}

impl Write for DurableLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        // `flush` first: never skip the userspace→kernel hand-off.
        self.file.flush()?;
        self.file.sync_data()?;
        Ok(())
    }
}

/// Default [`Env`] with a durable write-ahead log logger.
pub struct DurableDiskEnv {
    inner: PosixDiskEnv,
}

impl Default for DurableDiskEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl DurableDiskEnv {
    pub fn new() -> Self {
        Self {
            inner: PosixDiskEnv::new(),
        }
    }

    /// Shared handle for [`Options::env`](rusty_leveldb::Options::env).
    pub fn shared() -> Rc<Box<dyn Env>> {
        Rc::new(Box::new(Self::new()))
    }
}

impl Env for DurableDiskEnv {
    fn open_sequential_file(&self, path: &Path) -> Result<Box<dyn Read>> {
        if is_write_ahead_log(path) {
            // Upstream recovery silently stops on reader errors and can join an
            // unfinished FIRST with the next FULL. Validate before replaying.
            let end = complete_wal_prefix(File::open(path)?)
                .map_err(|error| error.annotate(format!("WAL {}", path.display())))?;
            return Ok(Box::new(File::open(path)?.take(end)));
        }
        self.inner.open_sequential_file(path)
    }

    fn open_random_access_file(&self, path: &Path) -> Result<Box<dyn RandomAccess>> {
        self.inner.open_random_access_file(path)
    }

    fn open_writable_file(&self, path: &Path) -> Result<Box<dyn Write>> {
        if is_write_ahead_log(path) {
            return Ok(Box::new(DurableLogWriter::new(path, false)?));
        }
        self.inner.open_writable_file(path)
    }

    fn open_appendable_file(&self, path: &Path) -> Result<Box<dyn Write>> {
        if is_write_ahead_log(path) {
            return Ok(Box::new(DurableLogWriter::new(path, true)?));
        }
        self.inner.open_appendable_file(path)
    }

    fn exists(&self, path: &Path) -> Result<bool> {
        self.inner.exists(path)
    }

    fn children(&self, path: &Path) -> Result<Vec<PathBuf>> {
        self.inner.children(path)
    }

    fn size_of(&self, path: &Path) -> Result<usize> {
        self.inner.size_of(path)
    }

    fn delete(&self, path: &Path) -> Result<()> {
        self.inner.delete(path)
    }

    fn mkdir(&self, path: &Path) -> Result<()> {
        self.inner.mkdir(path)
    }

    fn rmdir(&self, path: &Path) -> Result<()> {
        self.inner.rmdir(path)
    }

    fn rename(&self, old: &Path, new: &Path) -> Result<()> {
        self.inner.rename(old, new)
    }

    fn lock(&self, path: &Path) -> Result<FileLock> {
        self.inner.lock(path)
    }

    fn unlock(&self, lock: FileLock) -> Result<()> {
        self.inner.unlock(lock)
    }

    fn new_logger(&self, path: &Path) -> Result<Logger> {
        self.inner.new_logger(path)
    }

    fn micros(&self) -> u64 {
        self.inner.micros()
    }

    fn sleep_for(&self, micros: u32) {
        self.inner.sleep_for(micros);
    }
}

/// fsync a directory so that creates/renames inside it survive a power loss.
///
/// Best effort: platforms without directory fsync simply skip it.
pub fn fsync_dir(path: &Path) -> io::Result<()> {
    let directory = File::open(path)?;
    directory.sync_all()
}

/// Best-effort directory fsync used after the database is closed.
pub fn fsync_dir_best_effort(path: &Path) {
    if let Err(error) = fsync_dir(path) {
        log::warn!(
            "{}",
            t_log!(
                "console.leveldb.fsync_fail",
                path = path.display(),
                error = error
            )
        );
    }
}

/// Whether a path belongs to the write-ahead log: the only file whose flush
/// this environment turns into an `fsync`.
fn is_write_ahead_log(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("log"))
}

/// Only expose complete, checksummed logical records to upstream recovery.
/// A torn final record is recoverable; malformed records in the middle fail
/// opening the database rather than publishing mixed WriteBatch contents.
fn complete_wal_prefix(mut source: impl Read) -> Result<u64> {
    const BLOCK: usize = 32768;
    const HEADER: usize = 7;
    const CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISCSI);
    let corrupt = |offset: u64, message: &str| {
        Status::new(
            StatusCode::Corruption,
            &format!("offset {offset}: {message}"),
        )
    };
    let mut block = [0u8; BLOCK];
    let mut base = 0u64;
    let mut complete = 0u64;
    let mut fragmented = false;
    loop {
        let mut size = 0;
        while size < BLOCK {
            match source.read(&mut block[size..]) {
                Ok(0) => break,
                Ok(n) => size += n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(Status::from(error)),
            }
        }
        let mut offset = 0;
        while offset + HEADER <= size {
            let header = &block[offset..offset + HEADER];
            if header.iter().all(|byte| *byte == 0) {
                if block[offset..size].iter().any(|byte| *byte != 0) {
                    return Err(corrupt(base + offset as u64, "nonzero WAL padding"));
                }
                offset = size;
                break;
            }
            let length = u16::from_le_bytes([header[4], header[5]]) as usize;
            let end = offset + HEADER + length;
            if end > BLOCK {
                return Err(corrupt(
                    base + offset as u64,
                    "fragment crosses block boundary",
                ));
            }
            if end > size {
                return Ok(complete);
            }
            let kind = header[6];
            let mut digest = CRC.digest();
            digest.update(&[kind]);
            digest.update(&block[offset + HEADER..end]);
            let checksum = u32::from_le_bytes(header[..4].try_into().unwrap());
            let actual = checksum.wrapping_sub(0xa282ead8).rotate_right(17);
            if actual != digest.finalize() {
                return Err(corrupt(base + offset as u64, "invalid fragment checksum"));
            }
            match (kind, fragmented) {
                (1, false) => complete = base + end as u64,
                (2, false) => fragmented = true,
                (3, true) => {}
                (4, true) => {
                    fragmented = false;
                    complete = base + end as u64;
                }
                _ => return Err(corrupt(base + offset as u64, "invalid fragment sequence")),
            }
            offset = end;
        }
        if size == BLOCK && block[offset..size].iter().any(|byte| *byte != 0) {
            return Err(corrupt(base + offset as u64, "nonzero block trailer"));
        }
        if size < BLOCK {
            return Ok(complete);
        }
        base += BLOCK as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusty_leveldb::{Options, WriteBatch, DB};
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn fragment(kind: u8, payload: &[u8]) -> Vec<u8> {
        const CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISCSI);
        let mut digest = CRC.digest();
        digest.update(&[kind]);
        digest.update(payload);
        let checksum = digest.finalize().rotate_right(15).wrapping_add(0xa282ead8);
        let mut bytes = checksum.to_le_bytes().to_vec();
        bytes.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        bytes.push(kind);
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn wal_rejects_new_record_inside_unfinished_fragment() {
        let mut bytes = fragment(2, b"unfinished batch");
        bytes.extend(fragment(1, b"different batch"));
        assert!(complete_wal_prefix(bytes.as_slice()).is_err());
        assert!(complete_wal_prefix(fragment(4, b"orphan").as_slice()).is_err());
    }

    #[test]
    fn wal_only_replays_complete_prefix_after_torn_tail() {
        let first = fragment(1, b"complete batch");
        let mut bytes = first.clone();
        bytes.extend(fragment(2, b"unfinished batch"));
        assert_eq!(
            complete_wal_prefix(bytes.as_slice()).unwrap(),
            first.len() as u64
        );
        let mut bytes = first.clone();
        let mut torn = fragment(1, b"torn batch");
        torn.pop();
        bytes.extend(torn);
        assert_eq!(
            complete_wal_prefix(bytes.as_slice()).unwrap(),
            first.len() as u64
        );
    }

    #[test]
    fn environment_filters_torn_tail_without_changing_the_file() {
        let directory = TempDir::new("wal-torn-tail");
        let path = directory.0.join("000001.log");
        let complete = fragment(1, b"complete batch");
        let mut bytes = complete.clone();
        bytes.extend(fragment(2, b"unfinished batch"));
        fs::write(&path, &bytes).unwrap();
        let mut replay = Vec::new();
        DurableDiskEnv::new()
            .open_sequential_file(&path)
            .unwrap()
            .read_to_end(&mut replay)
            .unwrap();
        assert_eq!(replay, complete);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        bytes.extend(fragment(1, b"different batch"));
        fs::write(&path, &bytes).unwrap();
        assert!(DurableDiskEnv::new().open_sequential_file(&path).is_err());
    }

    #[test]
    fn wal_validates_checksums_and_block_spanning_records() {
        let mut bytes = fragment(2, &vec![42; 32768 - 7]);
        bytes.extend(fragment(4, b"last fragment"));
        assert_eq!(
            complete_wal_prefix(bytes.as_slice()).unwrap(),
            bytes.len() as u64
        );
        bytes[10] ^= 1;
        assert!(complete_wal_prefix(bytes.as_slice()).is_err());
    }

    #[test]
    fn wal_create_truncates_and_append_preserves_existing_bytes() {
        let directory = TempDir::new("wal-open-modes");
        let path = directory.0.join("000001.log");
        fs::write(&path, b"old bytes").unwrap();
        let env = DurableDiskEnv::new();
        env.open_writable_file(&path)
            .unwrap()
            .write_all(b"new")
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        env.open_appendable_file(&path)
            .unwrap()
            .write_all(b" appended")
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new appended");
    }

    #[test]
    fn reopened_database_preserves_block_spanning_batches() {
        let directory = TempDir::new("wal-reopen");
        let mut options = crate::leveldb::bedrock_options();
        options.create_if_missing = true;
        let value = vec![42; 100_000];
        {
            let mut db = DB::open(&directory.0, options.clone()).unwrap();
            let mut batch = WriteBatch::default();
            batch.put(b"large", &value);
            db.write(batch, true).unwrap();
            db.close().unwrap();
        }
        let mut db = DB::open(&directory.0, options).unwrap();
        assert_eq!(db.get(b"large").unwrap(), value);
        db.close().unwrap();
    }

    #[test]
    fn reopen_after_incomplete_batch_never_appends_to_its_log() {
        let directory = TempDir::new("wal-incomplete-reopen");
        let mut options = crate::leveldb::bedrock_options();
        options.create_if_missing = true;
        {
            let mut db = DB::open(&directory.0, options.clone()).unwrap();
            let mut batch = WriteBatch::default();
            batch.put(b"before", b"saved");
            db.write(batch, true).unwrap();
            db.close().unwrap();
        }
        let log_path = fs::read_dir(&directory.0)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| is_write_ahead_log(path))
            .unwrap();
        OpenOptions::new()
            .append(true)
            .open(&log_path)
            .unwrap()
            .write_all(&fragment(2, b"interrupted batch"))
            .unwrap();
        {
            let mut db = DB::open(&directory.0, options.clone()).unwrap();
            assert_eq!(db.get(b"before").unwrap(), b"saved");
            let mut batch = WriteBatch::default();
            batch.put(b"after", b"also saved");
            db.write(batch, true).unwrap();
            db.close().unwrap();
        }
        let mut db = DB::open(&directory.0, options).unwrap();
        assert_eq!(db.get(b"before").unwrap(), b"saved");
        assert_eq!(db.get(b"after").unwrap(), b"also saved");
        db.close().unwrap();
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("sc-leveldb-env-{name}"));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp database directory");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Counts `Write::flush` calls per file name so a test can prove that a
    /// synced batch really reaches the durability path.
    struct CountingEnv {
        inner: DurableDiskEnv,
        log_flushes: Arc<AtomicUsize>,
        table_flushes: Arc<AtomicUsize>,
    }

    struct CountingWriter {
        inner: Box<dyn Write>,
        log_flushes: Option<Arc<AtomicUsize>>,
        table_flushes: Option<Arc<AtomicUsize>>,
    }

    impl Write for CountingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.inner.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            if let Some(counter) = &self.log_flushes {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            if let Some(counter) = &self.table_flushes {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            self.inner.flush()
        }
    }

    impl Env for CountingEnv {
        fn open_sequential_file(&self, path: &Path) -> Result<Box<dyn Read>> {
            self.inner.open_sequential_file(path)
        }
        fn open_random_access_file(&self, path: &Path) -> Result<Box<dyn RandomAccess>> {
            self.inner.open_random_access_file(path)
        }
        fn open_writable_file(&self, path: &Path) -> Result<Box<dyn Write>> {
            let writer = self.inner.open_writable_file(path)?;
            let log = is_write_ahead_log(path);
            Ok(Box::new(CountingWriter {
                inner: writer,
                log_flushes: log.then(|| Arc::clone(&self.log_flushes)),
                table_flushes: (!log).then(|| Arc::clone(&self.table_flushes)),
            }))
        }
        fn open_appendable_file(&self, path: &Path) -> Result<Box<dyn Write>> {
            let writer = self.inner.open_appendable_file(path)?;
            let log = is_write_ahead_log(path);
            Ok(Box::new(CountingWriter {
                inner: writer,
                log_flushes: log.then(|| Arc::clone(&self.log_flushes)),
                table_flushes: (!log).then(|| Arc::clone(&self.table_flushes)),
            }))
        }
        fn exists(&self, path: &Path) -> Result<bool> {
            self.inner.exists(path)
        }
        fn children(&self, path: &Path) -> Result<Vec<PathBuf>> {
            self.inner.children(path)
        }
        fn size_of(&self, path: &Path) -> Result<usize> {
            self.inner.size_of(path)
        }
        fn delete(&self, path: &Path) -> Result<()> {
            self.inner.delete(path)
        }
        fn mkdir(&self, path: &Path) -> Result<()> {
            self.inner.mkdir(path)
        }
        fn rmdir(&self, path: &Path) -> Result<()> {
            self.inner.rmdir(path)
        }
        fn rename(&self, old: &Path, new: &Path) -> Result<()> {
            self.inner.rename(old, new)
        }
        fn lock(&self, path: &Path) -> Result<FileLock> {
            self.inner.lock(path)
        }
        fn unlock(&self, lock: FileLock) -> Result<()> {
            self.inner.unlock(lock)
        }
        fn new_logger(&self, path: &Path) -> Result<Logger> {
            self.inner.new_logger(path)
        }
        fn micros(&self) -> u64 {
            self.inner.micros()
        }
        fn sleep_for(&self, micros: u32) {
            self.inner.sleep_for(micros);
        }
    }

    fn test_options(
        directory: &TempDir,
        log_flushes: Arc<AtomicUsize>,
        table_flushes: Arc<AtomicUsize>,
    ) -> Options {
        let mut options = crate::leveldb::bedrock_options();
        options.create_if_missing = true;
        options.env = Rc::new(Box::new(CountingEnv {
            inner: DurableDiskEnv::new(),
            log_flushes,
            table_flushes,
        }));
        let _ = directory;
        options
    }

    #[test]
    fn synced_batches_reach_the_durable_wal_path_and_unsynced_ones_do_not() {
        let directory = TempDir::new("synced-batch");
        let log_flushes = Arc::new(AtomicUsize::new(0));
        let table_flushes = Arc::new(AtomicUsize::new(0));
        let options = test_options(
            &directory,
            Arc::clone(&log_flushes),
            Arc::clone(&table_flushes),
        );
        let mut db = DB::open(&directory.0, options).expect("open database");

        let baseline = log_flushes.load(Ordering::SeqCst);
        let mut batch = WriteBatch::default();
        batch.put(b"unsynced", b"value");
        db.write(batch, false).expect("unsynced write");
        assert_eq!(
            log_flushes.load(Ordering::SeqCst),
            baseline,
            "an unsynced batch must not pay for durability"
        );

        let mut batch = WriteBatch::default();
        batch.put(b"synced", b"value");
        db.write(batch, true).expect("synced write");
        assert!(
            log_flushes.load(Ordering::SeqCst) > baseline,
            "a synced batch must flush the write-ahead log through the durable writer"
        );
        assert!(
            table_flushes.load(Ordering::SeqCst) > 0,
            "manifest/sstable writes still use the buffered path"
        );
        assert_eq!(
            db.get(b"synced").expect("read back"),
            b"value".to_vec(),
            "the acknowledged write is readable immediately"
        );
        db.close().expect("close database");
    }

    #[test]
    fn durable_log_writer_flushes_bytes_to_the_file() {
        let directory = TempDir::new("durable-writer");
        let path = directory.0.join("000001.log");
        {
            let mut writer = DurableLogWriter::new(&path, false).expect("open wal");
            writer.write_all(b"record").expect("write record");
            writer.flush().expect("flush to stable storage");
        }
        let bytes = fs::read(&path).expect("read wal back");
        assert_eq!(bytes, b"record");
        assert!(is_write_ahead_log(&path));
        assert!(!is_write_ahead_log(&directory.0.join("000001.sst")));
    }

    #[test]
    fn durable_log_writer_reports_open_failures() {
        let missing = PathBuf::from("/proc/sc-not-a-real-dir/000001.log");
        if missing.parent().is_some_and(|parent| parent.exists()) {
            return;
        }
        assert!(DurableLogWriter::new(&missing, false).is_err());
    }

    #[test]
    fn fsync_dir_accepts_a_real_directory_and_reports_a_missing_one() {
        let directory = TempDir::new("fsync-dir");
        fsync_dir(&directory.0).expect("fsync existing directory");
        assert!(fsync_dir(&directory.0.join("nested-missing")).is_err());
    }
}
