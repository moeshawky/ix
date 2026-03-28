---
name: llm-guardrails
description: "Runtime guardrails for detecting and preventing LLM code generation failure patterns. Complements advanced-design (pre-implementation) and advanced-debugging (post-failure) by adding an active screening layer DURING code generation and review. Use when: (1) reviewing AI-generated code or patches before accepting, (2) dispatching coding tasks to LLM sub-agents or NIM models, (3) designing prompts for code-generation agents, (4) a fix attempt cascades into new failures, (5) validating that generated code meets security standards (CWE), (6) choosing which model to assign to a task, (7) setting up multi-agent coding workflows with verification gates. Triggers on: 'check this patch', 'review AI code', 'guardrail', 'is this safe to merge', 'dispatch to model', 'model selection', 'cascade detected', 'LLM failure'."
---

# LLM Guardrails

Use `productive_reason` for guardrail analysis. Use `scratchpad` to track which failure modes were checked.

LLM-generated code has systematic failure modes that survive prompting, fine-tuning, and model scaling. This skill provides runtime detection and prevention.

**Relationship to companion skills:**
- **advanced-design** prevents bugs at design phase (before code)
- **llm-guardrails** screens code during generation and review (during code)
- **advanced-debugging** fixes bugs with root cause analysis (after failure)

## The Five Failure Axioms

Every LLM-generated patch must be screened against these [1][2][3].

| # | Axiom | Signal | Guardrail |
|---|-------|--------|-----------|
| 1 | **Minimal-Patch Bias** | Fix is smaller than the problem | G-ARCH-1: blast radius check |
| 2 | **Template Fitting** | Code matches shape but misses semantics | G-TMPL-1: grep for existing patterns |
| 3 | **Semantic Trap** | Compiles correctly, behaves incorrectly | G-SEM-1: boundary value trace |
| 4 | **Plausible Vulnerability** | Passes tests, remains exploitable | G-SEC-1: CWE scan |
| 5 | **Stylistic Persistence** | Model-specific habits survive prompting | G-STYLE-1: consistency check |

## AOP v3 Failure Modes

All nine failure modes must be checked on every patch. Use `scratchpad` to tick each off.

| Code | Name | Signal | Action |
|------|------|--------|--------|
| **G-HALL** | Hallucination | Code references functions, types, or APIs that do not exist in the codebase | Verify every identifier against actual source before accepting |
| **G-SEC** | Security | Changed lines introduce CWE Top 10 patterns (injection, buffer issues, auth bypass, etc.) | CWE scan on every diff hunk |
| **G-EDGE** | Edge Cases | Boundary values (zero, max, empty, concurrent) not handled | Trace every conditional with min/max/zero/null |
| **G-SEM** | Semantics | Code compiles and passes tests but behaves incorrectly at runtime | Boundary value trace; test with actual data shapes |
| **G-ERR** | Error Handling | Errors silently swallowed, unwrap() on fallible paths, no propagation | Audit every `?`, `unwrap`, `expect`, `catch`, `try` |
| **G-CTX** | Context | Patch ignores caller/callee contracts, integration assumptions, or cross-module invariants | Read one hop up and one hop down in the call graph |
| **G-DRIFT** | Drift | Patch introduces style, naming, or structural patterns foreign to the codebase | Compare against existing patterns with grep before accepting |
| **G-PERF** | Performance | Algorithmic complexity regression, unbounded allocations, blocking in async context | Profile hot paths; flag O(n²) in loops |
| **G-DEP** | Dependencies | New imports, crate versions, or transitive deps added without audit | Diff Cargo.toml / package.json; check license and CVE |

## Screening Protocol

Run on EVERY AI-generated patch before accepting. Record each check in `scratchpad`.

```
1. SIZE CHECK        — Is patch smaller than problem scope?              → G-ARCH-1
2. PATTERN CHECK     — Does code use patterns foreign to codebase?       → G-TMPL-1 / G-DRIFT
3. LOGIC CHECK       — Trace every conditional with min/max/zero         → G-SEM-1 / G-EDGE
4. SECURITY CHECK    — Scan changed lines for CWE Top 10                 → G-SEC-1
5. HALLUCINATION CHECK — Every identifier exists in real source?         → G-HALL
6. ERROR HANDLING    — No silent swallows, no naked unwrap()             → G-ERR
7. CONTEXT CHECK     — Caller/callee contracts respected?                → G-CTX
8. PERF CHECK        — No complexity regression or blocking paths?       → G-PERF
9. DEP CHECK         — No new deps without audit?                        → G-DEP
10. DESTRUCTIVE REWRITE CHECK — file_write output <50% of original?      → REJECT
```

If ANY check fails, reject the patch with the specific failure code and reason.

## AOP v3 Cascade Detection

