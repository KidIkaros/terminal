# Headless verification

## Native conformance runner

The repository-owned runner is deterministic and does not require X11/Wayland:

```bash
cargo run --release --bin vt_conformance
cargo run --release --bin vt_conformance -- --json > bench/results/vt-conformance.json
```

It covers basic text/cursor behavior, CPR/DECOM responses, arbitrary Unicode cluster tails,
resize/reflow, Kitty keyboard and `modifyOtherKeys` negotiation, underline/DEC line styles,
bounded OSC input, shell-integration markers (OSC 133), in-band resize (mode 2048), sixel
decode/placement, autowrap-off clamping, REP, ICH/DCH/ECH, scroll-region confinement,
UTF-8 robustness next to invalid bytes, OSC 52, cursor shapes / bracketed paste, pending-wrap
behaviour, DECRQM, OSC 8 hyperlinks, IRM insert mode, DECSC/DECRC cursor save/restore,
truecolor SGR (colon + semicolon forms), CHT/CBT tabulation, DECTCEM cursor visibility,
DECOM origin mode, DECSTR soft reset, DECREQTPARM, DECCOLM/DECSCPP column switches,
DECIC/DECDC column insert/delete, DECFRA/DECERA rectangular fill/erase,
DECPAM/DECNKM/DECBKM, DECSLRM left/right margins (DECLRMM ?69), DECSCA
protected attributes with selective erase (DECSED/DECSEL/DECSERA), and the full
kitty keyboard protocol (push/pop/query flags) — **34 cases**.
The CHT, DECCOLM, DECSLRM and DECSCA cases caught real gaps: `CSI I` had no handler,
DECCOLM was accepted but did not resize, and DECSLRM/DECSCA were unimplemented.

## Parser fuzz harness

Deterministic seeded fuzzing of the parser/grid seam — no panic, cursor stays in bounds,
scrollback never exceeds capacity:

```bash
cargo run --release --bin fuzz -- --seeds 8 --bytes 8M
cargo run --release --bin fuzz -- --quick   # small CI smoke
```

## Throughput (as of 2026-08-13)

Parser+grid headless throughput (no GPU; 80×24 grid, 500-line scrollback, release build,
`--bulk-output`, matching the app's real PTY drain path):

```bash
cargo run --release --bin bench -- --ascii-mib 40 --random-mib 20 --bulk-output --runs 3
```

| Workload | Before (2026-08-13) | After scroll-path work | After COW rows |
|----------|---------------------|-----------------------|----------------|
| Printable ASCII | 1.25 MiB/s | ~33 MiB/s (best 38) | ~48 MiB/s |
| Random bytes | 3.2 MiB/s | ~12 MiB/s | ~12 MiB/s |

This is the CPU-side parser+grid tier only; the full GUI tier adds rendering. Shitty's published
118–170 MiB/s figures are whole-GUI numbers, so a like-for-like comparison requires the GUI
benchmark on a display.

## Scroll-path hot loop

Profiling showed `cat bigfile` spent ~85% of CPU in the scroll path, not the write loop
(measuring the write loop alone with no scrolling hits ~250 MiB/s). The fixes, in order:

1. **Blank-row recycling** — rows evicted from the scrollback are reused as the next blank
   rows, eliminating the per-scroll allocation (+23–43% by interleaved A/B).
2. **LF batching** — runs of consecutive line feeds collapse into a single scroll.
3. **Smaller Cell** — `combining: Option<String>` → `Option<Box<str>`, 48 → 40 bytes.
4. **COW rows** — grid rows are `Arc<Vec<Cell>>` and every blank slot points at one shared
   blank row: a scroll bumps a refcount instead of allocating + memsetting 3.8 KB, and a
   full-row write rebuilds the row in one pass. Blank-row dirtiness moved to per-row flags
   (`row_is_blank` / `row_blank_dirty`) so `take_dirty_cells`/`mark_all_dirty` never clone
   the shared blank. This is the big one: ~33 → ~48 MiB/s with tight run-to-run variance.

Benchmarks on this machine swing under background load (Brave, Hermes, etc.), so compare
with best-of-N or interleaved A/B, not single runs. The whole-GUI tier (`gui_bench.py`)
additionally pays process startup, PTY I/O, and per-frame rendering; it measured ~20–27
MiB/s under load.

## Sixel cross-validation

`bench/sixel_validate.py` validates the sixel decoder against an *independent* Python
encoder written from the DEC/libSIXEL wire format (no shared code with `src/sixel.rs`):

```bash
cargo build --release --example sixel_check
python3 bench/sixel_validate.py
```

It renders a 3-color test image with PIL, encodes it (raster attributes, inline
`#Pc;2;R;G;B` colors, `!N` repeats, `-`/`$` line ops — the same constructs chafa and
img2sixel emit), decodes it through `examples/sixel_check`, and requires a pixel-exact
match. This caught a real decoder bug: growing the buffer taller with a narrower column
request truncated already-drawn pixels (fixed by making buffer growth monotonic).

When `chafa` is on `PATH`, the script also runs a **real-encoder cross-check**: it encodes
the same test image with chafa, decodes it through `examples/sixel_check`, and verifies the
size and the position/hue of every colored block (chafa quantizes colors, so this checks
structure, not exact pixels). This caught a second real bug: the raster attribute
(`"Pn1;Pn2;Pn3;Pn4`) parser read Pn3 as height and Pn4 as width, but the DEC Sixel
Graphics Protocol defines Pn3 = width, Pn4 = height — chafa's `"1;1;200;120` for a
200x120 image decoded transposed to 120x200 with only the left half drawn. Fixed; chafa
payloads now decode to their declared size with full coverage.

### Sixel lifecycle

Placements are now grid-owned state (stable ids, capped at `MAX_LIVE_SIXELS`) instead of a
drained one-frame queue, so images track the content they were drawn on:

- **Scroll** — placements shift with their rows on LF/IL/DL scrolls; images whose top row
  leaves the live region (scrolled into history, or discarded by IL/DL semantics) are dropped.
- **Clear** — `ED`/`EL` remove placements inside the erased rectangle (ED 2/3 wipe all).
- **Resize** — reflow re-orders rows, so placements are dropped wholesale (positions can no
  longer be trusted).
- **Alt screen** — entering/exiting DECSET 1049 clears the shared placement list so images
  never bleed between screens.
- **Renderer** — GPU textures are reconciled against the grid's live placement ids every
  frame (no drain), and drawn shifted by the scrollback view offset like the cells below.

## esctest2 PTY suite

`esctest2` is an external GPL-2.0 test suite and is intentionally not vendored. Obtain it from:

<https://github.com/ThomasDickey/esctest2>

Run it from its checkout after wiring the terminal command into its local terminal-launch adapter:

```bash
python3 esctest/esctest.py --action=run --expected-terminal=terminal
```

The suite is a black-box PTY oracle for xterm-like control behavior. Do not copy its test bodies
into this repository; record any failures as clean-room fixtures in `src/bin/vt_conformance.rs`
with a spec-level description.

## Interactive layer

`vttest` remains useful for visual/input-only checks that cannot be made headless. The current
machine has `vttest`, `less`, `htop`, and `nano`; `vim`, `tmux`, `fzf`, and `fish` are unavailable.
