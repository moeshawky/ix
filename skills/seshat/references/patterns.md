# Seshat Patterns

Architectural patterns discovered through constraint-driven design. Each earned its place by solving a real problem under real constraints.

## Constraint Patterns

### The pgvector Move
**One database replaces three services.**
When you have PostgreSQL + ChromaDB + a separate embedding service, ask: can pgvector do all three? Result: 1.4GB RAM recovered, zero new infrastructure, managed backups for free.
*Principle: Rope and Copper*

### The Swap Ladder
**Hot models in RAM, cold models in swap.**
Guardrail (every tool call) → RAM. Judge (session end) → swap. QMD (on-demand) → swap. 8GB + 8GB swap = 16GB virtual memory. Only 3GB resident.
*Principle: Rope and Copper + Sekel (measure which models are hot)*

### The V3 Amplifier
**Structured reasoning makes small models perform like large ones.**
DR-CoT research: 1.5B + structured protocol → competitive with 30B. Productive reasoning V3 injected into system prompts amplifies 3B models to match raw 9B+.
*Principle: Rope and Copper (amplify what you have, don't buy bigger)*

### The Training Flywheel
**Every operation produces training data.**
Model call middleware captures (input, output, latency) for every LLM call. Reasoning V3 captures (phases, gates, outcomes) for every decision. The system improves by running.
*Principle: House of Life (the repository grows with use)*

### The Git History Mine
**Committed files have N temporal versions = N× more data than current state.**
255 codegraph commits × 13K entity summaries = 350K training pairs. Session transcripts × 6 months = reasoning corpus. Don't search for data in space — search in time.
*Principle: Stretch the Cord (measure ALL dimensions, including time)*

## Iteration Patterns

### The Bent Pyramid Sequence
**mastaba → step → bent → red → great**
1st attempt: works but primitive (flat, limited)
2nd attempt: innovative but unstable (step functions, unproven)
3rd attempt: ambitious but wrong at scale (angle changes midway — visible lesson)
4th attempt: corrected, conservative, proven (first true success)
5th attempt: perfection (incorporates all lessons, achieves 2cm precision)

Apply to any design: expect iterations. The 3rd attempt WILL show you the real constraint you missed. The 4th attempt incorporates it.

### The Aggressive Prune
**Kill the cash cow to become something bigger.**
HuggingFace killed their chatbot (100K DAU) → became GitHub of AI ($4.5B). Si-Ware sold NeoSpectra (10yr product) → building something unknown. When maintaining the current thing prevents becoming the next thing, prune aggressively.
*Principle: see which constraints are real (fahlawa) — is the "cash cow" actually constraining growth?*

## Anti-Patterns

### Wishful Architecture
Designing for resources you don't have. "If we had 32GB..." or "When we get GPU..."
*Seshat response: Stretch the Cord — measure what you HAVE, not what you want.*

### Abstract Requirements
"It should be scalable and fast."
*Seshat response: Sekel — 10K requests at p99 <200ms on 2 cores. Numbers or nothing.*

### Patch Stacking
Third fix for the same boundary.
*Seshat response: Bent Pyramid — redesign the boundary, don't add another patch.*

### Knowledge Hoarding
"I know how this works" without evidence in the repository.
*Seshat response: House of Life — if it's not written down, it doesn't exist for the collective.*
