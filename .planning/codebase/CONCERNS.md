# Codebase Concerns

**Analysis Date:** 2026-08-10

## Tech Debt

**Atlas dirty tracking always true:**
- Issue: `GlyphAtlas::dirty()` always returns `true` (line 172–175 of `src/render/font.rs`), causing full atlas re-upload every frame
- Files: `src/render/font.rs`, `src/render/pipeline.rs`
- Impact: Unnecessary GPU memory traffic every frame; atlas is re-uploaded even when nothing changed
- Fix approach: Add a `dirty: bool` field to `GlyphAtlas`, set on cache miss in `get_or_rasterize()`, clear after `upload_atlas()` in the pipeline

**Inverse rendering is broken:**
- Issue: In `src/render/pipeline.rs:451–453`, the inverse swap uses `.clone()` on a temporary which doesn't actually swap the local variables
- Files: `src/render/pipeline.rs:451–453`
- Impact: SGR inverse attribute (mode 7) has no visual effect
- Fix approach: Compute fg/bg directly with a match: `let (fg, bg) = if cell.attrs.inverse { (bg_computed, fg_computed) } else { (fg_computed, bg_computed) };`

**PtyWriter uses unsafe `from_raw_fd` per write:**
- Issue: Each `write()` call in `src/pty/mod.rs:72–81` creates a temporary `ManuallyDrop<File>` via `from_raw_fd`
- Files: `src/pty/mod.rs:72–81`
- Impact: Fragile; works now but unsafe pattern on every keystroke
- Fix approach: Create the `File` once at construction and store it, or use `OwnedFd` directly with `write`

**No SIGWINCH relay from PTY:**
- Issue: Terminal resize is only driven by window resize events. PTY-initiated size changes are not reflected to the grid
- Files: `src/main.rs`, `src/pty/mod.rs`
- Impact: Programs that resize the terminal from within (e.g., `screen`, `tmux` inside) won't update correctly
- Fix approach: Could be addressed later with a signal handler or `SIGWINCH` handling in the reader thread

**OSC handling is minimal:**
- Issue: OSC dispatch in `src/grid/mod.rs:681–688` only logs the window title but doesn't set it on the window. OSC 52 (clipboard) is completely unhandled
- Files: `src/grid/mod.rs:681–688`
- Impact: `echo -ne '\033]0;My Title\007'` does nothing visible. No clipboard support via OSC 52
- Fix approach: Store title in Grid state, pass it to window via `set_title()`. Add OSC 52 for clipboard

## Known Bugs

**DECTCEM (cursor visibility) ignored:**
- Symptoms: `\x1b[?25l` (hide cursor) and `\x1b[?25h` (show cursor) have no effect
- Files: `src/grid/mod.rs:613`
- Trigger: Any program that hides/shows cursor (vim, less, fzf, etc.)
- Workaround: Cursor always visible

**Tab stop handling is 8-column only:**
- Symptoms: Horizontal tab always advances to next 8-column stop. Real terminals support configurable tab stops
- Files: `src/grid/mod.rs:658–660`
- Trigger: Programs relying on custom tab stops (rare)
- Workaround: Most programs work fine with fixed 8-column tabs

**Alternate screen cursor restore is incorrect:**
- Symptoms: When exiting alternate screen (`?1049l`), cursor is restored from `saved_cursor` instead of the position saved when entering alt screen
- Files: `src/grid/mod.rs:616–624`
- Trigger: `vim`, `less`, `tmux` exit — cursor position may be wrong
- Workaround: `saved_cursor` is set at many points so it's often close enough

## Security Considerations

**PTY master fd shared via `ManuallyDrop` + `from_raw_fd`:**
- Risk: The master file descriptor is wrapped in `Arc<OwnedFd>` but the writer creates temporary file handles from the raw fd
- Files: `src/pty/mod.rs:162–168`
- Current mitigation: Single-threaded writer; `std::mem::forget(master)` prevents PtyMaster from closing the fd
- Recommendations: Store a proper `Arc<File>` or use a safe write wrapper instead of raw fd manipulation

**No PTY process cleanup on exit:**
- Risk: If the application crashes, the child shell may be orphaned
- Files: `src/pty/mod.rs`
- Current mitigation: Shell is a direct child — SIGHUP is sent when terminal exits
- Recommendations: Add a `Drop` impl for the PTY that explicitly kills and waits for the child

## Performance Bottlenecks

