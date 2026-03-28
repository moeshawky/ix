---
name: ralph-loop
description: Iterative AI agent execution pattern for autonomous task completion. Use when running AI agents overnight, implementing self-correcting loops, defining completion criteria, setting up stop hooks, running parallel agent instances, or building autonomous development pipelines. Also known as Ralph Wiggum technique. Covers iteration limits, state persistence, fresh context, progress tracking, and sandboxed execution.
---

# The Ralph Loop Methodology

Iterative, self-correcting AI agent execution for autonomous task completion.

---

## Part 1: What is the Ralph Loop?

An iterative AI development pattern where an agent runs in a **continuous loop until objective completion criteria are met**.

### Origin
Named after Ralph Wiggum from The Simpsons—persistent despite repeated failures. Popularized by Geoffrey Huntley.

### Core Principle
**Don't let the AI decide when it's done. Let objective criteria decide.**

```
Traditional: Human → AI → "I'm done" → Human reviews → Fix → Repeat
Ralph Loop:  Human → AI Loop → Criteria check → Not met → Try again → Met → Done
```

---

## Part 2: When to Use

### ✅ Ideal Use Cases
| Scenario | Why |
|----------|-----|
| Bug fixing until tests pass | Verifiable criteria |
| Linting until clean | Automated verification |
| Feature with specs/tests | Defined acceptance |
| Greenfield projects | Fresh context beneficial |
| Overnight/weekend tasks | Autonomous execution |
| Backlog grinding | Parallel instances |

### ❌ Poor Use Cases
| Scenario | Why |
|----------|-----|
| Ambiguous requirements | No clear done criteria |
| Design/architecture | Needs human judgment |
| Security-sensitive | Needs human review |
| Creative work | No objective measure |

### Decision Rule
If you can write a boolean check that returns TRUE when done, the Ralph Loop can work.

---

## Part 3: Core Mechanism

```pseudocode
MAX_ITERATIONS = 10
iteration = 0

while iteration < MAX_ITERATIONS:
    result = run_agent(prompt, context)
    
    if completion_criteria_met(result):
        return SUCCESS
    
    context = update_context(result)
    iteration += 1

return TIMEOUT
```

### Five Key Components
1. **Task Prompt**: Original instruction
2. **Completion Criteria**: Objective success check
3. **Stop Hook**: Intercepts exit attempts
4. **Context Updater**: Feeds new state
5. **Iteration Limiter**: Prevents infinite loops

---

## Part 4: Completion Criteria

**The most critical part. Bad criteria = bad results.**

### Criteria Types
| Type | Example | Check |
|------|---------|-------|
| Test-Based | All tests pass | `cargo test` exit code |
| Lint-Based | No errors | `eslint` exit code |
| Build-Based | Compiles | `cargo build` exit code |
| Output-Based | Tag in output | String search |
| File-Based | Files exist | Path check |
| Metric-Based | Threshold met | Benchmark |

### Good vs Bad Criteria

| ❌ Bad | ✅ Good |
|--------|---------|
| "Code should work well" | "All 47 tests in test_auth.py pass" |
| "Code should be clean" | "Zero clippy warnings" |
| "Performance should be good" | "Response time < 200ms" |

### Criteria Template
```
DONE when:
- All tests pass
- Build succeeds
- No new warnings

NOT DONE when:
- Any test fails
- Build errors
- New warnings introduced
```

---

## Part 5: Stop Hook

Intercepts exit attempts, re-injects task if criteria not met.

```
Agent → Stop Hook → Criteria Check
                         ↓
           ┌─────────────┴─────────────┐
      [Met]                      [Not Met]
           ↓                           ↓
      Allow Exit              Re-inject Prompt
```

### Implementation Pattern
```bash
#!/bin/bash
while [ $i -lt $MAX ]; do
    claude --prompt "$PROMPT" --context "$CONTEXT"
    
    if run_tests; then
        exit 0  # Success
    fi
    
    CONTEXT=$(gather_context)
    i=$((i + 1))
done
exit 1  # Timeout
```

### File-Based Check
```python
def criteria_met():
    return (
        Path("test_results.json").exists() and
        json.load(open("test_results.json"))["passed"] == True
    )
```

---

## Part 6: Context Management

### Fresh Context Advantage
Each iteration starts clean, avoiding:
- Accumulated noise
- Compounding errors
- Context overflow
- Conflicting statements

### State Persistence
Since each iteration is fresh, persist state externally:

| Mechanism | Purpose |
|-----------|---------|
| Git History | Code changes |
| progress.txt | Current status |
| prd.json | Requirements |
| errors.log | Failed attempts |

### Progress File Template
```markdown
## Task
[Task description]

## Criteria
- [x] Tests pass
- [ ] Lints clean

## Status
ITERATION: 3/10
RESULT: Partial - 2/3 tests pass

## History
- Iter 1: Build failed
- Iter 2: 1 test passes
- Iter 3: 2 tests pass
```

### Context Injection
```markdown
# Iteration N Context

## Original Task
[from prd.json]

## Current State
[git status, test results]

## Previous Error
[error message]

## Focus
[specific next step]
```

---

## Part 7: Safety & Limits

### Why Limits Matter
- Prevent infinite loops
- Control API costs
- Enable human checkpoints
- Avoid cascading failures

### Recommended Limits
| Task | Limit |
|------|-------|
| Simple bug fix | 3-5 |
| Feature | 7-10 |
| Complex refactor | 10-15 |
| Greenfield | 15-20 |

### Escalation on Timeout
1. Save progress.txt
2. Document blockers
3. Notify human
4. Preserve context
5. Suggest next steps

