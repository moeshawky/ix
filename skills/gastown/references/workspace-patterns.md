# Workspace Patterns — Hard-Won Knowledge

Patterns discovered from maintaining the openclaw-drop Gas Town workspace.
Each entry here cost real time to discover.

## Shell & Environment

### WSL PATH breaks bash -c quoting

Windows PATH entries with parentheses (`Program Files (x86)`) are inherited by WSL.
`bash -c "python3 -c '...'"` will fail with `syntax error near unexpected token '('`.

**Fix:** Write a `.py` script file via UNC path (`\\wsl.localhost\Debian\home\moe\script.py`),
then run `python3 ~/script.py` from WSL.

**Rule:** When a shell operation fails, stop. Don't retry with different quoting.
Write a script file instead.

### Never use python3 -c inline

Even without the PATH issue, `python3 -c` with complex quoting is fragile.
Always write a proper `.py` file. User preference confirmed explicitly.

### python3 vs python on Windows

On Windows, `python3` triggers the Windows Store alias (returns "Python not found").
Use `python` instead (resolves to Python 3.13.9).
In WSL, `python3` works normally.

## OpenClaw Config

### additionalProperties: false means exactly that

Plugin config schemas reject unknown keys. Adding any key not in the schema
causes gateway startup failure.

**Before adding ANY key to a plugin config:** Read the plugin's
`openclaw.plugin.json` → `configSchema.properties` to see exactly what is allowed.

### Plugin ID must match in three places

```json
"plugins": {
  "slots":   { "memory": "<SAME-ID>" },
  "allow":   ["<SAME-ID>"],
  "entries": { "<SAME-ID>": { ... } }
}
```

One mismatch = gateway won't start.

### Known plugin ID mismatch warning

The memory-powermem plugin logs a mismatch warning at startup:
`plugin id mismatch (config uses "openclaw-extension-powermem", export uses "memory-powermem")`

This is a WARNING, not an error. Gateway starts fine. Do NOT "fix" this.

### agents.defaults.tools is NOT a valid key

Attempting to allowlist tools via `agents.defaults.tools` crashes the gateway.
The `llm-task` plugin uses `optional: true` in its extension config instead.

## MCP & mcporter

### mcporter list on session start

Always run `mcporter list` at the start of a session to discover available tools.
Don't assume tool availability from docs alone — the live state is what matters.

### Charlie-compute is a Windows EXE

Path: `C:\Users\BETAK\Desktop\Charlie\bin\charlie-compute-mcp.exe`
Not a Docker service. Must be started independently on Windows.
63 tools across 8 categories: NLP, Economics, Game Theory, Oracle, TimeSeries,
Market Data, Statistics, Security.

### SearXNG is remote (not local Docker)

SearXNG runs on AWS at `https://sear.libermoe.com`, not in local Docker.
The mcp-searxng tool accesses it via Windows environment variables.
Correct tool name: `searxng_web_search` (not `search`).

### Direct Chroma is NOT via mcporter

OpenClaw's raw Chroma access uses direct Chroma v2 at `127.0.0.1:8001` on
the AWS host through SSH tunnel. NOT via mcporter. NOT via brain-core MCP.

Collection: `default_tenant/default_database/openclaw_memory`
Embedding: `nvidia/llama-nemotron-embed-1b-v2`

```bash
~/.openclaw/workspace/scripts/openclaw_chroma_tunnel.sh start
python3 ~/.openclaw/workspace/scripts/chroma_client.py search "query"
```

## PowerMem

### API endpoints

- `POST /api/v1/memories` — add memory
- `POST /api/v1/memories/search` — search
- `GET /api/v1/system/health` — health check

NOT `/memory` or `/health` (those return 404).

### Health can be misleading

The HTTP health endpoint returns `200` even when `POST /api/v1/memories` fails
(if the configured LLM provider is unsupported). Check the journal:
```bash
journalctl --user -u powermem.service -n 100 --no-pager
```

### Scoped recall

Always use `user_id=moe`, `agent_id=personal` on both store and search.
Mismatched IDs = memories not found.

## Lobster Workflows

### workflows.run --name does NOT work for YAML files

Only works for TypeScript-registered built-in workflows. Always use:
```bash
lobster run --file ~/.openclaw/workspace/workflows/<name>.lobster
```

### Every step must have a command field

`loadWorkflowFile` validates every step has a `command` string.
Add `command: "true"` to approval-gate-only steps.

## Behavioral Patterns (for Agents)

### Execute, don't narrate

The #1 systemic issue: agents talk about doing things instead of doing them.
If you know what to do, do it in the same turn. Never dump commands for the
user to run when you can run them yourself.

