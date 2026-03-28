# THE AGENTIC ORCHESTRATION PROTOCOL (AOP) v3.0

## Preamble

AOP v1.0 defined the agent-to-agent contract: context architecture, UTCP, cognitive funneling.
AOP v2.0 added failure-aware orchestration: five axioms, model-specific prompting, compound cascade prevention.
AOP v3.0 adds **operational mechanics**: the distinction between atomic and collective operations, role architecture with model assignment, the tool contract, flow engineering, and evidence-first execution via CodeGraph.

> v3.0 is informed by: DARPA AIxCC [1], AlphaCodium flow engineering [11], Qodo multi-agent review architecture [12], NeurIPS 2025 puppeteer paradigm [13], Google scaling agent systems research [14], and 6 months of operational data from Charlie kernel development.

**The core insight**: The orchestrator thinks. The agents execute. Intelligence lives in the FLOW, not in any single model.

---

## I. THE FIVE FAILURE AXIOMS

Unchanged from v2.0. Every orchestration decision must account for these.

| # | Axiom | Signal | Guardrail |
|---|-------|--------|-----------|
| 1 | **Minimal-Patch Bias** | Fix is smaller than the problem | G-ARCH-1: blast radius check |
| 2 | **Template Fitting** | Code matches shape but misses semantics | G-TMPL-1: grep for existing patterns |
| 3 | **Semantic Trap** | Compiles correctly, behaves incorrectly | G-SEM-1: boundary value trace |
| 4 | **Plausible Vulnerability** | Passes tests, remains exploitable | G-SEC-1: CWE scan |
| 5 | **Stylistic Persistence** | Model-specific habits survive prompting | G-STYLE-1: consistency check |

---

## II. THE NINE FAILURE MODES

Extends the 5 axioms with 4 additional failure categories from operational experience [1][5][6][11].

| Code | Mode | Signal | Test Pattern |
|------|------|--------|-------------|
| G-HALL | Hallucinated APIs | Imports/methods that don't exist | Import validation, `cargo check` |
| G-SEC | Security Vulnerabilities | SQL injection, auth bypass | Adversarial input testing |
| G-EDGE | Missing Edge Cases | Empty, null, boundary, unicode | Edge case matrix, property-based tests |
| G-SEM | Semantic Errors | Plausible but wrong logic | Behavioral testing, oracle comparison |
| G-ERR | Missing Error Handling | Happy path only, no null checks | Fault injection |
| G-CTX | Missing Context | Works in isolation, fails integrated | Integration tests, env simulation |
| G-DRIFT | Model Version Drift | Same prompt, different output | Golden file regression |
| G-PERF | Performance Anti-Patterns | O(n²) where O(n) exists | Complexity bounds |
| G-DEP | Outdated Dependencies | Deprecated APIs, old versions | Dependency scanning |

**Screening order** (fail fast): G-HALL → G-SEC → G-EDGE → G-SEM → G-ERR → G-CTX → G-DRIFT.

**Cascade detection**: When 2+ modes appear in the same output, the problem is upstream. Don't fix individual bugs — fix the flow.

| Cascade | Root Cause | Action |
|---------|-----------|--------|
| G-HALL + G-SEC + G-SEM | Prompt fundamentally wrong | Rewrite prompt with concrete examples |
| G-EDGE + G-ERR | Missing domain knowledge | Provide edge cases in context |
| G-SEM + G-CTX | Integration assumptions wrong | Add integration test fixtures |
| Any 3 consecutive failures | **STOP** | Question the architecture, not the bugs |

---

## III. ATOMIC vs COLLECTIVE OPERATIONS

This is the core orchestration model. Every action an agent performs is either atomic or collective. The distinction determines who thinks and who executes.

### Atomic Operations

A single, deterministic action requiring **NO decision-making** from the agent. The agent is a HAND — it applies the operation and reports the result. If it fails, it reports failure. It does NOT attempt to fix it.

**Properties:**
1. ONE action (not "read then decide then edit")
2. DETERMINISTIC outcome (same input → same output)
3. NO interpretation required (agent reports raw result)
4. REVERSIBLE or SAFE (cannot corrupt state)
5. VERIFIABLE (success/failure is binary)

**The atomic operation set:**

