# Model-Specific Failure Profiles

## Table of Contents

1. [Model Class Taxonomy](#model-class-taxonomy)
2. [Tested Model Profiles](#tested-model-profiles)
3. [Access Control Matrix](#access-control-matrix)
4. [Prompt Strategy by Class](#prompt-strategy-by-class)

## Model Class Taxonomy

Based on AOP v2 [1] and field testing (2026-03-20 convoy dispatch):

| Class | Size | Strength | Weakness | Example Models |
|-------|------|----------|----------|----------------|
| Large Reasoning | >200B | Deep architectural analysis | Destructive rewrite on file_write | nemotron-ultra-253b, Claude Opus |
| Code-Focused | 100-200B | Targeted surgical edits | Needs follow-up prompts, stops after evidence | nemotron-super-120b, Qwen-397B MoE |
| Instruction-Following | 50-200B | Reliable execution of explicit instructions | Won't fill gaps or infer intent | glm5, Llama-405b |
| Small/Distilled | <30B | Fast, cheap | Hallucinates APIs, fabricates imports | Llama-8b, Qwen-7b |

## Tested Model Profiles

### nvidia/nemotron-3-super-120b-a12b
- **Class:** Code-Focused (120B MoE, 12B active)
- **Status:** PROVEN WORKER
- **Strengths:** Correct tool calling, reads files before editing, writes targeted edits on files <300 lines, fast response
- **Weaknesses:** Stops after evidence gathering (needs explicit "now edit" follow-up), minor compilation errors (missing features, type annotations)
- **Safe for:** file_read, file_write (<300 lines), shell_exec, codegraph_query
- **Observed behavior:** Called file_read x3 in one pass, wrote correct MCP handshake code, needed tokio `time` feature fix

### nvidia/llama-3.1-nemotron-ultra-253b-v1
- **Class:** Large Reasoning (253B)
- **Status:** DANGEROUS AS WORKER, SAFE AS REVIEWER
- **Strengths:** Unknown (destroyed files before demonstrating any)
- **Weaknesses:** Destructive rewrite behavior — overwrites entire files with stubs. Destroyed core/types/src/lib.rs (2000+ lines → 11), plugins/brain/src/plugin.rs (1107 lines → 2), Cargo.toml manifests, .yaml configs
- **Safe for:** file_read ONLY. No file_write access.
- **Observed behavior:** AOP v2 Axiom 2 (Template Fitting) at scale. Created stub types that compiled but destroyed the codebase. Looped: saw compile error from own destruction, "fixed" by destroying more files.

### z-ai/glm5
- **Class:** Instruction-Following
- **Status:** UNTESTED AS WORKER, user reports near-opus quality
- **Safe for:** Likely all tools with explicit instructions
- **Response time:** Sub-second for simple queries

### qwen/qwen3.5-397b-a17b
- **Class:** Code-Focused (397B MoE, 17B active)
- **Status:** UNAVAILABLE (NIM endpoint timeout at 120s)
- **Note:** Previously proven excellent by user, likely capacity issue

## Access Control Matrix

| Model | file_read | file_write <300L | file_write >300L | shell_exec | Review |
|-------|-----------|------------------|------------------|------------|--------|
| nemotron-super | YES | YES | SUPERVISED | YES | YES |
| nemotron-ultra | YES | NO | NO | READ-ONLY | YES (preferred) |
| glm5 | YES | YES | SUPERVISED | YES | YES |
| qwen3.5 | YES | YES | SUPERVISED | YES | YES |
| Small (<30B) | YES | FILL-IN-MIDDLE ONLY | NO | NO | NO |

**SUPERVISED** = human reviews diff before accepting. **FILL-IN-MIDDLE ONLY** = provide surrounding code, ask only for the missing section.

## Prompt Strategy by Class

### For Code-Focused Models (Workers)
```
Execute these edits IN ORDER. Do not reason about them. Do not improve them.

Edit 1:
  File: {path}
  Old: {exact old string, 3+ lines of context}
  New: {exact new string}

After all edits, run: {verify command}
Report the output. Do not interpret it.

BANNED: Writing code not specified above. Rewriting files from scratch.
```

### For Large Reasoning Models (Reviewers)
```
Review this diff. You do NOT write code. You:
1. Check if the patch scope matches the problem scope (G-ARCH-1)
2. Trace every conditional with boundary values (G-SEM-1)
3. Scan for CWE patterns in changed lines (G-SEC-1)
4. State: APPROVE or REJECT with specific line numbers

BANNED: Suggesting alternative implementations. Writing code.
```

### For Instruction-Following Models (Mechanical Tasks)
```
Execute exactly {N} commands in sequence.
After each, report exit code and first 20 lines of output.
Do not skip, reorder, or add commands.

Command 1: {exact command}
Command 2: {exact command}
...

When complete, report: how many succeeded, how many failed, full output of failures.
```
