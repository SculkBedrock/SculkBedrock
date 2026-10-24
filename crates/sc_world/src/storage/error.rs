use super::{ChunkKey, MAX_DIRTY_FLUSH_KEY_SAMPLES};
use sc_error::{ScError, ScSite, capture_site};
use std::fmt::{Display, Formatter};

/// 脏列 flush 的结果：含关闭路径在持久化不完整或脏集无法检查时
/// 需要报告的内容。
///
/// Outcome of a dirty-column flush, including what the shutdown path needs to
/// report when persistence is incomplete or its dirty set cannot be inspected.
#[derive(Debug, Default)]
pub struct DirtyFlushReport {
    /// 尝试持久化的列数。
    ///
    /// Number of columns attempted.
    pub attempted: usize,
    /// 确认保存的列数。
    ///
    /// Number of columns confirmed saved.
    pub saved: usize,
    /// 失败键的有界采样；`failed_count` 是精确总数。
    ///
    /// Bounded samples of failed keys; `failed_count` is the exact total.
    pub failed_keys: Vec<ChunkKey>,
    /// 持久化期间内容发生变化的键的有界采样；精确总数见
    /// `changed_during_save_count`。
    ///
    /// Bounded samples of keys changed during persistence; exact total is in
    /// `changed_during_save_count`.
    pub changed_during_save: Vec<ChunkKey>,
    /// 失败精确总数。
    ///
    /// Exact total number of failures.
    pub failed_count: usize,
    /// 被单列关闭字节上限拒绝的快照数。
    ///
    /// Snapshots rejected by the single-column shutdown byte limit.
    pub oversize_count: usize,
    /// 持久化期间内容变化的精确总数。
    ///
    /// Exact total number of keys changed during persistence.
    pub changed_during_save_count: usize,
    /// 任一键采样超过固定容量时为真。
    ///
    /// True when either key sample exceeded its fixed capacity.
    pub key_samples_truncated: bool,
    /// `None` 表示 provider 无法检查剩余脏集。
    ///
    /// `None` means the provider could not inspect the remaining dirty set.
    pub remaining_dirty: Option<usize>,
    /// 脏集扫描失败为真。
    ///
    /// True when the dirty-set scan failed.
    pub scan_failed: bool,
}

impl DirtyFlushReport {
    /// 记录一次失败（键采样有界，超出置截断标记）。
    ///
    /// Records a failure (key samples are bounded; overflow sets the
    /// truncated flag).
    ///
    /// - `key`: 失败的区块键
    pub(crate) fn record_failure(&mut self, key: ChunkKey) {
        self.failed_count += 1;
        if self.failed_keys.len() < MAX_DIRTY_FLUSH_KEY_SAMPLES {
            self.failed_keys.push(key);
        } else {
            self.key_samples_truncated = true;
        }
    }

    /// 记录一次「持久化期间内容变化」（键采样有界）。
    ///
    /// Records a key changed during persistence (bounded samples).
    ///
    /// - `key`: 发生变化的区块键
    pub(crate) fn record_changed(&mut self, key: ChunkKey) {
        self.changed_during_save_count += 1;
        if self.changed_during_save.len() < MAX_DIRTY_FLUSH_KEY_SAMPLES {
            self.changed_during_save.push(key);
        } else {
            self.key_samples_truncated = true;
        }
    }

    /// 记录一次超限拒绝（同时计入失败）。
    ///
    /// Records an oversize rejection (also counted as a failure).
    ///
    /// - `key`: 被拒的区块键
    pub(crate) fn record_oversize(&mut self, key: ChunkKey) {
        self.oversize_count += 1;
        self.record_failure(key);
    }

    /// 是否全部干净完成：无扫描失败、无失败、无保存期变化、无剩余脏列。
    ///
    /// Whether the flush completed cleanly: no scan failure, no failures, no
    /// changes during save, and no remaining dirty columns.
    pub fn is_complete(&self) -> bool {
        !self.scan_failed
            && self.failed_count == 0
            && self.changed_during_save_count == 0
            && self.remaining_dirty == Some(0)
    }
}

