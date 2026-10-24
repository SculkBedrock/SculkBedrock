//! 区域所有权、worker 提交邮箱与本地发布。
//!
//! 每个区块在同一时刻只有一个权威所有者区域。worker 准备自有的结果（快照、
//! 碎片、组装好的候选），不直接修改权威区块存储。跨区域通信使用携带
//! `(RegionOwnerKey, OwnerEpoch, AttemptId)` 的有界值消息；所有者纪元变化后，
//! 携带旧纪元的提交会被拒绝。各区域并发处理各自的邮箱。
//!
//! Region ownership, worker submission mailbox, and local publication.
//!
//! Each chunk has exactly one authoritative owner region at a time. Workers
//! prepare owned results (snapshots, fragments, assembled candidates) and never
//! mutate authoritative chunk storage directly. Cross-Region communication uses
//! bounded value messages carrying `(RegionOwnerKey, OwnerEpoch, AttemptId)`;
//! submissions carrying a stale epoch are rejected after an owner epoch change.
//! Regions process their own mailboxes concurrently.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::generation::error::GenerationContractError;
use crate::generation::identity::{FragmentId, GenerationStage, OriginCell};
use crate::generation::record::{AttemptId, ChunkGenerationRecord, GenerationControlState, ReadyCertificate};
use crate::generation::view::BaseSnapshot;
use crate::storage::ChunkColumn;

/// 区域网格的默认大小（以区块计，2x2）。
///
/// Default size of a Region grid in chunks (2x2 chunks).
pub const DEFAULT_REGION_SIZE_CHUNKS: u32 = 2;

/// 权威区域所有者的身份。
///
/// Identity of an authoritative region owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RegionOwnerKey {
    pub dimension: i32,
    pub region_x: i32,
    pub region_z: i32,
}

impl RegionOwnerKey {
    /// 构造一个区域所有者键。
    ///
    /// Creates a region owner key.
    pub const fn new(dimension: i32, region_x: i32, region_z: i32) -> Self {
        Self { dimension, region_x, region_z }
    }

    /// 区块坐标在向下取整的欧几里得除法下所属的所有者区域。
    /// 负区块坐标正确映射到负区域索引。
    ///
    /// Owning region of chunk coordinates under floor Euclidean division.
    /// Negative chunk coordinates map to negative region indices correctly.
    #[inline]
    pub fn region_of(dimension: i32, chunk_x: i32, chunk_z: i32, region_size_chunks: u32) -> Self {
        let n = region_size_chunks.max(1) as i32;
        Self::new(
            dimension,
            chunk_x.div_euclid(n),
            chunk_z.div_euclid(n),
        )
    }

    /// 该区块是否属于本所有者。
    ///
    /// Returns whether the chunk belongs to this owner.
    #[inline]
    pub fn contains_chunk(&self, chunk_x: i32, chunk_z: i32, region_size_chunks: u32) -> bool {
        let expected = Self::region_of(self.dimension, chunk_x, chunk_z, region_size_chunks);
        expected.region_x == self.region_x && expected.region_z == self.region_z
    }

    /// 本区域网格边界内的所有区块。
    ///
    /// All chunks within this region's grid bounds.
    pub fn chunks(&self, region_size_chunks: u32) -> Vec<(i32, i32)> {
        let n = region_size_chunks.max(1) as i32;
        let start_x = self.region_x * n;
        let start_z = self.region_z * n;
        let mut list = Vec::with_capacity((n * n) as usize);
        for dx in 0..n {
            for dz in 0..n {
                list.push((start_x + dx, start_z + dz));
            }
        }
        list
    }
}

/// 单调的所有者纪元。在所有者移交、重载或隔离时递增。
///
/// Monotonic owner epoch. Advanced upon owner handoff, reload, or fencing.
pub type OwnerEpoch = u64;

/// 所有 worker 提交携带的身份与路由票据。
///
/// Identity and routing ticket carried by all worker submissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmissionTicket {
    pub owner: RegionOwnerKey,
    pub epoch: OwnerEpoch,
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub attempt: AttemptId,
}

