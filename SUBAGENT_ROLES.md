# Subagent Role Assignments — ix

*4 specialized subagents, each with distinct responsibilities. Role-specific models below.*

---

## Overview

| Agent | Role | Temperature | Primary Skills | When to Dispatch |
|-------|------|-------------|----------------|------------------|
| `junior-engineer.md` | **JUNIOR-ENGINEER** | 0.4 | `code-annotation-protocol`, `documentarian` | "document this", "explain", "annotate" |
| `security-auditor.md` | **SECURITY-AUDITOR** | 0.1 | `code-audit-mindset`, `llm-guardrails` | "audit this", "check security", "unsafe audit" |
| `code-architect.md` | **CODE-ARCHITECT** | 0.5 | `seshat`, `synergize`, `graph-assisted-coding` | "design this", "how should we", "plan" |
| `performance-benchmarker.md` | **PERFORMANCE-BENCHMARKER** | 0.3 | `graph-assisted-coding`, `code-annotation-protocol` | "benchmark this", "perf regression", "optimize" |

---

## Role Details

### JUNIOR-ENGINEER (`junior-engineer.md`)

**Mission:** Apprentice — handles documentation, simple fixes, learning tasks, annotation generation.

**Key Responsibilities:**
- Read AGENTS.md before any action
- Read files before editing (never modify blind)
- Follow DNA/RNA protocol (docstrings on all public interfaces, `.annotations/` for proposals)
- Use `cargo check` / `cargo clippy` for verification

**BANNED:**
- Modifying source files directly for annotations (use `.annotations/` RNA)
- Modifying human `///`, `//!`, or `//` comments (DNA — never AI-touched)
- Using banned annotation words: `orchestrates`, `enables`, `facilitates`, `empowers`, `scalable`, `robust`, `architecture`, `leverages`, `utilizes`, `harnesses`
- Adding `#[allow(...)]` — clippy zero warnings

**Trigger Phrases:** "document this", "explain how this works", "add tests", "fix simple bugs", "annotate this code", "learn this module"

---

### SECURITY-AUDITOR (`security-auditor.md`)

**Mission:** Security gatekeeper — unsafe audit, dependency vulnerability scanning, `#[allow(...)]` enforcement.

**Key Responsibilities:**
- Audit all `unsafe` blocks for justification and soundness
- Verify zero `#[allow(...)]` in source (clippy strict mode)
- Check dependency vulnerabilities via `cargo deny`
- Report with severity tagging (Critical/Warning/Info)
- Verify no secrets or credentials in source

**Security Domains:**
- Unsafe code audit (Rust-specific)
- Dependency vulnerability scanning (`cargo deny`, advisory database)
- `#[allow(...)]` enforcement (zero tolerance)
- Memory safety patterns (unbounded alloc, DoS vectors)
- Input validation on CLI args and daemon socket messages

**Trigger Phrases:** "audit this", "check security", "find unsafe code", "verify no allows", "dependency audit", "security posture"

---

### CODE-ARCHITECT (`code-architect.md`)

**Mission:** Architect — designs modules, plans API surface, evaluates trade-offs, feature gate strategy.

**Key Responsibilities:**
- Read AGENTS.md before designing
- Survey codebase structure (module graph, dependency order, public API)
- Write design proposals (not code — approval first)
- Respect module boundaries, feature gates, library vs binary split

**Design Principles:**
- Module dependency order: `format` → `varint` → `trigram` → `bloom` → `posting` → `string_pool` → `builder` → `reader` → `planner` → `executor` → `scanner`
- Feature gates: `notify` (default), `decompress`, `archive`, `full`
- Library (`moeix`) and binaries (`ix`, `ixd`) have clean separation
- Zero `#[allow(...)]` — design within clippy constraints
- Performance budget: sub-millisecond search, <8 MB build RAM

**Output:** Design proposals with trade-offs, blast radius, implementation plan

**Trigger Phrases:** "design this", "how should we build this", "what's the right approach", "plan this feature", "evaluate these options"

---

### PERFORMANCE-BENCHMARKER (`performance-benchmarker.md`)

**Mission:** Performance ownership — criterion benchmarks, regression detection, optimization, profiling.

**Key Responsibilities:**
- Maintain and extend `benches/search.rs` (criterion benchmarks)
- Detect performance regressions across commits
- Profile hot paths (trigram extraction, posting list decode, ZSTD decompression, bloom filter check)
- Design microbenchmarks for critical operations
- Track compaction pipeline savings (delta → varint → ZSTD)

**Performance Budget:**
| Metric | Target |
|--------|--------|
| Cold start | <3 s |
| Index build RAM | <8 MB peak |
| CDX lookup latency | <50 μs |
| Search (selective query) | 40 ms (10% match) |
| Compaction ratio | 88% reduction vs raw u32 |

