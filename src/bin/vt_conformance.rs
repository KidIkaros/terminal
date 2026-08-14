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
    expect(
        grid.kitty_flags & 0b1 != 0,
        "Kitty keyboard negotiation failed",
    )?;
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

fn kitty_keyboard_protocol() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 12, rows: 3 }, 32);
    // Quickstart push: disambiguate.
    feed(&mut grid, b"\x1b[>1u");
    expect(
        grid.kitty_flags & 0b1 != 0,
        "push did not enable disambiguation",
    )?;
    // Progressive enhancement set with mode semantics (set event-types bit).
    feed(&mut grid, b"\x1b[=2;2u");
    expect(
        grid.kitty_flags == 0b1 | 0b10,
        "mode 2 did not set event-types bit",
    )?;
    // Query replies with current flags.
    feed(&mut grid, b"\x1b[?u");
    expect(
        grid.take_responses() == vec![b"\x1b[?3u".to_vec()],
        "flag query reply mismatch",
    )?;
    // Pop restores the previous state.
    feed(&mut grid, b"\x1b[<u");
    expect(grid.kitty_flags == 0, "pop did not restore flags")
}

fn kitty_graphics_roundtrip() -> Result<(), String> {
    use base64::Engine as _;
    let mut grid = Grid::new(WinSize { cols: 40, rows: 12 }, 32);
    // Transmit-only (a=t) stores under id 5 and replies OK; no display.
    let raw = [255u8, 0, 0, 255];
    let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
    feed(
        &mut grid,
        format!("\x1b_Ga=t,i=5,f=32,s=1,v=1;{b64}\x1b\\").as_bytes(),
    );
    expect(grid.sixel_images.is_empty(), "a=t must not display")?;
    expect(
        grid.take_responses() == vec![b"\x1b_Gi=5;OK\x1b\\".to_vec()],
        "a=t OK reply mismatch",
    )?;
    // Put (a=p) displays the stored image and replies OK.
    feed(&mut grid, b"\x1b_Ga=p,i=5;\x1b\\");
    expect(grid.sixel_images.len() == 1, "a=p did not display")?;
    expect(
        grid.sixel_images[0].image_id == 5,
        "placement image_id mismatch",
    )?;
    expect(
        grid.take_responses() == vec![b"\x1b_Gi=5;OK\x1b\\".to_vec()],
        "a=p OK reply mismatch",
    )?;
    // Unknown put replies ENOENT.
    feed(&mut grid, b"\x1b_Ga=p,i=999;\x1b\\");
    expect(
        grid.take_responses() == vec![b"\x1b_Gi=999;ENOENT:no such image\x1b\\".to_vec()],
        "a=p ENOENT reply mismatch",
    )?;
    // Delete evicts the image and its placement.
    feed(&mut grid, b"\x1b_Ga=d,d=I,i=5;\x1b\\");
    expect(grid.sixel_images.is_empty(), "a=d did not evict placement")
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

fn shell_integration_markers() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 20, rows: 6 }, 32);
    feed(
        &mut grid,
        b"\x1b]133;A\x07prompt>\r\n\x1b]133;B\x07ls\r\n\x1b]133;C\x07out",
    );
    let stream: Vec<u8> = grid.marker_stream().collect();
    expect(stream[0] == 1, "row 0 should be a prompt marker")?;
    expect(stream[1] == 2, "row 1 should be a command marker")?;
    expect(stream[2] == 3, "row 2 should be an output marker")?;
    // Prev/next navigation over the stream.
    expect(grid.prev_prompt(3) == Some(0), "prev_prompt failed")?;
    expect(grid.next_prompt(0) == None, "next_prompt found a ghost")?;
    // OSC 7 cwd + OSC 9 notification.
    feed(&mut grid, b"\x1b]7;file:///srv\x07");
    expect(grid.cwd.as_deref() == Some("file:///srv"), "OSC 7 cwd lost")?;
    feed(&mut grid, b"\x1b]9;task done\x07");
    expect(
        grid.take_notification().as_deref() == Some("task done"),
        "OSC 9 notification lost",
    )
}

