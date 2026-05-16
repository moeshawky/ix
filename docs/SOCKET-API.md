# Daemon Socket API Reference

**Purpose**: External tooling integration with ixd daemon via Unix domain socket  
**Protocol**: NDJSON (newline-delimited JSON)  
**Last Verified**: 2026-05-16  
**Source**: `src/lib/daemon_sock.rs`  

---

## Quick Reference

| Message Type | Direction | Purpose |
|--------------|-----------|---------|
| `StatusQuery` | Client → Server | Request daemon status |
| `HistoryQuery` | Client → Server | Get changes since timestamp |
| `SearchQuery` | Client → Server | Execute search |
| `Shutdown` | Both | Graceful shutdown notification/ack |
| `Status` | Server → Client | Daemon status update |
| `FilesChanged` | Server → Client | File change notification |
| `QueryResult` | Server → Client | Search/status response |

---

## Socket Path Resolution

The socket path is deterministic based on the watched root directory:

```bash
# Priority 1: XDG runtime directory (systemd/modern Linux)
$XDG_RUNTIME_DIR/ixd/{hash}.sock

# Priority 2: User's local run directory
~/.local/run/ixd/{hash}.sock

# Priority 3: Temporary directory (fallback)
/tmp/ixd-{uid}-{hash}.sock
```

Where `{hash}` = first 16 hex characters of `XXH64(canonical_root, 0)`.

**Example:**
```bash
# Calculate hash for /workspace/ix
echo -n "/workspace/ix" | xxh64 | cut -c1-16
# Output: a1b2c3d4e5f67890

# Socket path would be:
# /run/user/1000/ixd/a1b2c3d4e5f67890.sock
```

**Source**: `src/lib/daemon_sock.rs:284-305`

---

## Wire Protocol

### Format
- **Encoding**: UTF-8
- **Framing**: One JSON object per line, terminated by `\n`
- **Direction**: Bidirectional (full-duplex)

### Example Session
```json
// Client → Server
{"t":"status_query","id":1}

// Server → Client
{"t":"status","pid":12345,"status":"idle","files":1523,"daemon_status":{"state":"idle"}}
```

**Source**: `src/lib/daemon_sock.rs:560-640`

---

## Client Messages (Client → Server)

### StatusQuery
Request current daemon status.

```json
{"t":"status_query","id":1}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `t` | string | Yes | Must be `"status_query"` |
| `id` | u64 | No | Client-assigned ID (echoed in response) |

**Source**: `src/lib/daemon_sock.rs:244-249`

---

### HistoryQuery
Request all changes since a given timestamp.

```json
{"t":"history_query","since":1776468000,"id":2}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `t` | string | Yes | Must be `"history_query"` |
| `since` | u64 | Yes | Unix timestamp (seconds) |
| `id` | u64 | No | Client-assigned ID |

**Source**: `src/lib/daemon_sock.rs:251-257`

---

### SearchQuery
Execute a search query.

```json
{
  "t":"search_query",
  "id":3,
  "pattern":"TODO",
  "is_regex":false,
  "ignore_case":false,
  "word_boundary":false,
  "max_results":0,
  "context_lines":0,
  "file_types":[],
  "decompress":false,
  "multiline":false,
  "archive":false,
  "binary":false
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `t` | string | Yes | - | Must be `"search_query"` |
| `pattern` | string | Yes | - | Search pattern |
| `is_regex` | bool | No | false | Interpret as regex |
| `ignore_case` | bool | No | false | Case-insensitive search |
| `word_boundary` | bool | No | false | Match whole words only |
| `max_results` | u64 | No | 0 | 0 = unlimited |
| `context_lines` | u64 | No | 0 | Lines of context |
| `file_types` | array | No | [] | File extensions (e.g., `["rs","py"]`) |
| `decompress` | bool | No | false | Search inside compressed files |
| `multiline` | bool | No | false | Dot matches newline (regex only) |
| `archive` | bool | No | false | Search inside archives |
| `binary` | bool | No | false | Search binary files |

**Source**: `src/lib/daemon_sock.rs:195-236`

---

### Shutdown (NEW)
Client acknowledgment of shutdown notice (optional, fire-and-forget).

```json
{"t":"shutdown","ack":true}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `t` | string | Yes | - | Must be `"shutdown"` |
| `ack` | bool | No | false | Acknowledgment flag |

