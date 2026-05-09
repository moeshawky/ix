# Post-Mortem: ixd Disk Space Exhaustion

**Date:** 2026-05-03
**Severity:** High — daemon-induced disk exhaustion on systems with constrained storage
**Affected versions:** ix 0.5.0 through 0.5.2
**Status:** Unfixed (as of 0.5.2)

---

## Summary

The `ixd` background daemon can exhaust disk space through a positive feedback loop: each file-change event triggers a full index rebuild, which creates temporary files proportional to the entire codebase size. When disk space is insufficient, the rebuild fails mid-write, leaving temporary artifacts behind. Subsequent rebuilds add more temporary data before cleaning old artifacts. The llmosafe safety pipeline that guards `ixd` is completely blind to disk pressure — it monitors RSS memory, iowait, and load average, but never checks available disk space. A disk-aware escalation API (`decide_with_pressure`) exists in llmosafe but `ixd` never invokes it, instead re-implementing an isolated disk check in the wrong place in the build pipeline.

---

## Timeline

| When | What |
|------|------|
| 0.5.0 (2026-04-29) | Daemon added with `ResourceGuard::auto(0.6)` for safety. Cache layer and `AdaptiveCachePolicy` added. `decide_with_pressure()` API available in llmosafe. |
| 0.5.0 (2026-04-29) | `Builder::free_bytes_at()` guard added inside `serialize()` — checks for ≥100MB free **after** all temp files are already written. |
| 0.5.0 (2026-04-29) | `evaluate_safety()` in ixd calls `policy.decide(raw, 0, false)` — the no-pressure overload. Disk signal never fed into safety pipeline. |
| 0.5.1 (2026-04-29) | All `expect()`/`unreachable!()` removed. Daemon now survives build failures instead of panicking — **unintentionally makes the feedback loop worse** because the daemon stays alive to retry. |
| 0.5.2 (2026-04-29) | Daemon socket hardening (symlink prevention, read timeouts). No changes to build/safety/disk logic. |
| 2026-05-03 | Disk exhaustion reported on a system running `ixd` while an agent generated files rapidly. |

---

## Root Cause

The failure is a **4-factor interaction** between llmosafe's API design, ixd's shallow integration of that API, the builder's error-path hygiene, and the daemon's rebuild architecture.

### Factor 1: Disk space check comes too late

**File:** `src/lib/builder.rs:569-580`

```rust
fn serialize(&mut self) -> Result<PathBuf> {
    // Disk space guard: abort if < 100MB free to avoid partial shard writes
    if let Ok(free) = Self::free_bytes_at(&self.ix_dir) {
        const MIN_FREE: u64 = 100 * 1024 * 1024; // 100 MB
        if free < MIN_FREE {
            return Err(...);
        }
    }
    // ... writes temp files, creates shard.ix.tmp, atomic rename ...
}
```

This check runs **inside** `serialize()`, after `build()` has already:
- Walked the entire file tree
- Written all external sort runs (`shard.ix.run.N`)
- Populated `.tmp.files`, `.tmp.blooms`, `.tmp.strings` with the full index data

By the time the 100MB guard fires, the disk may already be full from the current build's temporary output. The guard prevents writing `shard.ix.tmp` (the final merged index), but all the intermediate files are already on disk.

**Correct placement:** Before `build()` starts any I/O — at the top of `build()`, or better, in `evaluate_safety()` before the daemon decides to proceed.

### Factor 2: No cleanup on build failure

**File:** `src/lib/builder.rs:610-801`

When `serialize()` fails (e.g., disk full at line 574), the `Err` propagates up **before** reaching the cleanup block at lines 791-799:

```rust
// These lines only execute on SUCCESS:
let _ = fs::remove_file(self.ix_dir.join("shard.ix.tmp.files"));   // line 791
let _ = fs::remove_file(self.ix_dir.join("shard.ix.tmp.blooms"));  // line 792
let _ = fs::remove_file(self.ix_dir.join("shard.ix.tmp.strings")); // line 793
for path in &self.temp_runs { ... fs::remove_file(path) ... }       // line 794-798
self.temp_runs.clear();                                              // line 799
```

Additionally, `Builder` has **no `Drop` implementation** — confirmed by searching the codebase. If the Builder is dropped after a failed build (or if ixd crashes), these files persist:

