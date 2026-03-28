# LLM Patch Failure Modes

Derived from a 49-bug post-mortem analysis of AI-generated patches. These patterns recur systematically across LLM-generated code.

## Table of Contents

1. [Five Failure Classes](#five-failure-classes)
2. [Guardrail Rules](#guardrail-rules)
3. [Compound Cascade](#compound-cascade)
4. [Detection Patterns](#detection-patterns)
5. [Platform Mitigations](#platform-mitigations)
6. [Active Checklist](#active-checklist)

## Five Failure Classes

### 1. Minimal-Patch Bias (MPB)

LLMs prefer the smallest diff that compiles. This produces fixes that address the symptom at the error site rather than the root cause.

**Signals:**
- Fix is 1-3 lines for a problem that spans multiple files
- Fix adds a null check / try-catch wrapper around the symptom
- Same bug reappears in a different location after "fixing"

**Guardrail G-ARCH-1:** Before accepting a small fix, ask: "Does the scope of this fix match the scope of the problem?" If the problem is systemic (affects multiple call sites, data flows, or components), a single-site fix is almost certainly wrong.

### 2. Template Fitting (TF)

LLMs pattern-match against training data and apply familiar templates even when the current codebase uses different patterns.

**Signals:**
- Code uses a framework idiom not present elsewhere in the codebase
- Import paths or API calls that look "standard" but don't match the project
- Fix introduces a pattern (e.g., middleware, decorator, HOC) the project doesn't use

**Guardrail G-TMPL-1:** Before applying any pattern, grep the codebase for existing instances. If zero matches, the pattern is foreign — understand the local pattern first.

### 3. Semantic Trap (ST)

Code reads correctly at a glance but contains subtle logic errors. The LLM generates plausible-looking code that passes a casual review.

**Signals:**
- `<=` where `<` was needed (off-by-one)
- `||` where `&&` was needed (inverted logic)
- Variable shadowing (same name, different scope)
- Comparison against wrong enum value
- String interpolation with wrong variable

**Guardrail G-SEM-1:** For every conditional and loop boundary in the patch, manually trace with: (a) the minimum value, (b) the maximum value, (c) the boundary value, (d) zero/empty/null. If any trace produces wrong behavior, the semantic trap is confirmed.

### 4. Plausible-but-Vulnerable (PbV)

Code works for the happy path but fails on edge cases, adversarial input, or concurrent access.

**Signals:**
- No null/undefined checks on external input
- No error handling for network/IO operations
- Shared mutable state without synchronization
- SQL/shell/HTML constructed via string concatenation
- Assumptions about data format (e.g., "this is always a number")

**Guardrail G-VULN-1:** For every external input in the patch, enumerate: null, empty string, very large input, malformed input, concurrent duplicate input. If any case is unhandled, the vulnerability is confirmed.

### 5. Stylistic Fingerprint (SF)

Generated code has inconsistent style that reveals mechanical generation and often correlates with deeper issues.

**Signals:**
- Mixing camelCase and snake_case in same scope
- Over-commenting obvious code while leaving complex logic undocumented
- Unnecessary wrapper functions / abstractions for single-use code
- Inconsistent error handling patterns within same file
- Verbose where surrounding code is terse (or vice versa)

**Guardrail G-STYLE-1:** Compare the patch's style against the 5 nearest functions in the same file. If style diverges significantly, investigate whether the stylistic mismatch indicates a deeper misunderstanding of the code's patterns.

## Compound Cascade

Failure classes rarely appear in isolation. The most dangerous pattern is a cascade:

```
Template Fitting
  -> LLM applies familiar pattern from training data
  -> Minimal-Patch Bias
     -> Fix is small because the template "almost" works
     -> Semantic Trap
        -> Template adaptation introduces subtle logic error
        -> Plausible-but-Vulnerable
           -> Logic error is hidden in an edge case path
```

**Detection:** If you find one failure class, actively scan for the others. Cascades account for ~60% of the hardest-to-find bugs in the post-mortem.

**Response:** If a cascade is detected, the entire patch approach is suspect. Return to Phase 1 (Root Cause Investigation) and re-analyze from scratch rather than fixing individual classes.

## Detection Patterns

10 patterns identified from the 49-bug post-mortem:

| ID | Pattern | What to Check |
|----|---------|---------------|
| P-1 | **Scope Mismatch** | Fix touches fewer files than the bug affects |
| P-2 | **Foreign Import** | New import/require not used elsewhere in project |
| P-3 | **Boundary Blindness** | No tests for 0, 1, max, null, empty |
| P-4 | **Happy-Path Only** | Error/edge paths not exercised in tests |
| P-5 | **Silent Swallow** | catch blocks that log but don't propagate |
| P-6 | **Type Coercion** | `==` instead of `===`, implicit string/number conversion |
| P-7 | **Stale Reference** | Fix references old API, renamed function, or removed config |
| P-8 | **Copy-Paste Drift** | Duplicated code blocks with subtle variation |
| P-9 | **Missing Await** | Async function called without await |
| P-10 | **Inverted Guard** | Early-return condition is the negation of what it should be |

## Platform Mitigations

Strategies to reduce LLM failure modes at the system level:

1. **IncrementalReasoningGuardrail** - Force the LLM to reason about scope before generating the fix. "Before writing code, list every file and function this bug could affect."

2. **ACTIONABLE_RAG** - When the LLM generates a fix, retrieve the 5 most similar past bugs and their actual solutions. If the current fix doesn't match the pattern of known solutions, flag it.

3. **Cross-Model Verification** - Have a second model review the patch specifically for the 5 failure classes. Different models have different blind spots.

4. **Negative Test Enforcement** - Require at least one test that would FAIL if the bug were reintroduced. Red-green-red cycle: test fails -> fix -> test passes -> revert fix -> test fails again.

## Active Checklist

**Invoke every 2 implementation steps:**

- [ ] G-ARCH-1: Fix scope matches problem scope?
- [ ] G-TMPL-1: Pattern exists in this codebase? (grep check)
- [ ] G-SEM-1: Traced boundary values through all conditionals?
- [ ] G-VULN-1: Enumerated adversarial inputs for all external data?
- [ ] G-STYLE-1: Style consistent with surrounding code?
- [ ] Cascade check: Found one class -> scanned for others?
- [ ] P-1 through P-10: Scanned for detection patterns?
