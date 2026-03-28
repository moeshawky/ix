---
name: charlie-gastown
description: >
  Dispatch work to Charlie agents via provider-agnostic Python runtime with
  codegraph + hive-mind MCP tools, following the AOP v3 orchestration protocol
  (atomic/collective operations, puppeteer/hands/witness/deacon roles).
  Use when dispatching kernel work, codegraph tasks, or any Charlie rig work
  to free LLM agents. Triggers on: "dispatch to charlie", "sling to charlie",
  "charlie agent", "free inference", "dispatch kernel work", "parallel agents",
  "charlie_agent.py", "atomic operations", "collective operations", "puppeteer",
  or when planning work that should run on free LLM endpoints instead of Claude.
---

# Charlie Agent Dispatch + AOP v3 Orchestration

Provider-agnostic multi-model agent dispatch with 25+ tools. The puppeteer (you) thinks. The agents execute.

## The Core Rule

**Agents execute atomic operations. They do NOT think, decide, or improvise.**

```
WRONG: "Read this file and fix any bugs you find"
RIGHT: file_edit(path, old="broken line", new="fixed line")
```

## Quick Dispatch

```bash
# Single-shot
python3 scripts/charlie_agent.py "your prompt"
python3 scripts/charlie_agent.py --model <model-id> "your prompt"

# Via gastown
cd /workspace/charlie/charlie
ID=$(bd create "Task title" -d "description" -t task -p 2 --silent)
gt sling $ID charlie --agent charlie -m "instructions"
```

## Orchestration Roles (AOP v3)

| Role | Class | Function | Tools |
|------|-------|----------|-------|
| **Puppeteer** | Large Reasoning | Plans, decomposes, decides | All (via results) |
| **Hands** | Code-Focused (tool-calling) | Executes atomic ops IN ORDER | file_edit, shell_exec, codegraph, mcp |
| **Witness** | Reasoning (text-only, NO tools) | Reviews diffs, maximizes recall | NONE — text input only |
| **Deacon** | Reliable Instruction | Gate checks, tie-breaks | file_read, shell_exec (verify only) |

**Diversity rule**: Witness MUST be different model family than Hands.

## Collective Operation Flow

```
1. EVIDENCE: Hands → codegraph_query("impact", "<entity>")
2. PLAN: Puppeteer designs exact file_edit operations from evidence
3. EXECUTE: Hands applies atomic edits, runs cargo check
4. VERIFY: Witness reviews git diff (text only, no tools)
5. GATE: Puppeteer decides: PASS → merge, FAIL → re-plan, 3 fails → STOP
```

## Atomic Operations (the ONLY things agents do)

| Op | Input | Notes |
|----|-------|-------|
| `file_edit(path, old, new)` | Exact strings | NEVER file_write |
| `shell_exec(cmd)` | Exact command | Report output, don't interpret |
| `file_read(path)` | File path | Read before edit, always |
| `codegraph_query(cmd, args)` | Query | Evidence gathering |
| `mcp_call(server, tool, args)` | Tool call | Hive-mind memory/search |

**BANNED**: `file_write` (51% error rate on full rewrites per research [15]).

## Config

**LLM** — `.charlie/services.yaml` (any OpenAI-compatible endpoint)
**MCP** — `.charlie/mcp.yaml` (auto-discovered tools)
**Roles** — operator maps capability classes to models, not vendor names

## Verification Gates (5)

| Gate | After | Check |
|------|-------|-------|
| G1: Evidence | Before edit | codegraph impact queried |
| G2: Compile | After edit | cargo check passes |
| G3: Tests | After compile | cargo test passes |
| G4: Witness | After tests | Different model reviews diff |
| G5: Deacon | After review | Cross-check before merge |

**3-Failure Rule**: Same edit fails 3x at any gate → STOP, escalate to human.

## Prompt Template for Hands

```
Execute these operations IN ORDER. Do not reason. Do not improve.

Op 1: file_edit("path", old="...", new="...")
Op 2: shell_exec("cargo check -p charlie-kernel")

After all operations, report which succeeded and which failed.
BANNED: Rewriting files. Adding code not listed. Interpreting results.
```

## References

- [references/dispatch.md](references/dispatch.md) — Full dispatch guide: 25 tools, GUPP, architecture
- [references/aop-v3.md](references/aop-v3.md) — AOP v3: atomic/collective ops, role architecture, 9 failure modes, verification gates, flow engineering, tool contract. **Read before multi-agent work.**
