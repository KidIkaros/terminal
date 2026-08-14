# Configuration Reference

This document describes all configuration options available in `terminal.toml`.

## Configuration File Locations

Configuration is loaded from the following locations (in order of priority):

1. `./terminal.toml` - Current directory
2. `~/.config/terminal/config.toml` - XDG config directory
3. Built-in defaults

## Font Configuration

```toml
[font]
family = "JetBrains Mono"  # Font family name (resolved via fontconfig)
size = 15.0                # Font size in pixels
path = ""                  # Path to font file (optional, overrides family)
bold_path = ""             # Path to bold font file (optional)
italic_path = ""           # Path to italic font file (optional)
ligatures = true           # Enable ligature rendering
```

### Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `family` | string | `"JetBrains Mono"` | Font family name, looked up via `fc-match`; embedded fallback |
| `size` | float | `15.0` | Font size in pixels |
| `path` | string | `""` | Path to custom font file (TTF/OTF), overrides `family` |
| `bold_path` | string | `""` | Path to bold variant font (otherwise synthesized) |
| `italic_path` | string | `""` | Path to italic variant font (otherwise regular) |
| `ligatures` | bool | `true` | Enable ligature rendering |

## Window Configuration

```toml
[window]
title = "Terminal"        # Window title
cols = 80                 # Default columns
rows = 24                 # Default rows
padding = 0               # Window padding in pixels
opacity = 1.0             # Window opacity (0.0 - 1.0)
decorations = true        # Show window decorations
always_on_top = false     # Keep window on top
vsync = true              # Vertical sync
blur = false              # Background blur (Wayland/macOS; no-op on X11)
```

### Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `title` | string | `"Terminal"` | Window title |
| `cols` | u16 | `80` | Default terminal width in columns |
| `rows` | u16 | `24` | Default terminal height in rows |
| `padding` | u16 | `0` | Window padding in pixels |
| `opacity` | float | `1.0` | Window opacity (0.0 = transparent, 1.0 = opaque) |
| `decorations` | bool | `true` | Show window title bar and borders |
| `always_on_top` | bool | `false` | Keep window above other windows |
| `vsync` | bool | `true` | Vertical sync (screen-tearing vs. latency) |
| `blur` | bool | `false` | Background blur behind translucent windows (Wayland/macOS; no-op on X11) |

## Color Configuration

```toml
[colors]
# Theme name (overrides individual colors if set)
# theme = "Catppuccin Mocha"

# Base colors
background = "#1E1E2E"    # Background color
foreground = "#CDD6F4"    # Foreground text color
cursor = "#F5E0DC"        # Cursor color
cursor_text = "#1E1E2E"   # Text color under cursor
selection_bg = "#585B70"   # Selection background
selection_fg = "#CDD6F4"   # Selection text color

# Tab bar colors
tab_bar_bg = "#181825"    # Tab bar background
active_tab = "#89B4FA"    # Active tab color
inactive_tab = "#45475A"  # Inactive tab color

# ANSI color palette (16 colors)
ansi = [
    "#45475A",  # Black (0)
    "#F38BA8",  # Red (1)
    "#A6E3A1",  # Green (2)
    "#F9E2AF",  # Yellow (3)
    "#89B4FA",  # Blue (4)
    "#F5C2E7",  # Magenta (5)
    "#94E2D5",  # Cyan (6)
    "#BAC2DE",  # White (7)
    "#585B70",  # Bright Black (8)
    "#F38BA8",  # Bright Red (9)
    "#A6E3A1",  # Bright Green (10)
    "#F9E2AF",  # Bright Yellow (11)
    "#89B4FA",  # Bright Blue (12)
    "#F5C2E7",  # Bright Magenta (13)
    "#94E2D5",  # Bright Cyan (14)
    "#A6ADC8",  # Bright White (15)
]

# Optional overrides
# bold = "#FFFFFF"        # Bold text color
# dim = "#888888"         # Dim text color
```

