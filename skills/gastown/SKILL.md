---
name: gastown
description: >
  Gas Town (gt) multi-agent workspace orchestration with beads issue tracking
  and OpenClaw model access. Use when the user asks to dispatch work to agents,
  manage beads/issues, sling tasks to polecats, create convoys, use formulas,
  communicate between agents, manage rigs, coordinate multi-agent workflows,
  or call OpenClaw models. Also use when the user mentions "gt", "gastown",
  "sling", "bead", "polecat", "convoy", "formula", "handoff", "hook",
  "dispatch", "openclaw model", or "deacon". Covers: work dispatch (gt sling),
  issue tracking (bd), agent lifecycle, communication (nudge/mail/escalate),
  molecules, diagnostics, and OpenClaw model gateway access from Windows.
---

# Gas Town Agent Orchestration

gt v0.11.0 manages multi-agent workspaces. bd (beads) v0.59.0 tracks issues.
OpenClaw models run inside WSL and are accessed via localhost port forwarding.

## Architecture

```
Windows 10 (BETAK)
  Claude Code / Gas Town agents
        │
        │  http://localhost:<port>/v1/chat/completions
        │  (WSL auto-forwards ports to Windows)
        │
        ▼
  WSL Debian ──────────────────────────────────────────────
  │
  ├── OpenClaw Gateway        http://127.0.0.1:18789
  │     └── 12 NVIDIA NIM models (free), OpenRouter, Google, Codex fallbacks
  │
  ├── Gas Town Root (~/gt/ or project root)
  │     ├── mayor/            Chief of Staff - cross-rig coordination
  │     ├── deacon/           Town-level watchdog + dogs (infrastructure workers)
  │     ├── <rig>/            Project container
  │     │     ├── refinery/   Merge queue processor
  │     │     ├── witness/    Per-rig polecat health monitor
  │     │     ├── polecats/   Worker agents (persistent identity, ephemeral sessions)
  │     │     ├── crew/       Human workspaces
  │     │     └── .beads/     Rig-level issue tracking
  │     └── .beads/           Town-level issue tracking (hq-* prefix)
  │
  ├── PowerMem                http://127.0.0.1:43117  (memory backend)
  ├── mcporter                MCP bridge to charlie-compute, SearXNG
  └── Chroma (via SSH tunnel) http://127.0.0.1:8001   (vector store on AWS)
```

**Key rule:** OpenClaw models run inside WSL, not on Windows. Claude Code agents
call models through `http://localhost:<port>` — WSL auto-forwards the port.

## OpenClaw Model Access

### Starting the Server (in WSL)

```bash
# Start the gateway (if not already running via systemd)
openclaw gateway start
# Or restart after config changes:
openclaw gateway restart
# Verify:
curl http://127.0.0.1:18789/healthz
```

The gateway runs as `openclaw-gateway.service` under systemd and auto-starts.

### Calling Models from Claude Code / Agents

```bash
# OpenAI-compatible endpoint (from Windows or WSL — same localhost)
curl http://localhost:18789/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"nvidia-nim/moonshotai/kimi-k2-instruct-0905","messages":[{"role":"user","content":"hello"}]}'
```

### Available Models (Primary + Fallback Chain)

```
Primary: nvidia-nim/moonshotai/kimi-k2-instruct-0905    (131K ctx, free)
  → nvidia-nim/moonshotai/kimi-k2.5                     (256K ctx, reasoning, free)
  → openrouter/z-ai/glm-5
  → nvidia-nim/deepseek-ai/deepseek-v3.2                (131K ctx, free)
  → google/gemini-2.5-flash
  → nvidia-nim/minimaxai/minimax-m2.5                   (1M ctx, reasoning, free)
  → openrouter/x-ai/grok-4-fast
  → nvidia-nim/minimaxai/minimax-m2.1                   (1M ctx, reasoning, free)
  → openai-codex/gpt-5.2-codex
  → nvidia-nim/deepseek-ai/deepseek-r1                  (131K ctx, reasoning, free)
  → openrouter/openrouter/auto                          (last resort)
```

All 12 NVIDIA NIM models are free tier. No Anthropic models in the fallback chain.

### Model Selection Rules

- Use `kimi-k2-instruct-0905` for fast, non-reasoning tasks (default)
- Use `kimi-k2.5` or `deepseek-r1` when reasoning/chain-of-thought is needed
- Use `minimax-m2.5` for massive context (1M tokens)
- The fallback chain sandwiches providers (nvidia→openrouter→google→nvidia→...) so a single provider outage doesn't cascade

