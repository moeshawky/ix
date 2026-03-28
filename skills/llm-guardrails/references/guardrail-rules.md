# Guardrail Rules and Cascade Detection

## Table of Contents

1. [Guardrail Rule Index](#guardrail-rule-index)
2. [Detailed Rules](#detailed-rules)
3. [Compound Cascade](#compound-cascade)
4. [Active Checklist](#active-checklist)

## Guardrail Rule Index

| ID | Rule | Axiom | When to Apply |
|----|------|-------|---------------|
| G-ARCH-1 | Scope match | Minimal-Patch Bias | Before accepting any small fix |
| G-TMPL-1 | Pattern grep | Template Fitting | Before applying any pattern |
| G-SEM-1 | Boundary trace | Semantic Trap | Every conditional/loop in patch |
| G-SEC-1 | CWE scan | Plausible Vulnerability | Every changed line |
| G-STYLE-1 | Consistency | Stylistic Persistence | Cross-model reviews |
| G-MEM-1 | No data privacy assumption | Memorization | Any training data handling |
| G-CTX-1 | Position critical info at boundaries | Context Loss | Prompt construction |
| G-CTX-2 | Never rely on mid-context retrieval | Context Loss | Long prompts |
| G-SEM-2 | Input validation needs human review | Semantic Trap | CWE-20 fixes |
| G-SEM-3 | Reject >150 words or >12 LOC tasks | Semantic Trap | Task sizing |
| G-DEST-1 | Reject file_write <50% original size | Template Fitting | File write review |

## Detailed Rules

### G-ARCH-1: Blast Radius Match
Before accepting a small fix, ask: "Does the scope of this fix match the scope of the problem?" If the problem is systemic (affects multiple call sites, data flows, or components), a single-site fix is almost certainly wrong [1].

### G-TMPL-1: Pattern Foreign Check
Before applying any pattern, grep the codebase for existing instances. If zero matches, the pattern is foreign — understand the local pattern first [2].

### G-SEM-1: Boundary Value Trace
For every conditional and loop boundary in the patch, manually trace with: (a) the minimum value, (b) the maximum value, (c) the boundary value, (d) zero/empty/null. If any trace produces wrong behavior, the semantic trap is confirmed [3].

### G-SEC-1: CWE Scan
Scan changed lines for: CWE-476 (null deref), CWE-787 (OOB write), CWE-416 (use-after-free), CWE-119 (buffer overflow), CWE-400 (resource exhaustion), CWE-190 (integer overflow). See [cwe-patterns.md](cwe-patterns.md) for language-specific detection.

### G-STYLE-1: Cross-Model Consistency
Each model has characteristic failure modes that survive prompting [5]. Same-family review catches fewer bugs. Always use a DIFFERENT model family for review than the one that generated the code.

### G-DEST-1: Destructive Rewrite Detection
If a file_write produces output that is less than 50% of the original file's line count, the model has likely performed a destructive rewrite — replacing the entire file with a stub that compiles but destroys functionality. REJECT immediately and restore from git.

## Compound Cascade

Failures compound. A single model can trigger all five axioms simultaneously:

```
Step 1: Model receives "fix the handler"
Step 2: Template Fitting → writes stub matching handler shape
Step 3: Minimal-Patch Bias → stub is smallest change that compiles
Step 4: Semantic Trap → stub passes cargo check + clippy
Step 5: Plausible Vulnerability → program launches without error
Step 6: Stylistic Persistence → model's "rebuild from scratch" habit persists
Result: Empty handler shipped. User sees broken feature.
```

### Detection Signals
- Each fix reveals a new problem in a DIFFERENT place
- The fix count exceeds 3 for what seemed like 1 issue
- The model keeps re-reading the same file without making progress
- File sizes decrease after "fixes"

### Response
After 3 consecutive failures: STOP. Do not attempt fix #4. Instead:
1. Revert all changes (`git checkout -- <files>`)
2. Re-read the original code
3. Question whether the approach is architecturally sound
4. Discuss with the operator before resuming

## Active Checklist

Run every 2 steps during implementation:

- [ ] Does the fix match the actual scope of the problem? (G-ARCH-1)
- [ ] Am I fitting this to a template from memory vs. understanding the specific codebase? (G-TMPL-1)
- [ ] Have I checked for inverted conditions, off-by-ones, wrong operators? (G-SEM-1)
- [ ] Does this handle null, empty, concurrent, and adversarial inputs? (G-SEC-1)
- [ ] Is the style consistent with surrounding code? (G-STYLE-1)
- [ ] Is the output file similar in size to the input file? (G-DEST-1)
