# ix Bug Fixes — 3 Issues from Stress Testing

You are HANDS. Execute these operations IN ORDER. Do not add anything not listed.

## MANDATORY RULES (read before touching any code)

1. **NEVER use `#[allow(clippy::...)]`** to silence warnings. If clippy complains about too many arguments, create a struct. If it complains about complexity, simplify the code. Silencing warnings is BANNED.
2. **NEVER use `file_write` (full file rewrite)**. Use targeted edits only. If you need to change 5 lines, change 5 lines — don't rewrite the file.
3. **Every identifier you reference must EXIST** in the codebase. Before using a method or type, verify it exists with `ix` or grep. Do not hallucinate APIs.
4. **`use std::io::Write;`** — if you touch a file that calls `.write_all()`, verify this import exists. You forgot it last time.
5. **Run `cargo clippy -- -D warnings` BEFORE committing.** If it fails, fix the ROOT CAUSE, not the warning.

---

## Bug 1: Context line duplication

**Symptom**: When two matches are within N lines of each other, context lines repeat.
```
core/kernel/src/control_plane.rs:118:- :        let mut ctx = HookContext::pre_tool(
core/kernel/src/control_plane.rs:118:- :        let mut ctx = HookContext::pre_tool(
```
Line 118 appears twice because match at line 117 has context +1 (shows 118), and match at line 119 has context -1 (also shows 118).

**Fix**: In the context rendering logic (likely in `src/bin/ix.rs` or `src/lib/executor.rs`), track which lines have already been printed for each file. Use a `HashSet<u32>` of printed line numbers per file. Before printing a context line, check if it's already been printed. If yes, skip it.

**Verify**: `ix -C 1 "validate_input" core/kernel/src/control_plane.rs` should NOT show any line number twice.

---

## Bug 2: 44MB index for a 2K LOC project

**Symptom**: `/workspace/ix/` (1,871 lines of Rust) produces a 44MB `.ix/shard.ix` index.

**Diagnose first**: Before fixing, find out WHY.
```bash
# What did ix actually index?
ix -l "" .  # list ALL files in index (empty string matches everything)
# Or check if Cargo.lock, .git, or target/ leaked in
ix -l "Cargo" .
```

If Cargo.lock (which can be huge) or other non-source files are indexed, the builder's `.gitignore` or `.ixignore` filtering has a gap. The `.gitignore` in the ix repo should contain `/target/` and `Cargo.lock` — verify they're being respected.

If the files are correct but the index is still 44MB, check the sparse trigram table implementation — it may still be writing empty slots.

**Fix**: Whatever the root cause is. Do NOT guess — diagnose first, then fix.

**Verify**: After fix, `cd /workspace/ix && rm -rf .ix && ix --build && ls -lh .ix/shard.ix` should produce an index <5MB for a 2K LOC project.

---

## Bug 3: Max results message says "3+" instead of actual count

**Symptom**: `ix -n 3 "fn "` shows:
```
ix: showing 3 of 3+ matches (use -n 0 for all)
```

The "3+" is vague. The executor knows when it stopped early (because it hit the cap), but it doesn't know the total. That's fine — but the message should be clearer.

**Fix**: Change the message from `"3+"` to:
```
ix: output capped at 3 results (use -n 0 for all)
```

This is honest — we capped the output, we don't know the total, and we tell the user how to get all results.

**Verify**: `ix -n 5 "fn "` should show `ix: output capped at 5 results (use -n 0 for all)` on stderr.

---

## Bug 4 (from previous round): Missing `use std::io::Write` in tests

**Already fixed by the puppeteer session**, but verify it's committed. If `tests/fault_injection.rs` line 1-2 doesn't have `use std::io::Write;`, add it.

---

## Verification (run ALL of these)

```bash
cargo check
cargo test
cargo clippy -- -D warnings
cargo build --release
cargo install --path . --force

# Bug 1: no duplicate context lines
cd /workspace/charlie && rm -rf .ix && ix --build
ix -C 1 "validate_input" core/kernel/src/control_plane.rs | sort | uniq -d
# Should output NOTHING (no duplicate lines)

# Bug 2: small project index
cd /workspace/ix && rm -rf .ix && ix --build
ls -lh .ix/shard.ix
# Should be <5MB

# Bug 3: clear cap message
cd /workspace/charlie
ix -n 5 "fn " 2>&1 | grep "capped"
# Should show: ix: output capped at 5 results (use -n 0 for all)

# Bug 4: tests compile
cargo test --test fault_injection
```

Commit the fix for `tests/fault_injection.rs` (the Write import) along with everything else. Push to remote.

## BANNED

- `#[allow(clippy::...)]` — fix the code, not the warning
- Rewriting files from scratch
- Adding features not listed
- Touching README, DESIGN.md, AGENTS.md
- Changing any working functionality
