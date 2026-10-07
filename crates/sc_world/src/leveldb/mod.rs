//! Bedrock LevelDB world storage backend.
//!
//! Bedrock worlds store chunks in a LevelDB database (`<world>/db`) whose
//! blocks are compressed with zlib (compressor id 2) or raw deflate (id 4)
//! instead of snappy. `rusty_leveldb::DB` is not `Send` (it holds `Rc`s), so
//! a dedicated thread owns the database and serves chunk loads over a
//! channel, keeping [`LevelDbWorldStorage`] itself `Send + Sync` as the
//! [`WorldStorage`] trait requires.

pub mod block_hash;
mod env;
pub mod format;
mod journal;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

use log::warn;
use rusty_leveldb::{Compressor, CompressorList, Options, Status, WriteBatch, DB};

use crate::chunk::{dimension_bounds, Chunk, SubChunk, SubChunkIndex, SUBCHUNK_SIZE};
use crate::storage::{
    ChunkKey, GeneratedChunkCommit, SpilloverJournalEntry, SpilloverOperationId, WorldStorage,
    WorldStorageError,
};

use format::{
    chunk_record_key, encode_data_3d, encode_subchunk_persistent, parse_block_entities,
    parse_data_2d, parse_data_3d, parse_subchunk, TAG_BLOCK_ENTITY, TAG_CHUNK_VERSION, TAG_DATA_2D,
    TAG_DATA_3D, TAG_LEGACY_CHUNK_VERSION, TAG_SUBCHUNK_PREFIX,
};
use journal::SpilloverJournal;
use sc_binary::ByteWriter;
use sc_log::t_log;
use sc_nbt::local::BedrockLocalNbt;
use sc_nbt::writer::NbtWriter;

/// Bounded wait for one database-owner request.
///
/// The owner thread performs the actual (possibly blocking) storage IO. A
/// permanently hung disk would otherwise make every caller — including the
/// shutdown fallback — wait forever with no terminal state. Callers treat a
/// timeout as an explicit failure: the write stays dirty and the load is retried.
pub const DB_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Wait for one owner reply, mapping a timeout to an explicit backend error.
///
/// The reply channel carries the request's own `Result`, so this only has to
/// distinguish "the owner answered" from "the owner never answered".
fn await_reply<T>(
    receiver: &mpsc::Receiver<Result<T, WorldStorageError>>,
    what: &str,
) -> Result<T, WorldStorageError> {
    receiver.recv_timeout(DB_REQUEST_TIMEOUT).map_err(|error| {
        WorldStorageError::Backend(format!(
            "leveldb {what} did not complete within {}s: {error}",
            DB_REQUEST_TIMEOUT.as_secs()
        ))
    })?
}

/// Bedrock compressor id: zlib stream with header.
const COMPRESSOR_ZLIB: u8 = 2;
/// Bedrock compressor id: raw deflate stream (default since 1.16.100).
const COMPRESSOR_RAW_ZLIB: u8 = 4;

struct ZlibCompressor;

impl Compressor for ZlibCompressor {
    fn encode(&self, block: Vec<u8>) -> rusty_leveldb::Result<Vec<u8>> {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&block).map_err(Status::from)?;
        encoder.finish().map_err(Status::from)
    }

    fn decode(&self, block: Vec<u8>) -> rusty_leveldb::Result<Vec<u8>> {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(block.as_slice())
            .read_to_end(&mut out)
            .map_err(Status::from)?;
        Ok(out)
    }
}

struct RawZlibCompressor;

impl Compressor for RawZlibCompressor {
    fn encode(&self, block: Vec<u8>) -> rusty_leveldb::Result<Vec<u8>> {
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&block).map_err(Status::from)?;
        encoder.finish().map_err(Status::from)
    }

    fn decode(&self, block: Vec<u8>) -> rusty_leveldb::Result<Vec<u8>> {
        let mut out = Vec::new();
        flate2::read::DeflateDecoder::new(block.as_slice())
            .read_to_end(&mut out)
            .map_err(Status::from)?;
        Ok(out)
    }
}

fn bedrock_options() -> Options {
    let mut compressors = CompressorList::new();
    compressors.set_with_id(0, rusty_leveldb::compressor::NoneCompressor);
    compressors.set_with_id(COMPRESSOR_ZLIB, ZlibCompressor);
    compressors.set_with_id(COMPRESSOR_RAW_ZLIB, RawZlibCompressor);

    let mut options = Options::default();
    options.compressor = COMPRESSOR_RAW_ZLIB;
    options.compressor_list = Rc::new(compressors);
    options.create_if_missing = false;
    // Never append after an incomplete WAL tail left by an interrupted write.
    options.reuse_logs = false;
    // `sync = true` only reaches stable storage through this environment: the
    // upstream WAL writer's `flush()` never issues `sync_data`.
    options.env = env::DurableDiskEnv::shared();
    options
}

