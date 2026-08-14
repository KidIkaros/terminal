# Terminal Emulation Audit — VT Conformance & Code Quality

**Date:** 2026-08-10
**Sources:** vt100.net (Williams parser spec, DEC private-mode docs, VT100 User Guide), codebase audit of all 19 source files (~9k lines), external resource list (Gemini).
**Baseline:** 166 tests passing at the original audit; the current suite has 214 library tests plus 17 binary tests passing.

This is the working issue list for bringing the terminal to full VT/xterm conformance.
Tier 1 items are bugs that break real usage *today*. Work top-down unless noted.

---

## 🔴 Tier 1 — Actual bugs (broken today) — ✅ ALL FIXED 2026-08-10 (166→182 tests)

- [x] **T1-1: Ctrl+C/Z/D/L send literal letters, not control codes.**
  `handle_shortcut` returns `false` to "let them pass through" (`src/main.rs:273-291`),
  but `encode_key()` (`src/main.rs:945`) ignores modifiers — Ctrl+C sends `'c'`, not `0x03`.
  SIGINT / SIGTSTP / EOF / clear are all broken. Fix: map ctrl+letter → control byte in encode path.
- [x] **T1-2: OSC terminated by ST (`ESC \`) must dispatch.**
  The anywhere-rule for `0x1b` (`src/parser/mod.rs:164`) fires before the OscString handler,
  so the OSC is never dispatched. Williams' spec: ESC while in OSC must dispatch the OSC then
  process the ESC. Test `test_osc_terminates_on_st` passes while hiding this (asserts action
  count only). fish/tmux/kitty terminate OSC with ST.
- [x] **T1-3: IL/DL ignore cursor row.**
  `CSI L`/`CSI M` (`src/grid/mod.rs:901-909`) scroll the whole region; VT spec acts on
  `cursor_row..scroll_bottom`. Breaks vim (uses IL/DL constantly).
- [x] **T1-4: Scrollback navigation is cosmetic.**
  Shift+PageUp updates `scrollback_offset` (`src/main.rs:817-836`) but render loop only
  iterates the visible grid (`src/render/pipeline.rs:587`). Scrolling up shows nothing.
- [x] **T1-5: `TERM` never set in child.**
  `spawn_pty` does no `setenv` before `execvp` (`src/pty/mod.rs:178-182`). Set `TERM=xterm-256color`
  (and propagate COLORTERM if desired).
- [x] **T1-6: OSC 52 is a no-op.**
  `src/grid/mod.rs:1105-1107` only logs. `clipboard.rs` has working helpers
  (`osc52_set`/`parse_osc52_response`) never wired to the OSC handler.
- [x] **T1-7: Scrollback eviction is O(n).**
  `scrollback.remove(0)` (`src/grid/mod.rs:645`) shifts the whole Vec per line past capacity.
  Switch to `VecDeque` (also helps T1-4 rendering).
- [x] **T1-8: Combining chars get width 1.**
  `UnicodeWidthChar::width(ch).unwrap_or(1)` (`src/grid/mod.rs:580`) — combining marks return
  `None` and must be width 0. Per UAX #11/UAX #29. Mangles accented text.

## 🟠 Tier 2 — The terminal never answers (response channel) — ✅ DONE 2026-08-10 (182→194 tests)

Implemented with a `Grid.responses` outbox (st `ttywrite` pattern) drained into the PTY in
`drain_pty()` via `tab.pty_writer`.

- [x] **T2-1: DA1/DA2** — `CSI c` → `CSI ?62;22c` (VT220 class, ANSI color), `CSI > c` → `CSI >1;20;0c`.
- [x] **T2-2: DSR/CPR** — `CSI 5n` → `CSI 0n`; `CSI 6n` → `CSI row;col R`; DECXCPR `CSI ?6n` → `CSI ?r;c;1R`.
- [x] **T2-3: DECRQM/DECRPM** — `CSI ? Pd $ p` → `CSI ? Pd ; Ps $ y` for tracked modes (25, 1000-1006,
  1049, 2004), `Ps=0` for unknown; ANSI-mode DECRQM also answers `Ps=0` instead of hanging.
- [x] **T2-4: OSC color queries** — `OSC 4;idx;?`, `10;?`, `11;?`, `12;?` reply with `rgb:…/…/…` specs.
- [x] **T2-5: CPR/DECXCPR respect DECOM** — when origin mode is active, reported rows are relative to
  the active scrolling region; absolute rows remain unchanged when DECOM is reset. Regression tests
  cover both standard CPR and DECXCPR.

## 🟡 Tier 3 — VT conformance gaps (vt100.net / xterm private-mode checklist) — ✅ ALL DONE (218 tests)

- [x] **T3-1: DECCKM (?1) + DECPAM/DECPNM (`ESC =`/`ESC >`)** — application cursor keys.
  Arrows + Home/End remap CSI→SS3 when ?1 set (`app_cursor_remap` in main.rs).
  DECPAM/DECPNM accept-and-ignore (no keypad emulation yet).
- [x] **T3-2: DECOM (?6)** — origin mode: CUP/HVP relative to scroll region, region clamping,
  homes cursor on set/reset.
- [x] **T3-3: DECAWM (?7)** — autowrap toggle (off = clamp to last cell).
- [x] **T3-4: DECSCNM (?5)** — screen reverse (XOR with per-cell SGR inverse; renderer).
- [x] **T3-5: DECCOLM (?3)** — accepted without resize (no corruption).
- [x] **T3-6: DECSCUSR (`CSI Ps SP q`)** — cursor shape block/underline/bar; renderer draws it
  (`render/pipeline.rs`, was hardcoded 2px underline).
- [x] **T3-7: DECALN (`ESC # 8`)** — fills screen with 'E', homes cursor, resets scroll region.
- [x] **T3-8: Character sets** — `ESC ( 0` / `ESC ) 0` designate G0/G1 DEC special graphics
  (box drawing); SO/SI (0x0e/0x0f) switch active set. `print()` remaps via the canonical
  st vt100_0 table (62-entry). `ESC ( B`/`ESC ) B` restores US ASCII.
- [x] **T3-9: Tab stops** — HTS (`ESC H`) sets a stop, TBC (`CSI g` / `CSI 3 g`) clears one/all,
  HT (0x09) advances to the next stop (clamps to last col if none). Stops stored per-column in
  `tab_stops` and reinitialized to every-8 on resize.
- [x] **T3-10: ICH (`CSI @`)** — insert characters: shifts the row right, blank-fills `n` cells at
  the cursor, falls off the right edge. Cursor stays put.
- [x] **T3-11: REP (`CSI b`)** — repeats the last printed graphic char `n` times (the same char +
  `n-1` copies). Tracks `last_char` in `print()`.
- [x] **T3-12: IRM (?4)** — insert mode (ANSI arm `CSI 4 h/l`; print shifts row right).
- [x] **T3-13: ED 3 clears scrollback** — mode 2 and 3 both wipe the screen, and 3 also empties
  `scrollback` so the user can't scroll back into cleared content.
- [x] **T3-14: Per-screen saved cursor** — DECSC/DECRC (ESC 7/8 and CSI 7/s/8) store/restore a
  separate `SavedCursor` for the active screen; primary and alternate saves can't bleed across.
- [x] **T3-15: Shift+Tab** — `CSI Z` (CBT); `encode_key` emits `CSI Z` for Shift+Tab, grid
  handles `CSI Ps Z` backward tabulation to the Ps-th previous tab stop.
- [x] **T3-16: Modified keys** — `encode_key` takes `ModifiersState`; cursor keys emit
  `CSI 1 ; P X` with modifier param P (xterm/readline convention); Alt+char sends ESC prefix;
  Shift+Tab → `CSI Z`.
- [x] **T3-17: DCS passthrough** — parser fires `Action::Hook` with params/intermediates/final_byte
  on DCS entry; grid accumulates data via `Put`, answers `DECRQSS` (`DCS 1 $ q`) on `Unhook`
  with `DCS 1 $ r <query> ST`. Buffer capped at `MAX_OSC_DCS_LEN` (100KB).
- [x] **T3-18: OSC max length** — raised from 1024 to 100_000 (`MAX_OSC_LEN` in parser).
- [x] **T3-19: Mouse encoding 1015 (urxvt)** — `MouseEncoding::Urxvt` variant added;
  `encode_urxvt()` emits `CSI Cb ; Cx ; Cy M` with decimal 1-based coordinates.
- [x] **T3-20: SGR sub-parameters** — `apply_sgr` reads colon sub-params for `38:5:idx`,
  `38:2::r:g:b` (with tone slot), and `38:2:r:g:b` (without tone slot); same for `48`.

## 🟢 Tier 4 — Rendering & polish — ✅ ALL DONE (214 library tests)

- [x] **T4-1: Underline / strikethrough / dim / blink** — renderer draws underline (2px rect at
  cell bottom), strikethrough (2px rect at vertical middle), dim (fg intensity halved), blink
  (rendered at half intensity as a visual approximation; time-based toggle is future work).
  Invisible cells skip the glyph entirely. (`render/pipeline.rs` cell loop.)
- [x] **T4-2: Search/copy visible-lines-only** — `all_lines_with_scrollback` now returns
  scrollback lines + visible grid lines instead of just the viewport.
- [x] **T4-3: Focus reporting (?1004)** — `Grid.focus_in()`/`focus_out()` queue `CSI I`/`CSI O`
  when mode 1004 is set; `WindowEvent::Focused` in main.rs fires them.
- [x] **T4-4: Synchronized output (`CSI ? 2026 h/l`)** — `Grid.synchronized_output` flag;
  `about_to_wait` suppresses redraws while set, does one final redraw on reset.
- [x] **T4-5: Hyperlink map pruned** — `Grid.prune_hyperlinks()` scans visible grid + scrollback
  for live IDs and removes orphans; called on resize.
- [x] **T4-6: Cursor blink reset on input** — keypress resets `cursor_visible = true` and
  restarts the blink timer (`last_cursor_blink = now`).

## ✅ Stage 1 — Correctness and safety floor — COMPLETE 2026-08-11

- PTY writer ownership now uses an independently duplicated fd with normal `File` lifetime;
  partial writes retry `EINTR` and the writer remains blocking while the reader remains nonblocking.
- PTY child output/lifecycle regression coverage verifies output delivery from a short-lived child.
- Live PTY draining uses `Parser::advance_bytes`; receiver disconnection closes the app cleanly after
  queued output is drained.
- CPR and DECXCPR report DECOM-relative rows when origin mode is active, with regression tests.
- Parser robustness tests cover oversized OSC/DCS-like streams, malformed control bytes, and all-byte
  streams without panics.
- OSC 8 browser launching is restricted to HTTP/HTTPS URLs without control characters; unsupported
  schemes are blocked and covered by unit tests.
- Existing resize, alternate-screen, scrollback, OSC 52, and malformed UTF-8 tests remain green.

## ✅ Stage 2 — Unicode and resize parity core — COMPLETE 2026-08-11

- Combining marks are retained on their preceding base cell (including marks at a line boundary),
  preserved through string extraction and rendered at the base-cell origin. Two common marks per cell
  are retained without making the hot-path cell heap allocated.
- Primary-screen resize now reflows bounded scrollback and visible rows into the new width, keeps the
  cursor visible where possible, preserves content during shrink/grow operations, and retains the
  alternate screen's visible contents during resize.
- Added regression coverage for combining marks, multiple marks, resize reflow, scrollback retention,
  and existing wide-character behavior.

The following broader Stage 2 items remain explicit follow-up work rather than being silently claimed:
Unicode cluster tails now support arbitrary-length combining/variation/ZWJ codepoints with lazy
heap storage on affected cells; extraction, scrollback, resize, and rendering preserve the full tail.
RustyBuzz shaping is integrated for cluster render units with glyph offsets/advances and the existing
fontdue atlas. The full kitty keyboard protocol (CSI u) is supported — progressive-enhancement flags
(disambiguate, event types, alternate keys, all-keys-as-escape-codes, associated text), `CSI >/=/< /?`
push/set/pop/query with per-screen stacks, key repeat/release reporting, and Escape disambiguation —
plus xterm modifyOtherKeys negotiation. DEC double-width/double-height line presentation modes have
metadata and renderer support. Extended underline styles have packed state, SGR colon parsing,
rendering approximations, and regression coverage.

## ✅ Stage 3 — Renderer safe optimization slice — COMPLETE 2026-08-11

- CPU-side `GlyphInstance` storage is now persistent across frames, avoiding repeated vector
  allocation while retaining dynamic capacity growth.
- Double-buffered GPU instance buffers continue to reuse capacity and only grow when needed.
- The renderer records the number of dirty terminal cells per frame for future frame-time/damage
  instrumentation instead of silently discarding that signal.
- Added a persistent offscreen terminal framebuffer with resize recreation and a dedicated
  texture-to-swapchain composite pass. The swapchain remains on `LoadOp::Clear`; only the
  persistent offscreen target uses `LoadOp::Load`.
- Added guarded dirty-region rendering: small terminal-only damage updates the offscreen target,
  while initial frames, cursor/selection/search/tab-bar changes, scrollback views, and large damage
  fall back to full redraws. This preserves the regression fix for disappearing output.

## ✅ Stage 4 — Input and event-loop scheduling — COMPLETE 2026-08-11

- PTY wake events are coalesced with an atomic pending gate, preventing one event-loop event per
  reader chunk during heavy output while retaining immediate wakeup behavior.
- Failed wake sends clear the gate so later PTY output can retry; tests cover both coalescing and
  send-failure recovery.
- Idle behavior remains event-driven through `ControlFlow::Wait`/`WaitUntil`, with no fixed polling
  loop reintroduced.
- Full keyboard, PTY lifecycle, synchronized-output, focus-reporting, and redraw regression suites
  remain green.

## ✅ Stage 5 — Startup and resource discipline — COMPLETE 2026-08-11

- CJK/fallback font probing and parsing is lazy; the primary embedded font is sufficient for startup,
  and fallback files are loaded only after the first missing-glyph request.
- Added opt-in startup tracing with stage timestamps for `resumed`, primary font, window creation,
  GPU pipeline, PTY readiness, and first frame. Enable with `TERMINAL_STARTUP_TRACE=1` and
  `RUST_LOG=info`.
- Extended the GUI benchmark to capture `/usr/bin/time -v` maximum RSS alongside throughput and
  variance in JSON results.
- Current smoke trace on this Linux host records approximately 7ms font setup, 370ms GPU pipeline
  creation, 374ms PTY readiness, and 389ms first frame. These are baseline measurements, not a
  competitive claim.

## ✅ Stage 6 — Native UI/UX accessibility slice — COMPLETE 2026-08-11

- Existing tab-bar controls retain large non-overlapping hit areas, active/inactive hierarchy, hover,
  pressed feedback, visible new-tab/search/close controls, and keyboard shortcuts.
- Search remains a visible overlay with query, direction, current match, and total-match status; query
  editing now removes one Unicode scalar at a time instead of truncating UTF-8 by byte.
- Added `reduced_motion` configuration. When enabled, cursor blinking is disabled while keyboard,
  mouse, selection, clipboard, search, and tab interactions remain available.
- Added regression coverage for reduced-motion defaults, Unicode-safe search deletion, tab controls,
  search controls, selection behavior, and keyboard shortcuts.

## ✅ Stage 7 — Release hardening — COMPLETE 2026-08-11

- Added `bench/release_check.py`, a Linux-first reproducible release gate covering required packaging
  artifacts, formatting, locked tests, locked release build, Cargo metadata, benchmark-tool syntax,
  and executable release-binary validation.
- Verified Flathub and Snap packaging manifests are present alongside the benchmark and composite
  renderer artifacts.
- Verified PTY children set `TERM=xterm-256color`; release checks report the host `TERM` state rather
  than assuming the launcher provides one.
- Release checks pass with the existing warning baseline documented; warnings were not hidden or
  converted into a false clean-lint claim.
- RSS and startup trace artifacts are available from the Stage 5 benchmark and instrumentation.

## 🔵 Tier 5 — Performance architecture — ⚠️ PARTIAL (Stage 7 release-hardening slice complete)

- [x] **T5-1: Dirty-cell rendering** — a persistent offscreen target now supports guarded partial
  updates; the swapchain remains a clear-and-composite destination. Full-redraw fallbacks cover
  cursor, overlays, selection, scrollback, initialization, and large damage.
- [x] **T5-2: EventLoopProxy replaces idle wake storm** — `EventLoop::<UserEvent>` with a
  `PtyData` variant; PTY reader thread calls a wake callback (backed by `EventLoopProxy`)
  after each data chunk. `about_to_wait` uses `ControlFlow::Wait` (no blinking) or
  `WaitUntil(next_blink)` (with blinking) instead of fixed 16ms polling. Zero idle wakeups
  when the terminal is idle and cursor blinking is off.
- [x] **T5-3: Atlas dirty flag already fixed** — `GlyphAtlas::dirty` is only set in
  `pack_glyph()` (new glyph rasterized) and cleared by `take_dirty()`. The renderer only
  re-uploads when `take_dirty()` returns true. CONCERNS.md was stale; no code change needed.

## ✅ Verification plan

Per vt100.net's own advice: spec + real terminal + **vttest**.

### Automated verification — completed 2026-08-11

- [x] `cargo fmt -- --check`
- [x] `cargo test --locked` — **245 passed, 0 failed** (223 library + 22 binary tests)
- [x] `cargo test headless_render -- --nocapture` — passed; atlas artifacts written to
      `/tmp/terminal_atlas.pgm` and `/tmp/terminal_atlas.png`
- [x] `cargo build --release --locked` — release build passed
- [x] `cargo metadata --locked --no-deps --format-version 1` — dependency graph resolved
- [x] `python3 bench/release_check.py` — release gate passed
- [x] `git diff --check` — no whitespace errors
- [x] DA1 and DECALN coverage exists in unit tests (`test_da1_responds_vt220`,
      `test_decaln_fills_screen`), including parser ST assertions
- [x] Advanced compatibility coverage: grapheme tails/RustyBuzz shaping, Kitty keyboard,
      modifyOtherKeys, DEC line modes, and extended underline styles
- [x] Native headless runner: `cargo run --release --bin vt_conformance -- --json` — 31/31 cases
      passed (grew from 6 → 9 → 19 → 25 → 31 across sessions; additions: IRM insert mode,
      DECSC/DECRC save/restore, truecolor SGR colon+semicolon forms, CHT/CBT tabulation,
      DECTCEM cursor visibility, DECOM origin mode, plus a vt100.net research pass:
      DECSTR soft reset, DECREQTPARM, DECCOLM/DECSCPP real column switches (grid resize +
      app window-resize request), DECIC/DECDC column insert/delete, DECFRA/DECERA
      rectangular fill/erase, DECNKM + DECPAM/DECPNM keypad mode, DECBKM. The CHT and
      DECCOLM cases found real gaps: `CSI I` had no handler and DECCOLM was a no-op.
      JSON report saved to `bench/results/vt-conformance.json`.
- [x] Sixel lifecycle hardened (2026-08-13): placements are grid-owned (stable ids, capped at
      `MAX_LIVE_SIXELS`) and shift with scroll, drop on ED/EL/resize/alt-screen; the renderer
      reconciles GPU textures by id each frame instead of draining the queue.

### Remaining interactive verification

- [ ] Run the full `vttest` menu suite against the built binary. `vttest -V` confirms version 2.7
      (20251205), but the suite requires interactive PTY/GUI input and cannot be honestly marked
      complete from noninteractive stdin. Required menus: DA/DSR, cursor, screen, rendition,
      character sets, keyboard, reports, VT102, and known-bug checks.
- [ ] Manual smoke: `vim`, `less`, `tmux`, `fzf`, `htop`, `fish` (ST-terminated OSC), `nano`.
      Available: `less`, `htop`, `nano`; missing: `vim`, `tmux`, `fzf`, `fish`. The available
      programs still require interactive GUI input.
- [x] Binary startup smoke check — release binary launched with `-e` and completed a GUI/PTTY
      output benchmark without initialization failure.
- [ ] Install missing interactive smoke programs and repeat the checks; package installation
      requires authenticated `sudo` access in this environment.

### Quality follow-up

- [ ] Strict Clippy: `cargo clippy --all-targets --all-features --locked -- -D warnings`.
      Currently blocked by the existing warning baseline, primarily dead code, unused imports,
      and style lints outside the VT verification scope.
- [!] `cargo audit` completed. It reports unmaintained dependency advisories for `paste` via
      wgpu/Metal, `ttf-parser` via fontdue/winit, and `rustybuzz` 0.20.1. No dependency was
      silently replaced or security policy weakened; the RustyBuzz advisory needs a deliberate
      shaping-stack decision before release sign-off.

**Suggested order:** Tier 1 (all small, surgical; T1-1 and T1-2 first) → T2 response channel
(T3 DECCKM/DECOM + vttest depend on it) → vttest-driven Tier 3 → T4/T5.

---

## Research findings (2026-08-10 — primary sources read, evidence below)

Sources fetched and read this pass: Williams' parser spec (vt100.net/emu/dec_ansi_parser),
Alacritty `vte` crate source, suckless `st` source (real clone), xterm `ctlseqs` doc
(invisible-island, patch #410/2026-04), UAX #11, "Text Rendering Hates You" (gankra).
Each finding is tagged with the audit item it informs.

### 1. Williams parser spec — CONFIRMS T1-2 is a spec violation
- The OSC state's actions are `entry/osc_start`, `event 20-7F/osc_put`, **`exit/osc_end`** — the
  string is dispatched **on exit from the state**, not on a terminator byte.
- Williams addresses the `ESC \` question head-on: an ESC *cancels* a control string in progress;
  the following `\` (the ST) is then a no-op. Dispatch therefore must happen **when the ESC arrives**,
  i.e. the exit action fires on the ESC transition.
- **Our bug:** `src/parser/mod.rs:164` handles ESC in the anywhere-rule *before* the OscString
  state runs, transitioning to Escape and discarding `osc_buf` without dispatch. That drops every
  ST-terminated OSC. Fix: dispatch the pending OSC in the OSC/DCS/SOS-PM-APC string states when
  ESC is seen, then proceed to Escape.
- Other confirmations: C0 controls execute during sequences (we do this ✓); `7F` ignored in ground
  is VT100-correct (VT320 Latin-1 would print it); single `escape intermediate` state with `collect`
  is Williams' preferred form (xterm's multi-state variant is just an optimisation).

### 2. Alacritty `vte` crate — the canonical Williams implementation
- Has a literal test **`osc_containing_string_terminator`** feeding `\x1b]2;未\x1b\\` and asserting
  the OSC is dispatched → independent proof of the T1-2 fix direction.
- `osc_dispatch(params, bell_terminated: bool)` — passes a flag for BEL vs ST termination (useful
  if a reply must mirror the terminator).
- `MAX_OSC_RAW = 1024` is only the *default*; it's a const-generic `Parser::<SIZE>` so callers size
  it up. → For **T3-18**, match that: make the OSC cap configurable, not a hard 1024.
- Has a `Params`/`ParamsIter` type preserving ':' sub-parameters → the clean model for **T3-20**
  (our `apply_sgr` flattens sub-params, `grid/mod.rs:760`).
- `Perform::terminated()` hook exists specifically for synchronized-update handling → relevant to
  **T4-4**.
- Documented deltas vs Williams: UTF-8 input, OSC may terminate on `0x07`, 7-bit only.
- **Verdict:** `vte` is a drop-in replacement for our hand-rolled parser and would eliminate T1-2
  and the UTF-8 edge cases for free. Keep our parser only if we want zero dependencies; otherwise
  adopt `vte` and keep the bug fixes on the grid/handler side.

### 3. st (suckless) — reference patterns for our biggest structural gaps
- **Response channel (T2):** st answers DA with a literal **`"\033[?6c"`** (VT102 identity),
  configurable as `vtiden` in `config.def.h`; CPR/DSR reply via `ttywrite("\033[0n",…)` and a
  formatted `ttywrite(buf,len,1)`. Pattern: the term handler holds a writer handle and writes
  responses directly. → Our `Perform`/`Grid` needs the same outbox (`Grid.responses` drained into
  `write_to_pty`).
- **Character sets (T3-8):** `term.trantbl[4]` (G0–G3), `term.charset` (current), and a
  `vt100_0[62]` table mapping `0x41-0x7e` to Unicode box-drawing (`↑↓→←█▚☃…┘┐┌└┼─│`). Designation via
  `ESC ( ) * +`; SO/SI (`0x0e/0x0f`) switch with `term.charset = 1-(ascii-'\016')`. Copy this shape.
- **Origin mode (T3-2):** `CURSOR_ORIGIN` cursor-state bit; `tmoveto` adds `term.top` when set and
  clamps to the region. Exact semantics to implement.
- **Tab stops (T3-9):** st keeps a real tabstop array + `tputtab(±n)`.
- **Deferred wrap:** st models `CURSOR_WRAPNEXT` explicitly (cursor parks at last column, wrap
  pending). Our grid wraps eagerly in `print()`; fine for DECAWM-off defaults but use WRAPNEXT as the
  reference when implementing **T3-3**.
- **DA identity choice:** st claims VT102 (`?6c`). We claim VT220-class in the audit (`?1;2c`);
  either is defensible — pick one and make it a config constant.

### 4. xterm ctlseqs — exact encodings for the checklist
- **DECCKM (mode 1):** cursor keys send `CSI A..D` normally, **`SS3 A..D`** (= `ESC O A`) in app
  mode; Home/End `CSI H/F` vs `SS3 H/F`. → **T3-1**.
- **Mode numbers confirmed:** 1=DECCKM, 3=DECCOLM, 5=DECSCNM, 6=DECOM, 7=DECAWM → **T3-1..T3-5**.
- **CPR:** `CSI 6 n` → `CSI r ; c R`; DEC private form `CSI ? 6 n` → `CSI ? r ; c R` (adds page).
  → **T2-2**.
- **DA1:** `CSI ? 1 ; 2 c`=VT100+AVO, `CSI ? 6 c`=VT102; VT220+ = `CSI ? 62 ; Ps c` with feature bits
  (Ps=22 ⇒ ANSI color). **DA2:** `CSI > Pp ; Pv ; Pc c`. → **T2-1**.
- **REP:** `CSI Ps b` "repeat preceding graphic character Ps times" → **T3-11**.
- **Focus reporting:** `SET_FOCUS_EVENT_MOUSE = 1004` → **T4-3** (already mode 1004 in our plan).
- **DECUDK** (user-defined keys) is a DCS → reinforces that **T3-17** (DCS passthrough) unlocks real
  features, not just sixel.
- **Note:** synchronized output `2026` is *not* in the base ctlseqs doc — it's a newer patch-level
  extension (documented in the contour/terminal-wg gist). Mode `2048` (in-band resize notify,
  detected via DECRQM) is the related newer one. Keep **T4-4** but source the 2026 spec from the
  terminal-wg gist, not ctlseqs.

### 5. UAX #11 — width rules need tailoring, not just unicode-width
- `East_Asian_Width` has six values (Ambiguous/Fullwidth/Halfwidth/Narrow/Wide/Neutral) that resolve
  to narrow/wide by context.
- **Key caveat (verbatim):** "The East_Asian_Width property is **not intended for use by modern
  terminal emulators without appropriate tailoring** on a case-by-case basis." → the `unicode-width`
  crate is a starting point, not the whole answer (**T1-8**).
- **Emoji:** characters with `Emoji_Presentation` are Wide (2 cells), except `Regional_Indicator`.
  Our `unwrap_or(1)` mishandles both combining marks (should be 0) and some emoji. Plan: width 0 for
  combining/`None`, explicit wide set for `Emoji_Presentation`.

### 6. "Text Rendering Hates You" — context for renderer gaps
- A "character" is an **Extended Grapheme Cluster** (multiple scalars) → combining marks must attach,
  reinforcing **T1-8** (width-0 + attach to prior cell).
- Emoji may be ligatures of several emoji; fonts report partial support → renders as split components.
  Emoji need native color (ignore text color) and their own sizing → future renderer work.
- Style can change **mid-ligature** (hyperlink/color boundary splits a ligature) → relevant once we
  do real shaping; our `ligatures.rs` must handle partial-ligature masking if we keep ligatures on.

### Not fetched this pass (listed for completeness)
- **TLPI ch.62/64** (book) — authoritative for termios/forkpty; we already use the correct
  `posix_openpt` path, so low urgency.
- **Microsoft ConPTY** — Windows port only; not relevant yet.
- **Ghostty (mitchellh.com)** & **Refterm** — GPU-rendering/latency reference for **T5**; consult
  when optimizing the pipeline.

---

## Reference resources (curated list)

### Parser / VT behavior
- **vt100.net — Paul Williams' parser** (dec_ansi_parser): the state machine our parser claims;
  read 2026-08-10, confirms T1-2 (see findings §1). Also: DEC private modes docs, VT100 User Guide,
  VT220 Programmer Reference.
- **ECMA-48 (ISO/IEC 6429)** — control function semantics (free PDF).
- **ECMA-35 (ISO 2022)** — character-set designation (needed for T3-8).
- **XTerm Control Sequences** — invisible-island.net/xterm/ctlseqs (Thomas Dickey): exact encodings
  for DECCKM/DECOM/CPR/DA/mouse/focus (read 2026-08-10, see findings §4).
- **vttest** — invisible-island.net/vttest — conformance suite.

### PTY / OS layer
- **TLPI (Michael Kerrisk)** — Ch. 62 Terminals, Ch. 64 Pseudoterminals.
- **Microsoft ConPTY docs** — future Windows port.

### Text rendering / Unicode
- **"Text Rendering Hates You"** — gankra.github.io (read 2026-08-10; grapheme clusters, ligature
  splitting, emoji color; see findings §6).
- **UAX #11 (East Asian Width)** — read 2026-08-10; six-value property, requires tailoring,
  Emoji_Presentation=Wide (see findings §5, informs T1-8).

### Reference codebases
- **st (suckless)** — read source 2026-08-10 (see findings §3): DA/CPR response pattern, G0-G3
  charset tables, origin-mode bit, tabstop array. Best template for T2/T3-8/T3-2.
- **Alacritty** — Rust; **`vte` crate** read 2026-08-10 (see findings §2): canonical Williams impl,
  ST-OSC test, Params sub-param model, drop-in parser candidate.
- **Ghostty** (Mitchell Hashimoto, Zig) — mitchellh.com posts on ligatures, GPU rendering, latency.
- **Refterm** (Casey Muratori) — glyph cache + sub-millisecond frame rendering (T5 reference).

---

**Baseline:** 166 → 182 (Tier 1) → 194 (Tier 2) → 202 (T3 Batch A) → 213 (T3 Batch B) → 218 (Tier 3 complete) → 222 (Tier 4 complete) → **232 passed, 0 failed** (214 library tests + 18 binary tests; Stage 1 CPR/PTY/parser/security coverage added; T5-1 remains deferred). ROADMAP.md's "71" is stale.

## ✅ Stage 8 — Feature, conformance, performance, and security pass (2026-08-13) — 276 tests

Driven by a comparison against pg83/shitty ("a serious terminal emulator with a stupid name"):

### Feature gaps closed (things shitty lacks or we were missing)
- **Sixel inline images (DEC 54870)** — new `src/sixel.rs` decoder (palette + inline RGB/HLS + `!N` repeat + raster attributes + bounds clamping) wired through DCS `q` into `Grid::place_sixel`; cursor advances below the image; new `render/sixel.wgsl` + per-image texture/bind-group pipeline draws them over the terminal framebuffer. Shitty explicitly does not support sixel — this is a genuine differentiator.
- **Rectangular (block) selection** — `SelectionMode::Rectangular`, Alt+Click to activate, bounding-box `contains`, trailing-whitespace-trimmed extraction.
- **Shell integration** — OSC 133 prompt/command/output markers that scroll with their rows (visible + scrollback marker streams), Ctrl+Shift+Up/Down prompt jumping, OSC 7 cwd, OSC 9 notifications.
- **In-band resize (mode 2048)** — `CSI 4;rows;cols t` on resize, DECRQM-reportable.

### Conformance depth
- Headless runner grew 6 → **19 cases** (shell markers, mode 2048, sixel decode/placement, DECAWM-off clamp, REP, ICH/DCH/ECH, scroll-region confinement, UTF-8 robustness, OSC 52, cursor shapes/paste, pending wrap, DECRQM, OSC 8); all pass. Sixel decoder cross-validated against an independent Python encoder (`bench/sixel_validate.py`) with a pixel-exact match — this caught a silent pixel-loss bug in buffer growth, now fixed (monotonic growth).
- New `src/bin/fuzz.rs` deterministic harness (seeded LCG × 4 byte modes) asserting parser/grid invariants; `cargo test --bin fuzz` smoke + release-mode runs pass clean.

### Performance (parser+grid headless, `bench` binary)
- Row-vector grid (`Vec<Vec<Cell>>` per screen): scrolling rotates row handles and moves rows into scrollback instead of cloning the whole visible region per line feed.
- Bulk-output path wired into the real PTY drain (`bulk_output` + one `mark_all_dirty()` per chunk), eliminating O(rows×cols) per-scroll dirty marking.
- Printable ASCII: **1.25 → 30.6 MiB/s** (24×). Random bytes: **3.2 → 12.1 MiB/s** (3.8×). All 251 library tests stayed green through the refactor.

### Security posture (shitty-style "locked down by default")
- `[security]` config: `osc52_write` (on), `osc52_read` (off by default), `window_title` (on), `uri_schemes` allowlist (replaces the hardcoded http/https-only hyperlink policy).

### Remaining known gaps vs shitty
- Bidirectional text and GPU-side compute rendering are not implemented (neither does shitty have bidi; its compute renderer is its speed edge).

## ✅ Stage 9 — Kitty keyboard protocol + background-tab backpressure (2026-08-13) — 307 tests, 34/34 conformance

- **Full kitty keyboard protocol (CSI u)** — replaced the single `kitty_keyboard` bool with a
  per-screen flags + push/pop stack: `CSI > flags u` (push), `CSI < n u` (pop), `CSI = flags;mode u`
  (progressive enhancement with mode 1/2/3), `CSI ? u` (query → `CSI ? flags u` reply). The key encoder
  now handles all five enhancements (disambiguate, event types, alternate keys, all-keys-as-escape-codes,
  associated text), Escape disambiguation (`CSI 27 u`), functional keys in canonical form, and the
  super modifier (bit 8). Press/repeat/release events are wired through `KeyEvent.repeat` +
  `ElementState::Released`. New grid, encoder, and conformance cases.
- **Background-tab PTY backpressure** — each tab's reader thread now owns a shared `reading` flag;
  `TabManager::switch_to` pauses the outgoing tab's reader and resumes the incoming one. A background
  tab stops pulling bytes off the PTY, so the kernel buffer fills and the writer blocks (xterm/kitty
  semantics) instead of growing the unbounded channel — fixing a real OOM risk and making tab
  switches instant (no multi-GiB backlog to parse).

### Assessed, deferred
- **Parse-on-worker-thread with grid snapshot** (decoupling parse from render): the steady-state path
  is already smooth — I/O runs on per-tab reader threads and wakes the loop via `EventLoopProxy`, with
  coalesced wakes and small in-flight backlogs. The only residual risk is a rare large-backlog spike;
  the full snapshot refactor is high-risk (it moves the grid off the main thread and would touch
  render, selection, search, scrollback-nav, and resize) for low marginal value.
- **Parallel (rayon) instance building**: the per-cell loop's cost is dominated by
  `atlas.get_or_rasterize` (inherently serial, `&mut` atlas cache), and typical grids are ~10k cells
  (microseconds to build); thread dispatch would likely regress small grids. Deferred until a GPU
  profile shows instance building is the bottleneck.
- GUI-frame throughput is not re-benchmarked on a display; the 30 MiB/s figure is parser+grid only.
