---
name: python-ui-design
description: "Design system for building Python user interfaces spanning terminal TUIs (Textual, Rich) through desktop GUIs (tkinter, ttkbootstrap, CustomTkinter). Parallel to tui-design (Rust/Ratatui) but Python-native. Use when: (1) building Python terminal apps with Textual or Rich, (2) building desktop GUIs with tkinter/ttk, (3) modernizing tkinter with ttkbootstrap or CustomTkinter, (4) choosing between Python UI frameworks, (5) designing chat/conversational interfaces in Python, (6) adding themes, dark mode, or HiDPI support to Python apps, (7) structuring Python UI with MVC/MVU patterns. Triggers on: Python GUI, tkinter, ttkbootstrap, CustomTkinter, Textual, Rich TUI, Python desktop app, ttk theme, Python chat interface, Python dashboard."
---

# Python UI Design System

Design patterns for building Python user interfaces — from terminal TUIs (Textual/Rich) to desktop GUIs (tkinter/ttkbootstrap/CustomTkinter).

**Core philosophy:** Python UIs earn their place by being fast to build, cross-platform by default, and visually modern without external dependencies.

**Design process:** Use `productive_reason` for UI design decisions. Use `scratchpad` to track layout alternatives and state management choices.

## Framework Selector

Choose based on deployment target and visual requirements:

| Need | Framework | Install | Visual Quality |
|------|-----------|---------|---------------|
| Terminal dashboard/tool | **Textual** | `pip install textual` | Excellent (CSS-styled TUI) |
| Terminal output formatting | **Rich** | `pip install rich` | Excellent (tables, trees, markdown) |
| Desktop GUI, modern flat look | **ttkbootstrap** | `pip install ttkbootstrap` | Good (Bootstrap-inspired themes) |
| Desktop GUI, rounded widgets | **CustomTkinter** | `pip install customtkinter` | Good (macOS-inspired) |
| Desktop GUI, maximum control | **tkinter + ttk** | Built-in | Basic (needs manual styling) |
| Desktop GUI, native look | **PyQt6 / PySide6** | `pip install PyQt6` | Excellent (native widgets) |

### Decision Flow
```
Is it a terminal app?
  ├── Yes → Rich for output-only, Textual for interactive
  └── No (desktop GUI) →
        Need native OS look? → PyQt6/PySide6
        Need fast prototyping? → ttkbootstrap (13 themes, zero config)
        Need custom rounded widgets? → CustomTkinter
        Need zero dependencies? → tkinter + ttk (built-in)
```

---

## 1. Architecture Patterns

### MVC for GUI Apps (tkinter/ttkbootstrap/CustomTkinter)
```python
# Model: pure data, no UI imports
class ChatSession:
    messages: list[Message]
    model_name: str
    is_waiting: bool = False

# View: widgets only, no business logic
class ChatView(ttk.Frame):
    def __init__(self, parent):
        self.text_area = scrolledtext.ScrolledText(parent)
        self.input_entry = ttk.Entry(parent)
        self.send_btn = ttk.Button(parent, text="Send")

# Controller: binds model ↔ view
class ChatController:
    def __init__(self, model, view):
        view.send_btn.config(command=self.on_send)
    def on_send(self):
        text = self.view.input_entry.get()
        self.model.messages.append(Message("user", text))
        self.view.update_messages(self.model.messages)
```

### Message-Based for TUI Apps (Textual)
```python
# Textual uses Elm-style message passing
class ChatApp(App):
    class SendMessage(Message): pass
    class ReceiveResponse(Message):
        def __init__(self, content: str): self.content = content

    def on_send_message(self, msg: SendMessage) -> None:
        self.run_worker(self.call_api())  # async, non-blocking

    def on_receive_response(self, msg: ReceiveResponse) -> None:
        self.query_one(RichLog).write(msg.content)
```

**Rule:** Never call `time.sleep()` or blocking I/O in the main thread. Use `threading.Thread` (tkinter) or `run_worker` (Textual).

---

## 2. Threading & Async

