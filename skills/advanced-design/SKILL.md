---
name: advanced-design
description: "Systematic software design methodology for building correct systems before writing code. Counterpart to advanced-debugging — prevents bugs at the design phase instead of fixing them after. Use when: (1) designing new systems, features, or modules from scratch, (2) evaluating architecture decisions, (3) reviewing designs for completeness before implementation, (4) choosing between architectural patterns, (5) breaking down a design into implementable tasks, (6) user asks to 'design', 'architect', 'plan the architecture', or 'think through the design' of a system."
---

# Advanced Design

Bad design creates bugs that no amount of debugging can fix. Premature abstractions, missing contracts, copied architectures, and speculative features compound into systems that fight their own developers.

**Core principle:** DESIGN COMPLETELY BEFORE IMPLEMENTING. Every boundary typed. Every pattern justified. Every component traced to a requirement.

## The Iron Law

```
NO IMPLEMENTATION WITHOUT COMPLETE DESIGN FIRST
NO DESIGN CLAIMS WITHOUT GUARDRAIL VERIFICATION
```

If you haven't completed Phase 3, you cannot start implementation.
If you haven't run the Design Review Gate, you cannot claim the design is ready.

## Evidence-First Rule

Before any design decision that touches existing code, query codegraph for impact and relationships of the affected entities:

```
codegraph impact <entity> --depth 2     # blast radius of changes
codegraph neighbors <entity>            # direct callers and callees
codegraph xray <entity>                 # full picture: community + blast radius + summary
```

Do NOT propose a design for an existing boundary without first knowing what depends on it. Surprise coupling is how "simple refactors" break unrelated systems.

## Process

Use `productive_reason` (SEE → EXPLORE → CONVERGE → REFLECT) for all design reasoning. Use `scratchpad` to track design candidates and trade-offs as they accumulate across phases.

## Phase 1: Requirements Dissection

**BEFORE making ANY architecture decision:**

1. **Capture Functional Requirements**
   - What must the system DO? (not how)
   - Number each requirement (R-01, R-02, ...)
   - Each requirement gets acceptance criteria

2. **Capture Non-Functional Requirements**
   - Performance targets (latency, throughput — with numbers)
   - Availability targets (uptime percentage, RTO/RPO)
   - Security requirements (auth, data classification, compliance)
   - Scale expectations (users, data volume, growth rate)

3. **Identify Constraints**
   - Team size and expertise
   - Timeline and delivery milestones
   - Existing infrastructure and tech stack
   - Budget and operational capacity

4. **Mark Unknowns**
   - Requirements that need clarification → ask before designing
   - Technical unknowns → plan spike tasks
   - Do NOT design around unknowns — resolve them

**Output:** Numbered requirements list with acceptance criteria. See `references/planning-discipline.md` for the 3-file system.

## Phase 2: Pattern Selection

**Choose architecture based on constraints, not trends.**

1. **Start with the Simplest Pattern That Works**
   - Default: Modular Monolith (correct for 80% of projects)
   - Only add complexity when a specific requirement demands it
   - See `references/architecture-patterns.md` for the full decision framework

2. **Justify Every Pattern**
   - "We use pattern X because requirement R-NN needs Y, and X provides Y by Z"
   - If justification references a company name instead of a problem → F-COPY detected
   - If justification is "best practice" without constraint mapping → F-RESUME detected

3. **Run Failure Mode Scan**
   - Check all 6 failure classes from `references/design-failure-modes.md`
   - If 2+ failure modes detected → architecture is fundamentally misaligned, restart Phase 1

4. **Apply Guardrails**
   - **G-SCOPE-1:** Every component traces to a current requirement
   - **G-PATTERN-1:** Pattern justified by project constraint, not trend
   - **G-SIMPLE-1:** No simpler design meets the same requirements — but distinguish: essential complexity (the genuinely hard parts of the problem) must be preserved; strip only accidental complexity (boilerplate, unnecessary indirection, mechanical translation layers that add no semantic value)

**Output:** Architecture decision with rationale. Record in ADR format (see `references/decision-records.md`).