| Operation | Input | Output | Agent Role |
|-----------|-------|--------|-----------|
| `file_edit(path, old, new)` | Exact strings | Success/failure + context | Hands |
| `shell_exec(command)` | Exact command | stdout + stderr + exit code | Hands |
| `file_read(path)` | File path | File contents | Any |
| `codegraph_query(cmd, args)` | Query command | Structured result | Any |
| `mcp_call(server, tool, args)` | Tool + arguments | Tool result | Any |

**CRITICAL**: `file_write` (full file rewrite) is NOT an atomic operation. It is a **destructive operation** that requires the model to regenerate the entire file — triggering Template Fitting (Axiom 2) and Stylistic Persistence (Axiom 5). Research shows LLMs introduce errors in 51%+ of full rewrites [5][15]. Use `file_edit` (str_replace) exclusively.

**Anti-pattern — the "thinking" atomic operation:**
```
# WRONG: asks the agent to make a choice
"Read this file and fix any bugs you find"

# RIGHT: atomic operations, zero choices
"file_edit(core/kernel/src/control_plane.rs,
  old='pub knowledge_engine: Option<Arc<dyn KnowledgeQueryEngine>>',
  new='pub knowledge_router: Option<Arc<KnowledgeRouter>>')"
```

### Collective Operations

A coordinated sequence of atomic operations across one or more agents, orchestrated by the **puppeteer**. The puppeteer decomposes a goal into atomic operations, assigns each to the right agent based on the Role Matrix, collects results, and decides next steps.

**Properties:**
1. MULTIPLE atomic operations in sequence
2. DECISION POINTS between operations (handled by puppeteer ONLY)
3. ROLE ASSIGNMENT (different models for different operations)
4. GATE CHECKS between phases (any gate fail → puppeteer re-plans)
5. EVIDENCE-FIRST (codegraph queries before any edit)

**The collective operation flow:**

```
COLLECTIVE: "Migrate guardrails to verdict hooks"

Phase 1: EVIDENCE (Hands — code-focused model)
  atomic: codegraph_query("impact", "Guardrail --depth 2")
  atomic: codegraph_query("callers", "validate_input")
  atomic: file_read("core/kernel/src/control_plane.rs")
  → Results returned to puppeteer

Phase 2: PLAN (Puppeteer — large reasoning model)
  Analyzes evidence, designs exact edits
  Produces: list of file_edit operations

Phase 3: EXECUTE (Hands — code-focused model)
  atomic: file_edit(path1, old1, new1)
  atomic: file_edit(path2, old2, new2)
  atomic: shell_exec("cargo check -p charlie-kernel")
  → Results returned to puppeteer

Phase 4: VERIFY (Witness — reasoning model, NO tool calls)
  Receives: the diff (git diff output)
  Returns: text analysis — issues found or "PASS"
  → Verdict returned to puppeteer

Phase 5: GATE (Puppeteer decides)
  If PASS → merge
  If FAIL → re-plan from Phase 2 with failure context
  If 3 failures → STOP, escalate to human
```

**The critical difference**: In atomic operations, agents CANNOT make choices. In collective operations, ONLY the puppeteer makes choices. Agents still execute atomically.

---

## IV. ROLE ARCHITECTURE

Four roles. Each maps to a model class with specific failure profiles.

### The Puppeteer (Orchestrator)

**Model class**: Large Reasoning
**Function**: Thinks, plans, decomposes goals into atomic operations, decides between phases, handles failures
**Tools**: ALL (but primarily uses results from other agents)
**Key rule**: The puppeteer NEVER asks an agent to "figure it out." It provides exact operations.

### The Hands (Workers)

**Model class**: Code-Focused (any model with reliable tool-calling and structured output)
**Function**: Executes atomic operations. Reads files, applies edits, runs commands, queries codegraph.
**Tools**: file_edit, shell_exec, file_read, codegraph_query, mcp_call
**Key rule**: The Hands model receives a list of atomic operations and executes them IN ORDER. It does NOT reason about them, improve them, or add steps. If an operation fails, it reports the failure and STOPS.

**Prompt template for Hands:**
```
Execute these operations IN ORDER. Do not reason. Do not improve. Do not add steps.

Op 1: file_read("core/kernel/src/control_plane.rs")
Op 2: file_edit("core/kernel/src/control_plane.rs",
         old="pub guardrails: Vec<Box<dyn Guardrail>>",
         new="// @deprecated Phase 2\npub guardrails: Vec<Box<dyn Guardrail>>")
Op 3: shell_exec("cargo check -p charlie-kernel")

After all operations, report:
- Which succeeded (exit 0 or file found)
- Which failed (exit != 0 or file not found)
- The full output of any failed operation

BANNED: Writing code not specified above. Rewriting files from scratch.
Adding functions, structs, or modules not listed. Changing any line not mentioned.
```