impl SubmissionTicket {
    /// 构造一个提交票据。
    ///
    /// Creates a submission ticket.
    pub const fn new(
        owner: RegionOwnerKey,
        epoch: OwnerEpoch,
        chunk_x: i32,
        chunk_z: i32,
        attempt: AttemptId,
    ) -> Self {
        Self {
            owner,
            epoch,
            chunk_x,
            chunk_z,
            attempt,
        }
    }
}

/// 投影碎片中的一次自有方块写入。
///
/// Owned block write in a projected fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FragmentBlockWrite {
    /// 目标区块内的局部 X：0..16。
    ///
    /// Local X within target chunk: 0..16.
    pub local_x: u8,
    /// 世界 Y。
    ///
    /// World Y.
    pub y: i32,
    /// 目标区块内的局部 Z：0..16。
    ///
    /// Local Z within target chunk: 0..16.
    pub local_z: u8,
    /// 方块运行时 ID。
    ///
    /// Block runtime ID.
    pub runtime_id: u32,
    /// 方块层（0 = 普通，1 = 液体）。
    ///
    /// Block layer (0 = normal, 1 = liquid).
    pub layer: u8,
}

/// 供 worker 提交的已投影区块碎片。
///
/// Owned projected chunk fragment for worker submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkFragmentPayload {
    pub fragment_id: FragmentId,
    pub writes: Vec<FragmentBlockWrite>,
}

/// worker 在所有者发布前准备好的最终候选列。
///
/// Owned final candidate column prepared by worker before owner publish.
#[derive(Clone)]
pub struct FinalCandidatePayload {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub assembly_digest: u64,
    pub column: Arc<ChunkColumn>,
}

/// 通过区域邮箱投递的 worker 提交类型。
///
/// Types of worker submissions delivered through a region's mailbox.
#[derive(Clone)]
pub enum WorkerSubmission {
    /// worker 准备好了不可变的基础地形快照。
    ///
    /// Worker prepared an immutable base terrain snapshot.
    BasePrepared {
        ticket: SubmissionTicket,
        snapshot: Arc<BaseSnapshot>,
    },
    /// worker 生成了一批投影到目标区块的碎片。
    ///
    /// Worker generated a batch of projected fragments for a target chunk.
    FragmentsBatch {
        ticket: SubmissionTicket,
        fragments: Vec<ChunkFragmentPayload>,
        manifest_digest: u64,
    },
    /// 某个固定的源单元格完成了某阶段的全部特征产出。
    ///
    /// A fixed source cell finished producing all features for a stage.
    SourceSealed {
        ticket: SubmissionTicket,
        stage: GenerationStage,
        origin: OriginCell,
        manifest_digest: u64,
        declared_fragments: u32,
    },
    /// worker 完成了最终候选列的私有组装。
    ///
    /// Worker finished private assembly of a final candidate column.
    FinalCandidatePrepared {
        ticket: SubmissionTicket,
        candidate: FinalCandidatePayload,
        certificate: ReadyCertificate,
    },
}

impl WorkerSubmission {
    /// 本提交携带的票据。
    ///
    /// The ticket carried by this submission.
    pub fn ticket(&self) -> &SubmissionTicket {
        match self {
            Self::BasePrepared { ticket, .. } => ticket,
            Self::FragmentsBatch { ticket, .. } => ticket,
            Self::SourceSealed { ticket, .. } => ticket,
            Self::FinalCandidatePrepared { ticket, .. } => ticket,
        }
    }