### tkinter: Thread + Queue
```python
import threading, queue

class App:
    def __init__(self):
        self.queue = queue.Queue()
        self.root.after(100, self.poll_queue)  # check every 100ms

    def poll_queue(self):
        while not self.queue.empty():
            msg = self.queue.get_nowait()
            self.handle_message(msg)
        self.root.after(100, self.poll_queue)

    def start_api_call(self, prompt):
        threading.Thread(target=self._api_worker, args=(prompt,), daemon=True).start()

    def _api_worker(self, prompt):
        result = call_nim_api(prompt)  # blocking OK in thread
        self.queue.put(("response", result))  # NEVER touch widgets from thread
```

**Iron rule:** In tkinter, ONLY the main thread can touch widgets. Use `queue.Queue` + `root.after()` polling.

### Textual: Native Async
```python
class MyApp(App):
    @work(exclusive=True)
    async def call_api(self, prompt: str) -> None:
        async with httpx.AsyncClient() as client:
            resp = await client.post(url, json=body)
        self.post_message(self.ReceiveResponse(resp.json()))
```

Textual is async-native. Use `@work` decorator for background tasks. `self.post_message()` is thread-safe.

---

## 3. Visual Design

### ttkbootstrap Themes
13 built-in themes: `cosmo`, `flatly`, `journal`, `litera`, `lumen`, `minty`, `pulse`, `sandstone`, `solar`, `superhero`, `darkly`, `cyborg`, `vapor`.

```python
import ttkbootstrap as ttk
root = ttk.Window(themename="darkly")  # dark mode instant
```

| Theme | Style | Use For |
|-------|-------|---------|
| `darkly` | Dark, high contrast | Developer tools, monitoring |
| `cosmo` | Clean, modern | Business apps |
| `superhero` | Dark blue/orange | Dashboards |
| `solar` | Solarized dark | Code editors |
| `minty` | Light, green accent | Data entry, forms |
| `vapor` | Neon/cyberpunk | Creative tools |

### CustomTkinter Appearance
```python
import customtkinter as ctk
ctk.set_appearance_mode("dark")  # "light", "dark", "system"
ctk.set_default_color_theme("blue")  # "blue", "green", "dark-blue"
```

### Color System (parallel to tui-design §4)

| Slot | tkinter/ttk | ttkbootstrap | CustomTkinter |
|------|-------------|-------------|---------------|
| Primary | `style.configure(background=)` | `bootstyle="primary"` | `fg_color=` |
| Success | manual | `bootstyle="success"` | `fg_color="green"` |
| Danger | manual | `bootstyle="danger"` | `fg_color="red"` |
| Dark bg | `configure(bg="#1a1b26")` | `themename="darkly"` | `set_appearance_mode("dark")` |

**Rule:** Never hardcode hex colors in widget constructors. Use theme variables or a config dict.

---

## 4. Common Widget Patterns

### Chat Message Display

**tkinter/ttkbootstrap:**
```python
text_widget = scrolledtext.ScrolledText(parent, wrap=tk.WORD, state=tk.DISABLED)
text_widget.tag_configure("user", foreground="#7aa2f7", font=("", 10, "bold"))
text_widget.tag_configure("assistant", foreground="#c0caf5")
text_widget.tag_configure("error", foreground="#f7768e")

def append_message(role, content):
    text_widget.config(state=tk.NORMAL)
    text_widget.insert(tk.END, f"{role}> ", role)
    text_widget.insert(tk.END, f"{content}\n\n")
    text_widget.see(tk.END)  # auto-scroll
    text_widget.config(state=tk.DISABLED)
```

**Textual:**
```python
class ChatLog(RichLog):
    def add_message(self, role: str, content: str):
        style = "bold cyan" if role == "user" else "white"
        self.write(Text(f"{role}> {content}", style=style))
```

### Session Save/Load
```python
import json
from pathlib import Path

SESSIONS_DIR = Path.home() / ".myapp" / "sessions"

def save_session(session: ChatSession):
    path = SESSIONS_DIR / f"{session.id}.jsonl"
    with open(path, "a") as f:
        for msg in session.new_messages:
            f.write(json.dumps(msg.to_dict()) + "\n")

def load_session(session_id: str) -> ChatSession:
    path = SESSIONS_DIR / f"{session_id}.jsonl"
    messages = [json.loads(line) for line in open(path)]
    return ChatSession.from_messages(messages)
```

### Scrollable Content with Auto-Follow
```python
# tkinter
def auto_scroll(text_widget, follow=True):
    if follow:
        text_widget.see(tk.END)

# Detect manual scroll-up → disable auto-follow
def on_scroll(event):
    # If scrollbar is at bottom, re-enable follow
    if text_widget.yview()[1] >= 0.99:
        app.auto_follow = True
    else:
        app.auto_follow = False
```

