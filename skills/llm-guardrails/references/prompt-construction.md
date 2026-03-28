# Prompt Construction for Code Agents

## Table of Contents

1. [Anti-Pattern Catalog](#anti-pattern-catalog)
2. [Task Decomposition Rules](#task-decomposition-rules)
3. [Verification Suffix](#verification-suffix)
4. [Multi-Agent Workflow Patterns](#multi-agent-workflow-patterns)

## Anti-Pattern Catalog

Include model-specific anti-patterns in EVERY prompt to a sub-agent [1][5]:

```
## Anti-Patterns (DO NOT)
- Do not write empty function bodies or stubs [Template Fitting]
- Do not add "You are not X" identity patches [Minimal-Patch Bias]
- Do not rewrite files from scratch when asked to edit [Stylistic Persistence]
- Do not claim a fix works because it compiles [Semantic Trap]
- Do not add code not specified in the task [Scope Creep]
- Do not modify files larger than 300 lines via full rewrite [Destructive Rewrite]
```

## Task Decomposition Rules

Research shows tasks >150 words or >12 LOC produce ~60% garbage [1]. Decompose:

### Rule 1: One File Per Prompt
```
BAD:  "Fix lib.rs, router.rs, and main.rs"
GOOD: "Fix lib.rs: add initialize() method after connect()"
      "Fix router.rs: replace bail! with marker response"
      "Fix main.rs: load capabilities.yaml in PluginContext"
```

### Rule 2: Evidence Before Edit
```
Prompt 1 (evidence): "Read lib.rs and router.rs. Report what you find."
Prompt 2 (edit):     "Now modify lib.rs: add this specific function..."
```

Never combine "understand" and "change" in one prompt. Models stop after understanding.

### Rule 3: One Logical Change Per Prompt
```
BAD:  "Add handshake, timeout, notification, and router wiring"
GOOD: "Add handshake: send initialize request in connect()"
      "Add timeout: wrap read_line with tokio::time::timeout"
```

### Rule 4: Explicit File Write Instruction
Models with file_write access need explicit direction:
```
BAD:  "Fix the timeout issue in lib.rs"
GOOD: "Use file_write to modify libs/charlie-mcp/src/lib.rs.
       In send_request(), wrap the read_line call with tokio::time::timeout(Duration::from_secs(30), ...).
       Write the complete modified file."
```

## Verification Suffix

Append to EVERY prompt:

```
## Verification (REQUIRED)
After completing the task:
1. POSITIVE: Run {build_command} and confirm exit code 0
2. NEGATIVE: Attempt {bad_action} and confirm it is BLOCKED/REJECTED
3. REPORT: State what you changed, what works, what to test manually
```

Example for Rust:
```
## Verification (REQUIRED)
1. shell_exec cd /workspace/charlie && cargo check -p charlie-mcp 2>&1
2. shell_exec cd /workspace/charlie && cargo clippy -p charlie-mcp -- -D warnings 2>&1
3. Report: what files changed, what compiles, what the operator should test
```

## Multi-Agent Workflow Patterns

### Pattern: Worker + Witness
```
1. Worker (nemotron-super or glm5):
   - Receives exact edit instructions
   - Executes edits via file_write
   - Runs verification commands
   - Reports results

2. Witness (nemotron-ultra, READ-ONLY):
   - Receives the diff (git diff output)
   - Reviews against guardrail checklist
   - Reports: APPROVE or REJECT with specific issues
   - Does NOT have file_write access
```

### Pattern: Sequential Convoy
```
Convoy 1 (Critical) → merge to main
  ↓
Convoy 2 (High) → merge to main
  ↓
Convoy 3 (Medium) → merge to main

Each convoy:
  1. Mayor writes exact edit prompts
  2. Worker executes in isolated worktree
  3. Witness reviews diff
  4. Mayor merges on APPROVE
```

### Pattern: Evidence-First Dispatch
```
Phase 1: Gather evidence (all models can do this safely)
  - file_read target files
  - codegraph_query for callers/callees
  - shell_exec grep for patterns

Phase 2: Mayor designs edits based on evidence

Phase 3: Worker executes designed edits (exact instructions)

Phase 4: Witness reviews output (read-only)
```

The Mayor (orchestrator) ALWAYS designs the edits. Workers are HANDS, not BRAINS.