### Emergency Stop
```bash
# Create kill file
touch .STOP_RALPH_LOOP

# In loop script
if [ -f .STOP_RALPH_LOOP ]; then
    rm .STOP_RALPH_LOOP
    exit 2
fi
```

---

## Part 8: Sandboxing

Ralph Loops often run with elevated permissions. Sandbox to prevent damage.

### Strategy 1: Docker
```bash
docker run --rm \
    --memory="4g" --cpus="2" \
    --network=none \
    -v $(pwd):/workspace \
    loop-container
```

### Strategy 2: Branch Isolation
```bash
git checkout -b ralph-loop-attempt
./run_loop.sh

# Success → merge
git checkout main && git merge ralph-loop-attempt

# Failure → discard
git checkout main && git branch -D ralph-loop-attempt
```

### Strategy 3: Temp Directory
```bash
WORK=$(mktemp -d)
git clone . "$WORK" && cd "$WORK"
./run_loop.sh
# Success → copy results back
```

---

## Part 9: Parallel Execution

For independent tasks, run parallel loops:

```
┌────────────┐  ┌────────────┐  ┌────────────┐
│ Loop: A    │  │ Loop: B    │  │ Loop: C    │
│ (branch 1) │  │ (branch 2) │  │ (branch 3) │
└─────┬──────┘  └─────┬──────┘  └─────┬──────┘
      └──────────────┬┴───────────────┘
                     ↓
              Main (merge)
```

### Parallel Script
```bash
TASKS=("fix-auth" "add-logging" "update-deps")

for task in "${TASKS[@]}"; do
    git worktree add "../wt-$task" -b "ralph/$task"
    (cd "../wt-$task" && ./run_loop.sh "$task") &
done
wait
```

### Post-Parallel
1. Check for conflicts
2. Merge sequentially
3. Re-run tests
4. Clean up worktrees

---

## Part 10: Best Practices

### Before Starting
- [ ] Clear boolean criteria defined?
- [ ] Automated tests exist?
- [ ] Sandbox prepared?
- [ ] Iteration limit set?
- [ ] Kill switch ready?
- [ ] State persistence planned?

### During Execution
- [ ] Watch first 2-3 iterations
- [ ] Monitor resources (CPU/RAM)
- [ ] Verify context freshness
- [ ] Log all outputs

### After Completion
- [ ] Re-run tests manually
- [ ] Human code review
- [ ] Check for side effects
- [ ] Document learnings
- [ ] Clean up temp files

---

## Part 11: Troubleshooting

### Never Succeeds
**Cause**: Criteria too strict, task too hard, flaky tests
**Fix**: Relax criteria, break into subtasks, fix flaky tests

### Passes but Wrong
**Cause**: Criteria don't capture requirements, AI gamed it
**Fix**: Add integration tests, semantic checks

### Stuck on Same Error
**Cause**: Context not updating, blocker exists
**Fix**: Include error history in context, manually fix blocker

### Resource Exhaustion
**Cause**: No limits, no cleanup
**Fix**: Set strict limits, clean artifacts each iteration

---

## Part 12: Advanced Patterns

### Progressive Criteria
Start easy, tighten over iterations:
```python
def get_criteria(i):
    if i < 3: return ["builds"]
    if i < 6: return ["builds", "unit_tests"]
    return ["builds", "unit_tests", "integration"]
```

### Checkpoint Merging
Merge partial progress:
```bash
if [ $i -eq 5 ]; then
    git commit -am "Checkpoint iter 5"
fi
```

### Human-in-the-Loop
Pause for review:
```python
if iteration == MAX // 2:
    notify_human("Review?")
    wait_for_approval()
```

### Cascading Loops
```bash
./loop.sh --criteria "tests_pass"
./loop.sh --criteria "lints_clean"
./loop.sh --criteria "perf_ok"
```

---

## Part 13: Templates

### prd.json
```json
{
  "task_id": "TASK-123",
  "title": "Fix auth bug",
  "criteria": ["Tests pass", "No 401 on valid JWT"],
  "success_cmd": "cargo test auth::",
  "max_iterations": 7
}
```

### progress.txt
```
TASK: TASK-123
ITERATION: 4/7
CRITERIA: [x] Build [x] Auth tests [ ] Rate limit
ERROR: test rate_limit::throttle FAILED
```

### run_loop.sh
```bash
#!/bin/bash
MAX=${1:-10}; i=0

while [ $i -lt $MAX ]; do
    i=$((i + 1))
    [ -f .STOP ] && exit 2
    
    claude --prompt "$(cat prd.json)" --context "$(cat progress.txt)"
    
    if ./check.sh; then exit 0; fi
    ./update_context.sh >> progress.txt
done
exit 1
```

---

## Part 14: Operating Rules (Hard Constraints)

1. **Always set iteration limits.** No infinite loops.
2. **Always sandbox.** Branch, container, or temp dir.
3. **Always persist state externally.** Fresh context each run.
4. **Always use objective criteria.** Boolean or don't use Ralph.
5. **Always have kill switch.** Emergency stop accessible.
6. **Always review results.** Human verification before merge.
7. **Never run on production directly.** Isolate first.

---

## Quick Start

```bash
# 1. Task definition
echo '{"task":"Fix tests","criteria":"cargo test"}' > prd.json

# 2. Init progress
echo "STATUS: STARTING" > progress.txt

# 3. Run
./run_loop.sh 5

# 4. Verify
cat progress.txt
```

---

**Skill Status**: COMPREHENSIVE ✅
**Coverage**: Theory, mechanism, safety, patterns, templates ✅