    /// 本提交用于邮箱限流估算的内存占用。
    ///
    /// Estimated memory usage of this submission for mailbox bounding.
    pub fn estimated_bytes(&self) -> usize {
        match self {
            Self::BasePrepared { .. } => 512, // Arc handle overhead
            Self::FragmentsBatch { fragments, .. } => {
                let writes_count: usize = fragments.iter().map(|f| f.writes.len()).sum();
                std::mem::size_of::<Self>()
                    + fragments.len() * std::mem::size_of::<ChunkFragmentPayload>()
                    + writes_count * std::mem::size_of::<FragmentBlockWrite>()
            }
            Self::SourceSealed { .. } => std::mem::size_of::<Self>(),
            Self::FinalCandidatePrepared { .. } => 1024,
        }
    }
}

/// 单个区域所有者的有界提交邮箱。
///
/// Bounded submission mailbox for a single region owner.
pub struct RegionSubmissionMailbox {
    owner: RegionOwnerKey,
    max_messages: usize,
    max_bytes: usize,
    messages: Mutex<VecDeque<WorkerSubmission>>,
    current_bytes: AtomicUsize,
}

impl RegionSubmissionMailbox {
    /// 创建一个有界提交邮箱；各上限会被抬升到最小可用值。
    ///
    /// Creates a bounded submission mailbox; limits are raised to their
    /// minimum usable values.
    pub fn new(owner: RegionOwnerKey, max_messages: usize, max_bytes: usize) -> Self {
        Self {
            owner,
            max_messages: max_messages.max(1),
            max_bytes: max_bytes.max(1024),
            messages: Mutex::new(VecDeque::new()),
            current_bytes: AtomicUsize::new(0),
        }
    }

    /// 入队一条 worker 提交，严格执行队列数量与字节上限。
    ///
    /// Enqueues a worker submission, applying strict queue count and byte
    /// limits.
    pub fn send(&self, submission: WorkerSubmission) -> Result<(), GenerationContractError> {
        let bytes = submission.estimated_bytes();
        let mut queue = self.messages.lock();
        if queue.len() >= self.max_messages {
            return Err(GenerationContractError::MailboxSaturated {
                region_x: self.owner.region_x,
                region_z: self.owner.region_z,
                capacity: self.max_messages,
            });
        }
        let cur = self.current_bytes.load(Ordering::Acquire);
        if cur.saturating_add(bytes) > self.max_bytes {
            return Err(GenerationContractError::MailboxSaturated {
                region_x: self.owner.region_x,
                region_z: self.owner.region_z,
                capacity: self.max_bytes,
            });
        }
        self.current_bytes.fetch_add(bytes, Ordering::Release);
        queue.push_back(submission);
        Ok(())
    }

    /// 排空邮箱中当前所有待处理提交。
    ///
    /// Drains all currently pending submissions from the mailbox.
    pub fn drain(&self) -> Vec<WorkerSubmission> {
        let mut queue = self.messages.lock();
        let items: Vec<WorkerSubmission> = queue.drain(..).collect();
        self.current_bytes.store(0, Ordering::Release);
        items
    }

    /// 邮箱中当前的消息数量。
    ///
    /// Number of messages currently in the mailbox.
    pub fn len(&self) -> usize {
        self.messages.lock().len()
    }

    /// 邮箱是否为空。
    ///
    /// Whether the mailbox is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 当前已计入的字节数。
    ///
    /// Currently accounted bytes.
    pub fn current_bytes(&self) -> usize {
        self.current_bytes.load(Ordering::Acquire)
    }
}

/// 所有者处理提交并更新已发布状态时发出的事件。
///
/// Event emitted when an owner processes a submission and updates published
/// state.
#[derive(Clone)]
pub enum RegionPublishEvent {
    BaseReady {
        chunk_x: i32,
        chunk_z: i32,
        base_digest: u64,
    },
    SealsUpdated {
        chunk_x: i32,
        chunk_z: i32,
        stage: GenerationStage,
        is_complete: bool,
    },
    FragmentsReceived {
        chunk_x: i32,
        chunk_z: i32,
        received_count: u32,
        total_declared: u32,
    },
    ColumnPublished {
        chunk_x: i32,
        chunk_z: i32,
        column: Arc<ChunkColumn>,
        certificate: ReadyCertificate,
    },
}

