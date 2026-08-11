#!/usr/bin/env python3
"""Run the reproducible local release-hardening checks for Linux."""

from pathlib import Path
import os
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent


def run(command):
    print("+", " ".join(command))
    subprocess.run(command, cwd=ROOT, check=True)


def main():
    if sys.platform != "linux":
        print("release_check.py currently supports Linux first", file=sys.stderr)
        return 2

    required = [
        ROOT / "Cargo.lock",
        ROOT / "README.md",
        ROOT / "packaging" / "flathub" / "com.github.terminal.json",
        ROOT / "packaging" / "snap" / "snapcraft.yaml",
        ROOT / "bench" / "gui_bench.py",
        ROOT / "src" / "render" / "composite.wgsl",
    ]
    missing = [str(path.relative_to(ROOT)) for path in required if not path.is_file()]
    if missing:
        print("Missing release artifacts:", ", ".join(missing), file=sys.stderr)
        return 1

    run(["cargo", "fmt", "--", "--check"])
    run(["cargo", "test", "--locked"])
    run(["cargo", "build", "--release", "--locked"])
    run(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"])
    run([sys.executable, "-m", "py_compile", "bench/gui_bench.py", "bench/release_check.py"])

    if os.environ.get("TERM"):
        print(f"TERM={os.environ['TERM']}")
    else:
        print("TERM is unset in the release-check environment; PTY children set xterm-256color")

    binary = ROOT / "target" / "release" / "terminal"
    if not binary.is_file() or not os.access(binary, os.X_OK):
        print("Release binary is missing or not executable", file=sys.stderr)
        return 1
    print(f"Release binary: {binary.relative_to(ROOT)}")
    print("Release checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
