# Key Bindings Reference

This document lists all keyboard shortcuts available in the terminal.

## Default Key Bindings

### Clipboard Operations

| Shortcut | Action | Description |
|----------|--------|-------------|
| Ctrl+Shift+C | Copy | Copy selected text to clipboard |
| Ctrl+Shift+V | Paste | Paste from clipboard |

### Search

| Shortcut | Action | Description |
|----------|--------|-------------|
| Ctrl+Shift+F | Open Search | Activate search mode |
| F3 | Find Next | Jump to next search match |
| Ctrl+G | Find Next | Jump to next search match |
| Ctrl+Shift+G | Find Previous | Jump to previous search match |
| Escape | Close Search | Exit search mode |

### Selection

| Shortcut | Action | Description |
|----------|--------|-------------|
| Ctrl+Shift+A | Select All | Select all text in the terminal |
| Click + Drag | Select | Select text by dragging |

### Tabs (Future)

| Shortcut | Action | Description |
|----------|--------|-------------|
| Ctrl+Shift+T | New Tab | Open a new terminal tab |
| Ctrl+Shift+W | Close Tab | Close the current tab |
| Ctrl+PageUp | Previous Tab | Switch to the previous tab |
| Ctrl+PageDown | Next Tab | Switch to the next tab |

### Scrolling

| Shortcut | Action | Description |
|----------|--------|-------------|
| Shift+PageUp | Scroll Up | Scroll up in the buffer |
| Shift+PageDown | Scroll Down | Scroll down in the buffer |

### Terminal Control

| Shortcut | Action | Description |
|----------|--------|-------------|
| Ctrl+C | SIGINT | Send interrupt signal (default behavior) |
| Ctrl+Z | SIGTSTP | Send suspend signal (default behavior) |
| Ctrl+D | EOF | Send end-of-file (default behavior) |
| Ctrl+L | Clear | Clear screen (default behavior) |

## Mouse Support

When mouse reporting is enabled, the terminal supports:

- **X10 Mode** - Basic mouse tracking
- **SGR Mode** - Extended mouse tracking with coordinates

### Mouse Actions

| Action | Description |
|--------|-------------|
| Left Click | Position cursor |
| Right Click | Context menu (future) |
| Middle Click | Paste (future) |
| Scroll Up | Scroll up |
| Scroll Down | Scroll Down |

## Search Mode

When search mode is active:

| Key | Action |
|-----|--------|
| Any character | Add to search query |
| Backspace | Remove last character |
| Enter | Search for next match |
| F3 / Ctrl+G | Jump to next match |
| Ctrl+Shift+G | Jump to previous match |
| Escape | Close search |

### Search Features

- **Real-time search** - Results update as you type
- **Regex support** - Use regular expressions in search
- **Case-insensitive** - Default behavior
- **Wrap-around** - Search wraps from bottom to top

## Text Selection

### Selection Modes

- **Character Mode** - Select individual characters (default)
- **Line Mode** - Select entire lines (hold Shift)

### Selection Actions

| Action | Description |
|--------|-------------|
| Click + Drag | Start selection |
| Release | End selection (auto-copy) |
| Shift+Click | Extend selection to line |

### Auto-Copy

When you release the mouse button after selecting text, the selection is automatically copied to the clipboard.

## Terminal Emulation Keys

The following keys are sent to the PTY as escape sequences:

| Key | Sequence | Description |
|-----|----------|-------------|
| Enter | `\r` | Carriage return |
| Backspace | `\x7f` | Delete |
| Tab | `\t` | Horizontal tab |
| Escape | `\x1b` | Escape |
| Space | ` ` | Space |
| Arrow Up | `\x1b[A` | Cursor up |
| Arrow Down | `\x1b[B` | Cursor down |
| Arrow Right | `\x1b[C` | Cursor right |
| Arrow Left | `\x1b[D` | Cursor left |
| Home | `\x1b[H` | Beginning of line |
| End | `\x1b[F` | End of line |
| Page Up | `\x1b[5~` | Page up |
| Page Down | `\x1b[6~` | Page down |
| Delete | `\x1b[3~` | Delete character |
| Insert | `\x1b[2~` | Insert mode |
| F1 | `\x1bOP` | Function key 1 |
| F2 | `\x1bOQ` | Function key 2 |
| F3 | `\x1bOR` | Function key 3 |
| F4 | `\x1bOS` | Function key 4 |
