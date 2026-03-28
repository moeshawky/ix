---
name: elegantify
description: Find the most elegant solution to a design or implementation problem by combining structured reasoning, cross-domain research, and rigorous design guardrails. Use when the user says "elegantify", "make this elegant", "find elegance", "break tunnel vision", or asks for the best/cleanest/most beautiful solution to a problem.
---

# Elegantify

Elegance is not simplicity. Elegance is the minimum CORRECT solution — where every part is load-bearing and nothing accidental remains. A 500-line state machine that handles every edge case IS elegant if every state earns its place. A 5-line shortcut that ignores failure modes is not.

> "Perfection is achieved, not when there is nothing more to add, but when there is nothing more to remove." — Saint-Exupéry
>
> But: what you CANNOT remove without breaking correctness is essential complexity. Respect it.

## Process

Use `productive_reason` (SEE → EXPLORE → CONVERGE → REFLECT) throughout. Use `scratchpad` to track candidates and trade-offs.

### 1. Decompose (SEE phase)

What is the ACTUAL problem (not the symptom)? What are the degrees of freedom? What constraints exist? What's already in the codebase? What would the naive solution look like — the one to beat?

Identify the **essential complexity** — the parts that are genuinely hard, not just unfamiliar. These MUST survive any refactoring.

### 2. Search Outside (EXPLORE phase)

Break tunnel vision. Search for how OTHER domains solve analogous problems. Use web search and GitHub search tools. The goal: find at least one cross-domain insight that reframes the problem.

Think: biology, physics, mathematics, architecture, music, cooking, espionage, network theory, information theory — whatever domain maps to the problem's STRUCTURE, not its surface.

**Anti-fixedness probe:** "I'm seeing this as a [X] problem. What if it's actually a [Y] problem?"

### 3. The Engineering Elegance Test

Every candidate must pass ALL six:

| Test | Question | Fail Signal |
|------|----------|-------------|
| **Essential Complexity** | Is every complex part load-bearing? Can you name WHY each piece exists? | Can't justify a component → remove it. CAN justify it → it stays, no matter how complex. |
| **Accidental Removal** | Is there boilerplate, indirection, or abstraction that serves no purpose? | Yes → strip it. But ONLY accidental complexity, never essential. |
| **Inevitability** | Feels like the ONLY right answer? | Alternatives feel equally valid → dig deeper |
| **Extension** | Add a variant without changing existing code? | No → wrong abstraction |
| **Failure Modes** | What happens when inputs are wrong, networks fail, state is corrupt? | "It won't happen" → not engineering, it's wishful thinking |
| **Negative Space** | Name 3 things you chose NOT to build and WHY? | Can't → haven't explored enough |

**WARNING:** If the solution can be explained in one sentence, check: did you solve the problem or did you simplify it away? One-sentence explanations are a signal to verify correctness, not a goal in themselves.

### 4. Verify

Before claiming the design is ready, ask: **what skills might catch something I missed?**

- Load `advanced-design` — run Phase 5 Review Gate (G-SCOPE-1, G-PATTERN-1, G-CONTRACT-1, G-SIMPLE-1). If any fails, return to step 3.
- If implementing code: load `llm-guardrails` (screens for LLM failure modes) and `advanced-debugging` (root cause analysis).
- If the domain is specialized: search for a domain-specific skill. Even 1% chance a skill exists = check.
- If the solution touches security, testing, or infrastructure: there's almost certainly a skill for it. Load it.

### 5. Present

1. One-sentence insight (what makes this approach RIGHT, not what makes it simple)
2. Architecture (types or diagram)
3. Essential complexity preserved and why
4. Accidental complexity removed and why
5. What you chose NOT to build and why

## Reference

See `references/elegance-patterns.md` for patterns discovered across sessions. Add new ones as you find them.
