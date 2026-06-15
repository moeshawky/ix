use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// File change operation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileOp {
    /// File was created.
    Create,
    /// File was modified.
    Modify,
    /// File was deleted.
    Delete,
    /// File was renamed.
    Rename,
}

impl FileOp {
    /// Convert from the notify crate's event kind to our serializable enum.
    #[must_use]
    pub const fn from_notify_kind(kind: notify::EventKind) -> Self {
        match kind {
            notify::EventKind::Create(_) => Self::Create,
            notify::EventKind::Remove(_) => Self::Delete,
            notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) => Self::Rename,
            _ => Self::Modify,
        }
    }
}

/// Typed daemon status enum for structured status tracking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DaemonStatus {
    /// Daemon is idle, no active rebuild.
    Idle,
    /// Active index rebuild in progress.
    Indexing {
        /// Current entropy reading.
        entropy: u16,
    },
    /// Full compaction rebuild in progress (idle or delta-driven).
    Compacting,
    /// Rebuild deferred due to high entropy.
    Deferred {
        /// Current entropy reading.
        entropy: u16,
    },
    /// Safety escalation triggered.
    Escalated {
        /// Current entropy reading.
        entropy: u16,
    },
    /// Safety warning issued.
    Warned {
        /// Warning reason.
        reason: String,
    },
    /// Critical safety halt — daemon stopped.
    SafetyHalt,
    /// Unrecoverable safety exit.
    SafetyExit,
    /// Initial index build failed; daemon will watch for changes.
    BuildFailed {
        /// Human-readable build error message.
        error: String,
    },
}

impl std::fmt::Display for DaemonStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle { .. } => write!(f, "idle"),
            Self::Indexing { entropy } => write!(f, "indexing (entropy: {entropy})"),
            Self::Compacting => write!(f, "compacting"),
            Self::Deferred { entropy } => write!(f, "deferred (entropy: {entropy})"),
            Self::Escalated { entropy } => write!(f, "escalated (entropy: {entropy})"),
            Self::Warned { reason } => write!(f, "warned: {reason}"),
            Self::SafetyHalt => write!(f, "safety halt"),
            Self::SafetyExit => write!(f, "safety exit"),
            Self::BuildFailed { error } => write!(f, "build failed: {error}"),
        }
    }
}

/// A single file change record broadcast to connected clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    /// Path of the changed file (relative to watched root when possible).
    #[serde(rename = "p")]
    pub path: PathBuf,
    /// Modification timestamp (Unix seconds).
    #[serde(rename = "m")]
    pub mtime: u64,
    /// Operation performed on the file.
    #[serde(rename = "o")]
    pub op: FileOp,
}

/// Helper for serde defaults: `true`.
pub(super) const fn default_true() -> bool {
    true
}

/// Search results returned from daemon to client.
///
/// The `error` field is populated only when the daemon encounters an error
/// while executing the query (e.g., invalid regex pattern). Clients MUST
/// check this field and propagate the error instead of treating an empty
/// result as a successful zero-match search.
///
/// The field uses `#[serde(default)]` + `#[serde(skip_serializing_if)]`
/// for backward compatibility: new daemon ↔ old client (old client ignores
/// unknown field), old daemon ↔ new client (error deserializes as `None`).
///
/// For progressive queries, the daemon sends `SearchResults` messages,
/// ending with `done` = `true`.
///
/// NOTE: Progressive search currently operates in single-batch mode.
/// Every search returns exactly one batch with `done: true`. The
/// multi-batch progressive delivery path is not yet implemented.
/// Clients MUST NOT wait for additional batches after receiving
/// `done: true` — the channel is closed after the single batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults {
    /// Query ID (matches the id from the request).
    pub id: u64,
    /// Matching results.
    pub matches: Vec<crate::executor::Match>,
    /// Query execution statistics.
    pub stats: crate::executor::QueryStats,
    /// Error message from the daemon.
    ///
    /// Set when the query could not be executed at all (invalid regex,
    /// index corruption) OR when some files could not be verified during
    /// result collection (I/O errors). When present, clients MUST NOT
    /// treat the result set as complete — check
    /// [`QueryStats::files_failed_verify`][crate::executor::QueryStats::files_failed_verify]
    /// for the count of unverifiable files. An empty result with an error
    /// means the query failed. A non-empty result with an error means
    /// some files were skipped and the result set is partial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether this is the final batch in a progressive query.
    /// Always `true` for non-progressive queries (single response).
    #[serde(default = "default_true")]
    pub done: bool,
    /// Batch sequence number (0-based) for progressive queries.
    #[serde(default)]
    pub batch: u32,
}

