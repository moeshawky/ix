# Charlie Agent Dispatch

> **Status**: Working 2026-03-23
> **Runtime**: `scripts/charlie_agent.py` (Python, ~200 LOC)
> **Provider**: Any OpenAI-compatible API (NIM, Venice, Ollama, Groq, custom)
> **Cost**: Zero with free endpoints. No Anthropic API usage.

---

## Overview

Charlie agents are Python processes that connect an LLM to local tools and MCP servers.
Provider-agnostic. No compilation. No binary management.

```
gt sling → spawns polecat → python3 charlie_agent.py
                                    │
                    ┌───────────────┼───────────────┐
                    ▼               ▼               ▼
              services.yaml    .charlie/mcp.yaml   GUPP env vars
              (LLM provider)   (MCP servers)       (bead context)
                    │               │               │
                    ▼               ▼               ▼
              openai SDK      HTTP JSON-RPC     bd show → prompt
                    │               │
                    └───────┬───────┘
                            ▼
                   agentic tool loop
                   (native + MCP tools)
```

---

## Setup (One-Time)

```bash
# Register the agent
gt config agent set charlie "python3 /workspace/charlie/scripts/charlie_agent.py"

# Verify
gt config agent get charlie
#   Type:    custom
#   Command: python3
#   Args:    /workspace/charlie/scripts/charlie_agent.py
```

No build step. No binary. Change the script, it takes effect immediately.

---

## Config

### LLM Provider: `.charlie/services.yaml`

The agent picks the first service with a `model` and `api_key`. Override with `--service`.

```yaml
services:
- name: brain_llm
  url: https://integrate.api.nvidia.com/v1/    # Any OpenAI-compatible endpoint
  model: moonshotai/kimi-k2-instruct-0905
  api_key: nvapi-xxx
  timeout_secs: 120
```

Switch providers by changing `url` + `model` + `api_key`. No code changes.

| Provider | URL | Notes |
|----------|-----|-------|
| NVIDIA NIM | `https://integrate.api.nvidia.com/v1/` | Free tier available |
| Venice AI | `https://api.venice.ai/api/v1/` | Free tier |
| Ollama | `http://localhost:11434/v1/` | Local, free |
| Groq | `https://api.groq.com/openai/v1/` | Free tier |
| Custom | `http://your-server:8080/v1/` | Any OpenAI-compatible |

### MCP Servers: `.charlie/mcp.yaml`

Tools from MCP servers are auto-discovered at startup. Add a server, restart — tools appear.

```yaml
servers:
  hive-mind:
    url: https://gardening.libermoe.com/mcp
    auth: "Bearer <token>"
    description: "Shared memory, semantic search, inbox"
```

Currently: **21 hive-mind tools** including memory_search, query_chroma, inbox_send, insights_query.

---

## Usage

### Single-shot

```bash
python3 scripts/charlie_agent.py "explain the kernel consolidation plan"
python3 scripts/charlie_agent.py --service mechanic_llm "refactor this function"
```

### REPL

```bash
python3 scripts/charlie_agent.py
# Drops to interactive prompt: you>
```

### Gastown dispatch

```bash
cd /workspace/charlie/charlie
bd create --title="Phase 2: guardrails to hooks" --description="..." --type=task --priority=1
gt sling ch-xxx charlie --agent charlie -m "Execute Phase 2 of kernel consolidation"
```

### Monitor

```bash
tmux capture-pane -t "ch-<polecat>" -p | tail -20
bd show ch-xxx
```

---

## Tools (25 total)

### Native (4)

| Tool | Description |
|------|-------------|
| `shell_exec` | Bash command execution with timeout (60s default, 300s max) |
| `file_read` | Read file contents (truncated at 10KB) |
| `file_write` | Write file contents (creates dirs) |
| `codegraph_query` | CodeGraph Ferrari — 14 commands across 14K+ entities |

### MCP: hive-mind (21, auto-discovered)

| Tool | Description |
|------|-------------|
| `mcp__hive-mind__memory_search` | Search all memory tiers |
| `mcp__hive-mind__memory_store` | Store new memory |
| `mcp__hive-mind__query_chroma` | Semantic vector search |
| `mcp__hive-mind__upsert_chroma` | Store to semantic memory |
| `mcp__hive-mind__inbox_send` | Send message to another agent |
| `mcp__hive-mind__inbox_task` | Delegate task to another agent |
| `mcp__hive-mind__inbox_check` | Check pending messages |
| `mcp__hive-mind__insights_query` | Query learned insight rules |
| `mcp__hive-mind__server_health` | Check hive-mind health |
| ... | + 12 more (list_collections, create_collection, etc.) |

Tools are auto-discovered via MCP `tools/list`. Adding tools to hive-mind makes them available to all agents automatically.

---

## GUPP (Gastown Universal Propulsion Principle)

When spawned by gastown, the agent auto-starts:

1. Detects `GT_POLECAT` env var → knows it's in gastown context
2. Reads `GT_BEAD_ID` → gets the assigned bead
3. Reads `GT_SLING_MESSAGE` → gets the dispatch instructions
4. Runs `bd show <bead>` → extracts full description
5. Combines sling message + description → auto-executes as first prompt
6. Drops to interactive REPL for follow-up

No tmux send-keys injection needed. The agent bootstraps itself.

---

## Architecture

```
charlie_agent.py (~200 LOC)
    │
    ├── openai.OpenAI(base_url=..., api_key=...)    # Provider-agnostic LLM
    │
    ├── Native tools                                  # Local execution
    │     shell_exec, file_read, file_write, codegraph_query
    │
    ├── MCP tools (HTTP JSON-RPC 2.0)                # Remote services
    │     Auto-discovered from .charlie/mcp.yaml
    │     SSE transport supported (Accept: text/event-stream)
    │
    ├── GUPP bootstrap                                # Gastown integration
    │     GT_POLECAT, GT_BEAD_ID, GT_SLING_MESSAGE
    │
    └── History management                            # Context window
          24K char budget, system prompt preserved
```

What this replaces:
- `charlie-ctl chat repl` (265 LOC Rust) — required compilation, no MCP, NIM-locked
- Hardcoded 5 tools → 25+ auto-discovered tools
- Manual binary rebuild → no build step
- Fragile GUPP (string parsing gt mol status) → direct env var reads

---

## Troubleshooting

| Problem | Solution |
|---------|----------|
| No LLM service found | Check `.charlie/services.yaml` has a service with model + api_key |
| MCP tools not discovered | Check `.charlie/mcp.yaml` URL and auth. Test: `curl -H "Accept: application/json, text/event-stream" -H "Authorization: Bearer <token>" <url>` |
| GUPP not firing | Check env vars: `GT_POLECAT`, `GT_BEAD_ID`, `GT_SLING_MESSAGE` |
| Tool timeout | Increase `timeout_secs` in args or set `TOOL_TIMEOUT` env var |
| Model returns garbage | Change model in services.yaml. Some models don't support tool calling |

---

## DO NOT

- **DO NOT** use `--agent claude` for bulk work — burns Anthropic tokens
- **DO NOT** hardcode model names or API keys — use services.yaml
- **DO NOT** edit charlie_agent.py for provider-specific logic — it's provider-agnostic by design