**Full grid scan every frame:**
- Problem: `render()` in `src/render/pipeline.rs:442–492` iterates every cell every frame, even unchanged ones
- Files: `src/render/pipeline.rs:442–492`
- Cause: Cell dirty tracking exists (`Cell.dirty`) but is never checked
- Improvement path: Only generate instances for dirty cells; clear dirty flag after upload

**Instance buffer reallocation:**
- Problem: When instance count exceeds `instance_capacity`, a new buffer is created instead of grown
- Files: `src/render/pipeline.rs:518–529`
- Cause: wgpu doesn't support buffer resize
- Improvement path: Pre-allocate with generous capacity (`cols * rows * 2 + 1024`)

**PTY reader thread 1ms sleep on EAGAIN:**
- Problem: When no data is available, the reader thread sleeps 1ms before retrying
- Files: `src/pty/mod.rs:203`
- Cause: Non-blocking I/O with polling instead of `poll()`/`epoll()`
- Improvement path: Use `poll()` syscall to block until data is available or timeout

## Fragile Areas

**Grid resize doesn't handle scrollback:**
- Files: `src/grid/mod.rs:201–225`
- Why fragile: Resize copies min(old, new) rows/cols but discards scrollback and content beyond bounds
- Safe modification: Append visible lines to scrollback before overwriting on resize
- Test coverage: No tests for resize behavior

**CSI parameter parsing uses flat array:**
- Files: `src/grid/mod.rs:388–470`
- Why fragile: SGR parsing flattens sub-params, discarding structure. Works for all common SGR codes but breaks for nested sub-params
- Safe modification: Only change if a specific escape sequence fails
- Test coverage: Only one test (`headless_render`) — no SGR parsing tests

## Scaling Limits

**Glyph atlas 1024×1024:**
- Current capacity: ~2000–4000 glyphs depending on size
- Limit: Full Unicode coverage (CJK, emoji) would exhaust the atlas
- Scaling path: Dynamic atlas sizing or multi-page atlas. Sufficient for ASCII + Latin-1 + basic CJK now

**No scrollback limit:**
- Current capacity: `scrollback: Vec<Vec<Cell>>` grows unbounded
- Limit: Long-running sessions consume arbitrary memory
- Scaling path: Cap scrollback at 10,000–50,000 lines, evict oldest when full

## Dependencies at Risk

**nix 0.29:**
- Risk: Breaking changes every minor version; tightly coupled to Linux/POSIX APIs
- Impact: PTY management, fork, ioctl
- Migration plan: Pin to current version; unlikely to need migration

**fontdue 0.8:**
- Risk: Pure Rust font rasterizer — less battle-tested than FreeType/harfbuzz
- Impact: Glyph rendering quality and correctness
- Migration plan: Sufficient for monospace terminal use. Consider `swash` for better shaping if needed

## Missing Critical Features

**Clipboard support (Ctrl+Shift+C/V):**
- Problem: No way to copy/paste text
- Blocks: Essential for any usable terminal — makes it impractical for daily use

**Mouse tracking:**
- Problem: No mouse event encoding or DECSET modes 1000/1002/1003/1006
- Blocks: tmux, vim, fzf, and most TUI applications won't respond to mouse input

**Bell sound/visual:**
- Problem: BEL (0x07) is silently ignored (`src/grid/mod.rs:650`)
- Blocks: Terminal bell alerts, progress notifications

**Bracketed paste mode:**
- Problem: No DECSET 2004 support — pasted text indistinguishable from typed input
- Blocks: Pasting multi-line text into shells or editors

**Configuration file:**
- Problem: All settings hardcoded in `src/main.rs:25–28`
- Blocks: User customization

## Test Coverage Gaps

**No parser unit tests:**
- What's not tested: The VT parser state machine (`src/parser/mod.rs`) — 575 lines of state machine logic
- Files: `src/parser/mod.rs`
- Risk: Regressions in escape sequence handling invisible without manual testing
- Priority: High

**No grid unit tests:**
- What's not tested: Grid operations (print, scroll, erase, SGR, resize, alternate screen)
- Files: `src/grid/mod.rs`
- Risk: Terminal output corruption from grid bugs
- Priority: High — 732 lines with complex interactions

**No key encoding tests:**
- What's not tested: `encode_key()` in `src/main.rs:192–229`
- Files: `src/main.rs`
- Risk: Wrong escape sequences for special keys
- Priority: Medium

**Headless render test doesn't test rendering correctness:**
- What's not tested: The test in `src/render/test.rs` verifies atlas rasterization but not pixel output
- Files: `src/render/test.rs`
- Risk: Shader bugs, positioning errors, color mapping issues won't be caught
- Priority: Medium

---

*Concerns audit: 2026-08-10*
