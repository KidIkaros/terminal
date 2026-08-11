//! Terminal emulator benchmark harness.
//!
//! Measures parser+grid throughput in-process (no GPU, no winit, no PTY).
//! This isolates the CPU-side cost of the VT parser and grid update path
//! from rendering and I/O, matching the "headless throughput" tier of the
//! benchmark plan.
//!
//! Usage:
//!   cargo run --release --bin bench -- [options]
//!
//! Options:
//!   --cols N          Terminal width in columns (default 80)
//!   --rows N          Terminal height in rows (default 24)
//!   --scrollback N    Scrollback lines (default 500)
//!   --ascii-mib N     Printable ASCII payload size in MiB (default 100)
//!   --random-mib N    Random-byte payload size in MiB (default 10)
//!   --runs N          Number of repetitions per benchmark (default 3)
//!   --bulk-output     Defer dirty-cell marking until each batch completes
//!   --json            Output results as JSON
//!   --label TEXT      Label for this run (e.g. commit hash)

use std::time::Instant;

use terminal::grid::{Grid, WinSize};
use terminal::parser::Parser;

// ---------------------------------------------------------------------------
// Payload generation
// ---------------------------------------------------------------------------

/// Generate `mib` MiB of printable ASCII (0x20..0x7e, plus occasional newlines
/// to exercise scroll). Matches the shape of the shitty reference workload:
/// dense printable text that fills the grid and triggers scrollback eviction.
fn generate_ascii_payload(mib: usize) -> Vec<u8> {
    let len = mib * 1024 * 1024;
    let mut buf = Vec::with_capacity(len);
    // Use a simple LCG for reproducibility without external deps.
    let mut state: u32 = 0x12345678;
    for _ in 0..len {
        state = state.wrapping_mul(1103515245).wrapping_add(12345);
        let val = (state >> 16) & 0xFF;
        // Map to printable ASCII range 0x20..0x7e (95 chars)
        let ch = 0x20 + (val % 95) as u8;
        buf.push(ch);
        // Insert a newline every ~100 chars to trigger line wraps and scroll
        if buf.len() % 100 == 0 {
            buf.push(b'\n');
        }
    }
    buf
}

/// Generate `mib` MiB of random bytes (0x00..0xFF) to stress the parser's
/// handling of invalid UTF-8, control sequences, and edge cases.
fn generate_random_payload(mib: usize) -> Vec<u8> {
    let len = mib * 1024 * 1024;
    let mut buf = Vec::with_capacity(len);
    let mut state: u32 = 0xDEADBEEF;
    for _ in 0..len {
        state = state.wrapping_mul(1103515245).wrapping_add(12345);
        buf.push((state >> 16) as u8);
    }
    buf
}

// ---------------------------------------------------------------------------
// Benchmark core
// ---------------------------------------------------------------------------

struct BenchResult {
    name: String,
    bytes: usize,
    elapsed_secs: f64,
    mib_per_sec: f64,
}

fn bench_parser_grid(
    name: &str,
    payload: &[u8],
    cols: u16,
    rows: u16,
    scrollback: usize,
    bulk_output: bool,
) -> BenchResult {
    let size = WinSize { cols, rows };
    let mut grid = Grid::new(size, scrollback);
    let mut parser = Parser::new();

    grid.bulk_output = bulk_output;

    let start = Instant::now();
    parser.advance_bytes(&mut grid, payload);
    if bulk_output {
        grid.mark_all_dirty();
    }
    let elapsed = start.elapsed();

    let bytes = payload.len();
    let secs = elapsed.as_secs_f64();
    let mib = bytes as f64 / (1024.0 * 1024.0);
    let mib_per_sec = if secs > 0.0 { mib / secs } else { 0.0 };

    BenchResult {
        name: name.to_string(),
        bytes,
        elapsed_secs: secs,
        mib_per_sec,
    }
}

fn bench_parser_grid_per_byte(
    name: &str,
    payload: &[u8],
    cols: u16,
    rows: u16,
    scrollback: usize,
) -> BenchResult {
    let size = WinSize { cols, rows };
    let mut grid = Grid::new(size, scrollback);
    let mut parser = Parser::new();

    let start = Instant::now();
    for &byte in payload {
        parser.advance(&mut grid, byte);
    }
    let elapsed = start.elapsed();

    let bytes = payload.len();
    let secs = elapsed.as_secs_f64();
    let mib = bytes as f64 / (1024.0 * 1024.0);
    let mib_per_sec = if secs > 0.0 { mib / secs } else { 0.0 };

    BenchResult {
        name: name.to_string(),
        bytes,
        elapsed_secs: secs,
        mib_per_sec,
    }
}