## Phase 3: Contract-First Design

**Define every boundary BEFORE implementation.**

For each boundary (API, module interface, event, database):
1. Specify input types with constraints
2. Specify output types for ALL cases (success AND every error)
3. Define error shapes (consistent across the system)
4. State invariants (pre/post conditions, state machine transitions)

See `references/contract-design.md` for:
- REST endpoint specification format
- Database schema contract format
- Event/message schema format
- Module interface patterns
- Contract testing strategy

**Guardrail G-CONTRACT-1:** Every boundary has typed contracts, not prose descriptions.

**Output:** Typed interface contracts for every boundary in the system.

## Phase 4: Implementation Plan

**Break the design into independently verifiable tasks.**

1. **Create Requirements Trace** — Map R-XX → components → contracts → tests
2. **Decompose into Tasks** — Each task: clear input state, output state, verification method
3. **Order by Dependencies** — Phase 1 (no deps, parallel) → Phase 2 (depends on 1) → ...
4. **Size Check** — No task > 1 day. No task touches > 3 files. No output file exceeds 500 lines without a documented justification that splitting reduces cohesion.
5. **Identify Spikes** — Unknown implementation → time-boxed research task first

**Guardrail G-PLAN-1:** Each task independently implementable and verifiable.

See `references/planning-discipline.md` for task decomposition rules and TDD breakdown.

**Output:** Ordered task list with dependencies and verification methods.

## Phase 5: Design Review Gate

```
BEFORE claiming a design is complete:

1. Run the full guardrail checklist below
2. Check for failure modes (references/design-failure-modes.md)
3. Verify traceability: requirements → decisions → components → contracts → tasks
4. Verify against AOP v3 verification gates G1-G5 (see below)
5. Any BLOCK finding → return to the relevant phase
6. ONLY THEN: declare design ready for implementation
```

### Guardrail Checklist

| Rule | Check | Phase |
|------|-------|-------|
| G-SCOPE-1 | Every component traces to a requirement | 2, 5 |
| G-PATTERN-1 | Pattern justified by constraint, not trend | 2, 5 |
| G-CONTRACT-1 | Every boundary has typed contracts | 3, 5 |
| G-PLAN-1 | Each task independently verifiable | 4, 5 |
| G-SIMPLE-1 | No simpler design meets requirements (essential complexity preserved, accidental complexity stripped) | 2, 5 |

### AOP v3 Verification Gates (G1–G5)

Before declaring the design complete, confirm the path to each gate is clear:

| Gate | Name | Design Implication |
|------|------|--------------------|
| G1 | Evidence | Every design claim backed by codegraph data, contracts, or requirement traces — not assumption |
| G2 | Compilation | Contracts and interfaces are typed such that the compiler will reject violations at build time |
| G3 | Tests | Each contract has a corresponding test strategy; no boundary is untestable by design |
| G4 | Witness Review | Design is legible enough for a second agent (Witness) to verify correctness independently |
| G5 | Deacon Gate | Design satisfies merge criteria: no speculative scope, no unresolved unknowns, no BLOCK findings |

If a gate cannot be reached given the current design, the design is not done.

### Failure Mode Quick-Scan

| Code | Failure | Signal |
|------|---------|--------|
| F-ABS | Premature Abstraction | Interface with 1 implementation, generic for specific |
| F-RESUME | Resume-Driven Architecture | Tech choice unjustified by requirements |
| F-DIAGRAM | Diagram-Only Design | Boundaries in prose, not types |
| F-COPY | Copy-Paste Architecture | "Company X uses this" without problem mapping |
| F-SINK | Kitchen Sink Module | One module handles unrelated concerns |
| F-SPEC | Speculative Generality | Config/extension points nobody uses |
| F-MONO | Monolithic File | Single file > 500 lines mixing distinct concerns — usually F-SINK at the file level |

**If 2+ detected → compound cascade likely. Return to Phase 1.**

Full detection patterns (P-1 through P-23) in `references/design-failure-modes.md`.

## Failure Modes to Watch