### Verify before assuming

Before assuming a binary/service/path exists:
```bash
which <binary>                        # CLI tools
command -v <binary>                   # Fallback
ls <path>                             # Files
systemctl --user status <svc>         # Services
curl -s <url>/healthz                 # HTTP services
```

### Scope containment

If a task takes >5 minutes without progress:
1. Stop
2. State what's blocking
3. Ask for guidance

Do NOT build wrapper scripts, config files, or infrastructure to solve
a one-time lookup. Do NOT assume Docker is available — check `docker info`.

### Read error messages literally

- "already exists" != "fresh install"
- "No such file" != "wrong password"
- "connection refused" != "wrong credentials"

Quote the actual error in your reasoning before diagnosing.

## Dolt Safety

### Never restart without diagnostics

```bash
gt dolt status 2>&1 | tee /tmp/dolt-hang-$(date +%s).log
gt escalate -s HIGH "Dolt: <symptom>"
# THEN restart
```

### Communication hygiene

Every `gt mail send` creates a permanent bead + Dolt commit.
Every `gt nudge` creates nothing.
Default to nudge for routine agent-to-agent communication.

## Infrastructure

### lsof must be installed

`openclaw gateway restart` uses `lsof` for stale-pid cleanup.
Without it, stuck processes can block port 18789.
```bash
sudo apt install -y lsof
```

### Gateway restart fallback

If `openclaw gateway restart` fails:
```bash
sudo systemctl restart user@1000.service
```

## Gas Town on Linux/GCP

Patterns specific to running Gas Town on GCP Linux (not WSL).

### Bead prefix routing is strict

Town root beads (hq-*) CANNOT be slung to rig polecats without `--force`.
Always create beads from the rig directory so they get the rig prefix (ch-* for charlie).

```bash
# Wrong: creates hq-* bead, rejected by cross-rig guard
cd /workspace/mytown && bd create "task" -t task -p 1

# Right: creates ch-* bead, matches rig
cd /workspace/mytown/charlie && bd create "task" -t task -p 1
```

### gt rig boot vs polecat sessions

`gt rig boot <rig>` starts witness + refinery ONLY. Polecats are spawned on-demand
by `gt sling <bead> <rig>`. If you need to target a specific polecat, ensure its
tmux session exists first:

```bash
# Session naming convention: <rig-prefix>-<polecat-name>
tmux new-session -d -s ch-rust -c /path/to/polecats/rust/charlie
```

### bd show fails when bd list works

`bd list` queries current directory's beads DB. `bd show <id>` routes by prefix.
If you're in the wrong directory, show fails. Always `cd` to the directory matching
the bead's prefix before running `bd show`.

### Bare repo origin/main reference

After `gt polecat pool-init`, if sling fails with "configured default_branch not found
as origin/main", fix with:

```bash
cd /path/to/rig/.repo.git
git update-ref refs/remotes/origin/main refs/heads/main
```

### Custom types registration

If `gt sling` fails with type validation errors:

```bash
gt doctor --fix   # Canonical fix — registers Gas Town custom types
```

### Metadata alignment across databases

All `.beads/metadata.json` files must point to the same Dolt database and project UUID.
Check with `cat .beads/metadata.json` in both town root and rig directories.

## What NOT to Do

| Don't | Because | Do Instead |
|-------|---------|-----------|
| `bash -c` with complex quoting from Windows | WSL PATH breaks it | Write script file |
| `python3 -c` inline | Fragile | Write proper `.py` file |
| Add unknown keys to plugin config | Gateway crashes | Check configSchema first |
| `workflows.run --name` for YAML | Only TypeScript runners | `lobster run --file` |
| Omit `command` from approval steps | Rejected | Add `command: "true"` |
| Use `/health` for PowerMem | 404 | Use `/api/v1/system/health` |
| Assume PowerMem is running | Not always up | Check systemctl status |
| Guess LinkedIn URLs | ClawGuardian blocks | Search first, copy exact URL |
| Use Chroma via mcporter | Wrong path | Direct Chroma v2 client |
| Use mail for routine comms | Creates permanent beads | Use nudge |
| Run models on Windows | Not available | Call WSL gateway on localhost |
| Create beads at town root for rig work | Cross-rig guard rejects | `cd` into rig first |
| Sling to specific polecat without tmux | "getting pane" error | Sling to rig (auto-spawn) |
| Expect `gt rig boot` to spawn polecats | By design, it doesn't | Use `gt sling` for on-demand spawn |
| Use `bd show` from wrong directory | Prefix routing fails | Match CWD to bead prefix |
