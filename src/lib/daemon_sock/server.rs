use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use super::resolve::{ensure_socket_dir, socket_path};
use super::search::{SearchCaches, execute_search_inner, execute_search_progressive_inner};
use super::types::{
    ClientMessage, DaemonSockError, DaemonStatus, FileChange, Result, SearchResults, ServerMessage,
    ShutdownNotice, now_secs,
};
use crate::idle::IdleTracker;

/// Maximum number of change batches retained for history queries.
const HISTORY_CAPACITY: usize = 1024;

/// Maximum incoming NDJSON request line accepted from an IPC client (1 `MiB`).
const MAX_IPC_LINE_BYTES: usize = 1024 * 1024;

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
    clients: Vec<ClientHandle>,
    next_client_id: u64,
    client_threads: Vec<JoinHandle<()>>,
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
    /// Shared idle tracker — reset on every search query so the daemon
    /// stays Active during query traffic.
    idle_tracker: Option<Arc<Mutex<IdleTracker>>>,
    /// Optional shared reader/writer lock coordinating index compaction
    /// (`write`) against concurrent IPC search queries (`read`).
    index_lock: Option<Arc<RwLock<()>>>,
}

fn lock_shared(shared: &Arc<Mutex<Shared>>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|e| {
        tracing::warn!("ixd: shared state mutex poisoned \u{2014} recovering");
        e.into_inner()
    })
}

/// Simple permit counter for limiting concurrent progressive searches.
struct SearchSlots {
    max: u32,
    available: Mutex<u32>,
}

impl SearchSlots {
    const fn new(max: u32) -> Self {
        Self {
            max,
            available: Mutex::new(max),
        }
    }

    /// Try to acquire a slot. Returns `Some(SearchSlot)` on success,
    /// `None` if all slots are full. The slot is released when the
    /// guard is dropped.
    fn try_acquire(self: &Arc<Self>) -> Option<SearchSlot> {
        let mut count = self.available.lock().unwrap_or_else(|e| {
            tracing::warn!("ixd: search_slots mutex poisoned \u{2014} recovering");
            e.into_inner()
        });
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
        let mut count = self.slots.available.lock().unwrap_or_else(|e| {
            tracing::warn!("ixd: search_slots mutex poisoned on drop \u{2014} recovering");
            e.into_inner()
        });
        *count = (*count + 1).min(self.slots.max);
    }
}

#[derive(Clone)]
struct ClientHandle {
    id: u64,
    stream: Arc<Mutex<UnixStream>>,
}

fn send_ndjson_line(client: &ClientHandle, line: &str) -> bool {
    let Ok(mut stream) = client.stream.lock() else {
        tracing::warn!(
            "ixd: client {} write mutex poisoned \u{2014} disconnecting",
            client.id
        );
        return false;
    };
    stream.write_all(line.as_bytes()).is_ok() && stream.flush().is_ok()
}

fn send_server_message(client: &ClientHandle, msg: &ServerMessage) -> bool {
    let Ok(mut line) = serde_json::to_string(msg) else {
        tracing::warn!("ixd: failed to serialize server message");
        return false;
    };
    line.push('\n');
    send_ndjson_line(client, &line)
}

/// RAII guard removing a client connection entry from `Shared` when its
/// reader loop exits.
struct ClientCleanupGuard {
    id: u64,
    shared: Arc<Mutex<Shared>>,
}

impl Drop for ClientCleanupGuard {
    fn drop(&mut self) {
        let mut s = lock_shared(&self.shared);
        s.clients.retain(|c| c.id != self.id);
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
    accept_handle: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
}

impl DaemonServer {
    /// Create and bind a new daemon socket server for the given watched root.
    ///
    /// The socket path is derived from the canonical root (see [`socket_path`]).
    /// Refuses to clobber a live listening socket or symlink; stale dead socket
    /// files are unlinked before binding and restricted to `0o600`.
    ///
    /// # Errors
    ///
    /// Returns an error if the parent directory cannot be created, a live
    /// server already listens on the path, or the socket cannot be bound.
    pub fn new(root: &std::path::Path) -> Result<Self> {
        let sp = socket_path(root);
        ensure_socket_dir(&sp)?;

        if sp.is_symlink() {
            return Err(DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("symlink attack detected at {}", sp.display()),
            )));
        }