## Quick Reference

### Identity & Context
```bash
gt whoami                    # Current identity
gt prime                     # Full role context (run after compaction/new session)
gt status                    # Town overview: services, agents, rigs
export GT_ROLE=mayor         # Set role (mayor|deacon|witness|refinery|crew)
```

### Work Dispatch (The Core Loop)

**gt sling is THE command for assigning work.**

```bash
# Self-assign
gt sling <bead-id>                        # Hook to yourself

# Dispatch to agents
gt sling <bead> <rig>                     # Auto-spawn polecat in rig
gt sling <bead> <rig> --create            # Create polecat if missing
gt sling <bead> <rig>/<polecat>           # Specific polecat (session must exist)
gt sling <bead> mayor                     # To mayor
gt sling <bead> deacon/dogs               # Auto-dispatch to idle dog
gt sling <bead> crew                      # To crew worker

# With instructions (natural language - the executor is an LLM)
gt sling <bead> <target> -a "focus on security"
gt sling <bead> <target> --stdin <<'EOF'
Multi-line instructions here
EOF

# Batch dispatch (parallel)
gt sling <b1> <b2> <b3> <rig>            # Each gets own polecat
gt sling <b1> <b2> <rig> --max-concurrent 3

# Formula dispatch
gt sling shiny <target> --var feature="auth system"
gt sling mol-polecat-work <target>

# Merge strategy
gt sling <bead> <rig> --merge=direct      # Push to main
gt sling <bead> <rig> --merge=mr          # Merge queue (default)
gt sling <bead> <rig> --merge=local       # Keep on branch
```

### Sling Gotchas (Critical — Read This)

**1. Bead prefix must match target rig.**
`gt sling` enforces a cross-rig guard: beads with prefix `hq-` (town root) cannot be
slung to rig polecats without `--force`. The fix: **create beads from within the rig directory**.

```bash
# WRONG — creates hq-* bead, rejected when slinging to charlie rig:
cd /workspace/mytown && bd create "Fix bug" -t task -p 1
gt sling hq-abc charlie/rust   # ERROR: "belongs to town root"

# RIGHT — creates ch-* bead, matches charlie rig:
cd /workspace/mytown/charlie && bd create "Fix bug" -t task -p 1
gt sling ch-abc charlie/rust   # Works

# OVERRIDE — force town-root bead to rig (use sparingly):
gt sling hq-abc charlie/rust --force
```

**2. Sling to rig (auto-spawn) vs specific polecat.**
Slinging to a **rig name** auto-spawns a polecat. Slinging to a **specific polecat**
(`rig/name`) requires the tmux session to already exist.

```bash
# Auto-spawn (preferred) — gt creates tmux session + worktree:
gt sling ch-abc charlie

# Specific polecat — tmux session must exist:
gt sling ch-abc charlie/rust   # Fails if ch-rust tmux session doesn't exist

# If you need to target a specific polecat, ensure session exists first:
tmux new-session -d -s ch-rust -c /path/to/polecats/rust/charlie
gt sling ch-abc charlie/rust   # Now works
```

**3. `gt rig boot` does NOT spawn polecats.**
Boot only starts witness + refinery (persistent infrastructure). Polecats are
spawned on-demand by `gt sling <bead> <rig>`. This is by design.

```bash
gt rig boot charlie   # Starts witness + refinery ONLY
# To get polecats: sling work to the rig (auto-spawn) or create sessions manually
```

**4. `bd show` vs `bd list` — prefix routing.**
`bd list` queries the current directory's bead database. `bd show <id>` routes by
prefix: `ch-*` → rig database, `hq-*` → town root. If you're in the wrong directory,
`bd show` won't find the bead even though `bd list` shows it.

```bash
# If bd show fails but bd list shows the bead:
cd /path/to/rig && bd show ch-abc   # Query from rig directory
# Or from town root for hq-* beads:
cd /path/to/town && bd show hq-abc
```

**5. Custom types must be registered.**
Gas Town uses custom bead types (agent, role, rig, convoy, etc.) that must be
registered in the Dolt database. If `gt sling` fails with type validation errors:

```bash
gt doctor --fix   # Canonical fix — registers all custom types
# Manual workaround (if doctor fails):
bd config set types.custom "agent,role,rig,convoy,slot,queue,event,gate,merge-request"
```

