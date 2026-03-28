---
name: ach-codebase-debugger
description: Debugging and fixing codebase issues using Analysis of Competing Hypotheses (ACH). Use this when facing failing tests, CI/build failures, runtime errors, regressions, flaky behavior, or confusing multi-causal bugs. Forces multiple falsifiable hypotheses, disconfirmation-first evidence gathering, and ranked discriminating tests before large edits.
license: Complete terms in LICENSE.txt
---

ACH (Analysis of Competing Hypotheses) is a disciplined way to debug: you do not “find the answer,” you **eliminate wrong explanations** until the least-disconfirmed cause remains. This skill makes the agent behave like a careful investigator instead of a vibe-based patcher.

This happens in three steps:
1. Symptom Freeze + Evidence Inventory
2. Competing Hypotheses + ACH Matrix (disconfirmation-first)
3. Discriminating Tests → Fix → Verification → Guardrail

First, undertake this task:

## STEP 1 — SYMPTOM FREEZE (NON-NEGOTIABLE)

Before touching code, produce a concise “freeze frame”:

- **One-sentence symptom**: what is broken.
- **Exact failure artifacts**: error text, stack trace, failing test name(s), CI step, log excerpt.
- **Expected vs actual** behavior.
- **Definition of fixed**: e.g., `cargo test` clean, CI green, perf metric back under threshold, no repro.

If any of the following are missing, ask for them **once** (in the same message) and proceed with what’s available:
- reproduction steps (local or CI)
- environment (OS, versions, toolchain, container/CI)
- last-known-good commit or “when it started”
- inputs/data involved (files, payloads, configs)

## STEP 2 — GENERATE COMPETING HYPOTHESES (ACH CORE)

### Critical rule
You must generate **at least 3 hypotheses (prefer 5–7)** before you commit to any path.

Each hypothesis must be:
- **falsifiable**
- **specific enough to test**
- written as: **If Hi is true, we should observe X and not observe Y.**

### Required coverage
Include at least:
- one **code logic** hypothesis
- one **build/config/tooling** hypothesis
- one **environment/version/OS** hypothesis
- one **data/input/edge-case** hypothesis
- when relevant: one **concurrency/race/flakiness** hypothesis

Examples of good hypotheses:
- “If this is a feature-flag default mismatch, then CI will fail in prod config but pass locally with dev defaults.”
- “If this is a dependency semver break, then pinning/reverting package X will remove the symptom without code changes.”
- “If this is a null/None edge case, then a minimal reproduction using input Y will crash deterministically at code path Z.”

## STEP 3 — EVIDENCE INVENTORY (DISCONFIRMATION-ORIENTED)

Collect evidence from:
- failing test output (assertions, expected/actual)
- stack traces, logs, panic locations
- recent diffs / dependency changes / lockfile changes
- configuration and environment variables
- build flags / feature flags / CI scripts
- runtime metrics for performance issues

When evidence is absent, propose a **cheap evidence-gathering action** (not a guess).

## STEP 4 — ACH MATRIX (DISCONFIRM FIRST)

Create a matrix with:
- rows = hypotheses (H1..Hn)
- columns = key evidence items (E1..Em)

Score each cell *primarily by disconfirmation*:

- **-2** strongly contradicts hypothesis
- **-1** somewhat contradicts
- **0** irrelevant/unknown
- **+1** somewhat consistent (weak)
- **+2** strongly consistent (rare; be skeptical)

Then compute a “survival score”:
- hypotheses with the **most negative** totals are likely false
- hypotheses with the **least disconfirming** evidence survive

### Required output format
Provide:

**A) Hypotheses list (falsifiable statements)**  
**B) Evidence list (E1..Em)**  
**C) ACH table (with scores)**  
**D) Survivors (top 2) + why they survive**

## STEP 5 — DISCRIMINATING TEST PLAN (CHEAP FIRST)

Before coding large changes, propose tests that **separate** the top hypotheses.

Rank tests by:
1) **Cost** (minutes, complexity, blast radius)  
2) **Discriminating power** (how many hypotheses it rules out)

Examples:
- run a single failing test with verbose logging
- bisect commits / `git bisect`
- temporarily pin/revert a dependency
- toggle feature flags / config variants
- run sanitizers (ASan/TSan), race detector, or fuzz input
- add targeted assertions/tracing around suspected invariant breaks
- reduce to a minimal reproduction

## STEP 6 — FIX (MINIMAL) + VERIFICATION (MANDATORY)

### Fix principles
- Prefer the **smallest safe fix** that addresses the winning hypothesis.
- Do not “refactor to feel good” unless required by evidence.
- If multiple hypotheses remain viable, do **another discriminating test** first.

### Verification is not optional
The agent must verify using at least one:
- the previously failing test(s) now pass
- reproduction no longer reproduces
- CI step succeeds (or equivalent local simulation)
- performance metric is measured and improved

If verification can’t be run here, provide exact commands and what output to expect.

## STEP 7 — GUARDRAIL (PREVENT RECURRENCE)

After the fix, add at least one:
- regression test
- assertion/invariant
- lint rule / type tightening
- documentation note (if the failure was due to misuse)

Keep this short and practical.

---

## OPERATING RULES (HARD CONSTRAINTS)

1. **No single-hypothesis debugging.** Always generate competing hypotheses first.
2. **Disconfirm-first mindset.** Evidence is used to kill hypotheses, not to hype one.
3. **Cheap tests first.** Don’t make big edits before high-info checks.
4. **No magical thinking.** If you didn’t verify, you didn’t fix.
5. **State confidence.** Provide confidence with a numeric estimate:
   - High: 0.8–1.0
   - Medium: 0.5–0.79
   - Low: <0.5

---

## REQUIRED OUTPUT TEMPLATE (ALWAYS USE)

### 1) Symptom Freeze
- Symptom:
- Expected:
- Actual:
- Failure artifacts:
- “Fixed means”:

### 2) Hypotheses (H1..Hn)
- H1:
- H2:
- H3:
- ...

### 3) Evidence (E1..Em)
- E1:
- E2:
- ...

### 4) ACH Matrix (disconfirmation-first)
| Hypothesis | E1 | E2 | E3 | E4 | Notes |
|---|---:|---:|---:|---:|---|
| H1 | 0 | -1 | +1 | -2 | ... |
| H2 | +1 | 0 | -2 | 0 | ... |
| H3 | -2 | +2 | 0 | +1 | ... |

Survivors:
- S1:
- S2:

### 5) Ranked Discriminating Tests
1. Test:
   - What it rules out:
   - Cost:
   - How to run:
2. Test:
   - ...

### 6) Decision + Fix Plan
- Winning hypothesis:
- Confidence: 0.xx
- Why others were rejected:
- Minimal fix:
- Verification steps:

### 7) Guardrail
- Regression test / assertion / docs added:

---

## TONE + STYLE
Be blunt, practical, and technical. Use concrete file paths, commands, and expected outputs. Avoid filler.
