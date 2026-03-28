# Architecture Patterns

Selection guide for common patterns. Each includes: when to use, when NOT to use, and the minimum scale where the pattern pays for itself.

## Pattern Selection Matrix

| Pattern | Use When | Avoid When | Min Scale |
|---------|----------|------------|-----------|
| Monolith | Single team, shared deploy, < 100k LOC | Multiple teams with independent release cycles | Any |
| Modular Monolith | Need boundaries but single deploy is fine | Teams need independent scaling or languages | 1 team |
| Microservices | Independent team ownership, independent scaling required | < 3 teams, shared database, startup phase | 3+ teams |
| Clean/Hexagonal | Complex domain logic must survive framework changes | CRUD with no business rules, scripts, prototypes | Medium |
| Event-Driven | Decoupled producers/consumers, async workflows | Simple request/response, strong consistency required | Medium |
| CQRS | Read/write patterns differ significantly in shape or scale | Uniform access patterns, simple domain | High |
| Event Sourcing | Audit trail is a REQUIREMENT, temporal queries needed | Simple CRUD, no audit/compliance mandate | High |

## Monolith (Default)

**Start here unless you have evidence otherwise.**

Structure:
```
app/
├── domain/        # Business logic (no framework imports)
├── api/           # HTTP handlers, CLI, message consumers
├── persistence/   # Database access, caching
├── services/      # Orchestration between domain objects
└── infrastructure/# Config, logging, external clients
```

Boundaries: Use modules/packages with explicit public APIs. Enforce import rules (no circular deps).

**Upgrade path:** Monolith → Modular Monolith → Extract service when a module needs independent scaling or ownership.

## Modular Monolith

Same deploy unit, but with enforced module boundaries.

Rules:
1. Each module owns its database tables (no cross-module table access)
2. Modules communicate through defined interfaces (not direct DB queries)
3. Shared kernel is minimal and stable (IDs, value objects, events)
4. Module boundaries align with business domains, not technical layers

```
app/
├── modules/
│   ├── orders/
│   │   ├── api.ts          # Public interface
│   │   ├── domain/         # Internal business logic
│   │   ├── persistence/    # Internal DB access
│   │   └── index.ts        # Exports only api.ts
│   ├── payments/
│   │   ├── api.ts
│   │   ├── domain/
│   │   ├── persistence/
│   │   └── index.ts
│   └── shared/             # Minimal shared kernel
│       ├── types.ts        # IDs, value objects
│       └── events.ts       # Domain event definitions
└── main.ts                 # Wiring
```

**Test:** Can you delete a module folder and only break its dependents, not the whole app?

## Clean / Hexagonal Architecture

**Use when:** Domain logic is complex enough that coupling it to frameworks is a maintenance risk.

**Do NOT use when:** The app is primarily CRUD — you'll just add layers that pass data through.

```
          ┌─────────────────────┐
          │   Domain Layer      │  ← Pure business logic, no imports from outer layers
          │   (Entities, Rules) │
          └────────┬────────────┘
                   │ depends on
          ┌────────▼────────────┐
          │   Application Layer │  ← Use cases, orchestration
          │   (Ports = interfaces)│
          └────────┬────────────┘
                   │ implemented by
          ┌────────▼────────────┐
          │   Infrastructure    │  ← Adapters (DB, HTTP, messaging)
          │   (Adapters)        │
          └─────────────────────┘
```

**The Dependency Rule:** Source code dependencies point INWARD only. Domain never imports from Infrastructure.

Practical check:
- Can you swap the database without changing domain code? → Hexagonal is working
- Does every file import from every layer? → Hexagonal is theater

## Microservices

**Prerequisites (ALL must be true):**
1. Multiple teams that need independent release cycles
2. Different scaling requirements per component
3. Team has operational maturity (monitoring, tracing, deployment automation)
4. Failure modes of distributed systems are acceptable

**If any prerequisite is false → use Modular Monolith.**

Required infrastructure:
- Service discovery
- Distributed tracing
- Health checks and circuit breakers
- Contract testing between services
- Deployment automation per service

**Anti-pattern:** Distributed monolith — microservices that must deploy together. Worse than a monolith in every way.

## Event-Driven Architecture

**Use when:** Producers shouldn't know about consumers. Multiple consumers react differently to same event. Async processing is acceptable.

**Avoid when:** You need synchronous response. Strong consistency required across boundaries.

Event types:
1. **Domain Events** — Something happened in the domain ("OrderPlaced", "PaymentReceived")
2. **Integration Events** — Cross-service communication (subset of domain events, explicitly published)
3. **Commands** — Request to do something ("ProcessPayment") — NOT events, different semantics

Rules:
- Events are immutable facts about the past
- Events carry enough data that consumers don't need to call back
- Event schemas are contracts — version them
- Consumers must be idempotent

## CQRS (Command Query Responsibility Segregation)

**Use when:** Read models differ significantly from write models. Read-heavy with complex queries. Need to optimize reads independently.

**Avoid when:** Read and write shapes are similar. Simple CRUD operations.

```
Command Side                    Query Side
┌──────────┐                   ┌──────────┐
│ Commands │                   │ Queries  │
│ Handlers │──writes──►DB◄──projects──│ Handlers │
│ (Domain) │          (source)        │ (Read DB)│
└──────────┘                   └──────────┘
```

Complexity cost: Two models to maintain, eventual consistency between sides, projection logic.

## Decision Framework

When choosing a pattern, answer these questions IN ORDER:

1. **How many teams will own this codebase?**
   - 1 team → Monolith or Modular Monolith
   - 2+ teams with shared release → Modular Monolith
   - 3+ teams needing independent releases → Consider Microservices

2. **How complex is the domain logic?**
   - Mostly CRUD → Simple layered architecture
   - Complex business rules → Clean/Hexagonal for the domain module
   - Mix → Hexagonal for complex modules, simple layers for CRUD modules

3. **What are the consistency requirements?**
   - Strong consistency everywhere → Synchronous, single DB
   - Eventual consistency acceptable → Event-driven where beneficial
   - Mixed → Synchronous within bounded context, events between contexts

4. **What are the scaling requirements?**
   - Uniform → Single deployment unit
   - Wildly different read/write → CQRS for that component
   - Different components scale differently → Service extraction for those components

**Default answer:** Modular Monolith with Clean Architecture for complex modules. This is correct for 80% of projects.
