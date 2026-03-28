# ix Daemon — Complete the "Not Yet" Column

You are HANDS. Execute these operations IN ORDER.

The core search tool is shipped and working. These 5 features complete the daemon layer — the power-user upgrade that keeps the index fresh automatically.

Use `ix` itself for all code search in this repo:
```bash
ix --build && ix "pattern"
```

## MANDATORY RULES

1. **NEVER use `#[allow(clippy::...)]`** — fix the code, not the warning
2. **Targeted edits only** — do not rewrite files from scratch
3. **Every identifier must EXIST** — verify with `ix` before using
4. **`cargo clippy -- -D warnings`** must pass before committing
5. **Do not touch** any shipped functionality (CLI flags, builder, reader, executor, scanner, planner, bloom, format, trigram, posting, varint, string_pool)

---

## Op 1: Stale Detection

**File**: `src/lib/reader.rs` + `src/bin/ix.rs`

When `ix "pattern"` runs and finds an existing `.ix/shard.ix`, check if the index is stale before using it:

1. Read the index creation timestamp from the header (already stored in `format.rs` Header)
2. Walk the indexed directory and find the most recently modified file (use `ignore::WalkBuilder` — same as builder)
3. If any file's mtime is newer than the index creation time → print warning to stderr: `ix: index is stale (last built: <time>). Run 'ix --build' to update.`
4. Still use the stale index for the search — don't block. Just warn.

Add a `--fresh` flag that forces a rebuild before searching:
```
--fresh    Rebuild index before searching (equivalent to --build then search)
```

**Verify**: Touch a file (`touch src/lib/lib.rs`), run `ix "pattern"` — should see stale warning.

---

## Op 2: Filesystem Watcher

**File**: `src/lib/watcher.rs` (currently 29-line stub)

Implement a filesystem watcher that detects file changes:

1. Use the `notify` crate (add to Cargo.toml: `notify = "7"`)
2. Watch the indexed directory recursively
3. On file create/modify/delete events:
   - Collect changed file paths into a `Vec<PathBuf>`
   - Debounce: wait 500ms after last event before processing (use `notify-debouncer-mini` or manual timer)
4. Expose a public API:
   ```rust
   pub struct Watcher {
       // ...
   }
   impl Watcher {
       pub fn new(root: &Path) -> Result<Self>;
       pub fn start(&mut self) -> Result<Receiver<Vec<PathBuf>>>;
       pub fn stop(&mut self);
   }
   ```
5. The watcher does NOT rebuild the index itself — it sends changed paths to whoever is listening (the daemon)

**Verify**: Unit test — create a temp dir, start watcher, create a file, assert event received.

---

## Op 3: Incremental Delta Index

**File**: `src/lib/builder.rs`

Add an `update` method alongside the existing `build`:

1. `build()` — full rebuild (existing, unchanged)
2. `update(changed_files: &[PathBuf])` — incremental update:
   - For each changed file: re-extract trigrams, update posting lists
   - For deleted files: remove from file table and posting lists
   - For new files: add to file table and posting lists
   - Write updated index to disk (atomic rename to prevent corruption)

The existing format supports this because:
- File table has file IDs — can be looked up by path
- Posting lists reference file IDs — can be filtered
- Sparse trigram table — can be rebuilt from modified posting lists

If incremental is too complex for the current format, implement a simpler approach:
- Keep a `dirty_files` set
- On update: full rebuild but only re-scan dirty files (skip unchanged files using mtime comparison)
- This is "incremental by avoidance" — not true delta, but 10x faster than full rebuild for single-file changes

**Verify**: Build index, modify one file, run `update([modified_file])`, search — new content found.

---

## Op 4: Dormancy Detection

**File**: `src/lib/idle.rs` (currently 22-line stub)

Implement idle/dormancy detection for the daemon:

1. Track when the last search query was received
2. Track when the last file change event was received
3. Define dormancy states:
   - `Active` — query or change within last 5 minutes
   - `Idle` — no activity for 5-30 minutes
   - `Dormant` — no activity for 30+ minutes
4. When dormant:
   - Stop the filesystem watcher (save CPU/battery)
   - Keep the index in memory (mmap — OS handles paging)
5. Wake on next query — restart watcher

```rust
pub enum DaemonState {
    Active,
    Idle,
    Dormant,
}

pub struct IdleTracker {
    last_query: Instant,
    last_change: Instant,
}

impl IdleTracker {
    pub fn new() -> Self;
    pub fn record_query(&mut self);
    pub fn record_change(&mut self);
    pub fn state(&self) -> DaemonState;
}
```

**Verify**: Unit test — create tracker, assert Active, advance time, assert Idle, advance more, assert Dormant.

---

## Op 5: Daemon (ixd)

**File**: `src/bin/ixd.rs` (currently 8-line stub)

Wire everything together into the daemon binary:

1. Parse args: `ixd [PATH]` — directory to watch (default: `.`)
2. Build initial index (full build)
3. Start filesystem watcher on the directory
4. Start idle tracker
5. Main loop:
   ```
   loop {
       select! {
           changed_files = watcher.recv() => {
               idle.record_change();
               builder.update(&changed_files);
           }
           // Check dormancy every 60 seconds
           _ = sleep(60s) => {
               match idle.state() {
                   Dormant => watcher.stop(),
                   Active | Idle => { /* keep watching */ }
               }
           }
       }
   }
   ```
6. On SIGTERM/SIGINT: clean shutdown, stop watcher
7. Print status on startup: `ixd: watching <path> (N files indexed, M trigrams)`

IPC protocol is NOT in this task — ixd just keeps the index fresh. `ix` CLI reads the index file directly (mmap). No socket communication needed yet.

**Verify**: Run `ixd .` in background, modify a file, wait 1 second, run `ix "new_content"` — should find the new content without manual `--build`.

---

## Op 6: Verify Everything

```bash
cargo check
cargo test
cargo clippy -- -D warnings
cargo build --release
cargo install --path . --force

# Stale detection
cd /workspace/charlie && rm -rf .ix && ix --build
touch core/kernel/src/main.rs
ix "ControlPlane" 2>&1 | head -1
# Should show stale warning on stderr

# Fresh flag
ix --fresh "ControlPlane" | head -3
# Should rebuild then search

# Daemon
cd /workspace/ix
ixd . &
sleep 2
echo "// test content xyz123" >> src/lib/lib.rs
sleep 2
ix "xyz123"
# Should find the new content
kill %1
git checkout src/lib/lib.rs

# Full test suite
cargo test
```

Report results. Commit and push.

## BANNED

- `#[allow(clippy::...)]`
- Rewriting shipped modules (builder, reader, executor, scanner, etc.)
- Changing CLI flags that already work
- Adding features not listed
- Touching README unless updating the shipped/not-yet table
