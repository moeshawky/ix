# Hooks — Mechanics and Troubleshooting

One concern: how Claude Code hooks execute and how to debug them.

## Table of Contents
- [Hook Architecture](#hook-architecture)
- [UserPromptSubmit](#userpromptsubmit)
- [PreToolUse](#pretooluse)
- [Stop Hook](#stop-hook)
- [Exit Code Behavior](#exit-code-behavior)
- [Session State](#session-state)
- [Troubleshooting](#troubleshooting)

---

## Hook Architecture

```
User types prompt
  → UserPromptSubmit hook fires (proactive suggestions)
    → stdout injected as context for Claude
  → Claude processes prompt + injected context
    → Claude calls tools
      → PreToolUse hook fires (guardrail check)
        → exit 0: allow
        → exit 2: BLOCK (stderr shown to Claude)
  → Claude finishes response
    → Stop hook fires (gentle post-response reminders)
```

**Files:**
- `.claude/hooks/skill-activation-prompt.ts` — UserPromptSubmit
- `.claude/hooks/skill-verification-guard.ts` — PreToolUse
- `.claude/hooks/error-handling-reminder.ts` — Stop

**Config:** `.claude/settings.json` registers hooks.

---

## UserPromptSubmit

**When:** Before Claude sees the user's prompt.
**Input:** `{"session_id": "...", "prompt": "user's message"}`
**Output:** stdout → injected as context. stderr → ignored.
**Exit code:** Always 0 (non-blocking).

**What it does:**
1. Reads `skill-rules.json`
2. Matches prompt against keywords + intent patterns
3. Outputs formatted skill reminders to stdout
4. Claude sees reminders as additional context

**Performance target:** <100ms.

---

## PreToolUse

**When:** Before a tool (Edit, Write, etc.) executes.
**Input:** `{"session_id": "...", "tool_name": "Edit", "tool_input": {"file_path": "..."}}`
**Output:** stderr → shown to Claude as error message.
**Exit code:**
- `0` = allow tool execution
- `2` = BLOCK tool execution (Claude sees stderr)

**What it does:**
1. Checks if file matches path/content patterns in `skill-rules.json`
2. Checks skip conditions (session state, file markers, env vars)
3. If blocked: stderr explains what skill to use first

**Performance target:** <200ms.

---

## Stop Hook

**When:** After Claude finishes responding.
**Purpose:** Gentle reminders, not blocking.
**Example:** Check if Claude edited code without error handling patterns.

---

## Exit Code Behavior

| Exit Code | Hook Type | Effect |
|-----------|-----------|--------|
| 0 | Any | Normal — proceed |
| 2 | PreToolUse | **BLOCK** tool execution, show stderr to Claude |
| 2 | UserPromptSubmit | No effect (non-blocking hook) |
| 1 | Any | Hook error — logged, tool proceeds |

**Critical:** Only exit code 2 blocks. Exit code 1 is treated as hook failure, not a block.

---

## Session State

**Location:** `.claude/hooks/state/skills-used-{session_id}.json`

**Purpose:** Track which skills have been acknowledged this session so guardrails don't nag repeatedly.

**Flow:**
1. First edit triggers block → user acknowledges skill
2. Hook records skill as "used" in session state
3. Subsequent edits in same session → skip block
4. New session → state resets

---

## Troubleshooting

### Skill not triggering (UserPromptSubmit)

| Symptom | Cause | Fix |
|---------|-------|-----|
| No skill suggestion appears | Keywords don't match | Test: `echo '{"prompt":"..."}' \| npx tsx .claude/hooks/skill-activation-prompt.ts` |
| Wrong skill triggers | Overly broad pattern | Narrow intent regex, use `.*?` not `.*` |
| Skill triggers on unrelated prompts | Generic keywords | Use more specific terms |

### PreToolUse not blocking

| Symptom | Cause | Fix |
|---------|-------|-----|
| Edit proceeds without block | File doesn't match patterns | Check pathPatterns against actual file path |
| Block fires but tool still runs | Exit code isn't 2 | Verify: `echo $?` after running hook manually |
| Block fires every time | Session tracking not working | Check state file exists in `.claude/hooks/state/` |

### Hook not running at all

| Symptom | Cause | Fix |
|---------|-------|-----|
| No output from hook | Not registered in settings.json | Check `.claude/settings.json` hooks section |
| TypeScript error | Missing dependency | Run `npx tsx --version`, install if needed |
| Permission denied | File not executable | `chmod +x .claude/hooks/*.ts` |

### Performance issues

| Symptom | Cause | Fix |
|---------|-------|-----|
| Noticeable delay on every prompt | Slow regex compilation | Pre-compile patterns, cache results |
| Sluggish file content matching | Reading large files | Add path exclusions for large directories |

### Manual testing commands

```bash
# UserPromptSubmit
echo '{"session_id":"test","prompt":"add a new feature"}' | \
  npx tsx .claude/hooks/skill-activation-prompt.ts

# PreToolUse
cat <<'EOF' | npx tsx .claude/hooks/skill-verification-guard.ts
{"session_id":"test","tool_name":"Edit","tool_input":{"file_path":"src/service.ts"}}
EOF

# Validate JSON
jq . .claude/skills/skill-rules.json
```
