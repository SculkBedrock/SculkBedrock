use super::{ChunkKey, MAX_DIRTY_FLUSH_KEY_SAMPLES};
use sc_error::{ScError, ScSite, capture_site};
use std::fmt::{Display, Formatter};

/// Outcome of a dirty-column flush, including what the shutdown path needs to
/// report when persistence is incomplete or its dirty set cannot be inspected.
#[derive(Debug, Default)]
pub struct DirtyFlushReport {
    pub attempted: usize,
    pub saved: usize,
    /// Bounded samples of failed keys; `failed_count` is the exact total.
    pub failed_keys: Vec<ChunkKey>,
    /// Bounded samples of keys changed during persistence; exact total is in
    /// `changed_during_save_count`.
    pub changed_during_save: Vec<ChunkKey>,
    pub failed_count: usize,
    /// Snapshots rejected by the single-column shutdown byte limit.
    pub oversize_count: usize,
    pub changed_during_save_count: usize,
    /// True when either key sample exceeded its fixed capacity.
    pub key_samples_truncated: bool,
    /// `None` means the provider could not inspect the remaining dirty set.
    pub remaining_dirty: Option<usize>,
    pub scan_failed: bool,
}

impl DirtyFlushReport {
    pub(crate) fn record_failure(&mut self, key: ChunkKey) {
        self.failed_count += 1;
        if self.failed_keys.len() < MAX_DIRTY_FLUSH_KEY_SAMPLES {
            self.failed_keys.push(key);
        } else {
            self.key_samples_truncated = true;
        }
    }

    pub(crate) fn record_changed(&mut self, key: ChunkKey) {
        self.changed_during_save_count += 1;
        if self.changed_during_save.len() < MAX_DIRTY_FLUSH_KEY_SAMPLES {
            self.changed_during_save.push(key);
        } else {
            self.key_samples_truncated = true;
        }
    }

    pub(crate) fn record_oversize(&mut self, key: ChunkKey) {
        self.oversize_count += 1;
        self.record_failure(key);
    }

    pub fn is_complete(&self) -> bool {
        !self.scan_failed
            && self.failed_count == 0
            && self.changed_during_save_count == 0
            && self.remaining_dirty == Some(0)
    }
}
#[derive(Debug, ScError)]
pub enum WorldStorageError {
    /// Low-level I/O failure.
    #[sc_error(code = "world.storage.io")]
    Io {
        /// Underlying I/O error.
        source: std::io::Error,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Stored bytes fail validation.
    #[sc_error(code = "world.storage.corrupt")]
    Corrupt {
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Valid data the backend cannot represent.
    #[sc_error(code = "world.storage.unsupported")]
    Unsupported {
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Provider admission was rejected without publishing or dropping dirty data.
    #[sc_error(code = "world.storage.capacity")]
    Capacity {
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Backend failure (LevelDB, channel, worker).
    #[sc_error(code = "world.storage.backend")]
    Backend {
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
}

impl WorldStorageError {
    /// Build an I/O error, capturing the call site.
    #[track_caller]
    pub fn io(source: std::io::Error) -> Self {
        let detail = source.to_string();
        Self::Io {
            source,
            detail,
            site: capture_site(),
        }
    }

    /// Build a corruption error, capturing the call site.
    #[track_caller]
    pub fn corrupt(detail: String) -> Self {
        Self::Corrupt {
            detail,
            site: capture_site(),
        }
    }

    /// Build an unsupported-data error, capturing the call site.
    #[track_caller]
    pub fn unsupported(detail: String) -> Self {
        Self::Unsupported {
            detail,
            site: capture_site(),
        }
    }

    /// Build an admission-rejection error, capturing the call site.
    #[track_caller]
    pub fn capacity(detail: String) -> Self {
        Self::Capacity {
            detail,
            site: capture_site(),
        }
    }

    /// Build a backend error, capturing the call site.
    #[track_caller]
    pub fn backend(detail: String) -> Self {
        Self::Backend {
            detail,
            site: capture_site(),
        }
    }
}

impl Display for WorldStorageError {
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
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for WorldStorageError {
    #[track_caller]
    fn from(error: std::io::Error) -> Self {
        Self::io(error)
    }
}