fn in_band_resize_2048() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 5 }, 32);
    feed(&mut grid, b"\x1b[?2048h");
    expect(grid.in_band_resize, "mode 2048 not set")?;
    grid.resize_report();
    expect(
        grid.take_responses() == vec![b"\x1b[4;5;10t".to_vec()],
        "in-band resize report mismatch",
    )?;
    feed(&mut grid, b"\x1b[?2048$p");
    expect(
        grid.take_responses() == vec![b"\x1b[?2048;1$y".to_vec()],
        "DECRQM for 2048 mismatch",
    )
}

fn sixel_decode_and_place() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 40, rows: 10 }, 32);
    grid.set_cell_size(8, 16);
    // A 2x12 blue image: two bands of a 2-wide run, then LF + repeat.
    // Raster attrs declare width 2, height 12 (Pn3 = width per DEC spec).
    feed(&mut grid, b"\x1bPq\"1;1;2;12#1!2~-!2~\x1b\\");
    expect(grid.sixel_images.len() == 1, "sixel image not placed")?;
    let img = &grid.sixel_images[0];
    expect(img.col == 0 && img.row == 0, "sixel misplaced")?;
    expect(
        img.image.width == 2 && img.image.height == 12,
        "sixel dimensions wrong",
    )?;
    // Cursor advanced below the image (12px / 16px cell → 1 row).
    expect(
        grid.cursor.row == 1 && grid.cursor.col == 0,
        "cursor not advanced",
    )?;
    // The RGBA payload is non-trivial: 2x12 with alpha set on drawn pixels.
    expect(
        img.image.rgba[3] == 0xff && img.image.rgba[7] == 0xff,
        "sixel pixels not drawn",
    )
}

fn decawm_off_clamps() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 5, rows: 2 }, 32);
    feed(&mut grid, b"\x1b[?7labcdefgh");
    expect(
        grid.cursor.col == 5 && grid.cursor.row == 0,
        "autowrap-off should clamp at the edge, not wrap",
    )?;
    // Each char past the edge overwrites the last cell (xterm behaviour).
    expect(grid.cell(4, 0).ch == 'h', "overwrite clamp wrong")
}

fn rep_repeats_last_char() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 2 }, 32);
    feed(&mut grid, b"ab\x1b[4b");
    expect(grid.cursor.col == 6, "REP should repeat 4 times")?;
    expect(
        grid.line_to_string(0).starts_with("abbbbb"),
        "REP characters wrong",
    )
}

fn ich_dch_ech() -> Result<(), String> {
    // ICH: insert blanks at the cursor, pushing text right.
    let mut g1 = Grid::new(WinSize { cols: 8, rows: 2 }, 32);
    feed(&mut g1, b"abcdef\x1b[4D\x1b[2@");
    expect(
        g1.line_to_string(0) == "ab  cdef",
        format!("ICH shift wrong: {:?}", g1.line_to_string(0)),
    )?;

    // DCH: delete chars at the cursor, pulling text left.
    let mut g2 = Grid::new(WinSize { cols: 8, rows: 2 }, 32);
    feed(&mut g2, b"abcdef\x1b[4D\x1b[2P");
    expect(
        g2.line_to_string(0) == "abef    ",
        format!("DCH shift wrong: {:?}", g2.line_to_string(0)),
    )?;

    // ECH: blank chars in place (no shift).
    let mut g3 = Grid::new(WinSize { cols: 8, rows: 2 }, 32);
    feed(&mut g3, b"abcdef\x1b[4D\x1b[2X");
    expect(
        g3.line_to_string(0) == "ab  ef  ",
        format!("ECH blanking wrong: {:?}", g3.line_to_string(0)),
    )
}

fn scroll_region_confines() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 5, rows: 6 }, 32);
    feed(&mut grid, b"\x1b[2;5r"); // region = rows 2..5 (1-indexed)
    feed(
        &mut grid,
        b"11111\r\n22222\r\n33333\r\n44444\r\n55555\r\n66666\r\n77777\r\n88888",
    );
    expect(
        grid.line_to_string(0).starts_with("11111"),
        "row outside the region scrolled",
    )?;
    expect(
        grid.line_to_string(1).starts_with("55555"),
        "region top lost its oldest line",
    )?;
    expect(
        grid.line_to_string(4).starts_with("88888"),
        "region bottom missing newest line",
    )
}

