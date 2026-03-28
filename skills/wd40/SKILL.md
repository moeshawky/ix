---
name: wd40
description: Codebase hygiene through three sequential phases — penetrate, displace, protect. This skill should be used when the user asks to "clean up", "purge", "unify", "consolidate", "deduplicate", "hygiene pass", "pre-deployment cleanup", "wd40", or when identity confusion is detected (two things claiming to be the same entity). Activates pre-deployment, post-sprint, or when accumulated debt makes the codebase fragile.
---

# WD-40

*Penetrate. Displace. Protect. In that order — always.*

Not a lubricant. A displacement agent. The solvent gets into crevices, pushes out what doesn't belong, and evaporates — leaving a thin protective film. The cleanup is the solvent. What stays is the protection.

## The Three Phases

Like its namesake, WD-40 operates in strict sequence. You cannot displace what you haven't penetrated. You cannot protect what you haven't displaced.

### Phase 1 — PENETRATE

*Get into every crevice. Map what's there before touching anything.*

Survey the full state of the codebase:
- `git status` — what's untracked, modified, staged?
- `du -sh */` — where is disk space going?
- Dependency graph tool (codegraph, tree-sitter, etc.) — what depends on what?
- Secrets scan — any credentials, keys, tokens in untracked files?

Categorize every artifact:

| Category | Signal | Example |
|----------|--------|---------|
| **Duplicate** | Two things claiming to be the same entity | `DriverConfig` alias + `BrowserConfig` original |
| **Stale** | Exists but referenced by nothing | Root `brain/` copy superseded by `plugins/brain/` |
| **Exposed** | Secret or sensitive data in accessible location | `.scp_key` in untracked directory |
| **Orphaned** | Build artifact with no build script | Compiled binary with no Cargo.toml target |
| **Fragmented** | Same concern split across multiple locations | Training data in `zo_data/` AND `training_data/` |

**Gate G1**: Survey document complete. Every untracked file categorized. No mutations until penetration is complete.

### Phase 2 — DISPLACE

*Push out what doesn't belong. One category at a time.*

For each category identified in Phase 1, in this order:

1. **Exposed** first — move secrets to safe location immediately
2. **Duplicates** — hash both, verify zero content overlap, merge into canonical location
3. **Fragmented** — consolidate into one directory with clear structure
4. **Stale** — archive to temporary storage (compress, don't delete)
5. **Orphaned** — verify no references exist, then remove

Before each displacement:
- **Hash** the source and target (md5sum, sha256)
- **Grep** for references to the old location
- **Impact check** — what breaks if this moves?

After each displacement category:
- Compile check (`cargo check`, `npm build`, etc.)
- Verify nothing broke

**Gate G2**: Build passes clean after each displacement category. Zero data loss confirmed by hash comparison.

### Phase 3 — PROTECT

*Leave a thin film that prevents re-accumulation.*

The solvent (your attention) will evaporate. What stays must be self-enforcing:

- **.gitignore patterns** — for everything displaced (runtime files, build artifacts, agent configs)
- **Canonical locations** — every entity has exactly ONE home. Document where.
- **Unified configs** — no duplicate configuration files. One source of truth.
- **Lean documentation** — docs that were redundant get merged into parallel documents, not duplicated

**Gate G3**: `git status` fits on one screen. No entity has two locations. Linter/clippy clean.

## Anti-Patterns

| Anti-Pattern | Signal | WD-40 Analog | Response |
|---|---|---|---|
| **Cosmetic cleanup** | "It looks cleaner" but nothing verified | Using WD-40 as furniture polish | Run the survey. Verify with hashes. Looks mean nothing. |
| **Blind deletion** | `rm -rf` before understanding contents | Spraying WD-40 on a fire | Always hash and archive before removing. Compress to temp, don't delete. |
| **Protecting before displacing** | Adding .gitignore over unexamined files | Putting oil film over rust | Survey first. Understand what's there. THEN ignore it. |
| **Skipping penetration** | "I know what's here" without running survey | Surface spray that never reaches the joint | Run the commands. Read the output. Prior assumptions are not ground truth. |
| **Over-application** | Touching files that don't need cleanup | Drowning a mechanism — attracts dust | If it's not duplicate, stale, exposed, orphaned, or fragmented — leave it alone. |

## Iteration Is Expected

WD-40 was the 40th attempt. The first 39 failed. Cleanup is iterative:

- First pass catches the obvious (1000+ untracked files → 80)
- Second pass finds what the first exposed (duplicate configs revealed by the merge)
- Third pass hardens (the .gitignore patterns, the unified docs)

Don't aim for perfection in one pass. Aim for measurable improvement with each iteration.

## When to Apply

| Trigger | Why |
|---------|-----|
| **Pre-deployment** | You don't deploy a fragmented system |
| **Post-sprint** | Rapid feature work accumulates debris |
| **Identity confusion** | Two entities claim to be the same thing |
| **Context bloat** | Session injection exceeds useful size |
| **New team member** | If the repo confuses a human, it confuses an agent |

## Companion Skills

- **seshat** — measurement before design. If WD-40 reveals architectural gaps, seshat measures reality before redesigning.
- **llm-guardrails** — if cleanup reveals code quality issues in what remains, screen with guardrails.
- **advanced-design** — if the displacement phase reveals that the problem is architectural, not hygienic, escalate to design.

## Quick Reference

```
Phase 1: PENETRATE
  git status → du -sh → dependency graph → secrets scan
  Categorize: duplicate / stale / exposed / orphaned / fragmented
  Gate G1: survey complete, no mutations yet

Phase 2: DISPLACE (one category at a time)
  exposed → duplicates → fragmented → stale → orphaned
  Each: hash → grep refs → impact check → move → compile check
  Gate G2: build clean after each category

Phase 3: PROTECT
  .gitignore → canonical locations → unified configs → lean docs
  Gate G3: git status fits one screen, zero duplicate entities
```
