//! Unix domain socket interface for the ixd daemon.
//!
//! Provides real-time file-change notifications and status queries over a
//! local Unix domain socket using NDJSON (newline-delimited JSON) framing.
//!
//! # Socket Path Resolution
//!
//! The socket path is derived from the canonical watched root:
//!
//! ```text
//! $XDG_RUNTIME_DIR/ixd/{hash}.sock        # preferred (systemd, modern Linux)
//! ~/.local/run/ixd/{hash}.sock             # fallback
//! /tmp/ixd-{uid}-{hash}.sock              # last resort
//! ```
//!
//! Where `hash` = first 16 hex chars of `XXH64(canonical_path, seed=0)`.
//!
//! # Wire Protocol (NDJSON)
//!
//! Each line is a valid JSON object terminated by `\\n`.
//!
//! **Server → Client (push):**
//!
//! ```json
//! {"t":"status","pid":1234,"status":"idle","files":1523}
//! {"t":"files_changed","batch":[{"p":"src/main.rs","m":1776468629,"o":"modify"}],"ts":1776468629}
//! ```
//!
//! **Client → Server (query):**
//!
//! ```json
//! {"t":"status_query"}
//! {"t":"history_query","since":1776468000,"id":1}
//! ```
//!
//! **Server → Client (query response):**
//!
//! ```json
//! {"t":"query_result","id":1,"status":"idle","files":1523,"changes_since":[...]}
//! ```

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum number of change batches retained for history queries.
const HISTORY_CAPACITY: usize = 1024;

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
    pub fn from_notify_kind(kind: notify::EventKind) -> Self {
        match kind {
            notify::EventKind::Create(_) => Self::Create,
            notify::EventKind::Remove(_) => Self::Delete,
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
}

impl std::fmt::Display for DaemonStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle { .. } => write!(f, "idle"),
            Self::Indexing { entropy } => write!(f, "indexing (entropy: {entropy})"),
            Self::Deferred { entropy } => write!(f, "deferred (entropy: {entropy})"),
            Self::Escalated { entropy } => write!(f, "escalated (entropy: {entropy})"),
            Self::Warned { reason } => write!(f, "warned: {reason}"),
            Self::SafetyHalt => write!(f, "safety halt"),
            Self::SafetyExit => write!(f, "safety exit"),
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

/// Search results returned from daemon to client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults {
    /// Query ID (matches the id from the request).
    pub id: u64,
    /// Matching results.
    pub matches: Vec<crate::executor::Match>,
    /// Query execution statistics.
    pub stats: crate::executor::QueryStats,
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

type Result<T> = std::result::Result<T, DaemonSockError>;

/// Resolves the socket path for a given watched root directory.
///
/// Tries in order:
/// 1. `$XDG_RUNTIME_DIR/ixd/{hash}.sock`
/// 2. `$HOME/.local/run/ixd/{hash}.sock`
/// 3. `/tmp/ixd-{uid}-{hash}.sock`
///
/// Where `hash` is the first 16 hex characters of `XXH64(canonical_root, 0)`.
#[must_use]
pub fn socket_path(root: &Path) -> PathBuf {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let hash = format!(
        "{:016x}",
        xxhash_rust::xxh64::xxh64(canonical.to_string_lossy().as_bytes(), 0,)
    );

    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        let dir = PathBuf::from(xdg).join("ixd");
        return dir.join(format!("{hash}.sock"));
    }

    if let Ok(home) = std::env::var("HOME") {
        let dir = PathBuf::from(home).join(".local/run/ixd");
        return dir.join(format!("{hash}.sock"));
    }

    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/ixd-{uid}-{hash}.sock"))
}

/// Ensure the parent directory of a socket path exists.
fn ensure_socket_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Circular buffer of recent file-change batches for history queries.
struct History {
    entries: VecDeque<(u64, Vec<FileChange>)>,
}

impl History {
    fn new() -> Self {
        Self {
            entries: VecDeque::with_capacity(HISTORY_CAPACITY),
        }
    }

    fn push(&mut self, timestamp: u64, changes: Vec<FileChange>) {
        if self.entries.len() >= HISTORY_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back((timestamp, changes));
    }

    fn since(&self, cutoff: u64) -> Vec<FileChange> {
        self.entries
            .iter()
            .filter(|(ts, _)| *ts > cutoff)
            .flat_map(|(_, changes)| changes.iter().cloned())
            .collect()
    }
}

