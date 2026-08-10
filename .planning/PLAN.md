# Terminal Emulator — Issue Fix Plan

**Created:** 2026-08-10
**Goal:** Fix bugs, tech debt, and add missing test coverage
**Status:** Ready to execute

---

## Phase 1: Critical Bugs (Days 1–2)

### 1.1 Fix Inverse Rendering
**File:** `src/render/pipeline.rs:451–453`
**Problem:** The swap doesn't work — uses `.clone()` on a temporary
**Fix:**
```rust
// Before (broken):
if cell.attrs.inverse {
    std::mem::swap(&mut bg, &mut fg.clone());
}

// After (correct):
let (fg, bg) = if cell.attrs.inverse {
    (bg_computed, fg_computed)
} else {
    (fg_computed, bg_computed)
};
```
**Verify:** Run `cargo test headless_render` — check output visually if possible

### 1.2 Add DECTCEM (Cursor Visibility)
**File:** `src/grid/mod.rs:613`
**Problem:** `?25l` and `?25h` are ignored
**Fix:**
- Add `cursor_visible: bool` field to `Grid` (default `true`)
- In `handle_csi` match for `25`: set `self.cursor_visible = set`
- In `pipeline.rs` render method: skip cursor block if `!grid.cursor_visible`
- Expose `cursor_visible` via a getter method

**Verify:** Run a program that hides cursor (e.g., `vim`, `less`) — cursor should disappear

### 1.3 Fix Alternate Screen Cursor Restore
**File:** `src/grid/mod.rs:616–624`
**Problem:** Cursor restored from wrong saved position
**Fix:**
- Add `alt_saved_cursor: Cursor` field to `Grid`
- On `?1049h` (enter alt): save `self.cursor` to `alt_saved_cursor`
- On `?1049l` (exit alt): restore from `alt_saved_cursor` instead of `saved_cursor`

**Verify:** Run `vim` then `:q` — cursor should return to pre-vim position

---

## Phase 2: Tech Debt (Days 3–4)

### 2.1 Fix Atlas Dirty Tracking
**File:** `src/render/font.rs:172–175`
**Problem:** `dirty()` always returns `true`
**Fix:**
- Add `dirty: bool` field to `GlyphAtlas` (default `false`)
- In `get_or_rasterize()`: set `self.dirty = true` on cache miss
- In `pipeline.rs` `render()`: call `atlas.dirty = false` after `upload_atlas()`

**Verify:** Run `cargo test headless_render` — atlas should only upload on first frame + new glyphs

### 2.2 Safe PtyWriter (Eliminate Per-Write Unsafe)
**File:** `src/pty/mod.rs:66–81`
**Problem:** Creates `ManuallyDrop<File>` from raw fd on every write
**Fix:**
- Store `File` directly in `PtyWriter` instead of raw fd
- Use `std::os::unix::io::FromRawFd` once at construction
- Remove the `ManuallyDrop` wrapper
- Ensure `File` is not closed on drop (use `try_clone()` or store as `Option<File>`)

**Verify:** `cargo build` — no warnings; terminal still functions

### 2.3 Add PTY Cleanup on Drop
**File:** `src/pty/mod.rs`
**Problem:** Child shell may be orphaned on crash
**Fix:**
- Store `child_pid: Pid` in a new `PtyHandle` struct
- Implement `Drop` for `PtyHandle`:
  - Send `SIGHUP` to child
  - Call `waitpid()` to reap
- Return `PtyHandle` from `spawn_pty()` alongside `PtyWriter`/`Receiver`

**Verify:** Kill the terminal with `kill -9 <pid>` — check `ps` that no orphaned shell remains

---

## Phase 3: Test Coverage (Days 5–6)

### 3.1 Parser Unit Tests
**File:** `src/parser/mod.rs` (new `#[cfg(test)]` module)
**Tests to add:**
```rust
#[test]
fn test_print_ascii() { /* 'A' → Action::Print('A') */ }

#[test]
fn test_csi_cursor_move() { /* \x1b[10;20H → CsiDispatch with params [10,20] */ }

#[test]
fn test_sgr_colors() { /* \x1b[31m → fg=Color::Indexed(1) */ }

#[test]
fn test_utf8_multibyte() { /* 2-byte UTF-8 → Action::Print(char) */ }

#[test]
fn test_osc_dispatch() { /* \x1b]0;title\x07 → OscDispatch */ }

#[test]
fn test_escape_intermediate() { /* \x1b(0 → ESC with intermediate 0x28 */ }
```
**Verify:** `cargo test parser` — all pass

### 3.2 Grid Unit Tests
**File:** `src/grid/mod.rs` (new `#[cfg(test)]` module)
**Tests to add:**
```rust
#[test]
fn test_print_character() { /* print 'A' at 0,0 → cell has 'A' */ }

#[test]
fn test_cursor_advance() { /* print at col=cols-1 → wraps to next line */ }

#[test]
fn test_scroll_up() { /* fill screen + print → scrollback grows, bottom cleared */ }

#[test]
fn test_erase_in_display() { /* ED mode 0 clears from cursor to end */ }

#[test]
fn test_sgr_bold() { /* SGR 1 → cell.attrs.bold = true */ }

#[test]
fn test_alternate_screen() { /* ?1049h → primary preserved, alt is empty */ }

#[test]
fn test_resize() { /* resize smaller → content truncated, no panic */ }
```
**Verify:** `cargo test grid` — all pass

### 3.3 Key Encoding Tests
**File:** `src/main.rs` (new `#[cfg(test)]` module)
**Tests to add:**
```rust
#[test]
fn test_encode_enter() { /* NamedKey::Enter → b"\r" */ }

#[test]
fn test_encode_arrows() { /* ArrowUp → b"\x1b[A" */ }

#[test]
fn test_encode_ctrl_c() { /* Character("c") with ctrl modifier → b"\x03" */ }
```
**Verify:** `cargo test encode_key` — all pass

---

## Execution Order

```
Day 1:  1.1 (inverse) → 1.2 (cursor visibility)
Day 2:  1.3 (alt screen) → verify all Phase 1
Day 3:  2.1 (atlas dirty)
Day 4:  2.2 (safe pty) → 2.3 (pty cleanup)
Day 5:  3.1 (parser tests) → 3.2 (grid tests)
Day 6:  3.3 (key tests) → full test run
```

## Success Criteria

- [ ] Inverse text renders correctly (mode 7)
- [ ] `vim`/`less` hide/show cursor properly
- [ ] Cursor restores correctly after exiting alt screen
- [ ] Atlas only re-uploads when new glyphs are rasterized
- [ ] No `unsafe` in PtyWriter::write()
- [ ] PTY child process is reaped on terminal exit
- [ ] All parser tests pass
- [ ] All grid tests pass
- [ ] All key encoding tests pass
- [ ] `cargo test` — 100% pass rate
- [ ] No new compiler warnings

---

*Plan created from CONCERNS.md analysis*
