# Terminal Emulator Roadmap

**Project:** GPU-accelerated terminal emulator in Rust
**Created:** 2026-08-10
**Current State:** MVP with 71 tests, core functionality working

---

## Vision

Build a fast, beautiful, and fully-featured terminal emulator that can be used as a daily driver. Think: the rendering speed of Alacritty, the features of Kitty, and the simplicity of a well-architected Rust codebase.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────┐
│                   main.rs                        │
│  (winit event loop, keyboard input, app state)  │
├─────────────┬─────────────┬─────────────────────┤
│   parser/   │    grid/    │      render/        │
│   VT state  │   Cell      │   wgpu pipeline     │
│   machine   │   buffer    │   font atlas        │
│             │   cursor    │   WGSL shader       │
├─────────────┴─────────────┴─────────────────────┤
│                    pty/                           │
│  (fork/exec shell, reader thread, resize ioctl)  │
└─────────────────────────────────────────────────┘
```

---

## Phase 1: Essential Features (Current Sprint)

**Goal:** Make the terminal usable for daily tasks

### 1.1 Clipboard Support
**Priority:** HIGH — Can't copy/paste without this
- [ ] Ctrl+Shift+C → copy selection to system clipboard
- [ ] Ctrl+Shift+V → paste from system clipboard
- [ ] OSC 52 support (apps can set clipboard via escape sequence)
- [ ] Text selection with mouse (click + drag)
- [ ] Selection highlighting in renderer

**Files:** `main.rs`, `grid/mod.rs`, `render/pipeline.rs`
**Effort:** 2-3 days

### 1.2 Mouse Tracking
**Priority:** HIGH — tmux, vim, fzf won't work without this
- [ ] DECSET 1000 (normal mouse tracking)
- [ ] DECSET 1002 (button-event tracking)
- [ ] DECSET 1003 (any-event tracking)
- [ ] DECSET 1006 (SGR extended coordinates)
- [ ] Encode mouse events as CSI sequences
- [ ] Handle mouse wheel for scrollback

**Files:** `main.rs`, `grid/mod.rs`
**Effort:** 1-2 days

### 1.3 Keyboard Shortcuts
**Priority:** HIGH — Essential for shell interaction
- [ ] Ctrl+C → SIGINT (0x03)
- [ ] Ctrl+Z → SIGTSTP (0x1a)
- [ ] Ctrl+D → EOF (0x04)
- [ ] Ctrl+L → clear screen + redraw
- [ ] Ctrl+W → delete word backward
- [ ] Ctrl+U → delete to beginning of line
- [ ] Ctrl+K → delete to end of line
- [ ] Ctrl+A → beginning of line
- [ ] Ctrl+E → end of line
- [ ] Ctrl+R → reverse search (future)

**Files:** `main.rs`
**Effort:** 0.5 days

---

## Phase 2: Polish & Compatibility

**Goal:** Handle edge cases and improve compatibility

### 2.1 OSC Handling
- [ ] OSC 0/2 → set window title (currently logged but not applied)
- [ ] OSC 4 → set/query color palette
- [ ] OSC 10/11 → set/query fg/bg color
- [ ] OSC 52 → clipboard (from Phase 1)
- [ ] OSC 112 → reset cursor color

**Files:** `grid/mod.rs`
**Effort:** 1 day

### 2.2 Bell & Visual Feedback
- [ ] BEL (0x07) → visual bell (flash screen or border)
- [ ] Optional: BEL → system beep (configurable)
- [ ] Cursor blink (configurable interval)

**Files:** `grid/mod.rs`, `render/pipeline.rs`
**Effort:** 1 day

### 2.3 Bracketed Paste Mode
- [ ] DECSET 2004 → bracketed paste
- [ ] When pasting, wrap text in `\x1b[200~... \x1b[201~`
- [ ] Prevents pasted text from being interpreted as commands

**Files:** `main.rs`, `grid/mod.rs`
**Effort:** 0.5 days

### 2.4 Scrollback Navigation
- [ ] Scroll up/down with Shift+PageUp/PageDown
- [ ] Scrollback offset in grid
- [ ] Render scrollback buffer when scrolled
- [ ] Return to normal on any key press

**Files:** `grid/mod.rs`, `render/pipeline.rs`
**Effort:** 1 day

### 2.5 Hyperlinks
- [ ] OSC 8 → set hyperlink (URI + ID)
- [ ] Render underlined + colored text for hyperlinks
- [ ] Click to open in browser (via xdg-open)

**Files:** `grid/mod.rs`, `render/pipeline.rs`, `main.rs`
**Effort:** 1-2 days

---

## Phase 3: Configuration & Customization

**Goal:** Make the terminal configurable without recompiling

### 3.1 Configuration File
**Format:** TOML or YAML

```toml
[window]
title = "Terminal"
opacity = 0.95

[font]
family = "JetBrains Mono"
size = 15.0
bold_italic = true

[colors]
background = "#1E1E2E"
foreground = "#CDD6F4"
cursor = "#89B4FA"
selection_bg = "#45475A"
selection_fg = "#CDD6F4"

# 256-color palette
palette = [
    "#1E1E2E", "#F38BA8", "#A6E3A1", "#F9E2AF",
    "#89B4FA", "#F5C2E7", "#94E2D5", "#CDD6F4",
    # ... bright variants
]

[keyboard]
shell = "/bin/bash"
# Custom key bindings
[keyboard.bindings]
"Ctrl+Shift+C" = "copy"
"Ctrl+Shift+V" = "paste"
"Ctrl+Shift+F" = "find"
```

**Files:** New `config.rs` module
**Effort:** 2 days

### 3.2 Theme System
- [ ] Built-in themes (Catppuccin, Gruvbox, Dracula, Tokyo Night, etc.)
- [ ] Load from config file
- [ ] Dynamic theme switching (future)

**Files:** New `theme.rs` module
**Effort:** 1 day

### 3.3 Font Configuration
- [ ] Load custom font from file
- [ ] Font fallback chain (for CJK, emoji)
- [ ] Bold/italic font variants
- [ ] Ligature support (optional)

**Files:** `render/font.rs`
**Effort:** 2 days

---

## Phase 4: Advanced Features

**Goal:** Power user features

### 4.1 Tabs / Splits
- [ ] Tab bar with close buttons
- [ ] Split panes (horizontal/vertical)
- [ ] Ctrl+Shift+T → new tab
- [ ] Ctrl+Shift+W → close tab
- [ ] Ctrl+Shift+Arrow → switch tabs

**Files:** New `tabs.rs`, `splits.rs`
**Effort:** 5-7 days

### 4.2 Search
- [ ] Ctrl+Shift+F → open search bar
- [ ] Regex search support
- [ ] Highlight matches in grid
- [ ] Jump to next/previous match

**Files:** New `search.rs`, `grid/mod.rs`
**Effort:** 2-3 days

### 4.3 Image Rendering
- [ ] Kitty graphics protocol (ESC G)
- [ ] Sixel graphics (ESC ? P q)
- [ ] Inline images in terminal output

**Files:** New `image.rs`, `render/`
**Effort:** 3-5 days

### 4.4 Ligatures & Shaping
- [ ] HarfBuzz integration for complex text shaping
- [ ] Ligature rendering (=>, !=, ->, etc.)
- [ ] RTL text support (future)

**Files:** `render/font.rs`, `render/pipeline.rs`
**Effort:** 3-4 days

---

## Phase 5: Performance & Polish

**Goal:** Make it fast and beautiful

### 5.1 Rendering Optimizations
- [ ] Dirty cell tracking (only redraw changed cells)
- [ ] Double buffering for instance data
- [ ] GPU-driven rendering (compute shader for cell batching)
- [ ] VSync toggle

**Files:** `render/pipeline.rs`
**Effort:** 2-3 days

### 5.2 I/O Optimizations
- [ ] Use `poll()`/`epoll()` instead of sleep loop in reader thread
- [ ] Batch PTY reads (coalesce small writes)
- [ ] Zero-copy parsing (parse directly from read buffer)

**Files:** `pty/mod.rs`, `parser/mod.rs`
**Effort:** 1-2 days

### 5.3 Visual Polish
- [ ] Smooth cursor blink animation
- [ ] Smooth scrolling (animated scrollback)
- [ ] Window transparency (Wayland/X11)
- [ ] Background blur (Wayland)
- [ ] Rounded corners (optional)

**Files:** `render/pipeline.rs`, `main.rs`
**Effort:** 2-3 days

---

## Phase 6: Distribution & Documentation

**Goal:** Make it easy to install and use

### 6.1 Packaging
- [ ] Cargo install support
- [ ] AUR package (Arch Linux)
- [ ] Homebrew formula (macOS)
- [ ] Flatpak/Snap (Linux)
- [ ] Windows MSI (future)

**Files:** New `build.rs`, packaging scripts
**Effort:** 1-2 days

### 6.2 Documentation
- [ ] README.md with screenshots
- [ ] Configuration reference
- [ ] Key bindings reference
- [ ] Troubleshooting guide
- [ ] Contributing guidelines

**Files:** New `docs/` directory
**Effort:** 1-2 days

---

## Dependencies to Add

| Crate | Purpose | Phase |
|-------|---------|-------|
| `arboard` | System clipboard | 1.1 |
| `toml` / `serde` | Config file parsing | 3.1 |
| `dirs` | XDG config paths | 3.1 |
| `regex` | Search functionality | 4.2 |
| `image` | Image loading (future) | 4.3 |

---

## Testing Strategy

### Unit Tests (Current: 71)
- Parser tests: 28 ✓
- Grid tests: 28 ✓
- Key encoding tests: 14 ✓
- Render tests: 1 ✓

### Integration Tests Needed
- [ ] End-to-end PTY → parser → grid → render pipeline
- [ ] Keyboard input → PTY → output verification
- [ ] Mouse event encoding and decoding
- [ ] Configuration file loading
- [ ] Theme application

### Visual Tests
- [ ] Screenshot comparison tests
- [ ] Font rendering accuracy
- [ ] Color palette verification
- [ ] Cursor blink timing

---

## Release Milestones

| Milestone | Features | Target |
|-----------|----------|--------|
| **v0.1.0** | Core + Clipboard + Mouse + Shortcuts | 1 week |
| **v0.2.0** | Config + Themes + Scrollback | 2 weeks |
| **v0.3.0** | Tabs + Search + Hyperlinks | 1 month |
| **v0.4.0** | Images + Ligatures + Performance | 2 months |
| **v1.0.0** | Full feature set + Documentation | 3 months |

---

## Open Questions

1. **Wayland vs X11:** Which display protocol to prioritize?
2. **GPU acceleration:** Is wgpu the right choice, or should we use Vulkan/Metal directly?
3. **Tab management:** How to handle multiple terminal instances?
4. **Plugin system:** Should we support user plugins/scripts?

---

*Roadmap created from codebase analysis*