| File | Created by | Cleaned on error? |
|------|-----------|-------------------|
| `shard.ix.tmp.files` | `Builder::new()` (line 143) | No |
| `shard.ix.tmp.blooms` | `Builder::new()` (line 144) | No |
| `shard.ix.tmp.strings` | `Builder::new()` (line 145) | No |
| `shard.ix.tmp` | `serialize()` (line 613) | No (partially written) |
| `shard.ix.bak` | `serialize()` (line 775-783) | No (if rename succeeded but subsequent cleanup didn't) |
| `shard.ix.run.N` | `build()` (external sort) | Partially — next `build()` call cleans at start (lines 234-248) |

The `.tmp.files/blooms/strings` files are overwritten each build (same filename), so they don't accumulate across builds. But `shard.ix.tmp` (partially written final index) and `shard.ix.bak` (old index backup) do accumulate on failure.

**Peak disk overhead during a single rebuild:**

| Artifact | Size relative to final index |
|----------|------------------------------|
| `shard.ix` (old, still exists) | 1× |
| `shard.ix.bak` (backup of old) | 1× |
| `shard.ix.tmp` (new, being written) | 1× |
| `.tmp.files/blooms/strings` | ~0.3–0.5× |
| `shard.ix.run.N` (sort runs) | ~0.5–1× |
| **Total peak** | **~3.5–4.5× the final index size** |

On a system where the shard is 552MB (observed for the training_data collection in this workspace), peak temporary overhead reaches ~2.0–2.5GB. If the partition has less than ~2× the final shard size free, the rebuild cannot complete.

### Factor 3: Safety pipeline is blind to disk pressure

**File:** `src/bin/ixd.rs:312-332`

```rust
fn evaluate_safety(guard: &ResourceGuard) -> (u16, SafetyDecision) {
    match guard.check_blocking() {
        Ok(synapse) => {
            let raw = synapse.raw_entropy();
            let policy = EscalationPolicy::default();
            let decision = policy.decide(raw, 0, false);  // ← NO pressure signal
            (raw, decision)
        }
        Err(e) => { ... }
    }
}
```

The entropy score driving all safety decisions is computed by `ResourceGuard::raw_entropy()` (`llmosafe_body.rs:82-106`):

```
weighted_score = (rss_ratio × 500) + (iowait_ratio × 250) + (load_ratio × 250)
```

| Signal | Weight | Source |
|--------|--------|--------|
| RSS memory | 50% | `/proc/self/statm` |
| IO wait | 25% | `/proc/stat` (delta over 100ms) |
| Load average | 25% | `/proc/loadavg` |
| **Disk free** | **0%** | **Not monitored** |

`EnvironmentalVitals` (`llmosafe_body.rs:14-62`) captures only `iowait: u64` and `load_avg: f64`. No disk-free field exists. `ResourceGuard::pressure()` (`llmosafe_body.rs:108-116`) returns 0–100 based solely on RSS ratio.

The `AdaptiveCachePolicy` in `cache_policy.rs` uses `guard.pressure()` correctly for cache eviction decisions. But `evaluate_safety()` in ixd never calls `pressure()` — it goes through `check_blocking()` → `raw_entropy()` → `decide()`, completely bypassing the pressure path.

### Factor 4: The `decide_with_pressure()` bypass

This is the key architectural disconnect. llmosafe **does** provide an API for injecting external pressure signals:

**File:** `llmosafe/src/llmosafe_integration.rs:274-290`

```rust
pub fn decide_with_pressure(
    &self,
    entropy: u16,
    surprise: u16,
    has_bias: bool,
    pressure: PressureLevel,     // ← Nominal / Elevated / Critical / Emergency
) -> SafetyDecision {
    if pressure >= self.escalate_pressure {  // Default: Critical (60%)
        return SafetyDecision::Escalate {    // ← Would block the rebuild!
            entropy,
            reason: EscalationReason::ResourcePressure,
            cooldown_ms: 0,
        };
    }
    self.decide(entropy, surprise, has_bias)
}
```

`PressureLevel::from_percentage(u8)` maps 0–100 to `Nominal/Elevated/Critical/Emerergency`:

| Percentage | Level |
|------------|-------|
| 0–25 | Nominal |
| 26–50 | Elevated |
| 51–75 | Critical |
| 76–100 | Emergency |

If `ixd` had done this:

```rust
// What evaluate_safety() SHOULD do:
let disk_pct = disk_used_percentage(&ix_dir);  // e.g., 95% full → 95
let disk_pressure = PressureLevel::from_percentage(disk_pct);
let decision = policy.decide_with_pressure(raw, 0, false, disk_pressure);
// At 95% full → Emergency → Escalate → rebuild skipped
```

...the safety pipeline **would have caught the disk exhaustion** before it happened. The `Escalate` decision causes `handle_changes()` to return early (line 252), skipping the rebuild entirely.

But instead, `evaluate_safety()` calls `policy.decide(raw, 0, false)` — the zero-surprise, no-pressure overload. The `PressureLevel` parameter is never computed. The disk-aware API exists but is unused.

---

## The Positive Feedback Loop

The four factors combine into a self-reinforcing cycle:

```
Agent writes files
       │
       ▼
Watcher fires (500ms debounce)
       │
       ▼
evaluate_safety() → Proceed (disk invisible)
       │
       ▼
builder.update() → builder.build() (FULL rebuild, not incremental)
       │
       ▼
Temp files written: .tmp.files/blooms/strings + sort runs + shard.ix.tmp
       │
       ▼
Disk fills mid-build
       │
       ▼
serialize() fails at free_bytes_at() check (too late)
       │
       ▼
Err propagated — NO cleanup of temp files (Factor 2)
       │
       ▼
ixd logs "update failed — retrying on next change" (line 298)
       │
       ▼
Beacon set to "idle" (line 305) — daemon appears healthy
       │
       ▼
Agent writes more files → watcher fires again
       │
       ▼
evaluate_safety() → Proceed (disk still invisible)
       │
       ▼
build() cleans old run files (recovers some space)
but immediately writes NEW temp files (same or larger)
       │
       ▼
serialize() fails again → more artifacts accumulate
       │
       ▼
Less free space → next failure more likely → loop accelerates
```

The v0.5.1 fix that removed `expect()`/`unreachable!()` made this **worse** in one specific way: previously, a panic during build would crash `ixd` entirely, breaking the loop. Now with proper `Result` propagation, the daemon survives the failure, sets status to "idle", and remains in the loop ready to trigger another rebuild on the next change event. This is architecturally better (no panics in production) but exposes the feedback loop that was previously masked by crashes.

---

## Why `ix` (CLI) Doesn't Have This Problem

`ix --build` is a one-shot operation:

1. Builds once and exits
2. No loop, no watcher, no repeated rebuild cycle
3. If `serialize()` fails, the user sees the error and can free space manually
4. No daemon process to keep retrying

The `ix` CLI also doesn't use `ResourceGuard::check_blocking()` for safety decisions — it only reads `current_rss_bytes()` for a lightweight cache policy hint (`ix.rs:687-688`). There's no safety escalation loop to bypass.

---

## The `cooldown_ms: 0` Non-Throttle

Every `SafetyDecision::Escalate` produced by `EscalationPolicy::default()` has `cooldown_ms: 0`:

**File:** `llmosafe/src/llmosafe_integration.rs`

| Condition | Decision | cooldown_ms |
|-----------|----------|-------------|
| Bias detected | Escalate | **0** (line 239) |
| Entropy ≥ 800 (escalate threshold) | Escalate | **0** (line 251) |
| Surprise ≥ 500 (escalate threshold) | Escalate | **0** (line 263) |
| Pressure ≥ Critical | Escalate | **0** (line 286) |
| Entropy ≥ 1000 (halt threshold) | Halt | **0** (line 245) |

In `ixd.rs:251`:
```rust
std::thread::sleep(Duration::from_millis(u64::from(*cooldown_ms)));
// sleep(0ms) = no backpressure
```

Even if `decide_with_pressure()` were called and returned `Escalate`, the daemon would sleep for 0ms and return immediately — then re-enter the main loop on the next 500ms tick and try again. The `Warn` path at least has a 300ms cooldown (`WARN_COOLDOWN_MS`), but escalation has none.

The `check_blocking()` method in `ResourceGuard` (`llmosafe_body.rs:151-175`) has the same problem — it loops calling `policy.decide()` and sleeping `cooldown_ms` between iterations, but since cooldown is always 0, the loop effectively spin-waits until entropy drops.

---

## The `update()` Lie

**File:** `src/lib/builder.rs:379-381`

```rust
pub fn update(&mut self, _changed_files: &[PathBuf]) -> Result<PathBuf> {
    self.build()  // Full rebuild — _changed_files is UNUSED
}
```

The API promises incremental update but delivers a full rebuild. This amplifies disk I/O by the codebase-size factor: changing 1 file in a 10,000-file codebase triggers I/O proportional to all 10,000 files, not just the 1 that changed. This is not the root cause of the feedback loop, but it's the primary reason the loop is so expensive per iteration.

---

## Version Mismatch: crates.io vs Local

**File:** `Cargo.toml:56`

```toml
llmosafe = "0.5"  # Resolves to latest 0.5.x from crates.io
```

The local llmosafe source at `/workspace/llmosafe/` is version `0.5.4`, but `ix` pulls from crates.io with the `"0.5"` semver range. If a disk-space fix were added to llmosafe locally and published as 0.5.5, `ix` would pick it up automatically. But if the fix required API changes (e.g., adding a `disk_pressure()` method to `ResourceGuard`), those would need a minor version bump (0.6.0) and `ix` would not pick them up without updating the dependency constraint.

As of the audit date, neither the local nor the published version of llmosafe has any disk-awareness in `EnvironmentalVitals` or `ResourceGuard`.

---

## Recommended Fixes

### Priority 1 — Pre-build disk check (prevents the loop from starting)

Add a disk space check at the **start** of `build()`, before any temp files are written:

```rust
// builder.rs, top of build():
pub fn build(&mut self) -> Result<PathBuf> {
    // Disk space guard: check BEFORE writing temp files
    if let Ok(free) = Self::free_bytes_at(&self.ix_dir) {
        // Estimate: peak overhead is ~3x final shard size.
        // Use 2x existing shard as minimum, or 200MB if no shard exists yet.
        let existing_shard_size = fs::metadata(self.ix_dir.join("shard.ix"))
            .map(|m| m.len())
            .unwrap_or(0);
        let min_free = if existing_shard_size > 0 {
            existing_shard_size * 2
        } else {
            200 * 1024 * 1024 // 200 MB
        };
        if free < min_free {
            return Err(Error::Io(std::io::Error::other(format!(
                "insufficient disk space: {} MB free, need ≥{} MB \
                 (path: {})",
                free / 1024 / 1024,
                min_free / 1024 / 1024,
                self.ix_dir.display(),
            ))));
        }
    }
    // ... existing build logic ...
}
```

### Priority 2 — Implement `Drop` for `Builder` (prevents artifact accumulation)

```rust
impl Drop for Builder {
    fn drop(&mut self) {
        for path in &self.temp_runs {
            let _ = fs::remove_file(path);
        }
        let ix = &self.ix_dir;
        let _ = fs::remove_file(ix.join("shard.ix.tmp.files"));
        let _ = fs::remove_file(ix.join("shard.ix.tmp.blooms"));
        let _ = fs::remove_file(ix.join("shard.ix.tmp.strings"));
        let _ = fs::remove_file(ix.join("shard.ix.tmp"));
        // Don't remove shard.ix.bak — it's a valid backup of the last good index
    }
}
```

### Priority 3 — Wire disk pressure into llmosafe's safety pipeline

```rust
// ixd.rs, evaluate_safety():
fn evaluate_safety(guard: &ResourceGuard, ix_dir: &Path) -> (u16, SafetyDecision) {
    match guard.check_blocking() {
        Ok(synapse) => {
            let raw = synapse.raw_entropy();
            let policy = EscalationPolicy::default();

            // Map disk usage to pressure level
            let disk_pressure = match Self::disk_pressure(ix_dir) {
                Some(pct) => PressureLevel::from_percentage(pct),
                None => PressureLevel::Nominal, // unknown → don't block
            };

            let decision = policy.decide_with_pressure(raw, 0, false, disk_pressure);
            (raw, decision)
        }
        Err(e) => { ... }
    }
}

fn disk_pressure(ix_dir: &Path) -> Option<u8> {
    // statvfs to get used%
    #[cfg(target_os = "linux")]
    {
        let c_path = std::ffi::CString::new(ix_dir.as_os_str().as_bytes()).ok()?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
            return None;
        }
        let total = stat.f_blocks * stat.f_frsize;
        let avail = stat.f_bavail * stat.f_frsize; // user-available
        if total == 0 { return None; }
        let used_pct = ((total - avail) * 100 / total) as u8;
        Some(used_pct)
    }
    #[cfg(not(target_os = "linux"))]
    None
}
```

### Priority 4 — Set meaningful `cooldown_ms` in `EscalationPolicy`

```rust
// In llmosafe_integration.rs, change all cooldown_ms: 0 to meaningful values:
SafetyDecision::Escalate { cooldown_ms: 5000, ... }  // 5s throttle
SafetyDecision::Halt(_, 30000)                        // 30s pause
```

Or have `ixd` use a custom `EscalationPolicy` instead of `default()`:

```rust
let policy = EscalationPolicy::new()
    .with_escalate_entropy(800)
    .with_halt_entropy(1000);
// Then manually apply a minimum cooldown in handle_changes()
```

### Priority 5 — Implement incremental `update()`

This eliminates the per-change I/O amplification. Each file change would update only the relevant posting lists, bloom filter entries, and string pool entries — not rebuild from scratch. This is a major engineering effort but would reduce per-rebuild disk overhead from O(codebase) to O(changed files).

---

## Lessons Learned

1. **Safety APIs must be used at their full depth, or they provide a false sense of security.** `ixd` integrated llmosafe at the shallowest level (`decide()` with no pressure signal) while the deeper API (`decide_with_pressure()`) was designed for exactly this scenario. The daemon "felt safe" because it was calling llmosafe — but it wasn't feeding llmosafe the signal that mattered.

2. **A late guard is worse than no guard.** The `free_bytes_at()` check in `serialize()` creates a false assurance that disk space is handled. Because it runs after temp files are already written, it catches the problem too late to prevent it. A system with no guard but a clear "check before you write" convention would be safer than a guard that checks after.

3. **Error paths are load-bearing.** The cleanup block at `builder.rs:791-799` only runs on success. Every `?` between the disk check and the cleanup is a point where artifacts can leak. The absence of a `Drop` impl means there's no safety net.

4. **Removing panics can expose latent loops.** The v0.5.1 fix that replaced `expect()` with `Result` propagation was correct — panics are unacceptable in a daemon. But it unmasked a feedback loop that was previously self-limiting (crash → loop broken). When you remove a crash, you must also remove the condition that made the crash the only exit.

5. **`cooldown_ms: 0` is not a throttle.** Every escalation path in `EscalationPolicy::default()` produces `cooldown_ms: 0`. The `sleep(0ms)` in `ixd.rs:251` is a no-op. If you're going to escalate, the cooldown must be non-zero, or the escalation is purely advisory (it returns early this iteration but retries immediately next iteration).

6. **API surface that is unused is untested.** `decide_with_pressure()` has unit tests in llmosafe, but because no consumer actually calls it with a real disk pressure signal, the integration gap wasn't caught. Consider a "lighthouse" integration test that verifies a consumer uses the full API.

---

## Appendix A: Artifact Inventory

Complete list of files created during a single `build()` cycle and their cleanup behavior:

| File | When created | Size | Cleaned on success | Cleaned on error | Cleaned on Drop |
|------|-------------|------|--------------------|-----------------|----------------|
| `shard.ix.tmp.files` | `Builder::new()` | O(files × path_len) | Yes (line 791) | **No** | **No** |
| `shard.ix.tmp.blooms` | `Builder::new()` | O(files × bloom_size) | Yes (line 792) | **No** | **No** |
| `shard.ix.tmp.strings` | `Builder::new()` | O(string_pool) | Yes (line 793) | **No** | **No** |
| `shard.ix.run.N` | `build()` (sort) | O(trigrams) | Yes (lines 794-798) | **No** | **No** |
| `shard.ix.merged.N.T` | `serialize()` (merge) | O(trigrams) | Yes (lines 794-798) | **No** | **No** |
| `shard.ix.tmp` | `serialize()` (final) | O(final_index) | Renamed to `shard.ix` | **No** (partial) | **No** |
| `shard.ix.bak` | `serialize()` (swap) | O(old_index) | Yes (line 788) | **No** | **No** |

**Note:** `.tmp.files/blooms/strings` are overwritten on each build (same filename), so they don't accumulate across builds. But `shard.ix.tmp` and `shard.ix.bak` do.

## Appendix B: llmosafe API Usage Comparison

| API | `ixd` (daemon) | `ix` (CLI) | `AdaptiveCachePolicy` |
|-----|---------------|-----------|----------------------|
| `ResourceGuard::auto()` | Yes (0.6) | No | Yes (0.5) |
| `check_blocking()` | Yes | No | No |
| `raw_entropy()` | Yes (via check_blocking) | No | No |
| `pressure()` | **No** | Yes (lightweight, line 687) | Yes (directive, line 107) |
| `decide()` | Yes | No | No |
| `decide_with_pressure()` | **No** | No | No |
| `EscalationPolicy::default()` | Yes | No | No |
| `current_rss_bytes()` | No | Yes (line 687) | No |
| `EnvironmentalVitals` | Never constructed directly | No | No |

The cache layer uses `pressure()` correctly. The daemon uses `check_blocking()` but never feeds disk signals into the decision. The CLI doesn't use the safety pipeline at all (correct — it's one-shot).