fn utf8_and_invalid_bytes() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 2 }, 32);
    // Valid multibyte, then a bad lead byte, a stray continuation, and a C1
    // CSI (0x9B is the whole 8-bit CSI introducer — equivalent to ESC [).
    feed(&mut grid, b"caf\xc3\xa9 \xff\x80\x9b1;1H");
    expect(
        grid.line_to_string(0).starts_with("caf\u{e9}"),
        "valid UTF-8 lost next to invalid bytes",
    )?;
    expect(
        grid.cursor.col == 0 && grid.cursor.row == 0,
        "cursor corrupted by invalid bytes",
    )
}

fn osc52_clipboard() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 2 }, 32);
    // OSC 52 ; clipboard ; base64("hi") = aGk=
    feed(&mut grid, b"\x1b]52;c;aGk=\x07");
    expect(
        grid.clipboard_set.as_deref() == Some("hi"),
        "OSC 52 set payload lost",
    )?;
    feed(&mut grid, b"\x1b]52;c;?\x07");
    expect(grid.clipboard_query_requested, "OSC 52 query not flagged")
}

fn cursor_shapes_and_paste() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 2 }, 32);
    feed(&mut grid, b"\x1b[5 q");
    expect(grid.cursor_shape == 5, "DECSCUR bar-blink not set")?;
    feed(&mut grid, b"\x1b[?2004h");
    expect(grid.bracketed_paste, "DECSET 2004 not set")?;
    feed(&mut grid, b"\x1b[?2004l");
    expect(!grid.bracketed_paste, "DECRST 2004 not cleared")
}

fn pending_wrap_then_wrap() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 5, rows: 2 }, 32);
    feed(&mut grid, b"12345");
    expect(
        grid.cursor.col == 5,
        "cursor should sit at the pending wrap",
    )?;
    feed(&mut grid, b"X");
    expect(
        grid.cursor.row == 1 && grid.cursor.col == 1,
        "pending wrap did not fire on next char",
    )?;
    expect(grid.cell(0, 1).ch == 'X', "wrapped char landed wrong")
}

fn decrqm_ansi_and_private() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 2 }, 32);
    // ANSI mode 7 (autowrap, default on) and private mode 2026 (default off).
    feed(&mut grid, b"\x1b[7$p");
    expect(
        grid.take_responses() == vec![b"\x1b[7;1$y".to_vec()],
        "DECRQM ANSI mode 7 mismatch",
    )?;
    feed(&mut grid, b"\x1b[?2026$p");
    expect(
        grid.take_responses() == vec![b"\x1b[?2026;2$y".to_vec()],
        "DECRQM private mode 2026 mismatch",
    )
}

fn osc8_hyperlink() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 20, rows: 2 }, 32);
    feed(
        &mut grid,
        b"\x1b]8;;https://example.com\x07link\x1b]8;;\x07next",
    );
    expect(
        grid.get_hyperlink_at(0, 0) == Some("https://example.com"),
        "OSC 8 link url lost",
    )?;
    expect(
        grid.get_hyperlink_at(4, 0).is_none(),
        "OSC 8 close did not clear the link",
    )
}

fn irm_insert_mode() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 2 }, 32);
    // Type "abc", enable IRM, back up 2, then type: existing cells shift
    // right instead of being overwritten.
    feed(&mut grid, b"abc\x1b[4h\x1b[2DXYZ");
    expect(grid.insert_mode, "IRM (mode 4) not set")?;
    expect(
        grid.line_to_string(0) == "aXYZbc    ",
        format!("IRM insert wrong: {:?}", grid.line_to_string(0)),
    )?;
    feed(&mut grid, b"\x1b[4l");
    expect(!grid.insert_mode, "IRM (mode 4) not reset")
}