**Behavior**:
- Logged by daemon at debug level
- No response sent
- Does not affect shutdown sequence

**Source**: `src/lib/daemon_sock.rs:264-269`

---

## Server Messages (Server → Client)

### Status
Periodic or on-change daemon status update.

```json
{
  "t":"status",
  "pid":12345,
  "status":"idle",
  "files":1523,
  "daemon_status":{"state":"idle"}
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `t` | string | Yes | Must be `"status"` |
| `pid` | u32 | Yes | Daemon process ID |
| `status` | string | Yes | Human-readable status |
| `files` | u64 | Yes | Number of indexed files |
| `daemon_status` | object | No | Typed status (see below) |

**Source**: `src/lib/daemon_sock.rs:156-167`

---

### FilesChanged
Batch of file changes detected by the watcher.

```json
{
  "t":"files_changed",
  "batch":[
    {"p":"src/main.rs","m":1776468629,"o":"modify"}
  ],
  "ts":1776468629
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `t` | string | Yes | Must be `"files_changed"` |
| `batch` | array | Yes | Array of file changes |
| `ts` | u64 | Yes | Unix timestamp |

**FileChange object**:
| Field | Type | Description |
|-------|------|-------------|
| `p` | string | File path |
| `m` | u64 | Modification time |
| `o` | string | Operation: `"create"`, `"modify"`, `"delete"`, `"rename"` |

**Source**: `src/lib/daemon_sock.rs:169-180`

---

### QueryResult
Response to a client query (status, history, search).

```json
{
  "t":"query_result",
  "id":1,
  "status":"idle",
  "files":1523,
  "changes_since":[],
  "daemon_status":{"state":"idle"},
  "last_rebuild_at":1776468000
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `t` | string | Yes | Must be `"query_result"` |
| `id` | u64 | Yes | Echoed from request |
| `status` | string | Yes | Current status string |
| `files` | u64 | Yes | File count |
| `changes_since` | array | No | Changes since timestamp (for history queries) |
| `daemon_status` | object | No | Typed status |
| `last_rebuild_at` | u64 | No | Last successful rebuild timestamp |

**Source**: `src/lib/daemon_sock.rs:182-193`

---

### SearchResults
Search query results.

```json
{
  "t":"search_results",
  "id":3,
  "matches":[
    {
      "file_path":"src/main.rs",
      "line_number":42,
      "col":10,
      "line_content":"fn main() {",
      "byte_offset":1234,
      "context_before":[],
      "context_after":[],
      "is_binary":false
    }
  ],
  "stats":{
    "trigrams_queried":3,
    "posting_lists_decoded":2,
    "candidate_files":5,
    "files_verified":3,
    "bytes_verified":1024,
    "total_matches":1
  }
}
```

**Source**: `src/lib/daemon_sock.rs:191-193`, `src/lib/executor.rs:58-72`

---

### Shutdown (NEW)
Graceful shutdown notice sent to all clients before socket closes.

```json
{
  "t":"shutdown",
  "reason":"signal",
  "delay_ms":1000
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `t` | string | Yes | Must be `"shutdown"` |
| `reason` | string | Yes | Why shutting down (e.g., `"signal"`, `"user_request"`) |
| `delay_ms` | u32 | Yes | Milliseconds before socket closes |

**Client Behavior**:
1. Receive shutdown notice
2. Complete any in-flight operations
3. Save state if needed
4. Wait `delay_ms` milliseconds
5. Reconnect after delay (if auto-reconnect enabled)

**Source**: `src/lib/daemon_sock.rs:194-196, 570-579`

---

## Typed Daemon Status

The `daemon_status` field provides structured status information:

```json
// Idle
{"state":"idle"}

// Indexing
{"state":"indexing","entropy":42}

// Deferred (high entropy)
{"state":"deferred","entropy":1500}

// Escalated (safety concern)
{"state":"escalated","entropy":2000}

// Warned
{"state":"warned","reason":"memory pressure"}

// Safety halt
{"state":"safety_halt"}

// Safety exit
{"state":"safety_exit"}
```

**Source**: `src/lib/daemon_sock.rs:80-121`

---

## Graceful Shutdown Protocol

### Sequence
1. **Signal Received**: Daemon receives SIGTERM/SIGINT
2. **Broadcast**: `ServerMessage::Shutdown` sent to all connected clients
3. **Delay**: Daemon waits 1000ms (configurable via `delay_ms`)
4. **Close**: Socket file removed, connections closed

### Client Implementation Guide

```python
import socket
import json
import time

def connect_to_daemon(root_path):
    """Connect to ixd daemon socket."""
    import hashlib
    import xxhash
    
    # Calculate hash
    canonical = str(Path(root_path).resolve())
    hash_hex = xxhash.xxh64(canonical).hexdigest()[:16]
    
    # Try paths in order
    paths = [
        f"{os.environ.get('XDG_RUNTIME_DIR', f'/run/user/{os.getuid()}')}/ixd/{hash_hex}.sock",
        f"{os.path.expanduser('~')}/.local/run/ixd/{hash_hex}.sock",
        f"/tmp/ixd-{os.getuid()}-{hash_hex}.sock"
    ]
    
    for path in paths:
        try:
            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            sock.connect(path)
            return sock
        except FileNotFoundError:
            continue
    
    raise ConnectionRefusedError("Daemon not running")

def listen_for_shutdown(sock):
    """Listen for shutdown notice from daemon."""
    reader = sock.makefile()
    
    while True:
        line = reader.readline()
        if not line:
            break  # EOF - daemon closed connection
        
        msg = json.loads(line)
        
        if msg.get('t') == 'shutdown':
            reason = msg.get('reason', 'unknown')
            delay_ms = msg.get('delay_ms', 1000)
            
            print(f"Daemon shutting down: {reason}")
            print(f"Socket will close in {delay_ms}ms")
            
            # Complete in-flight operations
            # Save state if needed
            
            # Wait for delay (or less if you want to reconnect sooner)
            time.sleep(delay_ms / 1000.0)
            
            # Socket will be closed by daemon
            break
```

**Source**: `src/lib/daemon.rs:198-201`, `src/lib/daemon_sock.rs:570-579`

---

## Verification Commands

### Test Socket Connection
```bash
# Check if socket exists
ls -la $XDG_RUNTIME_DIR/ixd/*.sock

# Test connection
echo '{"t":"status_query"}' | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/ixd/{hash}.sock
```

### Verify Shutdown Protocol
```bash
# Start daemon in one terminal
ixd /tmp/test-repo

# In another terminal, listen for shutdown
# (use a client that prints received messages)

# Send SIGTERM to daemon
kill -TERM $(pgrep -f "ixd /tmp/test-repo")

# Expected: Client receives {"t":"shutdown","reason":"signal","delay_ms":1000}
```

**Source**: `tests/daemon_sock.rs:test_shutdown_protocol`

---

## Error Handling

| Error | Cause | Recovery |
|-------|-------|----------|
| `ENOENT` (socket not found) | Daemon not running | Start daemon with `ixd /path` |
| `ECONNREFUSED` | Socket exists but daemon dead | Remove socket file, restart daemon |
| `ETIMEDOUT` | Daemon not responding | Check daemon logs, restart if needed |
| JSON parse error | Malformed message | Verify message format, check protocol version |

**Source**: `src/lib/daemon_sock.rs:669-700`

---

## See Also
- `DAEMON-RUNBOOK.md` — Operational procedures
- `DELTA-FORMAT.md` — Index format specification
- `src/lib/daemon_sock.rs` — Full implementation
- `tests/daemon_sock.rs` — Test cases with examples
