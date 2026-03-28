---
name: skill-creator
description: Guide for creating effective skills. This skill should be used when users want to create a new skill (or update an existing skill) that extends Claude's capabilities with specialized knowledge, workflows, or tool integrations. Also triggers on "build a skill", "moeify", "skill architecture", "skill design", "create a skill", "add a skill", "skill triggers", "skill-rules.json", "hook activation", "debugging skill activation".
license: Complete terms in LICENSE.txt
---

# Skill Creator

*A skill is not a procedure. It is a thinking tool — a cognitive lens that changes how you approach a class of problems.*

## The Distinction

Generic documentation tells you WHAT to do. A skill changes HOW you think. The difference:

| Documentation | Skill |
|---|---|
| "Step 1: Check files" | A **principle** named for memorability that applies everywhere |
| "Avoid common mistakes" | An **anti-pattern table** with signals, diagnoses, and prescribed responses |
| "See also: other docs" | A **companion skill network** that declares what to load and when |
| Flat instructions | **Verification gates** — checkpoints that must pass before proceeding |
| Technical prose | A **voice** — a cognitive stance stated upfront that frames everything |

## Skill Anatomy

```
skill-name/
├── SKILL.md (required)
│   ├── YAML frontmatter: name + description (triggers)
│   └── Markdown body: voice, principles, process, anti-patterns, gates, companions
└── Bundled Resources (optional)
    ├── scripts/     — Executable code (deterministic, token-efficient)
    ├── references/  — Deep knowledge (the PAYLOAD, not supplementary)
    └── assets/      — Output files (templates, images, boilerplate)
```

### Progressive Disclosure

