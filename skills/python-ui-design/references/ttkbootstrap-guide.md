# ttkbootstrap Reference

Modern theming for tkinter. 13 built-in themes, Bootstrap-inspired widget styles, zero-config dark mode.

## Table of Contents

1. [Setup & Themes](#setup--themes)
2. [Widget Styles](#widget-styles)
3. [Special Widgets](#special-widgets)
4. [Custom Themes](#custom-themes)
5. [Layout Patterns](#layout-patterns)

---

## Setup & Themes

```python
import ttkbootstrap as ttk
from ttkbootstrap.constants import *

# Window with theme
root = ttk.Window(title="My App", themename="darkly", size=(800, 600))

# Change theme at runtime
root.style.theme_use("cosmo")
```

### Theme Gallery

| Theme | Background | Accent | Vibe |
|-------|-----------|--------|------|
| `cosmo` | White | Blue | Clean corporate |
| `flatly` | White | Green | Friendly, modern |
| `darkly` | Dark gray | Cyan | Developer tools |
| `solar` | Dark blue-gray | Yellow | Solarized |
| `superhero` | Dark navy | Orange | Bold dashboard |
| `cyborg` | Black | Cyan-blue | Cyberpunk |
| `vapor` | Dark purple | Pink/cyan | Neon/retro |
| `minty` | Light mint | Green | Fresh, light |
| `journal` | White | Red | Editorial |
| `litera` | White | Blue | Documentation |
| `lumen` | White | Blue | Bright, airy |
| `pulse` | White | Purple | Modern, vibrant |
| `sandstone` | Tan | Green | Earthy, warm |

---

## Widget Styles

Every ttk widget accepts a `bootstyle` parameter:

```python
# Color variants
ttk.Button(parent, text="Save", bootstyle="success")
ttk.Button(parent, text="Delete", bootstyle="danger")
ttk.Button(parent, text="Info", bootstyle="info")
ttk.Button(parent, text="Warn", bootstyle="warning")

# Style modifiers (combine with -)
ttk.Button(parent, text="Outline", bootstyle="success-outline")
ttk.Button(parent, text="Link", bootstyle="primary-link")
ttk.Button(parent, text="Solid round", bootstyle="info-round")

# Other widgets
ttk.Label(parent, text="Error!", bootstyle="danger")
ttk.Entry(parent, bootstyle="success")
ttk.Progressbar(parent, bootstyle="success-striped", value=67)
ttk.Checkbutton(parent, text="Accept", bootstyle="round-toggle")
```

### Style Keywords

| Keyword | Effect | Widgets |
|---------|--------|---------|
| `primary` | Theme primary color | All |
| `secondary` | Muted color | All |
| `success` | Green | All |
| `info` | Cyan/blue | All |
| `warning` | Yellow/orange | All |
| `danger` | Red | All |
| `light` / `dark` | Light/dark variant | All |
| `outline` | Border only, no fill | Button, Label |
| `link` | Underline, no border | Button |
| `round-toggle` | iOS-style toggle | Checkbutton |
| `square-toggle` | Square toggle | Checkbutton |
| `striped` | Striped animation | Progressbar |

---

## Special Widgets

### Meter (circular gauge)
```python
meter = ttk.Meter(
    parent,
    metersize=180,
    amountused=67,
    amounttotal=100,
    metertype="semi",          # "full" or "semi"
    subtext="CPU Usage",
    bootstyle="success",
    interactive=True,           # drag to change value
)
```

### Floodgauge (animated progress)
```python
gauge = ttk.Floodgauge(
    parent,
    bootstyle="info",
    mask="Loading... {}%",      # {} replaced with value
    maximum=100,
    value=45,
    font=("Helvetica", 14),
)
gauge.start()  # start animation
```

### DateEntry (date picker)
```python
date = ttk.DateEntry(
    parent,
    bootstyle="primary",
    dateformat="%Y-%m-%d",
    firstweekday=0,            # Monday
)
selected = date.entry.get()    # "2026-03-20"
```

### Scrolled widgets
```python
# ScrolledFrame — scrollable container
sf = ttk.ScrolledFrame(parent, autohide=True)
for i in range(100):
    ttk.Label(sf, text=f"Row {i}").pack()

# ScrolledText
st = ttk.ScrolledText(parent, height=10, autohide=True)
```

### Toast notifications
```python
from ttkbootstrap.toast import ToastNotification

toast = ToastNotification(
    title="Saved",
    message="Session saved successfully",
    duration=3000,              # ms
    bootstyle="success",
    position=(50, 50, "ne"),    # x, y, anchor
)
toast.show_toast()
```

---

## Custom Themes

```python
from ttkbootstrap import Style

style = Style()
style.theme_create(
    name="charlie-dark",
    parent="darkly",
    settings={
        "TButton": {
            "configure": {
                "font": ("JetBrains Mono", 10),
                "padding": (12, 6),
            }
        },
        "TLabel": {
            "configure": {
                "font": ("Inter", 11),
            }
        }
    }
)
style.theme_use("charlie-dark")
```

### Theme Creator GUI
```bash
python -m ttkbootstrap
```
Opens interactive theme builder — pick colors, preview widgets, export.

---

## Layout Patterns

### Dashboard Layout
```python
root = ttk.Window(themename="superhero", size=(1024, 768))

# Top bar
top = ttk.Frame(root)
top.pack(fill=X, padx=5, pady=5)
ttk.Label(top, text="Dashboard", font=("", 16, "bold")).pack(side=LEFT)

# Two-column content
content = ttk.Frame(root)
content.pack(fill=BOTH, expand=True, padx=5)

left = ttk.LabelFrame(content, text="Status", bootstyle="info")
left.pack(side=LEFT, fill=BOTH, expand=True, padx=(0, 5))

right = ttk.LabelFrame(content, text="Actions", bootstyle="warning")
right.pack(side=RIGHT, fill=BOTH, expand=True)

# Bottom status bar
status = ttk.Label(root, text="Ready", bootstyle="inverse-secondary")
status.pack(fill=X, side=BOTTOM)
```

### Chat Layout
```python
root = ttk.Window(themename="darkly", size=(800, 600))

# Messages area
msgs = ttk.ScrolledText(root, autohide=True, state=DISABLED, height=20)
msgs.pack(fill=BOTH, expand=True, padx=10, pady=(10, 0))

# Input bar
input_frame = ttk.Frame(root)
input_frame.pack(fill=X, padx=10, pady=10)

entry = ttk.Entry(input_frame, bootstyle="info", font=("", 12))
entry.pack(side=LEFT, fill=X, expand=True, padx=(0, 5))

send_btn = ttk.Button(input_frame, text="Send", bootstyle="info", width=8)
send_btn.pack(side=RIGHT)
```
