# Troubleshooting Guide

This guide helps you solve common issues with the terminal.

## Common Issues

### Terminal Won't Start

**Symptom:** Running `terminal` produces no output or crashes immediately.

**Solutions:**

1. **Check GPU support:**
   ```bash
   # Check if your GPU supports Vulkan (Linux)
   vulkaninfo | head -20
   
   # Check if your GPU supports Metal (macOS)
   system_profiler SPDisplaysDataType
   ```

2. **Update GPU drivers:**
   - Linux: `sudo apt update && sudo apt upgrade` (Ubuntu/Debian)
   - macOS: Update via System Preferences
   - Windows: Update from GPU manufacturer website

3. **Check Rust version:**
   ```bash
   rustc --version  # Should be 1.70 or higher
   ```

4. **Rebuild from source:**
   ```bash
   cargo clean
   cargo build --release
   ```

### No Text Displayed

**Symptom:** Window opens but shows blank or garbled text.

**Solutions:**

1. **Check font configuration:**
   ```toml
   # In terminal.toml
   [font]
   size = 15.0
   ```

2. **Try a different font:**
   ```toml
   [font]
   path = "/path/to/font.ttf"
   ```

3. **Reset configuration:**
   ```bash
   rm ~/.config/terminal/config.toml
   terminal
   ```

### Colors Look Wrong

**Symptom:** Colors don't match expected theme.

**Solutions:**

1. **Check theme configuration:**
   ```toml
   [colors]
   theme = "Catppuccin Mocha"
   ```

2. **Reset to defaults:**
   ```toml
   [colors]
   background = "#1E1E2E"
   foreground = "#CDD6F4"
   ```

3. **Check application colors:**
   Some applications override terminal colors. Try:
   ```bash
   echo -e "\e[31mRed\e[0m"
   ```

### Keyboard Shortcuts Not Working

**Symptom:** Ctrl+C, Ctrl+V, etc. don't work as expected.

**Solutions:**

1. **Check if shortcuts are intercepted:**
   - Window manager may capture shortcuts
   - Try different key combinations

2. **Verify terminal is focused:**
   Click on the terminal window to ensure it has focus.

3. **Check for conflicting applications:**
   - Screen recorders may capture Ctrl+Shift+C
   - Clipboard managers may interfere

### Slow Performance

**Symptom:** Terminal feels laggy or unresponsive.

**Solutions:**

1. **Disable cursor blink:**
   ```toml
   cursor_blink_ms = 0
   ```

2. **Reduce font size:**
   ```toml
   [font]
   size = 12.0
   ```

3. **Disable ligatures:**
   ```toml
   [font]
   ligatures = false
   ```

4. **Check GPU usage:**
   ```bash
   # Linux
   nvidia-smi  # For NVIDIA GPUs
   intel_gpu_top  # For Intel GPUs
   
   # macOS
   sudo powermetrics --samplers gpu
   ```

5. **Profile the render/parse path:**
   PTY output is drained in bounded chunks per frame, so a single burst can't
   stall one frame. To measure where time actually goes, run with
   `TERMINAL_RENDER_TRACE=1` and watch the `perf drain ...` / `perf render ...`
   log lines:
   ```bash
   TERMINAL_RENDER_TRACE=1 RUST_LOG=info terminal
   ```

### Mouse Not Working

**Symptom:** Mouse clicks don't position cursor or select text.

**Solutions:**

1. **Check mouse mode:**
   Some applications (vim, tmux) enable mouse tracking. Disable it:
   ```bash
   echo -e "\e[?1000l"  # Disable normal mouse tracking
   echo -e "\e[?1002l"  # Disable button-event tracking
   echo -e "\e[?1003l"  # Disable any-event tracking
   ```

2. **Use Shift+Click for selection:**
   Hold Shift while clicking to bypass mouse tracking.

### Copy/Paste Not Working

**Symptom:** Ctrl+Shift+C/V don't copy/paste.

**Solutions:**

1. **Check clipboard support:**
   ```bash
   # Linux (X11)
   xclip -selection clipboard < /dev/null
   
   # Linux (Wayland)
   wl-copy < /dev/null
   ```

2. **Try OSC52:**
   Some applications use OSC52 for clipboard:
   ```bash
   echo -e "\e]52;c;$(echo -n "text" | base64)\a"
   ```

### Window Too Small/Large

**Symptom:** Terminal window size is wrong.

**Solutions:**

1. **Set window size in config:**
   ```toml
   [window]
   cols = 120
   rows = 40
   ```

2. **Resize manually:**
   Drag window corners to resize.

3. **Check DPI scaling:**
   High DPI displays may need adjustment in your desktop environment.

### Search Not Working

**Symptom:** Ctrl+F doesn't open search or search doesn't find text.

**Solutions:**

1. **Check if search is active:**
   Press Escape to close search, then try again.

2. **Verify search mode:**
   Look for search bar at bottom of terminal.

3. **Try different search terms:**
   - Case-insensitive by default
   - Use regex for complex patterns

## Error Messages

### "No adapter found"

**Cause:** GPU not available or drivers not installed.

**Solution:** Install GPU drivers or try software rendering:
```bash
WGPU_BACKEND=gl terminal
```

### "Font not found"

**Cause:** Custom font path is invalid.

**Solution:** Check font path:
```toml
[font]
path = "/correct/path/to/font.ttf"
```

### "Permission denied"

**Cause:** Cannot access PTY or shell.

**Solution:** Check shell exists:
```bash
which $SHELL
ls -l $SHELL
```

## Platform-Specific Issues

### Linux

1. **Wayland vs X11:**
   - Try: `WGPU_BACKEND=vulkan terminal`
   - Or: `WGPU_BACKEND=gl terminal`

2. **Missing dependencies:**
   ```bash
   # Ubuntu/Debian
   sudo apt install libwayland-dev libxkbcommon-dev
   
   # Fedora
   sudo dnf install wayland-devel libxkbcommon-devel
   ```

### macOS

1. **Gatekeeper blocking:**
   ```bash
   xattr -dr com.apple.quarantine ./target/release/terminal
   ```

2. **Permissions:**
   - Grant Accessibility permissions in System Preferences
   - Grant Input Monitoring permissions if needed

### Windows

1. **WSL2 support:**
   Use Windows Terminal for best WSL2 integration.

2. **GPU acceleration:**
   Ensure DirectX 12 support is available.

## Getting Help

If you're still experiencing issues:

1. **Check logs:**
   ```bash
   RUST_LOG=debug terminal 2>&1 | tee terminal.log
   ```

2. **Report issues:**
   - Include your OS and GPU info
   - Include terminal.toml configuration
   - Include error messages or logs

3. **Community:**
   - GitHub Issues: [link]
   - Discord: [link]

## Performance Tips

1. **Use a modern GPU:** Integrated graphics work, but dedicated GPU is better.

2. **Keep font size reasonable:** 12-18px is optimal.

3. **Disable unnecessary features:**
   ```toml
   cursor_blink_ms = 0
   [font]
   ligatures = false
   ```

4. **Use a fast shell:** zsh, fish, or bash with minimal prompt.

5. **Limit scrollback:**
   ```toml
   scrollback = 5000
   ```
