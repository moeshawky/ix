# Daemon Socket Internals Report

- **Source:** `/home/ubuntu/ix`
- **Last Verified:** 2026-11-22 (v0.14.2)
- **Historical Generation Date:** 2026-05-03

> **v0.14.2 Layout Note:** The former single-file `src/lib/daemon_sock.rs` was split into `src/lib/daemon_sock/{mod,client,resolve,search,server,types}.rs`, and `src/bin/ixd.rs` delegates watch loops to `src/lib/daemon.rs`. See the updated symbol table at the bottom and inline notes below.

---

## 1. ClientMessage Handling in ixd / daemon.rs

The daemon does not handle `ClientMessage` variants in `src/bin/ixd.rs`. Request parsing runs in `client_read_loop()` in `src/lib/daemon_sock/server.rs:541`, which runs in a background thread per connected client, caps inbound frames with `read_bounded_line` at `MAX_IPC_LINE_BYTES` (`1 MiB` at `server.rs:24`), serializes replies via a per-client `Arc<Mutex<UnixStream>>`, and resets `IdleTracker::record_query()` on `SearchQuery` (`server.rs:632-638`).

### StatusQuery

**File:** `src/lib/daemon_sock.rs`
**Lines:** 444-455

```rust
ClientMessage::StatusQuery => {
    let Ok(s) = shared.lock() else {
        tracing::warn!("ixd: shared lock poisoned in status query");
        continue;
    };
    ServerMessage::QueryResult {
        id: u64::from(s.pid),    // Returns PID as query ID
        status: s.status.clone(),
        files: s.files_count,
        changes_since: Vec::new(), // No history returned
    }
}
```

**Returns:** A `QueryResult` with:
- `id` = daemon PID (not from the request)
- `status` = current daemon status string
- `files` = current file count in index
- `changes_since` = empty vec (no history)

### HistoryQuery

**File:** `src/lib/daemon_sock.rs`
**Lines:** 457-469

```rust
ClientMessage::HistoryQuery { since, id } => {
    let Ok(s) = shared.lock() else {
        tracing::warn!("ixd: shared lock poisoned in history query");
        continue;
    };
    let changes = s.history.since(since);
    ServerMessage::QueryResult {
        id,                      // Echoes back client's id
        status: s.status.clone(),
        files: s.files_count,
        changes_since: changes,  // History entries after `since`
    }
}
```

**Returns:** A `QueryResult` with:
- `id` = client's id (echoed back)
- `status` = current daemon status string
- `files` = current file count in index
- `changes_since` = all `FileChange` records with timestamp strictly greater than `since`

---

## 2. DaemonClient recv() and send()

**File:** `src/lib/daemon_sock.rs`

### recv()

**Lines:** 521-546

```rust
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
```

**Behavior:**
- **Blocking:** Yes — calls `read_line()` which blocks until a newline is received
- **Timeout:** Yes — configured at connect time (lines 509): `stream.set_read_timeout(Some(Duration::from_secs(5)))`
- **Timeout error:** Returns `DaemonSockError::Io` with `ErrorKind::TimedOut` and message "recv timed out after 5s"
- **EOF:** Returns `UnexpectedEof` error "daemon closed connection"
- **No async variant:** None exists in the public API

### send()

**Lines:** 553-560

```rust
pub fn send(&mut self, msg: &ClientMessage) -> Result<()> {
    let stream = self.stream.get_mut();
    let mut line = serde_json::to_string(msg)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()?;
    Ok(())
}
```

**Behavior:**
- **Blocking:** Yes — `write_all()` blocks until all bytes are written
- **Timeout:** Yes — configured at connect time (line 510): `stream.set_write_timeout(Some(Duration::from_secs(5)))`
- **No iterator/stream interface:** `DaemonClient` does not implement `Iterator` or any async trait. It is a simple request-response client.

---

## 3. FileChange Struct

**File:** `src/lib/daemon_sock.rs`
**Lines:** 79-91

```rust
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
```

| Field | Type | JSON Key | Description |
|-------|------|----------|-------------|
| `path` | `PathBuf` | `"p"` | File path (relative when possible) |
| `mtime` | `u64` | `"m"` | Unix timestamp in **seconds** |
| `op` | `FileOp` | `"o"` | Operation kind |

---

## 4. FileOp Enum

**File:** `src/lib/daemon_sock.rs`
**Lines:** 53-77

```rust
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
```

**All 4 variants:** `Create`, `Modify`, `Delete`, `Rename`

Note: In practice, `Rename` is never generated. The watcher maps all rename events to `Modify` (line 74):
```rust
_ => Self::Modify,
```

---

## 5. HistoryQuery `since` Field Semantics

**File:** `src/lib/daemon_sock.rs`
**Lines:** 135-140

```rust
HistoryQuery {
    /// Return changes with timestamps strictly after this value.
    since: u64,
    /// Client-assigned query ID (echoed back in the response).
    id: u64,
}
```

**`since` semantics:**
- **Type:** `u64` — Unix timestamp in **seconds**
- **Filter:** Returns only entries with timestamp strictly greater than `since` (see line 219: `*ts > cutoff`)
- **Example:** `since: 150` returns entries with timestamp > 150 (not >= 150)

### History Retention

**File:** `src/lib/daemon_sock.rs`
**Lines:** 50-51, 198-223