---

## 5. State Machine (parallel to tui-design §3)

Same principle as tui-design: enum over booleans, formal transitions.

**Before choosing a state pattern (global model, per-page state, shared context), use `productive_reason` to identify the degrees of freedom in the UI state.** How many independent dimensions can vary simultaneously? That count determines whether you need a flat enum, a product type, or a nested state hierarchy.

```python
from enum import Enum, auto

class UIMode(Enum):
    NORMAL = auto()
    INPUT = auto()
    WAITING = auto()
    ERROR = auto()

class UIState:
    mode: UIMode = UIMode.NORMAL

    def transition(self, event: str):
        transitions = {
            (UIMode.NORMAL, "start_input"): UIMode.INPUT,
            (UIMode.INPUT, "send"): UIMode.WAITING,
            (UIMode.INPUT, "cancel"): UIMode.NORMAL,
            (UIMode.WAITING, "receive"): UIMode.NORMAL,
            (UIMode.WAITING, "error"): UIMode.ERROR,
            (UIMode.WAITING, "cancel"): UIMode.NORMAL,  # MUST reset
            (UIMode.ERROR, "dismiss"): UIMode.NORMAL,
        }
        new_mode = transitions.get((self.mode, event))
        if new_mode:
            self.mode = new_mode
```

**Invariant:** Every WAITING state has a path back to NORMAL.

---

## 6. Essential Complexity in UI

> **Complex state machines (modal flows, multi-panel focus management, async data fetching) are ESSENTIAL complexity in TUIs. Don't simplify them away — a dashboard that can't handle async updates isn't simpler, it's broken.**

This is the most common mistake when "cleaning up" TUI code:

- A multi-panel layout that tracks which panel has focus is not over-engineered — removing focus tracking makes the keyboard unusable.
- A modal flow with enter/confirm/cancel states is not boilerplate — flattening it causes ghost inputs and broken cancels.
- Async data fetching with loading/error/success states is not premature — dropping the error state means the user stares at a spinner forever.

When you feel the urge to simplify a state machine, use `productive_reason` to ask: "What user-visible behavior breaks if I remove this state?" If the answer is "nothing," remove it. If the answer is "the user can't recover from X," keep it.

---

## 7. Chat Interface Patterns

These patterns are critical for agent-facing platforms (Charlie, agentic tools, RAG chat UIs).

### RAG-Powered Chat Pipeline

Query memory/ChromaDB before calling the LLM. Inject retrieved context as a system message or document block.

```python
async def handle_user_message(prompt: str) -> AsyncIterator[str]:
    # 1. Retrieve context
    results = chroma_client.query(
        collection_name="knowledge",
        query_texts=[prompt],
        n_results=5,
    )
    context_block = "\n\n".join(r["document"] for r in results["documents"][0])

    # 2. Build messages with injected context
    messages = [
        {"role": "system", "content": f"Relevant context:\n{context_block}"},
        *session.history,
        {"role": "user", "content": prompt},
    ]

    # 3. Stream NIM/LLM response
    async for chunk in nim_stream(messages):
        yield chunk
```

### Conversation Persistence

Use SQLite for single-user tools; PostgreSQL for multi-user or shared deployments.

**SQLite (single-user, Charlie-style):**
```python
import sqlite3
from pathlib import Path

DB_PATH = Path.home() / ".myapp" / "sessions.db"

def init_db(conn: sqlite3.Connection) -> None:
    conn.execute("""
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            created_at TEXT,
            model TEXT
        )
    """)
    conn.execute("""
        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT,
            role TEXT,
            content TEXT,
            timestamp TEXT,
            FOREIGN KEY (session_id) REFERENCES sessions(id)
        )
    """)
    conn.commit()

def append_message(conn: sqlite3.Connection, session_id: str, role: str, content: str) -> None:
    conn.execute(
        "INSERT INTO messages (session_id, role, content, timestamp) VALUES (?, ?, ?, datetime('now'))",
        (session_id, role, content),
    )
    conn.commit()

def load_history(conn: sqlite3.Connection, session_id: str) -> list[dict]:
    rows = conn.execute(
        "SELECT role, content FROM messages WHERE session_id = ? ORDER BY id",
        (session_id,),
    ).fetchall()
    return [{"role": r, "content": c} for r, c in rows]
```

