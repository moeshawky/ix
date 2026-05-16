# Daemon Operation Runbook

**Purpose:** Operate and troubleshoot the `ixd` background indexing daemon  
**Last Verified:** 2026-05-14  
**Time to Complete:** 10 minutes  
**Verification Command:** `./scripts/verify-daemon.sh`  

---

## Overview

The `ixd` daemon watches a directory for file changes and incrementally updates the trigram index. It provides:

- **Continuous indexing**: Automatic index updates on file changes
- **Unix domain socket**: Real-time notifications for editors/tooling
- **Memory safety**: LLMOSafe integration with 60% RSS ceiling
- **Concurrent instance guard**: Prevents multiple daemons on same root

**Binaries:**
- `ixd` — Standalone daemon binary
- `ix --daemon` — Deprecated, use `ixd` directly

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
# Watch a directory
./target/release/ixd /path/to/repo

# Expected output:
# ixd: watching /path/to/repo...
# ixd: initial build complete (1234 files, 56789 trigrams)
# ixd: socket at /run/user/1000/ixd/{hash}.sock
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

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `XDG_RUNTIME_DIR` | `/run/user/{uid}` | Socket location |
| `RUST_LOG` | `info` | Log level (debug/info/warn/error) |

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
**Entropy:** < 800  
**Behavior:** Waiting for file changes, socket queries served immediately

```json
{
  "t": "status",
  "pid": 12345,
  "status": "idle",
  "files": 1234,
  "daemon_status": {
    "t": "idle"
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
    "t": "indexing",
    "entropy": 450
  }
}
```

### Deferred State

**Status string:** `"deferred (entropy: N)`  
**Trigger:** System entropy > 800  
**Behavior:** Indexing postponed, queries served from current index

```json
{
  "t": "status",
  "pid": 12345,
  "status": "deferred (entropy: 950)",
  "files": 1234,
  "daemon_status": {
    "t": "deferred",
    "entropy": 950
  }
}
```

### Safety States

| Status | Trigger | Recovery |
|--------|---------|----------|
| `"warned: ..."` | Minor safety concern | Automatic after 300ms |
| `"escalated (entropy: N)"` | High entropy + surprise | Throttle, cooldown 1000ms |
| `"safety halt"` | Critical safety decision | Manual intervention required |
| `"safety exit"` | Unrecoverable error | Restart daemon |

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

### LLMOSafe Integration

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

```rust
// Beacon file: .ix/beacon.json
{
  "pid": 12345,
  "root": "/path/to/repo",
  "started_at": "2026-05-14T15:00:00Z"
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

```bash
# Set explicit memory ceiling (default: 60%)
export LLMOSAFE_CEILING=0.5  # 50% of system memory

# Monitor memory usage
watch -n1 'ps -o pid,rss,cmd -p $(pgrep -f ixd)'
```

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

## Verification Commands

```bash
# Full daemon verification
./scripts/verify-daemon.sh

# Expected output:
# [✓] Daemon process running
# [✓] Socket accessible
# [✓] Status query responds
# [✓] Index up to date
# All checks passed.
```

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