impl std::fmt::Debug for RegionPublishEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BaseReady { chunk_x, chunk_z, base_digest } => f
                .debug_struct("BaseReady")
                .field("chunk_x", chunk_x)
                .field("chunk_z", chunk_z)
                .field("base_digest", &format_args!("{base_digest:#x}"))
                .finish(),
            Self::SealsUpdated { chunk_x, chunk_z, stage, is_complete } => f
                .debug_struct("SealsUpdated")
                .field("chunk_x", chunk_x)
                .field("chunk_z", chunk_z)
                .field("stage", stage)
                .field("is_complete", is_complete)
                .finish(),
            Self::FragmentsReceived { chunk_x, chunk_z, received_count, total_declared } => f
                .debug_struct("FragmentsReceived")
                .field("chunk_x", chunk_x)
                .field("chunk_z", chunk_z)
                .field("received_count", received_count)
                .field("total_declared", total_declared)
                .finish(),
            Self::ColumnPublished { chunk_x, chunk_z, certificate, .. } => f
                .debug_struct("ColumnPublished")
                .field("chunk_x", chunk_x)
                .field("chunk_z", chunk_z)
                .field("certificate", certificate)
                .finish(),
        }
    }
}

/// 权威的区域所有者状态。
///
/// 拥有其网格内所有区块的权威生成控制记录与已发布区块列。
///
/// Authoritative Region owner state.
///
/// Owns the authoritative generation control records and published chunk
/// columns for all chunks in its grid.
pub struct RegionOwner {
    pub key: RegionOwnerKey,
    pub region_size_chunks: u32,
    pub epoch: OwnerEpoch,
    pub records: HashMap<(i32, i32), ChunkGenerationRecord>,
    pub mailbox: Arc<RegionSubmissionMailbox>,
    pub published_columns: HashMap<(i32, i32), Arc<ChunkColumn>>,
}

impl RegionOwner {
    /// 创建一个区域所有者；纪元从 1 开始。
    ///
    /// Creates a region owner; the epoch starts at 1.
    pub fn new(
        key: RegionOwnerKey,
        region_size_chunks: u32,
        mailbox_max_messages: usize,
        mailbox_max_bytes: usize,
    ) -> Self {
        let mailbox = Arc::new(RegionSubmissionMailbox::new(
            key,
            mailbox_max_messages,
            mailbox_max_bytes,
        ));
        Self {
            key,
            region_size_chunks: region_size_chunks.max(1),
            epoch: 1,
            records: HashMap::new(),
            mailbox,
            published_columns: HashMap::new(),
        }
    }

    /// 递增纪元，隔离任何携带旧纪元的在途 worker。
    ///
    /// Advances the epoch, fencing any in-flight workers carrying the old
    /// epoch.
    pub fn advance_epoch(&mut self) -> OwnerEpoch {
        self.epoch += 1;
        self.epoch
    }

    /// 将一个区块注册到本区域所有者的管辖之下。
    ///
    /// Registers a chunk under this region owner's authority.
    pub fn register_chunk(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
        generator_fingerprint: u64,
    ) -> Result<(), GenerationContractError> {
        if !self.key.contains_chunk(chunk_x, chunk_z, self.region_size_chunks) {
            let expected = RegionOwnerKey::region_of(
                self.key.dimension,
                chunk_x,
                chunk_z,
                self.region_size_chunks,
            );
            return Err(GenerationContractError::OwnerMismatch {
                chunk_x,
                chunk_z,
                expected_region_x: expected.region_x,
                expected_region_z: expected.region_z,
                actual_region_x: self.key.region_x,
                actual_region_z: self.key.region_z,
            });
        }
        self.records.entry((chunk_x, chunk_z)).or_insert_with(|| {
            ChunkGenerationRecord::new_queued(
                self.key.dimension,
                chunk_x,
                chunk_z,
                generator_fingerprint,
            )
        });
        Ok(())
    }

