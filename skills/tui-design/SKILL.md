---
name: tui-design
description: "Design system for building exceptional terminal user interfaces. Covers layout paradigms, keyboard navigation, color systems, data visualization, chat interfaces, session management, scroll viewports, and modal state machines. Use when designing TUI layouts, creating terminal dashboards, implementing keyboard navigation, building chat/conversational TUIs, adding session save/restore, fixing scroll behavior, debugging stuck UI states, choosing TUI color schemes, or working with Ratatui/Ink/Textual/Bubbletea. Activates on: TUI design, terminal UI, ratatui layout, keybinding design, panel layout, chat TUI, session persistence, scroll viewport, state machine, focus management, terminal accessibility, modal dialog."
---

# TUI Design System

Universal design patterns for building exceptional terminal user interfaces. Framework-agnostic core with Ratatui-specific reference.

**Core philosophy:** TUIs earn their power through spatial consistency, keyboard fluency, and information density that respects human attention.

## Design Process

```
What are you building?
  → Select layout paradigm (§1)
  → Design interaction model (§3)
  → Define visual system (§4)
  → Add chat/session patterns if conversational (§10-11)
  → Validate against anti-patterns (§8)
```

---

## 1. Layout Paradigm Selector

| App Type | Paradigm | Examples |
|----------|----------|----------|
| File manager | Miller Columns | yazi, ranger |
| Git / DevOps | Persistent Multi-Panel | lazygit, lazydocker |
| System monitor | Widget Dashboard | btop, bottom |
| Data browser | Drill-Down Stack | k9s, diskonaut |
| SQL / HTTP client | IDE Three-Panel | harlequin, posting |
| Shell augmentation | Overlay / Popup | atuin, fzf |
| Log / event viewer | Header + Scrollable List | htop, tig |
| **Chat / AI agent** | **Message Stream + Input Bar** | Claude Code, Codex, aichat |

See [references/app-patterns.md](references/app-patterns.md) for detailed analysis of each paradigm.

---

## 2. Responsive Terminal Design

| Strategy | When |
|----------|------|
| Proportional split | Panels maintain percentage ratios on resize |
| Priority collapse | Less important panels hide first |
| Stacking | Panels collapse to title bars, active expands |
| Breakpoint modes | Switch layout below threshold |
| Minimum size gate | "Terminal too small" below 80x24 |

Use constraint-based layouts, not absolute positions. Test at 80x24, 120x40, 200x60.

---

## 3. Interaction Model

### Keyboard Layers

| Layer | Keys | Audience | Visible? |
|-------|------|----------|----------|
| L0: Universal | Arrows, Enter, Esc, q, Ctrl+Q | Everyone | Footer |
| L1: Vim | hjkl, /, ?, :, gg, G | Intermediate | Footer |
| L2: Actions | d(elete), c(ommit), p(ush) | Regular | On `?` |
| L3: Power | Macros, custom bindings | Expert | Docs only |

**Lingua franca:** j/k=up/down, h/l=left/right, /=search, ?=help, :=command, q=quit, Enter=select, Tab=focus, Space=toggle.

**Never bind:** Ctrl+C, Ctrl+Z, Ctrl+\. These belong to the terminal.

### Modal State Machine

Every TUI with input modes MUST define a formal state machine. Ad-hoc booleans cause stuck states.

```
NORMAL ──[i]──→ INPUT ──[Enter]──→ WAITING ──[receive]──→ NORMAL
  ↑               │                    │
  │            [Esc]                [Esc] (cancel)
  │               │                    │
  └───────────────┘                    └──→ NORMAL (is_waiting=false!)
```

**Invariant:** Every state has an exit path. Every async state (WAITING) resets on BOTH success AND failure. See [references/state-machines.md](references/state-machines.md).

### Terminal ASCII Gotchas