enum Request {
    LoadChunk {
        key: ChunkKey,
        reply: mpsc::SyncSender<Result<Option<Chunk>, WorldStorageError>>,
    },
    LoadSpilloverJournal {
        key: ChunkKey,
        reply: mpsc::SyncSender<Result<Vec<SpilloverJournalEntry>, WorldStorageError>>,
    },
    LoadChunkWithSpillover {
        key: ChunkKey,
        reply: mpsc::SyncSender<
            Result<(Option<Chunk>, Vec<SpilloverJournalEntry>), WorldStorageError>,
        >,
    },
    CommitGeneration {
        key: ChunkKey,
        chunk: Chunk,
        outgoing: Vec<SpilloverJournalEntry>,
        reply: mpsc::SyncSender<Result<GeneratedChunkCommit, WorldStorageError>>,
    },
    AppendSpilloverJournal {
        entries: Vec<SpilloverJournalEntry>,
        reply: mpsc::SyncSender<Result<(), WorldStorageError>>,
    },
    WriteChunk {
        key: ChunkKey,
        chunk: Chunk,
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
        spillover_acks: Vec<SpilloverOperationId>,
        reply: mpsc::SyncSender<Result<(), WorldStorageError>>,
    },
    Shutdown,
}

pub struct LevelDbWorldStorage {
    sender: Mutex<mpsc::SyncSender<Request>>,
    thread: Option<JoinHandle<()>>,
}

impl LevelDbWorldStorage {
    /// Opens an **existing** `<world>/db`. Fails fast (before returning) when the
    /// database cannot be opened or its journal cannot be safely replayed.
    pub fn open(db_path: PathBuf) -> Result<Self, WorldStorageError> {
        Self::open_with(db_path, false)
    }

    /// Opens `<world>/db`, creating a fresh database when the directory carries
    /// no LevelDB state yet.
    ///
    /// This is only for a directory the caller has established is *empty of
    /// database files* (see `directory_has_database_state`). An existing
    /// database is still opened read-strict, so a typo in a world path never
    /// silently replaces real save data.
    pub fn open_or_create(db_path: PathBuf) -> Result<Self, WorldStorageError> {
        Self::open_with(db_path, true)
    }