    /// 查找某区块的生成记录（只读）。
    ///
    /// Looks up the generation record of a chunk (read-only).
    pub fn get_record(&self, chunk_x: i32, chunk_z: i32) -> Option<&ChunkGenerationRecord> {
        self.records.get(&(chunk_x, chunk_z))
    }

    /// 查找某区块的生成记录（可变）。
    ///
    /// Looks up the generation record of a chunk (mutable).
    pub fn get_record_mut(&mut self, chunk_x: i32, chunk_z: i32) -> Option<&mut ChunkGenerationRecord> {
        self.records.get_mut(&(chunk_x, chunk_z))
    }

    /// 排空区域邮箱，并依据权威记录与所有者纪元校验每条提交。
    ///
    /// Drains the region mailbox and validates each submission against the
    /// authoritative records and owner epoch.
    pub fn process_mailbox(&mut self) -> Result<Vec<RegionPublishEvent>, GenerationContractError> {
        let submissions = self.mailbox.drain();
        let mut events = Vec::new();

        for sub in submissions {
            let ticket = *sub.ticket();

            // 1. Validate owner match
            if ticket.owner != self.key {
                return Err(GenerationContractError::OwnerMismatch {
                    chunk_x: ticket.chunk_x,
                    chunk_z: ticket.chunk_z,
                    expected_region_x: self.key.region_x,
                    expected_region_z: self.key.region_z,
                    actual_region_x: ticket.owner.region_x,
                    actual_region_z: ticket.owner.region_z,
                });
            }

            // 2. Fence stale owner epoch
            if ticket.epoch != self.epoch {
                return Err(GenerationContractError::StaleOwnerEpoch {
                    region_x: self.key.region_x,
                    region_z: self.key.region_z,
                    expected: self.epoch,
                    received: ticket.epoch,
                });
            }

            // 3. Find record and check attempt
            let record = match self.records.get_mut(&(ticket.chunk_x, ticket.chunk_z)) {
                Some(r) => r,
                None => {
                    return Err(GenerationContractError::InvalidConfiguration(format!(
                        "submission for unregistered chunk ({},{})",
                        ticket.chunk_x, ticket.chunk_z
                    )));
                }
            };

            if ticket.attempt != record.attempt {
                return Err(GenerationContractError::StaleAttempt {
                    key_x: ticket.chunk_x,
                    key_z: ticket.chunk_z,
                    expected: record.attempt,
                    received: ticket.attempt,
                });
            }

            // 4. Process by submission type
            match sub {
                WorkerSubmission::BasePrepared { snapshot, .. } => {
                    record.base_digest = Some(snapshot.digest);
                    record.state = GenerationControlState::BaseReady;
                    events.push(RegionPublishEvent::BaseReady {
                        chunk_x: ticket.chunk_x,
                        chunk_z: ticket.chunk_z,
                        base_digest: snapshot.digest,
                    });
                }
                WorkerSubmission::SourceSealed {
                    stage,
                    origin,
                    manifest_digest,
                    declared_fragments,
                    ..
                } => {
                    record.apply_seal(stage, origin, manifest_digest, declared_fragments)?;
                    let is_complete = match stage {
                        GenerationStage::Base => true,
                        GenerationStage::Structure => record.structure_seals.is_complete(),
                        GenerationStage::Decoration => record.decoration_seals.is_complete(),
                    };
                    events.push(RegionPublishEvent::SealsUpdated {
                        chunk_x: ticket.chunk_x,
                        chunk_z: ticket.chunk_z,
                        stage,
                        is_complete,
                    });
                }
                WorkerSubmission::FragmentsBatch { fragments, .. } => {
                    let count = fragments.len() as u32;
                    record.accept_fragment(count)?;
                    events.push(RegionPublishEvent::FragmentsReceived {
                        chunk_x: ticket.chunk_x,
                        chunk_z: ticket.chunk_z,
                        received_count: record.received_fragments,
                        total_declared: record.declared_fragments,
                    });
                }
                WorkerSubmission::FinalCandidatePrepared {
                    candidate,
                    certificate,
                    ..
                } => {
                    record.assembly_digest = Some(candidate.assembly_digest);
                    record.commit_ready(ticket.attempt, certificate)?;
                    self.published_columns
                        .insert((ticket.chunk_x, ticket.chunk_z), candidate.column.clone());
                    events.push(RegionPublishEvent::ColumnPublished {
                        chunk_x: ticket.chunk_x,
                        chunk_z: ticket.chunk_z,
                        column: candidate.column,
                        certificate,
                    });
                }
            }
        }

        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPosition};