fn decsc_decrc_restore() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 4 }, 32);
    feed(&mut grid, b"ab"); // cursor at (col 2, row 0)
    feed(&mut grid, b"\x1b7"); // DECSC: save cursor
    feed(&mut grid, b"\x1b[3;1HXY"); // draw elsewhere
    expect(grid.cell(0, 2).ch == 'X', "DECSC detour text lost")?;
    feed(&mut grid, b"\x1b8"); // DECRC: restore cursor
    feed(&mut grid, b"Z");
    expect(
        grid.line_to_string(0) == "abZ       ",
        format!("DECRC restore wrong: {:?}", grid.line_to_string(0)),
    )?;
    expect(
        grid.cursor.col == 3 && grid.cursor.row == 0,
        "DECRC cursor position not restored",
    )
}

fn sgr_truecolor_colon_and_semicolon() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 2 }, 32);
    // Colon form (38:2::r:g:b) and classic semicolon form (38;2;r;g;b).
    feed(&mut grid, b"\x1b[38:2::12:34:56mA");
    expect(
        grid.cell(0, 0).fg == terminal::grid::Color::Rgb(12, 34, 56),
        "colon truecolor fg wrong",
    )?;
    feed(&mut grid, b"\x1b[38;2;200;100;50mB");
    expect(
        grid.cell(1, 0).fg == terminal::grid::Color::Rgb(200, 100, 50),
        "semicolon truecolor fg wrong",
    )
}

fn cht_cbt_tabs() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 40, rows: 2 }, 32);
    feed(&mut grid, b"\x1b[2I");
    expect(grid.cursor.col == 16, "CHT(2) should land on tab 16")?;
    feed(&mut grid, b"\x1b[2Z");
    expect(grid.cursor.col == 0, "CBT(2) should land on tab 0")?;
    feed(&mut grid, b"\x1b[3I");
    expect(grid.cursor.col == 24, "CHT(3) should land on tab 24")
}

fn dectcem_hide_show() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 2 }, 32);
    feed(&mut grid, b"\x1b[?25l");
    expect(!grid.cursor_visible, "DECTCEM hide failed")?;
    feed(&mut grid, b"\x1b[?25h");
    expect(grid.cursor_visible, "DECTCEM show failed")
}

fn decom_origin_mode() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 6 }, 32);
    feed(&mut grid, b"\x1b[3;5r\x1b[?6h\x1b[1;1H");
    // DECOM: CUP is relative to the region top (0-based row 2), not row 0.
    expect(
        grid.cursor.row == 2 && grid.cursor.col == 0,
        "DECOM CUP 1;1 not relative to region",
    )?;
    // Row 6 relative clamps to the region bottom (0-based row 4).
    feed(&mut grid, b"\x1b[6;1H");
    expect(
        grid.cursor.row == 4 && grid.cursor.col == 0,
        "DECOM CUP past region end did not clamp",
    )?;
    feed(&mut grid, b"\x1b[?6l\x1b[1;1H");
    expect(
        grid.cursor.row == 0 && grid.cursor.col == 0,
        "DECRST origin mode did not restore absolute CUP",
    )
}

// -- Clean-room fixtures (spec: vt100.net DEC private sequences / VT220 RM) --
// These mirror behaviors esctest2 exercises via its PTY oracle, written from
// the DEC specs rather than copied from that GPL suite.

fn decstr_soft_reset() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 6 }, 32);
    feed(&mut grid, b"\x1b[?7l\x1b[4h\x1b[5;1Hxy");
    feed(&mut grid, b"\x1b[!p"); // DECSTR
    expect(grid.autowrap, "DECSTR did not restore autowrap")?;
    expect(!grid.insert_mode, "DECSTR did not clear insert mode")?;
    expect(
        grid.cursor.row == 0 && grid.cursor.col == 0,
        "DECSTR did not home the cursor",
    )?;
    expect(
        grid.line_to_string(4) == "          ",
        "DECSTR did not clear the screen",
    )
}

fn decreqtparm_report() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 3 }, 32);
    feed(&mut grid, b"\x1b[x");
    expect(
        grid.take_responses() == vec![b"\x1b[2;1;1;112;112;1;0x".to_vec()],
        "DECREQTPARM request 0 reply wrong",
    )?;
    feed(&mut grid, b"\x1b[1x");
    expect(
        grid.take_responses() == vec![b"\x1b[3;1;1;112;112;1;0x".to_vec()],
        "DECREQTPARM request 1 reply wrong",
    )
}

