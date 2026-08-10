# Terminal

A fast, GPU-accelerated terminal emulator written in Rust.

## Features

- **GPU-Accelerated Rendering** - Uses wgpu for hardware-accelerated text rendering
- **VT100 Compatible** - Full state machine implementation based on Paul Williams' parser
- **True Color Support** - 24-bit color with 256-color fallback
- **Configurable** - TOML-based configuration with theme support
- **Search** - Built-in search with regex support
- **Clipboard Integration** - System clipboard and OSC52 protocol
- **Mouse Support** - X10 and SGR mouse tracking
- **Bracketed Paste** - Safe paste handling
- **Customizable Themes** - 16 built-in themes (Catppuccin, Gruvbox, Dracula, etc.)

## Installation

### From Source

```bash
# Install from crates.io (when published)
cargo install terminal

# Or install from git
cargo install --git https://github.com/user/terminal
```

### Build Locally

```bash
git clone https://github.com/user/terminal
cd terminal
cargo build --release
```

The binary will be at `target/release/terminal`.

## Usage

```bash
# Run with default settings
terminal

# Run with a specific config file
terminal --config /path/to/config.toml

# Run a specific shell
terminal --shell /bin/zsh
```

## Configuration

Configuration is loaded from (in order of priority):

1. `./terminal.toml` (current directory)
2. `~/.config/terminal/config.toml` (XDG config)
3. Built-in defaults

### Example Configuration

```toml
[font]
family = "JetBrains Mono"
size = 15.0
ligatures = true

[window]
title = "Terminal"
cols = 80
rows = 24
opacity = 1.0

[colors]
background = "#1E1E2E"
foreground = "#CDD6F4"
cursor = "#F5E0DC"

# Or use a built-in theme
# theme = "Catppuccin Mocha"

cursor_style = "block"
cursor_blink_ms = 500
```

### Available Themes

- Catppuccin (Mocha, Latte, Frappe, Macchiato)
- Gruvbox (Dark, Light)
- Dracula
- Tokyo Night (Normal, Storm)
- Nord
- Solarized (Dark, Light)
- One Dark
- Monokai
- GitHub (Dark, Light)

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| Ctrl+Shift+C | Copy selection |
| Ctrl+Shift+V | Paste |
| Ctrl+Shift+F | Open search |
| Ctrl+Shift+A | Select all |
| F3 / Ctrl+G | Find next |
| Ctrl+Shift+G | Find previous |
| Ctrl+Shift+T | New tab |
| Ctrl+Shift+W | Close tab |
| Ctrl+PageUp/PageDown | Switch tabs |
| Shift+PageUp/PageDown | Scroll buffer |

## Architecture

The terminal is built with a 4-layer architecture:

1. **PTY Layer** - Pseudoterminal management and shell process handling
2. **Parser Layer** - VT100/VT220 state machine (Paul Williams' design)
3. **Grid Layer** - 2D cell array with cursor, SGR attributes, and scrollback
4. **Renderer Layer** - GPU-accelerated text rendering with wgpu

## Requirements

- Rust 1.70+
- GPU with Vulkan/Metal/DX12 support
- Linux, macOS, or Windows

## License

MIT