/// Messages sent from the server to connected clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Periodic or on-change daemon status update.
    Status {
        /// PID of the daemon process.
        pid: u32,
        /// Human-readable status string (e.g. "idle", "indexing").
        status: String,
        /// Number of files currently in the index.
        files: usize,
        /// Typed daemon status (present when daemon is running).
        #[serde(skip_serializing_if = "Option::is_none")]
        daemon_status: Option<DaemonStatus>,
    },
    /// Batch of file changes detected by the watcher.
    FilesChanged {
        /// The changed files in this batch.
        batch: Vec<FileChange>,
        /// Timestamp of this event batch (Unix seconds).
        #[serde(rename = "ts")]
        timestamp: u64,
    },
    /// Response to a client query.
    QueryResult {
        /// Query ID (matches the `id` field from the request).
        id: u64,
        /// Current daemon status at query time.
        status: String,
        /// Number of files in the index.
        files: usize,
        /// Changes since the requested timestamp (for history queries).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        changes_since: Vec<FileChange>,
        /// Typed daemon status (present when daemon is running).
        #[serde(skip_serializing_if = "Option::is_none")]
        daemon_status: Option<DaemonStatus>,
        /// Timestamp of the last successful rebuild completion.
        #[serde(skip_serializing_if = "Option::is_none")]
        last_rebuild_at: Option<u64>,
    },
    /// Search query results.
    SearchResults(SearchResults),
    /// Graceful shutdown notice sent to all clients before closing.
    Shutdown(ShutdownNotice),
}

/// Search query parameters sent from client to daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
// SearchQuery is a wire format struct with 7 boolean query flags
// that cannot be decomposed without breaking the JSON protocol.
#[allow(clippy::struct_excessive_bools)]
pub struct SearchQuery {
    /// Client-assigned query ID (echoed back in the response).
    #[serde(default)]
    pub id: u64,
    /// Pattern to search for.
    pub pattern: String,
    /// Interpret pattern as regex.
    #[serde(default)]
    pub is_regex: bool,
    /// Case-insensitive search.
    #[serde(default)]
    pub ignore_case: bool,
    /// Match whole words only.
    #[serde(default)]
    pub word_boundary: bool,
    /// Maximum number of results (0 = unlimited).
    #[serde(default)]
    pub max_results: usize,
    /// Number of context lines.
    #[serde(default)]
    pub context_lines: usize,
    /// File extensions filter.
    #[serde(default)]
    pub file_types: Vec<String>,
    /// Decompress archives.
    #[serde(default)]
    pub decompress: bool,
    /// Multiline mode (dot matches newline).
    #[serde(default)]
    pub multiline: bool,
    /// Search inside archives.
    #[serde(default)]
    pub archive: bool,
    /// Search binary files.
    #[serde(default)]
    pub binary: bool,
    /// Absolute path prefix to filter results (None = search entire root).
    #[serde(default)]
    pub search_path: Option<std::path::PathBuf>,
    /// If true, the daemon streams results progressively as batches.
    #[serde(default)]
    pub progressive: bool,
    /// Per-chunk size in bytes for large-file chunked streaming.
    /// 0 means use the streaming module's default (16 `MiB`).
    #[serde(default)]
    pub chunk_size_bytes: usize,
    /// Overlap between adjacent chunks in bytes.
    /// 0 means use the streaming module's default (1 `MiB`).
    #[serde(default)]
    pub chunk_overlap_bytes: usize,
}

/// Graceful shutdown notice sent from server to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShutdownNotice {
    /// Reason for shutdown (e.g., "signal", "`user_request`").
    pub reason: String,
    /// Milliseconds clients have before socket closes.
    pub delay_ms: u32,
}

/// Messages sent from connected clients to the daemon server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Request current daemon status.
    StatusQuery {
        /// Client-assigned query ID (echoed back in the response).
        #[serde(default)]
        id: u64,
    },
    /// Request all changes since the given timestamp.
    HistoryQuery {
        /// Return changes with timestamps strictly after this value.
        since: u64,
        /// Client-assigned query ID (echoed back in the response).
        id: u64,
    },
    /// Execute a search query.
    SearchQuery(SearchQuery),
    /// Client acknowledgment of shutdown notice (optional, for slow-client detection).
    Shutdown {
        /// Acknowledgment flag (true = received shutdown notice).
        #[serde(default)]
        ack: bool,
    },
}

/// Errors specific to the daemon socket subsystem.
#[derive(Debug, thiserror::Error)]
pub enum DaemonSockError {
    /// I/O error on the socket.
    #[error("daemon socket I/O: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization or deserialization error.
    #[error("daemon socket JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Could not resolve a suitable socket path.
    #[error("daemon socket path resolution failed")]
    PathResolution,
}

pub(super) type Result<T> = std::result::Result<T, DaemonSockError>;

/// Current Unix timestamp in seconds.
pub(super) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
