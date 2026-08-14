//! Sixel (DEC 54870 / VT340 graphics) image decoding.
//!
//! Sixel is a bitmap protocol in which each printable character in the range
//! `0x3F..=0x7E` (`?` through `~`) encodes six vertical pixels as bits:
//! the character value minus `0x3F` is a 6-bit mask, one bit per band row.
//! Control characters within the data stream:
//!
//! - `#` switches colors: `#Pc` selects palette entry Pc, and an optional
//!   `;...` tail defines the color inline (RGB percentages, `;2;R;G;B`
//!   direct 8-bit RGB, or HLS when the first tail value is 1).
//! - `!N` repeats the following graphic character N times.
//! - `$` returns the x position to column 0.
//! - `-` advances the y position down six pixels (one band) and resets x.
//! - `"` introduces raster attributes declaring image width/height.
//!
//! The decoder is a pure function over the bytes between the DCS `q` final
//! and the ST terminator; the parser/grid layer feeds it and the renderer
//! uploads the resulting RGBA texture. Output dimensions are clamped to the
//! caller-provided bounds to guard against pathological payloads.

/// DEC's standard 16-color sixel palette (xterm-style dithering colors).
pub const DEFAULT_PALETTE: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], // 0  black
    [0x00, 0x00, 0xaa], // 1  blue
    [0x00, 0xaa, 0x00], // 2  green
    [0x00, 0xaa, 0xaa], // 3  cyan
    [0xaa, 0x00, 0x00], // 4  red
    [0xaa, 0x00, 0xaa], // 5  magenta
    [0xaa, 0x55, 0x00], // 6  yellow/brown
    [0xaa, 0xaa, 0xaa], // 7  light gray
    [0x55, 0x55, 0x55], // 8  dark gray
    [0x55, 0x55, 0xff], // 9  light blue
    [0x55, 0xff, 0x55], // 10 light green
    [0x55, 0xff, 0xff], // 11 light cyan
    [0xff, 0x55, 0x55], // 12 light red
    [0xff, 0x55, 0xff], // 13 light magenta
    [0xff, 0xff, 0x55], // 14 light yellow
    [0xff, 0xff, 0xff], // 15 white
];

/// A decoded sixel image with straight-alpha RGBA pixels, top-left origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SixelImage {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// RGBA8 scanlines, row-major (`width * height * 4` bytes).
    pub rgba: Vec<u8>,
}

/// Cap on concurrently live sixel images (GPU memory bound). The grid and
/// the renderer share this constant so eviction stays index-free: the grid
/// never holds more placements than the GPU can texture, and GPU images are
/// reconciled against the grid's live placement ids every frame.
pub const MAX_LIVE_SIXELS: usize = 16;

/// A decoded sixel image positioned in the grid, awaiting renderer upload.
/// The grid records the top-left cell; the app converts pixel size to cell
/// span and hands the payload to the GPU.
#[derive(Debug, Clone)]
pub struct SixelPlacement {
    /// Stable identity, unique per placement, used to reconcile GPU textures
    /// with grid state after scrolls/clears drop or move placements.
    pub id: u64,
    /// Top-left grid column.
    pub col: usize,
    /// Top-left grid row. The grid shifts this on scroll and drops the
    /// placement when it leaves the live region.
    pub row: usize,
    /// The decoded image.
    pub image: SixelImage,
}

/// Cap a single `!N` repeat count so a corrupted payload cannot spin; the
/// output bounds clamp actual work anyway, this is belt-and-braces.
const MAX_REPEAT: u64 = 10_000_000;

/// Byte-stream cursor used while decoding.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let b = self.peek();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    /// Read a decimal number. Returns `None` when the next byte is not a digit.
    fn number(&mut self) -> Option<u32> {
        let mut value: u32 = 0;
        let mut any = false;
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() {
                any = true;
                value = value.saturating_mul(10).saturating_add((b - b'0') as u32);
                self.pos += 1;
            } else {
                break;
            }
        }
        if any {
            Some(value)
        } else {
            None
        }
    }
}

/// Convert a 0-100 component (the sixel percentage scale) to a 0-255 byte.
fn scale_percent(v: u32) -> u8 {
    (v.min(100) as u16 * 255 / 100) as u8
}