### The Witness (Reviewer)

**Model class**: Reasoning (text-only) — high recall, tool-calling NOT required
**Function**: Reviews diffs. Validates semantic correctness. Catches what the Hands missed. TEXT ONLY — no tool calls.
**Tools**: NONE. Receives text input (diffs, code snippets), returns text analysis.
**Key rule**: The Witness maximizes recall (catch everything). Precision is the Judge's job. The Witness should NOT use tools because its strength is reasoning, not execution. Models with poor tool-calling but strong analysis are IDEAL witnesses.

**Prompt template for Witness:**
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

### The Deacon (Judge/Overseer)

**Model class**: Reliable Instruction — precise, deterministic, low hallucination
**Function**: Cross-model gate check. Resolves conflicts between Hands and Witness. Deduplicates. Filters low-signal findings. Final approval before merge.
**Tools**: file_read, shell_exec (verification only — never edits)
**Key rule**: The Deacon sees the work of both Hands and Witness. It is the TIE-BREAKER. When Witness says FAIL but the evidence is ambiguous, the Deacon re-reads the file and decides.

---

## V. EVIDENCE-FIRST EXECUTION (The CodeGraph Advantage)

Before ANY edit, gather evidence. This replaces prediction with observation.

```
BEFORE editing, ALWAYS run:
1. codegraph_query("impact", "<entity> --depth 2")  → blast radius
2. codegraph_query("callers", "<entity>")            → who depends on this
3. codegraph_query("neighbors", "<entity>")          → direct relationships
4. file_read(<target file>)                          → current state

Evidence = "what happens when I change this?"
Context = "what does this code do?"

Models are good at processing context but BAD at predicting consequences.
Evidence replaces prediction.
```

**Why this matters**: Qodo's approach "performs comprehensive analysis upfront, then leverages this pre-computed knowledge at runtime" [12]. Our CodeGraph is this pre-computed knowledge — 14K entities, 100K relations, 60K SimilarTo edges, community detection, blast radius analysis. Every agent has access to it via `codegraph_query`. This is our structural advantage.

**The evidence gate**: No edit operation is dispatched to Hands without evidence from CodeGraph. The puppeteer queries codegraph FIRST, then designs the exact edits based on the evidence.

---

## VI. FLOW ENGINEERING

Adapted from AlphaCodium [11]: "~95% of the time we did high-level design, reasoning, and injecting data at the correct places — a.k.a. flow engineering."

**The Flow Engineering Principles:**

1. **Structured output eliminates prompt engineering**: Use YAML/JSON for all agent responses. Parsing is deterministic.

2. **Semantic reasoning via bullet points**: "Bullet points analysis encourages in-depth understanding and forces the model to divide output into logical semantic sections" [11].

3. **Modular code generation**: "Divide generated code into small sub-functions with meaningful names — fewer bugs, higher success rates for iterative fixing" [11].

4. **Test-driven iteration**: Generate code → run tests → if fail, feed error back → iterate. The flow catches bugs, not the model.

5. **Model diversity for review**: "True multi-agent orchestration requires model diversity. Synthesize the unique analytical strengths of different model families" [12]. Never use the same model for generation AND review.

---

## VII. THE TOOL CONTRACT

The exact tools available to agents. No more, no less.

### For Hands (execute)

| Tool | Signature | When |
|------|-----------|------|
| `file_edit` | `(path, old_string, new_string)` | Targeted edits. NEVER file_write. |
| `shell_exec` | `(command, timeout_secs)` | Build, test, verify. Max 300s. |
| `file_read` | `(path)` | Read before edit. Always. |
| `codegraph_query` | `(command, args)` | Evidence gathering. 14 commands. |
| `mcp_call` | `(server, tool, args)` | Hive-mind memory, search. |

### For Witness (review — text only)

| Input | Format |
|-------|--------|
| Diff | `git diff` output (text) |
| Blast radius | `codegraph impact` output (text) |
| Test results | `cargo test` output (text) |

