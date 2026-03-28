# Ratatui-Specific Patterns

Framework-specific guidance for building TUIs with Ratatui (Rust). Covers the patterns that are unique to Ratatui's immediate-mode rendering model.

## Table of Contents

1. [Architecture: Model-View-Update](#architecture-model-view-update)
2. [Event Loop with Tokio](#event-loop-with-tokio)
3. [Widget Catalog](#widget-catalog)
4. [Layout Constraints](#layout-constraints)
5. [Common Pitfalls](#common-pitfalls)

---

## Architecture: Model-View-Update

Ratatui uses immediate-mode rendering: you redraw the entire UI every frame, and the library diffs against the previous frame to emit only changed cells.

```rust
// The MVU loop
loop {
    // 1. VIEW: Render current model state
    terminal.draw(|frame| render(frame, &model))?;

    // 2. UPDATE: Handle events, produce messages
    if let Some(event) = poll_event()? {
        let message = handle_event(event, &model);
        update(&mut model, message);
    }

    // 3. Check exit condition
    if model.should_quit { break; }
}
```

### Model (State)
```rust
struct Model {
    active_tab: ActiveTab,
    chat: ChatState,
    mechanic: MechanicState,
    // ... per-tab state
    tools: Vec<Tool>,
    should_quit: bool,
    show_help: bool,
}
```

**Rules:**
- Model is the SINGLE source of truth. No UI state lives outside it.
- Model is plain data (no async, no channels, no Arc). The event loop owns it exclusively.
- Per-tab state is a nested struct, not scattered booleans.

### View (Render)
```rust
fn render(frame: &mut Frame, model: &Model) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(frame.area());

    render_tab_content(frame, chunks[0], model);
    render_status_bar(frame, chunks[1], model);
}
```

**Rules:**
- Render functions are PURE — they read model, write to frame, no side effects.
- Never mutate model during render. Never send messages during render.
- If render is slow, profile it — usually it's string allocation, not drawing.

### Update (Message Handling)
```rust
enum Message {
    Quit,
    TabNext,
    ChatStartInput,
    ChatSend,
    ChatReceive(String),
    ChatError(String),
    ChatCancelGeneration,
    // ...
}

fn update(model: &mut Model, msg: Message) {
    match msg {
        Message::Quit => model.should_quit = true,
        Message::ChatCancelGeneration => {
            model.chat.mode = ChatMode::Normal; // ALWAYS reset
        }
        // ...
    }
}
```

**Rules:**
- Every message handler is a pure state transition. No I/O, no async.
- Async work is SPAWNED from the event handler, results arrive as messages via channel.
- Message handlers must be exhaustive — every enum variant handled.

---

## Event Loop with Tokio

### Channel-Based Async Integration
```rust
let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();

loop {
    // Poll for crossterm events OR async messages
    tokio::select! {
        // Terminal events (key presses, resize)
        event = crossterm_event_stream.next() => {
            if let Some(Ok(event)) = event {
                handle_key_event(event, &mut model, &tx);
            }
        }
        // Async results (API responses, tool outputs)
        Some(msg) = rx.recv() => {
            update(&mut model, msg);
        }
    }

    terminal.draw(|f| render(f, &model))?;

    if model.should_quit { break; }
}
```

### Spawning Async Work
```rust
// In key handler, when user presses Enter to send:
let tx = tx.clone();
let messages = model.chat.build_api_messages();
let cancel = Arc::new(AtomicBool::new(false));
model.chat.cancel_token = cancel.clone();

tokio::spawn(async move {
    match send_completion(messages, &service, &tools, &cancel).await {
        Ok(response) => { let _ = tx.send(Message::ChatReceive(response)); }
        Err(e) => { let _ = tx.send(Message::ChatError(e)); }
    }
});
```

**Rules:**
- Spawned tasks communicate ONLY via the message channel. Never mutate model from a task.
- Store the cancel token in the model so the event handler can trigger cancellation.
- Tasks must check the cancel flag periodically and exit early.

---

## Widget Catalog

### Key Widgets for Chat TUIs

| Widget | Use For | Key Properties |
|--------|---------|---------------|
| `Paragraph` | Chat messages, long text | `.scroll((offset, 0))`, `.wrap(Wrap { trim: false })` |
| `List` | Session list, tool list | `.highlight_style()`, `.highlight_symbol(">> ")` |
| `Table` | Structured data (services, tools) | Column widths via `Constraint`, sortable with state |
| `Gauge` | Progress bars | `.ratio(0.67)`, `.label("67%")` |
| `Block` | Panel borders | `.borders(Borders::ALL)`, `.border_type(BorderType::Rounded)` |
| `Tabs` | Tab bar | `.select(active_index)`, `.highlight_style()` |
| `Sparkline` | Inline charts | `.data(&[1,4,2,8,5])`, braille rendering |

### Paragraph Scrolling (Critical for Chat)
```rust
let paragraph = Paragraph::new(chat_text)
    .wrap(Wrap { trim: false })
    .scroll((model.chat.scroll_offset as u16, 0));

// Calculate total lines after wrapping
let line_count = paragraph.line_count(area.width);
model.chat.total_lines = line_count;
```

**Gotcha**: `Paragraph::scroll()` takes `(vertical_offset, horizontal_offset)`. The offset is in WRAPPED lines, not source lines. You must calculate wrapped line count to know the scroll range.

### Focus Indicator Pattern
```rust
let border_style = if is_focused {
    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
} else {
    Style::default().fg(Color::DarkGray)
};

let block = Block::default()
    .borders(Borders::ALL)
    .border_style(border_style)
    .title(if is_focused { " Panel (active) " } else { " Panel " });
```

---

## Layout Constraints

### The Constraint System
```rust
Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(3),      // Fixed: exactly 3 rows (tab bar)
        Constraint::Min(10),        // Flexible: at least 10 rows (content)
        Constraint::Length(3),      // Fixed: exactly 3 rows (input bar)
    ])
    .split(area)
```

| Constraint | Behavior |
|-----------|----------|
| `Length(n)` | Exactly n cells. Use for fixed chrome (headers, footers). |
| `Min(n)` | At least n cells, expands to fill. Use for main content. |
| `Max(n)` | At most n cells. Use for sidebars that shouldn't dominate. |
| `Percentage(p)` | p% of parent. Use for proportional splits. |
| `Ratio(a, b)` | a/b of parent. More precise than percentage. |
| `Fill(w)` | Fills remaining space with weight w. Use for flexible layouts. |

### Responsive Pattern
```rust
let area = frame.area();
if area.width < 80 {
    // Single column layout
    render_stacked(frame, area, model);
} else if area.width < 120 {
    // Two column layout
    render_split(frame, area, model);
} else {
    // Three column layout
    render_wide(frame, area, model);
}
```

---

## Common Pitfalls

| Pitfall | Symptom | Fix |
|---------|---------|-----|
| **Blocking the event loop** | UI freezes during API calls | Spawn async tasks, communicate via channel |
| **Mutating model in render** | Inconsistent state, flicker | Render functions are PURE — read only |
| **Forgetting terminal restore** | Terminal left in raw mode after crash | Use `scopeguard` or `Drop` impl to call `disable_raw_mode()` |
| **String allocation in render** | Slow frame times | Pre-format strings in update, cache in model |
| **Scroll offset overflow** | Panic on small terminal | Clamp: `offset = offset.min(total.saturating_sub(viewport))` |
| **Not handling resize** | Layout breaks | Re-render on `Event::Resize`, use constraint-based layouts |
| **State booleans instead of enum** | Stuck states, impossible combinations | Use enum for mode (Normal/Input/Waiting/Error) |
| **Ctrl+I surprise** | Tab navigation fires when user types Ctrl+I | Terminal ASCII: Ctrl+I = 0x09 = Tab. Can't distinguish. |
