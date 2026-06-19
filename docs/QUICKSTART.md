# Quick Start Guide

**Purpose**: Get started with ix code search in under 10 minutes  
**Last Verified**: 2026-05-16  
**Verification Command**: `./scripts/verify-docs.sh`  

---

## Table of Contents

1. [CLI Quick Start (2 minutes)](#cli-quick-start)
2. [Daemon Quick Start (5 minutes)](#daemon-quick-start)
3. [Socket API Quick Start (3 minutes)](#socket-api-quick-start)
4. [Next Steps](#next-steps)

---

## CLI Quick Start (2 minutes)

### Step 1: Install (30 seconds)

```bash
# Install from crates.io (standard — search + daemon)
cargo install moeix

# Or with all features (adds archive + compressed file support)
cargo install moeix --features full

# Expected output:
# Finished release [optimized] target(s) in 45s
# Installed binary `ix`
# Installed binary `ixd`
```

**Feature flags:**
| Feature | What it adds | Increases binary by |
|---------|-------------|---------------------|
| `notify` (default) | File watching for daemon | ~2 MB |
| `decompress` | `.gz` `.zst` `.bz2` `.xz` support | ~3 MB |
| `archive` | `.zip` `.tar.gz` support | ~2 MB |
| `full` | All of the above | ~7 MB |

**Source**: `Cargo.toml:49-54`

### Step 2: Build Index (30 seconds)

```bash
cd /path/to/your/repo
ix --build

# Expected output:
# ix: building index for /path/to/your/repo...
# ix: 1234 files, 56789 trigrams in 2.3s
```

**Source**: `src/bin/ix.rs:280-310`

### Step 3: Search (30 seconds)

```bash
# Literal search
ix "fn main"

# Expected output:
# src/main.rs:5:fn main() {
# src/lib.rs:10:pub fn main() -> Result<()> {

# Regex search
ix --regex "fn\s+\w+_handler"

# Expected output:
# src/handlers.rs:42:fn handle_request() {
# src/handlers.rs:58:fn handle_response() {
```

**Source**: `src/bin/ix.rs:320-420`

### Step 4: Verify (30 seconds)

```bash
# Check version
ix --version

# Expected: ix 0.6.3

# Check help
ix --help

# Expected: Full usage information
```

**✅ CLI is working if you see search results!**

---

## Daemon Quick Start (5 minutes)

### Prerequisites
- Rust 1.85+ installed
- `cargo` in PATH
- A repository to watch

### Step 1: Build Daemon (2 minutes)

```bash
cd /workspace/ix

# Standard daemon (notify is already the default)
cargo build --release

# Or with full feature set (archive + compressed files)
cargo build --release --features full

# Expected output:
# Finished release [optimized] target(s) in 45s
```

**Source**: `Cargo.toml:85-95`

### Step 2: Start Daemon (1 minute)

```bash
# Start watching a directory
./target/release/ixd /path/to/repo

# Expected output:
# ixd: watching /path/to/repo...
# ixd: initial build complete (1234 files, 56789 trigrams)
# ixd: socket at /run/user/1000/ixd/a1b2c3d4e5f67890.sock
```

**Source**: `src/lib/daemon.rs:54-120`

### Step 3: Verify Daemon (1 minute)

```bash
# Check daemon is running
ps aux | grep ixd

# Expected: ixd process visible

# Check socket exists
ls -la $XDG_RUNTIME_DIR/ixd/*.sock

# Expected: socket file present

# Query status
echo '{"t":"status_query"}' | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/ixd/*.sock

# Expected: {"pid":12345,"status":"idle","files":1234,"daemon_status":{"state":"idle"}}
```

**Source**: `src/lib/daemon_sock.rs:669-700`

### Step 4: Test Live Indexing (1 minute)

```bash
# Modify a file
echo "// test comment" >> /path/to/repo/src/main.rs

# Watch daemon output (in the terminal where ixd is running)
# Expected: "ixd: 1 files changed, updating index..."

# Query status again
echo '{"t":"status_query"}' | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/ixd/*.sock

# Expected: files count should be same or +1
```

**✅ Daemon is working if status query responds!**

---

## Socket API Quick Start (3 minutes)

### Step 1: Connect (1 minute)

**Python Example:**
```python
import socket
import json
import os
import xxhash

def get_socket_path(root):
    """Get daemon socket path for given root directory."""
    canonical = str(Path(root).resolve())
    hash_hex = xxhash.xxh64(canonical).hexdigest()[:16]
    
    # Try paths in order
    paths = [
        f"{os.environ.get('XDG_RUNTIME_DIR', f'/run/user/{os.getuid()}')}/ixd/{hash_hex}.sock",
        f"{os.path.expanduser('~')}/.local/run/ixd/{hash_hex}.sock",
        f"/tmp/ixd-{os.getuid()}-{hash_hex}.sock"
    ]
    
    for path in paths:
        if os.path.exists(path):
            return path
    
    raise FileNotFoundError("Daemon socket not found")

# Connect to daemon
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect(get_socket_path("/path/to/repo"))

print("Connected to daemon!")
```

**Source**: `src/lib/daemon_sock.rs:284-305`

### Step 2: Query Status (1 minute)

```python
# Send status query
query = {"t": "status_query", "id": 1}
sock.sendall((json.dumps(query) + "\n").encode())

# Receive response
response = sock.recv(4096).decode()
print(json.loads(response))

# Expected: {"t":"status","pid":12345,"status":"idle","files":1234,...}
```

**Source**: `src/lib/daemon_sock.rs:560-580`

### Step 3: Search (1 minute)

```python
# Send search query
query = {
    "t": "search_query",
    "id": 2,
    "pattern": "TODO",
    "is_regex": False,
    "max_results": 5
}
sock.sendall((json.dumps(query) + "\n").encode())

# Receive results
response = sock.recv(8192).decode()
data = json.loads(response)
print(f"Found {len(data.get('matches', []))} matches")

# Expected: {"t":"search_results","id":2,"matches":[...],"stats":{...}}
```

**Source**: `src/lib/daemon_sock.rs:600-640`

**✅ Socket API is working if you get search results!**

---

## Next Steps

### For CLI Users
- [ ] Read `README.md` for advanced search options
- [ ] Learn about regex patterns (`ix --regex`)
- [ ] Explore context options (`ix -C 3`)

### For Daemon Users
- [ ] Read `DAEMON-RUNBOOK.md` for operational procedures
- [ ] Configure systemd service (if on Linux)
- [ ] Set up log rotation

### For API Developers
- [ ] Read `SOCKET-API.md` for complete API reference
- [ ] Implement shutdown handling
- [ ] Add reconnection logic

### For All Users
- [ ] Join community (if applicable)
- [ ] Report issues on GitHub
- [ ] Contribute to documentation

---

## Troubleshooting

### "Command not found: ix"
**Fix**: Ensure `~/.cargo/bin` is in PATH
```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

### "Daemon won't start"
**Check**:
1. Is port already in use? `lsof -i :<port>`
2. Is another instance running? `pgrep -f ixd`
3. Check logs: `journalctl -u ixd` or console output

### "Socket not found"
**Fix**:
1. Verify daemon is running: `ps aux | grep ixd`
2. Check socket directory: `ls -la $XDG_RUNTIME_DIR/ixd/`
3. Restart daemon if needed

### "Index outdated"
**Fix**: Force rebuild
```bash
ix --fresh "pattern"
```

---

## Verification Checklist

Run these to verify your installation:

```bash
# CLI verification
ix --version                    # Should show version
ix --build                      # Should build index
ix "test"                       # Should find matches

# Daemon verification
ps aux | grep ixd               # Should show process
ls $XDG_RUNTIME_DIR/ixd/*.sock  # Should show socket
echo '{"t":"status_query"}' | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/ixd/*.sock

# API verification (Python)
python3 -c "
import socket, json
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect('YOUR_SOCKET_PATH')
sock.send(b'{\"t\":\"status_query\",\"id\":1}\n')
print(sock.recv(1024).decode())
"
```

**Expected**: All commands complete successfully with expected output.

---

## See Also
- `README.md` — Main project documentation
- `SOCKET-API.md` — Complete API reference
- `DAEMON-RUNBOOK.md` — Operational procedures
- `CONTRIBUTING.md` — How to contribute