fn run_bench(
    name: &str,
    payload: &[u8],
    cols: u16,
    rows: u16,
    scrollback: usize,
    runs: usize,
    bulk_output: bool,
) -> Vec<BenchResult> {
    let mut results = Vec::with_capacity(runs);
    for i in 0..runs {
        let label = format!("{} run {}/{}", name, i + 1, runs);
        let r = bench_parser_grid(&label, payload, cols, rows, scrollback, bulk_output);
        eprintln!(
            "  {} — {:.2} MiB/s  ({:.3}s, {} bytes)",
            r.name, r.mib_per_sec, r.elapsed_secs, r.bytes
        );
        results.push(r);
    }
    results
}

fn run_bench_per_byte(
    name: &str,
    payload: &[u8],
    cols: u16,
    rows: u16,
    scrollback: usize,
    runs: usize,
) -> Vec<BenchResult> {
    let mut results = Vec::with_capacity(runs);
    for i in 0..runs {
        let label = format!("{} run {}/{}", name, i + 1, runs);
        let r = bench_parser_grid_per_byte(&label, payload, cols, rows, scrollback);
        eprintln!(
            "  {} — {:.2} MiB/s  ({:.3}s, {} bytes)",
            r.name, r.mib_per_sec, r.elapsed_secs, r.bytes
        );
        results.push(r);
    }
    results
}

fn median(values: &[f64]) -> f64 {
    let mut sorted: Vec<f64> = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = sorted.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

fn variance(values: &[f64], med: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let sum: f64 = values.iter().map(|v| (v - med).powi(2)).sum();
    sum / values.len() as f64
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

fn parse_args(args: &[String]) -> clap_like::Args {
    let mut a = clap_like::Args::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cols" => {
                i += 1;
                a.cols = args[i].parse().unwrap_or(a.cols);
            }
            "--rows" => {
                i += 1;
                a.rows = args[i].parse().unwrap_or(a.rows);
            }
            "--scrollback" => {
                i += 1;
                a.scrollback = args[i].parse().unwrap_or(a.scrollback);
            }
            "--ascii-mib" => {
                i += 1;
                a.ascii_mib = args[i].parse().unwrap_or(a.ascii_mib);
            }
            "--random-mib" => {
                i += 1;
                a.random_mib = args[i].parse().unwrap_or(a.random_mib);
            }
            "--runs" => {
                i += 1;
                a.runs = args[i].parse().unwrap_or(a.runs);
            }
            "--bulk-output" => a.bulk_output = true,
            "--json" => a.json = true,
            "--label" => {
                i += 1;
                a.label = args[i].clone();
            }
            _ => {}
        }
        i += 1;
    }
    a
}

mod clap_like {
    pub struct Args {
        pub cols: u16,
        pub rows: u16,
        pub scrollback: usize,
        pub ascii_mib: usize,
        pub random_mib: usize,
        pub runs: usize,
        pub bulk_output: bool,
        pub json: bool,
        pub label: String,
    }