1. **Metadata** (~100 words) — always in context. `name` + `description` determine triggering.
2. **SKILL.md body** (<500 lines) — loaded when skill triggers. Principles, process, pointers.
3. **references/** (unlimited) — loaded on demand. This is where depth lives. references/ can be 5-10x the size of SKILL.md. The SKILL.md is the map; references/ is the territory.

## The Eight Properties of an Effective Skill

### 1. Principles, Not Procedures

A skill encodes a decision framework, not a checklist. Principles survive context changes; procedures break when context shifts.

- Name each principle. A vivid name outlives "Step 3: If it fails, try again."
- Principles should be few (3-7) and memorable.
- Each principle should be independently applicable — usable even if the rest of the skill isn't loaded.

### 2. Naming as Mnemonic

Name the skill and its concepts for memorability. Cultural, metaphorical, or narrative naming gives concepts weight and stickiness.

- The skill name should evoke its cognitive stance, not describe its file operations.
- Prefer gerund form (verb + -ing) for action-oriented skills.
- A good name is one the user will TYPE from memory, not look up.

### 3. Anti-Pattern Tables

Every skill must define what NOT to do with equal precision as what TO do. For each anti-pattern:

| Column | Purpose |
|--------|---------|
| **Signal** | How to detect this is happening |
| **Diagnosis** | Why it happens (root cause, not symptom) |
| **Response** | What to do instead (prescriptive, not advisory) |

Anti-patterns are not "common mistakes." They are **predictable failure modes**.

### 4. Companion Skill Network

Skills exist in families. No skill should claim to be complete alone.

- Declare which skills complement this one and how they divide the problem space.
- Specify WHEN to load companions (e.g., "before implementation, load X for screening").

### 5. Cognitive Tool Integration

If the environment has reasoning tools (structured thinking frameworks, scratchpads, memory systems, search tools), the skill should specify WHEN and HOW to use them.

A skill that ignores available cognitive infrastructure is underutilizing the agent.

### 6. Verification Gates

Build checkpoints INTO the skill at each major decision point:

| Gate element | Purpose |
|---|---|
| **Condition** | What must be true to proceed |
| **Check** | What command/action proves it |
| **Failure response** | What to do if it doesn't pass |

Gates are woven into the process — you cannot reach phase N+1 without passing phase N's gate.

### 7. References as Payload

`references/` is not supplementary documentation. It is the deep knowledge the skill draws from.

- SKILL.md tells you WHEN to look. references/ tells you WHAT you'll find.
- Budget accordingly: if SKILL.md is 400 lines, references/ can be 2,000-4,000 lines across multiple files.
- Structure references by domain, not by format.
- For files >100 lines, include a table of contents at top.

### 8. Voice

A skill has a voice — a cognitive stance that frames how to approach problems. State it upfront, before any instructions.

- 1-2 sentences at the top in italics.
- It sets the TONE for everything that follows.

## Creation Process

### Step 1: Understand with Examples

Identify 3-5 concrete scenarios where the skill would activate. For each:
- What triggers it?
- What does the agent do differently WITH the skill vs without?
- What's the failure mode if the skill ISN'T loaded?

### Step 2: Extract Principles

From the examples, extract the RECURRING DECISIONS. These become principles.
- "always do X" → principle
- "never do Y" → anti-pattern
- Two examples share a verification step → gate

### Step 3: Initialize

```bash
scripts/init_skill.py <skill-name> --path <output-directory>
```

Or create manually: `mkdir -p skill-name/{references,scripts}` + create `SKILL.md`.

Delete any generated example files you don't need.

### Step 4: Write the Skill

**SKILL.md body** (target: 300-500 lines):
1. Voice (1-2 sentences, italics)
2. Core principles (3-7, named)
3. Process (phases with gates)
4. Anti-pattern table
5. Companion skill references
6. Pointers to references/ files

**references/** (as needed):
- Deep knowledge the skill draws from
- One file per domain/concern
- Table of contents for files >100 lines

**Writing style**: Imperative form. No "you should." Direct: "Check X. If Y, do Z."

**Frontmatter description**: Third-person with specific trigger phrases:
```yaml
description: This skill should be used when the user asks to "specific phrase 1", "specific phrase 2", or when situation X is detected. Covers domain Y with emphasis on Z.
```

### Step 5: Package

```bash
scripts/package_skill.py <path/to/skill-folder>
```

Validates structure, frontmatter, description quality. Creates `.skill` distribution file.

## Activation & Wiring

Designing the skill's content is half the work. The other half is how it activates and integrates with Claude Code's hook system.

### Enforcement Level (design decision)

- **Block** — skill MUST be acknowledged before proceeding. Exit code 2 from hook. For skills where ignoring guidance causes data loss or security issues.
- **Suggest** — advisory context injection via stdout. Most skills live here.
- **Warn** — minimal nudge. If guidance isn't worth suggesting, question whether it's a skill.

### Hook Architecture

Two hooks control activation:

**UserPromptSubmit** (proactive): Fires BEFORE Claude sees the prompt. Matches keywords/intent patterns from `skill-rules.json`, injects skill reminder as context.

**Stop** (reactive): Fires AFTER Claude responds. Analyzes output for patterns that should have triggered a skill. Gentle reminders, not blocking.

### Trigger Design

Include ALL activation keywords in the frontmatter description (max 1024 chars). Claude's trigger matching runs against this field.

For programmatic activation via `skill-rules.json`:

```json
{
  "my-skill": {
    "type": "domain",
    "enforcement": "suggest",
    "priority": "medium",
    "promptTriggers": {
      "keywords": ["keyword1", "keyword2"],
      "intentPatterns": ["(create|add).*?something"]
    }
  }
}
```

### Skip Conditions

- **Session tracking**: Don't nag repeatedly. State file per session.
- **File markers**: `// @skip-validation` for permanently verified files.
- **Env vars**: `SKIP_SKILL_GUARDRAILS=true` for emergency disable.

### Testing

```bash
# Test UserPromptSubmit trigger
echo '{"session_id":"test","prompt":"your test prompt"}' | \
  npx tsx .claude/hooks/skill-activation-prompt.ts

# Test PreToolUse guard
cat <<'EOF' | npx tsx .claude/hooks/skill-verification-guard.ts
{"session_id":"test","tool_name":"Edit","tool_input":{"file_path":"test.ts"}}
EOF
```

## Validation Checklist

### Cognitive Architecture
- [ ] Does it have a voice? (Not "This skill provides guidance")
- [ ] Are principles NAMED and independently memorable?
- [ ] Does the anti-pattern table have signal/diagnosis/response columns?
- [ ] Are gates woven into the process (not appended)?
- [ ] Are companion skills declared?
- [ ] Do references/ contain deep knowledge (not just "additional docs")?
- [ ] Does loading this skill change HOW the agent thinks, not just WHAT it does?

### File Structure
- [ ] SKILL.md exists with valid YAML frontmatter
- [ ] Frontmatter has `name` and `description` with specific trigger phrases
- [ ] Body under 500 lines, imperative form
- [ ] Referenced files exist
- [ ] No extraneous docs (README, CHANGELOG, etc.)

### Activation
- [ ] Keywords tested with real prompts (3+ scenarios)
- [ ] No false positives, no false negatives
- [ ] Enforcement level matches criticality
- [ ] Skip conditions configured

## What NOT to Create

- README.md, CHANGELOG.md, INSTALLATION_GUIDE.md — a skill is not a software project
- "When to use this skill" in the body — that belongs ONLY in frontmatter description
- Verbose explanations of things Claude already knows — challenge each paragraph: "Does this justify its token cost?"
- Skills that are just reformatted documentation — if loading it doesn't change the agent's cognitive approach, it's not a skill

## Reference Files

Three atomic references, one concern each:
- **`references/activation.md`** — Triggers, skill-rules.json schema, pattern library, testing commands
- **`references/hooks.md`** — Hook architecture, exit codes, session state, troubleshooting
- **`references/content-patterns.md`** — Workflow patterns, output templates, input/output examples
