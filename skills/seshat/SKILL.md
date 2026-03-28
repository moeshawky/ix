---
name: seshat
description: "Architecture methodology rooted in ancient Egyptian engineering + fahlawa (strategic perception) + jugaad (frugal innovation). Replaces generic architecture skills with a philosophy-first approach: measure reality before designing, think in tangible ratios not abstractions, iterate through failure not around it, preserve knowledge collectively, and master constraints instead of wishing for better tools. Use when: (1) designing systems, infrastructure, or platforms, (2) making build-vs-buy decisions, (3) choosing between architectural patterns under real constraints, (4) evaluating whether a design is grounded or aspirational, (5) planning migrations or rewrites, (6) the user says 'architect', 'design the system', 'what's the right approach', or 'how should we build this'. Triggers on: architecture, system design, infrastructure planning, trade-off analysis, constraint-driven design, platform decisions."
---

# Seshat

*Measure Reality. Architect from Truth. Build with What Exists.*

> Named for the Egyptian goddess of measurement and architecture — the divine surveyor who aligned temples to the stars before a single stone was placed. She wore a leopard skin (adaptive predator) and a seven-pointed star (computation before construction).

Use `productive_reason` (SEE/EXPLORE/CONVERGE/REFLECT) throughout. Use `scratchpad` to track measurements, constraints, and iterations.

## The Five Principles

### I. Stretch the Cord

*"Align to the stars before you lay the foundation."*

Before designing ANYTHING, **measure reality against absolute references.** Not documentation. Not assumptions. Not what the last model told you.

**Actions:**
- Query codegraph for blast radius, callers, neighbors of every entity you'll touch
- Run the actual code. Hit the actual endpoint. Read the actual file.
- Measure current performance (latency, memory, throughput) before proposing changes
- If you can't measure it, you can't design for it

**Gate:** No architecture proposal without evidence gathered from real systems. "I assume X works this way" is a design fault.

### II. Sekel

*"Express the slope as palms and digits, not degrees."*

Every requirement, every constraint, every decision expressed as a **ratio between tangible things** — not abstract qualities.

| Abstract (reject) | Sekel (accept) |
|---|---|
| "Scalable" | "10K requests at p99 <200ms on 2 cores" |
| "Lightweight" | "130M params, 250MB RAM, <5ms inference" |
| "Fast" | "Cold start <3s, hot path <50ms" |
| "Secure" | "TLS 1.3, zero plaintext secrets, API key rotation <1hr" |
| "Elegant" | "3 files changed, 0 new abstractions, all tests pass" |

**Gate:** If a requirement contains an adjective without a number, it's not a requirement. It's a wish. Convert it or reject it.

### III. The Bent Pyramid

*"Note the failure. Redesign. Build again."*

When something fails:
1. **Note** what broke and WHY (not just what)
2. **Redesign** — change the architecture, not just the implementation
3. **Build again** from the redesign

The progression: mastaba → step pyramid → bent pyramid → Red Pyramid → Great Pyramid. **Four redesigns.** Each incorporated the structural lesson of the previous failure. They didn't patch the bent pyramid — they built a new one.

**Rules:**
- 1st failure: fix the implementation, investigate the architecture
- 2nd failure: the architecture is suspect — redesign the affected boundary
- 3rd failure: **STOP.** The architecture is wrong. Return to Principle I (Stretch the Cord) and re-measure reality

**Gate:** After 3 failures at the same boundary, no more implementation attempts. Redesign or escalate.

### IV. The House of Life

*"What one scribe learns, all scribes access."*

Every measurement, decision, failure, and insight goes into the collective repository — not individual memory.

**Actions:**
- Store architectural decisions in hive-mind with rationale (not just the decision)
- Record failed approaches with WHY they failed (training data for future agents)
- Use scratchpad for in-progress work, hive-mind memory_store for permanent knowledge
- Reference existing memories before proposing new designs: `memory_search` first

**Gate:** If you're designing something and haven't checked hive-mind for prior art, you're not using the House of Life.

### V. Rope and Copper

*"2cm precision across 13 acres. With rope and copper."*

The constraint is the design space. Don't wish for better tools — master the ones you have.

**The Three Questions:**
1. **Which constraints are real?** (fahlawa — see through theater)
   - "Rate limited to 40 RPM" — real? Or soft limit that resets on retry?
   - "Requires 16GB RAM" — real? Or assumed because nobody measured?
   - "Can't run locally" — real? Or nobody tried a smaller model?

2. **What's already available?** (jugaad — build from scraps)
   - Git history has training data nobody extracted
   - Free tier credits fund real infrastructure
   - Swap extends RAM for cold-path models
   - One database (pgvector) replaces three services

3. **What's the minimum that WORKS?** (Seshat — precise measurement)
   - Not the minimum that looks clean. The minimum that passes all acceptance criteria with measured evidence.

**Gate:** If your design requires resources you don't have, apply the Three Questions before requesting more.

## Process

1. **Stretch the Cord** — Measure current state. Codegraph. Tests. Endpoints. Files. Evidence.
2. **Sekel** — Express all requirements as tangible ratios. Reject adjectives without numbers.
3. **Design** — Propose architecture. Run through `advanced-design` Phase 2-3 (pattern selection + contracts).
4. **Elegance Test** — Load `elegantify`. Essential complexity preserved? Accidental stripped? Failure modes covered?
5. **Verify** — Load `llm-guardrails`. Screen for 9 failure modes. Run AOP v3 gates G1-G5.
6. **Record** — Store decisions + rationale in House of Life (hive-mind).

If any step fails → apply **Bent Pyramid**: note, redesign, rebuild. Not patch.

## Anti-Patterns

| Pattern | Signal | Seshat Response |
|---------|--------|----------------|
| **Resume Architecture** | "Netflix uses this" | Stretch the Cord — measure YOUR constraints, not Netflix's |
| **Aspiration Masking** | "We need it to be scalable" | Sekel — scalable to WHAT number? |
| **Patch Stacking** | 3rd fix for same boundary | Bent Pyramid — redesign, don't patch |
| **Knowledge Hoarding** | "I know how this works" (no evidence shared) | House of Life — write it down or it doesn't exist |
| **Tool Envy** | "If only we had Kubernetes/GPU/more RAM" | Rope and Copper — what can you build RIGHT NOW? |

## Reference

- `references/philosophy.md` — Deep context on fahlawa, jugaad, ancient Egyptian engineering methodology
- `references/patterns.md` — Constraint patterns, iteration patterns, and architectural decisions discovered across sessions

## Companion Skills

- `elegantify` — Engineering Elegance Test (essential vs accidental complexity)
- `advanced-design` — Contract-first design with typed boundaries and guardrail gates
- `llm-guardrails` — 9 failure mode screening for AI-generated architecture
- `advanced-debugging` — Root cause analysis when the Bent Pyramid triggers
- `charlie-prompt-engineering` — AOP v3 prompt patterns for dispatching work to agents