    fn open_with(db_path: PathBuf, create_if_missing: bool) -> Result<Self, WorldStorageError> {
        const REQUEST_QUEUE_CAPACITY: usize = 128;
        let (request_sender, request_receiver) =
            mpsc::sync_channel::<Request>(REQUEST_QUEUE_CAPACITY);
        let (startup_sender, startup_receiver) =
            mpsc::sync_channel::<Result<(), WorldStorageError>>(1);

        let thread = std::thread::Builder::new()
            .name("sc-world-leveldb".to_string())
            .spawn(move || {
                let mut options = bedrock_options();
                options.create_if_missing = create_if_missing;
                let mut db = match DB::open(&db_path, options) {
                    Ok(db) => db,
                    Err(status) => {
                        let _ = startup_sender
                            .send(Err(WorldStorageError::Backend(status.to_string())));
                        return;
                    }
                };
                let mut journal = match SpilloverJournal::open(&mut db) {
                    Ok(journal) => journal,
                    Err(error) => {
                        let _ = startup_sender.send(Err(error));
                        let _ = db.close();
                        return;
                    }
                };
                let _ = startup_sender.send(Ok(()));
                while let Ok(request) = request_receiver.recv() {
                    match request {
                        Request::LoadChunk { key, reply } => {
                            let _ = reply.send(load_chunk_blocking(&mut db, key));
                        }
                        Request::LoadSpilloverJournal { key, reply } => {
                            let _ = reply.send(journal::read_target(&mut db, key));
                        }
                        Request::LoadChunkWithSpillover { key, reply } => {
                            let result = load_chunk_blocking(&mut db, key).and_then(|chunk| {
                                journal::read_target(&mut db, key).map(|entries| (chunk, entries))
                            });
                            let _ = reply.send(result);
                        }
                        Request::CommitGeneration {
                            key,
                            chunk,
                            outgoing,
                            reply,
                        } => {
                            let _ = reply.send(commit_generation_blocking(
                                &mut db,
                                &mut journal,
                                key,
                                chunk,
                                &outgoing,
                            ));
                        }
                        Request::AppendSpilloverJournal { entries, reply } => {
                            let _ = reply.send(append_spillover_journal_blocking(
                                &mut db,
                                &mut journal,
                                &entries,
                            ));
                        }
                        Request::WriteChunk {
                            key,
                            chunk,
                            block_entities_dirty,
                            biomes_dirty,
                            heightmap_dirty,
                            spillover_acks,
                            reply,
                        } => {
                            let _ = reply.send(write_chunk_blocking(
                                &mut db,
                                &mut journal,
                                key,
                                &chunk,
                                block_entities_dirty,
                                biomes_dirty,
                                heightmap_dirty,
                                &spillover_acks,
                                &[],
                                false,
                            ));
                        }
                        Request::Shutdown => break,
                    }
                }
                if let Err(status) = db.close() {
                    warn!("{}", t_log!("console.leveldb.close_fail", status = status));
                }
                // Directory entries created/renamed by close (CURRENT, new
                // tables) are only durable once the parent directory syncs.
                env::fsync_dir_best_effort(&db_path);
            })
            .map_err(WorldStorageError::Io)?;

        match startup_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                sender: Mutex::new(request_sender),
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(WorldStorageError::Backend(
                    "leveldb thread died during open".into(),
                ))
            }
        }
    }

    fn request(&self, key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError> {
        let (reply_sender, reply_receiver) = mpsc::sync_channel(1);
        {
            let sender = self
                .sender
                .lock()
                .map_err(|_| WorldStorageError::Backend("leveldb sender poisoned".to_string()))?;
            sender
                .send(Request::LoadChunk {
                    key,
                    reply: reply_sender,
                })
                .map_err(|_| WorldStorageError::Backend("leveldb thread is gone".to_string()))?;
        }
        await_reply(&reply_receiver, "request")
    }

    fn request_spillover_journal(
        &self,
        key: ChunkKey,
    ) -> Result<Vec<SpilloverJournalEntry>, WorldStorageError> {
        let (reply_sender, reply_receiver) = mpsc::sync_channel(1);
        {
            let sender = self
                .sender
                .lock()
                .map_err(|_| WorldStorageError::Backend("leveldb sender poisoned".to_string()))?;
            sender
                .send(Request::LoadSpilloverJournal {
                    key,
                    reply: reply_sender,
                })
                .map_err(|_| WorldStorageError::Backend("leveldb thread is gone".to_string()))?;
        }
        await_reply(&reply_receiver, "request")
    }

    fn append_spillover_journal(
        &self,
        entries: &[SpilloverJournalEntry],
    ) -> Result<(), WorldStorageError> {
        if entries.is_empty() {
            return Ok(());
        }
        let (reply_sender, reply_receiver) = mpsc::sync_channel(1);
        {
            let sender = self
                .sender
                .lock()
                .map_err(|_| WorldStorageError::Backend("leveldb sender poisoned".to_string()))?;
            sender
                .send(Request::AppendSpilloverJournal {
                    entries: entries.to_vec(),
                    reply: reply_sender,
                })
                .map_err(|_| WorldStorageError::Backend("leveldb thread is gone".to_string()))?;
        }
        await_reply(&reply_receiver, "request")
    }

    /// Writes a chunk back (runs on the dedicated thread channel and waits for the result).
    fn save(&self, key: ChunkKey, chunk: &Chunk) -> Result<(), WorldStorageError> {
        self.save_owned_with_metadata(key, chunk.clone(), true, !chunk.biomes.is_empty(), true)
    }

    /// Transfer an owned writeback snapshot to the DB owner without making a
    /// second full Chunk clone on the writeback worker.
    fn save_owned(&self, key: ChunkKey, chunk: Chunk) -> Result<(), WorldStorageError> {
        let biomes_dirty = !chunk.biomes.is_empty();
        self.save_owned_with_metadata(key, chunk, true, biomes_dirty, true)
    }

    fn save_owned_with_metadata(
        &self,
        key: ChunkKey,
        chunk: Chunk,
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
    ) -> Result<(), WorldStorageError> {
        self.save_owned_with_metadata_and_spillover(
            key,
            chunk,
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            &[],
        )
    }

    fn save_owned_with_metadata_and_spillover(
        &self,
        key: ChunkKey,
        chunk: Chunk,
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
        spillover_acks: &[SpilloverOperationId],
    ) -> Result<(), WorldStorageError> {
        let (reply_sender, reply_receiver) = mpsc::sync_channel(1);
        {
            let sender = self
                .sender
                .lock()
                .map_err(|_| WorldStorageError::Backend("leveldb sender poisoned".to_string()))?;
            sender
                .send(Request::WriteChunk {
                    key,
                    chunk,
                    block_entities_dirty,
                    biomes_dirty,
                    heightmap_dirty,
                    spillover_acks: spillover_acks.to_vec(),
                    reply: reply_sender,
                })
                .map_err(|_| WorldStorageError::Backend("leveldb thread is gone".to_string()))?;
        }
        await_reply(&reply_receiver, "request")
    }
}

impl WorldStorage for LevelDbWorldStorage {
    fn load_chunk(&self, key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError> {
        self.request(key)
    }

    fn load_chunk_with_spillover(
        &self,
        key: ChunkKey,
    ) -> Result<(Option<Chunk>, Vec<SpilloverJournalEntry>), WorldStorageError> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .lock()
            .map_err(|_| WorldStorageError::Backend("leveldb sender poisoned".into()))?
            .send(Request::LoadChunkWithSpillover { key, reply })
            .map_err(|_| WorldStorageError::Backend("leveldb thread is gone".into()))?;
        await_reply(&receiver, "request")
    }

    fn commit_generated_chunk(
        &self,
        key: ChunkKey,
        chunk: Chunk,
        outgoing: &[SpilloverJournalEntry],
    ) -> Result<GeneratedChunkCommit, WorldStorageError> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .lock()
            .map_err(|_| WorldStorageError::Backend("leveldb sender poisoned".into()))?
            .send(Request::CommitGeneration {
                key,
                chunk,
                outgoing: outgoing.to_vec(),
                reply,
            })
            .map_err(|_| WorldStorageError::Backend("leveldb thread is gone".into()))?;
        await_reply(&receiver, "request")
    }

    fn supports_spillover_journal(&self) -> bool {
        true
    }

    fn load_spillover_journal(
        &self,
        key: ChunkKey,
    ) -> Result<Vec<SpilloverJournalEntry>, WorldStorageError> {
        self.request_spillover_journal(key)
    }

    fn append_spillover_journal(
        &self,
        entries: &[SpilloverJournalEntry],
    ) -> Result<(), WorldStorageError> {
        self.append_spillover_journal(entries)
    }

    fn save_chunk(&self, key: ChunkKey, chunk: &Chunk) -> Result<(), WorldStorageError> {
        self.save(key, chunk)
    }

    fn save_chunk_owned(&self, key: ChunkKey, chunk: Chunk) -> Result<(), WorldStorageError> {
        self.save_owned(key, chunk)
    }

    fn save_chunk_owned_with_metadata(
        &self,
        key: ChunkKey,
        chunk: Chunk,
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
    ) -> Result<(), WorldStorageError> {
        self.save_owned_with_metadata(
            key,
            chunk,
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
        )
    }

    fn save_chunk_owned_with_metadata_and_spillover(
        &self,
        key: ChunkKey,
        chunk: Chunk,
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
        spillover_acks: &[SpilloverOperationId],
    ) -> Result<(), WorldStorageError> {
        self.save_owned_with_metadata_and_spillover(
            key,
            chunk,
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            spillover_acks,
        )
    }
}