        if std::fs::symlink_metadata(&sp).is_ok() {
            match UnixStream::connect(&sp) {
                Ok(_) => {
                    return Err(DaemonSockError::Io(std::io::Error::new(
                        std::io::ErrorKind::AddrInUse,
                        format!("socket already in use at {}", sp.display()),
                    )));
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) =>
                {
                    match std::fs::remove_file(&sp) {
                        Ok(()) => {}
                        Err(rem_err) if rem_err.kind() == std::io::ErrorKind::NotFound => {}
                        Err(rem_err) => return Err(DaemonSockError::Io(rem_err)),
                    }
                }
                Err(e) => return Err(DaemonSockError::Io(e)),
            }
        }

        let listener = UnixListener::bind(&sp)?;
        std::fs::set_permissions(&sp, std::fs::Permissions::from_mode(0o600))?;

        let shared = Arc::new(Mutex::new(Shared {
            clients: Vec::new(),
            next_client_id: 1,
            client_threads: Vec::new(),
            history: History::new(),
            status: "idle".to_string(),
            daemon_status: Some(DaemonStatus::Idle),
            last_rebuild_at: None,
            files_count: 0,
            root: root.to_path_buf(),
            search_slots: Arc::new(SearchSlots::new(4)),
            search_caches: None,
            idle_tracker: None,
            index_lock: None,
        }));
        let running = Arc::new(AtomicBool::new(true));

        Ok(Self {
            shared,
            listener,
            socket_path: sp,
            accept_handle: None,
            running,
        })
    }

    /// Create and bind a new daemon socket server configured with a shared
    /// reader/writer lock coordinating index compaction against search queries.
    ///
    /// # Errors
    ///
    /// Returns an error under the same conditions as [`DaemonServer::new`].
    pub fn new_with_search_lock(
        root: &std::path::Path,
        index_lock: Arc<RwLock<()>>,
    ) -> Result<Self> {
        let mut server = Self::new(root)?;
        server.set_index_lock(index_lock);
        Ok(server)
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
        let mut s = lock_shared(&self.shared);
        s.search_caches = Some(caches);
    }

    /// Set the idle tracker used to reset the daemon's activity state from
    /// query activity arriving over the socket.
    ///
    /// Must be called before search queries arrive. A search query received
    /// before this is set does not reset the idle timer, so the daemon may
    /// report itself as `Idle` or `Dormant` (and run idle cleanup) while it
    /// is still actively serving queries.
    pub(crate) fn set_idle_tracker(&mut self, tracker: Arc<Mutex<IdleTracker>>) {
        let mut s = lock_shared(&self.shared);
        s.idle_tracker = Some(tracker);
    }

    /// Attach the shared index reader/writer lock used to synchronize
    /// concurrent IPC searches against daemon builds and compactions.
    pub fn set_index_lock(&mut self, index_lock: Arc<RwLock<()>>) {
        let mut s = lock_shared(&self.shared);
        s.index_lock = Some(index_lock);
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
            .spawn(move || accept_loop(&listener, &shared, &running))
            .map_err(DaemonSockError::Io)?;

        self.accept_handle = Some(handle);
        Ok(())
    }

    /// Broadcast a server message to all connected clients.
    ///
    /// Disconnected clients are automatically removed. The message is
    /// serialized once and written to each client outside the shared state
    /// mutex so a slow consumer never blocks other callers.
    pub fn broadcast(&self, msg: &ServerMessage) {
        let Ok(mut line) = serde_json::to_string(msg) else {
            return;
        };
        line.push('\n');

        let clients = {
            let s = lock_shared(&self.shared);
            s.clients.clone()
        };
        if clients.is_empty() {
            return;
        }

        let mut dead_ids = Vec::new();
        for client in &clients {
            if !send_ndjson_line(client, &line) {
                dead_ids.push(client.id);
            }
        }
        if !dead_ids.is_empty() {
            let mut s = lock_shared(&self.shared);
            s.clients.retain(|c| !dead_ids.contains(&c.id));
        }
    }

    /// Update the daemon status and file count (reflected in subsequent
    /// broadcasts and query responses).
    pub fn set_status(&self, daemon_status: &DaemonStatus, files_count: usize) {
        let mut s = lock_shared(&self.shared);
        s.status = daemon_status.to_string();
        s.daemon_status = Some(daemon_status.clone());
        s.files_count = files_count;
    }

    /// Record a file-change batch in the history buffer and broadcast it.
    pub fn notify_changes(&self, changes: Vec<FileChange>, files_count: usize) {
        let timestamp = now_secs();
        let (clients, msg) = {
            let mut s = lock_shared(&self.shared);
            s.history.push(timestamp, changes.clone());
            s.files_count = files_count;
            s.last_rebuild_at = Some(timestamp);
            let msg = ServerMessage::FilesChanged {
                batch: changes,
                timestamp,
            };
            (s.clients.clone(), msg)
        };

        if clients.is_empty() {
            return;
        }
        let Ok(mut line) = serde_json::to_string(&msg) else {
            return;
        };
        line.push('\n');

        let mut dead_ids = Vec::new();
        for client in &clients {
            if !send_ndjson_line(client, &line) {
                dead_ids.push(client.id);
            }
        }
        if !dead_ids.is_empty() {
            let mut s = lock_shared(&self.shared);
            s.clients.retain(|c| !dead_ids.contains(&c.id));
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
        self.broadcast(&ServerMessage::Shutdown(ShutdownNotice {
            reason: reason.to_string(),
            delay_ms,
        }));

        std::thread::sleep(Duration::from_millis(u64::from(delay_ms)));
    }
}

