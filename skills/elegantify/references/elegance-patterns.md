# Elegance Patterns

Patterns discovered by applying the elegantify process. Each earned its place by solving a real problem.

## Pneumatic Tube
**Capsule is self-contained. Tube is dumb. Endpoints are smart.**
Use when: multiple backends need a single query interface. The caller doesn't know which backend answered.

## Graph IS the Index
**Compute similarity at write time, store as edges. Search becomes traversal.**
Use when: vectors and graph coexist. Eliminates runtime vector search.

## Confidence Decay
**Edge weight × parent confidence = child confidence. Strongest paths first.**
Use when: graph traversal where result quality matters, not just reachability.

## Immune Layers
**Barriers (structural) → Innate (fast patterns) → Adaptive (context-aware) → Memory (persistent) → Homeostasis (self-regulation).**
Use when: security/safety needs both speed and depth. Each layer catches what the previous missed.

## Atomic Version Swap
**Write to staging. Atomic replace into active. Old becomes backup.**
Use when: long-running updates to a file-based store with concurrent readers.

## Via Negativa
**The elegant design is defined by what it removes, not what it adds.**
Use when: always. Every feature must answer: "does this earn its complexity?"
