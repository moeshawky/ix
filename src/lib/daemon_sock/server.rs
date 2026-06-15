use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::resolve::{ensure_socket_dir, socket_path};
use super::search::{SearchCaches, execute_search_inner, execute_search_progressive_inner};
use super::types::{
    ClientMessage, DaemonSockError, DaemonStatus, FileChange, Result, SearchResults, ServerMessage,
    ShutdownNotice, now_secs,
};

/// Maximum number of change batches retained for history queries.
const HISTORY_CAPACITY: usize = 1024;

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
    /// Max concurrent progressive search queries (backpressure).
    search_slots: Arc<SearchSlots>,
    /// Cache handles shared across all daemon-mode search queries.
    search_caches: Option<SearchCaches>,
}

/// Simple permit counter for limiting concurrent progressive searches.
struct SearchSlots {
    max: u32,
    available: std::sync::Mutex<u32>,
}

impl SearchSlots {
    fn new(max: u32) -> Self {
        Self {
            max,
            available: std::sync::Mutex::new(max),
        }
    }

    /// Try to acquire a slot. Returns `Some(SearchSlot)` on success,
    /// `None` if all slots are full. The slot is released when the
    /// guard is dropped.
    fn try_acquire(self: &Arc<Self>) -> Option<SearchSlot> {
        let mut count = self.available.lock().ok()?;
        if *count > 0 {
            *count -= 1;
            Some(SearchSlot {
                slots: Arc::clone(self),
            })
        } else {
            None
        }
    }
}

/// RAII guard: releases a search slot on drop.
struct SearchSlot {
    slots: Arc<SearchSlots>,
}