fn accept_loop(listener: &UnixListener, shared: &Arc<Mutex<Shared>>, running: &Arc<AtomicBool>) {
    if let Err(e) = listener.set_nonblocking(true) {
        tracing::error!("ixd: cannot set nonblocking: {e}");
        return;
    }

    while running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(e) = stream.set_nonblocking(false) {
                    tracing::warn!("ixd: cannot set blocking on client: {e}");
                    continue;
                }
                if let Err(e) = stream.set_write_timeout(Some(Duration::from_secs(5))) {
                    tracing::debug!("ixd: client write timeout setup failed: {e}");
                }
                let read_stream = match stream.try_clone() {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("ixd: cannot clone stream: {e}");
                        continue;
                    }
                };
                let client_handle = {
                    let mut s = lock_shared(shared);
                    s.client_threads.retain(|h| !h.is_finished());
                    let id = s.next_client_id;
                    s.next_client_id = s.next_client_id.saturating_add(1);
                    let handle = ClientHandle {
                        id,
                        stream: Arc::new(Mutex::new(stream)),
                    };
                    s.clients.push(handle.clone());
                    handle
                };
                let client_id = client_handle.id;
                let shared_clone = Arc::clone(shared);
                let running_clone = Arc::clone(running);
                match std::thread::Builder::new()
                    .name("ixd-sock-client".to_string())
                    .spawn(move || {
                        client_read_loop(
                            &client_handle,
                            &read_stream,
                            &shared_clone,
                            &running_clone,
                        );
                    }) {
                    Ok(join_handle) => {
                        let mut s = lock_shared(shared);
                        s.client_threads.push(join_handle);
                    }
                    Err(e) => {
                        tracing::warn!("ixd: failed to spawn client thread: {e}");
                        let mut s = lock_shared(shared);
                        s.clients.retain(|c| c.id != client_id);
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                tracing::warn!("ixd: accept error: {e}");
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

enum BoundedLineResult {
    Line(String),
    Eof,
    Timeout,
    TooLong,
    Error,
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max_bytes: usize,
) -> BoundedLineResult {
    buf.clear();
    let limit = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    match reader.take(limit).read_until(b'\n', buf) {
        Ok(0) => BoundedLineResult::Eof,
        Ok(_) => {
            if buf.len() > max_bytes {
                return BoundedLineResult::TooLong;
            }
            match std::str::from_utf8(buf) {
                Ok(s) => BoundedLineResult::Line(s.to_string()),
                Err(e) => {
                    tracing::debug!("ixd: invalid UTF-8 in client line: {e}");
                    BoundedLineResult::Error
                }
            }
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            BoundedLineResult::Timeout
        }
        Err(_) => BoundedLineResult::Error,
    }
}

fn client_read_loop(
    client_handle: &ClientHandle,
    stream: &UnixStream,
    shared: &Arc<Mutex<Shared>>,
    running: &Arc<AtomicBool>,
) {
    let _cleanup = ClientCleanupGuard {
        id: client_handle.id,
        shared: Arc::clone(shared),
    };
    if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(5))) {
        tracing::debug!("ixd: client read timeout setup failed: {e}");
    }
    let mut reader = BufReader::new(stream);
    let mut byte_buf = Vec::new();

    loop {
        if !running.load(Ordering::SeqCst) {
            break;
        }
        let line = match read_bounded_line(&mut reader, &mut byte_buf, MAX_IPC_LINE_BYTES) {
            BoundedLineResult::Line(s) => s,
            BoundedLineResult::Timeout => continue,
            BoundedLineResult::TooLong => {
                tracing::warn!(
                    "ixd: disconnecting client {} after oversized IPC frame (> {MAX_IPC_LINE_BYTES} bytes)",
                    client_handle.id
                );
                break;
            }
            BoundedLineResult::Eof | BoundedLineResult::Error => break,
        };

        let msg: ClientMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!("ixd: malformed client message: {e}");
                continue;
            }
        };

        let Some(response) = dispatch_client_message(msg, client_handle, shared) else {
            continue;
        };

        if !send_server_message(client_handle, &response) {
            tracing::warn!("ixd: client write failed for query response");
            break;
        }
    }
}