fn deccolm_switches_width() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 40, rows: 4 }, 32);
    feed(&mut grid, b"hello\x1b[2;3H");
    feed(&mut grid, b"\x1b[?3h"); // set → 80 columns
    expect(grid.cols == 80, "DECCOLM set did not widen to 80")?;
    expect(
        grid.cursor.row == 0 && grid.cursor.col == 0,
        "DECCOLM did not home the cursor",
    )?;
    feed(&mut grid, b"\x1b[132$|"); // DECSCPP 132
    expect(grid.cols == 132, "DECSCPP 132 failed")?;
    feed(&mut grid, b"\x1b[?3l"); // reset → 132
    expect(grid.cols == 132, "DECCOLM reset mismatch")?;
    expect(
        grid.window_resize_request.is_some(),
        "DECCOLM did not surface a window resize request",
    )
}

fn decic_decdc_columns() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 8, rows: 2 }, 32);
    feed(&mut grid, b"abcdef\x1b[4D\x1b['}");
    expect(
        grid.line_to_string(0) == "ab cdef ",
        format!("DECIC wrong: {:?}", grid.line_to_string(0)),
    )?;
    feed(&mut grid, b"\x1b[2D\x1b['~");
    expect(
        grid.line_to_string(0) == "b cdef  ",
        format!("DECDC wrong: {:?}", grid.line_to_string(0)),
    )
}

fn decfra_decera_rects() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 8, rows: 4 }, 32);
    feed(&mut grid, b"\x1b[65;2;2;3;4$x"); // fill 'A' rows 2-3, cols 2-4
    expect(grid.cell(1, 1).ch == 'A', "DECFRA top-left")?;
    expect(grid.cell(3, 2).ch == 'A', "DECFRA bottom-right")?;
    expect(grid.cell(0, 0).ch == ' ', "DECFRA leaked outside rect")?;
    feed(&mut grid, b"\x1b[1;1;2;3$z"); // erase rows 1-2, cols 1-3
    expect(grid.cell(0, 0).ch == ' ', "DECERA top-left not erased")?;
    expect(
        grid.cell(2, 1).ch == ' ',
        "DECERA did not erase filled cells",
    )?;
    expect(grid.cell(3, 1).ch == 'A', "DECERA over-erased")
}

fn decpam_decnkm_and_decbkm() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 10, rows: 2 }, 32);
    feed(&mut grid, b"\x1b="); // DECPAM
    expect(grid.keypad_app, "DECPAM did not set keypad app mode")?;
    feed(&mut grid, b"\x1b>"); // DECPNM
    expect(!grid.keypad_app, "DECPNM did not clear keypad app mode")?;
    feed(&mut grid, b"\x1b[?66h");
    expect(grid.keypad_app, "DECSET 66 did not set keypad app mode")?;
    feed(&mut grid, b"\x1b[?67h\x1b[?67$p");
    expect(
        grid.take_responses() == vec![b"\x1b[?67;1$y".to_vec()],
        "DECRQM mode 67 mismatch",
    )
}

fn decslrm_margins() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 8, rows: 3 }, 32);
    feed(&mut grid, b"\x1b[?69h\x1b[3;6s"); // DECLRMM + margins 3..6 (0-based 2..5)
    expect(grid.left_right_margins, "DECSET 69 not set")?;
    expect(
        grid.scroll_left == 2 && grid.scroll_right == 5,
        "DECSLRM margins wrong",
    )?;
    feed(&mut grid, b"abcdefgh");
    expect(
        grid.line_to_string(0) == "  abcd  " && grid.line_to_string(1) == "  efgh  ",
        format!(
            "margin wrap wrong: {:?} / {:?}",
            grid.line_to_string(0),
            grid.line_to_string(1)
        ),
    )?;
    feed(&mut grid, b"\x1b[1;1H\x1b[2K");
    expect(
        grid.line_to_string(0) == "        ",
        "margin-bounded EL wrong",
    )?;
    feed(&mut grid, b"\x1b[?69l");
    expect(!grid.left_right_margins, "DECRST 69 failed")?;
    expect(
        grid.scroll_left == 0 && grid.scroll_right == 7,
        "disabling DECLRMM should restore full width",
    )
}