/// State shared between the accept loop and broadcast callers.
struct Shared {
    clients: Vec<ClientConn>,
    history: History,
    status: String,
    daemon_status: Option<DaemonStatus>,
    last_rebuild_at: Option<u64>,
    files_count: usize,
    root: PathBuf,
}

struct ClientConn {
    stream: UnixStream,
}

impl ClientConn {
    fn send(&mut self, msg: &ServerMessage) -> bool {
        let Ok(mut line) = serde_json::to_string(msg) else {
            return false;
        };
        line.push('\n');
        self.stream.write_all(line.as_bytes()).is_ok() && self.stream.flush().is_ok()
    }
}

/// Daemon-side socket server.
///
/// Binds a Unix domain socket, accepts client connections, and broadcasts
/// file-change events and status updates to all connected clients.
pub struct DaemonServer {
    shared: Arc<Mutex<Shared>>,
    listener: UnixListener,
    socket_path: PathBuf,
    accept_handle: Option<std::thread::JoinHandle<()>>,
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl DaemonServer {
    /// Create and bind a new daemon socket server for the given watched root.
    ///
    /// The socket path is derived from the canonical root (see [`socket_path`]).
    /// Any existing socket file at the path is removed before binding.
    ///
    /// # Errors
    ///
    /// Returns an error if the parent directory cannot be created or the
    /// socket cannot be bound.
    pub fn new(root: &Path) -> Result<Self> {
        let sp = socket_path(root);
        ensure_socket_dir(&sp)?;

        if sp.exists() || sp.is_symlink() {
            let msg = if sp.is_symlink() {
                format!("symlink attack detected at {}", sp.display())
            } else {
                format!("socket file already exists at {}", sp.display())
            };
            return Err(DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                msg,
            )));
        }

        let listener = UnixListener::bind(&sp)?;

