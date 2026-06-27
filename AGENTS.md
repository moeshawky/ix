# AGENTS.md — ix Entry Gate
*Rust sparse-trigram code search. 4 cognitive lenses. Zero #[allow(...)]. Sub-millisecond search.*

## What This Is

This is the canonical entry gate for the ix workspace — a Rust code search tool with sparse trigram indexing. This file routes to SUBAGENT_ROLES.md (4 specialized subagents) and ~/.config/opencode/skills/ (60+ cognitive lenses). This is infrastructure, not documentation. Every rule survives filesystems, platform differences, and context compaction.

## The Iron Law

```
READ THIS FILE FIRST — every session, every compaction
ZERO #[allow(...)] — clippy strict mode is law
ROUTE THROUGH SUBAGENT_ROLES.md — never bypass the 4 lenses
LOAD SKILLS BY TRIGGER, NOT PREFERENCE — 1% rule applies
VERIFY EVERY REFERENCE PATH EXISTS — broken chain = silent failure
VIOLATE → BROKEN BUILD / SECURITY REGRESSION / WRONG LENS
```

## Load Order (Mandatory Sequence)

1. **moeify** — cognitive alignment, ALWAYS FIRST
2. **general-reasoning** — structured reasoning, ALWAYS SECOND
3. **Domain skills** — by task trigger (see categories below)
4. **Verification skills** — dialectical-challenging, code-audit-mindset — BEFORE COMMIT

## Skill Categories (Navigation Tables)

### Core Reasoning
| Skill | When |
|-------|------|
| `moeify` | Session start, after compaction, "align", "realign" |
| `general-reasoning` | Novel problems, architecture decisions, complex debugging |
| `sequential-thinking` | Multi-step analysis, uncertainty, course correction needed |

### Code Audit
| Skill | When |
|-------|------|
| `code-audit-mindset` | Code review, audit changes, check for bugs, "review this" |
| `security-auditor` | Unsafe audit, dependency check, "verify no allows" |
| `llm-guardrails` | Elevated compliance, reasoning failure screening |
| `compounded-bug-protocol` | Boundary violations, chain interactions, "trace the chain" |

### Design
| Skill | When |
|-------|------|
| `seshat` | Architecture decisions, build-vs-buy, constraints as design |
| `synergize` | Unify overlapping systems, "consolidate", "merge components" |
| `code-architect` | Module design, API surface, feature gate strategy |

### Performance
| Skill | When |
|-------|------|
| `performance-benchmarker` | Benchmarks, regression detection, profiling, "optimize" |
| `graph-assisted-coding` | Call chains, impact analysis, "who calls", architecture scan |

### Documentation
| Skill | When |
|-------|------|
| `documentarian` | API docs, runbooks, architecture docs, examples |
| `code-annotation-protocol` | Docstrings, DNA/RNA annotations, "document this" |

### Subagent Dispatch
| Skill | When |
|-------|------|
| `subagents` | Delegating work, parallelizing tasks, background execution |
| `puppeteer-prompter` | Designing system prompts, debugging agent misalignment |

### Verification
| Skill | When |
|-------|------|
| `dialectical-challenging` | "challenge this", steelman counterarguments, before decisions |
| `maat` | Pre-deployment gate, structural health check, "weigh the heart" |

### Maintenance
| Skill | When |
|-------|------|
| `repo-maintenance-workflow` | Audit and fix, pre-publish check, maintenance sweep |
| `wd40` | Cleanup, purge, deduplicate, hygiene pass, accumulated debt |
| `pre-publish` | Git hygiene, docs, changelog, publish discipline |

## Skill Discovery Protocol

1. **Scan** `~/.config/opencode/skills/` and `.opencode/skills/` for matching triggers
2. **1% rule**: if a skill might apply (≥1% relevance), load it
3. **Rationalization detection**: "this skill isn't needed" without evidence = violation
4. **AFTER loading any skill**: Read ALL `references/` files in that skill's directory
5. **Cross-skill references**: if Skill A references Skill B's files, load Skill B first
6. **Verification gate**: before any action, ask "did I load all relevant skills?"

## Platform Architecture

**Layer 1: Entry Gate** — AGENTS.md (this file) — routing table
**Layer 2: Subagent Layer** — SUBAGENT_ROLES.md — 4 specialized agents
**Layer 3: Skill Layer** — `~/.config/opencode/skills/` — 60+ cognitive lenses

## Subagent Roles (from SUBAGENT_ROLES.md)

| Agent | Role | Model | Temp | When |
|-------|------|-------|------|------|
| `junior-engineer` | Apprentice — docs, simple fixes, annotation generation | minimax-m3 | 0.4 | "document this", "explain", "annotate" |
| `security-auditor` | Security gatekeeper — unsafe audit, dependency scan | glm-5.1 | 0.1 | "audit this", "check security", "verify no allows" |
| `code-architect` | Architect — design, API surface, trade-offs | nemotron-3-ultra | 0.5 | "design this", "how should we", "plan" |
| `performance-benchmarker` | Performance ownership — benchmarks, profiling | minimax-m2.7 | 0.3 | "benchmark this", "perf regression", "optimize" |

## File Structure Convention

```
/home/ubuntu/ix/
├── AGENTS.md              — this entry gate
├── SUBAGENT_ROLES.md      — subagent dispatch protocol
├── src/
│   ├── lib/               — moeix library
│   │   ├── trigram.rs     — trigram extraction
│   │   ├── posting.rs     — posting list decode
│   │   ├── executor.rs    — search execution
│   │   └── ...
│   └── bin/
│       ├── ix/            — CLI binary
│       └── ixd/           — daemon binary
├── .annotations/          — RNA proposals (AI-generated, not DNA)
├── benches/               — criterion benchmarks
├── Cargo.toml             — workspace definition
└── ~/.config/opencode/skills/{skill-name}/
    ├── SKILL.md           — skill definition
    └── references/        — extended protocols (READ ALL)
```

## DNA/RNA Annotation Protocol

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

## BANNED

- Modifying source files directly for annotations (use `.annotations/` RNA)
- Modifying human `///`, `//!`, or `//` comments (DNA — never AI-touched)
- Using banned annotation words: `orchestrates`, `enables`, `facilitates`, `empowers`, `scalable`, `robust`, `architecture`, `leverages`, `utilizes`, `harnesses`
- Adding `#[allow(...)]` — clippy zero warnings is law
- Bypassing SUBAGENT_ROLES.md dispatch protocol
- Using tool-specific commands (grep, cat, Read, Write — say "search" "examine" "read" "write")
- Free-form thought without structured reasoning (12-thought protocol mandatory)
- Fabricated data or hallucinated file paths
- Skipping reference file reads (ALL references/ MUST be read)
- Committing without verification (cargo clippy --workspace -- -D warnings)
- Touching unsafe blocks without security-auditor review
- Ignoring ResourceGuard in parallel loops (use pressure(), not check())

## Performance Budget

| Metric | Target |
|--------|--------|
| Cold start | <3 s |
| Index build RAM | <8 MB peak |
| CDX lookup latency | <50 μs |
| Search (selective query) | 40 ms (10% match) |
| Compaction ratio | 88% reduction vs raw u32 |

---

*Every skill you skip is a correction you'll receive. Every reference unread is a bug you'll write. Every allow you add is technical debt with interest.*