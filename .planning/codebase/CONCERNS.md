# Codebase Concerns

**Analysis Date:** 2026-08-14 (supersedes the 2026-08-10 audit)

The initial audit predated most of the feature work. This revision records what
has since been resolved, what remains, and the few genuinely fragile areas.

## Resolved since the initial audit

The following items from the 2026-08-10 audit are no longer concerns:

- **Clipboard / mouse tracking / bracketed paste / config / themes** — all implemented.
- **OSC handling** — OSC 0/2 title, OSC 4/10/11 colors, OSC 8 hyperlinks,
  OSC 9 notifications, and OSC 52 clipboard (with a security gate) are wired.
- **Inverse video** — SGR inverse and DECSCNM are computed correctly
  (`src/render/pipeline.rs` inverse swap).
- **Atlas dirty tracking** — `GlyphAtlas::take_dirty()` gates the GPU re-upload;
  the atlas is no longer re-uploaded every frame.
- **PTY reader thread** — uses `poll()` for I/O; the only sleep is the 10 ms
  re-check when a background tab's reader is paused for backpressure.
- **PTY process cleanup** — `PtyHandle::drop` sends SIGHUP and reaps the child.
- **DECTCEM, tab stops (HTS/CHT/CBT), alt-screen cursor restore** — implemented
  and covered by conformance cases.
- **Scrollback limit** — bounded by `scrollback` config (default 10000), evicting
  oldest lines.
- **Bell** — BEL now produces config-gated feedback (`bell = "flash" | "audible" | "none"`).
- **Font selection** — `font.family` (fontconfig lookup), `font.path`,
  `font.bold_path`, and `font.italic_path` are honored; bold is synthesized when
  no bold face is loaded.
- **Config hot-reload** — SIGHUP re-applies the TOML config (themes, font,
  padding, opacity) without restarting.
- **Italic synthesis** — italic glyphs are skewed ~12° when no `italic_path`
  is loaded, so `ls --color` comment styling renders visibly.
- **Padding + opacity** — `padding` and `opacity` config are wired through to
  the renderer (previously parsed but ignored).
- **Selection** — double-click expands to a word, triple-click to a line
  (plain drag = char, Alt+drag = block, Shift+drag = line).
- **Smooth scrollback** — fractional-line scroll animation instead of row jumps.
- **Kitty graphics protocol** — `ESC _ G` with raw RGB (`f=24`) / RGBA (`f=32`)
  / PNG (`f=100`) and chunked `m=1`/`m=0` transfers; images reuse the sixel
  placement path. Image-id round-trips (`a=t`/`a=p`/`a=d`/`a=q`) let caching
  tools (ranger, image.nvim) store, re-place, query and delete images.
- **Inline video playback** — opt-in `--features video`: `terminal --video
  clip.mp4` (or Ctrl+Shift+M with a video path on the clipboard) decodes on a
  background thread via the `asciline` library and renders frames through the
  kitty-graphics pipeline (letterboxed, real-time paced).
- **Kitty graphics file transmission** — `t=f`/`t=t` read the image from a
  base64-encoded file path (SSH-friendly); paths into `/proc`, `/sys`, `/dev`,
  non-regular files, and files over 32 MiB are refused.

## Remaining tech debt / gaps

- **Kitty graphics shared-memory (`t=s`) and animation (`a=f`) are not
  implemented.** File transmission (`t=f`/`t=t`) and PNG are done; `t=s` is a
  rare Linux-shared-memory medium and `a=f` (frame animation) is niche.
- **Inline video is opt-in and ffmpeg-bound.** The `video` feature requires
  ffmpeg/ffprobe on PATH at runtime. The dependency tree was slimmed by
  feature-gating asciline's server/player stack (see below), so the feature now
  adds no tokio/axum/rayon.
- **Cluster shaping uses the primary face only.** `shape_cluster` shapes
  combining clusters against the primary font bytes; bold/italic/fallback
  variants are not shaped. Acceptable for monospace terminals.

## Known limitations (documented, intentional)

- **Resize drops sixel placements.** Reflow reorders rows, so live sixel
  placements are dropped wholesale on resize (rather than mis-mapped).
- **Glyph atlas is a single 1024×1024 page.** Sufficient for ASCII + Latin-1 +
  modest CJK, but full CJK/emoji coverage could exhaust it. The scaling path is
  multi-page atlases.
- **Parse thread / grid snapshot** — *implemented*. Each tab now runs a
  background engine thread (`src/engine.rs`) that owns the grid, parser, and
  PTY and publishes immutable `GridSnapshot`s (rows are shared `Arc` handles,
  so publishing copies only row pointers). The render/input threads never
  parse; they read snapshots and drive the engine through a command channel.
  Input state (selection, viewport scroll, resize, focus, mouse/key modes)
  round-trips as commands; side effects (bell, title, clipboard, DECCOLM)
  arrive as events. Measured win: see Performance notes — sustained `cat`
  throughput roughly quadrupled and the frame loop no longer stalls on the
  drain.

## Performance notes

- **Parsing runs on a per-tab engine thread** (`src/engine.rs`); the app never
  parses. The engine drains the PTY channel in budget-capped batches
  (`DRAIN_BUDGET_BYTES`, 256 KiB, overridable via `TERMINAL_DRAIN_BUDGET`),
  then publishes a snapshot and wakes the event loop. The budget bounds
  engine-side batch latency so snapshots stay fresh; it no longer caps GUI
  throughput the way the old single-threaded drain did.
- **Scroll-storm trace (measured 2026-08-14 after the engine refactor,
  `TERMINAL_RENDER_TRACE=1`, 24 MiB `cat`-style burst):** engine parse
  throughput ~45 MiB/s (up from ~6–12 MiB/s effective single-threaded), while
  the main thread rendered ~5000 frames at p50 ≈ 0.4 ms, p99 ≈ 1.1 ms — the
  frame loop no longer stalls on parsing. The engine and render loop run
  concurrently; the old profile (render 1.3 ms, drain 22–44 ms/call blocking
  the frame) is retired.
- **Raw parser throughput** is ~48 MiB/s headless (validated on a quiet
  machine). The GUI number is now PTY/reader-bound rather than frame-bound.

## Fragile areas

- **Grid resize discards scrollback content beyond the new bounds** and reflows
  reorder rows; the behavior is tested but has edge cases for very wide/narrow
  transitions.
- **Sixel raster-attribute parse** was previously swapped (Pn3/Pn4); now
  corrected and cross-validated against `chafa`. Keep the
  `bench/sixel_validate.py` chafa check in the release loop.

## Test coverage (current)

- **~319 tests** across lib + bins (parser, grid, key encoding, config, font,
  engine, tabs), **321** with `--features video`.
- **35/35 VT conformance** cases (`vt_conformance` binary).
- **Fuzz harness** (`src/bin/fuzz.rs`) — hundreds of MB fuzzed clean across
  parser/grid/scroll/erase/sixel paths.
- **Sixel cross-validation** against `chafa` (real encoder) in `bench/`.

## Dependencies at risk

- **nix 0.29** — breaking changes each minor; tightly coupled to POSIX PTY APIs.
- **fontdue 0.8** — pure-Rust rasterizer; sufficient for monospace use but less
  battle-tested than FreeType. Consider `swash` if shaping quality becomes a
  differentiator.