        let shared = Arc::new(Mutex::new(Shared {
            clients: Vec::new(),
            history: History::new(),
            status: "idle".to_string(),
            daemon_status: Some(DaemonStatus::Idle),
            last_rebuild_at: None,
            files_count: 0,
            root: root.to_path_buf(),
        }));
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));

        Ok(Self {
            shared,
            listener,
            socket_path: sp,
            accept_handle: None,
            running,
        })
    }

    /// Return the filesystem path of the bound socket.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.socket_path
    }

    /// Start the accept-and-read loop in a background thread.
    ///
    /// After calling `start()`, the server will accept new connections and
    /// respond to client queries automatically. Call [`DaemonServer::broadcast`]
    /// from the main loop to push events to all connected clients.
    ///
    /// # Errors
    ///
    /// Returns an error if the listener cannot be cloned, the accept thread
    /// cannot be spawned, or file descriptor operations fail.
    pub fn start(&mut self) -> Result<()> {
        let listener = self.listener.try_clone().map_err(DaemonSockError::Io)?;
        let shared = Arc::clone(&self.shared);
        let running = Arc::clone(&self.running);

        let handle = std::thread::Builder::new()
            .name("ixd-sock-accept".to_string())
            .spawn(move || {
                if let Err(e) = listener.set_nonblocking(true) {
                    tracing::error!("ixd: cannot set nonblocking: {e}");
                    return;
                }

                while running.load(std::sync::atomic::Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if let Err(e) = stream.set_nonblocking(false) {
                                tracing::warn!("ixd: cannot set blocking on client: {e}");
                                continue;
                            }
                            let _ =
                                stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
                            let read_stream = match stream.try_clone() {
                                Ok(s) => s,
                                Err(e) => {
                                    tracing::warn!("ixd: cannot clone stream: {e}");
                                    continue;
                                }
                            };
                            let shared_clone = Arc::clone(&shared);
                            let running_clone = Arc::clone(&running);
                            if let Err(e) = std::thread::Builder::new()
                                .name("ixd-sock-client".to_string())
                                .spawn(move || {
                                    client_read_loop(&read_stream, &shared_clone, &running_clone);
                                })
                            {
                                tracing::warn!("ixd: failed to spawn client thread: {e}");
                                continue;
                            }
                            let conn = ClientConn { stream };
                            if let Ok(mut s) = shared.lock() {
                                s.clients.push(conn);
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                        Err(e) => {
                            tracing::warn!("ixd: accept error: {e}");
                            std::thread::sleep(std::time::Duration::from_millis(200));
                        }
                    }
                }
            })
            .map_err(DaemonSockError::Io)?;

        self.accept_handle = Some(handle);
        Ok(())
    }

    /// Broadcast a server message to all connected clients.
    ///
    /// Disconnected clients are automatically removed. The message is
    /// serialized once and written to each client with a short write
    /// timeout to prevent a slow consumer from blocking the daemon.
    pub fn broadcast(&self, msg: &ServerMessage) {
        let Ok(mut s) = self.shared.lock() else {
            return;
        };
        s.clients.retain_mut(|c| c.send(msg));
    }

    /// Update the daemon status and file count (reflected in subsequent
    /// broadcasts and query responses).
    pub fn set_status(&self, daemon_status: &DaemonStatus, files_count: usize) {
        if let Ok(mut s) = self.shared.lock() {
            s.status = daemon_status.to_string();
            s.daemon_status = Some(daemon_status.clone());
            s.files_count = files_count;
        }
    }

    /// Record a file-change batch in the history buffer and broadcast it.
    pub fn notify_changes(&self, changes: Vec<FileChange>, files_count: usize) {
        let timestamp = now_secs();
        if let Ok(mut s) = self.shared.lock() {
            s.history.push(timestamp, changes.clone());
            s.files_count = files_count;
            if matches!(s.daemon_status, Some(DaemonStatus::Idle)) {
                s.status = "idle".to_string();
                s.daemon_status = Some(DaemonStatus::Idle);
                s.last_rebuild_at = Some(timestamp);
            }
            let msg = ServerMessage::FilesChanged {
                batch: changes,
                timestamp,
            };
            s.clients.retain_mut(|c| c.send(&msg));
        }
    }

    /// Broadcast graceful shutdown notice to all connected clients.
    ///
    /// Sends a `ServerMessage::Shutdown` to all clients, then waits for
    /// the specified delay to give clients time to finish in-flight operations.
    /// After the delay, the socket will be closed by the `Drop` implementation.
    ///
    /// # Arguments
    ///
    /// * `reason` - Human-readable reason for shutdown (e.g., "signal", "`user_request`")
    /// * `delay_ms` - Milliseconds to wait after broadcast before closing
    pub fn shutdown_notify(&self, reason: &str, delay_ms: u32) {
        // Broadcast shutdown notice to all connected clients
        self.broadcast(&ServerMessage::Shutdown(ShutdownNotice {
            reason: reason.to_string(),
            delay_ms,
        }));

        // Give clients time to finish in-flight operations
        std::thread::sleep(std::time::Duration::from_millis(u64::from(delay_ms)));
    }
}
fn client_read_loop(
    stream: &UnixStream,
    shared: &Arc<Mutex<Shared>>,
    running: &Arc<std::sync::atomic::AtomicBool>,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut reader = BufReader::new(stream);
    let mut line_buf = String::new();

    loop {
        if !running.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        line_buf.clear();
        match reader.read_line(&mut line_buf) {
            Ok(0) => break,
            Ok(_) => {
                let msg: ClientMessage = match serde_json::from_str(&line_buf) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::debug!("ixd: malformed client message: {e}");
                        continue;
                    }
                };

                let response = match msg {
                    ClientMessage::StatusQuery { id } => {
                        let Ok(s) = shared.lock() else {
                            tracing::warn!("ixd: shared lock poisoned in status query");
                            continue;
                        };
                        ServerMessage::QueryResult {
                            id,
                            status: s.status.clone(),
                            files: s.files_count,
                            changes_since: Vec::new(),
                            daemon_status: s.daemon_status.clone(),
                            last_rebuild_at: s.last_rebuild_at,
                        }
                    }
                    ClientMessage::HistoryQuery { since, id } => {
                        let Ok(s) = shared.lock() else {
                            tracing::warn!("ixd: shared lock poisoned in history query");
                            continue;
                        };
                        let changes = s.history.since(since);
                        ServerMessage::QueryResult {
                            id,
                            status: s.status.clone(),
                            files: s.files_count,
                            changes_since: changes,
                            daemon_status: s.daemon_status.clone(),
                            last_rebuild_at: s.last_rebuild_at,
                        }
                    }
                    ClientMessage::SearchQuery(query) => {
                        let root = {
                            let Ok(s) = shared.lock() else {
                                tracing::warn!("ixd: shared lock poisoned in search query");
                                continue;
                            };
                            s.root.clone()
                        };
                        match execute_search(&root, &query) {
                            Ok(results) => ServerMessage::SearchResults(results),
                            Err(e) => {
                                tracing::warn!("ixd: search failed: {e}");
                                ServerMessage::SearchResults(SearchResults {
                                    id: query.id,
                                    matches: vec![],
                                    stats: crate::executor::QueryStats::default(),
                                })
                            }
                        }
                    }
                    ClientMessage::Shutdown { ack } => {
                        // Client acknowledges shutdown notice
                        // Log for diagnostics, no response needed
                        tracing::debug!(
                            "ixd: client shutdown ack={}",
                            if ack { "true" } else { "false" }
                        );
                        // Continue loop - server will close connection after delay
                        continue;
                    }
                };

                if let Ok(mut write_stream) = stream.try_clone() {
                    match serde_json::to_string(&response) {
                        Ok(mut line) => {
                            line.push('\n');
                            if write_stream.write_all(line.as_bytes()).is_err()
                                || write_stream.flush().is_err()
                            {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("ixd: failed to serialize query response: {e}");
                            break;
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
}

/// Client-side connection to an ixd daemon socket.
pub struct DaemonClient {
    stream: BufReader<UnixStream>,
}

impl DaemonClient {
    /// Connect to the daemon socket for the given watched root.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket does not exist or the connection fails.
    pub fn connect(root: &Path) -> Result<Self> {
        let sp = socket_path(root);
        let stream = UnixStream::connect(&sp)?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
        Ok(Self {
            stream: BufReader::new(stream),
        })
    }

    /// Receive the next message from the daemon (blocking).
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure, timeout (5s), or malformed JSON.
    pub fn recv(&mut self) -> Result<ServerMessage> {
        let mut line = String::new();
        let bytes = self.stream.read_line(&mut line).map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut {
                DaemonSockError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "recv timed out after 5s",
                ))
            } else {
                DaemonSockError::Io(e)
            }
        })?;
        if bytes == 0 {
            return Err(DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "daemon closed connection",
            )));
        }
        let msg: ServerMessage = serde_json::from_str(line.trim_end()).map_err(|e| {
            DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid JSON: {e}"),
            ))
        })?;
        Ok(msg)
    }

    /// Send a query message to the daemon.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure or serialization error.
    pub fn send(&mut self, msg: &ClientMessage) -> Result<()> {
        let stream = self.stream.get_mut();
        let mut line = serde_json::to_string(msg)?;
        line.push('\n');
        stream.write_all(line.as_bytes())?;
        stream.flush()?;
        Ok(())
    }

    /// Execute a search query and return results.
    /// This sends a `SearchQuery` message and waits for the `SearchResults` response.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure, timeout, or if the response is not a `SearchResults`.
    pub fn search(&mut self, query: SearchQuery) -> Result<SearchResults> {
        use std::io::Write;

        let stream = self.stream.get_mut();
        stream.set_write_timeout(Some(std::time::Duration::from_secs(3)))?;

        // Send query
        let mut line = serde_json::to_string(&ClientMessage::SearchQuery(query))?;
        line.push('\n');
        stream.write_all(line.as_bytes())?;
        stream.flush()?;

        // Read response
        stream.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
        let mut response_line = String::new();
        self.stream.read_line(&mut response_line)?;

        match serde_json::from_str::<ServerMessage>(response_line.trim_end()) {
            Ok(ServerMessage::SearchResults(results)) => Ok(results),
            Ok(other) => Err(DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("expected SearchResults, got {other:?}"),
            ))),
            Err(e) => Err(DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid JSON: {e}"),
            ))),
        }
    }
}

