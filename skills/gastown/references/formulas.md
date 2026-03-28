# Gas Town Formulas Reference

## Core Workflow Formulas

### shiny (Engineer in a Box)
The canonical engineering workflow. Design before code. Review before ship.
```
Steps: design -> implement -> review -> test -> submit
Variables: {{assignee}}, {{feature}} (required)
Usage: gt sling shiny <target> --var feature="auth system"
```

### shiny-enterprise
Shiny + Rule of Five expansion (breaks work into 5 subtasks per step).

### shiny-secure
Shiny + security-audit aspect applied.

### mol-polecat-work (15K lines)
Full polecat work lifecycle from assignment through completion and self-cleanup.
```
Steps: 7 steps including checkout, implement, test, review, submit
Variables: 7 vars (auto-populated by gt sling)
```

### mol-polecat-code-review
Code review work assignment formula.

### mol-polecat-lease
Polecat lease management.

### mol-polecat-review-pr
Pull request review workflow.

### mol-polecat-conflict-resolve
Conflict resolution workflow for merge conflicts.

### mol-idea-to-plan (12K lines)
Full pipeline from vague idea to approved, beads-ready implementation plan.
```
Variables: {{idea}} (required), {{assignee}}, {{rig}}
```

### mol-plan-review
Plan review and validation workflow.

### mol-prd-review
Product requirements document review.

## Convoy Formulas (Parallel Work)

### code-review (13K lines)
Parallel code review with 10 legs:
correctness, performance, security, elegance, resilience, style, smells,
wiring, commit-discipline, test-quality.

### design (9K lines)
Design exploration with 6 parallel analysts:
api, data, ux, scale, security, integration.

### mol-convoy-feed
Feed stranded convoys by dispatching ready work to available agents. 11 variables.

### mol-convoy-cleanup
Archive completed convoys and notify overseer. 6 variables.

## Infrastructure Formulas (Dogs)

### mol-dog-doctor
Probe Dolt server health and inspect resource conditions. 13 variables.

### mol-dog-reaper
Reap stale wisps and close stale issues across all Dolt databases. 8 variables.

### mol-dog-backup
Sync Dolt database backups to local and offsite storage. 8 variables.

### mol-dog-compactor
Compact Dolt commit history on production databases. 4 variables.

### mol-dog-stale-db
Detect stale and test databases accumulating in the Dolt server. 10 variables.

### mol-dog-phantom-db
Detect phantom databases in .dolt-data/ that can crash the server. 7 variables.

### mol-dog-jsonl
Export Dolt databases to JSONL and push to git archive. 12 variables.

## Patrol Formulas (Recurring Ops)

### mol-deacon-patrol
Mayor's daemon patrol loop. 1 variable.

### mol-witness-patrol (32K lines)
Per-rig worker monitor patrol loop. Oversees polecat completion, escalates stuck workers. 1 variable.

### mol-refinery-patrol (37K lines)
Merge queue processor patrol loop. Handles polecat branch merging with sequential rebasing. 13 variables.

### mol-boot-triage
Boot triage cycle — the daemon's watchdog for Deacon health.

## Coordination Formulas

### mol-gastown-boot
Mayor bootstraps Gas Town via verification-gated lifecycle molecule.
8 steps: ensure daemon → ensure deacon → parallel witnesses → parallel refineries → verify town health.

### mol-orphan-scan
Find and reassign orphaned work. 18 variables.

### mol-session-gc
Clean stale sessions and garbage collect. 12 variables.

### mol-sync-workspace (14K lines)
Workspace synchronization across rigs.

### mol-town-shutdown
Graceful town shutdown.

### mol-shutdown-dance (16K lines)
Full shutdown coordination sequence.

### mol-digest-generate
Digest generation from completed work.

### mol-dep-propagate
Dependency propagation across projects.

## Release Formulas

### gastown-release (10K lines)
Gas Town release workflow — version bump through verified release.
11 steps: preflight → docs update → version bump → git commit → tag → push → install → daemon restart.

### beads-release (6K lines)
Beads release workflow. 14 steps including CI verification and artifact verification (GitHub, npm, PyPI).

## Expansion Templates

### rule-of-five
Jeffrey Emanuel's discovery: LLM agents produce best work when tasks are broken
into ~5 subtasks. Applied as expansion to other formulas.

## Aspect Templates

### security-audit
Cross-cutting security concern. Applied as aspect to any workflow formula.

## Creating Custom Formulas

Formulas are TOML files in `.beads/formulas/`:
```toml
[formula]
name = "my-workflow"
type = "workflow"
description = "My custom workflow"

[[steps]]
name = "plan"
description = "Plan {{feature}}"

[[steps]]
name = "implement"
description = "Implement {{feature}}"
needs = ["plan"]

[[steps]]
name = "verify"
description = "Verify {{feature}}"
needs = ["implement"]
```

Search paths (in priority order):
1. `.beads/formulas/` (project)
2. `~/.beads/formulas/` (user)
3. `$GT_ROOT/.beads/formulas/` (orchestrator)

## Formula Inventory (44 total)

This workspace has 44 installed formulas. Full listing:
```
beads-release              gastown-release            mol-gastown-boot
mol-deacon-patrol          mol-witness-patrol         mol-refinery-patrol
mol-boot-triage            mol-polecat-work           mol-polecat-code-review
mol-polecat-lease          mol-polecat-review-pr      mol-polecat-conflict-resolve
mol-idea-to-plan           mol-plan-review            mol-prd-review
mol-convoy-feed            mol-convoy-cleanup         mol-orphan-scan
mol-session-gc             mol-sync-workspace         mol-town-shutdown
mol-shutdown-dance         mol-digest-generate        mol-dep-propagate
mol-dog-backup             mol-dog-compactor          mol-dog-doctor
mol-dog-jsonl              mol-dog-phantom-db         mol-dog-reaper
mol-dog-stale-db           code-review                design
rule-of-five               security-audit             shiny
shiny-enterprise           shiny-secure               towers-of-hanoi (test)
towers-of-hanoi-7 (test)   towers-of-hanoi-9 (test)   towers-of-hanoi-10 (test)
```

Use `gt formula list` for the live installed list.
Use `gt formula show <name>` for step details and variables.
