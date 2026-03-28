# Decision Records & Design Review

Capture decisions with context. Review designs against guardrails before implementation.

## Architecture Decision Record (ADR) Template

```markdown
# ADR-NNN: [Decision Title]

## Status
[Proposed | Accepted | Deprecated | Superseded by ADR-NNN]

## Context
What problem are we solving? What constraints exist?
- Requirement(s): [R-XX from requirements trace]
- Constraint(s): [team size, timeline, tech stack, compliance, etc.]

## Decision
What did we decide?

## Rationale
Why this option over alternatives? Map to specific constraints.

## Alternatives Considered
| Option | Pros | Cons | Why Rejected |
|--------|------|------|--------------|
| ...    | ...  | ...  | ...          |

## Consequences
- **Positive:** What becomes easier?
- **Negative:** What becomes harder? What new constraints does this introduce?
- **Risks:** What could go wrong? What are the mitigation strategies?

## Review Date
When should this decision be revisited? [date or trigger condition]
```

## When to Write an ADR

Write an ADR for decisions that are:
- **Costly to reverse** — Database choice, language choice, architecture pattern
- **Affect multiple components** — Cross-cutting concerns, shared contracts
- **Controversial** — Multiple valid options where team members disagree
- **Counter-intuitive** — Decisions that need explanation for future developers

Do NOT write ADRs for:
- Obvious choices with no alternatives
- Implementation details within a single module
- Temporary decisions (use code comments instead)

## Traceability Matrix

Links requirements → decisions → components → tests:

```markdown
| Req  | Decision | Component       | Contract           | Test              |
|------|----------|-----------------|--------------------|-------------------|
| R-01 | D-001    | auth-module     | POST /auth/login   | auth.test.ts      |
| R-02 | D-002    | order-service   | POST /orders       | orders.test.ts    |
| R-03 | D-001    | admin-api       | GET /admin/orders  | admin.test.ts     |
```

Purpose:
- Every requirement has a path to implementation
- Every component traces to a requirement (nothing built "just in case")
- Every contract has tests planned
- Gaps are visible: empty cells = incomplete design

## Design Review Gate

### Pre-Review Checklist

Before submitting a design for review, verify:

**Requirements (Phase 1)**
- [ ] All requirements are captured and numbered
- [ ] Each requirement has acceptance criteria
- [ ] Non-functional requirements are explicit (performance, security, availability)
- [ ] No requirement is ambiguous — could two developers interpret it differently?

**Architecture (Phase 2)**
- [ ] Pattern selection justified by project constraints, not industry trends (G-PATTERN-1)
- [ ] No simpler design meets the same requirements (G-SIMPLE-1)
- [ ] Every component traces to a current requirement (G-SCOPE-1)
- [ ] Failure mode scan completed — no F-ABS, F-RESUME, F-COPY, F-SINK, F-SPEC detected

**Contracts (Phase 3)**
- [ ] Every boundary has typed contracts: input, output, error, invariants (G-CONTRACT-1)
- [ ] State machines have transition diagrams
- [ ] Error shapes are consistent
- [ ] Backward/forward compatibility rules defined for events

**Plan (Phase 4)**
- [ ] Each task is independently implementable and verifiable (G-PLAN-1)
- [ ] Dependencies are explicit and acyclic
- [ ] No task larger than 1 day
- [ ] Unknowns have spike tasks
- [ ] Requirements → decisions → components → tests traceability complete

### Review Questions

For each design element, ask:

1. **"What requirement does this serve?"** — If no answer → remove it
2. **"What's the simplest version of this?"** — If answer is simpler → simplify
3. **"What happens when this fails?"** — If no answer → contract is incomplete
4. **"Who changes this and why?"** — If answer is "everyone, for everything" → wrong boundary
5. **"What would make us revisit this decision?"** — If no answer → add review trigger to ADR

### Severity Levels

| Level | Meaning | Action |
|-------|---------|--------|
| **BLOCK** | Fundamental misalignment with requirements or missing contracts | Return to earlier phase |
| **WARN** | Potential issue that should be addressed before implementation | Fix before starting Phase 4 tasks |
| **NOTE** | Observation for future consideration | Document in ADR, proceed |

### Common Review Findings

| Finding | Severity | Guardrail |
|---------|----------|-----------|
| Component without requirement trace | BLOCK | G-SCOPE-1 |
| Pattern chosen without constraint justification | WARN | G-PATTERN-1 |
| Boundary described in prose, not types | BLOCK | G-CONTRACT-1 |
| Task not independently verifiable | WARN | G-PLAN-1 |
| More complex design exists that's simpler | WARN | G-SIMPLE-1 |
| Two or more failure modes detected | BLOCK | Compound cascade |
| Decision without rationale | WARN | ADR incomplete |
| Requirement without acceptance criteria | BLOCK | Phase 1 incomplete |

## Living Documents

Design documents are living artifacts:
- **Update** when implementation reveals new constraints
- **Deprecate** ADRs when superseded (don't delete — history matters)
- **Re-review** when scope changes significantly
- **Archive** after project completion (future reference, not active maintenance)
