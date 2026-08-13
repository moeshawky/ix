# Daemon Operation Runbook

**Purpose:** Operate and troubleshoot the `ixd` background indexing daemon  
**Last Verified:** 2026-08-13  
**Time to Complete:** 10 minutes

---

## Overview

The `ixd` daemon watches a directory for file changes and incrementally updates the trigram index. It provides:

- **Continuous indexing**: Automatic index updates on file changes
- **Unix domain socket**: Real-time notifications for editors/tooling
- **Memory safety**: `ResourceGuard` integration with 60% RSS ceiling
- **Concurrent instance guard**: Prevents multiple daemons on same root

**Binary:** `ixd` — standalone daemon binary

---

## Quick Start

### Step 1: Build the Daemon

```bash
cd /workspace/ix
cargo build --release --features notify
# Expected: "Finished release [optimized] target(s) in 45s"
```

### Step 2: Start the Daemon

```bash
# Watch a directory (foreground — for debugging or systemd units)
./target/release/ixd /path/to/repo

# Expected output:
# ixd: watching /path/to/repo...
# ixd: initial build complete (1234 files, 56789 trigrams)
# ixd: socket at /run/user/1000/ixd/{hash}.sock
```

`ixd` performs its own initial build on startup (`src/lib/daemon.rs:232`
unconditionally calls `builder.build()`), so there is no need to run
`ix --build` first — running both rebuilds the same shard twice
(measured 2026-08-13 on a 40-file / 240-trigram fixture: `ix --build`
12 ms, then `ixd` ~11 s rebuilding the identical base).

To detach from the terminal and run in the background, use `--daemon` (double-fork +
`setsid`; stdio is redirected to `/dev/null`, so no `nohup ... & disown` wrapper is
needed):

```bash
# Detach and run in the background. The shell returns immediately (exit 0).
./target/release/ixd --daemon /path/to/repo

# Confirm it is live:
ix service status
```

### Step 3: Verify Operation

```bash
# Check daemon is running
ps aux | grep ixd
# Expected: ixd process visible

# Check socket exists
ls -la $XDG_RUNTIME_DIR/ixd/*.sock
# Expected: socket file present

# Query status
echo '{"t":"status_query"}' | socat - UNIX-CONNECT:/path/to/socket
# Expected: {"pid":12345,"status":"idle","files":1234}
```

### Step 4: Test Indexing

```bash
# Modify a file
echo "// test" >> /path/to/repo/src/main.rs

# Check daemon logs
journalctl -u ixd -f  # if using systemd
# Or check console output
# Expected: "ixd: 1 files changed, updating index..."
```

---

## Configuration

### Configuration File (`.ixd.toml`)

The daemon and CLI read `.ixd.toml` files to scope indexing behavior. The daemon discovers config files at startup:

1. **Root config**: `.ixd.toml` at the watched root (e.g., `/path/to/repo/.ixd.toml`)
2. **Subdirectory configs**: One level deep (e.g., `/path/to/repo/project-a/.ixd.toml`)

Multiple configs are merged: root-level applies globally; subdirectory configs add `watch_roots` and `exclude_patterns`. Root-level `debounce_ms` takes precedence.

#### Schema

```toml
# .ixd.toml

# Directories within the watch root to index (optional).
# When empty, the daemon indexes the entire root recursively.
watch_roots = ["src", "lib", "tests"]

# Directory names to exclude from indexing.
# Defaults shown below — add or override as needed.
exclude_patterns = [
    ".git",
    "node_modules",
    "target",
    "vendor",
    "build",
]

# Debounce interval in milliseconds for file-watch event batching (optional).
# Minimum 50 ms, maximum 10 000 ms. Omit or set to null for the default (500 ms).
debounce_ms = 500
```

#### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `watch_roots` | `[String]` | `[]` | Subdirectories to scope indexing to. Empty = entire root. |
| `exclude_patterns` | `[String]` | `.git`, `node_modules`, `target` | Directory names skipped during file walk. |
| `debounce_ms` | `u64` or `null` | `null` (500 ms) | File-watch batching window in ms. Range: 50–10 000. Clamped to range. |