fn decsca_selective_erase() -> Result<(), String> {
    let mut grid = Grid::new(WinSize { cols: 8, rows: 2 }, 32);
    feed(&mut grid, b"\x1b[2\"qab\x1b[0\"qcd");
    feed(&mut grid, b"\x1b[1;1H\x1b[?2K"); // DECSEL 2
    expect(
        grid.line_to_string(0) == "ab      ",
        format!("DECSEL kept wrong cells: {:?}", grid.line_to_string(0)),
    )?;
    feed(&mut grid, b"\x1b[?J"); // DECSED from cursor: wipes the rest
    expect(
        grid.line_to_string(1) == "        " && grid.cell(2, 0).ch == ' ',
        "DECSED did not erase unprotected cells",
    )?;
    expect(grid.cell(0, 0).ch == 'a', "DECSED erased a protected cell")?;
    // DECRQM reports DECLRMM state.
    feed(&mut grid, b"\x1b[?69$p");
    expect(
        grid.take_responses() == vec![b"\x1b[?69;2$y".to_vec()],
        "DECRQM mode 69 mismatch",
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
        Case {
            name: "shell_integration_markers",
            run: shell_integration_markers,
        },
        Case {
            name: "in_band_resize_2048",
            run: in_band_resize_2048,
        },
        Case {
            name: "sixel_decode_and_place",
            run: sixel_decode_and_place,
        },
        Case {
            name: "decawm_off_clamps",
            run: decawm_off_clamps,
        },
        Case {
            name: "rep_repeats_last_char",
            run: rep_repeats_last_char,
        },
        Case {
            name: "ich_dch_ech",
            run: ich_dch_ech,
        },
        Case {
            name: "scroll_region_confines",
            run: scroll_region_confines,
        },
        Case {
            name: "utf8_and_invalid_bytes",
            run: utf8_and_invalid_bytes,
        },
        Case {
            name: "osc52_clipboard",
            run: osc52_clipboard,
        },
        Case {
            name: "cursor_shapes_and_paste",
            run: cursor_shapes_and_paste,
        },
        Case {
            name: "pending_wrap_then_wrap",
            run: pending_wrap_then_wrap,
        },
        Case {
            name: "decrqm_ansi_and_private",
            run: decrqm_ansi_and_private,
        },
        Case {
            name: "osc8_hyperlink",
            run: osc8_hyperlink,
        },
        Case {
            name: "irm_insert_mode",
            run: irm_insert_mode,
        },
        Case {
            name: "decsc_decrc_restore",
            run: decsc_decrc_restore,
        },
        Case {
            name: "sgr_truecolor_colon_and_semicolon",
            run: sgr_truecolor_colon_and_semicolon,
        },
        Case {
            name: "cht_cbt_tabs",
            run: cht_cbt_tabs,
        },
        Case {
            name: "dectcem_hide_show",
            run: dectcem_hide_show,
        },
        Case {
            name: "decom_origin_mode",
            run: decom_origin_mode,
        },
        Case {
            name: "decstr_soft_reset",
            run: decstr_soft_reset,
        },
        Case {
            name: "decreqtparm_report",
            run: decreqtparm_report,
        },
        Case {
            name: "deccolm_switches_width",
            run: deccolm_switches_width,
        },
        Case {
            name: "decic_decdc_columns",
            run: decic_decdc_columns,
        },
        Case {
            name: "decfra_decera_rects",
            run: decfra_decera_rects,
        },
        Case {
            name: "decpam_decnkm_and_decbkm",
            run: decpam_decnkm_and_decbkm,
        },
        Case {
            name: "decslrm_margins",
            run: decslrm_margins,
        },
        Case {
            name: "decsca_selective_erase",
            run: decsca_selective_erase,
        },
        Case {
            name: "kitty_keyboard_protocol",
            run: kitty_keyboard_protocol,
        },
        Case {
            name: "kitty_graphics_roundtrip",
            run: kitty_graphics_roundtrip,
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
