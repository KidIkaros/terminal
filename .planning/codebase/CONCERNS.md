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
  and chunked `m=1`/`m=0` transfers; images reuse the sixel placement path.

## Remaining tech debt / gaps

- **Kitty graphics PNG (`f=100`) is not decoded.** A PNG decoder dependency
  (`png`/`image`) is needed; raw RGB/RGBA covers `chafa`/`timg` but not
  `kitten icat`. File/shared-memory transmission (`t=f`/`t=s`) and image-id
  round-trips (`a=t`/`a=p`/`a=d`) are also deferred.
- **Cluster shaping uses the primary face only.** `shape_cluster` shapes
  combining clusters against the primary font bytes; bold/italic/fallback
  variants are not shaped. Acceptable for monospace terminals.

## Known limitations (documented, intentional)

- **Resize drops sixel placements.** Reflow reorders rows, so live sixel
  placements are dropped wholesale on resize (rather than mis-mapped).
- **Glyph atlas is a single 1024×1024 page.** Sufficient for ASCII + Latin-1 +
  modest CJK, but full CJK/emoji coverage could exhaust it. The scaling path is
  multi-page atlases.
- **Parse thread / grid snapshot** is deferred. Steady-state is already smooth
  (per-tab reader threads + coalesced wakes + bounded drain); the snapshot
  refactor is only worth it if a GPU profile shows frame hitches under large
  backlogs.

## Performance notes

- **PTY drain is budget-capped** (`DRAIN_BUDGET_BYTES`, 256 KiB, overridable via
  `TERMINAL_DRAIN_BUDGET`). A backlog drains across frames rather than stalling
  one. Throughput ceiling under vsync ≈ `budget × refresh_rate`; raise the
  budget for full ~48 MiB/s sustained `cat` throughput.
- **Raw parser throughput** is ~48 MiB/s headless (validated on a quiet
  machine). This is the number that matters for `bench`; the GUI number is
  vsync/contention-bound.

## Fragile areas

- **Grid resize discards scrollback content beyond the new bounds** and reflows
  reorder rows; the behavior is tested but has edge cases for very wide/narrow
  transitions.
- **Sixel raster-attribute parse** was previously swapped (Pn3/Pn4); now
  corrected and cross-validated against `chafa`. Keep the
  `bench/sixel_validate.py` chafa check in the release loop.

## Test coverage (current)

- **~314 tests** across lib + bins (parser, grid, key encoding, config, font).
- **34/34 VT conformance** cases (`vt_conformance` binary).
- **Fuzz harness** (`src/bin/fuzz.rs`) — hundreds of MB fuzzed clean across
  parser/grid/scroll/erase/sixel paths.
- **Sixel cross-validation** against `chafa` (real encoder) in `bench/`.

## Dependencies at risk

- **nix 0.29** — breaking changes each minor; tightly coupled to POSIX PTY APIs.
- **fontdue 0.8** — pure-Rust rasterizer; sufficient for monospace use but less
  battle-tested than FreeType. Consider `swash` if shaping quality becomes a
  differentiator.