/// Current Unix timestamp in seconds.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Execute a search query against the index at the given root path.
/// This reads directly from the shard.ix file, ensuring consistency with
/// the daemon's current state (since the daemon rebuilds the index on each change).
///
/// # Errors
///
/// Returns an error if the index file cannot be read, the index format is
/// corrupt, or the query cannot be executed.
pub fn execute_search(
    root: &Path,
    query: &SearchQuery,
) -> std::result::Result<SearchResults, Box<dyn std::error::Error + Send + Sync>> {
    use crate::executor::{Executor, QueryOptions};
    use crate::planner::Planner;
    use crate::reader::Reader;

    // Find the index file
    let index_dir = root.join(".ix");
    let index_path = index_dir.join("shard.ix");

    if !index_path.exists() {
        return Err("index not found".into());
    }

    let reader = Reader::open(&index_path)?;
    let mut executor = Executor::new(&reader);

    // Set up delta file path
    let delta_path = index_dir.join("shard.ix.delta");
    executor.set_delta_path(delta_path);

    let plan = Planner::plan_with_pool(
        &query.pattern,
        crate::planner::QueryOptions {
            is_regex: query.is_regex,
            ignore_case: query.ignore_case,
            multiline: query.multiline,
            word_boundary: query.word_boundary,
        },
        executor.regex_pool(),
    );

    let options = QueryOptions {
        count_only: false,
        files_only: false,
        max_results: query.max_results,
        type_filter: query.file_types.clone(),
        context_lines: query.context_lines,
        decompress: query.decompress,
        threads: 0,
        multiline: query.multiline,
        archive: query.archive,
        binary: query.binary,
        word_boundary: query.word_boundary,
    };

    let (matches, stats) = executor.execute(&plan, &options)?;
    Ok(SearchResults {
        id: query.id,
        matches,
        stats,
    })
}

