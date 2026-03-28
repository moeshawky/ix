# LLM Patch Guardrails

Mandatory pre/post-edit verification to prevent the 5 LLM failure classes from shipping broken code. Based on DARPA AIxCC findings, TrapEval (2025), and 49 bugs found in a single Charlie session.

**Type:** Rigid. Follow exactly. No adaptation. No shortcuts.

**When to use:** Before ANY code modification, fix, patch, or refactor. Load alongside `systematic-debugging`. If you skip this, you will ship empty stubs, wrong service resolution, identity patches, and hardcoded secrets — all compiling cleanly.

---

## PRE-EDIT CHECKLIST (complete before touching code)

```
[ ] 1. READ the file you're about to edit. Every line. Not just the function.
[ ] 2. RUN codegraph impact <entity> --depth 2. State the blast radius.
[ ] 3. STATE the root cause. Not the symptom. If you can't distinguish them, stop.
[ ] 4. LIST every file you will change. If it's more than 3, justify why.
[ ] 5. CITE the existing working pattern you're modeling (file:line).
```

If blast radius is MEDIUM or HIGH → propose architectural change, not local patch.

---

## POST-EDIT CHECKLIST (complete before committing)

```
[ ] 1. cargo clippy -p <crate> -- -D warnings → zero warnings
[ ] 2. scripts/validate_no_empty_bodies.sh → PASS
[ ] 3. POSITIVE test: describe exact user action → expected outcome
[ ] 4. NEGATIVE test: attempt the bad thing → verify it's BLOCKED/REJECTED
[ ] 5. DIFF review: does this fix touch fewer lines than the number of callers?
       If yes → you're patching a symptom, not fixing root cause.
[ ] 6. PATH check: any raw relative paths (".charlie/...")? Use resolve_charlie_file().
[ ] 7. SECRET check: any hardcoded keys, IPs, or URLs? Use services.yaml.
[ ] 8. STUB check: any function body < 3 lines with no external calls? Fill it.
```

---

## THE 5 FAILURE CLASSES (quick reference)

### 1. Minimal-Patch Bias
**You are doing this if:** Your fix is 1-3 lines and the bug affects 10+ callers.
**Detection question:** "Would this fix survive if the function were called from a different context?"
**Counter:** Run codegraph impact. If callers > lines changed, you're patching a symptom.

### 2. Template Fitting
**You are doing this if:** You wrote a function signature but the body is empty, log-only, or always returns the same value.
**Detection question:** "Does this function body reference any field of its enclosing struct?"
**Counter:** Every handler must model an existing working handler. Cite which one.

### 3. Semantic Trap
**You are doing this if:** Your fix compiles and passes clippy but you haven't tested actual behavior.
**Detection question:** "If I send a known-bad input, does this code actually block it?"
**Counter:** Run the negative test. `exec_shell("rm -rf /")` must return BLOCKED, not execute.

### 4. Plausible-but-Vulnerable
**You are doing this if:** The function returns a valid result but the WRONG result.
**Detection question:** "Does find_best_service() return the service the USER selected, or just the first one?"
**Counter:** Trace the data flow from user action → function call → result. Verify it matches user intent.

### 5. Stylistic Fingerprints
**You are doing this if:** You're Claude and you just added "You are NOT X" to a prompt, or created stubs planning to fill them later, or made a minimal patch that compiles but doesn't fix the architecture.
**Detection question:** "Am I falling into my known pattern?" (Check the fingerprint table)
**Counter:** Cross-validate. Would a different model catch this? Read your own diff as a reviewer.

---

## KNOWN MODEL FINGERPRINTS

| Model | Fingerprint | What to watch for |
|-------|-------------|-------------------|
| Claude Opus | Identity patches, empty stubs, over-explains | "You are NOT X" in prompts, { /* ... */ } bodies |
| kimi-k2 | Rewrites instead of copying | Output file differs entirely from source |
| llama-405b | Parallel tool calls | 400 Bad Request from NIM |
| deepseek-v3.2 | Unresponsive under load | Timeout, no output |

---

## PATH RESOLUTION BUG PATTERN

The #1 root cause found in Charlie (BUG-005): save uses raw relative path, load uses walk-up resolution. Changes save to wrong file, reload reads canonical file. Changes vanish.

**Every time you see a file path in code, ask:**
- Does the WRITE path match the READ path?
- Does `save_services()` write to the same location `load_services_config()` reads from?
- If using ".charlie/..." → use `resolve_charlie_file()` instead.

---

## COMPOUND FAILURE CASCADE

If you skip this checklist, here's what happens:
1. Template Fitting → empty stub
2. Minimal-Patch Bias → stub compiles
3. Semantic Trap → passes clippy
4. Plausible-but-Vulnerable → TUI launches
5. Stylistic Persistence → stubs survive review
6. User finds 49 bugs. All compiling cleanly.

This is not theoretical. It happened on 2026-03-19. Don't repeat it.