**NO tool calls for Witness.** Models with poor tool-calling but strong reasoning are EXCELLENT witnesses — their weakness becomes irrelevant when the role requires only text analysis.

### For Deacon (verify)

| Tool | Purpose |
|------|---------|
| `file_read` | Re-read the actual file (not the diff) |
| `shell_exec` | Run verification commands |

**NEVER edits.** Only reads and runs checks.

### BANNED operations

| Operation | Why | Alternative |
|-----------|-----|-------------|
| `file_write` (full rewrite) | 51% error rate on rewrites [5][15] | `file_edit` (str_replace) |
| "Read and fix bugs" | Requires intelligent choice → axiom cascade | Exact edit operations |
| Multiple tools in one prompt | Compounds failures [7] | One tool per atomic operation |
| "Improve while you're there" | Scope creep → regression | Separate bead for improvements |

---

## VIII. VERIFICATION GATES

Five gates. A patch must pass ALL gates to merge. From llm-guardrails + advanced-debugging.

| Gate | After | Check | Rejects If |
|------|-------|-------|------------|
| **G1: Evidence** | Before edit | `codegraph impact` | No blast radius data |
| **G2: Compilation** | After edit | `cargo check` | Doesn't compile |
| **G3: Tests** | After compile | `cargo test` | Any test fails |
| **G4: Witness Review** | After tests | Different model reviews diff | Witness finds issues |
| **G5: Deacon Gate** | After review | Deacon cross-checks | Deacon overrides Witness PASS |

**The 3-Failure Rule**: If the same edit fails 3 times at any gate, STOP. The problem is architectural, not syntactic. Escalate to human. "This is NOT a failed hypothesis — this is a wrong architecture" [advanced-debugging].

---

## IX. MODEL SELECTION MATRIX

Models are classified by CAPABILITY, not by vendor. Any model that meets the capability requirements for a role can fill it. The operator configures which specific models serve each role via `services.yaml`. The protocol is provider-agnostic — it works with any OpenAI-compatible API endpoint.

### Capability Classes

| Class | Requirements | Role Fit | Failure Profile |
|-------|-------------|----------|----------------|
| **Large Reasoning** (>100B, CoT) | Chain-of-thought, architectural analysis, long context | Puppeteer, Witness | Minimal-Patch Bias, over-explains |
| **Code-Focused** (any size, tool-calling) | Reliable tool calls, structured output, follows exact instructions | Hands | Template Fitting, rewrites files |
| **Reasoning (text-only)** (any size, strong analysis) | Deep text analysis, high recall, poor tool-calling OK | Witness | Misses nothing, but may over-flag |
| **Reliable Instruction** (>100B, instruction-following) | Precise execution, follows commands literally, low hallucination | Deacon | Literal interpretation, misses nuance |

### Role → Class Mapping

| Role | Required Class | Selection Criteria | Anti-Pattern to Avoid |
|------|---------------|-------------------|----------------------|
| **Puppeteer** | Large Reasoning | Highest architectural reasoning available | Using a code model for planning |
| **Hands** | Code-Focused | Best tool-calling + structured output | Using a reasoning model for execution |
| **Witness** | Reasoning (text-only) | Highest recall, tool-calling NOT required | Using same model family as Hands |
| **Deacon** | Reliable Instruction | Most deterministic available | Using a creative/reasoning model |

### Diversity Rule

The Witness MUST be from a **different model family** than the Hands. Same-family review catches fewer bugs because models share stylistic fingerprints [7]. Cross-family review is the cheapest way to increase recall.

### Configuration

The operator maps classes to specific models in `services.yaml`. The protocol does not name models — it names capabilities. Example:

```yaml
# These are OPERATOR CHOICES, not protocol requirements.
# Any model meeting the class requirements can fill the role.
roles:
  puppeteer: {service: brain_llm}      # Whatever Large Reasoning is available
  hands: {service: brain_llm, model: <code-focused model>}
  witness: {service: brain_llm, model: <different-family reasoning model>}
  deacon: {service: brain_llm, model: <reliable instruction model>}
```

**Model selection happens BEFORE prompt writing.** Write the prompt FOR the specific model class's failure profile, not for a vendor name.

---

## X. PROMPT CONSTRUCTION PROTOCOL

Every prompt to a sub-agent includes:

### 1. Role Declaration
```
You are HANDS. You execute atomic operations. You do not reason, improve, or decide.
```

