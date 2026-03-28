---
name: advanced-debugging
description: "Systematic debugging extended with LLM patch failure mode detection, tool-specific strategies, and verification gates. Use when encountering any bug, test failure, unexpected behavior, reviewing AI-generated patches for correctness, or when a fix attempt has failed. Combines root-cause analysis (4 phases), LLM failure pattern recognition (5 classes with guardrails), hypothesis-driven fixing with architectural escalation, and evidence-based verification. Supersedes systematic-debugging, debugging-strategies, and verification-before-completion."
---

# Advanced Debugging

Random fixes waste time and create new bugs. AI-generated patches introduce systematic failure patterns. Quick patches mask underlying issues.

**Core principle:** ALWAYS find root cause before attempting fixes. Scan for LLM failure patterns. Verify with evidence.

## The Iron Law

```
NO FIXES WITHOUT ROOT CAUSE INVESTIGATION FIRST
NO COMPLETION CLAIMS WITHOUT FRESH VERIFICATION EVIDENCE
```

If you haven't completed Phase 1, you cannot propose fixes.
If you haven't run verification, you cannot claim success.

## Reasoning Protocol

**Use `productive_reason` (SEE → EXPLORE → CONVERGE → REFLECT) for root cause analysis. Use `scratchpad` to track hypotheses.** Don't hold hypotheses in working memory — write them down, update them as evidence arrives, and reflect on what changed after each attempt.

## Phase 1: Root Cause Investigation

**BEFORE attempting ANY fix:**

### Evidence-First Rule

> **Before attempting ANY fix, gather evidence via codegraph (impact, callers, neighbors) + file_read.**
>
> Models are good at processing context but BAD at predicting consequences. Evidence replaces prediction.

Run `impact`, `callers`, and `neighbors` on the failing entity before touching code. Read the actual file. Only then form a hypothesis.

1. **Read Error Messages Carefully**
   - Read stack traces completely
   - Note line numbers, file paths, error codes
   - Don't skip past errors or warnings

2. **Reproduce Consistently**
   - Can you trigger it reliably?
   - What are the exact steps?
   - If not reproducible -> gather more data, don't guess

3. **Check Recent Changes**
   - Git diff, recent commits
   - New dependencies, config changes
   - Environmental differences

4. **Gather Evidence in Multi-Component Systems**

   BEFORE proposing fixes, add diagnostic instrumentation:
   ```
   For EACH component boundary:
     - Log what data enters component
     - Log what data exits component
     - Verify environment/config propagation
     - Check state at each layer

   Run once to gather evidence showing WHERE it breaks
   THEN analyze evidence to identify failing component
   THEN investigate that specific component
   ```

5. **Trace Data Flow**
   - Where does bad value originate?
   - What called this with bad value?
   - Keep tracing up until you find the source
   - Fix at source, not at symptom
   - See `references/root-cause-tracing.md` for the full backward tracing technique

### Cascade Detection

**When 2+ distinct failure modes appear together, the problem is upstream.** Multiple symptoms at the same layer are almost never independent bugs — they share a common cause one level above. Stop investigating symptoms; find the shared ancestor.

## Phase 2: Pattern Recognition

**Find the pattern before fixing:**

1. **Find Working Examples** - Locate similar working code in same codebase
2. **Compare Against References** - Read reference implementation COMPLETELY, don't skim
3. **Identify Differences** - List every difference, however small
4. **Understand Dependencies** - What components, config, environment does this need?

### LLM Failure Mode Scan

**If the code was AI-generated or AI-patched, scan for these 5 failure classes:**

| Class | Signal | Example |
|-------|--------|---------|
| **Minimal-Patch Bias** | Fix is suspiciously small for the problem scope | One-line fix for a systemic issue |
| **Template Fitting** | Code looks copied from a different context | Wrong framework idiom, mismatched patterns |
| **Semantic Trap** | Code reads correctly but has subtle logic errors | Off-by-one, wrong operator, inverted condition |
| **Plausible-but-Vulnerable** | Works for happy path, breaks on edge cases | Missing null checks, race conditions, injection |
| **Stylistic Fingerprint** | Inconsistent style reveals mechanical generation | Mixed conventions, over-commenting, verbose wrappers |

**If any class detected -> read `references/llm-failure-modes.md` for guardrail rules and compound cascade analysis.**

### Anti-Enumeration Check

**If you've listed 3+ candidate fixes along the same axis (e.g., all in the same file, all syntactic), STOP.**

Switch axes: check callers, check tests, check the architectural assumption. Enumerating variants of the same fix is a signal you haven't found the actual root cause yet — you're searching in the wrong dimension.

## Phase 3: Hypothesis and Testing

1. **Form Single Hypothesis** - "I think X is the root cause because Y"
2. **Test Minimally** - SMALLEST possible change, one variable at a time
3. **Verify Before Continuing** - Worked? -> Phase 4. Didn't? -> NEW hypothesis, don't stack fixes
4. **When You Don't Know** - Say so. Research more. Ask for help.

