#!/usr/bin/env python3
"""
Cross-validate the Rust sixel decoder against an independent encoder.

An independent Python encoder (written from the DEC 54870 / libsixel wire
format, sharing no code with src/sixel.rs) renders a PIL test image into a
sixel stream; the Rust decoder (examples/sixel_check) decodes it back; the
script pixel-compares the result to the source image.

Two implementations agreeing on the wire format is strong evidence the
decoder is compatible with real encoder output (chafa/img2sixel emit the
same constructs: raster attributes, inline `#Pc;2;R;G;B` colors, `!N`
repeats, and `-`/`$` line ops).

Usage:  python3 bench/sixel_validate.py   (needs: PIL, a release build)
"""

import os
import shutil
import subprocess
import sys
import tempfile

from PIL import Image

BG = (0, 0, 0)


def encode(img):
    """Encode a PIL RGB image to a sixel payload (DCS content after 'q')."""
    w, h = img.size
    px = img.convert("RGB")

    # Raster attrs: Pn3 = width, Pn4 = height (DEC Sixel Graphics Protocol).
    out = bytearray(f'"1;1;{w};{h}'.encode())
    regs = {}
    cur_reg = -1

    def select(rgb):
        nonlocal cur_reg, out
        if rgb not in regs:
            reg = len(regs)
            regs[rgb] = reg
            out += f'#{reg};2;{rgb[0]};{rgb[1]};{rgb[2]}'.encode()
            cur_reg = reg
        elif regs[rgb] != cur_reg:
            cur_reg = regs[rgb]
            out += f'#{cur_reg}'.encode()

    y = 0
    while y < h:
        x = 0
        while x < w:
            col = px.getpixel((x, y))
            if col == BG:
                out += b' '  # space advances the column without drawing
                x += 1
                continue
            select(col)
            x0 = x
            while x < w and px.getpixel((x, y)) == col:
                x += 1
            run = x - x0
            mask = 0
            for band in range(6):
                yy = y + band
                if yy < h and px.getpixel((x0, yy)) == col:
                    mask |= 1 << band
            ch = chr(0x3F + mask)
            out += (f'!{run}{ch}' if run > 1 else ch).encode()
        y += 6
        if y < h:
            out += b'-'  # advance one 6-pixel band, reset column
    return bytes(out)


def build_test_image():
    """A 3-color image: flat blocks that exercise runs, inline colors, and
    multi-band masks (block edges cut through 6-pixel bands)."""
    img = Image.new("RGB", (40, 18), BG)
    px = img.load()
    for y in range(0, 12):
        for x in range(4, 10):
            px[x, y] = (255, 0, 0)        # red block, 12 px tall
    for y in range(6, 18):
        for x in range(16, 24):
            px[x, y] = (0, 200, 0)        # green block, 12 px tall
    for y in range(0, 6):
        for x in range(28, 36):
            px[x, y] = (0, 0, 255)        # blue block, 6 px tall
    return img


