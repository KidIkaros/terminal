# Terminal

A fast, GPU-accelerated terminal emulator written in Rust.

## Features

- **GPU-Accelerated Rendering** - Uses wgpu for hardware-accelerated text rendering
- **VT100 Compatible** - Full state machine implementation based on Paul Williams' parser
- **True Color Support** - 24-bit color with 256-color fallback
- **Configurable** - TOML-based configuration with theme support
- **Search** - Built-in search with regex support
- **Clipboard Integration** - System clipboard and OSC52 protocol
- **Mouse Support** - X10, SGR, and urxvt mouse tracking (modes 1000/1002/1003/1006/1015)
- **Bracketed Paste** - Safe paste handling
- **Customizable Themes** - 16 built-in themes (Catppuccin, Gruvbox, Dracula, etc.)
- **Tabs** - Multiple terminal sessions with a tab bar; background tabs pause their PTY reader so runaway output blocks under kernel backpressure (no unbounded memory growth), and switching is instant
- **Sixel Inline Images** - Decode and render sixel (DEC 54870) graphics: `cat image.six`, chafa, img2sixel; images track their rows on scroll and are removed on clear/resize/alt-screen. Cross-validated against chafa (real encoder) and an independent Python encoder
- **Shell Integration** - OSC 133 prompt/command markers with Ctrl+Shift+Up/Down prompt jumping, OSC 7 cwd tracking
- **In-Band Resize** - Mode 2048 resize notifications (`CSI 4;h;w t`) for tmux/neovim
- **Rectangular Selection** - Alt+Click block selection (VS Code/kitty style)
- **Notifications** - OSC 9 desktop notifications via notify-send (falls back to logging without a notification daemon)
- **Locked Down by Default** - OSC 52 clipboard reads are off by default; URI-scheme allowlist for hyperlinks
- **Keyboard Protocols** - Full kitty keyboard protocol (`CSI u`) with progressive-enhancement flags, push/pop/query, key repeat/release events, alternate keys, and associated text; plus xterm modifyOtherKeys
- **Kitty Graphics Protocol** - Inline images via `ESC _ G` (`chafa --format=kitty`, `timg`): raw RGB (`f=24`), RGBA (`f=32`) and PNG (`f=100`) with chunked transfers, plus image-id round-trips (`a=t`/`a=p`/`a=d`/`a=q`) for caching tools like ranger and image.nvim
- **Inline Video** - Opt-in (`--features video`) playback of a video file directly in the terminal via `terminal --video clip.mp4`: asciline's decoder runs on a background thread and frames render through the kitty-graphics path (requires ffmpeg on PATH)
- **Modern Look & Feel** - Configurable padding and window opacity, double-click word / triple-click line selection, SIGHUP config hot-reload, and smooth scrollback animation
- **Parser Fuzz Harness** - Deterministic seeded fuzzing of the parser/grid seam

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

# Optional: inline video playback (needs ffmpeg/ffprobe on PATH)
cargo build --release --features video
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
text_blink_ms = 500
bell = "flash"
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
| Alt+Click+drag | Rectangular (block) selection |
| Shift+Click+drag | Line selection |
| F3 / Ctrl+G | Find next |
| Ctrl+Shift+G | Find previous |
| Ctrl+Shift+T | New tab |
| Ctrl+Shift+W | Close tab |
| Ctrl+PageUp/PageDown | Switch tabs |
| Ctrl+Shift+Up/Down | Jump between shell prompts (OSC 133) |
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

## Performance

Headless parser+grid throughput (CPU only, no GPU; 80×24 grid, 500 lines
scrollback, release build, bulk-output mode — the `bench` binary):

| Workload | Throughput |
|----------|------------|
| Printable ASCII (scroll path) | ~30 MiB/s |
| Random bytes (parser worst case) | ~12 MiB/s |

Run it yourself: `cargo run --release --bin bench`.

## Verification

- `cargo test --locked` — 314 tests (parser, grid, selection, sixel, fuzz smoke)
- `cargo run --release --bin vt_conformance` — 34 headless VT conformance cases
- `cargo run --release --bin fuzz -- --quick` — deterministic parser fuzzing

## License

MIT
