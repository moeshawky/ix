# Chat Interface Patterns for TUI

Patterns for building conversational interfaces in the terminal. The dominant TUI paradigm of 2025-2026 (Claude Code, Codex, aichat, Charlie CTL).

## Table of Contents

1. [Message Rendering](#message-rendering)
2. [Streaming Token Display](#streaming-token-display)
3. [Scroll Viewport](#scroll-viewport)
4. [Input Lifecycle](#input-lifecycle)
5. [Session Management](#session-management)
6. [Tool Call Display](#tool-call-display)
7. [Multi-Turn Layout](#multi-turn-layout)

---

## Message Rendering

### Role-Based Styling

| Role | Visual Treatment | Example |
|------|-----------------|---------|
| **User** | Right-aligned or prefixed `you>`, bold, accent color | `you> analyze this code` |
| **Assistant** | Left-aligned or prefixed `ai>`, default fg, markdown rendered | `ai> The function has a bug...` |
| **System** | Dimmed, italic, no prefix | *System prompt loaded* |
| **Tool** | Indented, monospace, bordered block | `  >> shell_exec("cargo check")` |
| **Error** | Red foreground, `[error]` prefix | `[error] Connection refused` |

### Message Block Structure
```
┌─ you ──────────────────────────────────────────────┐
│ Fix the timeout bug in lib.rs                       │
└─────────────────────────────────────────────────────┘

┌─ assistant ─────────────────────────────────────────┐
│ I'll fix the timeout by wrapping read_line with     │
│ tokio::time::timeout.                               │
│                                                     │
│   >> shell_exec("cargo check -p charlie-mcp")       │
│      = Finished dev profile [unoptimized] in 1.2s   │
│                                                     │
│ The fix compiles cleanly.                           │
└─────────────────────────────────────────────────────┘
```

### Markdown in Terminal
- **Bold**: Use SGR bold (attribute 1)
- **Code**: Use SGR reverse or bg.surface background
- **Inline code**: Backtick → reverse video span
- **Code blocks**: Indented 2 chars, syntax highlighted if language detected
- **Lists**: Preserve bullet/number formatting
- **Links**: Underline + clickable via OSC 8 hyperlinks

---

## Streaming Token Display

### Token-by-Token Rendering
```
Strategy: Append tokens to a growing Paragraph widget.
1. On each token: append to buffer string
2. Re-render the Paragraph with updated content
3. Auto-scroll viewport to bottom (if following)
4. Cap re-render rate to 30 FPS to prevent flicker
```

### Cursor Indicator During Streaming
Show a blinking block cursor `█` or spinner at the end of the streaming text:
```
ai> The function has a bug in the
    error handling path where█
```
Remove cursor when streaming completes.

### Batched Token Rendering
For fast token streams (>100 tokens/sec), batch tokens and render at fixed intervals:
```rust
// Buffer tokens, flush to UI every 33ms (30 FPS)
let mut token_buffer = String::new();
loop {
    match rx.try_recv() {
        Ok(token) => token_buffer.push_str(&token),
        Err(TryRecvError::Empty) => {
            if !token_buffer.is_empty() {
                // Flush buffer to UI
                append_to_message(&token_buffer);
                token_buffer.clear();
            }
            sleep(Duration::from_millis(33)).await;
        }
        Err(TryRecvError::Disconnected) => break,
    }
}
```

---

## Scroll Viewport

### State Model
```rust
struct ScrollState {
    total_lines: usize,       // Total rendered lines in chat history
    viewport_height: usize,   // Visible area height
    offset: usize,            // Lines scrolled from bottom (0 = at bottom)
    auto_follow: bool,        // True = snap to bottom on new content
}
```

### Auto-Follow Behavior
```
Rule: auto_follow = true by default.

On new message/token:
  if auto_follow → scroll to bottom
  else → stay at current offset, show "↓ N new" indicator

On user scroll up (PageUp, k, mouse wheel up):
  auto_follow = false
  Show scroll position indicator: "line 42/380"

On user scroll to bottom (End, G, click "↓ new"):
  auto_follow = true

On PageDown past the bottom:
  auto_follow = true
```

### Standard Scroll Bindings
| Key | Action | Context |
|-----|--------|---------|
| PageUp / Ctrl+U | Scroll up half viewport | Always |
| PageDown / Ctrl+D | Scroll down half viewport | Always |
| Home / gg | Scroll to top | Normal mode |
| End / G | Scroll to bottom, re-enable auto-follow | Normal mode |
| j / ↓ | Scroll down 1 line | Normal mode |
| k / ↑ | Scroll up 1 line | Normal mode |
| Mouse wheel | Scroll up/down 3 lines | Always |

### Scroll Indicator
Show in the right margin or status bar:
```
 42/380 ─── scrollbar ─── [↓ 12 new]
```

---

## Input Lifecycle

### State Machine
```
                    [i]
    NORMAL ──────────────→ INPUT
      │                      │
      │ (on new msg)         │ [Enter] (send)
      │                      ▼
      │                   WAITING ────→ NORMAL
      │                      │        (on receive/error)
      │                      │
      │                      │ [Esc] (cancel)
      │                      ▼
      │                   NORMAL
      │                   (is_waiting = false!)
      ▼
    (quit on q/Esc/Ctrl+Q)
```

**Critical invariant:** Every WAITING state MUST have a path back to NORMAL. Both success (receive) AND failure (error/cancel) must reset `is_waiting = false`.

### Multi-Line Input
Two approaches:
1. **Single-line with Ctrl+O for newline**: Default. Enter sends. Ctrl+O inserts literal newline.
2. **Multi-line with Alt+Enter to send**: Textarea mode. Enter inserts newline. Alt+Enter sends.

Show the active approach in the input prompt:
```
you> your message here...          [Enter: send | Ctrl+O: newline]
```

### Input History
- Up/Down arrow cycles through previous inputs (like shell history)
- Store last 100 inputs in memory
- Persist to session file for cross-session recall

---

## Session Management

### What to Persist
```json
{
  "id": "session-2026-03-20-abc123",
  "created_at": "2026-03-20T09:15:00Z",
  "updated_at": "2026-03-20T11:42:00Z",
  "model": "nvidia/nemotron-3-super-120b-a12b",
  "service": "brain_llm",
  "messages": [
    {"role": "system", "content": "..."},
    {"role": "user", "content": "fix the timeout bug"},
    {"role": "assistant", "content": "I'll wrap read_line..."}
  ],
  "title": "Fix MCP timeout",
  "tags": ["mcp", "bug-fix"]
}
```

### File Format
JSONL (one JSON object per line) for append-friendly writes:
```
{"type":"meta","id":"session-abc","model":"nemotron-super","created":"2026-03-20T09:15:00Z"}
{"type":"message","role":"user","content":"fix the timeout bug","ts":"2026-03-20T09:15:30Z"}
{"type":"message","role":"assistant","content":"I'll wrap read_line...","ts":"2026-03-20T09:16:45Z"}
```

### Storage Location
```
.charlie/sessions/
  session-2026-03-20-abc123.jsonl
  session-2026-03-19-def456.jsonl
  index.json  ← session list with titles, dates, message counts
```

### Session UI Patterns
```
┌─ Sessions (Ctrl+S) ────────────────────────────────┐
│  * Fix MCP timeout          2026-03-20  12 msgs    │
│    Debug TUI navigation     2026-03-19  47 msgs    │
│    Convoy dispatch planning  2026-03-18  89 msgs    │
│                                                     │
│  [Enter] Load  [n] New  [d] Delete  [Esc] Cancel   │
└─────────────────────────────────────────────────────┘
```

### Auto-Save
- Save after every assistant response (not during streaming)
- On exit: save current session automatically
- On crash: JSONL format is append-safe, partial writes don't corrupt earlier messages
- On startup: offer to resume last session or start new

---

## Tool Call Display

### Inline Tool Results
```
ai> Let me check the compilation.

  >> cargo check -p charlie-mcp
     ✓ Finished dev [unoptimized] in 1.2s

  >> file_read libs/charlie-mcp/src/lib.rs
     (223 lines)

The timeout fix compiles cleanly.
```

### Collapsed Tool Calls
For verbose tool output, show collapsed by default:
```
  >> shell_exec("cargo test -p charlie-mcp 2>&1")  [5 passed, 0 failed] ▸
```
Press Enter or `l` to expand. `h` to collapse.

---

## Multi-Turn Layout

### Conversation Flow
```
┌─ Chat ──────────────────────────── nemotron-super ─┐
│                                                     │
│  you> Fix the timeout in send_request               │
│                                                     │
│  ai> I'll wrap read_line with tokio::time::timeout. │
│      >> cargo check -p charlie-mcp                  │
│         ✓ Finished in 1.2s                          │
│      Done. The fix compiles.                        │
│                                                     │
│  you> Now add the notification filtering            │
│                                                     │
│  ai> Adding a retry loop that skips lines without   │
│      an id field...█                                │
│                                                     │
├─────────────────────────────────────────────────────┤
│ you> _                          [i]nput [v]erbose   │
└─────────────────────────────────────── 42/380 ──────┘
```

### Key Layout Elements
1. **Title bar**: Tab name + active model
2. **Message area**: Scrollable, auto-follow
3. **Status bar**: Scroll position, mode indicator
4. **Input bar**: Prompt + current input + available keys
5. **Separator**: Visual break between message area and input