### Visible Reasoning Phases

Render the agent's reasoning structure as formatted markdown sections. This is a first-class UI feature, not a debug artifact.

```python
PHASE_STYLES = {
    "## SEE":      "bold yellow",      # Observation phase
    "## EXPLORE":  "bold cyan",        # Hypothesis generation
    "## CONVERGE": "bold green",       # Decision / conclusion
    "## REFLECT":  "bold magenta",     # Meta-cognition / self-correction
}

def render_reasoning_chunk(log: RichLog, chunk: str) -> None:
    """Stream reasoning tokens, applying phase styles as headers appear."""
    for line in chunk.splitlines():
        style = next(
            (style for header, style in PHASE_STYLES.items() if line.startswith(header)),
            "dim white",
        )
        log.write(Text(line, style=style))
```

**Textual CSS for phase panels:**
```css
.phase-see     { color: $warning; border: solid $warning; }
.phase-explore { color: $accent;  border: solid $accent; }
.phase-converge{ color: $success; border: solid $success; }
.phase-reflect { color: $primary; border: solid $primary; }
```

### Operator Correction as DPO Signal

Every time the operator corrects, redirects, or overrides the agent's output, that interaction is a training signal. Capture it.

```python
@dataclass
class CorrectionEvent:
    session_id: str
    message_id: int          # the message being corrected
    original_content: str    # what the agent produced
    correction: str          # what the operator said instead
    timestamp: str

def record_correction(conn: sqlite3.Connection, event: CorrectionEvent) -> None:
    """Persist operator correction as a DPO (chosen/rejected) pair."""
    conn.execute("""
        INSERT INTO corrections
            (session_id, message_id, rejected, chosen, timestamp)
        VALUES (?, ?, ?, ?, ?)
    """, (
        event.session_id,
        event.message_id,
        event.original_content,   # rejected
        event.correction,         # chosen
        event.timestamp,
    ))
    conn.commit()
```

**UI integration:** Any input submitted after an assistant message that edits/contradicts it should fire `record_correction`. Make this automatic — operators don't think in terms of training data.

---

## 8. HiDPI & Cross-Platform

### tkinter HiDPI
```python
import ctypes, sys
if sys.platform == "win32":
    ctypes.windll.shcore.SetProcessDpiAwareness(2)  # Per-monitor DPI

root = tk.Tk()
root.tk.call("tk", "scaling", 2.0)  # Scale factor for HiDPI
```

### CustomTkinter (automatic)
CustomTkinter handles HiDPI automatically on Windows and macOS. No manual scaling needed.