    fn make_test_column(cx: i32, cz: i32) -> Arc<ChunkColumn> {
        Arc::new(ChunkColumn::new(Chunk::empty(
            ChunkPosition::new(cx, cz),
            0,
            -64,
            319,
        )))
    }

    #[test]
    fn region_owner_key_mapping_is_floor_euclid() {
        assert_eq!(
            RegionOwnerKey::region_of(0, 0, 0, 2),
            RegionOwnerKey::new(0, 0, 0)
        );
        assert_eq!(
            RegionOwnerKey::region_of(0, 1, 1, 2),
            RegionOwnerKey::new(0, 0, 0)
        );
        assert_eq!(
            RegionOwnerKey::region_of(0, 2, 0, 2),
            RegionOwnerKey::new(0, 1, 0)
        );
        // Negative coordinates
        assert_eq!(
            RegionOwnerKey::region_of(0, -1, -1, 2),
            RegionOwnerKey::new(0, -1, -1)
        );
        assert_eq!(
            RegionOwnerKey::region_of(0, -2, -2, 2),
            RegionOwnerKey::new(0, -1, -1)
        );
        assert_eq!(
            RegionOwnerKey::region_of(0, -3, -1, 2),
            RegionOwnerKey::new(0, -2, -1)
        );
    }

    #[test]
    fn mailbox_bounds_reject_when_full() {
        let key = RegionOwnerKey::new(0, 0, 0);
        let mailbox = RegionSubmissionMailbox::new(key, 2, 10_000);
        let ticket = SubmissionTicket::new(key, 1, 0, 0, 1);

        let sub1 = WorkerSubmission::SourceSealed {
            ticket,
            stage: GenerationStage::Structure,
            origin: OriginCell::new(0, 0),
            manifest_digest: 123,
            declared_fragments: 0,
        };
        let sub2 = WorkerSubmission::SourceSealed {
            ticket,
            stage: GenerationStage::Decoration,
            origin: OriginCell::new(0, 0),
            manifest_digest: 456,
            declared_fragments: 0,
        };
        let sub3 = WorkerSubmission::SourceSealed {
            ticket,
            stage: GenerationStage::Decoration,
            origin: OriginCell::new(1, 0),
            manifest_digest: 789,
            declared_fragments: 0,
        };

        assert!(mailbox.send(sub1).is_ok());
        assert!(mailbox.send(sub2).is_ok());
        // 3rd message exceeds capacity of 2!
        let err = mailbox.send(sub3).unwrap_err();
        assert!(matches!(err, GenerationContractError::MailboxSaturated { .. }));
    }

    #[test]
    fn stale_epoch_is_fenced_and_rejected() {
        let key = RegionOwnerKey::new(0, 0, 0);
        let mut owner = RegionOwner::new(key, 2, 64, 1_000_000);
        owner.register_chunk(0, 0, 0xcafe).unwrap();

        // Worker was dispatched with epoch 1
        let ticket = SubmissionTicket::new(key, 1, 0, 0, 1);
        let sub = WorkerSubmission::SourceSealed {
            ticket,
            stage: GenerationStage::Structure,
            origin: OriginCell::new(0, 0),
            manifest_digest: 123,
            declared_fragments: 0,
        };
        owner.mailbox.send(sub).unwrap();

        // Owner advances epoch before processing!
        owner.advance_epoch();
        assert_eq!(owner.epoch, 2);

        // Mailbox process must reject the stale submission
        let err = owner.process_mailbox().unwrap_err();
        assert!(matches!(
            err,
            GenerationContractError::StaleOwnerEpoch { expected: 2, received: 1, .. }
        ));
    }

