//! Terminal grid — Layer 3.
//!
//! A 2-D array of [`Cell`]s driven by the VT parser's `Action` stream.
//! The grid owns all terminal state: cursor position, active SGR attributes,
//! scroll region, and the alternate screen buffer.

use unicode_width::UnicodeWidthChar;

use crate::parser::{Action, Perform};

// ---------------------------------------------------------------------------
// WinSize — shared between PTY and grid
// ---------------------------------------------------------------------------

/// Terminal dimensions in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinSize {
    pub cols: u16,
    pub rows: u16,
}

// ---------------------------------------------------------------------------
// Color
// ---------------------------------------------------------------------------

/// Terminal color — default / 256-index / true-color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// Use the terminal default (usually inherits from theme).
    Default,
    /// Standard 8 colors (0–7), bright 8 colors (8–15), 6×6×6 cube (16–231),
    /// grayscale ramp (232–255).
    Indexed(u8),
    /// 24-bit true color (SGR 38;2;r;g;b / 48;2;r;g;b).
    Rgb(u8, u8, u8),
}

impl Color {
    /// Convert indexed color to RGB using the color palette.
    pub fn to_rgb(&self, palette: &ColorPalette) -> Option<(u8, u8, u8)> {
        match self {
            Color::Default => None,
            Color::Rgb(r, g, b) => Some((*r, *g, *b)),
            Color::Indexed(i) => palette.get_color(*i),
        }
    }
}

// ---------------------------------------------------------------------------
// ColorPalette — 256-color terminal palette
// ---------------------------------------------------------------------------

/// Default xterm-256 color palette.
fn default_palette() -> [(u8, u8, u8); 256] {
    let mut colors = [(0u8, 0u8, 0u8); 256];

    // Standard 8 colors (0-7)
    colors[0] = (0, 0, 0); // Black
    colors[1] = (205, 49, 49); // Red
    colors[2] = (13, 188, 121); // Green
    colors[3] = (229, 229, 16); // Yellow
    colors[4] = (36, 114, 200); // Blue
    colors[5] = (188, 63, 188); // Magenta
    colors[6] = (17, 168, 205); // Cyan
    colors[7] = (229, 229, 229); // White

    // Bright 8 colors (8-15)
    colors[8] = (102, 102, 102); // Bright Black
    colors[9] = (241, 76, 76); // Bright Red
    colors[10] = (35, 209, 139); // Bright Green
    colors[11] = (245, 245, 67); // Bright Yellow
    colors[12] = (59, 142, 234); // Bright Blue
    colors[13] = (214, 112, 214); // Bright Magenta
    colors[14] = (41, 184, 219); // Bright Cyan
    colors[15] = (255, 255, 255); // Bright White

    // 6x6x6 color cube (16-231)
    for i in 0..216 {
        let r = i / 36;
        let g = (i / 6) % 6;
        let b = i % 6;
        colors[16 + i] = (
            if r == 0 { 0 } else { 55 + r * 40 } as u8,
            if g == 0 { 0 } else { 55 + g * 40 } as u8,
            if b == 0 { 0 } else { 55 + b * 40 } as u8,
        );
    }

    // Grayscale ramp (232-255)
    for i in 0..24 {
        let v = 8 + i * 10;
        colors[232 + i] = (v as u8, v as u8, v as u8);
    }

    colors
}

/// Terminal color palette with OSC 4/10/11 support.
#[derive(Debug, Clone)]
pub struct ColorPalette {
    colors: [(u8, u8, u8); 256],
    /// Default foreground color (can be overridden by OSC 10).
    pub default_fg: (u8, u8, u8),
    /// Default background color (can be overridden by OSC 11).
    pub default_bg: (u8, u8, u8),
    /// Default cursor color (can be overridden by OSC 12).
    pub cursor_color: (u8, u8, u8),
    /// Window title (set by OSC 0/2).
    pub title: String,
}

impl Default for ColorPalette {
    fn default() -> Self {
        ColorPalette {
            colors: default_palette(),
            default_fg: (229, 229, 229),   // Light gray
            default_bg: (30, 30, 46),      // Dark background
            cursor_color: (137, 180, 250), // Blue cursor
            title: String::new(),
        }
    }
}

impl ColorPalette {
    /// Get RGB color for indexed color (0-255).
    pub fn get_color(&self, index: u8) -> Option<(u8, u8, u8)> {
        Some(self.colors[index as usize])
    }

    /// Set a color in the palette (OSC 4).
    pub fn set_color(&mut self, index: u8, r: u8, g: u8, b: u8) {
        if (index as usize) < self.colors.len() {
            self.colors[index as usize] = (r, g, b);
        }
    }

    /// Reset palette to defaults (OSC 104).
    pub fn reset_palette(&mut self) {
        self.colors = default_palette();
    }

    /// Set default foreground color (OSC 10).
    pub fn set_fg(&mut self, r: u8, g: u8, b: u8) {
        self.default_fg = (r, g, b);
    }

    /// Set default background color (OSC 11).
    pub fn set_bg(&mut self, r: u8, g: u8, b: u8) {
        self.default_bg = (r, g, b);
    }

    /// Set cursor color (OSC 12).
    pub fn set_cursor_color(&mut self, r: u8, g: u8, b: u8) {
        self.cursor_color = (r, g, b);
    }

    /// Reset cursor color to default (OSC 112).
    pub fn reset_cursor_color(&mut self) {
        self.cursor_color = (137, 180, 250); // Default blue
    }

    /// Set window title (OSC 0/2).
    pub fn set_title(&mut self, title: &str) {
        self.title = title.to_string();
        log::debug!("Window title: {}", self.title);
    }

    /// Get the default foreground as Color.
    pub fn fg_color(&self) -> Color {
        Color::Rgb(self.default_fg.0, self.default_fg.1, self.default_fg.2)
    }

    /// Get the default background as Color.
    pub fn bg_color(&self) -> Color {
        Color::Rgb(self.default_bg.0, self.default_bg.1, self.default_bg.2)
    }
}

// ---------------------------------------------------------------------------
// Attrs — SGR attribute bitfield
// ---------------------------------------------------------------------------

/// SGR attributes packed into a `u16` bitfield (2 bytes instead of 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attrs(u16);

const BOLD: u16 = 1 << 0;
const DIM: u16 = 1 << 1;
const ITALIC: u16 = 1 << 2;
const UNDERLINE: u16 = 1 << 3;
const BLINK: u16 = 1 << 4;
const BLINK_RAPID: u16 = 1 << 5;
const INVERSE: u16 = 1 << 6;
const INVISIBLE: u16 = 1 << 7;
const STRIKETHROUGH: u16 = 1 << 8;
const UNDERLINE_STYLE_SHIFT: u16 = 9;
const UNDERLINE_STYLE_MASK: u16 = 0b111 << UNDERLINE_STYLE_SHIFT;

impl Attrs {
    pub fn bold(&self) -> bool {
        self.0 & BOLD != 0
    }
    pub fn dim(&self) -> bool {
        self.0 & DIM != 0
    }
    pub fn italic(&self) -> bool {
        self.0 & ITALIC != 0
    }
    pub fn underline(&self) -> bool {
        self.0 & UNDERLINE != 0
    }
    pub fn blink(&self) -> bool {
        self.0 & BLINK != 0
    }
    pub fn blink_rapid(&self) -> bool {
        self.0 & BLINK_RAPID != 0
    }
    pub fn inverse(&self) -> bool {
        self.0 & INVERSE != 0
    }
    pub fn invisible(&self) -> bool {
        self.0 & INVISIBLE != 0
    }
    pub fn strikethrough(&self) -> bool {
        self.0 & STRIKETHROUGH != 0
    }

    pub fn set_bold(&mut self, v: bool) {
        if v {
            self.0 |= BOLD;
        } else {
            self.0 &= !BOLD;
        }
    }
    pub fn set_dim(&mut self, v: bool) {
        if v {
            self.0 |= DIM;
        } else {
            self.0 &= !DIM;
        }
    }
    pub fn set_italic(&mut self, v: bool) {
        if v {
            self.0 |= ITALIC;
        } else {
            self.0 &= !ITALIC;
        }
    }
    pub fn set_underline(&mut self, v: bool) {
        if v {
            self.0 |= UNDERLINE;
        } else {
            self.0 &= !UNDERLINE;
        }
    }
    pub fn set_blink(&mut self, v: bool) {
        if v {
            self.0 |= BLINK;
        } else {
            self.0 &= !BLINK;
        }
    }
    pub fn set_blink_rapid(&mut self, v: bool) {
        if v {
            self.0 |= BLINK_RAPID;
        } else {
            self.0 &= !BLINK_RAPID;
        }
    }
    pub fn set_inverse(&mut self, v: bool) {
        if v {
            self.0 |= INVERSE;
        } else {
            self.0 &= !INVERSE;
        }
    }
    pub fn set_invisible(&mut self, v: bool) {
        if v {
            self.0 |= INVISIBLE;
        } else {
            self.0 &= !INVISIBLE;
        }
    }
    pub fn set_strikethrough(&mut self, v: bool) {
        if v {
            self.0 |= STRIKETHROUGH;
        } else {
            self.0 &= !STRIKETHROUGH;
        }
    }

    /// Underline style: 0 none, 1 single, 2 double, 3 curly, 4 dotted,
    /// 5 dashed. Values outside the supported range are clamped to single.
    pub fn underline_style(&self) -> u8 {
        ((self.0 & UNDERLINE_STYLE_MASK) >> UNDERLINE_STYLE_SHIFT) as u8
    }

    pub fn set_underline_style(&mut self, style: u8) {
        let style = style.min(5) as u16;
        self.0 = (self.0 & !UNDERLINE_STYLE_MASK) | (style << UNDERLINE_STYLE_SHIFT);
        self.set_underline(style != 0);
    }
}

// ---------------------------------------------------------------------------
// Cell
// ---------------------------------------------------------------------------

/// A single terminal grid cell.
#[derive(Debug, Clone)]
pub struct Cell {
    /// The character occupying this cell (may be space / NUL for empty cells).
    pub ch: char,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// SGR attributes.
    pub attrs: Attrs,
    /// Marks cell as needing re-render.
    pub dirty: bool,
    /// True if this cell is the right half of a wide (double-width) character.
    pub wide_filler: bool,
    /// Hyperlink ID (if cell is part of a hyperlink).
    pub hyperlink_id: u32,
    /// Additional grapheme codepoints attached to this base character.
    /// Stored only for cells that actually have a cluster tail.
    pub combining: Option<String>,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::default(),
            dirty: true,
            wide_filler: false,
            hyperlink_id: 0,
            combining: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Cursor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct Cursor {
    pub col: usize,
    pub row: usize,
}

// ---------------------------------------------------------------------------
// Saved cursor state (DECSC / DECRC)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
struct SavedCursor {
    cursor: Cursor,
    fg: Color,
    bg: Color,
    attrs: Attrs,
}

impl Default for Color {
    fn default() -> Self {
        Color::Default
    }
}

// ---------------------------------------------------------------------------
// Mouse tracking mode
// ---------------------------------------------------------------------------

/// Mouse tracking mode as defined by DECSET private modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseMode {
    /// No mouse tracking (default).
    #[default]
    None,
    /// Normal tracking (DECSET 1000): Button press + release only.
    Normal,
    /// Button-event tracking (DECSET 1002): Press + release + motion while held.
    ButtonEvent,
    /// Any-event tracking (DECSET 1003): All motion + all buttons.
    AnyEvent,
}

/// Mouse event encoding format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseEncoding {
    /// Legacy X10 encoding (default). Caps coords at 223 (byte + 32 ≤ 255).
    #[default]
    X10,
    /// SGR extended encoding (DECSET 1006) — decimal coords, supports >223
    /// columns/rows (T3-19).
    SGR,
    /// urxvt encoding (DECSET 1015) — `CSI Cb ; Cx ; Cy M` with decimal
    /// coordinates (T3-19).
    Urxvt,
}

// ---------------------------------------------------------------------------
// Grid
// Max length for OSC and DCS strings (100KB; matches xterm class).
const MAX_OSC_DCS_LEN: usize = 100_000;

/// The terminal grid buffer and state.
pub struct Grid {
    pub cols: usize,
    pub rows: usize,

    // Two screens: primary and alternate (toggled by ?1049h / ?1049l)
    cells_primary: Vec<Cell>,
    cells_alt: Vec<Cell>,
    alt_active: bool,

    pub cursor: Cursor,
    saved_cursor: SavedCursor,

    // Active SGR state applied to newly printed characters
    active_fg: Color,
    active_bg: Color,
    active_attrs: Attrs,

    // Scroll region (inclusive, 0-indexed)
    scroll_top: usize,
    scroll_bottom: usize,
    /// DEC line presentation modes: 0 normal, 3/4 double-height halves,
    /// 6 double-width.
    line_modes: Vec<u8>,

    // Scrollback buffer (primary screen only). VecDeque gives O(1) eviction
    // of the oldest line (T1-7); a Vec required remove(0), which is O(n)
    // and burned CPU under heavy scroll output like `cat bigfile`.
    pub scrollback: std::collections::VecDeque<Vec<Cell>>,
    pub scrollback_offset: usize, // 0 = no scroll, >0 = lines scrolled up
    /// Fractional scroll offset for smooth scrolling (0.0 to 1.0)
    pub scroll_fraction: f32,
    /// Maximum number of lines in scrollback buffer
    pub scrollback_capacity: usize,

    // DECTCEM cursor visibility
    pub cursor_visible: bool,

    // --- Tier-3 private modes (DECSET/DECRST) ---
    /// DECOM (?6): origin mode — CUP/positioning relative to scroll region.
    pub origin_mode: bool,
    /// DECAWM (?7): autowrap at end of line (defaults on, per VT100).
    pub autowrap: bool,
    /// DECSCNM (?5): screen reverse video (renderer reads this).
    pub screen_reverse: bool,
    /// IRM (?4): insert mode — printing shifts text right.
    pub insert_mode: bool,
    /// DECSCUSR cursor shape: 0/1 block-blink, 2 block, 3 underline-blink,
    /// 4 underline, 5 bar-blink, 6 bar. Renderer consumes this (T3-6).
    pub cursor_shape: u8,
    /// DECCKM (?1): application cursor keys — arrows send SS3 A..D instead of
    /// CSI A..D. Read by the key encoder via the app (T3-1).
    pub application_cursor_keys: bool,
    /// Kitty keyboard protocol disambiguation mode.
    pub kitty_keyboard: bool,
    /// xterm modifyOtherKeys mode (0 disabled, 1/2 enabled).
    pub modify_other_keys: u8,

    // Cursor saved specifically for alternate screen entry/exit
    alt_saved_cursor: Cursor,

    // Mouse tracking state
    pub mouse_mode: MouseMode,
    pub mouse_encoding: MouseEncoding,
    pub mouse_position: Cursor, // last known mouse position (cell coords, 1-based)

    // Color palette and OSC support
    pub palette: ColorPalette,

    // Bracketed paste mode (DECSET 2004)
    pub bracketed_paste: bool,

    // --- Tier-4 modes ---
    /// Focus reporting (DECSET 1004): `CSI I` on focus-in, `CSI O` on focus-out.
    /// vim/neovim use this to update statusline/signcolumn when the terminal
    /// loses focus. The app fires the events via [`Grid::focus_in`] /
    /// [`Grid::focus_out`].
    pub focus_reporting: bool,
    /// Synchronized output (DECSET 2026): when set, the app suppresses
    /// redraws until the mode is reset, then does one final redraw. This
    /// kills flicker on htop/vim full-screen redraws (T4-4).
    pub synchronized_output: bool,

    /// When true, `print_ascii_run` skips per-cell dirty marking. The caller
    /// should call `mark_all_dirty()` once after the batch completes.
    pub bulk_output: bool,

    // --- Tier-3 Batch B state ---
    /// Horizontal tab stops, one flag per column (T3-9). Defaults every 8.
    tab_stops: Vec<bool>,
    /// Last graphic character printed, for REP (`CSI b`) (T3-11).
    last_char: Option<char>,
    /// Saved cursor for the alternate screen — DECSC/DECRC are per-screen (T3-14).
    saved_cursor_alt: SavedCursor,

    // --- Tier-3 character sets (T3-8): G0/G1 designation + SO/SI shift ---
    /// G0 designated charset: 'B' = US ASCII, '0' = DEC Special Graphics.
    g0_charset: u8,
    /// G1 designated charset (switched to by SO).
    g1_charset: u8,
    /// Which charset is currently active for printing: 0 = G0, 1 = G1.
    active_charset: u8,