impl Drop for SearchSlot {
    fn drop(&mut self) {
        if let Ok(mut count) = self.slots.available.lock() {
            *count = (*count + 1).min(self.slots.max);
        }
    }
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
    pub fn new(root: &std::path::Path) -> Result<Self> {
        let sp = socket_path(root);
        ensure_socket_dir(&sp)?;

        if sp.is_symlink() {
            return Err(DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("symlink attack detected at {}", sp.display()),
            )));
        }

        if sp.exists() {
            std::fs::remove_file(&sp)?;
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
            search_slots: Arc::new(SearchSlots::new(4)),
            search_caches: None,
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
    pub fn path(&self) -> &std::path::Path {
        &self.socket_path
    }

    /// Set the cache handles used by daemon-mode search queries.
    ///
    /// Must be called before search queries arrive. All subsequent
    /// progressive and non-progressive searches will use the supplied
    /// posting cache, negative cache, and regex pool.
    pub(crate) fn set_caches(&mut self, caches: SearchCaches) {
        if let Ok(mut s) = self.shared.lock() {
            s.search_caches = Some(caches);
        }
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
                            if let Err(e) =
                                stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))
                            {
                                tracing::debug!("ixd: client write timeout setup failed: {e}");
                            }
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
    if let Err(e) = stream.set_read_timeout(Some(std::time::Duration::from_secs(5))) {
        tracing::debug!("ixd: client read timeout setup failed: {e}");
    }
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
                        let (root, caches) = {
                            let Ok(s) = shared.lock() else {
                                tracing::warn!("ixd: shared lock poisoned in search query");
                                continue;
                            };
                            (s.root.clone(), s.search_caches.clone())
                        };

                        if query.progressive {
                            // Progressive mode: stream results as they arrive.
                            // Spawn a background thread to execute the search
                            // and forward batches through a channel.
                            // A slot-based permit system (max 4 concurrent)
                            // provides backpressure to prevent thread explosion.
                            let (result_sender, result_receiver) =
                                std::sync::mpsc::channel::<SearchResults>();
                            let root_clone = root.clone();
                            let slot = {
                                let Ok(s) = shared.lock() else {
                                    tracing::warn!("ixd: shared lock poisoned");
                                    continue;
                                };
                                let Some(slot) = s.search_slots.try_acquire() else {
                                    tracing::warn!(
                                        "ixd: too many concurrent progressive searches, rejecting"
                                    );
                                    // Send error response so the client knows.
                                    if let Ok(mut ws) = stream.try_clone() {
                                        if let Ok(mut line) = serde_json::to_string(
                                            &ServerMessage::SearchResults(SearchResults {
                                                id: query.id,
                                                matches: vec![],
                                                stats: crate::executor::QueryStats::default(),
                                                error: Some("server busy".into()),
                                                done: true,
                                                batch: 0,
                                            }),
                                        ) {
                                            line.push('\n');
                                            if let Err(e) = ws.write_all(line.as_bytes()) {
                                                tracing::warn!("daemon: client write failed: {e}");
                                            }
                                            if let Err(e) = ws.flush() {
                                                tracing::warn!("daemon: client flush failed: {e}");
                                            }
                                        }
                                    }
                                    continue;
                                };
                                slot
                            };
                            let _ = std::thread::Builder::new()
                                .name("ixd-search-prog".to_string())
                                .spawn(move || {
                                    // Hold the slot guard for the duration of the
                                    // search. Released when this closure ends.
                                    let _held = slot;
                                    if let Err(e) = execute_search_progressive_inner(
                                        &root_clone,
                                        &query,
                                        &result_sender,
                                        caches.as_ref(),
                                    ) {
                                    tracing::warn!("ixd: progressive search failed: {e}");
                                    if result_sender.send(SearchResults {
                                        id: query.id,
                                        matches: vec![],
                                        stats: crate::executor::QueryStats::default(),
                                        error: Some(e.to_string()),
                                        done: true,
                                        batch: 0,
                                    }).is_err() {
                                        tracing::debug!(
                                            "progressive search: receiver closed (client disconnected)"
                                        );
                                    }
                                    }
                                    // Ensure result_sender is dropped so the
                                    // receiver loop below terminates.
                                    drop(result_sender);
                                });
                            // Write each batch as a separate NDJSON line.
                            while let Ok(batch) = result_receiver.recv() {
                                if let Ok(mut write_stream) = stream.try_clone() {
                                    match serde_json::to_string(&ServerMessage::SearchResults(
                                        batch,
                                    )) {
                                        Ok(mut line) => {
                                            line.push('\n');
                                            if write_stream.write_all(line.as_bytes()).is_err()
                                                || write_stream.flush().is_err()
                                            {
                                                tracing::warn!(
                                                    "ixd: client write failed for progressive batch"
                                                );
                                                break;
                                            }
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                "ixd: failed to serialize progressive batch: {e}"
                                            );
                                            break;
                                        }
                                    }
                                }
                            }
                            continue;
                        }

                        // Non-progressive: single response path
                        execute_search_inner(&root, &query, caches.as_ref()).map_or_else(
                            |e| {
                                tracing::warn!("ixd: search failed: {e}");
                                ServerMessage::SearchResults(SearchResults {
                                    id: query.id,
                                    matches: vec![],
                                    stats: crate::executor::QueryStats::default(),
                                    error: Some(e.to_string()),
                                    done: true,
                                    batch: 0,
                                })
                            },
                            ServerMessage::SearchResults,
                        )
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
                                tracing::warn!("ixd: client write failed for query response");
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
    use super::super::types::FileOp;
    use super::*;

    #[test]
    fn history_since() {
        let mut h = History::new();
        h.push(
            100,
            vec![FileChange {
                path: std::path::PathBuf::from("a.rs"),
                mtime: 100,
                op: FileOp::Create,
            }],
        );
        h.push(
            200,
            vec![FileChange {
                path: std::path::PathBuf::from("b.rs"),
                mtime: 200,
                op: FileOp::Modify,
            }],
        );
        h.push(
            300,
            vec![FileChange {
                path: std::path::PathBuf::from("c.rs"),
                mtime: 300,
                op: FileOp::Delete,
            }],
        );

        let changes = h.since(150);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, std::path::PathBuf::from("b.rs"));
        assert_eq!(changes[1].path, std::path::PathBuf::from("c.rs"));
    }

    #[test]
    fn history_capacity() {
        let mut h = History::new();
        for i in 0..=HISTORY_CAPACITY {
            h.push(
                i as u64,
                vec![FileChange {
                    path: std::path::PathBuf::from(format!("f{i}")),
                    mtime: i as u64,
                    op: FileOp::Modify,
                }],
            );
        }
        assert_eq!(h.entries.len(), HISTORY_CAPACITY);
        // Oldest entry (ts=0) should have been evicted
        assert_eq!(h.entries.front().expect("non-empty").0, 1);
    }
}