impl Drop for LevelDbWorldStorage {
    fn drop(&mut self) {
        if let Ok(sender) = self.sender.lock() {
            let _ = sender.send(Request::Shutdown);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Runs on the database thread. A chunk exists when either chunk-version key
/// is present; individual records degrade independently so one bad or
/// unsupported record does not take the whole chunk down with it.
fn load_chunk_blocking(db: &mut DB, key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError> {
    let version_present = db
        .get(&chunk_record_key(key, TAG_CHUNK_VERSION, None))
        .or_else(|| db.get(&chunk_record_key(key, TAG_LEGACY_CHUNK_VERSION, None)))
        .is_some();
    if !version_present {
        return Ok(None);
    }

    let (min_y, max_y) = dimension_bounds(key.dimension);
    let first_section = min_y.div_euclid(SUBCHUNK_SIZE) as i8;
    let section_count = ((max_y - min_y + 1) / SUBCHUNK_SIZE) as usize;

    let mut subchunks = Vec::with_capacity(section_count);
    for offset in 0..section_count {
        let y = first_section + offset as i8;
        let record = db.get(&chunk_record_key(key, TAG_SUBCHUNK_PREFIX, Some(y)));
        let subchunk = match record {
            Some(bytes) => match parse_subchunk(&bytes, y) {
                Ok(subchunk) => subchunk,
                Err(WorldStorageError::Unsupported(message)) => {
                    warn!(
                        "{}",
                        t_log!(
                            "console.leveldb.subchunk",
                            pos = key.position,
                            y = y,
                            message = message
                        )
                    );
                    SubChunk::empty_network(SubChunkIndex::new(y))
                }
                Err(WorldStorageError::Corrupt(message)) => {
                    return Err(WorldStorageError::Corrupt(format!(
                        "chunk {} subchunk {y}: {message}",
                        key.position
                    )));
                }
                Err(error) => return Err(error),
            },
            None => SubChunk::empty_network(SubChunkIndex::new(y)),
        };
        subchunks.push(subchunk);
    }

    let data_3d_record = db.get(&chunk_record_key(key, TAG_DATA_3D, None));
    let data_2d_record = if data_3d_record.is_none() {
        db.get(&chunk_record_key(key, TAG_DATA_2D, None))
    } else {
        None
    };
    let data_3d_heightmap = data_3d_record
        .as_deref()
        .and_then(data_3d_heightmap_prefix)
        .or_else(|| data_2d_record.as_deref().and_then(data_3d_heightmap_prefix));
    let biomes = match data_3d_record {
        Some(bytes) => match parse_data_3d(&bytes, section_count) {
            Ok(sections) => sections,
            Err(error) => {
                warn!(
                    "{}",
                    t_log!("console.leveldb.data3d", pos = key.position, error = error)
                );
                Vec::new()
            }
        },
        None => match data_2d_record {
            Some(bytes) => match parse_data_2d(&bytes, section_count) {
                Ok(sections) => sections,
                Err(error) => {
                    warn!(
                        "{}",
                        t_log!("console.leveldb.data2d", pos = key.position, error = error)
                    );
                    Vec::new()
                }
            },
            None => Vec::new(),
        },
    };

    let block_entities = match db.get(&chunk_record_key(key, TAG_BLOCK_ENTITY, None)) {
        Some(bytes) => match parse_block_entities(&bytes) {
            Ok(entities) => entities,
            Err(error) => {
                warn!(
                    "{}",
                    t_log!(
                        "console.leveldb.block_entities",
                        pos = key.position,
                        error = error
                    )
                );
                Vec::new()
            }
        },
        None => Vec::new(),
    };

    Ok(Some(Chunk {
        position: key.position,
        dimension: key.dimension,
        min_y,
        max_y,
        subchunks,
        biomes,
        block_entities,
        data_3d_heightmap,
    }))
}

fn data_3d_heightmap_prefix(bytes: &[u8]) -> Option<Arc<[u8; 512]>> {
    let prefix = bytes.get(..512)?;
    let mut heightmap = [0u8; 512];
    heightmap.copy_from_slice(prefix);
    Some(Arc::new(heightmap))
}

fn append_spillover_journal_blocking(
    db: &mut DB,
    journal: &mut SpilloverJournal,
    entries: &[SpilloverJournalEntry],
) -> Result<(), WorldStorageError> {
    let mut batch = WriteBatch::default();
    let additions = journal.prepare_append(db, &mut batch, entries)?;
    if !additions.is_empty() {
        db.write(batch, true).map_err(|status| {
            WorldStorageError::Backend(format!("spillover journal write: {status}"))
        })?;
        journal.committed(
            additions,
            ChunkKey::new(0, crate::chunk::ChunkPosition::new(0, 0)),
            0,
        );
    }
    Ok(())
}

fn commit_generation_blocking(
    db: &mut DB,
    journal: &mut SpilloverJournal,
    key: ChunkKey,
    chunk: Chunk,
    outgoing: &[SpilloverJournalEntry],
) -> Result<GeneratedChunkCommit, WorldStorageError> {
    let snapshot = db.get_snapshot();
    let exists = db
        .get_at(&snapshot, &chunk_record_key(key, TAG_CHUNK_VERSION, None))
        .map_err(|status| WorldStorageError::Backend(status.to_string()))?
        .or(db
            .get_at(
                &snapshot,
                &chunk_record_key(key, TAG_LEGACY_CHUNK_VERSION, None),
            )
            .map_err(|status| WorldStorageError::Backend(status.to_string()))?)
        .is_some();
    let source = if exists {
        load_chunk_blocking(db, key)?.ok_or_else(|| {
            WorldStorageError::Corrupt("committed generation source is missing".into())
        })?
    } else {
        if outgoing
            .iter()
            .any(|entry| entry.operation_id.source != key)
        {
            return Err(WorldStorageError::Corrupt(
                "spillover source identity mismatch".into(),
            ));
        }
        write_chunk_blocking(
            db,
            journal,
            key,
            &chunk,
            !chunk.block_entities.is_empty(),
            !chunk.biomes.is_empty(),
            !chunk.biomes.is_empty() && chunk.data_3d_heightmap.is_none(),
            &[],
            outgoing,
            true,
        )?;
        chunk
    };
    let incoming = journal::read_target(db, key)?;
    Ok(GeneratedChunkCommit {
        source,
        incoming,
        newly_committed: !exists,
    })
}

/// Runs on the database thread. Writes the version key + subchunks (v8
/// persistent encoding) + block entities (single key, back-to-back LE NBT).
/// Dirty biomes are encoded as Data3D; block edits refresh only the 512-byte
/// heightmap prefix and retain the existing biome payload bytes.
fn write_chunk_blocking(
    db: &mut DB,
    journal: &mut SpilloverJournal,
    key: ChunkKey,
    chunk: &Chunk,
    block_entities_dirty: bool,
    biomes_dirty: bool,
    heightmap_dirty: bool,
    spillover_acks: &[SpilloverOperationId],
    outgoing: &[SpilloverJournalEntry],
    sync: bool,
) -> Result<(), WorldStorageError> {
    let mut batch = WriteBatch::default();

    // 1) version key (read side only checks existence; value writes the modern 1.18.30 value 26).
    batch.put(&chunk_record_key(key, TAG_CHUNK_VERSION, None), &[26]);

    // 2) Subchunks (v8 persistent encoding).
    for subchunk in &chunk.subchunks {
        let value = encode_subchunk_persistent(subchunk)?;
        let record_key = chunk_record_key(key, TAG_SUBCHUNK_PREFIX, Some(subchunk.index.y));
        batch.put(&record_key, &value);
    }

    // 3) Block entities (single key, back-to-back LE NBT, mirrors parse_block_entities).
    // A block entity record is only authoritative when its metadata was
    // explicitly dirtied. This preserves records that a lenient load could not
    // decode; an explicit empty dirty value deletes the stale Bedrock key.
    if block_entities_dirty {
        let record_key = chunk_record_key(key, TAG_BLOCK_ENTITY, None);
        if chunk.block_entities.is_empty() {
            batch.delete(&record_key);
        } else {
            let mut writer = ByteWriter::new();
            for entity in &chunk.block_entities {
                NbtWriter::from_writer(&mut writer)
                    .write::<BedrockLocalNbt>(entity)
                    .map_err(|error| {
                        WorldStorageError::Corrupt(format!("block entity NBT encode: {error}"))
                    })?;
            }
            let bytes = writer.as_slice().to_vec();
            batch.put(&record_key, &bytes);
        }
    }

    if biomes_dirty {
        let (min_y, max_y) = dimension_bounds(key.dimension);
        let section_count = ((max_y - min_y + 1) / SUBCHUNK_SIZE) as usize;
        let heightmap = if heightmap_dirty {
            heightmap_prefix_from_chunk(chunk)
        } else {
            heightmap_prefix_for_write(db, key, chunk)?
        };
        let data_3d = encode_data_3d(&heightmap, &chunk.biomes, section_count)?;
        batch.put(&chunk_record_key(key, TAG_DATA_3D, None), &data_3d);
    } else if heightmap_dirty {
        let heightmap = heightmap_prefix_from_chunk(chunk);
        // Preserve the original biome payload bytes verbatim; block edits only
        // replace the shared heightmap prefix in the existing 3D/legacy 2D key.
        if !replace_heightmap_prefix(db, &mut batch, key, TAG_DATA_3D, &heightmap)? {
            replace_heightmap_prefix(db, &mut batch, key, TAG_DATA_2D, &heightmap)?;
        }
    }

    let additions = journal.prepare_append(db, &mut batch, outgoing)?;
    let removed = journal.prepare_ack(db, &mut batch, key, spillover_acks)?;

    db.write(batch, sync || !spillover_acks.is_empty())
        .map_err(|status| {
            WorldStorageError::Backend(format!("leveldb write chunk batch: {status}"))
        })?;
    journal.committed(additions, key, removed);
    Ok(())
}

fn heightmap_prefix_from_chunk(chunk: &Chunk) -> [u8; 512] {
    let air = crate::chunk::BlockRuntimeId(crate::block_dictionary::air_runtime_id());
    let mut bytes = [0u8; 512];
    for x in 0..16u8 {
        for z in 0..16u8 {
            let height = chunk.highest_block_at(x, z, air).unwrap_or(0) as i16;
            let offset = (x as usize * 16 + z as usize) * 2;
            bytes[offset..offset + 2].copy_from_slice(&height.to_le_bytes());
        }
    }
    bytes
}

fn replace_heightmap_prefix(
    db: &mut DB,
    batch: &mut WriteBatch,
    key: ChunkKey,
    tag: u8,
    heightmap: &[u8; 512],
) -> Result<bool, WorldStorageError> {
    let record_key = chunk_record_key(key, tag, None);
    let Some(mut record) = db.get(&record_key) else {
        return Ok(false);
    };
    if record.len() < 512 {
        return Err(WorldStorageError::Corrupt(format!(
            "biome record tag {tag:#x} is shorter than the 512-byte heightmap prefix"
        )));
    }
    record[..512].copy_from_slice(heightmap);
    batch.put(&record_key, &record);
    Ok(true)
}

fn heightmap_prefix_for_write(
    db: &mut DB,
    key: ChunkKey,
    chunk: &Chunk,
) -> Result<[u8; 512], WorldStorageError> {
    for tag in [TAG_DATA_3D, TAG_DATA_2D] {
        if let Some(record) = db.get(&chunk_record_key(key, tag, None)) {
            let Some(prefix) = record.get(..512) else {
                return Err(WorldStorageError::Corrupt(format!(
                    "biome record tag {tag:#x} is shorter than the 512-byte heightmap prefix"
                )));
            };
            let mut heightmap = [0u8; 512];
            heightmap.copy_from_slice(prefix);
            return Ok(heightmap);
        }
    }
    chunk
        .data_3d_heightmap
        .as_ref()
        .map(|heightmap| **heightmap)
        .ok_or_else(|| {
            WorldStorageError::Unsupported(
                "cannot persist dirty Data3D biomes without a heightmap snapshot".into(),
            )
        })
}

#[cfg(test)]
mod journal_tests {
    use super::*;
    use crate::chunk::{BlockRuntimeId, ChunkPosition};
    use crate::storage::BlockSpilloverWrite;

    fn entry(source: ChunkKey, target: ChunkKey, block: u32) -> SpilloverJournalEntry {
        SpilloverJournalEntry {
            operation_id: SpilloverOperationId::new(source, 0),
            write: BlockSpilloverWrite {
                key: target,
                x: target.position.x * 16,
                y: 0,
                z: target.position.z * 16,
                layer: 0,
                block: BlockRuntimeId(block),
            },
        }
    }

    #[test]
    fn cleaned_spillover_is_not_recreated_by_duplicate_generation() {
        let mut db = DB::open("generation-journal-replay", rusty_leveldb::in_memory()).unwrap();
        let mut journal = SpilloverJournal::open(&mut db).unwrap();
        let source = ChunkKey::new(0, ChunkPosition::new(0, 0));
        let target = ChunkKey::new(0, ChunkPosition::new(1, 0));
        let fact = entry(source, target, crate::block_dictionary::air_runtime_id());
        assert!(
            commit_generation_blocking(
                &mut db,
                &mut journal,
                source,
                Chunk::empty_overworld(source.position),
                &[fact]
            )
            .unwrap()
            .newly_committed
        );
        assert_eq!(journal::read_target(&mut db, target).unwrap(), vec![fact]);

        let stone = super::block_hash::block_state_hash("minecraft:stone", None);
        crate::block_dictionary::BlockStateDictionary::global().record_with(stone, || {
            crate::block_dictionary::BlockStateEntry {
                name: "minecraft:stone".into(),
                states: None,
            }
        });
        let mut edited = Chunk::empty_overworld(target.position);
        edited.set_block_at(0, 0, 0, 0, BlockRuntimeId(stone));
        write_chunk_blocking(
            &mut db,
            &mut journal,
            target,
            &edited,
            false,
            false,
            false,
            &[fact.operation_id],
            &[],
            false,
        )
        .unwrap();
        assert!(journal::read_target(&mut db, target).unwrap().is_empty());

        let duplicate = commit_generation_blocking(
            &mut db,
            &mut journal,
            source,
            Chunk::empty_overworld(source.position),
            &[fact],
        )
        .unwrap();
        assert!(!duplicate.newly_committed);
        assert!(journal::read_target(&mut db, target).unwrap().is_empty());
        assert_eq!(
            load_chunk_blocking(&mut db, target)
                .unwrap()
                .unwrap()
                .block_at(0, 0, 0),
            Some(BlockRuntimeId(stone))
        );
        SpilloverJournal::open(&mut db).unwrap();
    }

    #[test]
    fn conflicting_spillover_rejects_the_source_transaction() {
        let mut db = DB::open("generation-journal-rejection", rusty_leveldb::in_memory()).unwrap();
        let mut journal = SpilloverJournal::open(&mut db).unwrap();
        let source = ChunkKey::new(0, ChunkPosition::new(0, 0));
        let target = ChunkKey::new(0, ChunkPosition::new(1, 0));
        let first = entry(source, target, 1);
        let conflict = entry(source, target, 2);
        assert!(matches!(
            commit_generation_blocking(
                &mut db,
                &mut journal,
                source,
                Chunk::empty_overworld(source.position),
                &[first, conflict]
            ),
            Err(WorldStorageError::Corrupt(_))
        ));
        assert!(load_chunk_blocking(&mut db, source).unwrap().is_none());
        assert!(journal::read_target(&mut db, target).unwrap().is_empty());
        assert!(
            commit_generation_blocking(
                &mut db,
                &mut journal,
                source,
                Chunk::empty_overworld(source.position),
                &[first]
            )
            .unwrap()
            .newly_committed
        );
    }
}

#[cfg(test)]
mod writeback_tests {
    use super::*;
    use crate::chunk::ChunkPosition;
    use crate::storage::{ChunkKey, WorldStorage};
    use sc_nbt::compound::CompoundNbt;
    use sc_nbt::NbtValue;
    use std::fs;

    #[test]
    #[ignore = "requires SC_PROBE_WORLD pointing to a disposable world copy"]
    fn scan_world_subchunk_integrity() {
        use rusty_leveldb::LdbIterator;
        let path = PathBuf::from(std::env::var("SC_PROBE_WORLD").expect("world copy path"));
        let mut db = DB::open(path.join("db"), bedrock_options()).unwrap();
        let mut iter = db.new_iter().unwrap();
        let mut records = 0;
        let mut errors = 0;
        while let Some((key, bytes)) = iter.next() {
            let tag_offset = match key.len() {
                10 => 8,
                14 => 12,
                _ => continue,
            };
            if key[tag_offset] != TAG_SUBCHUNK_PREFIX {
                continue;
            }
            records += 1;
            if let Err(error) = parse_subchunk(&bytes, key[tag_offset + 1] as i8) {
                errors += 1;
                if errors <= 25 {
                    let x = i32::from_le_bytes(key[..4].try_into().unwrap());
                    let z = i32::from_le_bytes(key[4..8].try_into().unwrap());
                    println!(
                        "({x}, {z}) y={} bytes={}: {error}",
                        key[tag_offset + 1] as i8,
                        bytes.len()
                    );
                }
            }
        }
        println!("subchunk records={records}, decode errors={errors}");
        assert_eq!(errors, 0, "world contains invalid subchunks");
    }

    #[test]
    #[ignore = "requires SC_PROBE_WORLD pointing to a disposable world copy"]
    fn probe_palette_failure_column() {
        let path = PathBuf::from(std::env::var("SC_PROBE_WORLD").expect("world copy path"));
        let mut db = DB::open(path.join("db"), bedrock_options()).unwrap();
        let key = ChunkKey::new(0, ChunkPosition::new(89, 196));
        for y in -4..20 {
            if let Some(bytes) = db.get(&chunk_record_key(key, TAG_SUBCHUNK_PREFIX, Some(y))) {
                println!(
                    "y={y} bytes={} header={:02x?} result={:?}",
                    bytes.len(),
                    &bytes[..bytes.len().min(8)],
                    parse_subchunk(&bytes, y).map(|s| s.layers.len())
                );
            }
        }
        load_chunk_blocking(&mut db, key).expect("column must decode");
    }

    #[test]
    fn block_entity_record_is_preserved_unless_metadata_dirty_then_deleted_if_empty() {
        let db_path = std::env::temp_dir().join(format!(
            "sc-world-block-entity-writeback-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&db_path).expect("create temporary LevelDB directory");
        let mut options = bedrock_options();
        options.create_if_missing = true;
        DB::open(&db_path, options)
            .expect("initialize temporary LevelDB")
            .close()
            .expect("close initial DB handle");
        let key = ChunkKey::new(0, ChunkPosition::new(3, -7));
        let block_entity_key = chunk_record_key(key, TAG_BLOCK_ENTITY, None);

        let mut with_entity = Chunk::empty(key.position, key.dimension, -64, 319);
        let mut entity = CompoundNbt::new(None);
        entity.insert("id", NbtValue::String("minecraft:chest".into()));
        with_entity.block_entities.push(NbtValue::Compound(entity));
        let without_entity = Chunk::empty(key.position, key.dimension, -64, 319);
        let storage = LevelDbWorldStorage::open(db_path.clone()).expect("open DB owner");
        storage
            .save_chunk_owned_with_metadata(key, with_entity, true, false, false)
            .expect("write block entity metadata through DB owner");
        storage
            .save_chunk_owned_with_metadata(key, without_entity.clone(), false, false, false)
            .expect("save unrelated dirty subchunks through DB owner");
        drop(storage);

        let mut db = DB::open(&db_path, bedrock_options()).expect("reopen temporary LevelDB");
        let original_record = db
            .get(&block_entity_key)
            .expect("block entity record exists")
            .to_vec();
        assert_eq!(
            parse_block_entities(&original_record)
                .expect("existing block entity remains parseable")
                .len(),
            1,
            "clean metadata must preserve existing and possibly unknown NBT"
        );
        db.close().expect("close verification DB handle");

        let storage = LevelDbWorldStorage::open(db_path.clone()).expect("reopen DB owner");
        storage
            .save_chunk_owned_with_metadata(key, without_entity, true, false, false)
            .expect("persist explicit block entity removal through DB owner");
        drop(storage);

        let mut db = DB::open(&db_path, bedrock_options()).expect("verify deleted record");
        assert!(
            db.get(&block_entity_key).is_none(),
            "an authoritative empty block-entity snapshot deletes the old record"
        );

        db.close().expect("close temporary LevelDB");
        fs::remove_dir_all(db_path).expect("remove temporary LevelDB directory");
    }

    #[test]
    fn dirty_biomes_write_data3d_and_clean_biomes_preserve_existing_record() {
        let db_path =
            std::env::temp_dir().join(format!("sc-world-biome-writeback-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&db_path).expect("create temporary LevelDB directory");
        let mut options = bedrock_options();
        options.create_if_missing = true;
        DB::open(&db_path, options)
            .expect("initialize temporary LevelDB")
            .close()
            .expect("close initial DB handle");

        let key = ChunkKey::new(0, ChunkPosition::new(8, -5));
        let data3d_key = chunk_record_key(key, TAG_DATA_3D, None);
        let heightmap = Arc::new(std::array::from_fn(|index| (index as u8).wrapping_mul(11)));
        let section_count = 24;
        let first_biomes = vec![crate::chunk::PalettedBiomeStorage::Single(1); section_count];
        let mut initial = Chunk::empty(key.position, key.dimension, -64, 319);
        initial.data_3d_heightmap = Some(heightmap.clone());
        initial.biomes = first_biomes.clone();

        let storage = LevelDbWorldStorage::open(db_path.clone()).expect("open DB owner");
        storage
            .save_chunk_owned_with_metadata(key, initial, false, true, false)
            .expect("write dirty Data3D biome record");
        drop(storage);

        let mut db = DB::open(&db_path, bedrock_options()).expect("read Data3D record");
        let original_record = db.get(&data3d_key).expect("Data3D exists");
        assert_eq!(&original_record[..512], heightmap.as_slice());
        assert_eq!(
            parse_data_3d(&original_record, section_count).expect("parse saved biomes"),
            first_biomes
        );
        db.close().expect("close verification DB handle");

        let mut changed = Chunk::empty(key.position, key.dimension, -64, 319);
        changed.data_3d_heightmap = Some(Arc::new([0xFF; 512]));
        changed.biomes = vec![crate::chunk::PalettedBiomeStorage::Single(7); section_count];
        let stone_hash = crate::leveldb::block_hash::block_state_hash("minecraft:stone", None);
        crate::block_dictionary::BlockStateDictionary::global().record_with(stone_hash, || {
            crate::block_dictionary::BlockStateEntry {
                name: "minecraft:stone".into(),
                states: None,
            }
        });
        changed
            .set_block_at(0, 5, 83, 7, crate::chunk::BlockRuntimeId(stone_hash))
            .expect("place heightmap test block");
        let storage = LevelDbWorldStorage::open(db_path.clone()).expect("reopen DB owner");
        storage
            .save_chunk_owned_with_metadata(key, changed.clone(), false, false, true)
            .expect("heightmap update must preserve clean biome payload bytes");
        drop(storage);

        let mut db = DB::open(&db_path, bedrock_options()).expect("verify heightmap-only update");
        let heightmap_updated = db.get(&data3d_key).expect("Data3D remains present");
        let heightmap_offset = (5 * 16 + 7) * 2;
        assert_eq!(
            i16::from_le_bytes([
                heightmap_updated[heightmap_offset],
                heightmap_updated[heightmap_offset + 1],
            ]),
            83
        );
        assert_eq!(
            parse_data_3d(&heightmap_updated, section_count).expect("parse preserved biomes"),
            first_biomes,
            "heightmap update must preserve existing biome storage bytes"
        );
        db.close().expect("close heightmap verification DB");

        let storage = LevelDbWorldStorage::open(db_path.clone()).expect("reopen DB owner");
        storage
            .save_chunk_owned_with_metadata(key, changed, false, true, false)
            .expect("explicit biome update");
        drop(storage);

        let mut db = DB::open(&db_path, bedrock_options()).expect("verify updated Data3D");
        let updated_record = db.get(&data3d_key).expect("updated Data3D exists");
        assert_eq!(
            i16::from_le_bytes([
                updated_record[heightmap_offset],
                updated_record[heightmap_offset + 1],
            ]),
            83
        );
        let updated_sections =
            parse_data_3d(&updated_record, section_count).expect("parse updated biomes");
        assert!(updated_sections
            .iter()
            .all(|section| section.get(0) == Some(7)));
        db.close().expect("close temporary LevelDB");
        fs::remove_dir_all(db_path).expect("remove temporary LevelDB directory");
    }
}