/// HLS (hue, lightness, saturation) to RGB. Hue is 0-360 degrees, lightness
/// and saturation are 0-100 on the sixel percentage scale.
fn hls_to_rgb(h: u32, l: u32, s: u32) -> [u8; 3] {
    let h = (h % 360) as f64 / 360.0;
    let l = (l.min(100) as f64) / 100.0;
    let s = (s.min(100) as f64) / 100.0;
    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return [v, v, v];
    }
    let m2 = if l <= 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let m1 = 2.0 * l - m2;
    let hue_channel = |mut h: f64| -> f64 {
        if h < 0.0 {
            h += 1.0;
        } else if h > 1.0 {
            h -= 1.0;
        }
        if h < 1.0 / 6.0 {
            m1 + (m2 - m1) * h * 6.0
        } else if h < 0.5 {
            m2
        } else if h < 2.0 / 3.0 {
            m1 + (m2 - m1) * (2.0 / 3.0 - h) * 6.0
        } else {
            m1
        }
    };
    [
        (hue_channel(h + 1.0 / 3.0) * 255.0).round() as u8,
        (hue_channel(h) * 255.0).round() as u8,
        (hue_channel(h - 1.0 / 3.0) * 255.0).round() as u8,
    ]
}

/// Grow the RGBA buffer to at least `need_w x need_h`, preserving existing
/// pixels. The buffer never exceeds `max_w x max_h`.
fn ensure(
    buf: &mut Vec<u8>,
    buf_w: &mut u32,
    buf_h: &mut u32,
    need_w: u32,
    need_h: u32,
    max_w: u32,
    max_h: u32,
) {
    // The buffer only ever grows: a new band row can request a narrower
    // width (its `$`/`-` resets put x back near 0), but widening the height
    // must never discard pixels already drawn in wider columns — the previous
    // code truncated the buffer to the narrower request and lost everything
    // right of it (found via the chafa-format cross-validation).
    let nw = need_w.clamp(1, max_w).max(*buf_w);
    let nh = need_h.clamp(1, max_h).max(*buf_h);
    if nw <= *buf_w && nh <= *buf_h {
        return;
    }
    let mut grown = vec![0u8; (nw as usize) * (nh as usize) * 4];
    // Copy the overlapping region; the clamp is belt-and-braces now that
    // growth is monotonic (fuzz-found panic protection).
    let copy_cols = (*buf_w).min(nw) as usize;
    let copy_rows = (*buf_h).min(nh) as usize;
    let len = copy_cols * 4;
    for r in 0..copy_rows {
        let src = (r as usize) * (*buf_w as usize) * 4;
        let dst = (r as usize) * (nw as usize) * 4;
        grown[dst..dst + len].copy_from_slice(&buf[src..src + len]);
    }
    *buf = grown;
    *buf_w = nw;
    *buf_h = nh;
}

/// Write one opaque pixel, ignoring writes outside the buffer bounds.
fn put_pixel(buf: &mut [u8], buf_w: u32, buf_h: u32, x: u32, y: u32, color: [u8; 3]) {
    if x >= buf_w || y >= buf_h {
        return;
    }
    let i = ((y * buf_w + x) * 4) as usize;
    buf[i] = color[0];
    buf[i + 1] = color[1];
    buf[i + 2] = color[2];
    buf[i + 3] = 0xff;
}

/// Draw the sixel character `ch` (a `0x3F..=0x7E` byte) `count` times at the
/// current position, advancing x. `ext_w`/`ext_h` track the drawn extents.
#[allow(clippy::too_many_arguments)]
fn draw_mask(
    ch: u8,
    count: u32,
    cur: [u8; 3],
    x: &mut u32,
    y: u32,
    ext_w: &mut u32,
    ext_h: &mut u32,
    buf: &mut Vec<u8>,
    buf_w: u32,
    buf_h: u32,
    max_w: u32,
    max_h: u32,
) {
    let mask = ch - 0x3f;
    let mut xi = *x;
    for _ in 0..count {
        if xi < max_w {
            for band in 0..6u32 {
                if (mask >> band) & 1 == 1 {
                    let py = y + band;
                    if py < max_h {
                        put_pixel(buf, buf_w, buf_h, xi, py, cur);
                    }
                    *ext_h = (*ext_h).max(py + 1);
                }
            }
            *ext_w = (*ext_w).max(xi + 1);
        }
        xi += 1;
    }
    *x = xi;
}