When failure codes co-occur, the issue is structural, not incidental. Match the pattern before attempting another fix.

| Cascade Pattern | Diagnosis | Prescribed Response |
|-----------------|-----------|---------------------|
| G-HALL + G-SEC + G-SEM | Prompt is fundamentally wrong | Rewrite prompt with concrete examples; do not patch the patch |
| G-EDGE + G-ERR | Missing domain knowledge in context | Provide edge cases and error contracts explicitly in the prompt context |
| G-SEM + G-CTX | Integration assumptions wrong | Add integration test fixtures that exercise the cross-module boundary |
| Any 3 consecutive failures | Architecture problem | STOP — question the architecture, not the bugs |

Three consecutive failures = **STOP**. Question the architecture, don't attempt more fixes. See [references/guardrail-rules.md](references/guardrail-rules.md) for detection patterns.

## 3-Failure Rule

If the same edit fails 3 times at any gate, STOP. The problem is architectural, not syntactic. Escalate or redesign; do not continue patching.

## AOP v3 Verification Gates

A patch must pass ALL five gates in order. No skipping, no claiming a gate passed without running its command.

| Gate | Name | What Passes |
|------|------|-------------|
| **G1** | Evidence | Every identifier, API, and import verified to exist in real source |
| **G2** | Compilation | `cargo check` / `tsc` / equivalent exits 0 with zero warnings |
| **G3** | Tests | Relevant test suite passes; no new failures introduced |
| **G4** | Witness | A second agent or reviewer independently confirms the patch is correct |
| **G5** | Deacon | Merge-queue / CI gate clears without suppression |

Verification protocol for each gate:

```
1. IDENTIFY what command proves the claim
2. RUN the command (fresh, not cached)
3. READ full output, check exit code
4. VERIFY output confirms claim
5. ONLY THEN advance to the next gate
```

## BANNED Operations (AOP v3)

These operations are prohibited in all LLM-dispatched coding tasks:

| Operation | Why Banned |
|-----------|-----------|
| `file_write` (full rewrite) | Destroys context, introduces unrelated regressions, fails the destructive-rewrite check |
| "read and fix bugs" (combined in one prompt) | Combines evidence gathering and editing — produces ~60% garbage; split them |
| Multiple tools / multiple file edits in one prompt | Exceeds task complexity threshold; each prompt must target one logical change |
| "improve while you're there" | Scope creep that bypasses the screening protocol for the improvement |

Any task that requires a banned operation must be split into discrete prompts, each passing the screening protocol independently.

## Model Selection Matrix

Match model class to task type BEFORE writing the prompt [5]:

| Task Type | Model Class | Prompt Strategy |
|-----------|-------------|-----------------|
| Architectural (>10 entities) | Large Reasoning (>200B) | Force evidence first, ban identity patches |
| Implementation (exact edits) | Code-Focused (100-200B) | Exact old→new edits, no reasoning |
| Mechanical (shell/config) | Instruction-Following | Exhaustively explicit commands |
| Micro (single function) | Small/Distilled (<30B) | Fill-in-the-middle, constrain imports |

**Critical:** Large reasoning models (>200B) must NOT have unsupervised file_write on files >300 lines. They exhibit destructive rewrite behavior. See [references/model-profiles.md](references/model-profiles.md).

## Task Complexity Threshold

Tasks >150 words or >12 LOC produce ~60% garbage outputs [1]. Split:
- One file per prompt
- One logical change per prompt
- Evidence gathering separate from editing
- Verification separate from implementation

## References

- **Guardrail rules and cascade detection**: [references/guardrail-rules.md](references/guardrail-rules.md)
- **CWE security patterns for LLM code**: [references/cwe-patterns.md](references/cwe-patterns.md)
- **Model-specific failure profiles**: [references/model-profiles.md](references/model-profiles.md)
- **Prompt construction for code agents**: [references/prompt-construction.md](references/prompt-construction.md)

## Sources

[1] SoK: DARPA's AI Cyber Challenge (AIxCC). arxiv.org/html/2602.07666
[2] A Systematic Study of LLM-Based Architectures for Automated Patching. arxiv.org/html/2603.01257v1
[3] The Semantic Trap: Do Fine-tuned LLMs Learn Vulnerability Root Cause or Just Functional Pattern? arxiv.org/abs/2601.22655
[4] Why LLMs Fail: A Failure Analysis for Automated Security Patch Generation. arxiv.org/html/2603.10072
[5] LLM-Generated Code Stylometry for Authorship Attribution. arxiv.org/html/2506.17323v1
[6] How Safe Are AI-Generated Patches? Security Risks in LLM Automated Patch Generation. arxiv.org/html/2507.02976
[7] Patch Validation in Automated Vulnerability Repair. arxiv.org/pdf/2603.06858