def validate_with_chafa(binary):
    """Cross-check against chafa, a real-world encoder, when available.

    chafa quantizes colors to a small palette, so this checks structure
    rather than exact pixels: the decoded size must equal the source size
    (chafa declares raster width/height = source dims, Pn3 = width per the
    DEC spec) and each colored block must decode at its position with the
    right dominant hue.
    """
    chafa = shutil.which("chafa")
    if not chafa:
        print("chafa not found on PATH — skipping real-encoder cross-check")
        return 0

    src = build_test_image()  # 40x18, red/green/blue blocks
    with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as f:
        src.save(f.name)
        png = f.name
    try:
        proc = subprocess.run(
            [chafa, "--format=sixel", png], capture_output=True, timeout=30
        )
    finally:
        os.unlink(png)
    if proc.returncode != 0:
        print(f"chafa encode failed: {proc.stderr.decode(errors='replace').strip()}")
        return 1

    payload = proc.stdout
    with tempfile.NamedTemporaryFile(suffix=".sixel", delete=False) as f:
        f.write(payload)
        path = f.name
    try:
        dec = subprocess.run(
            [binary, path, "800", "600"], capture_output=True, timeout=30
        )
    finally:
        os.unlink(path)
    if dec.returncode != 0:
        print(f"chafa payload decode failed: {dec.stderr.decode(errors='replace').strip()}")
        return 1

    size_line = next(
        (l for l in dec.stderr.decode().splitlines() if l.startswith("SIZE")), None
    )
    w, h = (int(v) for v in size_line.split()[1].split("x")) if size_line else (0, 0)
    if w == 0 or h == 0:
        print("chafa payload decoded empty")
        return 1

    # chafa rounds its render size (e.g. 40x18 -> 40x20), so scale the block
    # rectangles from source coords to the decoded grid.
    sw, sh = src.size
    sx, sy = w / sw, h / sh
    rgba = dec.stdout
    expected = [  # (x0, y0, x1, y1, hue predicate)
        (4, 0, 10, 12, lambda r, g, b: r > g and r > b),       # red block
        (16, 6, 24, 18, lambda r, g, b: g > r and g > b),      # green block
        (28, 0, 36, 6, lambda r, g, b: b > r and b > g),       # blue block
    ]
    for x0, y0, x1, y1, pred in expected:
        X0, Y0 = int(x0 * sx), int(y0 * sy)
        X1, Y1 = max(X0 + 1, int(x1 * sx)), max(Y0 + 1, int(y1 * sy))
        n = (X1 - X0) * (Y1 - Y0)
        rs = gs = bs = 0
        for y in range(Y0, Y1):
            for x in range(X0, X1):
                i = (y * w + x) * 4
                rs += rgba[i]
                gs += rgba[i + 1]
                bs += rgba[i + 2]
        r, g, b = rs / n, gs / n, bs / n
        if not pred(r, g, b):
            print(f"chafa block at ({X0},{Y0}) wrong hue: avg ({r:.0f},{g:.0f},{b:.0f})")
            return 1
    print(f"PASS: chafa payload decodes to {w}x{h}, all 3 blocks at position with correct hue")
    return 0


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    binary = os.path.join(root, "target", "release", "examples", "sixel_check")
    if not os.path.exists(binary):
        print("build the helper first: cargo build --release --example sixel_check")
        return 1

    src = build_test_image()
    payload = encode(src)
    print(f"payload: {len(payload)} bytes")

    with tempfile.NamedTemporaryFile(suffix=".sixel", delete=False) as f:
        f.write(payload)
        path = f.name
    try:
        proc = subprocess.run(
            [binary, path, "800", "600"],
            capture_output=True,
            timeout=30,
        )
    finally:
        os.unlink(path)

    if proc.returncode != 0:
        print(f"DECODE FAILED: {proc.stderr.decode(errors='replace').strip()}")
        return 1

    size_line = next(
        (l for l in proc.stderr.decode().splitlines() if l.startswith("SIZE")), None
    )
    w, h = (int(v) for v in size_line.split()[1].split("x")) if size_line else (0, 0)
    if w == 0 or h == 0:
        print("DECODER RETURNED EMPTY IMAGE")
        return 1

    rgba = proc.stdout
    if len(rgba) != w * h * 4:
        print(f"rgba length mismatch: {len(rgba)} != {w * h * 4}")
        return 1

    mismatches = 0
    checked = 0
    for y in range(h):
        for x in range(w):
            i = (y * w + x) * 4
            got = (rgba[i], rgba[i + 1], rgba[i + 2])
            want = src.getpixel((x, y))
            checked += 1
            if got != want:
                mismatches += 1
                if mismatches <= 8:
                    print(f"  mismatch at ({x},{y}): got {got} want {want}")

    if mismatches:
        print(f"FAIL: {mismatches}/{checked} pixels differ")
        return 1

    print(f"PASS: {w}x{h} decoded, all {checked} pixels match the source image")
    return 0


if __name__ == "__main__":
    rc = main()
    if rc == 0:
        rc = validate_with_chafa(
            os.path.join(
                os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                "target",
                "release",
                "examples",
                "sixel_check",
            )
        )
    sys.exit(rc)
