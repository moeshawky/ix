# OpenClaw Model Access for Gas Town Agents

## Architecture

```
Claude Code (Windows)
        │
  localhost:18789          ← WSL auto-forwards ports to Windows
        │
  WSL Debian
        │
  OpenClaw Gateway         ← systemd: openclaw-gateway.service
        │
  Model Providers          ← NVIDIA NIM (free), OpenRouter, Google, Codex
```

**Rule:** The model runs inside WSL. Agents on Windows call it through
`http://localhost:18789`. Do not try to run models directly on Windows.

## Starting the Gateway

```bash
# In WSL (usually auto-started by systemd):
systemctl --user status openclaw-gateway.service

# Manual start if needed:
openclaw gateway start

# Restart after config changes:
openclaw gateway restart

# Health check:
curl http://127.0.0.1:18789/healthz
```

## API Endpoint

OpenClaw exposes an **OpenAI-compatible** chat completions endpoint:

```
POST http://localhost:18789/v1/chat/completions
Content-Type: application/json

{
  "model": "nvidia-nim/moonshotai/kimi-k2-instruct-0905",
  "messages": [
    {"role": "system", "content": "You are a helpful assistant."},
    {"role": "user", "content": "Your prompt here"}
  ],
  "temperature": 0.7,
  "max_tokens": 4096
}
```

## Full Model Inventory

### NVIDIA NIM (12 models, all free tier)

| Model ID | Reasoning | Context | Best For |
|---|---|---|---|
| `moonshotai/kimi-k2-instruct-0905` | no | 131K | Fast general tasks (PRIMARY) |
| `moonshotai/kimi-k2.5` | yes | 256K | Complex reasoning, large context |
| `moonshotai/kimi-k2-instruct` | no | 131K | General tasks (older version) |
| `deepseek-ai/deepseek-r1` | yes | 131K | Deep reasoning, math, code |
| `deepseek-ai/deepseek-v3.2` | no | 131K | Fast code, general |
| `deepseek-ai/deepseek-v3.1-terminus` | no | 131K | General tasks |
| `minimaxai/minimax-m2.5` | yes | 1M | Massive context reasoning |
| `minimaxai/minimax-m2.1` | yes | 1M | Massive context reasoning |
| `qwen/qwen3-235b-a22b` | yes | 131K | Multilingual reasoning |
| `qwen/qwen3-coder` | no | 131K | Code generation |
| `meta/llama-4-maverick-17b-128e-instruct` | no | 131K | Fast, lightweight |
| `mistralai/mistral-large-2-instruct` | no | 131K | General tasks |

### OpenRouter (20+ models)

Key models: `glm-5`, `grok-4-fast`, `devstral-2512`, `deepseek-chat`,
`mimo-v2-flash`, plus free tier options.

### Google (7 models)

`gemini-2.5-flash`, `gemini-2.5-pro`, `gemini-3.1-pro-preview`, and more.

### OpenAI Codex (1 model)

`gpt-5.2-codex` (OAuth authenticated)

## Fallback Chain

Providers are **sandwiched** so a single provider outage doesn't cascade:

```
nvidia-nim → openrouter → nvidia-nim → google → nvidia-nim → openrouter →
nvidia-nim → openai-codex → nvidia-nim → openrouter → nvidia-nim → openrouter/auto
```

No Anthropic models in the chain.

## Model Selection Guide

| Scenario | Model | Why |
|----------|-------|-----|
| Fast general task | `kimi-k2-instruct-0905` | Default, low latency |
| Complex reasoning | `kimi-k2.5` or `deepseek-r1` | Chain-of-thought |
| Huge context (100K+ tokens) | `minimax-m2.5` | 1M token window |
| Code generation | `qwen3-coder` or `deepseek-v3.2` | Code-optimized |
| Multilingual | `qwen3-235b-a22b` | Strong non-English |
| Budget fallback | `openrouter/auto` | Auto-routes cheapest |

## API Key Resolution

OpenClaw resolves API keys in this order:
1. Auth profiles (`auth.profiles.<provider>`)
2. Environment variable (e.g., `NVIDIA_API_KEY`)
3. Provider config (`models.providers.<provider>.apiKey`)

All NVIDIA NIM keys are on the free tier — no billing.

## Configuration

Master config: `/home/moe/.openclaw/openclaw.json` (~1,142 lines)

Key paths in the config:
- `agents.defaults.model.primary` — default model
- `agents.defaults.model.fallbacks` — fallback order
- `agents.defaults.models` — allowlist (only these show in pickers)
- `models.providers.nvidia-nim` — provider definition + model metadata

## Troubleshooting

**Gateway won't start:**
```bash
openclaw doctor          # Names the exact failing key
```
Common cause: invalid key in `openclaw.json` (strict schema validation).

**Model returns errors:**
- Check provider is reachable: `curl https://integrate.api.nvidia.com/v1/models`
- Check API key: look in `openclaw.json` → `env.vars.NVIDIA_API_KEY`
- Fallback will auto-engage if primary fails

**Port not forwarding from Windows:**
WSL2 auto-forwards ports bound to `0.0.0.0` or `127.0.0.1`.
If not working, check: `ss -tlnp | grep 18789` in WSL.

**Gateway restart requires sudo:**
If `openclaw gateway restart` fails, `lsof` may be missing:
```bash
sudo apt install -y lsof   # Needed for stale-pid cleanup
```
Fallback: `sudo systemctl restart user@1000.service`