fn dispatch_client_message(
    msg: ClientMessage,
    client_handle: &ClientHandle,
    shared: &Arc<Mutex<Shared>>,
) -> Option<ServerMessage> {
    match msg {
        ClientMessage::StatusQuery { id } => {
            let s = lock_shared(shared);
            Some(ServerMessage::QueryResult {
                id,
                status: s.status.clone(),
                files: s.files_count,
                changes_since: Vec::new(),
                daemon_status: s.daemon_status.clone(),
                last_rebuild_at: s.last_rebuild_at,
            })
        }
        ClientMessage::HistoryQuery { since, id } => {
            let s = lock_shared(shared);
            let changes = s.history.since(since);
            Some(ServerMessage::QueryResult {
                id,
                status: s.status.clone(),
                files: s.files_count,
                changes_since: changes,
                daemon_status: s.daemon_status.clone(),
                last_rebuild_at: s.last_rebuild_at,
            })
        }
        ClientMessage::SearchQuery(query) => {
            let (root, caches, idle, index_lock) = {
                let s = lock_shared(shared);
                (
                    s.root.clone(),
                    s.search_caches.clone(),
                    s.idle_tracker.clone(),
                    s.index_lock.clone(),
                )
            };
            if let Some(tracker) = idle {
                let mut t = tracker.lock().unwrap_or_else(|e| {
                    tracing::warn!("ixd: idle tracker lock poisoned in search query");
                    e.into_inner()
                });
                t.record_query();
            }

            if query.progressive {
                handle_progressive_query(client_handle, shared, &root, query, caches, index_lock);
                return None;
            }

            let _read_guard = index_lock
                .as_ref()
                .map(|lk| lk.read().unwrap_or_else(PoisonError::into_inner));
            Some(
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
                ),
            )
        }
        ClientMessage::Shutdown { ack } => {
            tracing::debug!(
                "ixd: client shutdown ack={}",
                if ack { "true" } else { "false" }
            );
            None
        }
    }
}