### Color Formats

Colors can be specified in the following formats:

- `#RRGGBB` - 6-digit hex (e.g., `#FF0000`)
- `#RRGGBBAA` - 8-digit hex with alpha (e.g., `#FF000080`)
- `rgb:R/G/B` - RGB format (e.g., `rgb:255/0/0`)

### Available Themes

| Theme | Variant |
|-------|---------|
| Catppuccin | Mocha, Latte, Frappe, Macchiato |
| Gruvbox | Dark, Light |
| Dracula | - |
| Tokyo Night | Normal, Storm |
| Nord | - |
| Solarized | Dark, Light |
| One Dark | - |
| Monokai | - |
| GitHub | Dark, Light |

## Cursor Configuration

```toml
cursor_style = "block"    # Cursor style: "block", "bar", or "underline"
cursor_blink_ms = 500     # Cursor blink interval in milliseconds (0 = no blink)
text_blink_ms = 500       # SGR text blink interval in milliseconds (0 = no blink)
```

### Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `cursor_style` | string | `"block"` | Cursor appearance |
| `cursor_blink_ms` | u64 | `500` | Cursor blink interval (0 to disable) |
| `text_blink_ms` | u64 | `500` | SGR text blink interval (0 to disable) |
| `bell` | string | `"flash"` | BEL feedback: `"flash"`, `"audible"`, or `"none"` |

### Cursor Styles

- `"block"` - Full cell block cursor
- `"bar"` - Vertical line cursor
- `"underline"` - Underline cursor

## Keyboard Configuration

```toml
[keyboard]
# Custom key bindings (future implementation)
# bindings = [
#     { key = "Ctrl+Shift+C", action = "copy" },
#     { key = "Ctrl+Shift+V", action = "paste" },
# ]
```

## Shell Configuration

```toml
# Shell to launch (defaults to $SHELL or /bin/bash)
# shell = "/bin/bash"
```

## Scrollback Configuration

```toml
# Number of lines to keep in scrollback buffer
# scrollback = 10000
```

## Mouse Configuration

```toml
# Enable mouse reporting by default
# mouse_reporting = false
```

## Security Configuration

Locked down by default: applications cannot read the clipboard or open
arbitrary URI handlers unless explicitly allowed.

```toml
[security]
osc52_write = true   # Allow applications to write to the clipboard (OSC 52)
osc52_read = false   # Allow applications to READ the clipboard (OSC 52 queries)
window_title = true  # Allow applications to change the window title (OSC 0/2)
uri_schemes = ["http", "https"]  # Schemes hyperlinks may open
```

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `osc52_write` | bool | `true` | Allow OSC 52 to set the system clipboard |
| `osc52_read` | bool | `false` | Allow OSC 52 queries to read the clipboard (disable to keep secrets safe from hostile prompts) |
| `window_title` | bool | `true` | Allow OSC 0/2 window-title changes |
| `uri_schemes` | list | `["http", "https"]` | URI schemes that Ctrl-click / OSC 8 hyperlinks may open; replaces the default outright |

## Shell Integration

- **OSC 133** — prompt/command/output markers; `Ctrl+Shift+Up/Down` jumps between prompts.
- **OSC 7** — records the working directory (`grid.cwd`).
- **OSC 9** — desktop notification requests (logged).
- **Mode 2048** — in-band resize reports (`CSI 4;rows;cols t`) so tmux/neovim redraw without polling.

## Example Configurations

### Minimal Configuration

```toml
[font]
size = 14.0

[window]
cols = 120
rows = 40
```

### Catppuccin Mocha Theme

```toml
[colors]
background = "#1E1E2E"
foreground = "#CDD6F4"
cursor = "#F5E0DC"
```

Or simply:

```toml
[colors]
theme = "Catppuccin Mocha"
```

### Performance Tuning

```toml
[font]
ligatures = false  # Disable ligatures for better performance

cursor_blink_ms = 0  # Disable cursor blink
```
