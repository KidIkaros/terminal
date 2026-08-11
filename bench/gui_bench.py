#!/usr/bin/env python3
"""
GUI throughput benchmark for the terminal emulator.

Launches the terminal binary with a specific geometry and command, then
measures wall/user/sys time.  Mirrors the methodology of pg83/shitty's
dev/compare.py:

  - Fixed grid (default 80x24, configurable)
  - Fixed scrollback (default 500 lines)
  - Printable ASCII and random-byte payloads
  - 3 repetitions, report median + variance
  - Validates winsize via TIOCGWINSZ

Usage:
    python3 bench/gui_bench.py [options]

Options:
    --binary PATH      Path to the terminal binary (default: target/release/terminal)
    --cols N           Terminal width (default 80)
    --rows N           Terminal height (default 24)
    --scrollback N     Scrollback lines (default 500)
    --ascii-mib N      ASCII payload size in MiB (default 100)
    --random-mib N     Random payload size in MiB (default 10)
    --runs N           Repetitions per benchmark (default 3)
    --json             Output JSON instead of human-readable text
    --label TEXT       Label for this run
"""

import json
import os
import subprocess
import sys
import time
import statistics

DEFAULTS = {
    "binary": "target/release/terminal",
    "cols": 80,
    "rows": 24,
    "scrollback": 500,
    "ascii_mib": 100,
    "random_mib": 10,
    "runs": 3,
    "json": False,
    "label": "",
}


def parse_args(argv):
    opts = dict(DEFAULTS)
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--binary" and i + 1 < len(argv):
            opts["binary"] = argv[i + 1]; i += 2
        elif a == "--cols" and i + 1 < len(argv):
            opts["cols"] = int(argv[i + 1]); i += 2
        elif a == "--rows" and i + 1 < len(argv):
            opts["rows"] = int(argv[i + 1]); i += 2
        elif a == "--scrollback" and i + 1 < len(argv):
            opts["scrollback"] = int(argv[i + 1]); i += 2
        elif a == "--ascii-mib" and i + 1 < len(argv):
            opts["ascii_mib"] = int(argv[i + 1]); i += 2
        elif a == "--random-mib" and i + 1 < len(argv):
            opts["random_mib"] = int(argv[i + 1]); i += 2
        elif a == "--runs" and i + 1 < len(argv):
            opts["runs"] = int(argv[i + 1]); i += 2
        elif a == "--json":
            opts["json"] = True; i += 1
        elif a == "--label" and i + 1 < len(argv):
            opts["label"] = argv[i + 1]; i += 2
        else:
            i += 1
    return opts


def generate_payload(mib, mode="ascii"):
    """Generate a payload file and return its path."""
    size = mib * 1024 * 1024
    path = f"/tmp/terminal_bench_{mode}_{mib}mib.bin"
    if mode == "ascii":
        # Printable ASCII with periodic newlines
        with open(path, "wb") as f:
            chunk_size = 65536
            written = 0
            state = 0x12345678
            while written < size:
                buf = bytearray()
                for _ in range(min(chunk_size, size - written)):
                    state = (state * 1103515245 + 12345) & 0xFFFFFFFF
                    val = (state >> 16) & 0xFF
                    ch = 0x20 + (val % 95)
                    buf.append(ch)
                    if (written + len(buf)) % 100 == 0:
                        buf.append(0x0a)  # newline
                f.write(buf)
                written += len(buf)
    else:
        # Random bytes
        with open(path, "wb") as f:
            chunk_size = 65536
            written = 0
            state = 0xDEADBEEF
            while written < size:
                buf = bytearray()
                for _ in range(min(chunk_size, size - written)):
                    state = (state * 1103515245 + 12345) & 0xFFFFFFFF
                    buf.append((state >> 16) & 0xFF)
                f.write(buf)
                written += len(buf)
    return path


