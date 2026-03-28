# Planning Discipline

Turn designs into implementable plans. Every task independently buildable, testable, verifiable.

## The 3-File System

Complex designs use three files to maintain alignment from requirements through implementation:

### File 1: Requirements Trace (`requirements.md`)

Maps every requirement to the component(s) that fulfill it.

```markdown
# Requirements Trace

| ID   | Requirement                        | Component(s)         | Status   |
|------|------------------------------------|----------------------|----------|
| R-01 | Users can create accounts          | auth-module, user-db | Designed |
| R-02 | Orders cannot exceed credit limit  | order-service        | Designed |
| R-03 | Admin can view all orders          | admin-api, order-db  | Planned  |
```

Rules:
- Every requirement has at least one component
- Every component traces back to at least one requirement
- If a component exists without a requirement → flag for removal (G-SCOPE-1)
- If a requirement has no component → design is incomplete

### File 2: Decision Log (`decisions.md`)

Records WHY, not just WHAT. Uses lightweight ADR format.

```markdown
# Decision Log

## D-001: Use PostgreSQL over MongoDB
**Context:** Need transactions across orders and inventory.
**Decision:** PostgreSQL with row-level locking.
**Rationale:** Multi-table transactions are a hard requirement (R-02). MongoDB transactions exist but add complexity without benefit here.
**Alternatives rejected:** MongoDB (weaker transaction model for this use case), MySQL (team has no MySQL experience).
**Consequences:** Must manage schema migrations. Connection pooling needed at ~50 concurrent users.

## D-002: Monolith over microservices
**Context:** Single team of 2 developers, shared release cycle.
**Decision:** Modular monolith with enforced module boundaries.
**Rationale:** Team size doesn't justify operational overhead of microservices. Module boundaries allow future extraction.
**Alternatives rejected:** Microservices (operational overhead for 2-person team).
**Consequences:** Must enforce module boundaries via import rules. Extract to services if team grows past 5.
```

### File 3: Implementation Plan (`plan.md`)

Ordered list of independently implementable tasks.

```markdown
# Implementation Plan

## Phase 1: Foundation (no dependencies)
- [ ] Task 1.1: Set up project structure and CI pipeline
      Verify: CI runs, tests pass (even if empty)
- [ ] Task 1.2: Database schema migration for users table
      Verify: Migration runs up and down cleanly

## Phase 2: Core (depends on Phase 1)
- [ ] Task 2.1: User registration endpoint
      Depends: 1.1, 1.2
      Verify: POST /users returns 201, user exists in DB
- [ ] Task 2.2: Authentication middleware
      Depends: 2.1
      Verify: Protected endpoint returns 401 without token, 200 with valid token
```

## Task Decomposition Rules

### G-PLAN-1: Each Task Must Be Independently Verifiable

A task is well-defined when:
1. **Clear input state** — What must exist before this task starts
2. **Clear output state** — What exists after this task completes
3. **Verification method** — How to prove it works (test, command, observation)
4. **No hidden dependencies** — All prerequisites are listed

Bad task: "Implement the order system"
Good task: "Create POST /orders endpoint that validates items against inventory and returns 201 with order ID. Verify: integration test with valid/invalid inventory."

### Size Heuristic

- **Too big:** Takes more than 1 day or touches more than 3 files
- **Too small:** A single line change that doesn't warrant its own verification
- **Right size:** 1-4 hours, clear start/end, one concern

### Dependency Ordering

```
Independent tasks (no deps)     → Phase 1 (parallel)
Tasks depending on Phase 1      → Phase 2
Tasks depending on Phase 2      → Phase 3
Tasks with circular deps        → STOP. Redesign to break the cycle.
```

Rules:
- Within a phase, tasks can run in parallel
- Between phases, strict ordering
- If a task depends on something in the same phase → move it to next phase
- Circular dependencies between tasks = design problem, not planning problem

## TDD Task Breakdown

For each implementation task, the sub-steps are:

1. **Write failing test** — Defines expected behavior
2. **Implement minimum code** — Make the test pass
3. **Refactor** — Clean up without changing behavior
4. **Verify** — Run full test suite, not just the new test

The test is written FIRST because:
- It forces you to define the interface before implementation
- It catches when the task scope is unclear (can't write a test = can't define done)
- It prevents "works on my machine" — the test IS the verification

## Estimation Anti-Patterns

Do NOT estimate time. Instead:
- **Count tasks** — How many independently verifiable tasks?
- **Count unknowns** — How many tasks have unclear implementation?
- **Count dependencies** — How many tasks are blocked by other tasks?

High unknowns or high dependencies = high risk. Address by:
- **Spike tasks** — Time-boxed research to reduce unknowns
- **Dependency breaking** — Redesign to reduce cross-task dependencies
- **Incremental delivery** — Deliver Phase 1 before planning Phase 3 in detail

## Plan Validation Checklist

Before declaring a plan "ready":

- [ ] Every task traces to a requirement (requirements.md)
- [ ] Every task has a verification method
- [ ] Dependencies are explicit and acyclic
- [ ] No task takes more than 1 day
- [ ] Unknowns have spike tasks
- [ ] Phase 1 can start immediately (no external blockers)
- [ ] Decisions are recorded with rationale (decisions.md)