## Phase 4: Implementation

### Essential vs. Accidental Complexity

**Ensure fixes address essential complexity (the actual bug) not accidental complexity (cosmetic issues nearby).** If your fix cleans up style, renames variables, or reorganizes structure without directly addressing the failure mechanism, you are fixing accidental complexity. That is not a fix — it is noise that makes the next attempt harder to reason about.

1. **Create Failing Test Case** - Simplest reproduction, automated if possible. MUST have before fixing.
2. **Implement Single Fix** - Address root cause. ONE change. No "while I'm here" improvements.
3. **Run Guardrail Checklist** (every 2 steps during implementation):
   - [ ] Does the fix match the actual scope of the problem? (G-ARCH-1)
   - [ ] Am I fitting this to a template from memory vs. understanding the specific codebase? (G-TMPL-1)
   - [ ] Have I checked for inverted conditions, off-by-ones, wrong operators? (G-SEM-1)
   - [ ] Does this handle null, empty, concurrent, and adversarial inputs? (G-VULN-1)
   - [ ] Is the style consistent with surrounding code? (G-STYLE-1)
4. **Verify Fix** - Test passes? No other tests broken? Issue actually resolved?
5. **Defense-in-Depth** - After fixing root cause, add validation at every layer data passes through. See `references/defense-in-depth.md`.

### When Fixes Keep Failing

- **< 3 failures:** Return to Phase 1, re-analyze with new information
- **>= 3 failures:** STOP. The problem is architectural — not a bad fix, a wrong model. Do NOT iterate further:
  - Is this pattern fundamentally sound?
  - Are we sticking with it through inertia?
  - Should we refactor vs. continue fixing symptoms?
  - **Escalate to your human partner with evidence before attempting more fixes**
  - Continuing to iterate after 3 failures without changing the architectural assumption is not debugging — it is thrashing

### Compound Cascade Awareness

When an LLM fix fails, check if failures are cascading:
```
Template Fitting -> Minimal-Patch Bias -> Semantic Trap -> Plausible-but-Vulnerable
```
If you see this chain: the approach is fundamentally wrong. Step back to Phase 1.

## Phase 5: Verification Gate

```
BEFORE claiming any status or expressing satisfaction:

1. IDENTIFY: What command proves this claim?
2. RUN: Execute the FULL command (fresh, complete)
3. READ: Full output, check exit code, count failures
4. VERIFY: Does output confirm the claim?
   - If NO: State actual status with evidence
   - If YES: State claim WITH evidence
5. ONLY THEN: Make the claim

Skip any step = lying, not verifying
```

| Claim | Requires | Not Sufficient |
|-------|----------|----------------|
| Tests pass | Test output: 0 failures | Previous run, "should pass" |
| Build succeeds | Build command: exit 0 | Linter passing |
| Bug fixed | Original symptom: passes | Code changed, assumed fixed |
| Requirements met | Line-by-line checklist | Tests passing |

## Red Flags - STOP and Return to Phase 1

If you catch yourself thinking:
- "Quick fix for now, investigate later"
- "Just try changing X and see"
- "Skip the test, I'll manually verify"
- "It's probably X, let me fix that"
- "One more fix attempt" (when already tried 2+)
- "Should work now" / "I'm confident" (without running verification)
- Each fix reveals new problem in different place
- "Let me try a slightly different version of this same fix" (anti-enumeration signal)

## Tool Reference

For language-specific debugger commands, git bisect, memory leak detection, performance profiling -> see `references/tools-and-techniques.md`.

## Supporting References

- **`references/root-cause-tracing.md`** - Trace bugs backward through call stack to find original trigger
- **`references/defense-in-depth.md`** - Add validation at multiple layers after finding root cause
- **`references/condition-based-waiting.md`** - Replace arbitrary timeouts with condition polling
- **`references/llm-failure-modes.md`** - 5 LLM failure classes, guardrail rules, compound cascade, 10 detection patterns from 49-bug post-mortem
- **`references/tools-and-techniques.md`** - JS/Python/Go debuggers, git bisect, memory leaks, profiling

## Quick Reference

| Phase | Key Activities | Success Criteria |
|-------|---------------|------------------|
| **1. Root Cause** | Read errors, reproduce, check changes, gather evidence (codegraph + file_read FIRST) | Understand WHAT and WHY |
| **2. Pattern** | Find working examples, compare, LLM failure scan, anti-enumeration check | Identify differences and failure class |
| **3. Hypothesis** | Form theory, test minimally, use scratchpad | Confirmed or new hypothesis |
| **4. Implementation** | Create test, fix essential complexity only, guardrail checklist, defense-in-depth | Bug resolved, tests pass, guardrails clear |
| **5. Verification** | Run commands, read output, evidence-based claims | Fresh proof before any success claim |
