# BUG: `ix service install` cannot manage multiple project roots

**Severity:** HIGH — daemon silently drops watch coverage when installed from second project  
**Discovered:** 2026-05-31 11:34 EEST  
**Version affected:** v0.11.0 (all versions since multi-root daemon v0.9.0)  
**Components:** `ix service install` → systemd unit → `ix --daemon` → `daemon::run()`

---

## Symptom

Installing `ix service install` from `/workspace/ix`, then from `/workspace/secure_future`, results in only the second project being watched. The first project's daemon is silently killed with no warning, no merge, and no migration path.

**User-visible failure on 2026-05-31:**

```
$ ix service install        # ran from /workspace/ix
Watch path: /workspace/ix
$ ix service start
$ ix service install        # ran from /workspace/secure_future
Watch path: /workspace/secure_future   # silently overwrote first project
```

After second install, system state is split-brained:

| Component | Points to | State |
|-----------|-----------|-------|
| Systemd unit file (`ixd.service`) | `/workspace/secure_future` | Watches second project |
| Running process (PID 2539213) | `/workspace/ix` | Watches first project — stale, never restarted |
| `/workspace/ix/.ix/shard.ix` | 1.6 MB | Orphaned index, no daemon |
| `/workspace/secure_future/.ix/shard.ix` | 17 MB | Fresh index from initial build |

---

## Root Cause — Two cascading bugs

### Bug 1: `ix --daemon` only watches the first path (dead loop)

**File:** `src/bin/ix.rs:344-356`

```rust
if cli.daemon {
    let paths: Vec<PathBuf> = if cli.path.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        cli.path.clone()
    };
    for path in &paths {                          // ← iterates paths
        if let Err(e) = ix::daemon::run(path) {   // ← calls run() per path
            eprintln!("Error watching {}: {e}", path.display());
            std::process::exit(1);
        }
    }
    return;
}
```

**File:** `src/lib/daemon.rs:65-67`

```rust
pub fn run(root: &Path) -> crate::error::Result<()> {
    run_many(&[root.to_path_buf()])  // ← spawns thread, joins — NEVER RETURNS
}
```

`run_many()` spawns a thread per root and calls `handle.join()` on each — the join blocks indefinitely because daemon threads run forever. The `for path in &paths` loop stalls at `paths[0]`. Paths `[1..]` are **never reached**.

Even if `ix --daemon /project-a /project-b` were invoked, only `/project-a` would be watched.

**Contract comparison — the library supports multi-root, the CLI breaks it:**

| Call site | Multi-root? | Mechanism |
|-----------|------------|-----------|
| `ixd.rs:52` → `daemon::run_many(&cli.paths)` | YES | Passes all paths in one call, one thread per root |
| `ix.rs:352` → `daemon::run(path)` in loop | NO | Calls `run()` (which calls `run_many(&[single_root])`) — blocks on first |

### Bug 2: `ix service install` overwrites single-path unit file

**File:** `src/bin/ix.rs:446,449-481`

```rust
let service_dir = PathBuf::from(&home).join(".config/systemd/user");
let service_file = service_dir.join("ixd.service");    // ← HARDCODED single unit name
```

```rust
let watch_path = path.unwrap_or_else(|| {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from(&home))
});                                                       // ← SINGLE path
let watch_path_abs = watch_path.canonicalize().unwrap_or(watch_path);

let daemon_cmd = format!("{} --daemon", ix_path.display());  // ← uses ix --daemon (Bug 1 path)

let service_content = format!(
    r"[Unit]
Description=ix background daemon
After=network.target

[Service]
ExecStart={} {}                      // ← SINGLE watch_path_abs in ExecStart
Restart=on-failure
RestartSec=10
StartLimitBurst=3
StartLimitIntervalSec=60

[Install]
WantedBy=default.target
",
    daemon_cmd,
    watch_path_abs.display()
);

std::fs::write(&service_file, service_content)?;   // ← OVERWRITES previous install
```

Three sub-bugs:
1. **Single path only** (line 452-455): picks one `watch_path`, never accumulates
2. **Hardcoded unit name** (line 446): always `ixd.service` — overwrites
3. **Uses `ix --daemon`** (line 460): invokes Bug 1 path, which can't even handle multi-root if the unit tried

---

## Impact

- **Consumer breakage**: codegraph-ferrari depends on ix for file watching. Installing the service from a different project kills codegraph's watch coverage with no error
- **Silent failure**: no warning when overwriting, no command to list or merge roots
- **Cross-contamination**: `/workspace/secure_future` daemon indexes `ix` repo files if `exclude_patterns` doesn't block it; `/workspace/ix` daemon indexes `secure_future` files — depending on which was installed last
- **Disk waste**: orphaned `.ix/` directories with no daemon
- **30+ hour catastrophic build**: if a user accidentally runs `ix service install` from `$HOME` (pre-`8bacb48` fix), the daemon indexes the entire home directory

---

## Fix Requirements

### Phase 1 — Stop the overwrite (no breaking change)

Make `ix service install` append to the existing unit file rather than overwrite. The systemd unit file `ExecStart=` directive can appear multiple times when `Type=oneshot` — or better, switch the unit to use `ixd` (which properly supports multi-root).

### Phase 2 — Wire `ix --daemon` to `run_many`

Replace the `for path in &paths { daemon::run(path) }` loop (line 351-356) with a single `daemon::run_many(&paths)` call:

```rust
if cli.daemon {
    let paths: Vec<PathBuf> = if cli.path.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        cli.path.clone()
    };
    if let Err(e) = ix::daemon::run_many(&paths) {   // ← run_many, not run() in loop
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
    return;
}
```

### Phase 3 — Service install uses `ixd` binary with multi-root

Generate unit file using `ixd` (which calls `run_many` correctly) with all accumulated paths:

```
ExecStart=/home/user/.cargo/bin/ixd /workspace/ix /workspace/secure_future
```

Or maintain a separate config file (`.ixd.toml` or `/home/user/.config/ixd/roots.toml`) that lists all watched roots, and have the service unit invoke `ixd` against that config.

---

## Verification

```bash
# Reproduce Bug 1 — ix --daemon ignores second path
ix --daemon /workspace/ix /workspace/secure_future &
sleep 2
ls /workspace/ix/.ix/beacon.json        # exists (watched)
ls /workspace/secure_future/.ix/beacon.json  # DOES NOT EXIST (never watched)
kill %1

# Reproduce Bug 2 — service install overwrites
cp ~/.config/systemd/user/ixd.service /tmp/ixd.service.bak
ix service install          # from /workspace/ix
ix service install          # from /workspace/secure_future — OVERWRITES
diff /tmp/ixd.service.bak ~/.config/systemd/user/ixd.service
# Expected: files differ — second install wiped the first

# Fix verification — ixd correctly handles multi-root
ixd /workspace/ix /workspace/secure_future &
sleep 2
ls /workspace/ix/.ix/beacon.json        # exists
ls /workspace/secure_future/.ix/beacon.json  # exists — MULTI-ROOT WORKS
kill %1
```
