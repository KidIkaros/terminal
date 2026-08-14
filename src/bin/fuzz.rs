//! Deterministic fuzz harness for the VT parser/grid seam.
//!
//! Feeds byte streams from a seeded LCG through the parser into a grid and
//! asserts the invariants that must hold after *any* input: no panic, the
//! cursor stays in bounds, and scrollback never exceeds its capacity. The
//! same seed reproduces the same stream, so failures are reproducible.
//!
//! Usage:
//!   cargo run --release --bin fuzz -- --seeds 8 --bytes 8M
//!   cargo run --release --bin fuzz -- --quick     # small smoke run (CI)
//!
//! A small multi-seed smoke run also runs under `cargo test --bin fuzz`.

use std::time::Instant;

use terminal::grid::{Grid, WinSize};
use terminal::parser::Parser;

/// SplitMix64 — tiny deterministic PRNG (no external deps).
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn next_u8(&mut self) -> u8 {
        (self.next_u64() >> 56) as u8
    }
}

/// Byte-generation modes: uniform random, escape-heavy, printable text, and
/// CSI-structured soup (digits/semicolons/ESC). Each stresses a different
/// parser path.
#[inline]
fn gen_byte(mode: u8, rng: &mut Rng) -> u8 {
    match mode {
        0 => rng.next_u8(), // uniform — worst case, invalid UTF-8 throughout
        1 => match rng.next_u8() % 4 {
            // escape-heavy: ESC, CSI, DCS, OSC intro bytes plus noise
            0 => 0x1b,
            1 => 0x9b,
            2 => 0x90,
            _ => rng.next_u8(),
        },
        2 => 0x20 + (rng.next_u8() % 95), // printable ASCII
        _ => match rng.next_u8() % 4 {
            // structured: CSI parameter soup
            0 => b'0' + rng.next_u8() % 10,
            1 => b';',
            2 => 0x1b,
            _ => rng.next_u8() % 0x80,
        },
    }
}

/// Assert grid invariants that must hold after any byte stream.
fn check_invariants(grid: &Grid, seed: u64, bytes: u64) -> Result<(), String> {
    if grid.cursor.row >= grid.rows {
        return Err(format!(
            "seed {seed} after {bytes} bytes: cursor row {} out of bounds ({})",
            grid.cursor.row, grid.rows
        ));
    }
    if grid.cursor.col > grid.cols {
        return Err(format!(
            "seed {seed} after {bytes} bytes: cursor col {} out of bounds ({})",
            grid.cursor.col, grid.cols
        ));
    }
    if grid.scrollback.len() > grid.scrollback_capacity {
        return Err(format!(
            "seed {seed} after {bytes} bytes: scrollback {} > capacity {}",
            grid.scrollback.len(),
            grid.scrollback_capacity
        ));
    }
    if grid.sixel_images.len() > 64 {
        return Err(format!(
            "seed {seed} after {bytes} bytes: sixel image backlog grew unbounded"
        ));
    }
    Ok(())
}

/// Run one seed: `bytes` bytes of the given mode through a fresh parser/grid.
fn run_seed(seed: u64, mode: u8, bytes: usize, cols: u16, rows: u16) -> Result<(), String> {
    let mut rng = Rng(seed ^ 0x5EED_5EED);
    let mut grid = Grid::new(WinSize { cols, rows }, 4096);
    let mut parser = Parser::new();
    let mut processed: usize = 0;

    let mut chunk = Vec::with_capacity(8192);
    while processed < bytes {
        let take = (bytes - processed).min(8192);
        chunk.clear();
        for _ in 0..take {
            chunk.push(gen_byte(mode, &mut rng));
        }
        parser.advance_bytes(&mut grid, &chunk);
        processed += take;
        check_invariants(&grid, seed, processed as u64)?;
    }
    Ok(())
}

/// Run all four modes for `seeds` seeds each.
fn run_all(seeds: u64, bytes: usize, cols: u16, rows: u16) -> Result<u64, String> {
    let mut total = 0u64;
    let modes = ["uniform", "escape-heavy", "printable", "csi-soup"];
    for (mode_idx, mode_name) in modes.iter().enumerate() {
        for seed in 0..seeds {
            let t = Instant::now();
            run_seed(seed, mode_idx as u8, bytes, cols, rows)
                .map_err(|e| format!("mode {mode_name} seed {seed}: {e}"))?;
            let secs = t.elapsed().as_secs_f64();
            let mib = bytes as f64 / (1024.0 * 1024.0);
            eprintln!(
                "  [{mode_name:>12}] seed {seed}: {mib:.1} MiB in {secs:.2}s ({:.1} MiB/s)",
                mib / secs
            );
            total += bytes as u64;
        }
    }
    Ok(total)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut seeds: u64 = 8;
    let mut bytes: usize = 8 * 1024 * 1024;
    let mut quick = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--seeds" => {
                i += 1;
                if i < args.len() {
                    seeds = args[i].parse().unwrap_or(seeds);
                }
            }
            "--bytes" => {
                i += 1;
                if i < args.len() {
                    let s = &args[i];
                    bytes = if let Some(mib) = s.strip_suffix("M").or_else(|| s.strip_suffix("m")) {
                        mib.parse::<usize>().unwrap_or(8) * 1024 * 1024
                    } else {
                        s.parse().unwrap_or(bytes)
                    };
                }
            }
            "--quick" => quick = true,
            _ => {}
        }
        i += 1;
    }
    if quick {
        seeds = 2;
        bytes = 256 * 1024;
    }

    eprintln!("=== Parser fuzz harness ===");
    eprintln!("seeds={seeds} bytes/seed={bytes} modes=4");
    let t = Instant::now();
    match run_all(seeds, bytes, 80, 24) {
        Ok(total) => {
            let secs = t.elapsed().as_secs_f64();
            let mib = total as f64 / (1024.0 * 1024.0);
            println!(
                "OK: {total} bytes across {seeds}x4 seeds, no invariant violations ({secs:.2}s, {:.1} MiB/s)",
                mib / secs.max(1e-9)
            );
        }
        Err(e) => {
            eprintln!("FAIL: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_run_does_not_violate_invariants() {
        // Small deterministic smoke run covering all four modes.
        for mode in 0..4u8 {
            run_seed(0xABCD, mode, 64 * 1024, 80, 24).expect("invariants hold");
        }
    }

    #[test]
    fn extreme_grid_sizes_do_not_panic() {
        // Tiny and large grids under random input.
        for (cols, rows) in [(1, 1), (200, 100)] {
            run_seed(42, 0, 32 * 1024, cols, rows).expect("invariants hold");
        }
    }
}
