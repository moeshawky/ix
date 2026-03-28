# ix LLM-Native Flags — Atomic Operations for Gemini

You are HANDS. You execute atomic operations. You do not reason, improve, or decide.

## Context

`ix` is a trigram-indexed code search tool at `/workspace/ix/`. It works, is installed system-wide, and has --json and --stats flags from the previous round. Now we need 5 new flags that make ix useful for LLM agents (Claude Code, OpenCode, etc.) that call it via subprocess.

## Design Rationale (from productive reasoning)

LLMs use search in 4 patterns. Each needs different output:
1. EXISTENCE CHECK ("how widespread?"): needs match count → `--count`
2. LOCATION ("which files?"): needs file paths only → `--files-only`
3. UNDERSTANDING ("show me context"): needs surrounding lines → `--context N`
4. ENUMERATION ("list all"): needs full results → default (already works)

The #1 failure mode when LLMs use search is CONTEXT FLOODING — a query returns 500+ lines and the LLM loses track of its task. A `--max-results` cap prevents this.

## Operations

Execute IN ORDER. Do not reason about alternatives. Do not refactor unrelated code. Do not add features not listed.

### Op 1: Add `--count` flag (`-c`)

File: `src/bin/ix.rs` (CLI args struct + output logic)

Add a `--count` / `-c` flag. When set:
- Do NOT print individual matches
- Print only the total match count as a single integer to stdout
- Example: `ix -c "TODO"` → `14`
- Combinable with `--type` for scoped counting
- When combined with `--json`: output `{"count": 14}`

Performance: short-circuit in executor — don't collect line content, only count. This requires touching `src/lib/executor.rs` to support a count-only mode that skips line content extraction.

### Op 2: Add `--files-only` flag (`-l`)

File: `src/bin/ix.rs` (CLI args struct + output logic)

Add a `--files-only` / `-l` flag. When set:
- Print only unique file paths, one per line, deduplicated
- Do NOT print line numbers or content
- Example: `ix -l "HookEngine"` → one file path per line
- When combined with `--json`: output `{"files": ["path1", "path2"]}`

Implementation: collect file paths into a `HashSet<PathBuf>` before printing.

### Op 3: Add `--context` flag (`-C N`)

File: `src/bin/ix.rs` + `src/lib/executor.rs` + `src/lib/scanner.rs`

Add a `--context N` / `-C N` flag (N: usize, default 0). When set:
- Print N lines before and N lines after each match
- Separate groups of matches with `--` on its own line (grep convention)
- Context lines are prefixed with `-` (grep convention): `-  surrounding line`
- Match lines prefixed with `:` as they are now
- When combined with `--json`: add `"context_before": ["line1", "line2"]` and `"context_after": ["line1", "line2"]` to each match object

Implementation notes:
- After finding a match on line L, read lines L-N through L+N from the file
- Overlapping contexts (matches within N lines of each other) should merge — don't repeat lines
- This requires the match to remember which file it came from and re-read the file for context (or keep the mmap'd data accessible during output)

### Op 4: Add `--max-results` flag (`-n N`)

File: `src/bin/ix.rs` + `src/lib/executor.rs`

Add a `--max-results N` / `-n N` flag (N: usize, default 100). When set:
- Stop collecting matches after N results
- After output, if more results exist, print to STDERR: `ix: showing N of M+ matches (use -n 0 for all)`
- N=0 means unlimited (no cap)
- Default 100 is critical — this prevents LLMs from accidentally flooding their context

Implementation: in the executor, check result count after each match and stop early when cap is reached. Pass the total-so-far count back so the CLI can print the truncation warning.

### Op 5: Add `--type` flag (`-t T`)

File: `src/bin/ix.rs` + `src/lib/executor.rs` + `src/lib/scanner.rs` + `src/lib/builder.rs` (reader needs to filter by extension)

Add a `--type T` / `-t T` flag (T: string, repeatable). When set:
- Only return matches from files with matching extension
- Supported types map to extensions:
  - `rs` → `.rs`
  - `py` → `.py`
  - `ts` → `.ts`
  - `js` → `.js`
  - `go` → `.go`
  - `c` → `.c`
  - `cpp` → `.cpp`, `.cc`, `.cxx`
  - `h` → `.h`, `.hpp`
  - `md` → `.md`
  - `toml` → `.toml`
  - `yaml` → `.yaml`, `.yml`
  - `json` → `.json`
- Multiple `--type` flags are OR'd: `ix -t rs -t py "pattern"` matches .rs OR .py files
- Unknown type string → treat as literal extension (e.g., `-t txt` → `.txt`)

Implementation: during the verify phase (when reading candidate files), check file extension before opening. Skip files that don't match. For the indexed path, this filters AFTER posting list intersection but BEFORE file I/O — saving time.

### Op 6: Update `--help` for LLMs

File: `src/bin/ix.rs`

Update the help text to include a section specifically for LLM/agent usage:

```
LLM AGENT USAGE:
    Quick count:     ix -c "pattern"              → "14"
    Find files:      ix -l "pattern"              → file paths
    With context:    ix -C 3 "pattern"            → ±3 lines around matches
    Rust files only: ix -t rs "pattern"           → only .rs files
    JSON output:     ix --json "pattern"          → machine-parseable
    Safe default:    ix "pattern"                 → max 100 results
    All results:     ix -n 0 "pattern"            → unlimited
```

### Op 7: Verify

After all operations:

```bash
cargo check
cargo test
cargo clippy -- -D warnings
cargo build --release
cargo install --path . --force
```

Then test on /workspace/charlie (it already has an .ix index):

```bash
# Rebuild with current ix
cd /workspace/charlie && rm -rf .ix && ix --build

# Count
ix -c "fn " && echo "---" && ix -c -t rs "fn "

# Files only
ix -l "ControlPlane" | head -5

# Context
ix -C 2 "validate_input" core/kernel/src/control_plane.rs

# Max results (should cap at 5)
ix -n 5 "fn "

# Type filter
ix -t rs "HookEngine" | wc -l && ix -t py "HookEngine" | wc -l

# JSON + context combo
ix --json -C 1 -t rs "Verdict" core/ | head -3

# Verify default cap works (should show 100 max)
ix "fn " 2>&1 | wc -l
```

Report:
- Which operations succeeded / failed (with full error output)
- All verification test results
- Commit and push to remote

## BANNED

- Writing code not specified above
- Rewriting files from scratch (use targeted edits)
- Adding functions, structs, or modules not listed
- Changing any line not mentioned in the operations
- "Improving" or "cleaning up" anything
- Modifying tests that already pass (add new tests for new features)
- Touching `DESIGN.md`, `AGENTS.md`, or `README.md`
- Changing the existing --json and --stats behavior (extend, don't break)