    // Bell state
    pub bell_pending: bool,

    // OSC 52 clipboard requests (set by handle_osc, drained by the app).
    /// Decoded text the application asked us to copy (`OSC 52 ; sel ; b64`).
    pub clipboard_set: Option<String>,
    /// Set when the application queries the clipboard (`OSC 52 ; sel ; ?`).
    pub clipboard_query_requested: bool,

    /// Response outbox (T2): the terminal's replies to device queries —
    /// DA1/DA2, DSR/CPR, DECRQM, OSC color queries. The grid cannot reach
    /// the PTY directly, so the app drains this after each parse pass and
    /// writes it to the master fd (the st `ttywrite` pattern).
    responses: Vec<Vec<u8>>,

    /// DCS passthrough buffer (T3-17). Accumulates the bytes of a DCS
    /// sequence (between `DCS` and ST) so we can answer requests like
    /// DECRQSS instead of discarding the data. Capped at the same limit as
    /// OSC strings to avoid pathological allocation.
    dcs_buf: Vec<u8>,
    /// True while inside a `DECRQSS` DCS sequence (`1$ q ... ST`), so we
    /// know to emit the reply on Unhook.
    dcs_request: bool,

    // Hyperlink support (OSC 8)
    hyperlinks: std::collections::HashMap<u32, String>,
    active_hyperlink_id: u32,
    next_hyperlink_id: u32,
}

/// Default horizontal tab stops: every 8 columns (VT100 power-up state).
fn default_tab_stops(cols: usize) -> Vec<bool> {
    (0..cols).map(|c| c % 8 == 0 && c != 0).collect()
}

/// Convert a scrollback line (`Vec<Cell>`) to a string, skipping wide-cell
/// fillers. Used by `all_lines_with_scrollback` (T4-2).
fn line_to_string_from_cells(line: &[Cell]) -> String {
    let mut s = String::new();
    for cell in line {
        if !cell.wide_filler {
            s.push(cell.ch);
            if let Some(cluster_tail) = &cell.combining {
                s.push_str(cluster_tail);
            }
        }
    }
    s
}

/// Map a byte in the DEC Special Graphics set (G0 = `ESC ( 0`) to its
/// Unicode box-drawing / symbol equivalent. Source: the canonical vt100_0
/// table from st (proudly stolen from rxvt), covering 0x41–0x7E.
/// Returns `None` for positions that are not remapped (printed as-is).
fn map_dec_special(ch: char) -> Option<char> {
    const TABLE: &[Option<char>] = &[
        Some('↑'),
        Some('↓'),
        Some('→'),
        Some('←'),
        Some('█'),
        Some('▚'),
        Some('☃'), // A-G
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None, // H-O
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None, // P-W
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None, // X-_
        Some('◆'),
        Some('▒'),
        Some('␉'),
        Some('␌'),
        Some('␍'),
        Some('␊'),
        Some('°'),
        Some('±'), // `-g
        Some('␤'),
        Some('␋'),
        Some('┘'),
        Some('┐'),
        Some('┌'),
        Some('└'),
        Some('┼'),
        Some('⎺'), // h-o
        Some('⎻'),
        Some('─'),
        Some('⎼'),
        Some('⎽'),
        Some('├'),
        Some('┤'),
        Some('┴'),
        Some('┬'), // p-w
        Some('│'),
        Some('≤'),
        Some('≥'),
        Some('π'),
        Some('≠'),
        Some('£'),
        Some('·'), // x-~
    ];
    let code = ch as u32;
    if (0x41..=0x7E).contains(&code) {
        TABLE[(code - 0x41) as usize]
    } else {
        None
    }
}

impl Grid {
    pub fn new(size: WinSize, scrollback: usize) -> Self {
        let cols = size.cols as usize;
        let rows = size.rows as usize;
        let len = cols * rows;
        Grid {
            cols,
            rows,
            cells_primary: vec![Cell::default(); len],
            cells_alt: vec![Cell::default(); len],
            alt_active: false,
            cursor: Cursor::default(),
            saved_cursor: SavedCursor::default(),
            active_fg: Color::Default,
            active_bg: Color::Default,
            active_attrs: Attrs::default(),
            scroll_top: 0,
            scroll_bottom: rows - 1,
            line_modes: vec![0; rows],
            scrollback: std::collections::VecDeque::new(),
            scrollback_offset: 0,
            scroll_fraction: 0.0,
            scrollback_capacity: scrollback,
            cursor_visible: true,
            origin_mode: false,
            autowrap: true, // DECAWM defaults ON (VT100 behaviour)
            screen_reverse: false,
            insert_mode: false,
            cursor_shape: 0, // 0 = terminal default (blinking block)
            application_cursor_keys: false,
            kitty_keyboard: false,
            modify_other_keys: 0,
            tab_stops: default_tab_stops(cols),
            last_char: None,
            saved_cursor_alt: SavedCursor::default(),
            alt_saved_cursor: Cursor::default(),
            g0_charset: b'B', // US ASCII power-up default
            g1_charset: b'B',
            active_charset: 0, // G0
            mouse_mode: MouseMode::None,
            mouse_encoding: MouseEncoding::X10,
            mouse_position: Cursor::default(),
            palette: ColorPalette::default(),
            bracketed_paste: false,
            focus_reporting: false,
            synchronized_output: false,
            bulk_output: false,
            bell_pending: false,
            clipboard_set: None,
            clipboard_query_requested: false,
            responses: Vec::new(),
            dcs_buf: Vec::new(),
            dcs_request: false,
            hyperlinks: std::collections::HashMap::new(),
            active_hyperlink_id: 0,
            next_hyperlink_id: 1,
        }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    fn cells(&self) -> &[Cell] {
        if self.alt_active {
            &self.cells_alt
        } else {
            &self.cells_primary
        }
    }

    fn cells_mut(&mut self) -> &mut Vec<Cell> {
        if self.alt_active {
            &mut self.cells_alt
        } else {
            &mut self.cells_primary
        }
    }

    fn idx(&self, col: usize, row: usize) -> usize {
        row * self.cols + col
    }

    // -----------------------------------------------------------------------
    // Response channel (T2) — the terminal talks back
    // -----------------------------------------------------------------------

    /// Queue a response for the application (DA, CPR, DECRQM, color query).
    /// Drained by the app via [`Grid::take_responses`] after each parse pass.
    fn respond(&mut self, data: &[u8]) {
        self.responses.push(data.to_vec());
    }

    /// Return the cursor row used by CPR/DECXCPR. With DECOM enabled, VT
    /// reports the cursor relative to the active scrolling region; otherwise
    /// it reports the absolute screen row.
    fn reported_cursor_row(&self) -> usize {
        if self.origin_mode {
            self.cursor
                .row
                .saturating_sub(self.scroll_top)
                .min(self.scroll_bottom.saturating_sub(self.scroll_top))
                + 1
        } else {
            self.cursor.row + 1
        }
    }

    /// Focus-in event (T4-3). When mode 1004 is set, queue `CSI I` so the
    /// app (vim/neovim) can react to the terminal regaining focus.
    pub fn focus_in(&mut self) {
        if self.focus_reporting {
            self.respond(b"\x1b[I");
        }
    }

    /// Focus-out event (T4-3). When mode 1004 is set, queue `CSI O`.
    pub fn focus_out(&mut self) {
        if self.focus_reporting {
            self.respond(b"\x1b[O");
        }
    }

    /// Answer a `DECRQSS` DCS request (T3-17). The query string names a
    /// sequence to report back, e.g. `"m"` → SGR, `"r"` → DECSTBM. We echo
    /// it wrapped as `DCS 1$ r <query> ST`. Apps like tmux use this to read
    /// the current state of modes they didn't set.
    fn answer_decrqss(&mut self) {
        let query = String::from_utf8_lossy(&self.dcs_buf);
        let reply = format!("\x1bP1$r{}\x1b\\", query);
        self.respond(reply.as_bytes());
    }

    /// Take all queued responses, leaving the outbox empty.
    pub fn take_responses(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.responses)
    }

    /// DECRPM mode value for DECRQM reports: 1 = set, 2 = reset,
    /// 0 = mode not recognized.
    fn mode_state(&self, mode: u16) -> u8 {
        let set = match mode {
            1 => self.application_cursor_keys, // DECCKM
            4 => self.insert_mode,             // IRM
            5 => self.screen_reverse,          // DECSCNM
            6 => self.origin_mode,             // DECOM
            7 => self.autowrap,                // DECAWM
            25 => self.cursor_visible,         // DECTCEM
            1000 => self.mouse_mode == MouseMode::Normal,
            1002 => self.mouse_mode == MouseMode::ButtonEvent,
            1003 => self.mouse_mode == MouseMode::AnyEvent,
            1006 => self.mouse_encoding == MouseEncoding::SGR,
            1049 => self.alt_active, // alt screen
            2004 => self.bracketed_paste,
            1004 => self.focus_reporting,
            2026 => self.synchronized_output,
            _ => return 0,
        };
        if set {
            1
        } else {
            2
        }
    }

    /// Format an RGB triple as an xterm color spec (16 bits per channel):
    /// `rgb:rrrr/gggg/bbbb` — the form OSC color query responses use.
    fn color_spec(r: u8, g: u8, b: u8) -> String {
        // Scale 8-bit channels to 16-bit (x257 maps 0xff -> 0xffff).
        format!(
            "rgb:{:04x}/{:04x}/{:04x}",
            r as u16 * 257,
            g as u16 * 257,
            b as u16 * 257
        )
    }

    pub fn cell(&self, col: usize, row: usize) -> &Cell {
        let i = self.idx(col, row);
        &self.cells()[i]
    }

    /// DEC line presentation mode for a visible row.
    pub fn line_mode(&self, row: usize) -> u8 {
        self.line_modes.get(row).copied().unwrap_or(0)
    }

    /// Number of scrollback lines currently shown at the top of the screen
    /// when the view is scrolled up (`scrollback_offset` lines, capped at
    /// what the buffer holds).
    pub fn view_scrollback_lines(&self) -> usize {
        self.scrollback_offset.min(self.scrollback.len())
    }

    /// Map a *screen* row/col to its backing cell, accounting for the
    /// scrollback view offset (T1-4). Rows above the live grid are served
    /// from the scrollback buffer; rows below fall through to the live grid
    /// shifted down by the offset (the bottom live rows clip off-screen).
    /// Returns None out of bounds.
    pub fn cell_at_view(&self, col: usize, screen_row: usize) -> Option<&Cell> {
        if screen_row >= self.rows || col >= self.cols {
            return None;
        }
        let k = self.view_scrollback_lines();
        if screen_row < k {
            // Top k screen rows come from the newest k scrollback lines.
            let sb_idx = self.scrollback.len() - k + screen_row;
            self.scrollback.get(sb_idx).and_then(|line| line.get(col))
        } else {
            let live_row = screen_row - k;
            if live_row < self.rows {
                Some(self.cell(col, live_row))
            } else {
                None
            }
        }
    }

    fn cell_mut(&mut self, col: usize, row: usize) -> &mut Cell {
        let i = self.idx(col, row);
        &mut self.cells_mut()[i]
    }

    // -----------------------------------------------------------------------
    // Grid resize — reflows content (best-effort, preserves last `rows` lines)
    // -----------------------------------------------------------------------

    pub fn resize(&mut self, size: WinSize) {
        let new_cols = (size.cols as usize).max(1);
        let new_rows = (size.rows as usize).max(1);
        let old_cols = self.cols.max(1);
        let old_rows = self.rows.max(1);
        let old_cursor = self.cursor;
        let old_scrollback_len = self.scrollback.len();

        // Reflow primary history and visible rows as a bounded sequence of
        // cell lines. The existing model does not retain soft-wrap metadata,
        // so each old row is treated as an independent logical line; this is
        // still materially safer than dropping scrollback on every resize.
        let mut source_lines: Vec<Vec<Cell>> = self.scrollback.iter().cloned().collect();
        source_lines.extend(
            self.cells_primary
                .chunks(old_cols)
                .map(|line| line.to_vec()),
        );
        let old_cursor_line = old_scrollback_len + old_cursor.row.min(old_rows - 1);
        let mut reflowed = Vec::new();
        let mut cursor_line = 0;
        let mut cursor_col = 0;

        for (line_index, line) in source_lines.iter().enumerate() {
            let output_start = reflowed.len();
            let chunks = line.chunks(new_cols);
            let mut produced = 0;
            for chunk in chunks {
                let mut row = chunk.to_vec();
                row.resize(new_cols, Cell::default());
                reflowed.push(row);
                produced += 1;
            }
            if produced == 0 {
                reflowed.push(vec![Cell::default(); new_cols]);
                produced = 1;
            }

            if line_index == old_cursor_line {
                cursor_line = output_start + (old_cursor.col / new_cols).min(produced - 1);
                cursor_col = old_cursor.col % new_cols;
            }
        }

        if reflowed.is_empty() {
            reflowed.push(vec![Cell::default(); new_cols]);
        }

        let preferred_start = reflowed.len().saturating_sub(new_rows);
        let visible_start = cursor_line
            .saturating_sub(new_rows - 1)
            .min(preferred_start);
        let visible_lines = &reflowed[visible_start..reflowed.len().min(visible_start + new_rows)];
        let mut new_primary = Vec::with_capacity(new_cols * new_rows);
        for line in visible_lines {
            new_primary.extend(line.iter().cloned());
        }
        while new_primary.len() < new_cols * new_rows {
            new_primary.extend(std::iter::repeat(Cell::default()).take(new_cols));
        }

        let scrollback_start = visible_start.saturating_sub(self.scrollback_capacity);
        self.scrollback = reflowed[scrollback_start..visible_start]
            .iter()
            .cloned()
            .collect();

        // Resize the alternate screen in place, preserving its visible top-left
        // content. Alternate-screen applications generally redraw immediately,
        // but losing it during a transient resize causes visible corruption.
        let mut new_alt = vec![Cell::default(); new_cols * new_rows];
        for row in 0..old_rows.min(new_rows) {
            let copy_cols = old_cols.min(new_cols);
            let src = row * old_cols;
            let dst = row * new_cols;
            new_alt[dst..dst + copy_cols].clone_from_slice(&self.cells_alt[src..src + copy_cols]);
        }

        self.cols = new_cols;
        self.rows = new_rows;
        self.cells_primary = new_primary;
        self.cells_alt = new_alt;
        self.line_modes.resize(new_rows, 0);
        self.line_modes.truncate(new_rows);
        self.tab_stops = default_tab_stops(new_cols);
        self.scroll_top = 0;
        self.scroll_bottom = new_rows - 1;
        self.cursor.row = if cursor_line >= visible_start {
            (cursor_line - visible_start).min(new_rows - 1)
        } else {
            0
        };
        self.cursor.col = cursor_col.min(new_cols - 1);
        self.alt_saved_cursor.row = self.alt_saved_cursor.row.min(new_rows - 1);
        self.alt_saved_cursor.col = self.alt_saved_cursor.col.min(new_cols - 1);

        // Mark all cells dirty since the grid has been resized.
        self.mark_all_dirty();
        // T4-5: prune orphaned hyperlink entries on resize.
        self.prune_hyperlinks();
    }

    // -----------------------------------------------------------------------
    // Search helpers — extract lines as strings for searching
    // -----------------------------------------------------------------------

    /// Get a specific row as a String for searching
    pub fn line_to_string(&self, row: usize) -> String {
        if row >= self.rows {
            return String::new();
        }
        let mut s = String::new();
        for col in 0..self.cols {
            let cell = self.cell(col, row);
            if !cell.wide_filler {
                s.push(cell.ch);
                if let Some(cluster_tail) = &cell.combining {
                    s.push_str(cluster_tail);
                }
            }
        }
        s
    }

    /// Get all visible lines as strings
    pub fn all_lines(&self) -> Vec<String> {
        (0..self.rows).map(|r| self.line_to_string(r)).collect()
    }

    /// Get scrollback lines (if any) + visible lines (T4-2). Search and copy
    /// now see the full scrollback history, not just the visible viewport.
    pub fn all_lines_with_scrollback(&self) -> Vec<String> {
        let mut lines = Vec::new();
        // Scrollback lines (oldest first) — primary screen only.
        for line in &self.scrollback {
            lines.push(line_to_string_from_cells(line));
        }
        // Visible grid lines.
        for row in 0..self.rows {
            lines.push(self.line_to_string(row));
        }
        lines
    }