/// Parse and apply an inline color definition following `#Pc;`.
///
/// Accepts the forms emitted by real encoders:
/// - `;1;H;L;S` — HLS (hue 0-360, lightness/saturation 0-100)
/// - `;2;R;G;B` — direct 8-bit RGB (img2sixel / modern encoders)
/// - `;R;G;B`   — RGB percentages 0-100 (classic DEC)
fn parse_inline_color(cursor: &mut Cursor<'_>) -> [u8; 3] {
    let mut params = [0u32; 4];
    let mut n = 0;
    while n < 4 && cursor.peek() == Some(b';') {
        cursor.next();
        if let Some(v) = cursor.number() {
            params[n] = v;
        }
        n += 1;
    }
    match n {
        // `;Pu;Px;Py` / `;Pu;Px;Py;Pz`
        4 => {
            if params[0] == 1 {
                hls_to_rgb(params[1], params[2], params[3])
            } else if params[0] == 2 {
                [
                    params[1].min(255) as u8,
                    params[2].min(255) as u8,
                    params[3].min(255) as u8,
                ]
            } else {
                [
                    scale_percent(params[1]),
                    scale_percent(params[2]),
                    scale_percent(params[3]),
                ]
            }
        }
        3 => [
            scale_percent(params[0]),
            scale_percent(params[1]),
            scale_percent(params[2]),
        ],
        _ => [0, 0, 0],
    }
}