fn handle_progressive_query(
    client_handle: &ClientHandle,
    shared: &Arc<Mutex<Shared>>,
    root: &std::path::Path,
    query: super::types::SearchQuery,
    caches: Option<SearchCaches>,
    index_lock: Option<Arc<RwLock<()>>>,
) {
    let slot = {
        let s = lock_shared(shared);
        s.search_slots.try_acquire()
    };
    let Some(slot) = slot else {
        tracing::warn!("ixd: too many concurrent progressive searches, rejecting");
        let busy_msg = ServerMessage::SearchResults(SearchResults {
            id: query.id,
            matches: vec![],
            stats: crate::executor::QueryStats::default(),
            error: Some("server busy".into()),
            done: true,
            batch: 0,
        });
        let _ = send_server_message(client_handle, &busy_msg);
        return;
    };

    let (result_sender, result_receiver) = std::sync::mpsc::channel::<SearchResults>();
    let root_clone = root.to_path_buf();
    let prog_handle = std::thread::Builder::new()
        .name("ixd-search-prog".to_string())
        .spawn(move || {
            let _held = slot;
            let _read_guard = index_lock
                .as_ref()
                .map(|lk| lk.read().unwrap_or_else(PoisonError::into_inner));
            if let Err(e) = execute_search_progressive_inner(
                &root_clone,
                &query,
                &result_sender,
                caches.as_ref(),
            ) {
                tracing::warn!("ixd: progressive search failed: {e}");
                let _ = result_sender.send(SearchResults {
                    id: query.id,
                    matches: vec![],
                    stats: crate::executor::QueryStats::default(),
                    error: Some(e.to_string()),
                    done: true,
                    batch: 0,
                });
            }
        });

    while let Ok(batch) = result_receiver.recv() {
        if !send_server_message(client_handle, &ServerMessage::SearchResults(batch)) {
            tracing::warn!("ixd: client write failed for progressive batch");
            break;
        }
    }
    drop(result_receiver);
    if let Ok(handle) = prog_handle {
        let _ = handle.join();
    }
}

impl Drop for DaemonServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.accept_handle.take() {
            let _ = handle.join();
        }
        let (clients, threads) = {
            let mut s = lock_shared(&self.shared);
            (
                std::mem::take(&mut s.clients),
                std::mem::take(&mut s.client_threads),
            )
        };
        for client in clients {
            if let Ok(stream) = client.stream.lock() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        for handle in threads {
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
            let ts = u64::try_from(i).expect("index fits in u64");
            h.push(
                ts,
                vec![FileChange {
                    path: PathBuf::from(format!("f{i}")),
                    mtime: ts,
                    op: FileOp::Modify,
                }],
            );
        }
        assert_eq!(h.entries.len(), HISTORY_CAPACITY);
        // Oldest entry (ts=0) should have been evicted
        assert_eq!(h.entries.front().expect("non-empty").0, 1);
    }

    #[test]
    fn bounded_line_rejects_oversized_frame() {
        let data = b"{\"t\":\"status_query\"}\n";
        let mut reader = BufReader::new(&data[..]);
        let mut buf = Vec::new();
        match read_bounded_line(&mut reader, &mut buf, 8) {
            BoundedLineResult::TooLong => {}
            _ => panic!("expected TooLong for oversized frame"),
        }

        let mut ok_reader = BufReader::new(&data[..]);
        match read_bounded_line(&mut ok_reader, &mut buf, 64) {
            BoundedLineResult::Line(s) => assert_eq!(s.trim_end(), "{\"t\":\"status_query\"}"),
            _ => panic!("expected valid line within limit"),
        }
    }
}