    #[test]
    fn owner_accepts_valid_lifecycle_and_publishes_column() {
        let key = RegionOwnerKey::new(0, 0, 0);
        let mut owner = RegionOwner::new(key, 2, 64, 1_000_000);
        owner.register_chunk(0, 0, 0xcafe).unwrap();

        // 1. Base prepared
        let snapshot = Arc::new(BaseSnapshot::new(
            0,
            ChunkPosition::new(0, 0),
            1,
            0xcafe,
            -64,
            319,
            Arc::new(Chunk::empty(ChunkPosition::new(0, 0), 0, -64, 319)),
            [64; 256],
        ));
        let expected_digest = snapshot.digest;
        let ticket = SubmissionTicket::new(key, owner.epoch, 0, 0, 1);
        owner.mailbox.send(WorkerSubmission::BasePrepared {
            ticket,
            snapshot: snapshot.clone(),
        }).unwrap();

        let events = owner.process_mailbox().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], RegionPublishEvent::BaseReady { base_digest, .. } if base_digest == expected_digest));

        // 2. Structure seals
        owner.get_record_mut(0, 0).unwrap().register_expected_seals(
            GenerationStage::Structure,
            vec![OriginCell::new(0, 0)],
        ).unwrap();
        owner.mailbox.send(WorkerSubmission::SourceSealed {
            ticket,
            stage: GenerationStage::Structure,
            origin: OriginCell::new(0, 0),
            manifest_digest: 111,
            declared_fragments: 1,
        }).unwrap();
        let events = owner.process_mailbox().unwrap();
        assert_eq!(events.len(), 1);

        // 3. Fragment batch
        let fragment = ChunkFragmentPayload {
            fragment_id: FragmentId {
                feature: crate::generation::identity::FeatureId {
                    generator_fingerprint: 0xcafe,
                    dimension: 0,
                    stage: GenerationStage::Structure,
                    feature_type: crate::generation::identity::FeatureType::DesertWell,
                    origin: OriginCell::new(0, 0),
                    candidate_ordinal: 0,
                },
                target_x: 0,
                target_z: 0,
                part_index: 0,
            },
            writes: vec![FragmentBlockWrite {
                local_x: 5,
                y: 65,
                local_z: 5,
                runtime_id: 100,
                layer: 0,
            }],
        };
        owner.mailbox.send(WorkerSubmission::FragmentsBatch {
            ticket,
            fragments: vec![fragment],
            manifest_digest: 111,
        }).unwrap();
        let events = owner.process_mailbox().unwrap();
        assert_eq!(events.len(), 1);

        // 4. Decoration seals (empty)
        owner.get_record_mut(0, 0).unwrap().register_expected_seals(
            GenerationStage::Decoration,
            vec![],
        ).unwrap();

        // 5. Final candidate prepared
        let col = make_test_column(0, 0);
        let cert = ReadyCertificate {
            base_valid: true,
            structure_seals_complete: true,
            decoration_seals_complete: true,
            all_fragments_verified: true,
            assembly_validated: true,
            metadata_included: true,
            owner_epoch: owner.epoch,
        };
        owner.mailbox.send(WorkerSubmission::FinalCandidatePrepared {
            ticket,
            candidate: FinalCandidatePayload {
                chunk_x: 0,
                chunk_z: 0,
                assembly_digest: 0xbeef,
                column: col.clone(),
            },
            certificate: cert,
        }).unwrap();

        let events = owner.process_mailbox().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], RegionPublishEvent::ColumnPublished { .. }));
        assert!(owner.published_columns.contains_key(&(0, 0)));
        assert_eq!(owner.get_record(0, 0).unwrap().state, GenerationControlState::Ready);
    }
}
