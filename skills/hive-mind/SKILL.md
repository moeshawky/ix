---
name: hive-mind
description: |
  Connect to the Hive-Mind cognitive backend — the shared subconscious for all coding agents.
  Provides memory retrieval, skill recommendations, guardrail directives, and reasoning guidance.
  Use for ANY task when the hive-mind MCP server is connected. Triggers on all substantive work.
  If you see <hive-mind-directives> or <instruction> tags in your context, this skill is active.
---

# Hive-Mind

Hive-Mind is the shared cognitive backend. If connected via MCP, use it.

## Quick Start

1. Call `query_search(query="<your task>")` before substantive work
2. Follow any `<instruction>` tags in the response
3. Use `sequential-thinking` MCP for complex reasoning
4. Load skills recommended in directives

## When You See Directives

```xml
<hive-mind-directives>
<instruction priority="high" source="...">
Follow this guidance.
</instruction>
</hive-mind-directives>
```

**Priority levels:**
- `high` — MUST follow
- `medium` — SHOULD follow
- `low` — FYI, consider

## The Four Cognitive Behaviors

When using sequential-thinking, exhibit these (Stanford "Four Habits"):
1. **Verification** — check your reasoning ("This won't work because...")
2. **Backtracking** — abandon dead ends (use `isRevision: true`)
3. **Subgoal setting** — decompose the problem (estimate `totalThoughts`)
4. **Backward chaining** — reason from the goal backward

These behaviors matter MORE than getting the right answer.

## Key MCP Tools

| Tool | When |
|------|------|
| `query_search` | Before starting work — retrieves context + directives |
| `upsert_chroma` | Store important findings for future agents |
| `inbox_send` | Coordinate with other agents |
| `training_stats` | Check training data health |

## G-TUNNEL

For design/architecture tasks: find a cross-domain analogy BEFORE software patterns.
The directive system will remind you, but internalize this.

## Feedback

When a directive helps → the system learns. When you ignore one and succeed → the system recalibrates. Your behavior IS the training signal.
