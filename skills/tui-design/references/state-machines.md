# Modal State Machines for TUI

Formal state machine definitions prevent the #1 TUI bug class: state leaks where the UI gets stuck in an unrecoverable mode.

## Table of Contents

1. [The Problem](#the-problem)
2. [State Machine Definition](#state-machine-definition)
3. [Transition Table Pattern](#transition-table-pattern)
4. [Error Recovery](#error-recovery)
5. [Terminal ASCII Gotchas](#terminal-ascii-gotchas)
6. [Real-World Examples](#real-world-examples)

---

## The Problem

Ad-hoc boolean state management causes:
- **Stuck states**: `is_waiting = true` never reset after cancelled async operation → input permanently locked
- **Unreachable exits**: Esc/q blocked on specific tabs with no alternative quit path
- **Mode confusion**: User doesn't know if they're in input mode, normal mode, or waiting mode
- **Ghost cancellations**: Cancel signal sent to async task but UI state not updated

**Root cause**: State transitions are scattered across match arms and message handlers without a central authority. Each `is_waiting = true` has N paths that should set it back to `false`, and any missed path creates a permanent lock.

---

## State Machine Definition

### Enum Over Booleans
```rust
// BAD: Multiple booleans with implicit interaction rules
struct TabState {
    is_inputting: bool,   // Can these both be true?
    is_waiting: bool,     // What if is_waiting && is_inputting?
    is_editing: bool,     // Three booleans = 8 possible states, most invalid
}

// GOOD: Explicit enum with only valid states
enum TabMode {
    Normal,                        // Browsing, scrolling, navigation
    Input { buffer: TextInput },   // Composing a message
    Waiting { cancel: CancelToken }, // Async operation in progress
    Error { message: String },     // Recoverable error displayed
}
```

### Transition Function
```rust
impl TabMode {
    fn transition(self, event: TabEvent) -> TabMode {
        match (self, event) {
            // Normal mode transitions
            (TabMode::Normal, TabEvent::StartInput) => TabMode::Input { buffer: TextInput::new() },
            (TabMode::Normal, TabEvent::Quit) => /* exit app */,

            // Input mode transitions
            (TabMode::Input { buffer }, TabEvent::Send) => {
                if buffer.is_empty() { return TabMode::Input { buffer }; }
                TabMode::Waiting { cancel: CancelToken::new() }
            }
            (TabMode::Input { .. }, TabEvent::Cancel) => TabMode::Normal,

            // Waiting mode transitions — EVERY path returns to Normal
            (TabMode::Waiting { .. }, TabEvent::Receive(_)) => TabMode::Normal,
            (TabMode::Waiting { .. }, TabEvent::Error(e)) => TabMode::Error { message: e },
            (TabMode::Waiting { cancel }, TabEvent::Cancel) => {
                cancel.cancel();  // Signal async task
                TabMode::Normal   // IMMEDIATELY unlock UI
            }

            // Error mode transitions
            (TabMode::Error { .. }, TabEvent::Dismiss) => TabMode::Normal,
            (TabMode::Error { .. }, TabEvent::StartInput) => TabMode::Input { buffer: TextInput::new() },

            // Invalid transitions — stay in current state
            (state, _) => state,
        }
    }
}
```

---

## Transition Table Pattern

Document every valid transition as a table. Empty cells = invalid transition (ignored).

### Chat Tab

| From \ Event | StartInput | Send | Cancel | Receive | Error | Quit | ScrollUp |
|-------------|-----------|------|--------|---------|-------|------|----------|
| **Normal** | → Input | | | | | → Exit | stay, scroll |
| **Input** | | → Waiting | → Normal | | | | |
| **Waiting** | | | → Normal | → Normal | → Error | | stay, scroll |
| **Error** | → Input | | → Normal | | | → Exit | |

### Key Properties
1. **Every state has an exit path** — no dead ends
2. **Waiting ALWAYS returns to Normal** — via Receive, Error, or Cancel
3. **Cancel is immediate** — async task gets signal, UI doesn't wait for it
4. **Scroll works in Normal and Waiting** — don't lock out read-only navigation during async

---

## Error Recovery

### The Cancel Protocol
```
User presses Esc during Waiting:
1. Set AtomicBool cancel flag (signals async task to stop)
2. IMMEDIATELY set mode = Normal (unlock UI)
3. Async task checks flag, stops, sends Error or partial result
4. If Error arrives in Normal mode, show in status bar (don't change mode)
```

**Critical**: Step 2 happens BEFORE the async task acknowledges. The UI must never wait for the task to confirm cancellation. The task might be hung, timed out, or crashed. The UI must be independently recoverable.

### Timeout Recovery
```
If Waiting state persists longer than timeout_secs:
1. Show "operation timed out" in status bar
2. Transition to Normal mode
3. Log the timeout for diagnostics
4. Do NOT retry automatically — let user decide
```

### Orphaned Async Tasks
When a task completes after the user has already cancelled:
```rust
match (current_mode, event) {
    // Response arrives but we're already back in Normal → show in status bar, don't change mode
    (TabMode::Normal, TabEvent::Receive(content)) => {
        status_bar.show("Late response received (was cancelled)");
        TabMode::Normal
    }
    // Error arrives but we're already back in Normal → show in status bar
    (TabMode::Normal, TabEvent::Error(e)) => {
        status_bar.show(format!("Background error: {}", e));
        TabMode::Normal
    }
}
```

---

## Terminal ASCII Gotchas

These are properties of the terminal, not bugs in your code:

| Key Combo | Byte Produced | crossterm Sees | Why |
|-----------|--------------|----------------|-----|
| Ctrl+I | 0x09 | KeyCode::Tab | ASCII: Ctrl + letter = letter - 0x40. I = 0x49, 0x49 - 0x40 = 0x09 = Tab |
| Ctrl+M | 0x0D | KeyCode::Enter | Same: M = 0x4D, 0x4D - 0x40 = 0x0D = CR |
| Ctrl+H | 0x08 | KeyCode::Backspace | H = 0x48, 0x48 - 0x40 = 0x08 = BS |
| Ctrl+[ | 0x1B | KeyCode::Esc | [ = 0x5B, 0x5B - 0x40 = 0x1B = ESC |
| Ctrl+J | 0x0A | KeyCode::Enter (LF) | J = 0x4A, 0x4A - 0x40 = 0x0A = LF |

**Implication**: You CANNOT distinguish Ctrl+I from Tab, Ctrl+M from Enter, or Ctrl+H from Backspace at the terminal level. Don't try. Don't bind Ctrl+I to anything different from Tab.

Modern terminals with kitty keyboard protocol (CSI u) CAN distinguish these, but support is limited to kitty, WezTerm, foot, and ghostty. If targeting broad compatibility, treat them as identical.

---

## Real-World Examples

### lazygit State Machine
```
States: Normal, Staging, Committing, Rebasing, Merging, Cherry-Picking
- Each state changes the keybinding footer
- Each state has Esc → back to Normal
- Multi-step workflows (rebase) use a sub-state machine with explicit progress
- Every popup is a focus trap that returns to the parent state on dismiss
```

### k9s State Machine
```
States: Normal, Command(:), Filter(/), Confirm(y/n), Describe, Logs, Shell
- Command mode (:) opens an input at the bottom
- Enter in command mode transitions to the matching resource view
- Esc in any state returns to Normal
- Logs state has its own sub-states: Follow, Paused, Searching
```

### Charlie TUI State Machine (current, with bugs marked)
```
States: Normal, Input, Waiting
- [i] → Input ✓
- Enter → Waiting ✓
- Receive → Normal ✓
- Error → Normal ✓
- Cancel (Esc in Waiting) → ??? ← BUG: didn't reset is_waiting
- Esc in Normal → ??? ← BUG: no quit path on Chat/Mechanic tabs
- q in Normal → ??? ← BUG: blocked by guard on Chat/Mechanic
```

**Fix applied**: Cancel now resets is_waiting. Ctrl+Q added as universal quit. Esc/q allowed in Normal mode on all tabs.