### Cross-Platform Checklist
- [ ] Test on Windows, macOS, Linux
- [ ] Fonts: use system fonts or bundle TTF (tkinter can't discover fonts reliably)
- [ ] File paths: `pathlib.Path`, never hardcoded separators
- [ ] Theme: test both light and dark system appearance
- [ ] Keyboard: test Ctrl vs Cmd on macOS

---

## 9. Textual-Specific Patterns (Charlie Platform)

These patterns come from Charlie's production TUI experience (charlie-ctl, 12-page sidebar TUI).

### PagePlugin Trait Pattern

Every page is an independent plugin with a stable contract. This enables the sidebar registry to load, render, and route input to any page without knowing its internals.

```python
from abc import ABC, abstractmethod
from textual.widget import Widget

class PagePlugin(ABC):
    @property
    @abstractmethod
    def id(self) -> str:
        """Stable machine identifier (e.g. 'chat', 'scanner'). Never changes."""
        ...

    @property
    @abstractmethod
    def name(self) -> str:
        """Human-readable label shown in sidebar."""
        ...

    @abstractmethod
    def compose(self) -> Widget:
        """Return the root widget tree for this page."""
        ...

    @abstractmethod
    def handle_key(self, key: str) -> bool:
        """Handle a keypress. Return True if consumed, False to propagate."""
        ...
```

### Sidebar Navigation with Registry

```python
class PageRegistry:
    def __init__(self):
        self._pages: dict[str, PagePlugin] = {}
        self._order: list[str] = []

    def register(self, page: PagePlugin) -> None:
        self._pages[page.id] = page
        self._order.append(page.id)

    def get(self, page_id: str) -> PagePlugin | None:
        return self._pages.get(page_id)

    def all_in_order(self) -> list[PagePlugin]:
        return [self._pages[pid] for pid in self._order if pid in self._pages]

    def next_id(self, current_id: str) -> str:
        idx = self._order.index(current_id)
        return self._order[(idx + 1) % len(self._order)]

    def prev_id(self, current_id: str) -> str:
        idx = self._order.index(current_id)
        return self._order[(idx - 1) % len(self._order)]
```

**YAML-driven page config** (`.charlie/tui_pages.yaml` pattern):
```yaml
pages:
  - id: chat
    name: Chat
    tier: Ops
    plugin: brain
  - id: scanner
    name: Scanner
    tier: Infra
    plugin: scanner
```

Load at startup, register in order. The sidebar renders tiers as section headers automatically.

### Theme System with Named Colors

Never use raw hex in Textual CSS. Define a named palette and reference it everywhere.

```css
/* app.tcss */
$bg:        #1a1b26;
$bg-panel:  #24283b;
$fg:        #c0caf5;
$primary:   #7aa2f7;
$secondary: #bb9af7;
$success:   #9ece6a;
$warning:   #e0af68;
$danger:    #f7768e;
$dim:       #565f89;

Screen {
    background: $bg;
    color: $fg;
}

.panel {
    background: $bg-panel;
    border: solid $dim;
}

.panel:focus-within {
    border: solid $primary;
}

.sidebar-item {
    color: $dim;
}

.sidebar-item.--active {
    color: $primary;
    text-style: bold;
}
```

### Multi-Panel Focus Management

Track which panel has focus as explicit state. Tab cycles through panels; the focused panel receives key events first.

```python
from enum import Enum, auto

class FocusPanel(Enum):
    SIDEBAR = auto()
    MAIN = auto()
    INPUT = auto()

class TUIModel:
    focus: FocusPanel = FocusPanel.SIDEBAR
    active_page_id: str = "chat"

    def cycle_focus(self) -> None:
        order = list(FocusPanel)
        idx = order.index(self.focus)
        self.focus = order[(idx + 1) % len(order)]
```

---

## 10. Anti-Patterns

| # | Anti-Pattern | Fix |
|---|-------------|-----|
| 1 | `time.sleep()` in main thread | Thread + queue (tkinter), `@work` (Textual) |
| 2 | Touching widgets from background thread | Queue messages, poll with `root.after()` |
| 3 | Hardcoded colors/fonts | Theme variables, config dict, ttkbootstrap styles |
| 4 | Boolean state flags | Enum state machine |
| 5 | No error recovery from async ops | WAITING always resets on error AND cancel |
| 6 | Blocking file dialogs during async | Disable buttons during operations, use threading |
| 7 | No HiDPI support | `SetProcessDpiAwareness` (Win), `tk scaling` |
| 8 | Giant monolithic App class (>300 lines) | MVC: Model + View + Controller in separate files. If splitting reduces cohesion, document why in a comment at top. |
| 9 | No keyboard shortcuts | `root.bind("<Control-q>", quit)`, accelerators on menus |
| 10 | Forgetting `state=DISABLED` on text displays | Prevent user editing of output areas |
| 11 | Removing complex state machines to "simplify" | Essential complexity — see §6 |
| 12 | Skipping RAG context injection | Query ChromaDB/memory before every LLM call |
| 13 | No operator correction capture | Every redirect is training data — record it |
| 14 | Single-file scripts growing past 500 lines | Split into package: `__init__.py` + `model.py` + `views.py` + `app.py`. One concern per file. |

---

## References

| File | Content | When to Read |
|------|---------|-------------|
| [references/ttkbootstrap-guide.md](references/ttkbootstrap-guide.md) | Widget catalog, themes, Meter/Floodgauge/DateEntry, custom themes | Building with ttkbootstrap |
| [references/textual-guide.md](references/textual-guide.md) | CSS styling, widget tree, screens, workers, reactive attrs | Building with Textual |
| [references/customtkinter-guide.md](references/customtkinter-guide.md) | CTk widgets, appearance modes, scaling, color themes | Building with CustomTkinter |
| [references/tkinter-patterns.md](references/tkinter-patterns.md) | Layout managers, event binding, ttk styling, megawidgets | Raw tkinter/ttk patterns |