impl Drop for DaemonServer {
    fn drop(&mut self) {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.accept_handle.take() {
            let _ = handle.join();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn socket_path_deterministic() {
        let root = PathBuf::from("/tmp/test-project");
        let p1 = socket_path(&root);
        let p2 = socket_path(&root);
        assert_eq!(p1, p2, "same root must produce same socket path");
    }

    #[test]
    fn socket_path_different_roots() {
        let r1 = PathBuf::from("/tmp/project-a");
        let r2 = PathBuf::from("/tmp/project-b");
        assert_ne!(socket_path(&r1), socket_path(&r2));
    }

    #[test]
    fn socket_path_uses_xdg() {
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/tmp/xdg-test-runtime") };
        let p = socket_path(Path::new("/tmp/some-project"));
        assert!(p.starts_with("/tmp/xdg-test-runtime/ixd/"));
        assert!(p.extension().is_some_and(|e| e == "sock"));
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
    }

    #[test]
    fn server_message_ndjson_roundtrip() {
        let msg = ServerMessage::Status {
            pid: 1234,
            status: "idle".to_string(),
            files: 42,
            daemon_status: None,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        assert!(json.contains("\"t\":\"status\""), "tag field present");
        assert!(
            !json.contains("daemon_status"),
            "daemon_status should be omitted when None"
        );

        let back: ServerMessage = serde_json::from_str(&json).expect("deserialize");
        if let ServerMessage::Status {
            pid,
            status,
            files,
            daemon_status,
        } = back
        {
            assert_eq!(pid, 1234);
            assert_eq!(status, "idle");
            assert_eq!(files, 42);
            assert_eq!(daemon_status, None);
        } else {
            panic!("wrong variant after roundtrip");
        }
    }

    #[test]
    fn files_changed_roundtrip() {
        let msg = ServerMessage::FilesChanged {
            batch: vec![FileChange {
                path: PathBuf::from("src/main.rs"),
                mtime: 1_776_468_629,
                op: FileOp::Modify,
            }],
            timestamp: 1_776_468_629,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let back: ServerMessage = serde_json::from_str(&json).expect("deserialize");
        if let ServerMessage::FilesChanged { batch, timestamp } = back {
            assert_eq!(batch.len(), 1);
            assert_eq!(batch[0].path, PathBuf::from("src/main.rs"));
            assert_eq!(timestamp, 1_776_468_629);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn client_message_roundtrip() {
        let msg = ClientMessage::HistoryQuery { since: 1000, id: 7 };
        let json = serde_json::to_string(&msg).expect("serialize");
        let back: ClientMessage = serde_json::from_str(&json).expect("deserialize");
        if let ClientMessage::HistoryQuery { since, id } = back {
            assert_eq!(since, 1000);
            assert_eq!(id, 7);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn history_since() {
        let mut h = History::new();
        h.push(
            100,
            vec![FileChange {
                path: PathBuf::from("a.rs"),
                mtime: 100,
                op: FileOp::Create,
            }],
        );
        h.push(
            200,
            vec![FileChange {
                path: PathBuf::from("b.rs"),
                mtime: 200,
                op: FileOp::Modify,
            }],
        );
        h.push(
            300,
            vec![FileChange {
                path: PathBuf::from("c.rs"),
                mtime: 300,
                op: FileOp::Delete,
            }],
        );

        let changes = h.since(150);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, PathBuf::from("b.rs"));
        assert_eq!(changes[1].path, PathBuf::from("c.rs"));
    }

    #[test]
    fn history_capacity() {
        let mut h = History::new();
        for i in 0..=HISTORY_CAPACITY {
            h.push(
                i as u64,
                vec![FileChange {
                    path: PathBuf::from(format!("f{i}")),
                    mtime: i as u64,
                    op: FileOp::Modify,
                }],
            );
        }
        assert_eq!(h.entries.len(), HISTORY_CAPACITY);
        // Oldest entry (ts=0) should have been evicted
        assert_eq!(h.entries.front().expect("non-empty").0, 1);
    }

    #[test]
    fn server_client_connect_and_broadcast() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();

        let mut server = DaemonServer::new(&root).expect("create server");
        let sp = server.path().to_path_buf();
        let _ = server.start();

        // Connect a client
        let stream = UnixStream::connect(&sp).expect("connect");
        let mut client = DaemonClient {
            stream: BufReader::new(stream),
        };

        // Give the accept thread time to register the client
        std::thread::sleep(std::time::Duration::from_millis(200));

        server.set_status(&DaemonStatus::Idle, 10);

        // Broadcast a status message
        server.broadcast(&ServerMessage::Status {
            pid: 1234,
            status: "idle".to_string(),
            files: 10,
            daemon_status: Some(DaemonStatus::Idle),
        });

        // Client should receive the message
        // Use a timeout to avoid hanging forever
        client
            .stream
            .get_mut()
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("set timeout");

        match client.recv() {
            Ok(ServerMessage::Status {
                pid,
                status,
                files,
                daemon_status,
            }) => {
                assert_eq!(pid, 1234);
                assert_eq!(status, "idle");
                assert_eq!(files, 10);
                assert!(daemon_status.is_some());
            }
            Ok(other) => panic!("expected Status, got {other:?}"),
            Err(e) => panic!("recv failed: {e}"),
        }
    }

    #[test]
    fn client_query_status() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();

        let mut server = DaemonServer::new(&root).expect("create server");
        let sp = server.path().to_path_buf();
        let _ = server.start();
        server.set_status(&DaemonStatus::Indexing { entropy: 42 }, 99);

        let stream = UnixStream::connect(&sp).expect("connect");
        let mut client = DaemonClient {
            stream: BufReader::new(stream),
        };

        std::thread::sleep(std::time::Duration::from_millis(200));

        client
            .send(&ClientMessage::StatusQuery { id: 123 })
            .expect("send query");

        client
            .stream
            .get_mut()
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("set timeout");

        match client.recv() {
            Ok(ServerMessage::QueryResult {
                id,
                status,
                files,
                changes_since,
                daemon_status,
                last_rebuild_at,
            }) => {
                eprintln!(
                    "[JSON] id={}, status={}, files={}, daemon_status={:?}, last_rebuild_at={:?}",
                    id, status, files, daemon_status, last_rebuild_at
                );
                assert_eq!(id, 123);
                assert_eq!(status, "indexing (entropy: 42)");
                assert_eq!(files, 99);
                assert!(changes_since.is_empty());
                assert_eq!(daemon_status, Some(DaemonStatus::Indexing { entropy: 42 }));
                assert_eq!(last_rebuild_at, None);
            }
            Ok(other) => panic!("expected QueryResult, got {other:?}"),
            Err(e) => panic!("recv failed: {e}"),
        }
    }
}

#[test]
fn test_shutdown_protocol() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let mut server = DaemonServer::new(&root).expect("create server");
    let _ = server.start();

    // Test shutdown notification
    server.shutdown_notify("test_signal", 100);

    // Give it time to broadcast
    std::thread::sleep(std::time::Duration::from_millis(150));

    // Server should still be functional after shutdown notify
    server.set_status(&DaemonStatus::Idle, 0);
}

#[test]
fn test_client_shutdown_ack() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let mut server = DaemonServer::new(&root).expect("create server");
    let sp = server.path().to_path_buf();
    let _ = server.start();

    let stream = UnixStream::connect(&sp).expect("connect");
    let mut client = DaemonClient {
        stream: BufReader::new(stream),
    };

    // Client sends shutdown acknowledgment
    client
        .send(&ClientMessage::Shutdown { ack: true })
        .expect("send shutdown ack");

    // Give server time to process
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Server should still be functional (shutdown ack is fire-and-forget)
    server.set_status(&DaemonStatus::Idle, 0);
}
