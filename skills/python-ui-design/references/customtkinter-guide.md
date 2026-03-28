# CustomTkinter Reference

Modern, rounded-corner widgets for tkinter. Auto HiDPI, system dark/light mode detection, macOS-inspired aesthetic.

## Table of Contents

1. [Setup](#setup)
2. [Widget Catalog](#widget-catalog)
3. [Appearance & Themes](#appearance--themes)
4. [Layout Patterns](#layout-patterns)
5. [Chat App Pattern](#chat-app-pattern)

---

## Setup

```python
import customtkinter as ctk

ctk.set_appearance_mode("system")    # "light", "dark", "system"
ctk.set_default_color_theme("blue")  # "blue", "green", "dark-blue"

app = ctk.CTk()
app.geometry("800x600")
app.title("My App")
```

### Key Differences from tkinter
- Widgets prefixed with `CTk`: `CTkButton`, `CTkEntry`, `CTkFrame`
- Auto-handles HiDPI on Windows and macOS
- Rounded corners by default (`corner_radius=` on all widgets)
- Dark/light mode via `set_appearance_mode()`
- Can mix with standard tkinter widgets (they won't match style)

---

## Widget Catalog

| Widget | tkinter Equivalent | Key Props |
|--------|-------------------|-----------|
| `CTkButton` | `Button` | `corner_radius`, `hover_color`, `image`, `compound` |
| `CTkLabel` | `Label` | `corner_radius`, `text_color`, `anchor` |
| `CTkEntry` | `Entry` | `placeholder_text`, `placeholder_text_color` |
| `CTkTextbox` | `Text` | `activate_scrollbars`, `wrap`, `font` |
| `CTkFrame` | `Frame` | `corner_radius`, `fg_color`, `border_width` |
| `CTkSlider` | `Scale` | `from_`, `to`, `number_of_steps` |
| `CTkProgressBar` | `Progressbar` | `mode="determinate"`, `.set(0.5)` |
| `CTkSwitch` | `Checkbutton` | `onvalue`, `offvalue`, `switch_width` |
| `CTkCheckBox` | `Checkbutton` | `corner_radius`, `checkbox_width` |
| `CTkRadioButton` | `Radiobutton` | `radiobutton_width` |
| `CTkOptionMenu` | `OptionMenu` | `values=["a","b"]`, `command=` |
| `CTkComboBox` | `Combobox` | `values=[]`, `command=` |
| `CTkSegmentedButton` | — | `values=["Tab1","Tab2"]`, `command=` |
| `CTkTabview` | `Notebook` | `.add("Tab Name")`, `.set("Tab Name")` |
| `CTkScrollableFrame` | — | `orientation="vertical"`, auto scrollbar |
| `CTkInputDialog` | `simpledialog` | `text=`, `title=` |
| `CTkToplevel` | `Toplevel` | Same as CTk but for secondary windows |

### Button with Icon
```python
from PIL import Image
icon = ctk.CTkImage(
    light_image=Image.open("icon_light.png"),
    dark_image=Image.open("icon_dark.png"),
    size=(20, 20),
)
btn = ctk.CTkButton(parent, text="Send", image=icon, compound="left")
```

### Segmented Button (tab-like selector)
```python
seg = ctk.CTkSegmentedButton(
    parent,
    values=["Chat", "Tools", "Settings"],
    command=on_tab_change,
)
seg.set("Chat")  # select default
```

---

## Appearance & Themes

### Built-in Color Themes
```python
ctk.set_default_color_theme("blue")      # Default
ctk.set_default_color_theme("green")     # Green accent
ctk.set_default_color_theme("dark-blue") # Deeper blue
```

### Custom Theme (JSON)
```json
{
  "CTk": {
    "fg_color": ["#f0f0f0", "#1a1a2e"]
  },
  "CTkButton": {
    "fg_color": ["#3a7ebf", "#1f6aa5"],
    "hover_color": ["#325882", "#14375e"],
    "text_color": ["#ffffff", "#ffffff"],
    "corner_radius": 8
  },
  "CTkEntry": {
    "fg_color": ["#f9f9fa", "#343638"],
    "border_color": ["#979da2", "#565b5e"],
    "placeholder_text_color": ["#a0a0a0", "#6c6c6c"]
  }
}
```

Load: `ctk.set_default_color_theme("path/to/theme.json")`

### Per-Widget Color Override
```python
btn = ctk.CTkButton(
    parent,
    text="Danger",
    fg_color="#f7768e",      # background
    hover_color="#d63d5e",   # hover background
    text_color="#ffffff",
    corner_radius=6,
)
```

### Dynamic Appearance Toggle
```python
def toggle_theme():
    current = ctk.get_appearance_mode()
    new_mode = "light" if current == "Dark" else "dark"
    ctk.set_appearance_mode(new_mode)
```

---

## Layout Patterns

### Grid Layout (recommended)
```python
app = ctk.CTk()
app.grid_columnconfigure(0, weight=1)
app.grid_rowconfigure(1, weight=1)

# Top bar
top = ctk.CTkFrame(app)
top.grid(row=0, column=0, sticky="ew", padx=10, pady=(10, 0))

# Main content
content = ctk.CTkFrame(app)
content.grid(row=1, column=0, sticky="nsew", padx=10, pady=10)

# Bottom bar
bottom = ctk.CTkFrame(app, height=40)
bottom.grid(row=2, column=0, sticky="ew", padx=10, pady=(0, 10))
```

### Sidebar + Content
```python
app.grid_columnconfigure(1, weight=1)
app.grid_rowconfigure(0, weight=1)

sidebar = ctk.CTkFrame(app, width=200, corner_radius=0)
sidebar.grid(row=0, column=0, sticky="nsw")
sidebar.grid_propagate(False)  # fixed width

content = ctk.CTkFrame(app)
content.grid(row=0, column=1, sticky="nsew", padx=10, pady=10)
```

### Tabview
```python
tabview = ctk.CTkTabview(app)
tabview.grid(row=0, column=0, sticky="nsew", padx=10, pady=10)

chat_tab = tabview.add("Chat")
tools_tab = tabview.add("Tools")
settings_tab = tabview.add("Settings")

# Add widgets to each tab
ctk.CTkLabel(chat_tab, text="Chat goes here").pack()
```

---

## Chat App Pattern

```python
import customtkinter as ctk
import threading, queue

class ChatApp(ctk.CTk):
    def __init__(self):
        super().__init__()
        self.title("Charlie Chat")
        self.geometry("800x600")
        ctk.set_appearance_mode("dark")

        self.msg_queue = queue.Queue()
        self.is_waiting = False

        # Layout
        self.grid_columnconfigure(0, weight=1)
        self.grid_rowconfigure(0, weight=1)

        # Chat display
        self.chat = ctk.CTkTextbox(self, wrap="word", state="disabled",
                                    font=("JetBrains Mono", 12))
        self.chat.grid(row=0, column=0, sticky="nsew", padx=10, pady=(10, 0))
        self.chat.tag_config("user", foreground="#7aa2f7")
        self.chat.tag_config("assistant", foreground="#c0caf5")

        # Input bar
        input_frame = ctk.CTkFrame(self)
        input_frame.grid(row=1, column=0, sticky="ew", padx=10, pady=10)
        input_frame.grid_columnconfigure(0, weight=1)

        self.entry = ctk.CTkEntry(input_frame, placeholder_text="Type a message...",
                                   font=("", 13))
        self.entry.grid(row=0, column=0, sticky="ew", padx=(0, 5))
        self.entry.bind("<Return>", self.on_send)

        self.send_btn = ctk.CTkButton(input_frame, text="Send", width=80,
                                       command=self.on_send)
        self.send_btn.grid(row=0, column=1)

        # Poll queue
        self.after(100, self.poll_queue)

    def append_msg(self, role, text):
        self.chat.configure(state="normal")
        self.chat.insert("end", f"{role}> ", role)
        self.chat.insert("end", f"{text}\n\n")
        self.chat.see("end")
        self.chat.configure(state="disabled")

    def on_send(self, event=None):
        if self.is_waiting:
            return
        text = self.entry.get().strip()
        if not text:
            return
        self.entry.delete(0, "end")
        self.append_msg("you", text)
        self.is_waiting = True
        self.send_btn.configure(state="disabled")
        threading.Thread(target=self._api_call, args=(text,), daemon=True).start()

    def _api_call(self, prompt):
        try:
            result = call_nim(prompt)  # blocking OK in thread
            self.msg_queue.put(("response", result))
        except Exception as e:
            self.msg_queue.put(("error", str(e)))

    def poll_queue(self):
        while not self.msg_queue.empty():
            kind, data = self.msg_queue.get_nowait()
            if kind == "response":
                self.append_msg("ai", data)
            elif kind == "error":
                self.append_msg("error", data)
            self.is_waiting = False
            self.send_btn.configure(state="normal")
        self.after(100, self.poll_queue)

if __name__ == "__main__":
    ChatApp().mainloop()
```