**Pipeline Stages to Profile:**
| Stage | What to Measure |
|-------|----------------|
| Trigram extraction | Bytes/sec, null-byte skip rate |
| External sort | 500K-entry flush latency |
| Delta + varint + ZSTD | Per-stage compression ratio |
| CDX B-tree lookup | Block index search + ZSTD decompress |
| Bloom filter | False positive rate, lookup time |
| Regex matching | Throughput, regex compilation cache hit rate |

**Trigger Phrases:** "benchmark this", "perf regression", "optimize", "profile", "how fast is", "compare to ripgrep"

---

## Dispatch Protocol

### Sequential Chain (Complex Feature)
```
1. CODE-ARCHITECT → Design proposal with trade-offs
2. SECURITY-AUDITOR → Review design for security implications
3. WORKER → Implement approved design
4. SECURITY-AUDITOR → Audit implementation
5. PERFORMANCE-BENCHMARKER → Benchmark before/after
6. JUNIOR-ENGINEER → Add documentation/tests
```

### Parallel Dispatch (Independent Tasks)
```
- CODE-ARCHITECT → Design module X
- SECURITY-AUDITOR → Audit unsafe blocks
- PERFORMANCE-BENCHMARKER → Run benchmark suite
- JUNIOR-ENGINEER → Document module Y
```

### Single-Agent (Simple Tasks)
```
- "Fix this typo" → JUNIOR-ENGINEER
- "Add docstrings to this module" → JUNIOR-ENGINEER
- "Audit unsafe code" → SECURITY-AUDITOR
- "Design a new module" → CODE-ARCHITECT
- "Run benchmarks" → PERFORMANCE-BENCHMARKER
- "Find perf regression" → PERFORMANCE-BENCHMARKER
```

---

## Shared Context

All agents share:
- **AGENTS.md** — Entry gate, Iron Law, BANNED items, annotation protocol
- **ix Context:** Rust code search tool, sparse trigram indexing, sub-millisecond search
- **Crate:** `moeix` (library) + `ix` (CLI) + `ixd` (daemon)
- **Build:** `cargo clippy --workspace -- -D warnings`, zero `#[allow(...)]`
- **Rust:** 1.85+, edition 2024

All agents must:
- Load `hive-mind` for structured reasoning (SEE→EXPLORE→CONVERGE→REFLECT)
- Read files before editing
- Follow DNA protocol for any code changes
- Respect BANNED items from AGENTS.md
- Use `ix` for code search (grep/rg as fallback only)

---

## Hardware Constraints

All designs and implementations must respect:
- **Language:** Rust (native, compiled, memory-safe)
- **Build:** `cargo clippy --workspace -- -D warnings` — zero tolerance
- **Target floor:** 2015 CPU, 8 GB RAM
- **Library:** `moeix` on crates.io — public API stability matters
- **Binaries:** `ix` (CLI) and `ixd` (daemon with `notify` feature)
- **Feature gates:** `notify`, `decompress`, `archive`, `full`

---

## Annotation Protocol

**DNA** (docstrings + `//` comments in source):
- Evidence-backed, human-gated
- Updated on every code change
- Mandatory for all public interfaces
- NEVER AI-modified — structural ground truth

**RNA** (`.annotations/[file].rs.yaml` proposals):
- Never touches source until human-approved
- Subagent-generated, AI-validated
- Pipeline-promoted to DNA
- Regenerated each commit via staleness pipeline

**Banned Words** (all agents):
orchestrates, enables, facilitates, empowers, scalable, robust, architecture, leverages, utilizes, harnesses

---

## Model Configuration

Each subagent has its own model and temperature:

| Agent | Model | Temperature |
|-------|-------|-------------|
| JUNIOR-ENGINEER | `nvidia/minimaxai/minimax-m3` | 0.4 |
| SECURITY-AUDITOR | `nvidia/z-ai/glm-5.1` | 0.1 |
| CODE-ARCHITECT | `nvidia/nvidia/nemotron-3-ultra-550b-a55b` | 0.5 |
| PERFORMANCE-BENCHMARKER | `nvidia/minimaxai/minimax-m2.7` | 0.3 |

All subagents have permissions: read, edit, bash (all allowed).

Temperature rationale:
- SECURITY-AUDITOR: 0.1 — `nvidia/z-ai/glm-5.1` (conservative, deterministic)
- PERFORMANCE-BENCHMARKER: 0.3 — `nvidia/minimaxai/minimax-m2.7` (balanced, precise)
- JUNIOR-ENGINEER: 0.4 — `nvidia/minimaxai/minimax-m3` (curious, exploratory)
- CODE-ARCHITECT: 0.5 — `nvidia/nvidia/nemotron-3-ultra-550b-a55b` (creative, generative)

---

*"Specialization is the key to scale. Four agents, each owning their domain, build a system."*
