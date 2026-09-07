# moeix Project Roadmap
*The living plan that gates scope creep and guides controlled evolution.*

## 1. Intent Statement

> **"Sub-millisecond sparse-trigram code search with zero-warning clippy discipline, background daemon watching across multiple roots, and bounded resource consumption without systemd lock-in."**

---

## 2. Current State & The Gap

### Current State
- High performance trigram search engine in Rust with sub-50μs CDX lookups and 88% delta compression.
- Dedicated multi-root background daemon binary `ixd` using native Unix double-fork detachment.
- CLI wrapper `ix service` tied to systemd user services.

### The Gap
1. **Perpetual indexing & resource runaway**: `.codegraph` directory is omitted from built-in exclusion lists in watcher and builder, causing a 30-second loop on active repos (109% CPU, 32MB delta on `codegraph-ferrari`).
2. **Config schema mismatch**: 6 of 8 repositories configure `.ixd.toml` with nested `[watch]` and `[build]` tables which `serde` silently drops, ignoring user exclusions.
3. **CLI daemon loop stall**: `ix --daemon` calls `run()` in a sequential loop, blocking forever on the first path and ignoring any additional paths passed.
4. **Service lock-in**: `ix service` relies on `systemctl --user`, failing completely on non-systemd environments.

---

## 3. Planned Phases & Execution Results

### Phase 1: HOT-FIX — `.codegraph` Built-in Exclusion [COMPLETED]
- **Goal**: Halt the perpetual indexing feedback loop and runaway CPU usage.
- **Targets**:
  - `src/lib/watcher.rs`: Added `.codegraph` to directory noise filter.
  - `src/lib/builder.rs`: Added `.codegraph` to index walker directory filter.
  - `src/lib/config.rs`: Added `.codegraph` to `Config::default()` exclude list.
- **Success Criteria**: `ixd` CPU drops from 109% to <5% at idle; `codegraph-ferrari` delta stops accumulating.
- **Verified Result**: `ixd` instantaneous CPU dropped to **0.0%**; RSS dropped from 317MB to 20MB; all 8 roots report `"status": "idle"`.

### Phase 2: CONFIG SCHEMA — Dual Flat/Nested Compatibility [COMPLETED]
- **Goal**: Restore user exclusions without breaking existing configurations.
- **Targets**:
  - `src/lib/config.rs`: Added `WatchSection` and `BuildSection` with backward-compatible deserialization.
  - Normalized nested tables into canonical fields with `tracing::warn` on legacy formats.
  - Added unit test `test_config_nested_table_compatibility`.
- **Success Criteria**: 8 of 8 repository `.ixd.toml` files parse correctly with user ignore patterns honored; 0 silently dropped fields.
- **Verified Result**: 100% test pass across workspace (121 unit tests, 9 boundary, 13 proptests).

### Phase 3: MULTI-ROOT FIX — `ix --daemon` Parity [COMPLETED]
- **Goal**: Fix multi-root handling in `ix` CLI.
- **Targets**:
  - `src/bin/ix/main.rs`: Replaced sequential loop over `ix::daemon::run` with `ix::daemon::run_many(&paths)`.
- **Success Criteria**: `ix --daemon /path1 /path2` monitors both paths concurrently without blocking on path 1.
- **Verified Result**: Clippy strict zero-warnings; API parity with standalone `ixd`.

### Phase 4: CLI UNIFICATION — `ix service` Native Aliases & `ixd --stop` [COMPLETED]
- **Goal**: Provide seamless non-systemd service control via `ix service` delegating directly to `ixd` and add native `ixd --stop`.
- **Targets**:
  - `src/bin/ixd.rs`: Added `stop: bool` flag and `stop_daemons()` using live beacon discovery and clean SIGTERM signaling.
  - `src/bin/ix/args.rs`: Updated `ServiceAction::Start`, `Stop`, and `Restart` to accept optional target path arguments.
  - `src/bin/ix/service.rs`: Replaced systemd invocation with direct `ixd` delegation (`ix service start` -> `ixd --daemon`, `ix service stop` -> `ixd --stop`, `ix service restart` -> `ixd --stop` + `ixd --daemon`).
- **Success Criteria**: `ix service start/stop/restart/status` work natively without systemd; `ixd --stop` stops running daemons reliably.
- **Verified Result**: Verified live start, stop, restart, and status cycles without systemd. 0 errors, 0 clippy warnings.

---

## 4. Scope-Creep Defenses

### In Scope
- File system watcher and builder directory exclusion filters.
- `.ixd.toml` deserialization and normalization.
- CLI argument routing to `run_many`.
- Deprecation messaging in `ix service`.
- Documentation updates in `docs/DAEMON-RUNBOOK.md` and `AGENTS.md`.

### Out of Scope (BANNED)
- Modifying trigram extraction algorithms, CDX page formats, or posting serialization.
- Adding any `#[allow(...)]` attributes (strict zero-allow rule).
- Modifying human-written `///` or `//` docstrings (DNA).
- Touching `unsafe` blocks.
- Rewriting `IdleTracker` or `ResourceGuard` memory arbitration.

---

## 5. Controlled-Evolution Path
- Any enhancement beyond the 4 phases requires explicit operator request and a roadmap update.
- If a future phase proposes modifying posting formats, it must be gated behind a new RFC.

---

## 6. Drift-Check Checkpoints
- Re-read Intent Statement every 10 actions.
- Ask: "Am I making code correct or realizing intent?"
- Run `cargo clippy --workspace --all-targets -- -D warnings` before every commit/verification milestone.