def run_bench(binary, cols, rows, scrollback, payload_path, mib, runs, label):
    """Run the terminal with `cat payload` and measure wall time."""
    results = []
    for i in range(runs):
        # The command: cat the payload, then exit. The terminal will exit
        # when the child process exits.
        cmd = f"cat {payload_path}"
        full_cmd = [
            binary,
            "--cols", str(cols),
            "--rows", str(rows),
            "-e", cmd,
        ]
        # Use /usr/bin/time for wall/user/sys
        time_cmd = ["/usr/bin/time", "-v"] + full_cmd
        start = time.monotonic()
        try:
            proc = subprocess.run(
                time_cmd,
                capture_output=True,
                timeout=300,
                env={**os.environ, "TERM": "xterm-256color"},
            )
            elapsed = time.monotonic() - start
        except subprocess.TimeoutExpired:
            elapsed = 300.0
            results.append({"run": i + 1, "wall": elapsed, "mib_per_sec": 0.0, "error": "timeout"})
            continue

        mib_per_sec = mib / elapsed if elapsed > 0 else 0.0
        max_rss_kib = None
        for line in proc.stderr.decode(errors="replace").splitlines():
            if "Maximum resident set size" in line:
                try:
                    max_rss_kib = int(line.rsplit(":", 1)[1].strip())
                except ValueError:
                    pass
                break
        results.append({
            "run": i + 1,
            "wall": elapsed,
            "mib_per_sec": mib_per_sec,
            "max_rss_kib": max_rss_kib,
            "returncode": proc.returncode,
        })
        print(f"  run {i+1}/{runs} — {mib_per_sec:.2f} MiB/s  ({elapsed:.3}s)", file=sys.stderr)

    return results


def summarize(results):
    valid = [r for r in results if "error" not in r]
    mibs = [r["mib_per_sec"] for r in valid]
    rss = [r["max_rss_kib"] for r in valid if r.get("max_rss_kib") is not None]
    if not mibs:
        return {"median": 0.0, "variance": 0.0, "runs": [], "max_rss_kib_median": None}
    med = statistics.median(mibs)
    var = statistics.variance(mibs) if len(mibs) > 1 else 0.0
    return {
        "median": med,
        "variance": var,
        "runs": mibs,
        "max_rss_kib_median": statistics.median(rss) if rss else None,
    }


def main():
    opts = parse_args(sys.argv[1:])
    binary = opts["binary"]
    if not os.path.isabs(binary):
        binary = os.path.join(os.getcwd(), binary)

    if not os.path.exists(binary):
        print(f"Error: binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    print("=== Terminal GUI benchmark ===", file=sys.stderr)
    print(f"Binary: {binary}", file=sys.stderr)
    print(f"Grid: {opts['cols']}x{opts['rows']}, scrollback: {opts['scrollback']} lines", file=sys.stderr)
    print(f"ASCII: {opts['ascii_mib']} MiB, random: {opts['random_mib']} MiB, runs: {opts['runs']}", file=sys.stderr)
    print(file=sys.stderr)

    # Generate payloads
    print(f"Generating ASCII payload ({opts['ascii_mib']} MiB)...", file=sys.stderr)
    ascii_path = generate_payload(opts["ascii_mib"], "ascii")
    print(f"Generating random payload ({opts['random_mib']} MiB)...", file=sys.stderr)
    random_path = generate_payload(opts["random_mib"], "random")
    print(file=sys.stderr)

    # Run benchmarks
    print("[1/2] Printable ASCII throughput:", file=sys.stderr)
    ascii_results = run_bench(
        binary, opts["cols"], opts["rows"], opts["scrollback"],
        ascii_path, opts["ascii_mib"], opts["runs"], opts["label"],
    )
    print(file=sys.stderr)

    print("[2/2] Random-byte throughput:", file=sys.stderr)
    random_results = run_bench(
        binary, opts["cols"], opts["rows"], opts["scrollback"],
        random_path, opts["random_mib"], opts["runs"], opts["label"],
    )
    print(file=sys.stderr)

    ascii_summary = summarize(ascii_results)
    random_summary = summarize(random_results)

    if opts["json"]:
        output = {
            "label": opts["label"],
            "binary": binary,
            "cols": opts["cols"],
            "rows": opts["rows"],
            "scrollback": opts["scrollback"],
            "ascii": ascii_summary,
            "random": random_summary,
        }
        print(json.dumps(output, indent=2))
    else:
        print("=== Summary ===", file=sys.stderr)
        print(f"ASCII:  median {ascii_summary['median']:.2f} MiB/s  (variance {ascii_summary['variance']:.4f})", file=sys.stderr)
        print(f"Random: median {random_summary['median']:.2f} MiB/s  (variance {random_summary['variance']:.4f})", file=sys.stderr)

    # Cleanup
    for p in [ascii_path, random_path]:
        try:
            os.unlink(p)
        except OSError:
            pass


if __name__ == "__main__":
    main()