    impl Default for Args {
        fn default() -> Self {
            Args {
                cols: 80,
                rows: 24,
                scrollback: 500,
                ascii_mib: 100,
                random_mib: 10,
                runs: 3,
                bulk_output: false,
                json: false,
                label: String::new(),
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = parse_args(&args);

    eprintln!("=== Terminal parser/grid benchmark ===");
    eprintln!(
        "Grid: {}x{}, scrollback: {} lines",
        opts.cols, opts.rows, opts.scrollback
    );
    eprintln!(
        "ASCII payload: {} MiB, random payload: {} MiB, runs: {}, bulk output: {}",
        opts.ascii_mib, opts.random_mib, opts.runs, opts.bulk_output
    );
    if !opts.label.is_empty() {
        eprintln!("Label: {}", opts.label);
    }
    eprintln!();

    // Generate payloads
    eprintln!("Generating ASCII payload ({} MiB)...", opts.ascii_mib);
    let ascii_payload = generate_ascii_payload(opts.ascii_mib);
    eprintln!("Generating random payload ({} MiB)...", opts.random_mib);
    let random_payload = generate_random_payload(opts.random_mib);
    eprintln!();

    // Run benchmarks — batch (advance_bytes) path
    eprintln!("[1/4] Printable ASCII throughput (batch):");
    let ascii_results = run_bench(
        "ascii-batch",
        &ascii_payload,
        opts.cols,
        opts.rows,
        opts.scrollback,
        opts.runs,
        opts.bulk_output,
    );
    eprintln!();

    eprintln!("[2/4] Random-byte throughput (batch):");
    let random_results = run_bench(
        "random-batch",
        &random_payload,
        opts.cols,
        opts.rows,
        opts.scrollback,
        opts.runs,
        opts.bulk_output,
    );
    eprintln!();

    // Per-byte comparison (to measure the speedup from batching)
    eprintln!("[3/4] Printable ASCII throughput (per-byte):");
    let ascii_per_byte = run_bench_per_byte(
        "ascii-perbyte",
        &ascii_payload,
        opts.cols,
        opts.rows,
        opts.scrollback,
        opts.runs,
    );
    eprintln!();

    eprintln!("[4/4] Random-byte throughput (per-byte):");
    let random_per_byte = run_bench_per_byte(
        "random-perbyte",
        &random_payload,
        opts.cols,
        opts.rows,
        opts.scrollback,
        opts.runs,
    );
    eprintln!();

    // Summary
    let ascii_mibs: Vec<f64> = ascii_results.iter().map(|r| r.mib_per_sec).collect();
    let random_mibs: Vec<f64> = random_results.iter().map(|r| r.mib_per_sec).collect();
    let ascii_pb_mibs: Vec<f64> = ascii_per_byte.iter().map(|r| r.mib_per_sec).collect();
    let random_pb_mibs: Vec<f64> = random_per_byte.iter().map(|r| r.mib_per_sec).collect();

    let ascii_med = median(&ascii_mibs);
    let ascii_var = variance(&ascii_mibs, ascii_med);
    let random_med = median(&random_mibs);
    let random_var = variance(&random_mibs, random_med);
    let ascii_pb_med = median(&ascii_pb_mibs);
    let random_pb_med = median(&random_pb_mibs);

    if opts.json {
        println!("{{");
        println!("  \"label\": \"{}\",", opts.label);
        println!("  \"cols\": {},", opts.cols);
        println!("  \"rows\": {},", opts.rows);
        println!("  \"scrollback\": {},", opts.scrollback);
        println!("  \"ascii_batch\": {{");
        println!("    \"mib_per_sec_median\": {:.2},", ascii_med);
        println!("    \"variance\": {:.4},", ascii_var);
        println!(
            "    \"runs\": [{}]",
            ascii_mibs
                .iter()
                .map(|v| format!("{:.2}", v))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("  }},");
        println!("  \"random_batch\": {{");
        println!("    \"mib_per_sec_median\": {:.2},", random_med);
        println!("    \"variance\": {:.4},", random_var);
        println!(
            "    \"runs\": [{}]",
            random_mibs
                .iter()
                .map(|v| format!("{:.2}", v))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("  }},");
        println!("  \"ascii_per_byte\": {{");
        println!("    \"mib_per_sec_median\": {:.2},", ascii_pb_med);
        println!(
            "    \"runs\": [{}]",
            ascii_pb_mibs
                .iter()
                .map(|v| format!("{:.2}", v))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("  }},");
        println!("  \"random_per_byte\": {{");
        println!("    \"mib_per_sec_median\": {:.2},", random_pb_med);
        println!(
            "    \"runs\": [{}]",
            random_pb_mibs
                .iter()
                .map(|v| format!("{:.2}", v))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("  }}");
        println!("}}");
    } else {
        eprintln!("=== Summary ===");
        eprintln!(
            "ASCII  batch:   median {:.2} MiB/s  (variance {:.4})",
            ascii_med, ascii_var
        );
        eprintln!(
            "Random batch:   median {:.2} MiB/s  (variance {:.4})",
            random_med, random_var
        );
        eprintln!("ASCII  per-byte: median {:.2} MiB/s", ascii_pb_med);
        eprintln!("Random per-byte: median {:.2} MiB/s", random_pb_med);
        if ascii_pb_med > 0.0 {
            eprintln!("Batch speedup (ASCII):  {:.1}x", ascii_med / ascii_pb_med);
        }
    }
}