/// Decode a sixel data stream into an RGBA image.
///
/// `max_w` / `max_h` clamp the output dimensions (callers should pass the
/// terminal's pixel viewport). Returns `None` when no pixels were drawn.
pub fn decode_sixel(data: &[u8], max_w: u32, max_h: u32) -> Option<SixelImage> {
    let max_w = max_w.max(1);
    let max_h = max_h.max(1);

    // Indexed palette; grows on demand when a color number is redefined.
    let mut palette: Vec<[u8; 3]> = DEFAULT_PALETTE.to_vec();
    let mut cur: [u8; 3] = palette[0];

    let mut cursor = Cursor::new(data);
    let mut x: u32 = 0;
    let mut y: u32 = 0;
    let mut ext_w: u32 = 0; // extent of actually drawn pixels
    let mut ext_h: u32 = 0;
    let mut raster_w: u32 = 0;
    let mut raster_h: u32 = 0;

    // Row-major RGBA buffer, grown on demand; unwritten pixels stay
    // transparent (0,0,0,0).
    let mut buf: Vec<u8> = Vec::new();
    let mut buf_w: u32 = 0;
    let mut buf_h: u32 = 0;

    while let Some(b) = cursor.next() {
        match b {
            // Space: advance one pixel column without drawing.
            0x20 => x = x.saturating_add(1),
            // `!N` — repeat the next character N times.
            0x21 => {
                let n = cursor.number().unwrap_or(1).min(MAX_REPEAT as u32);
                let repeat = n.max(1);
                match cursor.next() {
                    Some(nb) if (0x3f..=0x7e).contains(&nb) => {
                        ensure(
                            &mut buf,
                            &mut buf_w,
                            &mut buf_h,
                            x + repeat,
                            y + 6,
                            max_w,
                            max_h,
                        );
                        draw_mask(
                            nb, repeat, cur, &mut x, y, &mut ext_w, &mut ext_h, &mut buf, buf_w,
                            buf_h, max_w, max_h,
                        );
                    }
                    Some(0x20) => x = x.saturating_add(repeat),
                    Some(0x24) => x = 0,
                    Some(0x2d) => {
                        y = y.saturating_add(6 * repeat);
                        x = 0;
                    }
                    _ => {}
                }
            }
            // `"P1;P2;P3;P4` — raster attributes (DEC Sixel Graphics
            // Protocol, VT330/VT340 RM): Pn1/Pn2 are the pixel aspect ratio
            // (pan/pad, ignored here), Pn3 is the HORIZONTAL size in pixels
            // (width) and Pn4 the VERTICAL size (height). chafa/img2sixel
            // emit width first, e.g. `"1;1;200;120` for a 200x120 image.
            0x22 => {
                let _pan = cursor.number();
                if cursor.peek() == Some(b';') {
                    cursor.next();
                    let _pad = cursor.number();
                    if cursor.peek() == Some(b';') {
                        cursor.next();
                        if let Some(w) = cursor.number() {
                            raster_w = w;
                        }
                        if cursor.peek() == Some(b';') {
                            cursor.next();
                            if let Some(h) = cursor.number() {
                                raster_h = h;
                            }
                        }
                    }
                }
            }
            // `#Pc[;...]` — color select / define.
            0x23 => {
                if let Some(pc) = cursor.number() {
                    let color = if cursor.peek() == Some(b';') {
                        parse_inline_color(&mut cursor)
                    } else if (pc as usize) < palette.len() {
                        palette[pc as usize]
                    } else {
                        [0, 0, 0]
                    };
                    if palette.len() <= pc as usize {
                        palette.resize(pc as usize + 1, [0, 0, 0]);
                    }
                    palette[pc as usize] = color;
                    cur = color;
                }
                // `#P`/`#p`/`#q` palette push/pop forms: keep current color.
            }
            // `$` — carriage return: back to column 0 of the current band.
            0x24 => x = 0,
            // `-` — line feed: advance one band (6 px), reset column.
            0x2d => {
                y = y.saturating_add(6);
                x = 0;
            }
            // Sixel graphic characters.
            0x3f..=0x7e => {
                ensure(&mut buf, &mut buf_w, &mut buf_h, x + 1, y + 6, max_w, max_h);
                draw_mask(
                    b, 1, cur, &mut x, y, &mut ext_w, &mut ext_h, &mut buf, buf_w, buf_h, max_w,
                    max_h,
                );
            }
            // 0x25-0x2e etc.: ignored control characters.
            _ => {}
        }
    }

    if ext_w == 0 || ext_h == 0 {
        return None;
    }

    // Final size: prefer raster attributes when declared, else the drawn
    // extents; never exceed the caller's bounds.
    let w = if raster_w > 0 { raster_w } else { ext_w }.min(max_w);
    let h = if raster_h > 0 { raster_h } else { ext_h }.min(max_h);
    if w == 0 || h == 0 {
        return None;
    }

    // Pad the buffer up to the declared size (undrawn areas stay transparent)
    // so images whose encoder declares ph/pw but under-draws keep their size.
    if w > buf_w || h > buf_h {
        let mut padded = vec![0u8; (w as usize) * (h as usize) * 4];
        for r in 0..buf_h.min(h) {
            let src = (r * buf_w * 4) as usize;
            let dst = (r * w * 4) as usize;
            let len = (buf_w.min(w) * 4) as usize;
            padded[dst..dst + len].copy_from_slice(&buf[src..src + len]);
        }
        buf = padded;
        buf_w = w;
    }

    // Crop the top-left `w x h` region out of the buffer (a no-op when the
    // buffer already matches the final size).
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for r in 0..h {
        let src = (r * buf_w * 4) as usize;
        let len = (w * 4) as usize;
        rgba.extend_from_slice(&buf[src..src + len]);
    }

    Some(SixelImage {
        width: w,
        height: h,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(img: &SixelImage, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let i = ((y * img.width + x) * 4) as usize;
        (
            img.rgba[i],
            img.rgba[i + 1],
            img.rgba[i + 2],
            img.rgba[i + 3],
        )
    }

    #[test]
    fn decodes_single_black_pixel() {
        // `#0~` — color 0 (black), one sixel char (`~` = all six bands) = 1x6.
        let img = decode_sixel(b"#0~", 100, 100).expect("decodes");
        assert_eq!((img.width, img.height), (1, 6));
        assert_eq!(pixel(&img, 0, 0), (0, 0, 0, 255));
        assert_eq!(pixel(&img, 0, 5), (0, 0, 0, 255));
    }

    #[test]
    fn decodes_blue_stripe_with_repeat() {
        // `#1!10~` — color 1 (blue), repeated 10 times → 10x6 blue.
        let img = decode_sixel(b"#1!10~", 100, 100).expect("decodes");
        assert_eq!((img.width, img.height), (10, 6));
        assert_eq!(pixel(&img, 0, 0), (0x00, 0x00, 0xaa, 255));
        assert_eq!(pixel(&img, 9, 0), (0x00, 0x00, 0xaa, 255));
    }

    #[test]
    fn inline_rgb_percent_color() {
        // `#0;100;0;0~` — redefine color 0 as full red (percentages).
        let img = decode_sixel(b"#0;100;0;0~", 100, 100).expect("decodes");
        assert_eq!(pixel(&img, 0, 0), (255, 0, 0, 255));
    }

    #[test]
    fn inline_direct_rgb_color() {
        // `#0;2;255;128;0~` — direct 8-bit RGB form.
        let img = decode_sixel(b"#0;2;255;128;0~", 100, 100).expect("decodes");
        assert_eq!(pixel(&img, 0, 0), (255, 128, 0, 255));
    }

    #[test]
    fn inline_hls_color() {
        // `#0;1;180;50;100~` — HLS hue=180 (cyan), lightness 50%, sat 100%.
        let img = decode_sixel(b"#0;1;180;50;100~", 100, 100).expect("decodes");
        let (r, g, b, a) = pixel(&img, 0, 0);
        assert_eq!(a, 255);
        assert!(
            g > 200 && b > 200 && r < 60,
            "expected cyan, got ({r},{g},{b})"
        );
    }

    #[test]
    fn cr_and_lf_positioning() {
        // `#2~$~$~$~-~` — one char at band 0, LF, one char at band 1.
        let img = decode_sixel(b"#2~$~$~$~-~", 100, 100).expect("decodes");
        assert_eq!((img.width, img.height), (1, 12));
        assert_eq!(pixel(&img, 0, 0), (0, 0xaa, 0, 255)); // green at band 0
        assert_eq!(pixel(&img, 0, 6), (0, 0xaa, 0, 255)); // green at band 1
        assert_eq!(pixel(&img, 0, 11), (0, 0xaa, 0, 255));
    }

    #[test]
    fn raster_attributes_size_the_output() {
        // `"1;1;12;4` declares 12 wide x 4 tall (Pn3 = width, Pn4 = height
        // per the DEC spec); draw 4 columns of color 3, so the buffer pads
        // to the declared width with the drawn columns top-left.
        let img = decode_sixel(b"\"1;1;12;4#3!4~", 100, 100).expect("decodes");
        assert_eq!((img.width, img.height), (12, 4));
    }

    #[test]
    fn clamps_to_bounds() {
        // Huge repeat on a tiny output bound must not panic or explode memory.
        let img = decode_sixel(b"#1!999999999~", 8, 8).expect("decodes");
        assert_eq!(img.width, 8);
        assert_eq!(img.height, 6);
        assert_eq!(pixel(&img, 0, 0), (0, 0, 0xaa, 255));
    }

    #[test]
    fn grow_taller_narrower_does_not_panic() {
        // Fuzz-found: draw wide, then `$`/`-` reset the column and draw
        // narrow-but-taller, forcing the buffer to grow vertically while the
        // requested width is smaller than the current buffer.
        let img = decode_sixel(b"#1!20~$-!1~", 100, 100).expect("decodes");
        // The wide row survives a grow-taller-narrower step: the buffer must
        // not truncate to the narrower request and lose drawn pixels.
        assert_eq!((img.width, img.height), (20, 12));
        assert_eq!(pixel(&img, 0, 0), (0, 0, 0xaa, 255));
        assert_eq!(pixel(&img, 0, 6), (0, 0, 0xaa, 255));
        assert_eq!(pixel(&img, 0, 11), (0, 0, 0xaa, 255));
        assert_eq!(pixel(&img, 19, 0), (0, 0, 0xaa, 255));
    }

    #[test]
    fn empty_payload_is_none() {
        assert!(decode_sixel(b"", 100, 100).is_none());
        assert!(decode_sixel(b"$-$-", 100, 100).is_none());
    }

    #[test]
    fn all_bands_white() {
        // `~` = 0x7E → mask 0x3F → all six bands lit, color 15 (white).
        let img = decode_sixel(b"#15~", 100, 100).expect("decodes");
        assert_eq!((img.width, img.height), (1, 6));
        for band in 0..6 {
            assert_eq!(pixel(&img, 0, band), (255, 255, 255, 255));
        }
    }
}