/// 世界存储错误。
///
/// Errors produced by world storage.
#[derive(Debug, ScError)]
pub enum WorldStorageError {
    /// 底层 I/O 失败。
    ///
    /// Low-level I/O failure.
    #[sc_error(code = "world.storage.io")]
    Io {
        /// 底层 I/O 错误。
        ///
        /// Underlying I/O error.
        source: std::io::Error,
        /// 说明原因的英文细节。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    /// 存储的字节未通过校验。
    ///
    /// Stored bytes fail validation.
    #[sc_error(code = "world.storage.corrupt")]
    Corrupt {
        /// 说明原因的英文细节。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    /// 数据合法但后端无法表示。
    ///
    /// Valid data the backend cannot represent.
    #[sc_error(code = "world.storage.unsupported")]
    Unsupported {
        /// 说明原因的英文细节。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    /// 准入被拒，且未发布或丢弃任何脏数据。
    ///
    /// Provider admission was rejected without publishing or dropping dirty data.
    #[sc_error(code = "world.storage.capacity")]
    Capacity {
        /// 说明原因的英文细节。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    /// 后端失败（LevelDB、通道、worker）。
    ///
    /// Backend failure (LevelDB, channel, worker).
    #[sc_error(code = "world.storage.backend")]
    Backend {
        /// 说明原因的英文细节。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
}

impl WorldStorageError {
    /// 构造 I/O 错误，并捕获调用位置。
    ///
    /// Build an I/O error, capturing the call site.
    ///
    /// - `source`: 底层 I/O 错误
    #[track_caller]
    pub fn io(source: std::io::Error) -> Self {
        let detail = source.to_string();
        Self::Io {
            source,
            detail,
            site: capture_site(),
        }
    }

    /// 构造数据损坏错误，并捕获调用位置。
    ///
    /// Build a corruption error, capturing the call site.
    ///
    /// - `detail`: 说明原因的英文细节
    #[track_caller]
    pub fn corrupt(detail: String) -> Self {
        Self::Corrupt {
            detail,
            site: capture_site(),
        }
    }

    /// 构造「数据不受支持」错误，并捕获调用位置。
    ///
    /// Build an unsupported-data error, capturing the call site.
    ///
    /// - `detail`: 说明原因的英文细节
    #[track_caller]
    pub fn unsupported(detail: String) -> Self {
        Self::Unsupported {
            detail,
            site: capture_site(),
        }
    }

    /// 构造「准入被拒」错误，并捕获调用位置。
    ///
    /// Build an admission-rejection error, capturing the call site.
    ///
    /// - `detail`: 说明原因的英文细节
    #[track_caller]
    pub fn capacity(detail: String) -> Self {
        Self::Capacity {
            detail,
            site: capture_site(),
        }
    }

    /// 构造后端错误，并捕获调用位置。
    ///
    /// Build a backend error, capturing the call site.
    ///
    /// - `detail`: 说明原因的英文细节
    #[track_caller]
    pub fn backend(detail: String) -> Self {
        Self::Backend {
            detail,
            site: capture_site(),
        }
    }
}

impl Display for WorldStorageError {
    /// 按错误码 + 细节 + 位置的格式格式化错误。
    ///
    /// Formats the error as code + detail + site.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { detail, site, .. } => {
                write!(formatter, "[world.storage.io] {detail} (at {site})")
            }
            Self::Corrupt { detail, site } => {
                write!(formatter, "[world.storage.corrupt] {detail} (at {site})")
            }
            Self::Unsupported { detail, site } => {
                write!(formatter, "[world.storage.unsupported] {detail} (at {site})")
            }
            Self::Capacity { detail, site } => {
                write!(
                    formatter,
                    "[world.storage.capacity] {detail} (at {site})"
                )
            }
            Self::Backend { detail, site } => {
                write!(formatter, "[world.storage.backend] {detail} (at {site})")
            }
        }
    }
}

impl std::error::Error for WorldStorageError {
    /// 返回底层错误源（仅 I/O 变体有源）。
    ///
    /// Returns the underlying error source (only the I/O variant has one).
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for WorldStorageError {
    /// 由 [`std::io::Error`] 转换（捕获调用位置）。
    ///
    /// Converts from [`std::io::Error`] (captures the call site).
    #[track_caller]
    fn from(error: std::io::Error) -> Self {
        Self::io(error)
    }
}