#### Examples

**Scope to specific directories:**
```toml
watch_roots = ["src", "lib", "include"]
```

Only `src/`, `lib/`, and `include/` will be indexed. All other directories under the watch root are ignored.

**Add custom exclusions:**
```toml
exclude_patterns = [".git", "node_modules", "target", "vendor", "build"]
```

**Tune debounce for faster or quieter change pickup:**
```toml
# Faster: 100ms debounce — changes become searchable ~100ms after save.
debounce_ms = 100

# Slower: 2000ms debounce — fewer rebuilds during bulk operations.
debounce_ms = 2000
```

The debounce window starts when the **first** file-change event arrives and resets every time a **new** event arrives within the window. Lower values give faster searchability; higher values reduce rebuild churn during bursty writes.

**Per-project scoping with multi-root daemon:**
```bash
# /home/ubuntu/.ixd.toml — global excludes only
# /home/ubuntu/project-a/.ixd.toml — watch src/ and lib/
# /home/ubuntu/project-b/.ixd.toml — watch app/ only

ixd /home/ubuntu
```

The daemon discovers all three config files. Project A indexes only its `src/` and `lib/`. Project B indexes only its `app/`. Global excludes apply everywhere.

**Verification:**
```bash
# Start daemon with verbose output to see config loading
ixd /path/to/root 2>&1 | grep "loaded config"

# Expected:
# ixd [project-name]: loaded config — 1 exclude patterns, 2 watch roots
```

For full schema and more examples, see [docs/.ixd.toml.md](.ixd.toml.md).

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `XDG_RUNTIME_DIR` | `/run/user/{uid}` | Socket location |
| `RUST_LOG` | `info` | Log level (debug/info/warn/error) |
| `IX_DEBUG_BUILD` | (unset) | If set, emits `IX-INDEXED:` and `IX-FLUSH:` per-file lines to stderr during indexing |

### Socket Locations

The daemon socket path is deterministic based on the watched root:

1. **Primary**: `$XDG_RUNTIME_DIR/ixd/{hash}.sock`
2. **Fallback**: `~/.local/run/ixd/{hash}.sock`
3. **Last resort**: `/tmp/ixd-{uid}-{hash}.sock`

Where `{hash}` = first 16 hex chars of `XXH64(canonical_root, 0)`.

**Example:**
```bash
# Calculate hash for /workspace/ix
echo -n "/workspace/ix" | xxh64 | cut -c1-16
# Output: a1b2c3d4e5f67890

# Socket path
ls -la /run/user/1000/ixd/a1b2c3d4e5f67890.sock
```

---

## Operation Modes

### Idle State

**Status string:** `"idle"`  
**Entropy:** < 1000  
**Behavior:** Waiting for file changes, socket queries served immediately

```json
{
  "t": "status",
  "pid": 12345,
  "status": "idle",
  "files": 1234,
  "daemon_status": {
    "state": "idle"
  }
}
```

### Indexing State

**Status string:** `"indexing (entropy: N)"`  
**Trigger:** File change detected  
**Behavior:** Updating delta index, queries may block

```json
{
  "t": "status",
  "pid": 12345,
  "status": "indexing (entropy: 450)",
  "files": 1234,
  "daemon_status": {
    "state": "indexing",
    "entropy": 450
  }
}
```

### Deferred State

**Status string:** `"deferred (entropy: N)`  
**Trigger:** System entropy > 1000  
**Behavior:** Indexing postponed, queries served from current index

```json
{
  "t": "status",
  "pid": 12345,
  "status": "deferred (entropy: 950)",
  "files": 1234,
  "daemon_status": {
    "state": "deferred",
    "entropy": 950
  }
}
```

### Safety States

| Status | Trigger | Recovery |
|--------|---------|----------|
| `"compacting"` | Full index rebuild (idle or delta size > 50 MB) | Automatic — completes within 30s–5min |
| `"warned: ..."` | Minor safety concern | Automatic after 300ms |
| `"escalated (entropy: N)"` | High entropy + surprise | Throttle, cooldown 1000ms |
| `"safety halt"` | Critical safety decision | Manual intervention required |
| `"safety exit"` | Unrecoverable error | Restart daemon |
| `"build failed: ..."` | Initial index build failed | Fix underlying issue, daemon will retry |

