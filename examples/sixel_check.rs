//! Cross-validation helper: decodes a sixel payload file and dumps the
//! decoded RGBA image (raw bytes) to stdout, dimensions to stderr.
//!
//! Usage: sixel_check <payload-file> <max-w> <max-h>
//! Used by bench/sixel_validate.py — not a user-facing tool.

use std::io::{Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().expect("payload path");
    let max_w: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(800);
    let max_h: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(600);

    let mut data = Vec::new();
    std::fs::File::open(path)
        .expect("open payload")
        .read_to_end(&mut data)
        .expect("read payload");

    let Some(img) = terminal::sixel::decode_sixel(&data, max_w, max_h) else {
        eprintln!("DECODE_FAILED");
        std::process::exit(1);
    };
    eprintln!("SIZE {}x{}", img.width, img.height);
    let _ = std::io::stdout().write_all(&img.rgba);
}