**6. Metadata alignment.**
All `metadata.json` files (town root + each rig) must point to the same Dolt database
and project UUID. Misalignment causes prefix routing failures. Check with:

```bash
cat .beads/metadata.json          # Town root
cat charlie/.beads/metadata.json  # Rig
# Both should have same dolt_database and project_id
```

### Hook (Durability Primitive)

Work on a hook survives session restarts, compaction, handoffs.

```bash
gt hook                      # Show what's on your hook
gt hook <bead>               # Attach work
gt hook <bead> <target>      # Attach to another agent's hook
gt unsling                   # Remove work from hook
```

**Compare dispatch commands:**
- `gt hook <bead>` - Attach only (no action)
- `gt sling <bead>` - Attach + start now (keep context)
- `gt handoff <bead>` - Attach + restart (fresh context)

### Communication

```bash
# Nudge (ephemeral, no bead created - DEFAULT for routine comms)
gt nudge <target> "message"

# Mail (permanent bead - use for handoffs, structured protocol)
gt mail send <target> -s "subject" -b "body"
gt mail list                 # Check inbox

# Escalation (severity-routed)
gt escalate "description" -s high
gt escalate "description" -s critical   # Notifies overseer
gt escalate ack <id>                    # Acknowledge
gt escalate close <id> --reason "fixed" # Resolve

# Broadcast
gt broadcast "message to all workers"
```

**Rule**: Default to nudge. Only use mail when message must survive session death.

## Beads Issue Tracking (bd)

### Core Commands
```bash
# Create
bd create "title" -d "description" -t task -p 2 -l "label1,label2"
bd create "title" -d "desc" -t bug -p 1 --silent   # Output only ID

# Query
bd ready                     # Show work ready (no blockers)
bd list --status=open        # All open
bd list --status=in_progress # Active work
bd show <id>                 # Full details + deps
bd search "keyword"          # Text search
bd blocked                   # Show blocked issues

# Update
bd update <id> --status=in_progress
bd update <id> --claim       # Claim atomically
bd update <id> --notes="progress update"
bd close <id>                # Mark complete
bd close <id1> <id2> <id3>   # Batch close

# Dependencies
bd dep add <issue> <depends-on>
bd dep add <new> <parent> --type discovered-from   # Link discovered work
```

**Priority**: 0-4 (P0=critical, P2=medium, P4=backlog). NOT "high"/"medium".
**Types**: bug, feature, task, epic, chore.

### Agent Beads (v0.40+)
```bash
bd create "Worker name" --type=agent    # Track agent state
bd agent state <id> running             # Update agent state
bd slot set <agent-id> hook <bead-id>   # Assign work to agent
```

Agent states: `idle → spawning → running/working → done → idle` (also: `stuck`, `stopped`, `dead`)

### Molecules & Chemistry

Formulas are reusable workflow templates. Molecules are instances.

```bash
# List and inspect
gt formula list              # Available formulas
gt formula show <name>       # Show steps and variables

# Spawn workflows
bd mol spawn <proto>                     # Create wisp (ephemeral, default)
bd mol spawn <proto> --pour              # Create mol (persistent)
bd mol run <proto> --var key=value       # Spawn + assign + pin (durable)

# During execution
gt mol current               # What should I work on?
gt mol progress              # Show step progress
gt mol step done             # Complete current step (auto-continues)

# End lifecycle
gt mol squash                # Compress to digest (permanent record)
gt mol burn                  # Discard (no record)

# Chemistry shortcuts
bd mol pour <proto>          # Persistent instance
bd mol wisp <proto>          # Ephemeral instance
bd mol distill <epic>        # Extract template from ad-hoc work
bd mol bond A B              # Combine workflows (sequential)
bd mol bond A B --type parallel    # Combine (parallel)
```

**Phase transitions:**
- Proto (solid/template) → `pour` → Mol (liquid/persistent)
- Proto → `wisp create` → Wisp (vapor/ephemeral)
- Wisp → `squash` → Digest (permanent summary)
- Wisp → `burn` → Nothing (deleted)
- Ad-hoc epic → `distill` → Proto (reusable template)

### Session Protocol — Brain-Core Integrated (The Full Loop)

Every agent session follows RECALL → PLAN → EXECUTE → LEARN. Brain-core provides
institutional memory; Gas Town provides durable work queues. Together they ensure
no knowledge is lost across sessions.

