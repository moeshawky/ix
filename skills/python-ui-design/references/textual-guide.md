# Textual TUI Reference

CSS-styled terminal applications in Python. Rich rendering, async workers, reactive attributes, screen navigation.

## Table of Contents

1. [App Structure](#app-structure)
2. [CSS Styling](#css-styling)
3. [Key Widgets](#key-widgets)
4. [Workers & Async](#workers--async)
5. [Screens & Navigation](#screens--navigation)
6. [Chat App Pattern](#chat-app-pattern)

---

## App Structure

```python
from textual.app import App, ComposeResult
from textual.widgets import Header, Footer, Static

class MyApp(App):
    CSS_PATH = "style.tcss"             # external CSS
    BINDINGS = [
        ("q", "quit", "Quit"),
        ("d", "toggle_dark", "Dark mode"),
        ("?", "help", "Help"),
    ]

    def compose(self) -> ComposeResult:
        yield Header()
        yield Static("Hello, World!", id="main")
        yield Footer()

    def action_toggle_dark(self) -> None:
        self.dark = not self.dark

if __name__ == "__main__":
    MyApp().run()
```

### Lifecycle
1. `compose()` — build widget tree (called once)
2. `on_mount()` — after widgets attached to DOM
3. Event handlers: `on_key()`, `on_button_pressed()`, etc.
4. `action_*()` — bound to keybindings

---

## CSS Styling

Textual uses TCSS (subset of CSS):

```css
/* style.tcss */
Screen {
    layout: vertical;
}

#main {
    width: 100%;
    height: 1fr;
    background: $surface;
    color: $text;
    padding: 1 2;
}

/* Class selectors */
.message-user {
    color: cyan;
    text-style: bold;
    margin: 0 0 1 0;
}

.message-assistant {
    color: $text;
    margin: 0 0 1 0;
}

.error {
    color: red;
    text-style: italic;
}

/* Focus styling */
Input:focus {
    border: tall $accent;
}

/* Dark/light mode */
Screen.-dark-mode {
    background: #1a1b26;
}
```

### Key CSS Properties

| Property | Values | Use |
|----------|--------|-----|
| `layout` | `vertical`, `horizontal`, `grid` | Container layout |
| `width`/`height` | `auto`, `100%`, `1fr`, `30` | Sizing (fr = fractional) |
| `dock` | `top`, `bottom`, `left`, `right` | Pin to edge |
| `margin`/`padding` | `1 2 1 2` (top right bottom left) | Spacing |
| `background` | `$surface`, `#hex`, `rgb()` | Background color |
| `color` | `$text`, `cyan`, `#hex` | Text color |
| `text-style` | `bold`, `italic`, `underline` | Text decoration |
| `border` | `tall $accent`, `round green` | Border style |
| `overflow-y` | `auto`, `scroll`, `hidden` | Vertical scroll |

### Design Tokens
```css
$background: #1a1b26;
$surface: #24283b;
$text: #c0caf5;
$text-muted: #565f89;
$accent: #7aa2f7;
$success: #9ece6a;
$warning: #e0af68;
$error: #f7768e;
```

---

## Key Widgets

| Widget | Purpose | Key Props |
|--------|---------|-----------|
| `Header` | App title bar | `show_clock=True` |
| `Footer` | Keybinding display | Auto from `BINDINGS` |
| `Static` | Text display | `update()` to change content |
| `Label` | Short text | Like Static but simpler |
| `Input` | Single-line text input | `placeholder=`, `password=` |
| `TextArea` | Multi-line editor | `language=` for highlighting |
| `Button` | Clickable button | `variant="primary"`, `on_press` |
| `ListView` | Scrollable list | `ListItem` children |
| `DataTable` | Sortable table | `add_columns()`, `add_rows()` |
| `Tree` | Collapsible tree | `TreeNode` children |
| `TabbedContent` | Tab panels | `TabPane` children |
| `RichLog` | Append-only log | `.write()` for Rich renderables |
| `ProgressBar` | Progress indicator | `.update(progress=0.5)` |
| `LoadingIndicator` | Spinner | Auto-animating |
| `Select` | Dropdown | `options=[("label", value)]` |
| `Switch` | Toggle | `value: bool` |
| `Collapsible` | Expand/collapse | `title=`, `collapsed=True` |

---

## Workers & Async

### Background Work (non-blocking)
```python
from textual import work

class MyApp(App):
    @work(exclusive=True, thread=True)
    async def call_api(self, prompt: str) -> None:
        """Run API call without freezing UI."""
        import httpx
        async with httpx.AsyncClient(timeout=30) as client:
            resp = await client.post(url, json={"prompt": prompt})
            data = resp.json()
        # Post result back to UI thread
        self.post_message(self.APIResponse(data))
```

### Cancellation
```python
@work(exclusive=True)
async def long_task(self) -> None:
    for i in range(100):
        if self.workers.is_cancelled:
            return
        await asyncio.sleep(0.1)
        self.query_one(ProgressBar).update(progress=i/100)

def action_cancel(self) -> None:
    self.workers.cancel_all()
```

### Reactive Attributes
```python
class ChatWidget(Widget):
    is_waiting = reactive(False)
    message_count = reactive(0)

    def watch_is_waiting(self, waiting: bool) -> None:
        """Auto-called when is_waiting changes."""
        self.query_one(Input).disabled = waiting
        self.query_one("#spinner").display = waiting

    def watch_message_count(self, count: int) -> None:
        self.query_one("#counter").update(f"{count} messages")
```

---

## Screens & Navigation

```python
from textual.screen import Screen

class SessionListScreen(Screen):
    BINDINGS = [("escape", "pop_screen", "Back")]

    def compose(self) -> ComposeResult:
        yield ListView(
            ListItem(Label(s.title)) for s in load_sessions()
        )

class ChatScreen(Screen):
    def compose(self) -> ComposeResult:
        yield RichLog(id="chat")
        yield Input(placeholder="Type a message...", id="input")

class MyApp(App):
    SCREENS = {"sessions": SessionListScreen, "chat": ChatScreen}

    def action_sessions(self) -> None:
        self.push_screen("sessions")
```

Screen stack: `push_screen()` → `pop_screen()`. Like browser navigation.

---

## Chat App Pattern

Complete chat TUI in Textual:

```python
from textual.app import App, ComposeResult
from textual.widgets import Header, Footer, RichLog, Input
from textual.containers import Vertical
from textual import work
from rich.text import Text

class ChatApp(App):
    CSS = """
    RichLog { height: 1fr; border: round $accent; padding: 1; }
    Input { dock: bottom; margin: 1 0 0 0; }
    """
    BINDINGS = [("ctrl+q", "quit", "Quit"), ("ctrl+s", "sessions", "Sessions")]

    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
        yield Vertical(
            RichLog(id="chat", wrap=True, highlight=True, markup=True),
            Input(placeholder="[i] to focus, Enter to send", id="input"),
        )
        yield Footer()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        log = self.query_one(RichLog)
        log.write(Text(f"you> {event.value}", style="bold cyan"))
        self.call_api(event.value)
        event.input.clear()

    @work(exclusive=True)
    async def call_api(self, prompt: str) -> None:
        log = self.query_one(RichLog)
        log.write(Text("thinking...", style="dim"))
        # ... API call ...
        log.write(Text(f"ai> {response}", style="white"))

if __name__ == "__main__":
    ChatApp().run()
```