AOP v3 defines 9 failure modes (G-HALL through G-DEP) that manifest in design decisions before a single line of code is written. Recognize them early.

| Code | Name | What it looks like in DESIGN |
|------|------|------------------------------|
| G-HALL | Hallucinated Capability | Designing around an API, library, or behavior that hasn't been verified to exist or work as assumed — always check before specifying |
| G-SCOPE | Scope Creep | Requirements grow during Phase 2–4 without returning to Phase 1; components accumulate without a traced requirement |
| G-SPEC | Speculative Generality | Interfaces designed for callers that don't exist yet; extension points with no concrete use case in the current requirements |
| G-COPY | Copy Architecture | Structural decisions imported wholesale from another project without mapping failure modes to THIS system's constraints |
| G-PERF | Premature Optimization | Performance requirements invented (not measured, not in NFRs) drive complexity before correctness is established |
| G-COUP | Hidden Coupling | Design appears modular on paper but shares mutable state, global registries, or implicit ordering — check with codegraph neighbors before finalizing |
| G-ABST | Wrong Abstraction Level | Contracts defined at the wrong layer — too low (leaking internals) or too high (losing specificity needed for implementation) |
| G-TRUST | Trust Boundary Violation | Sensitive data or elevated-privilege operations cross a boundary without an explicit contract governing auth/validation |
| G-DEP | Dependency Blindness | A new component introduced without querying its impact on existing dependents — always run Evidence-First Rule before finalizing any boundary |

When two or more of these appear together, they compound. A design with G-HALL + G-COUP + G-DEP is a cascade waiting to happen — return to Phase 1.

## Red Flags — STOP and Return to Earlier Phase

If you catch yourself thinking:
- "We might need this later" → F-SPEC. Build for today.
- "Netflix/Google/Stripe does it this way" → F-COPY. Map to YOUR constraints.
- "Let's add an abstraction layer" → F-ABS. Wait for third concrete instance.
- "Services communicate via API" (no contract) → F-DIAGRAM. Define the types.
- "It's simpler to keep it all together" → F-SINK. Check if concerns are related.
- "It's industry best practice" → F-RESUME. Map to specific requirement.
- "We'll figure out the schema later" → Phase 3 incomplete. Stop.
- "Let me start coding, I'll design as I go" → Iron Law violation. Stop.
- "I know what depends on this" (without querying codegraph) → G-DEP. Run the Evidence-First Rule.

## Reference Files

| File | Content | When to Read |
|------|---------|-------------|
| `references/design-failure-modes.md` | 6 failure classes, 23 detection patterns, guardrails, compound cascade | Phase 2 (failure scan), Phase 5 (review) |
| `references/architecture-patterns.md` | Monolith/Modular/Micro/Clean/Event/CQRS with selection framework | Phase 2 (pattern selection) |
| `references/contract-design.md` | API contracts, schema design, event schemas, interface patterns | Phase 3 (contract design) |
| `references/planning-discipline.md` | 3-file system, task decomposition, TDD breakdown, dependency ordering | Phase 4 (planning) |
| `references/decision-records.md` | ADR template, traceability matrix, review checklist, severity levels | Phase 2 (decisions), Phase 5 (review) |

## Quick Reference

| Phase | Key Activities | Output | Success Criteria |
|-------|---------------|--------|------------------|
| **1. Requirements** | Capture functional/non-functional, identify constraints, mark unknowns | Numbered requirements with acceptance criteria | Every requirement testable, no ambiguity |
| **2. Pattern Selection** | Choose architecture, justify, failure scan, guardrails | ADR with rationale | G-SCOPE-1, G-PATTERN-1, G-SIMPLE-1 pass |
| **3. Contracts** | Type every boundary: input, output, error, invariants | Typed interface contracts | G-CONTRACT-1: no prose boundaries |
| **4. Planning** | Decompose tasks, order deps, size check, identify spikes | Ordered task list | G-PLAN-1: each task independently verifiable |
| **5. Review Gate** | Full guardrail checklist, AOP v3 G1-G5, failure scan, traceability check | Design approval or return to phase | All guardrails pass, no BLOCK findings |