| Combo | Byte | Appears As | Implication |
|-------|------|-----------|-------------|
| Ctrl+I | 0x09 | Tab | Cannot distinguish from Tab key |
| Ctrl+M | 0x0D | Enter | Cannot distinguish from Enter |
| Ctrl+H | 0x08 | Backspace | Cannot distinguish from Backspace |
| Ctrl+[ | 0x1B | Esc | Cannot distinguish from Esc |

This is terminal ASCII, not a bug. Don't bind Ctrl+I differently from Tab.

### Focus Management

- One widget receives input at a time. Tab cycles forward, Shift+Tab backward.
- Focus indicator: highlighted border or color change.
- Modal dialogs trap focus — background gets no events.
- Search: `/` to open, `n`/`N` next/prev, `Esc` dismiss, fuzzy by default.

### Help: Three Tiers

| Tier | Trigger | Content |
|------|---------|---------|
| Always visible | Footer bar | 3-5 essential shortcuts |
| On demand | `?` key | Full keybindings for current context |
| Documentation | `--help` | Complete reference |

---

## 4. Color Design System

Design for graceful degradation: 16 ANSI → 256 → True Color.

### Semantic Color Slots

| Slot | Purpose |
|------|---------|
| `fg.default` / `fg.muted` / `fg.emphasis` | Text hierarchy |
| `bg.base` / `bg.surface` / `bg.overlay` | Depth layering |
| `accent.primary` / `accent.secondary` | Interactive elements |
| `status.error` / `warning` / `success` / `info` | Semantic feedback |

Never hardcode hex in widget code — reference semantic slots. See [references/visual-catalog.md](references/visual-catalog.md) for character reference.

### Hierarchy Recipe
80% content in `fg.default`. Headers bold + `fg.emphasis`. Metadata dim + `fg.muted`. Status in semantic colors. Accents for interactive elements only.

### Accessibility
- WCAG AA: 4.5:1 contrast for body, 3:1 for UI elements
- Never color alone: pair with symbols, text, position
- Safe pairs: blue+orange, blue+yellow, black+white
- Respect `NO_COLOR` environment variable

---

## 5. Data Visualization

| Element | Characters | Use For |
|---------|-----------|---------|
| Full blocks | `█▉▊▋▌▍▎▏` | Progress bars, bar charts |
| Shade blocks | `░▒▓█` | Heatmaps, density |
| Braille | `⠁⠂⠃...⣿` | High-res line graphs |
| Sparkline | `▁▂▃▄▅▆▇█` | Inline mini-charts |

Spinners: braille dots `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` at 80ms for default. Show only after 200ms delay.

---

## 6. Animation & Motion

- Double buffer + synchronized output + batched writes = flicker-free
- Selection changes: instant (0ms). View transitions: 100-200ms. Data loading: spinner.
- Animations NEVER delay user input. Cancel on keypress.
- Cap refresh to 15-30 FPS for dashboards.

---

## 7. Seven Design Principles

1. **Keyboard-first, mouse-optional**
2. **Spatial consistency** — panels stay fixed, users build memory
3. **Progressive disclosure** — footer → `?` → docs
4. **Async everything** — never freeze, cancel with Esc
5. **Semantic color** — usable without color
6. **Contextual intelligence** — keys/status update per mode
7. **Design in layers** — monochrome → 16 ANSI → true color

---

## 8. Anti-Pattern Checklist

| # | Anti-Pattern | Fix |
|---|-------------|-----|
| 1 | Colors break on terminals | 16 ANSI foundation, test 3+ emulators |
| 2 | Flickering | Double buffer + sync output |
| 3 | Undiscoverable keys | Footer + `?` overlay |
| 4 | Broken on Windows/WSL | Test Windows Terminal |
| 5 | Unicode inconsistency | Box-drawing + block elements only |
| 6 | Multiplexer incompatibility | Test in tmux/zellij |
| 7 | No accessibility | `NO_COLOR`, never color-only |
| 8 | Blocking UI | Async + spinners + progress |
| 9 | Modal confusion | Mode in status bar, cursor shape changes |
| 10 | Over-decorated chrome | Content IS the interface |
| **11** | **State leak (stuck mode)** | **Enum state machine, not booleans** |
| **12** | **Scroll amnesia** | **Auto-follow with manual override, PageUp/Down** |
| **13** | **No session persistence** | **JSONL auto-save, session list UI** |

---

## 9. Compatibility Checklist

- [ ] Works at 80x24 minimum
- [ ] Handles resize without crash
- [ ] Correct on dark AND light themes
- [ ] Respects `NO_COLOR`
- [ ] Works inside tmux/zellij
- [ ] Functions over SSH
- [ ] Mouse doesn't break text selection
- [ ] All features keyboard-accessible
- [ ] No escape sequence leaks
- [ ] Exits cleanly on Ctrl+C (restores terminal)
- [ ] **Ctrl+Q quits from ANY mode**
- [ ] **Esc returns to normal from ANY input/waiting state**

---

## 10. Chat Interface Patterns

The dominant TUI pattern of 2025-2026. For detailed patterns, see [references/chat-patterns.md](references/chat-patterns.md).

### Message Rendering
- User messages: bold, accent color, right-aligned or `you>` prefix
- Assistant: default fg, markdown rendered, left-aligned
- Tool calls: indented, monospace, collapsible
- Errors: red, `[error]` prefix

### Streaming Display
Append tokens to a growing Paragraph. Show blinking cursor `█` during stream. Cap render to 30 FPS. Remove cursor on completion.

### Input Lifecycle
```
Normal → [i] → Input → [Enter] → Waiting → receive → Normal
```
Enter sends. Esc cancels (in Input: clears; in Waiting: cancels async AND resets state). Ctrl+O for literal newlines.

---

## 11. Session Management

### What to Persist
Messages, model config, timestamps, scroll position. Format: JSONL (append-safe, crash-recoverable).

### UI Pattern
Ctrl+S opens session list. Enter loads. `n` creates new. `d` deletes. Esc dismisses. Auto-save after every assistant response. On startup, offer to resume or start fresh.

See [references/chat-patterns.md](references/chat-patterns.md) for file format and storage layout.

---

## 12. Scroll Viewport

### State
```
{ total_lines, viewport_height, scroll_offset, auto_follow: bool }
```

### Behavior
- `auto_follow = true` by default (snap to bottom on new content)
- User scrolls up → `auto_follow = false`, show `↓ N new` indicator
- User scrolls to bottom (End/G) → `auto_follow = true`
- PageUp/PageDown move half viewport. Home/End jump to extremes.

---

## References

| File | Content | When to Read |
|------|---------|-------------|
| [app-patterns.md](references/app-patterns.md) | Real-world TUI analysis (lazygit, k9s, yazi, etc.) | Layout selection |
| [visual-catalog.md](references/visual-catalog.md) | Unicode characters, box-drawing, block elements | Visual design |
| [chat-patterns.md](references/chat-patterns.md) | Message rendering, streaming, sessions, tool calls | Building chat TUIs |
| [state-machines.md](references/state-machines.md) | Modal state machines, transition tables, error recovery | Fixing stuck states |
| [ratatui-patterns.md](references/ratatui-patterns.md) | MVU architecture, event loop, widgets, async | Ratatui-specific work |
