---
name: charlie-prompt-engineering
description: Failure-mode-aware prompt engineering for multi-agent systems. Encodes AOP v3 prompt patterns — role-specific templates, anti-pattern injection, model-class-aware construction, and verification gates. Use when writing prompts for agents, designing system prompts, dispatching work to sub-agents, or debugging why an agent produced bad output.
---

# Charlie Prompt Engineering

Prompt engineering is not "how to talk to an LLM." It's **how to prevent the 9 documented failure modes while respecting the role architecture and tool contract.**

> "The orchestrator thinks. The agents execute. Intelligence lives in the FLOW, not in any single model." — AOP v3

## The Failure-First Approach

Every prompt you write will trigger one or more failure modes. The question is not IF but WHICH. Design the prompt to suppress the likely failures for the target model class.

### The 9 Failure Modes (know these cold)

| Code | Mode | What Goes Wrong | Prompt Prevention |
|------|------|----------------|-------------------|
| G-HALL | Hallucinated APIs | Imports/methods that don't exist | Provide exact imports. BANNED: inventing APIs. |
| G-SEC | Security Vulns | SQL injection, auth bypass | Include adversarial input examples in the prompt. |
| G-EDGE | Missing Edge Cases | Empty, null, boundary, unicode | List the edge cases explicitly. Don't say "handle edge cases." |
| G-SEM | Semantic Errors | Looks right, behaves wrong | Add behavioral test: "After this change, X should still return Y." |
| G-ERR | Missing Error Handling | Happy path only | "What happens when [input] is null/empty/malformed?" |
| G-CTX | Missing Context | Works alone, fails integrated | Provide blast radius from codegraph. Show callers. |
| G-DRIFT | Model Version Drift | Same prompt, different output | Use structured output (YAML/JSON). Pin expected format. |
| G-PERF | Performance Anti-Patterns | O(n²) where O(n) exists | Specify complexity constraints when relevant. |
| G-DEP | Outdated Dependencies | Deprecated APIs | Specify exact versions. "Use X v2.0, NOT v1.x." |

**Cascade rule**: When 2+ modes appear in output, the prompt is fundamentally wrong. Don't patch — rewrite with concrete examples.

## Role-Specific Prompt Templates

Prompts MUST match the target role. A prompt for Hands that asks it to "think about" something will fail. A prompt for Witness that gives it tool access wastes its strength.

### For Hands (Code-Focused models — executes, doesn't think)

```
You are HANDS. You execute atomic operations. You do not reason, improve, or decide.

Execute these operations IN ORDER:

Op 1: file_read("path/to/file.rs")
Op 2: file_edit("path/to/file.rs",
         old="pub fn old_signature()",
         new="pub fn new_signature()")
Op 3: shell_exec("cargo check -p crate-name")

After all operations, report:
- Which succeeded (exit 0 or file found)
- Which failed (exit != 0 or file not found)
- The full output of any failed operation

BANNED:
- Writing code not specified above
- Rewriting files from scratch
- Adding functions, structs, or modules not listed
- Changing any line not mentioned
- "Improving" or "cleaning up" anything
```

**Why this works**: Atomic operations. Zero choices. Structured report. Explicit BANNED list. The model can't trigger Template Fitting (no creative space) or Stylistic Persistence (no file rewrites).

### For Witness (Reasoning models — reviews, no tools)

```
Review this diff for the following concerns:
1. SEMANTIC: Does the new code do what the old code did? Any logic changes?
2. COMPLETENESS: Are all callers updated? Any dangling references?
3. SAFETY: Any new unwrap(), panic!, or unsafe blocks?
4. EDGE CASES: What inputs would break this?

Diff:
{git_diff_output}

Context (blast radius from codegraph):
{codegraph_impact_output}

Return ONLY:
- PASS: if no issues found
- FAIL: with numbered list of specific issues
```

**Why this works**: Text-only input. Maximizes recall (Witness catches everything). No tools to misuse. Structured output prevents drift.

### For Puppeteer (Large Reasoning — plans, decomposes)

```
Given this evidence:
{codegraph_impact_output}
{file_contents}
{test_results}

Decompose into atomic operations. Each operation must:
1. Be a single action (file_edit, shell_exec, file_read)
2. Require zero decisions from the executor
3. Be independently verifiable

For each operation, specify:
- Exact tool call with exact arguments
- Expected result (what success looks like)
- What to do if it fails

Format: numbered list of atomic operations.
```

### For Deacon (Reliable Instruction — verifies, tie-breaks)

```
The Witness reviewed this diff and said: {witness_verdict}
The Hands executed these operations: {operation_results}

Your job: verify the Witness is correct. Re-read the actual file:
{actual_file_contents}

Decide:
- AGREE with Witness → explain why
- OVERRIDE Witness → explain what they missed
- ESCALATE → explain why you can't decide
```

## Model-Class-Aware Construction

Don't write prompts for "Claude" or "GPT." Write prompts for CAPABILITY CLASSES.

| Class | Strength | Weakness | Prompt Strategy |
|-------|----------|----------|----------------|
| **Large Reasoning** (>100B, CoT) | Architectural analysis, planning | Over-explains, Minimal-Patch Bias | Ask for decomposition, not implementation |
| **Code-Focused** (tool-calling) | Reliable execution, structured output | Template Fitting, rewrites files | Atomic ops only, explicit BANNED list |
| **Reasoning text-only** (analysis) | Deep analysis, high recall | May over-flag, poor tool-calling | Text input only, no tools, structured verdict |
| **Reliable Instruction** (deterministic) | Precise execution, low hallucination | Misses nuance, literal interpretation | Clear binary decisions, tie-breaking only |

## The Anti-Pattern Injection Pattern

Every prompt to a sub-agent MUST include a BANNED section tailored to that model class's failure profile:

```
BANNED:
- [For Code-Focused]: Rewriting files from scratch, empty stubs, "improvements"
- [For Reasoning]: Making tool calls, executing commands, editing files
- [For Large Reasoning]: Implementing instead of planning, skipping evidence
- [For Reliable Instruction]: Creative interpretation, scope expansion
```

## The Verification Suffix

Every prompt ends with verification:

```
After completing:
1. POSITIVE: Run {command}, confirm {expected output}
2. NEGATIVE: Confirm {bad behavior} does NOT happen
3. REPORT: structured YAML with operations_completed, operations_failed, verification results
```

## The Evidence-First Rule

**No edit prompt is dispatched without evidence.** Before writing any edit prompt:

1. `codegraph impact <entity> --depth 2` → blast radius
2. `codegraph callers <entity>` → who depends on this
3. `file_read <target>` → current state

Include this evidence IN the prompt. Models are good at processing context but BAD at predicting consequences. Evidence replaces prediction.

## Quick Reference: Prompt Debugging

When an agent produces bad output, diagnose using the failure mode table:

| Symptom | Likely Mode | Fix |
|---------|-------------|-----|
| Invented API calls | G-HALL | Provide exact imports in prompt |
| "Fixed" by rewriting entire file | Template Fitting (Axiom 2) | Use file_edit, add to BANNED list |
| Looks right, tests fail | G-SEM | Add behavioral test to verification suffix |
| Works alone, breaks integrated | G-CTX | Add codegraph blast radius to prompt |
| 3+ consecutive failures | Cascade | Stop. Rewrite prompt. Question architecture. |

## Reference

Full protocol: `charlie-gastown/references/aop-v3.md` — Sections IV (roles), IX (model selection), X (prompt construction).
