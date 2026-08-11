//! Headless VT conformance runner.
//!
//! This is the fast, deterministic layer of the verification plan. It feeds
//! original fixture sequences directly into the parser/grid seam and checks
//! observable terminal state without requiring a display server.
//!
//! Run with:
//!   cargo run --release --bin vt_conformance
//!   cargo run --release --bin vt_conformance -- --json

use terminal::grid::{Grid, WinSize};
use terminal::parser::Parser;

struct Case {
    name: &'static str,
    run: fn() -> Result<(), String>,
}

fn feed(grid: &mut Grid, bytes: &[u8]) {
    let mut parser = Parser::new();
    parser.advance_bytes(grid, bytes);
}

fn expect(condition: bool, message: impl Into<String>) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn basic_text() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 3 }, 32);
    feed(&mut grid, b"hello\r\nworld");
    expect(
        grid.line_to_string(0).starts_with("hello"),
        "line 0 missing hello",
    )?;
    expect(
        grid.line_to_string(1).starts_with("world"),
        "line 1 missing world",
    )?;
    expect(
        grid.cursor.row == 1 && grid.cursor.col == 5,
        "cursor mismatch",
    )
}

fn cpr_and_decom() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 8 }, 32);
    feed(&mut grid, b"\x1b[3;6r\x1b[?6h\x1b[2;4H\x1b[6n\x1b[?6n");
    let responses = grid.take_responses();
    expect(
        responses == vec![b"\x1b[2;4R".to_vec(), b"\x1b[?2;4;1R".to_vec()],
        format!("unexpected CPR responses: {responses:?}"),
    )
}

fn unicode_cluster() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 12, rows: 2 }, 32);
    feed(&mut grid, b"a\xcc\x81\xcc\x82\xe2\x80\x8d\xef\xb8\x8fb");
    expect(
        grid.line_to_string(0)
            .starts_with("a\u{301}\u{302}\u{200d}\u{fe0f}b"),
        "unicode cluster was not preserved",
    )?;
    expect(grid.cursor.col == 2, "cluster consumed more than one cell")
}

fn resize_reflow() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 6, rows: 3 }, 32);
    feed(&mut grid, b"abcdef\n123456\nuvwxyz\nlast");
    grid.resize(WinSize { cols: 4, rows: 3 });
    let text = grid.all_lines_with_scrollback().join("");
    expect(text.contains("abcdef"), "resize lost first line")?;
    expect(text.contains("123456"), "resize lost middle line")?;
    expect(text.contains("uvwxyz"), "resize lost final line")
}

fn protocols_and_styles() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 12, rows: 3 }, 32);
    feed(&mut grid, b"\x1b[>1u\x1b[>4;1m\x1b[4:2mX\x1b#6");
    expect(grid.kitty_keyboard, "Kitty keyboard negotiation failed")?;
    expect(
        grid.modify_other_keys == 1,
        "modifyOtherKeys negotiation failed",
    )?;
    expect(
        grid.cell(0, 0).attrs.underline_style() == 2,
        "underline style failed",
    )?;
    expect(grid.line_mode(0) == 6, "double-width mode failed")
}

fn osc_and_limits() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 12, rows: 3 }, 32);
    let mut input = b"\x1b]2;headless-title\x07".to_vec();
    input.extend(std::iter::repeat(b"x"[0]).take(110_000));
    feed(&mut grid, &input);
    expect(
        grid.palette.title == "headless-title",
        "OSC title was not dispatched",
    )
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "basic_text",
            run: basic_text,
        },
        Case {
            name: "cpr_and_decom",
            run: cpr_and_decom,
        },
        Case {
            name: "unicode_cluster",
            run: unicode_cluster,
        },
        Case {
            name: "resize_reflow",
            run: resize_reflow,
        },
        Case {
            name: "protocols_and_styles",
            run: protocols_and_styles,
        },
        Case {
            name: "osc_and_limits",
            run: osc_and_limits,
        },
    ]
}

fn main() {
    let json = std::env::args().any(|arg| arg == "--json");
    let mut failed = 0;
    let mut passed = 0;

    for case in cases() {
        match (case.run)() {
            Ok(()) => {
                passed += 1;
                if json {
                    println!("{{\"case\":\"{}\",\"status\":\"pass\"}}", case.name);
                } else {
                    println!("PASS {}", case.name);
                }
            }
            Err(error) => {
                failed += 1;
                if json {
                    println!(
                        "{{\"case\":\"{}\",\"status\":\"fail\",\"error\":{:?}}}",
                        case.name, error
                    );
                } else {
                    eprintln!("FAIL {}: {}", case.name, error);
                }
            }
        }
    }

    if !json {
        println!("Summary: {passed} passed, {failed} failed");
    }
    if failed != 0 {
        std::process::exit(1);
    }
}