#### Phase 0: ORIENT (session start, ~10s)
```bash
gt prime                     # Identity + hook + context injection
gt vitals                    # Infrastructure health (Dolt, tmux, git)
```
Brain-core hooks fire automatically on SessionStart — working memory blocks
(preferences, pending_items, project_context) are injected into context.

#### Phase 1: RECALL (before touching code)
Query brain-core for what's already known. This replaces expensive file reads
and prevents re-discovering solved problems.

```bash
# Via MCP tools (agents call these directly):
mcp__brain-core__query_chroma(collection="rig_<GT_RIG>", query="<task description>")
mcp__brain-core__query_chroma(collection="rig_<GT_RIG>", query="<task description>", category="error")
mcp__brain-core__memory_search(query="<key concept>")
```

**What to recall by category:**
- `architecture` — codebase structure, module layout, key files
- `pattern` — tested approaches, build commands, working code patterns
- `error` — known failures, gotchas, time-wasting traps to avoid
- `preference` — coding style, framework choices, reviewer expectations

**Rule:** If brain-core returns relevant context, USE IT. Don't re-read files
that brain-core already summarized. Brain-core context is pre-digested.

#### Phase 2: PLAN (claim + strategize)
```bash
bd ready                     # Find unblocked work
bd show <id>                 # Full context + deps
bd update <id> --claim       # Claim atomically
```
Combine brain-core recall with bead context to form a plan.
If brain-core returned past errors for this area, address them preemptively.

#### Phase 3: EXECUTE (do the work)
Normal development cycle. Add notes as you work for compaction survival:
```bash
bd update <id> --notes="COMPLETED: X. IN_PROGRESS: Y. NEXT: Z. BLOCKER: none"
```

#### Phase 4: LEARN (after completing work)
Upsert outcomes back to brain-core so the NEXT agent benefits.

```bash
# What worked — upsert as pattern:
mcp__brain-core__upsert_chroma(
  collection="rig_<GT_RIG>",
  category="pattern",
  content="<what you did, what worked, why>"
)

# What broke — upsert as error:
mcp__brain-core__upsert_chroma(
  collection="rig_<GT_RIG>",
  category="error",
  content="<what failed, root cause, fix>"
)

# Architecture changes — upsert as architecture:
mcp__brain-core__upsert_chroma(
  collection="rig_<GT_RIG>",
  category="architecture",
  content="<new module, changed structure, key decision>"
)
```

**What to LEARN (upsert):**
- Patterns that worked (code approaches, build tricks, test strategies)
- Errors encountered (with root cause — saves the next agent hours)
- Architecture changes (new files, restructured modules, dependency changes)
- Gotchas (counterintuitive behavior, undocumented constraints)