```rust
/// Maximum number of change batches retained for history queries.
const HISTORY_CAPACITY: usize = 1024;

struct History {
    entries: VecDeque<(u64, Vec<FileChange>)>,
}
```

**Retention:**
- **Buffer type:** `VecDeque` (circular buffer)
- **Capacity:** 1024 batches (line 51)
- **Eviction:** Oldest batch is removed when capacity is exceeded (line 210-211)
- **No time window:** There is no time-based expiry — only count-based (1024 batches max)

**Implication:** If batches arrive roughly every 500ms (watcher debounce), the daemon retains approximately:
- 1024 × 0.5s = **~8.5 minutes** of history
- (Exact duration depends on actual event frequency)

---

## 6. Index Rebuild Complete Signal

**No explicit message variant or enum exists for this.**

The daemon uses a plain `String` for status. Defined status values in the source:

**File:** `src/bin/ixd.rs`

| Status String | Set at (line) | Meaning |
|---------------|---------------|---------|
| `"idle"` | 305 | Index is idle, no active rebuild |
| `"indexing (entropy: N)"` | 273 | Rebuild in progress |
| `"deferred (entropy: N)"` | 289 | Rebuild skipped due to high entropy |
| `"escalated (entropy: N)"` | 249 | Safety escalation triggered |
| `"warned: ..."` | 260 | Safety warning |
| `"safety halt"` | 230 | Critical safety halt |
| `"safety exit"` | 237 | Unrecoverable safety exit |

**No "ready" or "complete" status variant exists.** The only indication of rebuild completion is the status reverting to `"idle"` after a rebuild succeeds (line 305):
```rust
ctx.beacon.status = "idle".to_string();
```

The print statement at `src/lib/lib.rs:145` ("ixd: initial index ready") is a console message, not a socket status:
```rust
println!(
    "ixd: initial index ready ({} files, {} trigrams)",
    builder.files_len(),
    builder.trigrams_len()
);
```

---

## 7. FilesChanged Push Event — Code Path and Storage

### Code Path

**File:** `src/bin/ixd.rs`
**Lines:** 350-363

```rust
let Some(sock) = ctx.daemon_sock else { return };
let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs();
let changes: Vec<FileChange> = changed_files
    .iter()
    .map(|p| FileChange {
        path: p.clone(),
        mtime: now,          // All files get same timestamp (wall clock)
        op: FileOp::Modify,  // Always Modify (no differentiate)
    })
    .collect();
sock.notify_changes(changes, ctx.builder.files_len());
```

### Storage

**File:** `src/lib/daemon_sock.rs`
**Lines:** 406-416

```rust
pub fn notify_changes(&self, changes: Vec<FileChange>, files_count: usize) {
    let timestamp = now_secs();
    if let Ok(mut s) = self.shared.lock() {
        s.history.push(timestamp, changes.clone());  // Stored in memory
        s.files_count = files_count;
        let msg = ServerMessage::FilesChanged {
            batch: changes,
            timestamp,
        };
        s.clients.retain_mut(|c| c.send(&msg));      // Pushed to clients
    }
}
```

**Storage summary:**
| Aspect | Details |
|--------|---------|
| **Storage location** | Memory only (`VecDeque<(u64, Vec<FileChange>)>`) |
| **Persistence** | **Not persisted to disk** — lost on daemon restart |
| **Retention limit** | 1024 batches (see section 5) |
| **Key used** | Timestamp (Unix seconds) |
| **Queryable** | Yes — via `HistoryQuery { since }` |

**Flow:**
1. `handle_changes()` in ixd.rs creates `FileChange` vector from changed file paths
2. `sock.notify_changes()` is called
3. Changes are **both:**
   - Pushed to all connected clients via `ServerMessage::FilesChanged`
   - Stored in the `History` buffer in memory
4. Later, clients can query history via `HistoryQuery { since }`

---

## Summary Table (`v0.14.2` Paths)

| Item | File | Line(s) |
|------|------|---------|
| ClientMessage enum | `src/lib/daemon_sock/types.rs` | 276-298 |
| ServerMessage enum | `src/lib/daemon_sock/types.rs` | 154-197 |
| SearchQuery struct | `src/lib/daemon_sock/types.rs` | 205-262 |
| FileChange struct | `src/lib/daemon_sock/types.rs` | 89-99 |
| FileOp enum | `src/lib/daemon_sock/types.rs` | 8-17 |
| `client_read_loop()` | `src/lib/daemon_sock/server.rs` | 541-590 |
| `IdleTracker` reset on `SearchQuery` | `src/lib/daemon_sock/server.rs` | 632-638 |
| `MAX_IPC_LINE_BYTES` (1 MiB server / 64 MiB client) | `src/lib/daemon_sock/server.rs`, `client.rs` | 24 / 12 |
| `HISTORY_CAPACITY` (1024 batches) | `src/lib/daemon_sock/server.rs` | 21 |
| `notify_changes()` | `src/lib/daemon_sock/server.rs` | 374-390 |
| `recv()` impl | `src/lib/daemon_sock/client.rs` | 118-170 |
| `send()` impl | `src/lib/daemon_sock/client.rs` | 172-182 |
| `socket_path()` impl | `src/lib/daemon_sock/resolve.rs` | 19-74 |
| `run_many()` / initial build | `src/lib/daemon.rs` | 117, 354 |