---

## Monitoring

### Health Checks

```bash
# Check if daemon is running
pgrep -f "ixd /path/to/repo"
# Exit code 0 = running, 1 = not running

# Query daemon status
echo '{"t":"status_query"}' | socat - UNIX-CONNECT:/path/to/socket

# Check socket responsiveness
timeout 5 bash -c 'echo -ne "{\"t\":\"status_query\"}\n" | socat - UNIX-CONNECT:/path/to/socket'
# Expected: response within 5 seconds
```

### Log Analysis

```bash
# Enable debug logging
export RUST_LOG=ix=debug
./target/release/ixd /path/to/repo 2>&1 | tee ixd.log

# Search for errors
grep -i error ixd.log
grep -i "failed\|panic" ixd.log

# Monitor file change rate
grep "files changed" ixd.log | tail -100
# Expected: frequency indicates indexing load
```

### Metrics Collection

```bash
# Extract metrics from logs
grep "files changed" ixd.log | awk '{print $4}' | sort -n | uniq -c

# Count indexing operations
grep -c "index updated" ixd.log

# Measure latency (time between change and completion)
grep -A1 "files changed" ixd.log | grep "index updated" | \
  awk '{print $2 - prev_time}'  # Requires timestamp parsing
```

---

## Troubleshooting

### Daemon Won't Start

**Symptom:** `ixd` exits immediately with error

**Check:**
```bash
# Check for concurrent instance
cat /path/to/repo/.ix/beacon.json
# If exists and PID is valid, another instance is running

# Check permissions
ls -la /path/to/repo/.ix/
# Expected: writable by current user

# Check disk space
df -h /path/to/repo
# Expected: >10% free space
```

**Fix:**
```bash
# Remove stale beacon (if PID not running)
rm /path/to/repo/.ix/beacon.json

# Rebuild index from scratch
rm -rf /path/to/repo/.ix/
ixd /path/to/repo
```

### High Entropy / Deferred Indexing

**Symptom:** Status shows `"deferred (entropy: N)"` frequently

**Check:**
```bash
# Check system memory pressure
free -h
cat /proc/stat | grep cpu

# Check daemon memory usage
ps -o pid,rss,vsz,cmd -p $(pgrep -f "ixd /path")
```

**Fix:**
```bash
# Reduce watched directory scope
# Edit .gitignore or watcher config to exclude large directories

# Increase system memory
# Close other memory-intensive applications

# Restart daemon (clears accumulated state)
pkill -f "ixd /path/to/repo"
ixd /path/to/repo
```

### Socket Connection Failures

**Symptom:** Client cannot connect to daemon socket

**Check:**
```bash
# Verify socket exists
ls -la $XDG_RUNTIME_DIR/ixd/*.sock

# Check socket permissions
stat -c "%a %U:%G" /path/to/socket
# Expected: 0755 or 0777

# Test connection
echo '{"t":"status_query"}' | socat - UNIX-CONNECT:/path/to/socket
```

**Fix:**
```bash
# Restart daemon (recreates socket)
pkill -f "ixd"
ixd /path/to/repo

# Check XDG_RUNTIME_DIR is accessible
echo $XDG_RUNTIME_DIR
ls -la $XDG_RUNTIME_DIR
```

### Index Corruption

**Symptom:** Search returns incorrect results or crashes

**Check:**
```bash
# Verify index integrity
ix --stats "test" 2>&1 | grep -i error

# Check delta file size
ls -la /path/to/repo/.ix/shard.ix.delta
# Expected: reasonable size (<100MB typical)
```

**Fix:**
```bash
# Delete delta and rebuild
rm /path/to/repo/.ix/shard.ix.delta
ix --build /path/to/repo

# Full rebuild if corruption persists
rm -rf /path/to/repo/.ix/
ix --build /path/to/repo
```

---

## Safety Features

### ResourceGuard Integration

The daemon uses `ResourceGuard` to monitor:

- **RSS Memory**: 60% system ceiling
- **Entropy**: System-wide pressure metric
- **Surprise**: Rate of change detection
- **Bias**: Long-term trend analysis

**Safety Decisions:**

| Decision | Trigger | Action |
|----------|---------|--------|
| `Proceed` | All metrics nominal | Continue indexing |
| `Warn` | Minor threshold breach | Log warning, 300ms pause |
| `Escalate` | High entropy + surprise | Throttle, 1000ms cooldown |
| `Halt` | Critical breach | Pause operations, manual review |
| `Exit` | Unrecoverable | Terminate daemon |

### Concurrent Instance Prevention

The daemon prevents multiple instances watching the same root:

```json
{
  "pid": 12345,
  "root": "/path/to/repo",
  "start_time": 1684089600,
  "status": "idle",
  "last_event_at": 1684089600,
  "socket_path": "/run/user/1000/ixd/a1b2c3d4e5f67890.sock",
  "instance_id": 1684089600000000000
}
```

**Verification:**
```bash
# Check beacon
cat /path/to/repo/.ix/beacon.json

# Verify PID is running
ps -p 12345

# Check if PID is actually ixd
cat /proc/12345/comm
# Expected: "ixd"
```

---

## Shutdown Procedures

### Graceful Shutdown

```bash
# Send SIGTERM (recommended)
kill -TERM $(pgrep -f "ixd /path/to/repo")

# Or use Ctrl+C if running in foreground
# Daemon will:
# 1. Complete current indexing operation
# 2. Write final status to beacon
# 3. Remove beacon file
# 4. Exit cleanly
```

### Forceful Shutdown

```bash
# Only if graceful shutdown fails
kill -KILL $(pgrep -f "ixd /path/to/repo")

# Clean up beacon manually
rm /path/to/repo/.ix/beacon.json
```

---

## Performance Tuning

### Memory Limits

The 60% system-memory ceiling is hardcoded in `ResourceGuard`
(`src/bin/ixd.rs:31`); it is not configurable via an environment
variable. To control memory pressure in practice, scope what the daemon
watches via `.ixd.toml` (see Configuration above) — smaller watch roots
mean a smaller index and lower RSS.

```bash
# Monitor memory usage
watch -n1 'ps -o pid,rss,cmd -p $(pgrep -f ixd)'
```

If `ixd` reports `deferred (entropy: N)` frequently, reduce watch scope
or close other memory-intensive applications, then restart the daemon.

### File Watch Limits

```bash
# Check inotify limits (Linux)
cat /proc/sys/fs/inotify/max_user_watches
# Expected: 8192 or higher

# Increase if needed
echo fs.inotify.max_user_watches=524288 | sudo tee -a /etc/sysctl.conf
sudo sysctl -p
```

---

---

## See Also

- `DAEMON-SOCKET-INTERNALS.md` — Socket protocol specification
- `docs/DELTA-FORMAT.md` — Delta index format
- `src/lib/daemon.rs` — Full daemon implementation
- `src/lib/daemon_sock.rs` — Socket handling code

### Client Shutdown Protocol

When the daemon receives a shutdown signal (SIGTERM/SIGINT), it:

1. **Broadcasts** `ServerMessage::Shutdown` to all connected clients via the Unix socket
2. **Waits** 1000ms for clients to receive the notice
3. **Closes** the socket and exits

**Client Behavior:**
- Clients receive `{"t":"shutdown","reason":"signal","delay_ms":1000}`
- Clients have 1000ms to finish in-flight operations
- After delay, socket closes (EOF)
- Clients can distinguish graceful shutdown from crash

**Example Client Handling:**
```json
// Client receives:
{"t":"shutdown","reason":"signal","delay_ms":1000}

// Client should:
// 1. Complete current query (if any)
// 2. Save state if needed
// 3. Reconnect after delay (if auto-reconnect enabled)
```

**Optional Acknowledgment:**
Clients can send acknowledgment (fire-and-forget):
```json
{"t":"shutdown","ack":true}
```

This is logged by the daemon but not required - the shutdown proceeds regardless.
