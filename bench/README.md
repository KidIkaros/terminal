# Headless verification

## Native conformance runner

The repository-owned runner is deterministic and does not require X11/Wayland:

```bash
cargo run --release --bin vt_conformance
cargo run --release --bin vt_conformance -- --json > bench/results/vt-conformance.json
```

It covers basic text/cursor behavior, CPR/DECOM responses, arbitrary Unicode cluster tails,
resize/reflow, Kitty keyboard and `modifyOtherKeys` negotiation, underline/DEC line styles, and
bounded OSC input.

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