    /// Set the scrollback offset (for smooth scrolling)
    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.scrollback_offset = offset;
    }

    /// Set the fractional scroll offset for smooth scrolling
    pub fn set_scroll_fraction(&mut self, fraction: f32) {
        self.scroll_fraction = fraction.clamp(0.0, 1.0);
    }

    /// Reset scroll offset (return to bottom)
    pub fn reset_scroll(&mut self) {
        self.scrollback_offset = 0;
        self.scroll_fraction = 0.0;
    }

    /// Scroll up by a fractional amount (for smooth scrolling)
    pub fn smooth_scroll_up(&mut self, amount: f32) {
        let total = self.scrollback_offset as f32 + self.scroll_fraction + amount;
        self.scrollback_offset = (total / self.rows as f32).floor() as usize;
        self.scroll_fraction = total.fract();
    }

    /// Scroll down by a fractional amount (for smooth scrolling)
    pub fn smooth_scroll_down(&mut self, amount: f32) {
        let total = self.scrollback_offset as f32 + self.scroll_fraction - amount;
        if total <= 0.0 {
            self.scrollback_offset = 0;
            self.scroll_fraction = 0.0;
        } else {
            self.scrollback_offset = (total / self.rows as f32).floor() as usize;
            self.scroll_fraction = total.fract();
        }
    }

    /// Mark cells in a match range as dirty for highlighting
    pub fn mark_match_dirty(&mut self, row: usize, start_col: usize, end_col: usize) {
        if row >= self.rows {
            return;
        }
        for col in start_col..end_col.min(self.cols) {
            let cell = self.cell_mut(col, row);
            cell.dirty = true;
        }
    }

    // -----------------------------------------------------------------------
    // Dirty cell tracking for optimized rendering
    // -----------------------------------------------------------------------

    /// Check if any cell is dirty
    pub fn has_dirty_cells(&self) -> bool {
        for row in 0..self.rows {
            for col in 0..self.cols {
                if self.cell(col, row).dirty {
                    return true;
                }
            }
        }
        false
    }

    /// Get positions of all dirty cells and clear their dirty flags
    /// Returns a vector of (row, col) pairs
    pub fn take_dirty_cells(&mut self) -> Vec<(usize, usize)> {
        let mut dirty = Vec::new();
        for row in 0..self.rows {
            for col in 0..self.cols {
                let cell = self.cell_mut(col, row);
                if cell.dirty {
                    dirty.push((row, col));
                    cell.dirty = false;
                }
            }
        }
        dirty
    }

    /// Mark all cells as dirty (full redraw)
    pub fn mark_all_dirty(&mut self) {
        for row in 0..self.rows {
            for col in 0..self.cols {
                let cell = self.cell_mut(col, row);
                cell.dirty = true;
            }
        }
    }

    /// Mark a specific cell as dirty
    pub fn mark_dirty(&mut self, col: usize, row: usize) {
        if col < self.cols && row < self.rows {
            let cell = self.cell_mut(col, row);
            cell.dirty = true;
        }
    }

    // -----------------------------------------------------------------------
    // Core print — write a char at cursor and advance
    // -----------------------------------------------------------------------

    fn print(&mut self, ch: char) {
        // Apply the active charset mapping. G0/G1 designate which set is in
        // play; only DEC Special Graphics ('0') remaps printable ASCII to
        // box-drawing glyphs (T3-8). US ASCII ('B') and anything else pass
        // through unchanged.
        let ch = if self.active_charset == 1 {
            if self.g1_charset == b'0' {
                map_dec_special(ch).unwrap_or(ch)
            } else {
                ch
            }
        } else if self.g0_charset == b'0' {
            map_dec_special(ch).unwrap_or(ch)
        } else {
            ch
        };

        let width = UnicodeWidthChar::width(ch).unwrap_or(1);

        // Zero-width characters (combining marks per UAX #11 / UAX #29
        // grapheme rules) attach to the preceding cell instead of occupying
        // their own — writing them into the current cell would clobber the
        // next column. If there is no preceding cell, drop the mark.
        // ("Text Rendering Hates You": a character is a grapheme cluster.)
        if width == 0 {
            let target = if self.cursor.col > 0 {
                Some((self.cursor.col - 1, self.cursor.row))
            } else if self.cursor.row > 0 {
                Some((self.cols - 1, self.cursor.row - 1))
            } else {
                None
            };
            if let Some((col, row)) = target {
                let cell = self.cell_mut(col, row);
                cell.combining.get_or_insert_with(String::new).push(ch);
                cell.dirty = true;
            }
            return;
        }

        // Wrap if at end of line. DECAWM (?7) controls this: with autowrap
        // off, text clamps to the last cell(s) and overwrites them (T3-3).
        if self.cursor.col + width > self.cols {
            if self.autowrap {
                self.cursor.col = 0;
                self.cursor.row += 1;
            } else {
                self.cursor.col = self.cols.saturating_sub(width);
            }
        }

        if self.cursor.row > self.scroll_bottom {
            self.scroll_up(1);
            self.cursor.row = self.scroll_bottom;
        }

        // Remember the last printed graphic char for REP (`CSI Ps b`) (T3-11).
        self.last_char = Some(ch);

        // IRM (?4): insert mode shifts the row right before writing (T3-12).
        if self.insert_mode {
            let row = self.cursor.row;
            let col = self.cursor.col;
            let cols = self.cols;
            let cells = self.cells_mut();
            for c in (col + width..cols).rev() {
                cells[row * cols + c] = cells[row * cols + c - width].clone();
                cells[row * cols + c].dirty = true;
            }
        }

        // Snapshot SGR state before the mutable borrow
        let fg = self.active_fg;
        let bg = self.active_bg;
        let attrs = self.active_attrs;
        let hyperlink_id = self.active_hyperlink_id;

        // Write the character
        let col = self.cursor.col;
        let row = self.cursor.row;
        {
            let cell = self.cell_mut(col, row);
            cell.ch = ch;
            cell.fg = fg;
            cell.bg = bg;
            cell.attrs = attrs;
            cell.dirty = true;
            cell.wide_filler = false;
            cell.hyperlink_id = hyperlink_id;
            cell.combining = None;
        }

        // Fill the second column of wide characters with a filler cell
        if width == 2 && col + 1 < self.cols {
            let cell = self.cell_mut(col + 1, row);
            cell.ch = ' ';
            cell.fg = fg;
            cell.bg = bg;
            cell.attrs = attrs;
            cell.dirty = true;
            cell.wide_filler = true;
            cell.hyperlink_id = hyperlink_id;
        }

        self.cursor.col += width;
    }

    /// Fast batch path for printable ASCII runs (0x20..=0x7e).
    ///
    /// Writes a contiguous run of ASCII characters directly into the cell
    /// array, handling autowrap and scroll-up without per-character function
    /// call overhead. This bypasses charset mapping, Unicode width lookups,
    /// and zero-width checks — all of which are no-ops for ASCII when the
    /// active charset is not DEC Special Graphics.
    ///
    /// Returns the number of bytes consumed. Stops early if a non-ASCII byte
    /// is encountered (the caller should feed remaining bytes through the
    /// normal per-byte path).
    pub fn print_ascii_run(&mut self, bytes: &[u8]) -> usize {
        if bytes.is_empty() {
            return 0;
        }

        // Bail to per-byte path if DEC Special Graphics is active — those
        // chars need the charset remap.
        let special_graphics = if self.active_charset == 1 {
            self.g1_charset == b'0'
        } else {
            self.g0_charset == b'0'
        };
        if special_graphics {
            return 0;
        }

        // Bail if insert mode is active — it needs per-char row shifting.
        if self.insert_mode {
            return 0;
        }

        // In bulk output mode, skip per-cell dirty marking — the caller will
        // mark the whole grid dirty once after the batch completes.
        let mark_dirty_per_cell = !self.bulk_output;

        let cols = self.cols;
        let scroll_bottom = self.scroll_bottom;
        let fg = self.active_fg;
        let bg = self.active_bg;
        let attrs = self.active_attrs;
        let hyperlink_id = self.active_hyperlink_id;

        let mut consumed = 0;
        let mut idx = 0;

        while idx < bytes.len() {
            let b = bytes[idx];

            // Handle common control characters inline to avoid breaking the
            // batch run for newlines (the most frequent control in text output).
            match b {
                0x0a => {
                    // LF — line feed
                    self.cursor.row += 1;
                    if self.cursor.row > scroll_bottom {
                        self.scroll_up(1);
                        self.cursor.row = scroll_bottom;
                    }
                    self.last_char = None;
                    consumed += 1;
                    idx += 1;
                    continue;
                }
                0x0d => {
                    // CR — carriage return
                    self.cursor.col = 0;
                    self.last_char = None;
                    consumed += 1;
                    idx += 1;
                    continue;
                }
                0x08 => {
                    // BS — backspace
                    if self.cursor.col > 0 {
                        self.cursor.col -= 1;
                    }
                    self.last_char = None;
                    consumed += 1;
                    idx += 1;
                    continue;
                }
                0x09 => {
                    // HT — tab: advance to next tab stop (every 8 by default)
                    let next_stop = (self.cursor.col / 8 + 1) * 8;
                    self.cursor.col = next_stop.min(cols - 1);
                    self.last_char = None;
                    consumed += 1;
                    idx += 1;
                    continue;
                }
                _ => {}
            }

            // Find the end of the printable ASCII run
            let remaining = &bytes[idx..];
            let run_end = remaining
                .iter()
                .position(|&b| !(0x20..=0x7e).contains(&b))
                .unwrap_or(remaining.len());
            if run_end == 0 {
                break;
            }

            let run = &remaining[..run_end];
            let mut run_idx = 0;

            while run_idx < run.len() {
                // Wrap if at end of line
                if self.cursor.col >= cols {
                    if self.autowrap {
                        self.cursor.col = 0;
                        self.cursor.row += 1;
                    } else {
                        self.cursor.col = cols - 1;
                    }
                }

                // Scroll if past bottom
                if self.cursor.row > scroll_bottom {
                    self.scroll_up(1);
                    self.cursor.row = scroll_bottom;
                }

                // How many chars fit on the current line?
                let space_left = cols - self.cursor.col;
                let take = (run.len() - run_idx).min(space_left);

                // Write the chunk directly into the cells array.
                let col = self.cursor.col;
                let row = self.cursor.row;
                {
                    let cells = self.cells_mut();
                    let start = row * cols + col;
                    let template = Cell {
                        ch: ' ',
                        fg,
                        bg,
                        attrs,
                        dirty: mark_dirty_per_cell,
                        wide_filler: false,
                        hyperlink_id,
                        combining: None,
                    };
                    for i in 0..take {
                        let b = run[run_idx + i];
                        let mut cell = template.clone();
                        cell.ch = b as char;
                        cells[start + i] = cell;
                    }
                }

                self.cursor.col += take;
                run_idx += take;
                consumed += take;
            }

            // Update last_char for REP (CSI b)
            if let Some(&b) = run.last() {
                self.last_char = Some(b as char);
            }

            idx += run_end;
        }

        consumed
    }

    // -----------------------------------------------------------------------
    // Scrolling
    // -----------------------------------------------------------------------

    fn scroll_up(&mut self, n: usize) {
        self.scroll_up_from(self.scroll_top, n);
    }

    /// Shift lines in `[top, scroll_bottom]` up by `n`, filling the bottom
    /// rows with blanks. When `top` is the region top on the primary screen,
    /// the displaced lines enter the scrollback (normal scrolling); otherwise
    /// they are discarded — that is VT DL semantics (deleted lines are lost,
    /// never saved). Mirrors st's `tscrollup(y, n, copyhist=0)`.
    fn scroll_up_from(&mut self, top: usize, n: usize) {
        let bot = self.scroll_bottom;
        if top > bot {
            return;
        }
        let n = n.min(bot - top + 1); // larger n just clears the rest
        let cols = self.cols;
        let mark_dirty = !self.bulk_output;

        // Push top line(s) into scrollback (primary screen, full-region only)
        if !self.alt_active && top == self.scroll_top {
            for r in top..top + n {
                if r < self.rows {
                    // Clone the row into scrollback before we overwrite it.
                    let start = r * cols;
                    let row: Vec<Cell> = self.cells_primary[start..start + cols].to_vec();
                    self.scrollback.push_back(row);
                }
            }
            // Enforce scrollback limit (O(1) eviction with VecDeque, T1-7)
            while self.scrollback.len() > self.scrollback_capacity {
                self.scrollback.pop_front();
            }
        }

        // Shift rows up using ptr::copy (memmove semantics) instead of
        // per-cell clone loops. This is the hot path under heavy output.
        let cells = self.cells_mut();
        let total = (bot - top + 1) * cols;
        let shift = n * cols;
        if shift < total {
            // Move rows up through a temporary clone so cells with grapheme
            // tails remain safely owned.
            let source_start = (top + n) * cols;
            let moved: Vec<Cell> = cells[source_start..source_start + total - shift].to_vec();
            cells[top * cols..top * cols + total - shift].clone_from_slice(&moved);
        }
        // Fill the bottom n rows with defaults
        let blank_start = (bot + 1 - n) * cols;
        let blank_end = (bot + 1) * cols;
        for i in blank_start..blank_end {
            cells[i] = Cell::default();
        }

        // Mark scrolled region dirty in bulk (skip in bulk_output mode)
        if mark_dirty {
            for r in top..=bot {
                let start = r * cols;
                for i in start..start + cols {
                    cells[i].dirty = true;
                }
            }
        }
    }

    fn scroll_down(&mut self, n: usize) {
        self.scroll_down_from(self.scroll_top, n);
    }

    /// Shift lines in `[top, scroll_bottom]` down by `n`, filling the top
    /// rows with blanks (VT IL semantics when `top > scroll_top`). Lines
    /// pushed off the bottom of the region are discarded.
    fn scroll_down_from(&mut self, top: usize, n: usize) {
        let bot = self.scroll_bottom;
        if top > bot {
            return;
        }
        let n = n.min(bot - top + 1);

        let cols = self.cols;
        let mark_dirty = !self.bulk_output;
        let cells = self.cells_mut();
        // Shift rows down using ptr::copy (memmove). Iterate from bottom to
        // top so we don't overwrite source rows before copying them.
        let total = (bot - top + 1) * cols;
        let shift = n * cols;
        if shift < total {
            // Move rows down through a temporary clone so cells with grapheme
            // tails remain safely owned.
            let source_start = top * cols;
            let moved: Vec<Cell> = cells[source_start..source_start + total - shift].to_vec();
            cells[source_start + shift..source_start + total].clone_from_slice(&moved);
        }
        // Fill the top n rows with defaults
        let blank_end = (top + n) * cols;
        for i in top * cols..blank_end {
            cells[i] = Cell::default();
        }

        // Mark scrolled region dirty in bulk (skip in bulk_output mode)
        if mark_dirty {
            for r in top..=bot {
                let start = r * cols;
                for i in start..start + cols {
                    cells[i].dirty = true;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Erase operations
    // -----------------------------------------------------------------------

    fn erase_in_display(&mut self, mode: u16) {
        let cursor = self.cursor;
        let (start, end) = match mode {
            0 => {
                // From cursor to end of screen
                let s = self.idx(cursor.col, cursor.row);
                let e = self.cols * self.rows;
                (s, e)
            }
            1 => {
                // From start to cursor (inclusive)
                let s = 0;
                let e = self.idx(cursor.col, cursor.row) + 1;
                (s, e)
            }
            2 | 3 => {
                // Entire screen. Mode 3 also wipes the scrollback history
                // (T3-13) so the user can't scroll up into cleared content.
                if mode == 3 {
                    self.scrollback.clear();
                    self.scrollback_offset = 0;
                }
                (0, self.cols * self.rows)
            }
            _ => return,
        };

        let fg = self.active_fg;
        let bg = self.active_bg;
        let cells = self.cells_mut();
        for i in start..end {
            cells[i] = Cell {
                fg,
                bg,
                dirty: true,
                ..Cell::default()
            };
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        let cursor = self.cursor;
        let (start_col, end_col) = match mode {
            0 => (cursor.col, self.cols), // cursor to end of line
            1 => (0, cursor.col + 1),     // start to cursor
            2 => (0, self.cols),          // entire line
            _ => return,
        };

        let fg = self.active_fg;
        let bg = self.active_bg;
        let row = cursor.row;
        let cols = self.cols;
        let cells = self.cells_mut();
        for c in start_col..end_col {
            cells[row * cols + c] = Cell {
                fg,
                bg,
                dirty: true,
                ..Cell::default()
            };
        }
    }

    // -----------------------------------------------------------------------
    // SGR — Select Graphic Rendition
    // -----------------------------------------------------------------------

    fn apply_sgr(&mut self, params: &[Vec<u16>]) {
        // Flatten to the primary value of each `;`-separated group — this is
        // what the simple SGR codes (1, 4, 30..=37, ...) use. Extended color
        // specs (38/48) are handled separately below so they can read colon
        // sub-parameters (T3-20: `38:2::r:g:b`).
        let flat: Vec<u16> = params
            .iter()
            .map(|p| p.first().copied().unwrap_or(0))
            .collect();

        let mut i = 0;
        while i < flat.len() {
            match flat[i] {
                0 => {
                    self.active_fg = Color::Default;
                    self.active_bg = Color::Default;
                    self.active_attrs = Attrs::default();
                    i += 1;
                }
                1 => {
                    self.active_attrs.set_bold(true);
                    i += 1;
                }
                2 => {
                    self.active_attrs.set_dim(true);
                    i += 1;
                }
                3 => {
                    self.active_attrs.set_italic(true);
                    i += 1;
                }
                4 => {
                    let style = params
                        .get(i)
                        .and_then(|group| group.get(1))
                        .copied()
                        .unwrap_or(1) as u8;
                    self.active_attrs.set_underline_style(style);
                    i += 1;
                }
                5 => {
                    self.active_attrs.set_blink(true);
                    i += 1;
                }
                6 => {
                    self.active_attrs.set_blink_rapid(true);
                    i += 1;
                }
                7 => {
                    self.active_attrs.set_inverse(true);
                    i += 1;
                }
                8 => {
                    self.active_attrs.set_invisible(true);
                    i += 1;
                }
                9 => {
                    self.active_attrs.set_strikethrough(true);
                    i += 1;
                }
                22 => {
                    self.active_attrs.set_bold(false);
                    self.active_attrs.set_dim(false);
                    i += 1;
                }
                23 => {
                    self.active_attrs.set_italic(false);
                    i += 1;
                }
                24 => {
                    self.active_attrs.set_underline_style(0);
                    i += 1;
                }
                25 => {
                    self.active_attrs.set_blink(false);
                    i += 1;
                }
                27 => {
                    self.active_attrs.set_inverse(false);
                    i += 1;
                }
                28 => {
                    self.active_attrs.set_invisible(false);
                    i += 1;
                }
                29 => {
                    self.active_attrs.set_strikethrough(false);
                    i += 1;
                }
                // Standard foreground colors
                30..=37 => {
                    self.active_fg = Color::Indexed(flat[i] as u8 - 30);
                    i += 1;
                }
                38 => {
                    // Extended foreground color. Two encodings are equivalent:
                    //   semicolon: `38;5;idx` or `38;2;r;g;b`
                    //   colon:     `38:5:idx` or `38:2:<tone>:r:g:b` (T3-20)
                    // When the whole spec sits in one `:` group (len > 1) we
                    // read within it; otherwise it spans `;` groups.
                    let is_colon = params.get(i).map(|g| g.len() > 1).unwrap_or(false);
                    if is_colon {
                        let g = &params[i];
                        match g.get(1).copied() {
                            Some(5) if g.len() >= 3 => {
                                self.active_fg = Color::Indexed(g[2] as u8);
                            }
                            Some(2) if g.len() >= 6 => {
                                // [38, 2, tone, r, g, b] — tone slot (g[2]) ignored
                                self.active_fg = Color::Rgb(g[3] as u8, g[4] as u8, g[5] as u8);
                            }
                            Some(2) if g.len() >= 5 => {
                                // [38, 2, r, g, b] — no tone slot (xterm allows both)
                                self.active_fg = Color::Rgb(g[2] as u8, g[3] as u8, g[4] as u8);
                            }
                            _ => {}
                        }
                    } else if i + 1 < flat.len() {
                        match flat[i + 1] {
                            5 if i + 2 < flat.len() => {
                                self.active_fg = Color::Indexed(flat[i + 2] as u8);
                                i += 2;
                            }
                            2 if i + 4 < flat.len() => {
                                self.active_fg = Color::Rgb(
                                    flat[i + 2] as u8,
                                    flat[i + 3] as u8,
                                    flat[i + 4] as u8,
                                );
                                i += 4;
                            }
                            _ => {}
                        }
                    }
                    i += 1;
                }
                39 => {
                    self.active_fg = Color::Default;
                    i += 1;
                }
                // Standard background colors
                40..=47 => {
                    self.active_bg = Color::Indexed(flat[i] as u8 - 40);
                    i += 1;
                }
                48 => {
                    // Extended background color (see 38 above).
                    let is_colon = params.get(i).map(|g| g.len() > 1).unwrap_or(false);
                    if is_colon {
                        let g = &params[i];
                        match g.get(1).copied() {
                            Some(5) if g.len() >= 3 => {
                                self.active_bg = Color::Indexed(g[2] as u8);
                            }
                            Some(2) if g.len() >= 6 => {
                                self.active_bg = Color::Rgb(g[3] as u8, g[4] as u8, g[5] as u8);
                            }
                            Some(2) if g.len() >= 5 => {
                                // [48, 2, r, g, b] — no tone slot
                                self.active_bg = Color::Rgb(g[2] as u8, g[3] as u8, g[4] as u8);
                            }
                            _ => {}
                        }
                    } else if i + 1 < flat.len() {
                        match flat[i + 1] {
                            5 if i + 2 < flat.len() => {
                                self.active_bg = Color::Indexed(flat[i + 2] as u8);
                                i += 2;
                            }
                            2 if i + 4 < flat.len() => {
                                self.active_bg = Color::Rgb(
                                    flat[i + 2] as u8,
                                    flat[i + 3] as u8,
                                    flat[i + 4] as u8,
                                );
                                i += 4;
                            }
                            _ => {}
                        }
                    }
                    i += 1;
                }
                49 => {
                    self.active_bg = Color::Default;
                    i += 1;
                }
                // Bright foreground (xterm extension)
                90..=97 => {
                    self.active_fg = Color::Indexed(flat[i] as u8 - 90 + 8);
                    i += 1;
                }
                // Bright background (xterm extension)
                100..=107 => {
                    self.active_bg = Color::Indexed(flat[i] as u8 - 100 + 8);
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Perform impl helper — CSI dispatch
    // -----------------------------------------------------------------------

    fn handle_csi(&mut self, params: &[Vec<u16>], intermediates: &[u8], final_byte: u8) {
        let p0 = params.first().and_then(|p| p.first()).copied().unwrap_or(0) as usize;
        let p1 = params.get(1).and_then(|p| p.first()).copied().unwrap_or(0) as usize;

        match (intermediates, final_byte) {
            // CUU — cursor up
            (_, b'A') => {
                let n = p0.max(1);
                self.cursor.row = self.cursor.row.saturating_sub(n).max(self.scroll_top);
            }
            // CUD — cursor down
            (_, b'B') => {
                let n = p0.max(1);
                self.cursor.row = (self.cursor.row + n).min(self.scroll_bottom);
            }
            // CUF — cursor forward
            (_, b'C') => {
                let n = p0.max(1);
                self.cursor.col = (self.cursor.col + n).min(self.cols - 1);
            }
            // CUB — cursor backward
            (_, b'D') => {
                let n = p0.max(1);
                self.cursor.col = self.cursor.col.saturating_sub(n);
            }
            // CNL — cursor next line
            (_, b'E') => {
                let n = p0.max(1);
                self.cursor.row = (self.cursor.row + n).min(self.rows - 1);
                self.cursor.col = 0;
            }
            // CPL — cursor preceding line
            (_, b'F') => {
                let n = p0.max(1);
                self.cursor.row = self.cursor.row.saturating_sub(n);
                self.cursor.col = 0;
            }
            // CHA — cursor horizontal absolute
            (_, b'G') => {
                self.cursor.col = p0.saturating_sub(1).min(self.cols - 1);
            }
            // CUP / HVP — cursor position. In DECOM (?6) the row is relative
            // to the scroll region top and clamped inside the region (T3-2).
            (_, b'H') | (_, b'f') => {
                let row = if self.origin_mode {
                    (p0.saturating_sub(1) + self.scroll_top).min(self.scroll_bottom)
                } else {
                    p0.saturating_sub(1).min(self.rows - 1)
                };
                self.cursor.row = row;
                self.cursor.col = p1.saturating_sub(1).min(self.cols - 1);
            }
            // ED — erase in display
            (_, b'J') => self.erase_in_display(p0 as u16),
            // EL — erase in line
            (_, b'K') => self.erase_in_line(p0 as u16),
            // ICH — insert characters (T3-10). Shifts the row right, leaving
            // `n` blank cells at the cursor; remaining cells fall off the end.
            (_, b'@') => {
                let n = p0.max(1);
                let row = self.cursor.row;
                let col = self.cursor.col;
                let cols = self.cols;
                let cells = self.cells_mut();
                // Shift everything at/after `col` right by `n`, working from
                // the right edge so we don't clobber source cells.
                for c in (col..cols).rev() {
                    if c + n < cols {
                        cells[row * cols + c + n] = cells[row * cols + c].clone();
                    }
                    if c >= col {
                        cells[row * cols + c].dirty = true;
                    }
                }
                // Blank the freshly-inserted cells at the cursor.
                for c in col..(col + n).min(cols) {
                    cells[row * cols + c] = Cell::default();
                }
            }
            // TBC — tabulation clear (T3-9). `CSI g` clears the stop at the
            // cursor column; `CSI 3 g` clears all stops.
            (_, b'g') => match p0 {
                0 => {
                    if self.cursor.col < self.tab_stops.len() {
                        self.tab_stops[self.cursor.col] = false;
                    }
                }
                3 => {
                    for s in self.tab_stops.iter_mut() {
                        *s = false;
                    }
                }
                _ => {}
            },
            // CBT — cursor backward tabulation (T3-15). `CSI Ps Z` moves to the
            // `Ps`-th previous tab stop (default 1). Past the first stop → col 0.
            (_, b'Z') => {
                let n = p0.max(1);
                let mut col = self.cursor.col;
                for _ in 0..n {
                    let mut prev = 0usize;
                    let mut found = false;
                    for c in (0..col).rev() {
                        if self.tab_stops.get(c).copied().unwrap_or(false) {
                            prev = c;
                            found = true;
                            break;
                        }
                    }
                    if found {
                        col = prev;
                    } else {
                        col = 0;
                        break;
                    }
                }
                self.cursor.col = col;
            }
            // IL — insert lines at cursor row, shifting cursor..region-bottom down.
            // No-op when the cursor is outside the scroll region (VT220 spec).
            (_, b'L') => {
                let n = p0.max(1);
                if self.cursor.row >= self.scroll_top && self.cursor.row <= self.scroll_bottom {
                    self.scroll_down_from(self.cursor.row, n);
                }
            }
            // DL — delete lines at cursor row, shifting cursor..region-bottom up.
            // Deleted lines are lost (never enter scrollback). No-op outside the
            // scroll region.
            (_, b'M') => {
                let n = p0.max(1);
                if self.cursor.row >= self.scroll_top && self.cursor.row <= self.scroll_bottom {
                    self.scroll_up_from(self.cursor.row, n);
                }
            }
            // DCH — delete characters
            (_, b'P') => {
                let n = p0.max(1);
                let row = self.cursor.row;
                let col = self.cursor.col;
                let cols = self.cols;
                let cells = self.cells_mut();
                for c in col..cols {
                    if c + n < cols {
                        cells[row * cols + c] = cells[row * cols + c + n].clone();
                    } else {
                        cells[row * cols + c] = Cell::default();
                    }
                    cells[row * cols + c].dirty = true;
                }
            }
            // REP — repeat preceding graphic character (T3-11).
            // Repeats `last_char`, `n` times total (the original + n-1 copies).
            (_, b'b') => {
                let n = p0.max(1);
                if let Some(ch) = self.last_char {
                    for _ in 0..n {
                        self.print(ch);
                    }
                }
            }
            // SD — scroll down
            (_, b'T') => self.scroll_down(p0.max(1)),
            // ECH — erase characters
            (_, b'X') => {
                let n = p0.max(1);
                let row = self.cursor.row;
                let col = self.cursor.col;
                let end = (col + n).min(self.cols);
                let fg = self.active_fg;
                let bg = self.active_bg;
                let cells_len = self.cols;
                let cells = self.cells_mut();
                for c in col..end {
                    cells[row * cells_len + c] = Cell {
                        fg,
                        bg,
                        dirty: true,
                        ..Cell::default()
                    };
                }
            }
            // VPA — vertical position absolute
            (_, b'd') => {
                self.cursor.row = p0.saturating_sub(1).min(self.rows - 1);
            }
            // SGR — select graphic rendition
            (b"", b'm') => self.apply_sgr(params),
            // DECSTBM — set top/bottom margins (scroll region)
            (_, b'r') => {
                let top = p0.saturating_sub(1);
                let bot = if p1 == 0 { self.rows - 1 } else { p1 - 1 };
                if top < bot && bot < self.rows {
                    self.scroll_top = top;
                    self.scroll_bottom = bot;
                }
                self.cursor = Cursor::default(); // home cursor
            }
            // DECSC — save cursor (per-screen, T3-14)
            (b"", b'7') | (b"", b's') => {
                let slot = if self.alt_active {
                    &mut self.saved_cursor_alt
                } else {
                    &mut self.saved_cursor
                };
                *slot = SavedCursor {
                    cursor: self.cursor,
                    fg: self.active_fg,
                    bg: self.active_bg,
                    attrs: self.active_attrs,
                };
            }
            // DECRC — restore cursor (per-screen, T3-14)
            (b"", b'8') | (b"", b'u') => {
                let slot = if self.alt_active {
                    &self.saved_cursor_alt
                } else {
                    &self.saved_cursor
                };
                self.cursor = slot.cursor;
                self.active_fg = slot.fg;
                self.active_bg = slot.bg;
                self.active_attrs = slot.attrs;
            }
            // ANSI mode set/reset (h / l) — no '?' intermediate.
            // IRM (?4 → actually ANSI 4): insert mode (T3-12).
            (b"", b'h') | (b"", b'l') => {
                let set = final_byte == b'h';
                for p in params {
                    let n = p.first().copied().unwrap_or(0);
                    if n == 4 {
                        self.insert_mode = set;
                    }
                }
            }
            // Private mode set/reset (?h / ?l)
            (b"?", b'h') | (b"?", b'l') => {
                let set = final_byte == b'h';
                for p in params {
                    let n = p.first().copied().unwrap_or(0);
                    match n {
                        1 => {
                            // DECCKM — application cursor keys (T3-1).
                            self.application_cursor_keys = set;
                        }
                        3 => {
                            // DECCOLM — 80/132 column switch (T3-5). We accept
                            // the mode without resizing (must not corrupt).
                            log::debug!("DECCOLM {}: accepted, no resize", set);
                        }
                        5 => {
                            // DECSCNM — screen reverse video (T3-4).
                            self.screen_reverse = set;
                            self.mark_all_dirty();
                        }
                        6 => {
                            // DECOM — origin mode (T3-2). Setting or resetting
                            // homes the cursor (to the region top when set).
                            self.origin_mode = set;
                            self.cursor.col = 0;
                            self.cursor.row = if set { self.scroll_top } else { 0 };
                        }
                        7 => {
                            // DECAWM — autowrap (T3-3), defaults ON.
                            self.autowrap = set;
                        }
                        25 => {
                            self.cursor_visible = set;
                        }
                        1000 => {
                            // Normal mouse tracking
                            self.mouse_mode = if set {
                                MouseMode::Normal
                            } else {
                                MouseMode::None
                            };
                        }
                        1002 => {
                            // Button-event mouse tracking
                            self.mouse_mode = if set {
                                MouseMode::ButtonEvent
                            } else {
                                MouseMode::None
                            };
                        }
                        1003 => {
                            // Any-event mouse tracking
                            self.mouse_mode = if set {
                                MouseMode::AnyEvent
                            } else {
                                MouseMode::None
                            };
                        }
                        1006 => {
                            // SGR extended mouse encoding (T3-19)
                            self.mouse_encoding = if set {
                                MouseEncoding::SGR
                            } else {
                                MouseEncoding::X10
                            };
                        }
                        1015 => {
                            // urxvt extended mouse encoding (T3-19)
                            self.mouse_encoding = if set {
                                MouseEncoding::Urxvt
                            } else {
                                MouseEncoding::X10
                            };
                        }
                        1049 => {
                            // Alternate screen
                            if set && !self.alt_active {
                                // Save cursor position for later restoration
                                self.alt_saved_cursor = self.cursor;
                                self.alt_active = true;
                                // Clear alternate screen
                                self.cells_alt = vec![Cell::default(); self.cols * self.rows];
                                self.cursor = Cursor::default();
                            } else if !set && self.alt_active {
                                self.alt_active = false;
                                // Restore cursor from the position saved at alt-screen entry
                                self.cursor = self.alt_saved_cursor;
                            }
                        }
                        2004 => {
                            // Bracketed paste mode
                            self.bracketed_paste = set;
                            log::debug!(
                                "Bracketed paste: {}",
                                if set { "enabled" } else { "disabled" }
                            );
                        }
                        1004 => {
                            // Focus reporting (T4-3)
                            self.focus_reporting = set;
                            log::debug!(
                                "Focus reporting: {}",
                                if set { "enabled" } else { "disabled" }
                            );
                        }
                        2026 => {
                            // Synchronized output (T4-4): suppress redraws
                            // while set; the app checks this flag before
                            // requesting redraws and does one final redraw
                            // on reset.
                            self.synchronized_output = set;
                            log::debug!(
                                "Synchronized output: {}",
                                if set { "enabled" } else { "disabled" }
                            );
                        }
                        _ => {}
                    }
                }
            }
            // -----------------------------------------------------------------
            // Device queries (T2) — the terminal responds over the outbox.
            // -----------------------------------------------------------------
            // DA1 — Primary Device Attributes: CSI c / CSI 0 c.
            // Claim VT220 class (62) with ANSI color (22), the features we
            // actually implement. (T2-1)
            (b"", b'c') => {
                let request = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
                if request == 0 {
                    self.respond(b"\x1b[?62;22c");
                }
                // Non-zero Ps: no response per VT spec.
            }
            // DA2 — Secondary Device Attributes: CSI > c.
            // Pp=1 (VT220), Pv=20 (firmware 2.0), Pc=0 (no options).
            (b">", b'c') => {
                let request = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
                if request == 0 {
                    self.respond(b"\x1b[>1;20;0c");
                }
            }
            // DSR — Device Status Report: CSI 5 n → terminal OK, CSI 6 n → CPR.
            // DEC private form CSI ? 6 n → DECXCPR (adds page number, we report 1).
            (b"", b'n') => {
                match params.first().and_then(|p| p.first()).copied().unwrap_or(0) {
                    5 => self.respond(b"\x1b[0n"), // terminal OK
                    6 => {
                        // CPR — cursor position, 1-based.
                        let resp = format!(
                            "\x1b[{};{}R",
                            self.reported_cursor_row(),
                            self.cursor.col + 1
                        );
                        self.respond(resp.as_bytes());
                    }
                    _ => {}
                }
            }
            (b"?", b'n') => {
                if params.first().and_then(|p| p.first()).copied().unwrap_or(0) == 6 {
                    let resp = format!(
                        "\x1b[?{};{};1R", // DECXCPR: row;col;page
                        self.reported_cursor_row(),
                        self.cursor.col + 1
                    );
                    self.respond(resp.as_bytes());
                }
            }
            // DECRQM — Mode Request: CSI ? Pd $ p → DECRPM CSI ? Pd ; Ps $ y.
            // '$' is collected as a second intermediate, so we see [?, $].
            // Ps: 1=set, 2=reset, 0=not recognized. (T2-3)
            (b"?$", b'p') => {
                let mode = params.first().and_then(|p| p.first()).copied().unwrap_or(0) as u16;
                let state = self.mode_state(mode);
                let resp = format!("\x1b[?{};{}$y", mode, state);
                self.respond(resp.as_bytes());
            }
            // DECRQM for ANSI (non-private) modes: CSI Pd $ p. We only track
            // IRM (4); anything else reports "not recognized" rather than
            // staying silent (apps probing ANSI modes would otherwise hang).
            (b"$", b'p') => {
                let mode = params.first().and_then(|p| p.first()).copied().unwrap_or(0) as u16;
                let state = self.mode_state(mode);
                let resp = format!("\x1b[{};{}$y", mode, state);
                self.respond(resp.as_bytes());
            }
            // Kitty keyboard protocol: CSI > 1 u enables disambiguation;
            // CSI < u restores the previous mode. This handles the common
            // negotiation path used by modern TUIs.
            (b">", b'u') => {
                self.kitty_keyboard = params
                    .first()
                    .and_then(|group| group.first())
                    .copied()
                    .unwrap_or(1)
                    != 0;
            }
            (b"<", b'u') => {
                self.kitty_keyboard = false;
            }
            // modifyOtherKeys: CSI > 4 ; 1 m enables, CSI > 4 ; 0 m disables.
            (b">", b'm') => {
                let groups: Vec<u16> = params
                    .iter()
                    .map(|group| group.first().copied().unwrap_or(0))
                    .collect();
                if groups.first() == Some(&4) {
                    self.modify_other_keys = groups.get(1).copied().unwrap_or(0).min(2) as u8;
                }
            }
            // DECSCUSR — Set Cursor Style: CSI Ps SP q (T3-6).
            // 0/1 block-blink, 2 block, 3 underline-blink, 4 underline,
            // 5 bar-blink, 6 bar. zsh-vi-mode and neovim send these.
            (b" ", b'q') => {
                self.cursor_shape =
                    (params.first().and_then(|p| p.first()).copied().unwrap_or(0) as u8).min(6);
            }
            _ => {
                log::trace!(
                    "unhandled CSI: intermediates={:?} final={:?} params={:?}",
                    intermediates,
                    final_byte as char,
                    params
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // OSC dispatch — Operating System Command handling
    // -----------------------------------------------------------------------

    fn handle_osc(&mut self, params: &[Vec<u8>]) {
        // Get the OSC command (first parameter) - parse as number from ASCII bytes
        let cmd = params
            .first()
            .and_then(|p| std::str::from_utf8(p).ok())
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0);

        match cmd {
            // OSC 0 / 2 — Set window title
            0 | 2 => {
                // Title is everything after the command, joined by semicolons
                if params.len() > 1 {
                    let title_parts: Vec<&[u8]> =
                        params[1..].iter().map(|p| p.as_slice()).collect();
                    let title = title_parts.join(&b';');
                    if let Ok(title_str) = std::str::from_utf8(&title) {
                        self.palette.set_title(title_str);
                    }
                }
            }
            // OSC 4 — Set/query color palette: OSC 4 ; index ; spec
            // A spec of '?' queries the current value (T2-4).
            4 => {
                if params.len() >= 3 {
                    if let Ok(index_str) = std::str::from_utf8(&params[1]) {
                        if let Ok(index) = index_str.parse::<u8>() {
                            let spec = &params[2];
                            if spec == b"?" {
                                if let Some((r, g, b)) = self.palette.get_color(index) {
                                    let resp = format!(
                                        "\x1b]4;{};{}\x1b\\",
                                        index,
                                        Grid::color_spec(r, g, b)
                                    );
                                    self.respond(resp.as_bytes());
                                }
                            } else if let Ok(spec_str) = std::str::from_utf8(spec) {
                                if let Some((r, g, b)) = parse_color_spec(spec_str) {
                                    self.palette.set_color(index, r, g, b);
                                    log::debug!(
                                        "OSC 4: Set color {} to ({}, {}, {})",
                                        index,
                                        r,
                                        g,
                                        b
                                    );
                                }
                            }
                        }
                    }
                }
            }
            // OSC 10 — Set/query foreground color: OSC 10 ; spec
            // A spec of '?' queries the current value (T2-4; neovim uses this).
            10 => {
                if params.len() >= 2 {
                    if params[1] == b"?" {
                        let (r, g, b) = self.palette.default_fg;
                        let resp = format!("\x1b]10;{}\x1b\\", Grid::color_spec(r, g, b));
                        self.respond(resp.as_bytes());
                    } else if let Ok(spec_str) = std::str::from_utf8(&params[1]) {
                        if let Some((r, g, b)) = parse_color_spec(spec_str) {
                            self.palette.set_fg(r, g, b);
                            log::debug!("OSC 10: Set foreground to ({}, {}, {})", r, g, b);
                        }
                    }
                }
            }
            // OSC 11 — Set/query background color: OSC 11 ; spec
            // A spec of '?' queries the current value (T2-4).
            11 => {
                if params.len() >= 2 {
                    if params[1] == b"?" {
                        let (r, g, b) = self.palette.default_bg;
                        let resp = format!("\x1b]11;{}\x1b\\", Grid::color_spec(r, g, b));
                        self.respond(resp.as_bytes());
                    } else if let Ok(spec_str) = std::str::from_utf8(&params[1]) {
                        if let Some((r, g, b)) = parse_color_spec(spec_str) {
                            self.palette.set_bg(r, g, b);
                            log::debug!("OSC 11: Set background to ({}, {}, {})", r, g, b);
                        }
                    }
                }
            }
            // OSC 12 — Set/query cursor color: OSC 12 ; spec
            // A spec of '?' queries the current value (T2-4).
            12 => {
                if params.len() >= 2 {
                    if params[1] == b"?" {
                        let (r, g, b) = self.palette.cursor_color;
                        let resp = format!("\x1b]12;{}\x1b\\", Grid::color_spec(r, g, b));
                        self.respond(resp.as_bytes());
                    } else if let Ok(spec_str) = std::str::from_utf8(&params[1]) {
                        if let Some((r, g, b)) = parse_color_spec(spec_str) {
                            self.palette.set_cursor_color(r, g, b);
                            log::debug!("OSC 12: Set cursor color to ({}, {}, {})", r, g, b);
                        }
                    }
                }
            }
            // OSC 52 — Clipboard (T1-6). Format: OSC 52 ; Pc ; Pd ST where
            // Pc is the selection target (c=clipboard, p=primary, …) and Pd
            // is base64-encoded data, '?' to query, or empty to clear.
            // The grid records the request; the app drains clipboard_set /
            // clipboard_query_requested and talks to the system clipboard.
            52 => {
                if params.len() >= 3 {
                    let data = &params[2];
                    if data == b"?" {
                        // Query: reply with current clipboard contents.
                        self.clipboard_query_requested = true;
                        log::debug!("OSC 52: clipboard query");
                    } else if data.is_empty() {
                        // Empty data: clear the clipboard (handled as set of "").
                        self.clipboard_set = Some(String::new());
                        log::debug!("OSC 52: clipboard clear");
                    } else {
                        use base64::Engine;
                        match base64::engine::general_purpose::STANDARD.decode(data) {
                            Ok(decoded) => match String::from_utf8(decoded) {
                                Ok(text) => {
                                    self.clipboard_set = Some(text);
                                    log::debug!("OSC 52: clipboard set");
                                }
                                Err(_) => log::warn!("OSC 52: data is not valid UTF-8"),
                            },
                            Err(_) => log::warn!("OSC 52: data is not valid base64"),
                        }
                    }
                }
            }
            // OSC 104 — Reset color palette
            104 => {
                self.palette.reset_palette();
                log::debug!("OSC 104: Reset color palette");
            }
            // OSC 112 — Reset cursor color
            112 => {
                self.palette.reset_cursor_color();
                log::debug!("OSC 112: Reset cursor color");
            }
            // OSC 8 — Hyperlinks: OSC 8 ; params ; URI
            8 => {
                if params.len() >= 3 {
                    // Parse parameters (e.g., "id=1")
                    let params_str = std::str::from_utf8(&params[1]).unwrap_or("");
                    let uri = std::str::from_utf8(&params[2]).unwrap_or("");

                    // Extract ID if present
                    let id = params_str
                        .split(';')
                        .find(|p| p.starts_with("id="))
                        .and_then(|p| p[3..].parse::<u32>().ok());

                    if uri.is_empty() {
                        // Empty URI = end hyperlink
                        self.active_hyperlink_id = 0;
                        log::debug!("OSC 8: End hyperlink");
                    } else {
                        // Start hyperlink
                        let link_id = id.unwrap_or(self.next_hyperlink_id);
                        self.hyperlinks.insert(link_id, uri.to_string());
                        self.active_hyperlink_id = link_id;
                        if id.is_none() {
                            self.next_hyperlink_id = link_id + 1;
                        }
                        log::debug!("OSC 8: Start hyperlink id={}: {}", link_id, uri);
                    }
                } else if params.len() == 1 {
                    // Just OSC 8 ; = end hyperlink
                    self.active_hyperlink_id = 0;
                    log::debug!("OSC 8: End hyperlink");
                }
            }
            _ => {
                log::trace!("Unhandled OSC {}: {:?}", cmd, params);
            }
        }
    }

    /// Get the hyperlink URL at a specific cell position
    pub fn get_hyperlink_at(&self, col: usize, row: usize) -> Option<&str> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        let cell = self.cell(col, row);
        let id = cell.hyperlink_id;
        if id == 0 {
            None
        } else {
            self.hyperlinks.get(&id).map(|s| s.as_str())
        }
    }

    /// Get the hyperlink URL for a cell
    pub fn get_cell_hyperlink(&self, cell: &Cell) -> Option<&str> {
        let id = cell.hyperlink_id;
        if id == 0 {
            None
        } else {
            self.hyperlinks.get(&id).map(|s| s.as_str())
        }
    }

    /// T4-5: Remove hyperlink map entries no longer referenced by any cell
    /// on the visible grid or in scrollback. Called periodically (e.g. on
    /// resize) to prevent unbounded growth from long-running sessions that
    /// emit many OSC 8 links.
    pub fn prune_hyperlinks(&mut self) {
        let mut live: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for row in 0..self.rows {
            for col in 0..self.cols {
                let id = self.cell(col, row).hyperlink_id;
                if id != 0 {
                    live.insert(id);
                }
            }
        }
        // Also scan scrollback (primary screen only — alt screen has none).
        for line in &self.scrollback {
            for cell in line {
                let id = cell.hyperlink_id;
                if id != 0 {
                    live.insert(id);
                }
            }
        }
        self.hyperlinks.retain(|id, _| live.contains(id));
    }
}

/// Parse a color specification string to RGB values.
/// Supports: #RRGGBB, rgb:RR/GG/BB, R/G/B (0-65535 range)
fn parse_color_spec(spec: &str) -> Option<(u8, u8, u8)> {
    if spec.starts_with('#') && spec.len() == 7 {
        // #RRGGBB format
        let r = u8::from_str_radix(&spec[1..3], 16).ok()?;
        let g = u8::from_str_radix(&spec[3..5], 16).ok()?;
        let b = u8::from_str_radix(&spec[5..7], 16).ok()?;
        Some((r, g, b))
    } else if spec.starts_with("rgb:") {
        // rgb:RR/GG/BB format (each 0-65535)
        let parts: Vec<&str> = spec[4..].split('/').collect();
        if parts.len() == 3 {
            let r = parse_color_component(parts[0])?;
            let g = parse_color_component(parts[1])?;
            let b = parse_color_component(parts[2])?;
            Some((r, g, b))
        } else {
            None
        }
    } else {
        // Try parsing as #RRGGBB without the #
        if spec.len() == 6 {
            let r = u8::from_str_radix(&spec[0..2], 16).ok()?;
            let g = u8::from_str_radix(&spec[2..4], 16).ok()?;
            let b = u8::from_str_radix(&spec[4..6], 16).ok()?;
            Some((r, g, b))
        } else {
            None
        }
    }
}

/// Parse a color component (0-65535 range to 0-255).
fn parse_color_component(s: &str) -> Option<u8> {
    let value = s.parse::<u16>().ok()?;
    // Convert from 0-65535 to 0-255
    Some((value / 257) as u8)
}

// ---------------------------------------------------------------------------
// Perform — connects parser to grid
// ---------------------------------------------------------------------------

impl Perform for Grid {
    fn perform(&mut self, action: Action) {
        match action {
            Action::Print(ch) => self.print(ch),

            Action::Execute(byte) => match byte {
                0x07 => {
                    // BEL — visual bell
                    self.bell_pending = true;
                    log::debug!("BEL: Visual bell triggered");
                }
                0x08 => {
                    // BS — backspace
                    if self.cursor.col > 0 {
                        self.cursor.col -= 1;
                    }
                }
                0x09 => {
                    // HT — horizontal tab: advance to the next tab stop
                    // (T3-9). If no stop remains, clamp to the last column.
                    let mut col = self.cursor.col;
                    loop {
                        col += 1;
                        if col >= self.cols - 1 {
                            col = self.cols - 1;
                            break;
                        }
                        if self.tab_stops.get(col).copied().unwrap_or(false) {
                            break;
                        }
                    }
                    self.cursor.col = col;
                }
                0x0a | 0x0b | 0x0c => {
                    // LF / VT / FF — line feed
                    if self.cursor.row == self.scroll_bottom {
                        self.scroll_up(1);
                    } else {
                        self.cursor.row += 1;
                    }
                }
                0x0d => {
                    // CR — carriage return
                    self.cursor.col = 0;
                }
                0x0e => {
                    // SO (LS1) — shift in G1 for subsequent prints (T3-8).
                    self.active_charset = 1;
                }
                0x0f => {
                    // SI (LS0) — shift back to G0 for subsequent prints (T3-8).
                    self.active_charset = 0;
                }
                _ => {}
            },

            Action::CsiDispatch {
                params,
                intermediates,
                ignore: _,
                final_byte,
            } => {
                self.handle_csi(&params, &intermediates, final_byte);
            }

            Action::OscDispatch { params } => {
                self.handle_osc(&params);
            }

            Action::EscDispatch {
                intermediates,
                ignore: _,
                final_byte,
            } => {
                match (intermediates.as_slice(), final_byte) {
                    // DECDHL/DECSWL/DECDWL — line presentation modes.
                    (&[b'#'], b'3') | (&[b'#'], b'4') | (&[b'#'], b'5') | (&[b'#'], b'6') => {
                        if self.cursor.row < self.line_modes.len() {
                            self.line_modes[self.cursor.row] = final_byte - b'0';
                            self.mark_all_dirty();
                        }
                    }
                    // DECALN — screen alignment display (T3-7): fill screen
                    // with 'E', home cursor, reset scroll region. vttest
                    // prerequisite.
                    (&[b'#'], b'8') => {
                        let rows = self.rows;
                        let cells = self.cells_mut();
                        for cell in cells.iter_mut() {
                            cell.ch = 'E';
                            cell.fg = Color::Default;
                            cell.bg = Color::Default;
                            cell.attrs = Attrs::default();
                            cell.dirty = true;
                            cell.wide_filler = false;
                            cell.hyperlink_id = 0;
                        }
                        drop(cells);
                        self.cursor.row = 0;
                        self.cursor.col = 0;
                        self.scroll_top = 0;
                        self.scroll_bottom = rows - 1;
                        self.origin_mode = false;
                    }
                    (_, b'7') => {
                        // DECSC — save cursor (per-screen, T3-14).
                        let slot = if self.alt_active {
                            &mut self.saved_cursor_alt
                        } else {
                            &mut self.saved_cursor
                        };
                        *slot = SavedCursor {
                            cursor: self.cursor,
                            fg: self.active_fg,
                            bg: self.active_bg,
                            attrs: self.active_attrs,
                        };
                    }
                    (_, b'8') => {
                        // DECRC — restore cursor (per-screen, T3-14).
                        let slot = if self.alt_active {
                            &self.saved_cursor_alt
                        } else {
                            &self.saved_cursor
                        };
                        self.cursor = slot.cursor;
                        self.active_fg = slot.fg;
                        self.active_bg = slot.bg;
                        self.active_attrs = slot.attrs;
                    }
                    (_, b'H') => {
                        // HTS — set a tab stop at the current column (T3-9).
                        if self.cursor.col < self.tab_stops.len() {
                            self.tab_stops[self.cursor.col] = true;
                        }
                    }
                    // --- Character set designation (T3-8) ---
                    // ESC ( X → designate G0, ESC ) X → designate G1.
                    // X is 'B' (US ASCII) or '0' (DEC Special Graphics).
                    (&[b'('], cs) => {
                        self.g0_charset = cs;
                    }
                    (&[b')'], cs) => {
                        self.g1_charset = cs;
                    }
                    // SO / SI (0x0E / 0x0F) are C0 controls handled in Action::Execute
                    // (LS1 / LS0 shift), not here.
                    (_, b'M') => {
                        // RI — reverse index (scroll down)
                        if self.cursor.row == self.scroll_top {
                            self.scroll_down(1);
                        } else {
                            self.cursor.row = self.cursor.row.saturating_sub(1);
                        }
                    }
                    (_, b'c') => {
                        // RIS — full reset
                        *self = Grid::new(
                            WinSize {
                                cols: self.cols as u16,
                                rows: self.rows as u16,
                            },
                            self.scrollback_capacity,
                        );
                        self.cursor_visible = true;
                    }
                    _ => {}
                }
            }

            Action::Hook {
                params,
                intermediates,
                final_byte,
                ..
            } => {
                // DCS start. Reset the passthrough buffer; remember whether it's
                // a DECRQSS request (`DCS 1 $ q`) so we can answer on Unhook.
                // DECRQSS: param=1, intermediate='$' (0x24), final='q' (0x71).
                self.dcs_buf.clear();
                self.dcs_request = params.first().and_then(|p| p.first()).copied() == Some(1)
                    && intermediates.contains(&b'$')
                    && final_byte == b'q';
            }
            Action::Put(byte) => {
                // Accumulate DCS data bytes (DECRQSS sends the query string).
                if self.dcs_buf.len() < MAX_OSC_DCS_LEN {
                    self.dcs_buf.push(byte);
                }
            }
            Action::Unhook => {
                // On DCS end, answer a DECRQSS request we recognized.
                if self.dcs_request {
                    self.answer_decrqss();
                }
                self.dcs_request = false;
                self.dcs_buf.clear();
            }
        }
    }

    /// Override with the optimized batch path for ASCII runs.
    fn print_ascii_run(&mut self, bytes: &[u8]) -> usize {
        self.print_ascii_run(bytes)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mouse::{MouseButton, MouseEvent, MouseEventType};

    fn make_grid(cols: u16, rows: u16) -> Grid {
        Grid::new(WinSize { cols, rows }, 1000)
    }

    fn feed(grid: &mut Grid, input: &[u8]) {
        let mut parser = crate::parser::Parser::new();
        for &b in input {
            parser.advance(grid, b);
        }
    }

    // -- Basic printing --

    #[test]
    fn test_print_single_char() {
        let mut g = make_grid(10, 5);
        feed(&mut g, b"A");
        assert_eq!(g.cell(0, 0).ch, 'A');
        assert_eq!(g.cursor.col, 1);
        assert_eq!(g.cursor.row, 0);
    }

    #[test]
    fn test_print_multiple_chars() {
        let mut g = make_grid(10, 5);
        feed(&mut g, b"Hello");
        assert_eq!(g.cell(0, 0).ch, 'H');
        assert_eq!(g.cell(4, 0).ch, 'o');
        assert_eq!(g.cursor.col, 5);
    }

    #[test]
    fn test_print_wraps_to_next_line() {
        let mut g = make_grid(5, 5);
        feed(&mut g, b"123456789");
        // First line: 12345, wraps at col 5
        assert_eq!(g.cell(0, 0).ch, '1');
        assert_eq!(g.cell(4, 0).ch, '5');
        // Second line: 6789
        assert_eq!(g.cell(0, 1).ch, '6');
        assert_eq!(g.cell(3, 1).ch, '9');
        assert_eq!(g.cursor.col, 4);
        assert_eq!(g.cursor.row, 1);
    }

    // -- Cursor movement --

    #[test]
    fn test_cursor_up() {
        let mut g = make_grid(10, 5);
        feed(&mut g, b"\x1b[3A"); // move up 3
        assert_eq!(g.cursor.row, 0); // can't go above 0
        assert_eq!(g.cursor.col, 0);
    }

    #[test]
    fn test_cursor_down() {
        let mut g = make_grid(10, 5);
        feed(&mut g, b"\x1b[3B"); // move down 3
        assert_eq!(g.cursor.row, 3); // 0-indexed, 1-based param
    }

    #[test]
    fn test_cursor_forward() {
        let mut g = make_grid(10, 5);
        feed(&mut g, b"\x1b[5C"); // move right 5
        assert_eq!(g.cursor.col, 5);
    }

    #[test]
    fn test_cursor_position() {
        let mut g = make_grid(10, 5);
        feed(&mut g, b"\x1b[3;7H"); // row 3, col 7
        assert_eq!(g.cursor.row, 2); // 0-indexed
        assert_eq!(g.cursor.col, 6);
    }

    // -- Scrolling --

    #[test]
    fn test_scroll_up_on_overflow() {
        let mut g = make_grid(10, 3);
        // Fill 3 lines
        feed(&mut g, b"Line1\r\nLine2\r\nLine3\r\n");
        assert_eq!(g.cursor.row, 2);
        // Print one more line — should scroll
        feed(&mut g, b"Line4\r\n");
        // After scroll, "Line1" should be in scrollback
        assert!(!g.scrollback.is_empty());
        // Cursor should still be on row 2
        assert_eq!(g.cursor.row, 2);
    }

    #[test]
    fn test_scroll_region() {
        let mut g = make_grid(10, 5);
        // Set scroll region to rows 2-4 (1-indexed: 3;5r)
        feed(&mut g, b"\x1b[3;5r");
        assert_eq!(g.scroll_top, 2);
        assert_eq!(g.scroll_bottom, 4);
    }

    // -- Erase operations --

    #[test]
    fn test_erase_display() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"Hello");
        assert_eq!(g.cell(0, 0).ch, 'H');
        // ED 2 = clear entire screen
        feed(&mut g, b"\x1b[2J");
        assert_eq!(g.cell(0, 0).ch, ' ');
    }

    #[test]
    fn test_erase_line() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"Hello");
        g.cursor.col = 2; // move to col 2
                          // EL 0 = clear from cursor to end of line
        feed(&mut g, b"\x1b[0K");
        assert_eq!(g.cell(0, 0).ch, 'H');
        assert_eq!(g.cell(1, 0).ch, 'e');
        assert_eq!(g.cell(2, 0).ch, ' '); // cleared
    }

    // -- SGR (colors and attributes) --

    #[test]
    fn test_sgr_bold() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[1mB");
        assert!(g.cell(0, 0).attrs.bold());
    }

    #[test]
    fn test_sgr_foreground_color() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[31mR"); // red foreground
        assert_eq!(g.cell(0, 0).fg, Color::Indexed(1));
    }

    #[test]
    fn test_sgr_background_color() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[44mB"); // blue background
        assert_eq!(g.cell(0, 0).bg, Color::Indexed(4));
    }

    #[test]
    fn test_sgr_truecolor() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[38;2;100;200;50mT"); // truecolor fg
        assert_eq!(g.cell(0, 0).fg, Color::Rgb(100, 200, 50));
    }

    #[test]
    fn test_sgr_reset() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[1;31mRB\x1b[0mX");
        assert!(g.cell(0, 0).attrs.bold());
        assert_eq!(g.cell(0, 0).fg, Color::Indexed(1));
        // After reset, 'X' should have default attributes
        assert!(!g.cell(2, 0).attrs.bold());
        assert_eq!(g.cell(2, 0).fg, Color::Default);
    }

    // -- Alternate screen --

    #[test]
    fn test_alternate_screen() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"Hello"); // write to primary
        assert_eq!(g.cell(0, 0).ch, 'H');

        // Enter alternate screen
        feed(&mut g, b"\x1b[?1049h");
        assert!(g.alt_active);
        assert_eq!(g.cell(0, 0).ch, ' '); // alt screen is empty

        // Write to alt screen
        feed(&mut g, b"Alt");
        assert_eq!(g.cell(0, 0).ch, 'A');

        // Exit alt screen — primary should be restored
        feed(&mut g, b"\x1b[?1049l");
        assert!(!g.alt_active);
        assert_eq!(g.cell(0, 0).ch, 'H'); // primary preserved
    }

    #[test]
    fn test_cursor_restore_on_exit_alt() {
        let mut g = make_grid(10, 5);
        // Move cursor to specific position, then enter alt screen
        feed(&mut g, b"\x1b[3;5H"); // row 3, col 5
        let saved_row = g.cursor.row;
        let saved_col = g.cursor.col;

        feed(&mut g, b"\x1b[?1049h"); // enter alt
                                      // Cursor should be at 0,0 in alt screen
        assert_eq!(g.cursor.row, 0);
        assert_eq!(g.cursor.col, 0);

        feed(&mut g, b"\x1b[?1049l"); // exit alt
                                      // Cursor should be restored to where it was before entering alt
        assert_eq!(g.cursor.row, saved_row);
        assert_eq!(g.cursor.col, saved_col);
    }

    // -- DECTCEM (cursor visibility) --

    #[test]
    fn test_cursor_visibility() {
        let mut g = make_grid(10, 3);
        assert!(g.cursor_visible); // default visible

        feed(&mut g, b"\x1b[?25l"); // hide cursor
        assert!(!g.cursor_visible);

        feed(&mut g, b"\x1b[?25h"); // show cursor
        assert!(g.cursor_visible);
    }

    // -- Resize --

    #[test]
    fn test_resize_larger() {
        let mut g = make_grid(5, 3);
        feed(&mut g, b"Hello");
        g.resize(WinSize { cols: 10, rows: 5 });
        assert_eq!(g.cols, 10);
        assert_eq!(g.rows, 5);
        assert_eq!(g.cell(0, 0).ch, 'H'); // content preserved
    }

    #[test]
    fn test_resize_reflows_primary_scrollback_without_losing_text() {
        let mut g = make_grid(6, 3);
        feed(&mut g, b"abcdef\n123456\nuvwxyz\nlast");
        assert!(!g.scrollback.is_empty());

        g.resize(WinSize { cols: 4, rows: 3 });

        let text = g.all_lines_with_scrollback().join("");
        assert!(text.contains("abcdef"));
        assert!(text.contains("123456"));
        assert!(text.contains("uvwxyz"));
    }

    #[test]
    fn test_resize_smaller() {
        let mut g = make_grid(10, 5);
        feed(&mut g, b"Hello");
        g.resize(WinSize { cols: 3, rows: 2 });
        assert_eq!(g.cols, 3);
        assert_eq!(g.rows, 2);
        // First 3 chars should still be there
        assert_eq!(g.cell(0, 0).ch, 'H');
        assert_eq!(g.cell(2, 0).ch, 'l');
    }

    // -- Backspace --

    #[test]
    fn test_backspace() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"AB\x08"); // BS
        assert_eq!(g.cursor.col, 1); // moved back
    }

    // -- Tab --

    #[test]
    fn test_horizontal_tab() {
        let mut g = make_grid(20, 3);
        feed(&mut g, b"\t"); // HT
        assert_eq!(g.cursor.col, 8); // tab stop at 8
    }

    // -- Carriage return --

    #[test]
    fn test_carriage_return() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"ABC\r");
        assert_eq!(g.cursor.col, 0); // back to column 0
    }

    // -- Wide characters --

    #[test]
    fn test_wide_character() {
        let mut g = make_grid(10, 3);
        // '中' is a double-width character (U+4E2D, UTF-8: 0xE4 0xB8 0xAD)
        feed(&mut g, "中".as_bytes());
        // After printing a wide char, cursor should advance by 2
        assert_eq!(g.cursor.col, 2);
        // The second cell should be marked as wide_filler
        assert!(g.cell(1, 0).wide_filler);
    }

    // -- Line feed at bottom with scroll region --

    #[test]
    fn test_lf_at_scroll_bottom() {
        let mut g = make_grid(10, 3);
        // Move to bottom row
        feed(&mut g, b"\x1b[3;1H"); // row 3
        assert_eq!(g.cursor.row, 2);

        // Print LF at bottom — should scroll
        feed(&mut g, b"\n");
        // Cursor should stay at row 2 (scroll happened)
        assert_eq!(g.cursor.row, 2);
    }

    // -- IL/DL act from the cursor row (T1-3) --

    #[test]
    fn test_il_inserts_at_cursor_row() {
        let mut g = make_grid(10, 5);
        feed(&mut g, b"L0\r\nL1\r\nL2\r\nL3\r\nL4");
        // Cursor at row 1 (after "L1\r\n"), insert 1 line
        g.cursor = crate::grid::Cursor { col: 0, row: 1 };
        let sb_before = g.scrollback.len();
        feed(&mut g, b"\x1b[1L");
        // Row 1 should now be blank; L1..L3 shifted down; L4 fell off.
        assert_eq!(g.cell(0, 1).ch, ' ');
        assert_eq!(g.cell(0, 2).ch, 'L');
        assert_eq!(g.cell(1, 2).ch, '1');
        assert_eq!(g.cell(0, 3).ch, 'L');
        assert_eq!(g.cell(1, 3).ch, '2');
        // Row 0 untouched
        assert_eq!(g.cell(0, 0).ch, 'L');
        assert_eq!(g.cell(1, 0).ch, '0');
        // IL must not add to scrollback
        assert_eq!(g.scrollback.len(), sb_before);
    }

    #[test]
    fn test_dl_deletes_at_cursor_row() {
        let mut g = make_grid(10, 5);
        feed(&mut g, b"L0\r\nL1\r\nL2\r\nL3\r\nL4");
        g.cursor = crate::grid::Cursor { col: 0, row: 1 };
        let sb_before = g.scrollback.len();
        feed(&mut g, b"\x1b[1M");
        // L1 deleted; L2 moved up into row 1.
        assert_eq!(g.cell(1, 1).ch, '2');
        assert_eq!(g.cell(1, 2).ch, '3');
        assert_eq!(g.cell(1, 3).ch, '4');
        // Bottom row blanked
        assert_eq!(g.cell(0, 4).ch, ' ');
        // Row 0 untouched
        assert_eq!(g.cell(1, 0).ch, '0');
        // DL'd lines are lost, never saved to scrollback
        assert_eq!(g.scrollback.len(), sb_before);
    }

    #[test]
    fn test_il_dl_noop_outside_scroll_region() {
        let mut g = make_grid(10, 5);
        // Region rows 2-4 (1-indexed 3;5r); cursor parked above it at row 0.
        feed(&mut g, b"\x1b[3;5r\x1b[H");
        assert_eq!(g.cursor.row, 0);
        feed(&mut g, b"\x1b[1L");
        feed(&mut g, b"\x1b[1M");
        // No effect on row 0 and no crash; region untouched as well.
        assert_eq!(g.cursor.row, 0);
    }

    // -- Zero-width combining characters (T1-8) --

    #[test]
    fn test_combining_mark_preserves_base_char() {
        let mut g = make_grid(10, 3);
        // "e" + U+0301 (combining acute accent, width 0). The mark must not
        // clobber the base character or consume a cell.
        feed(&mut g, b"e\xcc\x81x");
        assert_eq!(g.cell(0, 0).ch, 'e'); // base preserved
        assert_eq!(g.cell(0, 0).combining.as_deref(), Some("\u{0301}"));
        assert_eq!(g.cell(1, 0).ch, 'x'); // next char lands in col 1
        assert_eq!(g.cursor.col, 2); // only 2 cells consumed
    }

    #[test]
    fn test_arbitrary_combining_tail_remains_attached() {
        let mut g = make_grid(10, 3);
        feed(
            &mut g,
            b"a\xcc\x81\xcc\x82\xe2\x80\x8d\xef\xb8\x8f\xcc\x80b",
        );
        assert_eq!(
            g.cell(0, 0).combining.as_deref(),
            Some("\u{0301}\u{0302}\u{200d}\u{fe0f}\u{0300}")
        );
        assert_eq!(g.cell(1, 0).ch, 'b');
        assert_eq!(g.line_to_string(0).chars().count(), 15);
    }

    #[test]
    fn test_multiple_combining_marks_remain_attached() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"a\xcc\x81\xcc\x82b");
        assert_eq!(g.cell(0, 0).combining.as_deref(), Some("\u{0301}\u{0302}"));
        assert_eq!(g.cell(1, 0).ch, 'b');
    }

    #[test]
    fn test_combining_mark_at_col0_is_dropped() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\xcc\x81x"); // leading mark with no base cell
        assert_eq!(g.cell(0, 0).ch, 'x');
        assert_eq!(g.cursor.col, 1);
    }

    // -- OSC title --

    #[test]
    fn test_osc_title_does_not_crash() {
        let mut g = make_grid(10, 3);
        // OSC 0 ; title BEL
        feed(&mut g, b"\x1b]0;My Title\x07");
        // No crash, title is logged
    }

    // -- Full reset (RIS) --

    #[test]
    fn test_full_reset() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[1mB"); // bold + print B
        assert!(g.cell(0, 0).attrs.bold());
        assert_eq!(g.cell(0, 0).ch, 'B');

        feed(&mut g, b"\x1bc"); // RIS
        assert_eq!(g.cell(0, 0).ch, ' ');
        assert!(!g.cell(0, 0).attrs.bold());
        assert!(g.cursor_visible); // reset
    }

    // -- OSC title --

    #[test]
    fn test_osc_title_sets_palette() {
        let mut g = make_grid(10, 3);
        // OSC 2 ; title BEL
        feed(&mut g, b"\x1b]2;My Terminal\x07");
        assert_eq!(g.palette.title, "My Terminal");
    }

    // -- OSC color palette --

    #[test]
    fn test_osc4_set_color() {
        let mut g = make_grid(10, 3);
        // OSC 4 ; 0 ; #FF0000 BEL (set color 0 to red)
        // Parser splits at ;, so params = ["4", "0", "#FF0000"]
        feed(&mut g, b"\x1b]4;0;#FF0000\x07");
        assert_eq!(g.palette.get_color(0), Some((255, 0, 0)));
    }

    #[test]
    fn test_osc10_set_fg() {
        let mut g = make_grid(10, 3);
        // OSC 10 ; #00FF00 BEL (set fg to green)
        // Parser splits at ;, so params = ["10", "#00FF00"]
        feed(&mut g, b"\x1b]10;#00FF00\x07");
        assert_eq!(g.palette.default_fg, (0, 255, 0));
    }

    #[test]
    fn test_osc11_set_bg() {
        let mut g = make_grid(10, 3);
        // OSC 11 ; #0000FF BEL (set bg to blue)
        // Parser splits at ;, so params = ["11", "#0000FF"]
        feed(&mut g, b"\x1b]11;#0000FF\x07");
        assert_eq!(g.palette.default_bg, (0, 0, 255));
    }

    #[test]
    fn test_osc12_set_cursor_color() {
        let mut g = make_grid(10, 3);
        // OSC 12 ; #FFFF00 BEL (set cursor to yellow)
        // Parser splits at ;, so params = ["12", "#FFFF00"]
        feed(&mut g, b"\x1b]12;#FFFF00\x07");
        assert_eq!(g.palette.cursor_color, (255, 255, 0));
    }

    #[test]
    fn test_osc104_reset_palette() {
        let mut g = make_grid(10, 3);
        // First modify a color
        feed(&mut g, b"\x1b]4;0;#FF0000\x07");
        assert_eq!(g.palette.get_color(0), Some((255, 0, 0)));
        // Then reset
        feed(&mut g, b"\x1b]104\x07");
        // Should be back to default black
        assert_eq!(g.palette.get_color(0), Some((0, 0, 0)));
    }

    #[test]
    fn test_osc112_reset_cursor_color() {
        let mut g = make_grid(10, 3);
        // Modify cursor color
        feed(&mut g, b"\x1b]12;#FFFF00\x07");
        assert_eq!(g.palette.cursor_color, (255, 255, 0));
        // Reset
        feed(&mut g, b"\x1b]112\x07");
        // Should be back to default blue
        assert_eq!(g.palette.cursor_color, (137, 180, 250));
    }

    // -- OSC 52 clipboard (T1-6) --

    #[test]
    fn test_osc52_set_records_request() {
        let mut g = make_grid(10, 3);
        // OSC 52 ; c ; base64("hello world") BEL
        feed(&mut g, b"\x1b]52;c;aGVsbG8gd29ybGQ=\x07");
        assert_eq!(g.clipboard_set.take(), Some("hello world".to_string()));
        assert!(!g.clipboard_query_requested);
    }

    #[test]
    fn test_osc52_query_sets_flag() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b]52;c;?\x07");
        assert!(g.clipboard_query_requested);
        assert!(g.clipboard_set.is_none());
    }

    #[test]
    fn test_osc52_st_terminated() {
        // OSC 52 terminated by ESC \ (ST) — exercises the T1-2 fix path too.
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b]52;c;aGk=\x1b\\");
        assert_eq!(g.clipboard_set.take(), Some("hi".to_string()));
    }

    // -- Bracketed paste mode --

    #[test]
    fn test_bracketed_paste_mode() {
        let mut g = make_grid(10, 3);
        assert!(!g.bracketed_paste); // default disabled

        feed(&mut g, b"\x1b[?2004h"); // enable
        assert!(g.bracketed_paste);

        feed(&mut g, b"\x1b[?2004l"); // disable
        assert!(!g.bracketed_paste);
    }

    // -- Bell --

    #[test]
    fn test_bell_sets_pending() {
        let mut g = make_grid(10, 3);
        assert!(!g.bell_pending);

        feed(&mut g, b"\x07"); // BEL
        assert!(g.bell_pending);
    }

    // -- Color palette --

    #[test]
    fn test_color_palette_rgb_conversion() {
        let palette = ColorPalette::default();
        let color = Color::Indexed(1); // Red
        assert_eq!(color.to_rgb(&palette), Some((205, 49, 49)));
    }

    #[test]
    fn test_color_palette_custom_color() {
        let mut palette = ColorPalette::default();
        palette.set_color(100, 123, 45, 67);
        assert_eq!(palette.get_color(100), Some((123, 45, 67)));
    }

    #[test]
    fn test_dirty_cells_after_print() {
        let mut g = make_grid(10, 3);
        // Initially all cells are dirty (Cell::default sets dirty: true)
        assert!(g.has_dirty_cells());

        // Clear initial dirty flags
        let _ = g.take_dirty_cells();

        // No dirty cells now
        assert!(!g.has_dirty_cells());

        // Print some text - should mark cells dirty
        feed(&mut g, b"Hello");
        assert!(g.has_dirty_cells());

        // Take dirty cells - should return them and clear flags
        let dirty = g.take_dirty_cells();
        assert!(dirty.len() >= 5); // At least 5 cells for "Hello"

        // Second call should return empty (flags cleared)
        let dirty2 = g.take_dirty_cells();
        assert!(dirty2.is_empty());

        // No dirty cells now
        assert!(!g.has_dirty_cells());
    }

    #[test]
    fn test_scrollback_capacity_enforcement() {
        // Grid with only 3 lines of scrollback capacity
        let mut g = Grid::new(WinSize { cols: 10, rows: 3 }, 3);

        // Fill the visible area and scroll up to generate scrollback
        feed(&mut g, b"Line1\r\nLine2\r\nLine3\r\n");
        // Cursor is now at row 3, which will cause scroll up
        feed(&mut g, b"Line4\r\n");
        feed(&mut g, b"Line5\r\n");
        feed(&mut g, b"Line6\r\n");
        feed(&mut g, b"Line7\r\n");

        // Scrollback should never exceed capacity
        assert!(g.scrollback.len() <= 3);
    }

    // -- Scrollback view mapping (T1-4) --

    #[test]
    fn test_cell_at_view_no_offset_is_identity() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"ABC");
        // With no offset the view maps straight onto the live grid.
        assert_eq!(g.cell_at_view(0, 0).map(|c| c.ch), Some('A'));
        assert_eq!(g.cell_at_view(2, 0).map(|c| c.ch), Some('C'));
    }

    #[test]
    fn test_cell_at_view_with_offset_shows_scrollback() {
        let mut g = make_grid(10, 3);
        // 5 lines through a 3-row grid → 2 lines land in scrollback.
        feed(&mut g, b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        assert_eq!(g.scrollback.len(), 2);
        assert_eq!(g.cell(0, 0).ch, 't'); // live grid shows three/four/five

        // Scroll up by 1 line: top screen row shows "two" (newest scrollback).
        g.scrollback_offset = 1;
        assert_eq!(g.view_scrollback_lines(), 1);
        assert_eq!(g.cell_at_view(0, 0).map(|c| c.ch), Some('t'));
        assert_eq!(g.cell_at_view(1, 0).map(|c| c.ch), Some('w'));
        // Live grid shifts down by the offset.
        assert_eq!(g.cell_at_view(0, 1).map(|c| c.ch), Some('t')); // "three"
        assert_eq!(g.cell_at_view(0, 2).map(|c| c.ch), Some('f')); // "four"

        // Scroll up by 2 lines: both scrollback lines visible on top.
        g.scrollback_offset = 2;
        assert_eq!(g.cell_at_view(0, 0).map(|c| c.ch), Some('o')); // "one"
        assert_eq!(g.cell_at_view(0, 1).map(|c| c.ch), Some('t')); // "two"
        assert_eq!(g.cell_at_view(0, 2).map(|c| c.ch), Some('t')); // "three"

        // Offset capped by buffer size; far out-of-bounds rows return None.
        g.scrollback_offset = 5;
        assert_eq!(g.view_scrollback_lines(), 2);
        assert!(g.cell_at_view(0, g.rows + 10).is_none());
        assert!(g.cell_at_view(g.cols, 0).is_none());
    }

    // -- Device query responses (T2) --

    #[test]
    fn test_da1_responds_vt220() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[c");
        let responses = g.take_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], b"\x1b[?62;22c");
        // Second call yields nothing (outbox drained).
        assert!(g.take_responses().is_empty());
    }

    #[test]
    fn test_da1_no_response_for_nonzero_request() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[1c");
        assert!(g.take_responses().is_empty());
    }

    #[test]
    fn test_da2_responds() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[>c");
        let responses = g.take_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], b"\x1b[>1;20;0c");
    }

    #[test]
    fn test_dsr_ok() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[5n");
        let responses = g.take_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], b"\x1b[0n");
    }

    #[test]
    fn test_cpr_reports_cursor_position() {
        let mut g = make_grid(10, 5);
        feed(&mut g, b"\x1b[3;7H\x1b[6n");
        let responses = g.take_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], b"\x1b[3;7R");
    }

    #[test]
    fn test_cpr_reports_origin_relative_row_with_decom() {
        let mut g = make_grid(10, 8);
        feed(&mut g, b"\x1b[3;6r\x1b[?6h\x1b[2;4H\x1b[6n");
        let responses = g.take_responses();
        assert_eq!(responses, vec![b"\x1b[2;4R".to_vec()]);
    }

    #[test]
    fn test_decxcpr_reports_origin_relative_row_with_decom() {
        let mut g = make_grid(10, 8);
        feed(&mut g, b"\x1b[3;6r\x1b[?6h\x1b[2;4H\x1b[?6n");
        let responses = g.take_responses();
        assert_eq!(responses, vec![b"\x1b[?2;4;1R".to_vec()]);
    }

    #[test]
    fn test_decxcpr_reports_page() {
        let mut g = make_grid(10, 5);
        feed(&mut g, b"\x1b[2;4H\x1b[?6n");
        let responses = g.take_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], b"\x1b[?2;4;1R");
    }

    #[test]
    fn test_decrqm_known_modes() {
        let mut g = make_grid(10, 3);
        // DECTCEM defaults to set (cursor visible).
        feed(&mut g, b"\x1b[?25$p");
        assert_eq!(g.take_responses()[0], b"\x1b[?25;1$y");
        // Hide cursor, then query again.
        feed(&mut g, b"\x1b[?25l\x1b[?25$p");
        assert_eq!(g.take_responses()[0], b"\x1b[?25;2$y");
        // Unknown private mode → 0.
        feed(&mut g, b"\x1b[?1234$p");
        assert_eq!(g.take_responses()[0], b"\x1b[?1234;0$y");
    }

    #[test]
    fn test_decrqm_ansi_modes() {
        let mut g = make_grid(10, 3);
        // IRM (ANSI 4) is tracked; defaults to reset (Ps=2).
        feed(&mut g, b"\x1b[4$p");
        assert_eq!(g.take_responses()[0], b"\x1b[4;2$y");
        // After enabling IRM it reports set.
        feed(&mut g, b"\x1b[4h\x1b[4$p");
        assert_eq!(g.take_responses()[0], b"\x1b[4;1$y");
        // Untracked ANSI mode → Ps=0.
        feed(&mut g, b"\x1b[20$p");
        assert_eq!(g.take_responses()[0], b"\x1b[20;0$y");
    }

    #[test]
    fn test_osc10_query_responds() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b]10;?\x07");
        let responses = g.take_responses();
        assert_eq!(responses.len(), 1);
        // Default fg is (229,229,229) → 0xe5*257 = 0xe5e5.
        assert_eq!(
            std::str::from_utf8(&responses[0]).unwrap(),
            "\x1b]10;rgb:e5e5/e5e5/e5e5\x1b\\"
        );
    }

    #[test]
    fn test_osc11_query_after_set() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b]11;#ff0000\x07\x1b]11;?\x07");
        let responses = g.take_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(
            std::str::from_utf8(&responses[0]).unwrap(),
            "\x1b]11;rgb:ffff/0000/0000\x1b\\"
        );
    }

    #[test]
    fn test_osc4_query_responds() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b]4;0;?\x07");
        let responses = g.take_responses();
        assert_eq!(responses.len(), 1);
        // Palette index 0 defaults to black.
        assert_eq!(
            std::str::from_utf8(&responses[0]).unwrap(),
            "\x1b]4;0;rgb:0000/0000/0000\x1b\\"
        );
    }

    #[test]
    fn test_color_spec_formatting() {
        assert_eq!(Grid::color_spec(0, 0, 0), "rgb:0000/0000/0000");
        assert_eq!(Grid::color_spec(255, 255, 255), "rgb:ffff/ffff/ffff");
        assert_eq!(Grid::color_spec(0x80, 0, 0xff), "rgb:8080/0000/ffff");
    }

    // -- Tier-3 private modes --

    #[test]
    fn test_decckm_flag() {
        let mut g = make_grid(10, 3);
        assert!(!g.application_cursor_keys);
        feed(&mut g, b"\x1b[?1h");
        assert!(g.application_cursor_keys);
        feed(&mut g, b"\x1b[?1l");
        assert!(!g.application_cursor_keys);
        // DECRQM reports it.
        feed(&mut g, b"\x1b[?1h\x1b[?1$p");
        assert_eq!(g.take_responses()[0], b"\x1b[?1;1$y");
    }

    #[test]
    fn test_decom_region_relative_cup() {
        let mut g = make_grid(10, 8);
        feed(&mut g, b"\x1b[2;5r"); // scroll region rows 2..5 (1-based)
        feed(&mut g, b"\x1b[?6h"); // DECOM on — cursor homes to region top
        assert_eq!(g.cursor.row, 1); // scroll_top (0-based)
        assert_eq!(g.cursor.col, 0);
        // CUP 1;1 lands at region top, not screen top.
        feed(&mut g, b"\x1b[1;1H");
        assert_eq!(g.cursor.row, 1);
        // Row param clamps inside the region bottom (row 4, 0-based).
        feed(&mut g, b"\x1b[9;1H");
        assert_eq!(g.cursor.row, 4);
        // Reset DECOM: CUP is absolute again.
        feed(&mut g, b"\x1b[?6l\x1b[1;1H");
        assert_eq!(g.cursor.row, 0);
    }

    #[test]
    fn test_decawm_off_clamps() {
        let mut g = make_grid(4, 3);
        feed(&mut g, b"\x1b[?7l"); // autowrap off
        feed(&mut g, b"ABCDEF");
        // No wrap: stays on row 0; last cell(s) hold the tail.
        assert_eq!(g.cursor.row, 0);
        assert_eq!(g.cell(0, 0).ch, 'A');
        assert_eq!(g.cell(3, 0).ch, 'F');
        // Autowrap back on (default behaviour): wraps again.
        feed(&mut g, b"\x1b[2J\x1b[H\x1b[?7h");
        feed(&mut g, b"ABCDE");
        assert_eq!(g.cursor.row, 1);
    }

    #[test]
    fn test_decscnm_flag() {
        let mut g = make_grid(10, 3);
        assert!(!g.screen_reverse);
        feed(&mut g, b"\x1b[?5h");
        assert!(g.screen_reverse);
        feed(&mut g, b"\x1b[?5l");
        assert!(!g.screen_reverse);
    }

    #[test]
    fn test_decscusr_sets_shape() {
        let mut g = make_grid(10, 3);
        assert_eq!(g.cursor_shape, 0);
        feed(&mut g, b"\x1b[4 q"); // underline steady
        assert_eq!(g.cursor_shape, 4);
        feed(&mut g, b"\x1b[6 q"); // bar steady
        assert_eq!(g.cursor_shape, 6);
        // Out-of-range clamps to 6.
        feed(&mut g, b"\x1b[9 q");
        assert_eq!(g.cursor_shape, 6);
        // No param → 0.
        feed(&mut g, b"\x1b[ q");
        assert_eq!(g.cursor_shape, 0);
    }

    #[test]
    fn test_decaln_fills_screen() {
        let mut g = make_grid(4, 3);
        feed(&mut g, b"junk text here");
        feed(&mut g, b"\x1b#8");
        for row in 0..3 {
            for col in 0..4 {
                assert_eq!(g.cell(col, row).ch, 'E');
            }
        }
        assert_eq!(g.cursor.row, 0);
        assert_eq!(g.cursor.col, 0);
        // Scroll region reset to full screen.
        assert_eq!(g.scroll_top, 0);
        assert_eq!(g.scroll_bottom, 2);
    }

    #[test]
    fn test_irm_inserts() {
        let mut g = make_grid(8, 3);
        feed(&mut g, b"ACDE");
        feed(&mut g, b"\x1b[1;2H"); // col 2 (between A and C)
        feed(&mut g, b"\x1b[4h"); // IRM on
        feed(&mut g, b"B");
        feed(&mut g, b"\x1b[4l");
        let line: String = (0..5).map(|c| g.cell(c, 0).ch).collect();
        assert_eq!(line, "ABCDE");
    }

    #[test]
    fn test_deccolm_accepted_harmlessly() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[?3h"); // must not corrupt
        assert_eq!(g.cols, 80);
        assert_eq!(g.rows, 24);
        feed(&mut g, b"\x1b[?3l");
        assert_eq!(g.cols, 80);
    }

    // -- Tier-3 Batch B: tab stops, ICH, REP, ED 3, per-screen cursor --

    #[test]
    fn test_tab_default_stops_every_8() {
        let g = make_grid(40, 3);
        // Power-up state: stops at 8,16,24,32 (not col 0).
        assert!(!g.tab_stops[0]);
        assert!(g.tab_stops[8]);
        assert!(!g.tab_stops[9]);
        assert!(g.tab_stops[16]);
        assert!(g.tab_stops[32]);
    }

    #[test]
    fn test_ht_advances_to_next_stop() {
        let mut g = make_grid(40, 3);
        feed(&mut g, b"X\t"); // 'X' then tab
        assert_eq!(g.cell(0, 0).ch, 'X');
        // Cursor should land on col 8 (the next stop after col 1).
        assert_eq!(g.cursor.col, 8);
    }

    #[test]
    fn test_hts_sets_and_tbc_clears_stop() {
        let mut g = make_grid(40, 3);
        feed(&mut g, b"\x1b[3G"); // move to col 3 (1-based)
        feed(&mut g, b"\x1bH"); // HTS — set stop at col 2
        assert!(g.tab_stops[2]);
        feed(&mut g, b"\x1b[g"); // TBC — clear stop at cursor column
        assert!(!g.tab_stops[2]);
    }

    #[test]
    fn test_tbc_3_clears_all_stops() {
        let mut g = make_grid(40, 3);
        feed(&mut g, b"\x1b[3g"); // TBC 3 — clear all
        assert!(!g.tab_stops.iter().any(|&s| s));
    }

    #[test]
    fn test_ich_shifts_row_right() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"ABCD");
        feed(&mut g, b"\x1b[1;2H"); // col 2 (between A and B)
        feed(&mut g, b"\x1b[2@"); // ICH 2 — insert 2 blanks
        let line: String = (0..6).map(|c| g.cell(c, 0).ch).collect();
        assert_eq!(line, "A  BCD");
        // ICH does NOT move the cursor; it stays at the insert position.
        assert_eq!(g.cursor.col, 1);
    }

    #[test]
    fn test_rep_repeats_last_char() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"X"); // last char = 'X'
        feed(&mut g, b"\x1b[4b"); // REP 4 -> 4 more 'X' (5 total incl. original)
        let line: String = (0..5).map(|c| g.cell(c, 0).ch).collect();
        assert_eq!(line, "XXXXX");
    }

    #[test]
    fn test_ed_3_clears_scrollback() {
        let mut g = Grid::new(WinSize { cols: 10, rows: 3 }, 100);
        feed(&mut g, b"L1\r\nL2\r\nL3\r\nL4\r\nL5"); // scrolls, builds history
        assert!(!g.scrollback.is_empty());
        feed(&mut g, b"\x1b[3J"); // ED 3 — also wipe history
        assert!(g.scrollback.is_empty());
    }

    #[test]
    fn test_decsc_decre_per_screen_isolated() {
        // Save cursor on primary, switch to alternate, save a different
        // cursor, restore on each — they must not bleed across screens.
        let mut g = make_grid(10, 5);
        feed(&mut g, b"\x1b[2;3H");
        feed(&mut g, b"\x1b7"); // DECSC on primary -> (row1,col2)
        feed(&mut g, b"\x1b[?1049h"); // enter alt
        feed(&mut g, b"\x1b[4;5H");
        feed(&mut g, b"\x1b7"); // DECSC on alt -> (row3,col4)
        feed(&mut g, b"\x1b8"); // DECRC on alt -> restore alt save
        assert_eq!(g.cursor.row, 3);
        assert_eq!(g.cursor.col, 4);
        feed(&mut g, b"\x1b[?1049l"); // back to primary
        feed(&mut g, b"\x1b8"); // DECRC on primary -> restore primary save
        assert_eq!(g.cursor.row, 1);
        assert_eq!(g.cursor.col, 2);
    }

    // -- Tier-3 T3-8: character sets --

    #[test]
    fn test_dec_special_graphics_box_drawing() {
        // ESC ( 0 selects DEC Special Graphics for G0. With it active, the
        // ASCII codepoints should be remapped to box-drawing glyphs:
        // 'q' (0x71) -> '─', 'x' (0x78) -> '│', 'l' (0x6C) -> '┌'.
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b(0"); // designate G0 = DEC Special Graphics
        feed(&mut g, b"qxl"); // prints ─ │ ┌
        assert_eq!(g.cell(0, 0).ch, '─');
        assert_eq!(g.cell(1, 0).ch, '│');
        assert_eq!(g.cell(2, 0).ch, '┌');
    }

    #[test]
    fn test_charset_reset_to_ascii() {
        // ESC ( B restores US ASCII; the same byte then prints literally.
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b(0");
        feed(&mut g, b"q"); // box-drawing ─
        assert_eq!(g.cell(0, 0).ch, '─');
        feed(&mut g, b"\x1b(B"); // back to US ASCII
        feed(&mut g, b"q"); // literal 'q'
        assert_eq!(g.cell(1, 0).ch, 'q');
    }

    #[test]
    fn test_so_si_charset_shift() {
        // G1 can hold DEC Special Graphics; SO shifts to it, SI back to G0.
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b)0"); // designate G1 = DEC Special Graphics
        feed(&mut g, b"q"); // G0 still ASCII -> literal 'q'
        assert_eq!(g.cell(0, 0).ch, 'q');
        feed(&mut g, b"\x0e"); // SO -> activate G1
        feed(&mut g, b"q"); // now box-drawing ─
        assert_eq!(g.cell(1, 0).ch, '─');
        feed(&mut g, b"\x0f"); // SI -> back to G0
        feed(&mut g, b"q"); // literal 'q'
        assert_eq!(g.cell(2, 0).ch, 'q');
    }

    // -- Tier-3 remaining items: CBT, modified keys, DCS, OSC cap, mouse, SGR colon --

    #[test]
    fn test_cbt_backward_tab() {
        let mut g = make_grid(40, 3);
        feed(&mut g, b"TAB\t"); // T,A,B → col 3, HT → col 8 (stop)
        assert_eq!(g.cursor.col, 8);
        feed(&mut g, b"\x1b[Z"); // CBT -> back to col 0
        assert_eq!(g.cursor.col, 0);
    }

    #[test]
    fn test_decrqss_answered() {
        // `DECRQSS m` should produce a DCS reply reporting the SGR state.
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1bP1$qm\x1b\\"); // DCS 1 $ q  m  ST
        let resp = g.take_responses();
        assert!(resp
            .iter()
            .any(|r| r.starts_with(b"\x1bP1$r") && r.ends_with(b"\x1b\\")));
    }

    #[test]
    fn test_sgr_colon_truecolor() {
        // T3-20: colon sub-parameters `38:2::r:g:b` (tone slot empty).
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[38:2::255:0:0m");
        match g.active_fg {
            Color::Rgb(r, gg, b) => {
                assert_eq!((r, gg, b), (255, 0, 0));
            }
            other => panic!("expected RGB, got {:?}", other),
        }
        // Background colon form as well.
        feed(&mut g, b"\x1b[48:2:0:0:255m");
        match g.active_bg {
            Color::Rgb(r, gg, b) => {
                assert_eq!((r, gg, b), (0, 0, 255));
            }
            other => panic!("expected RGB, got {:?}", other),
        }
    }

    #[test]
    fn test_sgr_extended_underline_styles() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[4:2mX");
        assert_eq!(g.cell(0, 0).attrs.underline_style(), 2);
        feed(&mut g, b"\x1b[4:5mY");
        assert_eq!(g.cell(1, 0).attrs.underline_style(), 5);
        feed(&mut g, b"\x1b[24mZ");
        assert_eq!(g.cell(2, 0).attrs.underline_style(), 0);
    }

    #[test]
    fn test_sgr_colon_indexed() {
        // T3-20: colon form of indexed color `38:5:208`.
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[38:5:208m");
        match g.active_fg {
            Color::Indexed(i) => assert_eq!(i, 208),
            other => panic!("expected Indexed, got {:?}", other),
        }
    }

    #[test]
    fn test_mouse_urxvt_encoding() {
        // T3-19: mode 1015 emits urxvt-style `CSI Cb ; Cx ; Cy M`.
        let me = MouseEvent {
            button: MouseButton::Left,
            event_type: MouseEventType::Press,
            col: 10,
            row: 5,
            shift: false,
            ctrl: false,
            alt: false,
        };
        assert_eq!(me.encode(MouseEncoding::Urxvt), "\x1b[0;11;6M");
    }

    // -- Tier 4: focus reporting, synchronized output, hyperlink pruning --

    #[test]
    fn test_focus_reporting() {
        let mut g = make_grid(10, 3);
        // Off by default — no response.
        g.focus_in();
        g.focus_out();
        assert!(g.take_responses().is_empty());

        // Enable mode 1004.
        feed(&mut g, b"\x1b[?1004h");
        g.focus_in();
        let resp = g.take_responses();
        assert_eq!(resp, vec![b"\x1b[I".to_vec()]);
        g.focus_out();
        let resp = g.take_responses();
        assert_eq!(resp, vec![b"\x1b[O".to_vec()]);

        // Disable — no more responses.
        feed(&mut g, b"\x1b[?1004l");
        g.focus_in();
        assert!(g.take_responses().is_empty());
    }

    #[test]
    fn test_double_width_line_mode() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b#6");
        assert_eq!(g.line_mode(0), 6);
    }

    #[test]
    fn test_keyboard_protocol_negotiation() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[>1u");
        assert!(g.kitty_keyboard);
        feed(&mut g, b"\x1b[<u");
        assert!(!g.kitty_keyboard);
        feed(&mut g, b"\x1b[>4;1m");
        assert_eq!(g.modify_other_keys, 1);
        feed(&mut g, b"\x1b[>4;0m");
        assert_eq!(g.modify_other_keys, 0);
    }

    #[test]
    fn test_synchronized_output_mode() {
        let mut g = make_grid(10, 3);
        assert!(!g.synchronized_output);
        feed(&mut g, b"\x1b[?2026h");
        assert!(g.synchronized_output);
        feed(&mut g, b"\x1b[?2026l");
        assert!(!g.synchronized_output);
    }

    #[test]
    fn test_hyperlink_prune() {
        let mut g = make_grid(10, 2);
        // OSC 8 ; ; https://example.com ST — start hyperlink, print, end.
        feed(&mut g, b"\x1b]8;;https://example.com\x07X\x1b]8;;\x07");
        assert_eq!(g.hyperlinks.len(), 1);
        // Overwrite the cell with a non-hyperlink char — the link is now orphaned.
        feed(&mut g, b"\rY");
        // After prune, the orphaned entry should be gone.
        g.prune_hyperlinks();
        assert!(g.hyperlinks.is_empty());
    }

    #[test]
    fn test_all_lines_with_scrollback() {
        let mut g = make_grid(5, 2);
        g.scrollback_capacity = 10;
        feed(&mut g, b"abc\r\ndef\r\n");
        // After two CRLFs on a 2-row grid, "abc" should be in scrollback.
        let lines = g.all_lines_with_scrollback();
        assert!(lines.iter().any(|l| l.contains("abc")));
        assert!(lines.iter().any(|l| l.contains("def")));
    }
}
