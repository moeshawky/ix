# tkinter / ttk Core Patterns

Foundation patterns for Python's built-in GUI toolkit. Everything here applies to ttkbootstrap and CustomTkinter too.

## Table of Contents

1. [Layout Managers](#layout-managers)
2. [Event Binding](#event-binding)
3. [ttk Styling](#ttk-styling)
4. [Megawidgets](#megawidgets)
5. [Common Gotchas](#common-gotchas)

---

## Layout Managers

### pack — Simple stacking
```python
# Vertical stack (default)
label.pack(fill=X, padx=10, pady=5)
entry.pack(fill=X, padx=10)
button.pack(pady=10)

# Horizontal row
frame = ttk.Frame(parent)
btn1.pack(side=LEFT, padx=5)
btn2.pack(side=LEFT, padx=5)
btn3.pack(side=RIGHT)  # right-aligned
```

**When to use:** Simple layouts, toolbars, status bars, stacked forms.

### grid — Precise placement
```python
# Two-column form
ttk.Label(parent, text="Name:").grid(row=0, column=0, sticky=W, padx=5, pady=2)
ttk.Entry(parent).grid(row=0, column=1, sticky=EW, padx=5, pady=2)

ttk.Label(parent, text="Email:").grid(row=1, column=0, sticky=W, padx=5, pady=2)
ttk.Entry(parent).grid(row=1, column=1, sticky=EW, padx=5, pady=2)

# Make column 1 expand
parent.grid_columnconfigure(1, weight=1)
```

**When to use:** Forms, dashboards, any layout needing row/column alignment.

### place — Absolute positioning (avoid)
```python
# Only use for overlays/popups
popup.place(relx=0.5, rely=0.5, anchor=CENTER)
```

**When to use:** Almost never. Overlays, floating popups only.

### Golden Rule
**NEVER mix pack and grid in the same parent container.** They use different geometry algorithms and will conflict.

---

## Event Binding

### Key Bindings
```python
# Global (any widget focused)
root.bind("<Control-q>", lambda e: root.quit())
root.bind("<F1>", show_help)
root.bind("<Escape>", on_escape)

# Widget-specific
entry.bind("<Return>", on_submit)
entry.bind("<Control-a>", select_all)
text_widget.bind("<Control-v>", on_paste)

# Key event info
def on_key(event):
    print(event.keysym)   # "Return", "a", "Tab"
    print(event.keycode)  # numeric keycode
    print(event.state)    # modifier bitmask
    print(event.char)     # character or ""
```

### Mouse Bindings
```python
widget.bind("<Button-1>", on_click)        # left click
widget.bind("<Button-3>", on_right_click)  # right click
widget.bind("<Double-Button-1>", on_dbl)   # double click
widget.bind("<Enter>", on_hover_enter)     # mouse enters widget
widget.bind("<Leave>", on_hover_leave)     # mouse leaves
widget.bind("<MouseWheel>", on_scroll)     # scroll wheel
```

### Virtual Events
```python
# Combobox selection
combo.bind("<<ComboboxSelected>>", on_select)

# Treeview selection
tree.bind("<<TreeviewSelect>>", on_tree_select)

# Custom events
widget.event_generate("<<MyCustomEvent>>")
widget.bind("<<MyCustomEvent>>", handler)
```

### Protocol Handlers
```python
# Window close button (X)
root.protocol("WM_DELETE_WINDOW", on_closing)

def on_closing():
    if messagebox.askokcancel("Quit", "Save session before quitting?"):
        save_session()
    root.destroy()
```

---

## ttk Styling

### Style Object
```python
style = ttk.Style()

# Configure existing style
style.configure("TButton", font=("Helvetica", 11), padding=6)
style.configure("TLabel", font=("Helvetica", 10))

# Create custom named style
style.configure("Chat.TFrame", background="#1a1b26")
style.configure("User.TLabel", foreground="#7aa2f7", font=("", 10, "bold"))
style.configure("Assistant.TLabel", foreground="#c0caf5")

# Use named style
frame = ttk.Frame(parent, style="Chat.TFrame")
label = ttk.Label(parent, text="Hello", style="User.TLabel")
```

### State-Based Styling
```python
style.map("TButton",
    foreground=[("disabled", "gray"), ("active", "white")],
    background=[("disabled", "#333"), ("active", "#555"), ("pressed", "#222")],
)
```

### Theme Inspection
```python
style = ttk.Style()
print(style.theme_names())      # available themes
print(style.theme_use())        # current theme
style.theme_use("clam")         # switch theme

# Inspect widget options
print(style.layout("TButton"))  # widget layout elements
print(style.configure("TButton"))  # current configuration
```

---

## Megawidgets

### ScrolledText (built-in)
```python
from tkinter import scrolledtext

text = scrolledtext.ScrolledText(parent, wrap=tk.WORD, height=20)
text.pack(fill=BOTH, expand=True)

# Read-only display
text.config(state=DISABLED)

# Programmatic insert (must enable, insert, disable)
text.config(state=NORMAL)
text.insert(END, "New message\n")
text.see(END)  # scroll to bottom
text.config(state=DISABLED)
```

### Treeview (table/tree)
```python
tree = ttk.Treeview(parent, columns=("name", "status", "model"), show="headings")
tree.heading("name", text="Service", command=lambda: sort_column("name"))
tree.heading("status", text="Status")
tree.heading("model", text="Model")

tree.column("name", width=150)
tree.column("status", width=80)
tree.column("model", width=200)

# Add rows
tree.insert("", END, values=("brain_llm", "Active", "nemotron-super"))
tree.insert("", END, values=("chromadb", "Offline", ""))

# Selection
tree.bind("<<TreeviewSelect>>", lambda e: on_select(tree.selection()))
```

### PanedWindow (resizable split)
```python
paned = ttk.PanedWindow(parent, orient=HORIZONTAL)
paned.pack(fill=BOTH, expand=True)

left = ttk.Frame(paned, width=200)
right = ttk.Frame(paned)

paned.add(left, weight=1)
paned.add(right, weight=3)
```

### Notebook (tabs)
```python
notebook = ttk.Notebook(parent)
notebook.pack(fill=BOTH, expand=True)

chat_frame = ttk.Frame(notebook)
tools_frame = ttk.Frame(notebook)

notebook.add(chat_frame, text="Chat")
notebook.add(tools_frame, text="Tools")

notebook.bind("<<NotebookTabChanged>>", on_tab_change)
```

---

## Common Gotchas

| Gotcha | Symptom | Fix |
|--------|---------|-----|
| Widgets from thread | `RuntimeError` or silent corruption | Queue + `root.after()` |
| `pack` + `grid` in same parent | Infinite loop, frozen UI | Choose one per container |
| Forgetting `weight` on grid cols/rows | Widgets don't resize | `columnconfigure(0, weight=1)` |
| `StringVar` garbage collected | Widget shows empty | Store as `self.var = StringVar()` |
| `Entry.get()` returns string always | Type errors with numbers | `int(entry.get())` with try/except |
| `Text` widget insert when DISABLED | Nothing happens, no error | Set NORMAL → insert → set DISABLED |
| Window close doesn't save state | Data loss | `protocol("WM_DELETE_WINDOW", handler)` |
| Font not found | Falls back to system default | Bundle TTF or use platform fonts |
| macOS menu bar | Menu doesn't appear in menu bar | Use `root.createcommand("::tk::mac::Quit", ...)` |
| HiDPI blurry on Windows | Widgets render at 1x then scale | `ctypes.windll.shcore.SetProcessDpiAwareness(2)` |