**What NOT to learn:**
- Ephemeral state (current branch, in-progress work — that's beads)
- Things derivable from code (function signatures, file contents — that's codegraph)
- Raw diffs (that's git)

#### Phase 5: CLOSE (hand off cleanly)
```bash
bd close <id> --reason "..."     # Complete task
gt done                          # Submit to merge queue
# Or:
gt done --status ESCALATED       # Hit blocker
gt done --status DEFERRED        # Pause work
gt handoff                       # End session, hand off to fresh agent
gt handoff -c                    # Collect state into handoff message
```

### Brain-Core Collection Naming Convention

| Collection | Scope | What goes in |
|-----------|-------|-------------|
| `rig_<name>` | Per-rig | Architecture, patterns, errors, preferences for that codebase |
| `claude_code` | Per-user | Cross-rig patterns, user preferences, gt operational knowledge |
| `mechanic` | Per-plugin | Plugin-specific patterns, SOPs, architectural decisions |
| `brain_core_system` | Platform | Brain-core design notes, inspiration, novel synthesis |
| `training_data` | Auto-captured | LLM-judged training pairs from sessions (stop hook) |

**Convention:** Always specify `collection="rig_<GT_RIG>"` when upserting rig-specific
knowledge. Use `claude_code` for cross-cutting knowledge. The rig collection is the
primary recall source; agent collections are secondary.

### Brain-Core Quick Reference
```bash
# Health check
mcp__brain-core__server_health()

# Recall (query semantic memory)
mcp__brain-core__query_chroma(collection="rig_charlie", query="...", n_results=5)
mcp__brain-core__query_chroma(collection="rig_charlie", query="...", category="error")
mcp__brain-core__memory_search(query="...")  # Cross-tier search

# Learn (store to semantic memory)
mcp__brain-core__upsert_chroma(collection="rig_charlie", category="pattern", content="...")
mcp__brain-core__upsert_chroma(collection="rig_charlie", category="error", content="...")

# Working memory (shared blocks, auto-injected on session start)
mcp__brain-core__memory_store(content="...", block="pending_items")
mcp__brain-core__memory_store(content="...", block="project_context")

# Collections
mcp__brain-core__list_collections()
mcp__brain-core__create_collection(name="rig_newproject")
```

**Compaction survival:** Brain-core memories survive compaction automatically.
Bead notes survive via `bd show`. Between the two, agents can resume from any point.

### Convoys (Batch Tracking)
```bash
gt convoy create "Release v2" <b1> <b2>  # Track multiple beads
gt convoy add <convoy-id> <bead>         # Add more work
gt convoy status <id>                    # Progress view
gt convoy list                           # Dashboard
gt convoy close <id>                     # Close (verifies all done)
```

### Services & Diagnostics
```bash
# Services
gt up                        # Start all services
gt down                      # Stop all services
gt dolt status               # Dolt SQL server health
gt dolt start / stop         # Manage Dolt
gt dolt cleanup              # Remove orphan test databases
gt daemon start / stop       # Background daemon
gt vitals                    # Unified health dashboard
gt doctor                    # Run health checks

# Rigs
gt rig list                  # List rigs
gt rig add <name> <git-url>  # Add new rig
gt rig add <name> --adopt    # Register existing directory
gt rig boot <name>           # Start witness + refinery (NOT polecats)
gt polecat list <rig>        # List polecats in rig
gt polecat status <rig>/<name>  # Polecat details
gt polecat pool-init <rig>   # Initialize persistent polecat pool

# Diagnostics
gt costs                     # Claude session costs
gt trail                     # Recent agent activity
gt feed                      # Real-time activity feed
gt metrics                   # Command usage stats
gt agents                    # List active agent sessions
gt audit <actor>             # Work history by actor
gt seance                    # Talk to predecessor sessions
```

## Workflow Patterns

### Dispatch N Tasks in Parallel
```bash
# IMPORTANT: cd into the rig directory first so beads get the rig prefix
cd /path/to/rig
ID1=$(bd create "Task 1" -d "..." --silent)
ID2=$(bd create "Task 2" -d "..." --silent)
ID3=$(bd create "Task 3" -d "..." --silent)
gt sling $ID1 $ID2 $ID3 <rig> --create
# Or track with convoy:
gt convoy create "Sprint work" $ID1 $ID2 $ID3
```

### Dispatch to Existing Polecat (Full Sequence)
```bash
# 1. Ensure polecat pool exists
gt polecat pool-init <rig> --size 1

# 2. Create bead FROM THE RIG DIRECTORY (gets rig prefix)
cd /path/to/rig
ID=$(bd create "Task title" -d "description" -t task -p 1 --silent)

# 3. Sling to rig (auto-spawn, preferred):
gt sling $ID <rig> -m "instructions" --no-boot

# 4. Or to specific polecat (requires tmux session):
tmux new-session -d -s <rig-prefix>-<polecat> -c /path/to/polecats/<polecat>/<rig>
gt sling $ID <rig>/<polecat> -m "instructions" --no-boot
```

### Self-Assign with Brain-Core Recall
```bash
# 1. Create task
ID=$(bd create "Fix clippy warnings" -d "..." --silent)

# 2. Recall what brain-core knows about this area
# mcp__brain-core__query_chroma(collection="rig_charlie", query="clippy warnings common fixes")

# 3. Execute with context
gt sling $ID --no-convoy --hook-raw-bead -a "instructions"

# 4. After completion, learn what you found
# mcp__brain-core__upsert_chroma(collection="rig_charlie", category="pattern", content="...")
```

### New Rig Setup with Brain-Core Seeding
```bash
# 1. Create rig
gt rig add myproject https://github.com/org/repo.git

# 2. Create rig collection in brain-core
# mcp__brain-core__create_collection(name="rig_myproject")

# 3. Seed with initial architecture knowledge
# mcp__brain-core__upsert_chroma(collection="rig_myproject", category="architecture",
#   content="<codebase overview, key files, build commands, test patterns>")

# 4. Seed with known errors/gotchas
# mcp__brain-core__upsert_chroma(collection="rig_myproject", category="error",
#   content="<known issues, common failures, workarounds>")

# 5. Boot rig infrastructure
gt rig boot myproject
gt polecat pool-init myproject --size 2

# Now any polecat dispatched to this rig can RECALL from rig_myproject
```

### Use OpenClaw Models for Analysis
```bash
# Agent calls OpenClaw for NLP/analysis via the gateway
curl -s http://localhost:18789/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"nvidia-nim/moonshotai/kimi-k2-instruct-0905",
       "messages":[{"role":"user","content":"Analyze this job posting..."}]}'
```

### Ephemeral Patrol with Squash
```bash
bd mol wisp mol-patrol
# Execute patrol work...
bd mol squash wisp-abc123 --summary "Patrol complete: 3 issues found"
```

### Capture Tribal Knowledge
```bash
# After completing a good ad-hoc workflow:
# Option A: Gas Town formula (reusable workflow template)
bd mol distill bd-release-epic --as "Release Process" --var version=X.Y.Z

# Option B: Brain-core pattern (institutional memory, survives everything)
# mcp__brain-core__upsert_chroma(collection="rig_charlie", category="pattern",
#   content="Release process: 1) bump version 2) update changelog 3) cargo test 4) tag 5) deploy")

# Use both: formulas for structured multi-step workflows, brain-core for knowledge
```

### Dolt Safety Protocol
Before restarting Dolt, ALWAYS collect diagnostics first:
```bash
gt dolt status 2>&1 | tee /tmp/dolt-hang-$(date +%s).log
gt escalate -s HIGH "Dolt: <symptom>"
# THEN restart if needed
```

## Dolt / Bead DB — Non-Essential Infrastructure

**The bead DB (Dolt) is NOT essential.** It can be nuked and recreated at any time.

The real persistence layers (in order of importance):
1. **The user** — ask them before spending inference on recovery
2. **brain-core memories** — institutional knowledge survives everything
3. **codegraph** — SQLite + ChromaDB, auto-rebuilt from source
4. **Git history** — the actual code and commits

### Recovery rules
- If Dolt is corrupted: `bd init --force` (clean slate). Do NOT attempt root hash repair.
- If bead history is needed from prior environments: it's in `.rar` backups, ask the user.
- **2-attempt rule**: If infrastructure recovery fails twice, STOP and ASK the user. Never spend >2 attempts recovering non-essential state.
- Fresh instances (new GCP, new machine) should start with clean rig state — do NOT import old Dolt data.

### What Dolt stores (all replaceable)
- Bead metadata (issues, deps, comments) — recreatable from brain-core + user memory
- Agent state tracking — ephemeral by nature
- Mail history — use nudge instead (no persistence needed)

## Anti-Patterns

- Do NOT use `bd edit` (opens $EDITOR, blocks agents)
- Do NOT use `rm -rf` on `.dolt-data/` (use `gt dolt cleanup` or `bd init --force`)
- Do NOT use mail for routine comms (use nudge)
- Do NOT spend more than 2 attempts recovering Dolt — ask the user instead
- Do NOT import old bead history to new environments — start clean
- Do NOT use priority strings ("high") — use numbers (0-4)
- Do NOT run OpenClaw models directly on Windows — use WSL gateway
- Do NOT hardcode model provider internal IDs — use `nvidia-nim/` prefix
- Do NOT assume services are running — verify with health checks first
- Do NOT narrate what you'll do — execute it
- Do NOT give the user commands to run when you can run them yourself
- Do NOT create beads at town root (hq-*) when dispatching to rig polecats — `cd` into the rig first
- Do NOT sling to a specific polecat (`rig/name`) unless its tmux session exists — sling to the rig instead for auto-spawn
- Do NOT expect `gt rig boot` to spawn polecats — it only starts witness + refinery
- Do NOT mix `bd show` across prefix boundaries — query from the directory matching the bead's prefix

## Exec-First Rules for Agents

1. **Run it, don't say it.** If you know what command to run, run it. Never dump
   example commands as a response.
2. **Verify before assuming.** `which <binary>`, `curl <endpoint>/healthz`,
   `systemctl --user status <svc>` — check state before acting on assumptions.
3. **Scope containment.** If a task takes >5 minutes without progress, stop and
   escalate. Do not over-engineer scaffolding for one-time lookups.
4. **Read error messages literally.** "already exists" != "fresh install".
   "No such file" != "wrong password". Quote the actual error in reasoning.

## References

| Topic | File |
|-------|------|
| Available formulas | [references/formulas.md](references/formulas.md) |
| OpenClaw model details | [references/openclaw-models.md](references/openclaw-models.md) |
| Workspace patterns | [references/workspace-patterns.md](references/workspace-patterns.md) |
