# Design Failure Modes

6 systematic failure patterns in software design. Each has detection signals, cost analysis, and guardrail rules.

## 1. Premature Abstraction (F-ABS)

**Signal:** Interfaces with one implementation. Generic frameworks for specific problems. "We might need this later."
**Cost:** Wrong abstraction is worse than duplication — every future change must navigate the abstraction layer.
**Detection patterns:**
- P-1: Interface/abstract class with exactly one concrete implementation
- P-2: Generic type parameters that are only ever instantiated with one type
- P-3: Plugin/extension system with one plugin
- P-4: Configuration for behavior that never varies

**Guardrail: G-SCOPE-1** — Does every component trace to a current requirement? If a component exists for a future requirement, delete it.

**Rule of Three:** Wait for the third concrete instance before abstracting. Two instances = coincidence. Three = pattern.

## 2. Resume-Driven Architecture (F-RESUME)

**Signal:** Tech choice cannot be justified by project requirements. New framework/language per project. "Let's use X" where X is trendy.
**Cost:** Team learns instead of ships. Operational burden exceeds problem complexity.
**Detection patterns:**
- P-5: Kubernetes for a single-process app
- P-6: Microservices for a team of 1-3 developers
- P-7: Event sourcing for simple CRUD with no audit requirements
- P-8: GraphQL for a single client consuming predictable shapes

**Guardrail: G-PATTERN-1** — Can the team explain WHY this pattern, not just WHAT it is? If the justification is "it's industry best practice" without mapping to a specific constraint, it's resume-driven.

**Boring Technology Rule:** Default to what the team already knows. Novel technology requires extraordinary justification: a requirement that existing technology CANNOT meet (not "meets less elegantly").

## 3. Diagram-Only Design (F-DIAGRAM)

**Signal:** Architecture diagrams with zero interface contracts. "Services communicate via APIs" with no schema. Design reviews that discuss boxes and arrows.
**Cost:** Integration failures at every boundary. Each team interprets the diagram differently. Discovery of incompatibilities at integration time (the most expensive phase).
**Detection patterns:**
- P-9: Architecture deck with no typed interface definitions
- P-10: "REST API" boundary with no endpoint/schema specification
- P-11: Database schema not defined but "we'll figure it out"
- P-12: Message formats described in prose, not schema

**Guardrail: G-CONTRACT-1** — Is every boundary specified with types, not prose? A design is incomplete until every interface has: input types, output types, error types, and invariants.

## 4. Copy-Paste Architecture (F-COPY)

**Signal:** "Company X uses this pattern." Microservices because Netflix. Event sourcing because banking. Hexagonal because "Clean Architecture book."
**Cost:** Inherits constraints you don't have. Misses constraints you do have.
**Detection patterns:**
- P-13: Pattern choice references a company name, not a problem description
- P-14: Architecture includes components that solve problems the project doesn't have
- P-15: No adaptation of the pattern to project-specific constraints

**Mitigation:** Trace every pattern back to the PROBLEM it solves. Write: "We use pattern X because we have problem Y, and X solves Y by Z." If you can't fill in Y and Z from your own requirements, you're copying.

## 5. Kitchen Sink Module (F-SINK)

**Signal:** One service/module handles unrelated concerns. "It's simpler to keep it together." God objects. Fat controllers. `utils.js` with 2000 lines.
**Cost:** Every change risks everything. Cannot deploy/scale/test independently. Team bottleneck — everyone touches the same module.
**Detection patterns:**
- P-16: Module with more than 3 unrelated domain concepts
- P-17: Single deployment unit handling both read-heavy and write-heavy workloads
- P-18: Circular dependencies between modules (symptom of wrong boundaries)
- P-19: "shared" or "common" module that every other module depends on

**Guardrail: G-SIMPLE-1** — Boundaries follow business domains, not technical layers. Split along behavior (what changes together stays together), not data.

## 6. Speculative Generality (F-SPEC)

**Signal:** Config flags for features that don't exist. "Extensibility points" nobody extends. Abstract factories for objects that are always created the same way.
**Cost:** Dead code that must be maintained. Constraints on future design from imagined requirements. Cognitive overhead for every reader.
**Detection patterns:**
- P-20: Feature flags with one value in all environments
- P-21: Strategy/factory patterns with one strategy/product
- P-22: "Provider" abstraction wrapping a single concrete service
- P-23: Comments saying "TODO: make this configurable" or "future: support X"

**Rule:** Build for today's requirements. Tomorrow's requirements will surprise you. The cost of adding an abstraction later (when you understand the real requirement) is lower than maintaining a wrong abstraction now.

## Compound Cascade

Design failures cascade in predictable chains:

```
Resume-Driven → Copy-Paste → Premature Abstraction → Kitchen Sink
```

If the architecture was chosen for resume value (F-RESUME), patterns will be copied without adaptation (F-COPY), abstractions will be added "because the pattern requires it" (F-ABS), and unrelated concerns will be forced into mismatched modules (F-SINK).

**If you detect two or more failure modes in the same design, the architecture is likely fundamentally misaligned with requirements. Return to Phase 1.**

## Guardrail Summary

| Rule | Check | Phase |
|------|-------|-------|
| G-SCOPE-1 | Every component traces to a requirement | 5 |
| G-PATTERN-1 | Pattern justified by project constraint, not industry trend | 2, 5 |
| G-CONTRACT-1 | Every boundary has typed interface contracts | 3, 5 |
| G-PLAN-1 | Each task independently implementable and verifiable | 4, 5 |
| G-SIMPLE-1 | No simpler design meets the same requirements | 2, 5 |