### 2. Anti-Pattern Injection (model-specific)
```
BANNED:
- Empty function bodies or stubs [Template Fitting]
- Rewriting files from scratch [Stylistic Persistence]
- "You are not X" identity patches [Minimal-Patch Bias]
- Claiming success because it compiles [Semantic Trap]
```

### 3. Exact Operations
```
Op 1: file_edit(...)
Op 2: shell_exec(...)
```

### 4. Verification Suffix
```
After completing:
1. POSITIVE: Run {command}, confirm {expected}
2. NEGATIVE: Confirm {bad thing} is blocked
3. REPORT: What changed, what works, what failed
```

### 5. Structured Output Format
```
Return YAML:
  operations_completed: N
  operations_failed: N
  failed_details:
    - op: 2
      error: "..."
  verification:
    cargo_check: pass|fail
    cargo_test: pass|fail
```

---

## XI. OPERATIONAL CHECKLIST

Before dispatching ANY work to agents:

1. [ ] **Evidence gathered?** CodeGraph impact + callers queried
2. [ ] **Model selected?** Right model class for task type
3. [ ] **Operations atomic?** Each operation requires zero choices
4. [ ] **Tool contract respected?** file_edit not file_write, no banned ops
5. [ ] **Anti-patterns injected?** Model-specific BANNED section in prompt
6. [ ] **Verification defined?** Positive + negative test specified
7. [ ] **Gates defined?** Which gates apply (G1-G5)
8. [ ] **Failure plan?** What happens at 1, 2, 3 failures
9. [ ] **Witness assigned?** Different model family than Hands
10. [ ] **Evidence fresh?** CodeGraph data is current (not from stale session)

---

## XII. REFERENCES

[1] SoK: DARPA's AI Cyber Challenge (AIxCC). arxiv.org/html/2602.07666
[2] A Systematic Study of LLM-Based Architectures for Automated Patching. arxiv.org/html/2603.01257v1
[3] The Semantic Trap: Do Fine-tuned LLMs Learn Vulnerability Root Cause or Just Functional Pattern? arxiv.org/abs/2601.22655
[4] Why LLMs Fail: A Failure Analysis for Automated Security Patch Generation. arxiv.org/html/2603.10072
[5] How Safe Are AI-Generated Patches? Security Risks in LLM Automated Program Repair. arxiv.org/html/2507.02976
[6] Patch Validation in Automated Vulnerability Repair. arxiv.org/pdf/2603.06858
[7] LLM-Generated Code Stylometry for Authorship Attribution. arxiv.org/html/2506.17323v1
[8] Why Do Multi-Agent LLM Systems Fail? NeurIPS 2025. openreview.net/forum?id=fAjbYBmonr
[9] From Spark to Fire: Modeling and Mitigating Error Cascades in LLM-Based Multi-Agent Collaboration. arxiv.org/html/2603.04474v1
[10] Towards a Science of Scaling Agent Systems. Google Research 2025. research.google/blog/towards-a-science-of-scaling-agent-systems
[11] Code Generation with AlphaCodium: From Prompt Engineering to Flow Engineering. arxiv.org/abs/2401.08500
[12] Qodo 2.0: Multi-Agent Expert Review Architecture. qodo.ai/blog/introducing-qodo-2-0-agentic-code-review/
[13] Multi-Agent Collaboration via Evolving Orchestration (Puppeteer Paradigm). arxiv.org/abs/2505.19591
[14] Why Your Multi-Agent System is Failing: The 17x Error Trap. towardsdatascience.com
[15] Prompting LLMs for Code Editing: Struggles and Remedies. arxiv.org/html/2504.20196v1
[16] Where Do AI Coding Agents Fail? Empirical Study of Failed Agentic PRs. arxiv.org/html/2601.15195
[17] Why Multi-Agent LLM Systems Fail: 79% specification + coordination failures. augmentcode.com

---

## CHANGELOG

| Version | Date | Changes |
|---------|------|---------|
| v1.0 | 2025-03 | Foundation: context architecture, UTCP, cognitive funneling |
| v2.0 | 2025-12 | Failure-aware: 5 axioms, model-specific prompting, cascade prevention |
| v3.0 | 2026-03 | Operational: atomic/collective ops, role architecture, tool contract, evidence-first, flow engineering, 9 failure modes, verification gates |
