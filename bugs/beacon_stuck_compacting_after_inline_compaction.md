# Bug: Beacon status stuck at "compacting" after inline handle_changes compaction

**Labels:** bug, daemon, beacon, compaction
**Severity:** medium (cosmetic for health reporting; no data loss)
**Component:** `src/lib/daemon.rs` — `handle_changes` inline compaction path
**Affects:** moeix 0.11.7 (installed), 0.12.6, 0.12.8 (source verified)

---

## Description

When `ixd` triggers compaction via the **inline path** inside `handle_changes`
(delta > 0 and (daemon idle/Dormant or delta > 50MB)), the beacon status is
set to `"compacting"` before `builder.build()` but is **never reset to
`"idle"`** after the build completes. The beacon remains permanently stuck at
`"compacting"` even though the compaction itself succeeded.

The standalone `compact()` function (idle-window path, triggered after 30s of
daemon inactivity) **does** correctly reset the beacon to `DaemonStatus::Idle`
after building — the two compaction paths have an asymmetry.

## Reproduction

1. Run ixd with a watched repo that has accumulated file changes (delta file
   grows).
2. Wait for the daemon to become Dormant (30s idle) — or accumulate > 50MB of
   delta.
3. ixd triggers inline compaction via `handle_changes`.
4. Observe `.ix/beacon.json`: `status` remains `"compacting"` indefinitely,
   even though `shard.ix` has been rebuilt (mtime updated) and the delta file
   has been consumed.
5. `ix.service_status()` returns `{"status": "compacting", ...}` forever.

**Observed on codegraph-ferrari (2026-08-04):**
- `beacon.json` `status`: `"compacting"`, `last_event_at`: 14:54:01
- `shard.ix` mtime: 14:54:05 (build completed 4s after beacon write)
- `shard.ix.delta`: absent (consumed by compaction)
- `ixd` CPU: 0.3%, sleeping — compaction already finished
- Beacon stuck for 26+ minutes with no further update

## Expected Behavior

After `builder.build()` succeeds (or fails), the beacon status should be reset
to `DaemonStatus::Idle` (or an appropriate error status) and written back to
`beacon.json`. This matches the behavior of the `compact()` idle-window path.

## Root Cause

In `src/lib/daemon.rs`, the `handle_changes` function (inline compaction path,
around line 756 in 0.12.8) sets `ctx.beacon.status = "compacting"` and writes
the beacon before calling `ctx.builder.build()`. After the build completes
(Ok or Err), the function drains deferred batches and calls `ctx.idle.record_change()`
— but **never resets `ctx.beacon.status` back to `"idle"`** and never writes
the beacon again.

Compare with the `compact()` function (idle-window path, around line 834 in
0.12.8) which correctly does:

```rust
beacon.status = "compacting".to_string();
let _ = beacon.write_to(ix_dir);
match builder.build() { ... }
let idle_status = DaemonStatus::Idle;
beacon.status = idle_status.to_string();
let _ = beacon.write_to(ix_dir);  // ← reset happens here
```

The inline path is missing the two lines after the match block.

## Impact

- **Health reporting:** `codegraph_ferrari/ixd_lifecycle.py:124` maps
  `"compacting"` → `root_status = "busy"`. With the stuck beacon, codegraph
  reports the ixd daemon as perpetually "busy" even when idle.
- **MCP timeouts (secondary):** codegraph's `sync_changed_files` previously
  called `idx.rebuild()` on every pending delta under the (now-corrected)
  belief that ixd doesn't auto-compact. This raced ixd's own compaction with
  no file locking in moeix, causing CPU spikes and `-32001` MCP timeouts.
  The codegraph-side fix (gating rebuild on daemon-dead) mitigates this.

## Fix (moeix side)

Add the beacon reset after `builder.build()` in the inline compaction path of
`handle_changes`, mirroring the `compact()` function:

```rust
// After the match builder.build() { ... } block in handle_changes:
let idle_status = DaemonStatus::Idle;
ctx.beacon.status = idle_status.to_string();
let _ = ctx.beacon.write_to(ctx.ix_dir);
if let Some(sock) = ctx.daemon_sock {
    sock.set_status(&idle_status, ctx.builder.files_len());
}
```

## Fix (codegraph side — already applied)

`codegraph_ferrari/ixd_socket.py`: `sync_changed_files` now gates the manual
`idx.rebuild()` behind `not is_daemon_alive(project_root)`. When ixd is alive
it auto-compacts; codegraph no longer races it with a redundant rebuild.
This prevents the concurrent write contention that caused MCP -32001 timeouts.

## Verification

After the moeix fix, trigger compaction (accumulate delta, let daemon go
Dormant) and confirm `beacon.json` `status` returns to `"idle"` within seconds
of `builder.build()` completing.
