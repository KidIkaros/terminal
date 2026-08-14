//! Terminal grid — Layer 3.
//!
//! A 2-D array of [`Cell`]s driven by the VT parser's `Action` stream.
//! The grid owns all terminal state: cursor position, active SGR attributes,
//! scroll region, and the alternate screen buffer.

use std::collections::VecDeque;
use std::sync::Arc;

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
/// DECSCA protection: protected cells survive selective erase (DECSED/DECSEL).
const PROTECTED: u16 = 1 << 12;

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
    pub fn protected(&self) -> bool {
        self.0 & PROTECTED != 0
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
    pub fn set_protected(&mut self, v: bool) {
        if v {
            self.0 |= PROTECTED;
        } else {
            self.0 &= !PROTECTED;
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
    /// `Box<str>` (16 B) instead of `String` (24 B): combining marks are
    /// frozen after append, and the smaller field shrinks Cell from 48 to
    /// 40 bytes — less per-cell copy and scroll-blank traffic on the hot path.
    pub combining: Option<Box<str>>,
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
// Sixel payloads routinely exceed the OSC cap (a 640x480 image is ~100-300KB
// of run-length-encoded data); allow up to 2MB, still bounded against
// pathological allocation.
const MAX_DCS_LEN: usize = 2_000_000;

/// The terminal grid buffer and state.
pub struct Grid {
    pub cols: usize,
    pub rows: usize,

    // Two screens: primary and alternate (toggled by ?1049h / ?1049l).
    // Each screen is a row-major array of rows (`Vec<Cell>` per row) so that
    // scrolling rotates row handles instead of copying every cell — the hot
    // path under heavy output like `cat bigfile`.
    cells_primary: Vec<Arc<Vec<Cell>>>,
    cells_alt: Vec<Arc<Vec<Cell>>>,
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
    /// DECSLRM left/right margins (inclusive, 0-indexed). Full width by
    /// default; honored only while DECLRMM (?69) is set.
    pub scroll_left: usize,
    pub scroll_right: usize,
    /// DECLRMM (?69): left/right margin mode. When off, the left/right
    /// margins are always the full screen width.
    pub left_right_margins: bool,
    /// DEC line presentation modes: 0 normal, 3/4 double-height halves,
    /// 6 double-width.
    line_modes: Vec<u8>,

    // Scrollback buffer (primary screen only). VecDeque gives O(1) eviction
    // of the oldest line (T1-7); a Vec required remove(0), which is O(n)
    // and burned CPU under heavy scroll output like `cat bigfile`.
    pub scrollback: std::collections::VecDeque<Arc<Vec<Cell>>>,
    /// Shared blank row: every blank grid slot points at this one Arc, so a
    /// scroll bumps a refcount instead of allocating + memsetting a fresh
    /// row. Blank-slot dirtiness is tracked per row in `row_blank_dirty` so
    /// the dirty utilities never clone the shared row.
    blank_row: Arc<Vec<Cell>>,
    /// Per-row: true when the slot points at `blank_row`.
    row_is_blank: Vec<bool>,
    /// Per-row: a blank slot that must be redrawn (set at scroll time,
    /// consumed once by `take_dirty_cells`).
    row_blank_dirty: Vec<bool>,
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
    /// Kitty keyboard protocol: current progressive-enhancement flags.
    /// Bit 0b1 disambiguate, 0b10 event types, 0b100 alternate keys,
    /// 0b1000 all-keys-as-escape-codes, 0b10000 associated text.
    pub kitty_flags: u8,
    /// Kitty keyboard protocol push/pop stack for the active screen.
    kitty_stack: Vec<u8>,
    /// Saved kitty state (flags + stack) for the primary screen while the
    /// alternate screen is active. The spec requires independent stacks.
    kitty_flags_primary: u8,
    kitty_stack_primary: Vec<u8>,
    /// Saved kitty state for the alternate screen while the primary is up.
    kitty_flags_alt: u8,
    kitty_stack_alt: Vec<u8>,
    /// xterm modifyOtherKeys mode (0 disabled, 1/2 enabled).
    pub modify_other_keys: u8,
    /// DECNKM (?66): application keypad mode (DECPAM `ESC =` on, DECPNM
    /// `ESC >` off). Read by the key encoder to choose SS3 vs CSI digits.
    pub keypad_app: bool,
    /// DECBKM (?67): backarrow key sends DEL instead of BS.
    pub backarrow_del: bool,
    /// Set by DECCOLM (?3) / DECSCPP (`CSI | ~`) when the terminal requests
    /// a column-count change; the app drains this and resizes the window.
    pub window_resize_request: Option<WinSize>,

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

    // --- In-band resize notification (terminal-wg mode 2048) ---
    /// When set, the app reports window resizes to the application as
    /// `CSI 4 ; rows ; cols t` (XTWINOPS text-area size) via [`Grid::resize_report`].
    pub in_band_resize: bool,

    // --- Shell integration (OSC 133) markers ---
    /// Per-row marker for the visible grid: 0 = none, 1 = prompt start,
    /// 2 = command start, 3 = command output. Scrolls with the rows and is
    /// copied to `shell_scrollback_markers` when lines enter the scrollback.
    shell_markers: Vec<u8>,
    /// Markers for scrollback lines, parallel to `scrollback`.
    shell_scrollback_markers: std::collections::VecDeque<u8>,
    /// Current working directory reported via OSC 7 (raw `file://…` URI).
    pub cwd: Option<String>,
    /// Notification text requested via OSC 9 / OSC 9;4 (drained by the app).
    pub notification: Option<String>,

    // --- Sixel images (DCS `q`) ---
    /// Decoded sixel images awaiting renderer upload, in arrival order. The
    /// grid owns placement state: rows shift on scroll, placements drop on
    /// clear/resize/alt-screen switches. The renderer reconciles its GPU
    /// textures against these ids every frame instead of draining the list.
    pub sixel_images: Vec<crate::sixel::SixelPlacement>,
    /// Monotonic id source for [`Self::sixel_images`].
    next_sixel_id: u64,
    /// True while inside a sixel DCS sequence so Put accumulates raw data.
    dcs_sixel: bool,
    /// In-progress kitty graphics transmission (chunked `m=1`).
    kitty_gfx: Option<KittyGfxPending>,
    /// Images transmitted but not yet (or already) displayed, keyed by the
    /// client-assigned `i=<id>` (kitty graphics `a=t`).
    kitty_images: Vec<KittyImage>,
    /// Inline video frame (feature `video`), drawn by the renderer over the
    /// terminal content. `None` when no video is playing.
    pub video_frame: Option<crate::sixel::SixelImage>,
    /// Monotonic version, bumped whenever `video_frame` changes, so the
    /// renderer can re-upload the texture only when a new frame arrives.
    pub video_frame_version: u64,
    /// Cell size in pixels (set by the app after font load; used for the
    /// post-image cursor advance).
    pub cell_w: u32,
    pub cell_h: u32,
}

/// A stored kitty graphics image awaiting display (`a=t`, then `a=p`).
struct KittyImage {
    /// Client-assigned image id (`i=<id>`, 1..=u32::MAX).
    id: u32,
    image: crate::sixel::SixelImage,
}

/// In-progress kitty graphics (APC `G`) transmission. Chunked transfers
/// (`m=1`) accumulate base64 across multiple APC sequences until the final
/// `m=0` chunk arrives.
struct KittyGfxPending {
    /// Pixel format: 24 = RGB24, 32 = RGBA8 (the default), 100 = PNG.
    format: u32,
    width: u32,
    height: u32,
    /// Client-assigned image id (`i=<id>`), 0 when none.
    image_id: u32,
    /// True for `a=t` (transmit + store + respond), false for `a=T`
    /// (transmit + display immediately).
    transmit_only: bool,
    /// Transmission medium: `t=d` direct (default), `t=f`/`t=t` read the
    /// image from a file whose path is the (base64) payload.
    transmission: u8,
    /// Accumulated base64 payload (decoded at finalize).
    data: Vec<u8>,
}

/// Parse a non-negative ASCII integer (kitty graphics control values).
fn parse_ascii_u32(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0u32, |acc, &b| {
        if b.is_ascii_digit() {
            acc * 10 + (b - b'0') as u32
        } else {
            acc
        }
    })
}

/// Read an image file for kitty graphics `t=f`/`t=t` transmission. The path
/// comes from untrusted terminal input, so we refuse pseudo-filesystems,
/// non-regular files, and oversized files. `metadata` follows symlinks (the
/// spec requires it); symlink loops surface as OS errors on open.
fn read_graphics_file(path: &str) -> Result<Vec<u8>, String> {
    const MAX_BYTES: u64 = 32 * 1024 * 1024; // 32 MiB
    let blocked = path == "/proc"
        || path == "/sys"
        || path == "/dev"
        || path.starts_with("/proc/")
        || path.starts_with("/sys/")
        || path.starts_with("/dev/");
    if blocked {
        return Err(format!("path not allowed: {path}"));
    }
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot stat: {e}"))?;
    if !meta.is_file() {
        return Err(format!("not a regular file: {path}"));
    }
    if meta.len() > MAX_BYTES {
        return Err(format!("file too large ({} bytes)", meta.len()));
    }
    std::fs::read(path).map_err(|e| format!("read failed: {e}"))
}

/// Decode a PNG payload into `(width, height, RGBA8)` pixels. The
/// transformations expand palette/grayscale to RGB, add an alpha channel, and
/// strip 16-bit depth so the output is always straight RGBA8.
fn decode_png_rgba(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(
        png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA,
    );
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let rgba = buf[..info.buffer_size()].to_vec();
    Some((info.width, info.height, rgba))
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

// ---------------------------------------------------------------------------
// GridSnapshot
// ---------------------------------------------------------------------------

/// Immutable, thread-safe view of a grid at a point in time.
///
/// Built by the per-tab engine thread after each parse batch and consumed by
/// the render/input threads without locking the grid. Rows are shared `Arc`
/// handles — the grid copy-on-writes rows via `Arc::make_mut`, so snapshot
/// construction copies only row *pointers* (O(rows)), never cell contents.
/// The renderer diffs consecutive snapshots by row-pointer identity to find
/// dirty rows.
#[derive(Clone)]
pub struct GridSnapshot {
    pub cols: usize,
    pub rows: usize,
    /// Active screen's row handles, row-major.
    pub cells: Vec<Arc<Vec<Cell>>>,
    /// Scrollback rows, oldest first.
    pub scrollback: VecDeque<Arc<Vec<Cell>>>,
    /// Per-row DEC line modes (0 normal, 3/4 double-height halves,
    /// 6 double-width).
    pub line_modes: Vec<u8>,
    /// Viewport scroll state (0 = live view, >0 = scrolled up).
    pub scrollback_offset: usize,
    pub scroll_fraction: f32,
    pub cursor: Cursor,
    pub cursor_visible: bool,
    pub cursor_shape: u8,
    /// DECSCNM (?5) screen-reverse video.
    pub screen_reverse: bool,
    pub synchronized_output: bool,
    pub palette: ColorPalette,
    /// Sixel/kitty image placements decoded by the engine.
    pub sixel_images: Vec<crate::sixel::SixelPlacement>,
    /// Input-state flags the app thread reads to encode keys/mouse events.
    pub mouse_mode: MouseMode,
    pub mouse_encoding: MouseEncoding,
    pub bracketed_paste: bool,
    pub kitty_flags: u8,
    pub modify_other_keys: u8,
    pub application_cursor_keys: bool,
    pub keypad_app: bool,
    pub backarrow_del: bool,
    pub focus_reporting: bool,
    /// Monotonic counter bumped on every snapshot; cheap change detection.
    pub generation: u64,
    /// Shared blank cell returned for out-of-range lookups.
    blank_cell: Cell,
}

impl GridSnapshot {
    /// `(offset, fraction)` view position — see [`Grid::smooth_view`].
    pub fn smooth_view(&self) -> (usize, f32) {
        let total = self.scrollback_offset as f32 + self.scroll_fraction;
        let offset = total.floor() as usize;
        let frac = total - offset as f32;
        (offset.min(self.scrollback.len()), frac)
    }

    pub fn is_scrolled(&self) -> bool {
        self.scrollback_offset > 0 || self.scroll_fraction > 0.0
    }

    pub fn line_mode(&self, row: usize) -> u8 {
        self.line_modes.get(row).copied().unwrap_or(0)
    }

    /// Cell at a *live grid* coordinate (row < `rows`).
    pub fn cell(&self, col: usize, row: usize) -> &Cell {
        self.cells
            .get(row)
            .and_then(|r| r.get(col))
            .unwrap_or(&self.blank_cell)
    }

    /// Scrollback length in lines.
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }
}

impl Grid {
    pub fn new(size: WinSize, scrollback: usize) -> Self {
        let cols = size.cols as usize;
        let rows = size.rows as usize;
        // The initial screen is entirely blank: every row starts as a handle
        // to one shared blank row (a refcount bump each, no per-row alloc).
        let blank = Arc::new(vec![Cell::default(); cols]);
        Grid {
            cols,
            rows,
            cells_primary: vec![blank.clone(); rows],
            cells_alt: vec![blank.clone(); rows],
            alt_active: false,
            cursor: Cursor::default(),
            saved_cursor: SavedCursor::default(),
            active_fg: Color::Default,
            active_bg: Color::Default,
            active_attrs: Attrs::default(),
            scroll_top: 0,
            scroll_bottom: rows - 1,
            scroll_left: 0,
            scroll_right: cols - 1,
            left_right_margins: false,
            line_modes: vec![0; rows],
            scrollback: std::collections::VecDeque::new(),
            blank_row: blank,
            row_is_blank: vec![true; rows],
            // A fresh grid needs a full first redraw.
            row_blank_dirty: vec![true; rows],
            scrollback_offset: 0,
            scroll_fraction: 0.0,
            scrollback_capacity: scrollback,
            next_sixel_id: 0,
            cursor_visible: true,
            origin_mode: false,
            autowrap: true, // DECAWM defaults ON (VT100 behaviour)
            screen_reverse: false,
            insert_mode: false,
            cursor_shape: 0, // 0 = terminal default (blinking block)
            application_cursor_keys: false,
            kitty_flags: 0,
            kitty_stack: Vec::new(),
            kitty_flags_primary: 0,
            kitty_stack_primary: Vec::new(),
            kitty_flags_alt: 0,
            kitty_stack_alt: Vec::new(),
            modify_other_keys: 0,
            keypad_app: false,
            backarrow_del: false,
            window_resize_request: None,
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
            in_band_resize: false,
            shell_markers: vec![0; rows],
            shell_scrollback_markers: std::collections::VecDeque::new(),
            cwd: None,
            notification: None,
            sixel_images: Vec::new(),
            dcs_sixel: false,
            kitty_gfx: None,
            kitty_images: Vec::new(),
            video_frame: None,
            video_frame_version: 0,
            cell_w: 8,
            cell_h: 16,
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

    fn cells(&self) -> &[Arc<Vec<Cell>>] {
        if self.alt_active {
            &self.cells_alt
        } else {
            &self.cells_primary
        }
    }

    fn cells_mut(&mut self) -> &mut Vec<Arc<Vec<Cell>>> {
        if self.alt_active {
            &mut self.cells_alt
        } else {
            &mut self.cells_primary
        }
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

    /// Report a terminal resize in-band (mode 2048). When the mode is set,
    /// queue `CSI 4 ; rows ; cols t` (XTWINOPS text-area size) so the
    /// application learns about resizes without polling `TIOCGWINSZ`.
    pub fn resize_report(&mut self) {
        if self.in_band_resize {
            let resp = format!("\x1b[4;{};{}t", self.rows, self.cols);
            self.respond(resp.as_bytes());
        }
    }

    /// Take the pending bell flag (BEL / OSC 9;7). The app consumes this to
    /// flash or beep; cleared after the call.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell_pending)
    }

    /// Decode and place a completed sixel DCS payload (`DCS … q … ST`). The
    /// image is positioned at the cursor and the cursor advances to column 0
    /// of the line below the image (DEC 54870). Decoded images are queued in
    /// [`Grid::sixel_images`] for the renderer to upload.
    fn place_sixel(&mut self) {
        let max_w = (self.cols as u32).saturating_mul(self.cell_w.max(1));
        let max_h = (self.rows as u32).saturating_mul(self.cell_h.max(1));
        let Some(image) = crate::sixel::decode_sixel(&self.dcs_buf, max_w.max(1), max_h.max(1))
        else {
            log::debug!("Sixel: payload did not decode");
            return;
        };
        let col = self.cursor.col;
        let row = self.cursor.row;
        let rows = (image.height + self.cell_h - 1) / self.cell_h.max(1);
        let id = self.next_sixel_id;
        self.next_sixel_id = self.next_sixel_id.wrapping_add(1);
        self.sixel_images.push(crate::sixel::SixelPlacement {
            id,
            col,
            row,
            image,
            image_id: 0,
        });
        // Keep the placement list within the renderer's GPU texture budget
        // (oldest evicted first — a refcount/free, no per-cell work).
        while self.sixel_images.len() > crate::sixel::MAX_LIVE_SIXELS {
            self.sixel_images.remove(0);
        }
        self.cursor.col = 0;
        self.cursor.row = (self.cursor.row + rows as usize).min(self.rows - 1);
        self.mark_all_dirty();
        log::debug!("Sixel: placed image #{id} at ({col},{row})");
    }

    /// Handle a completed APC string. Only the kitty graphics protocol
    /// (`ESC _ G ... ST`) is recognized; other APC payloads are ignored.
    fn handle_apc(&mut self, data: &[u8]) {
        if data.first() != Some(&b'G') {
            return;
        }
        // Split control params from the base64 payload at the first ';'.
        let (control, payload) = match data[1..].iter().position(|&b| b == b';') {
            Some(i) => (&data[1..1 + i], &data[2 + i..]),
            None => (&data[1..], &[][..]),
        };

        let mut action = 0u8;
        let mut format = 32u32;
        let mut width = 0u32;
        let mut height = 0u32;
        let mut image_id = 0u32;
        let mut delete = 0u8;
        let mut transmission = b'd';
        let mut more_chunks = false;
        for kv in control.split(|&b| b == b',') {
            let Some(eq) = kv.iter().position(|&b| b == b'=') else {
                continue;
            };
            let (k, v) = (&kv[..eq], &kv[eq + 1..]);
            match k {
                b"a" => action = v.first().copied().unwrap_or(0),
                b"f" => format = parse_ascii_u32(v),
                b"s" => width = parse_ascii_u32(v),
                b"v" => height = parse_ascii_u32(v),
                b"i" => image_id = parse_ascii_u32(v),
                b"d" => delete = v.first().copied().unwrap_or(0),
                b"t" => transmission = v.first().copied().unwrap_or(b'd'),
                b"m" => more_chunks = parse_ascii_u32(v) == 1,
                _ => {}
            }
        }

        match action {
            // Transmit: `a=T` displays immediately, `a=t` stores under the
            // client id and replies OK (the caching flow used by ranger,
            // image.nvim, etc.).
            b'T' | b't' => {
                self.kitty_gfx = Some(KittyGfxPending {
                    format,
                    width,
                    height,
                    image_id,
                    transmit_only: action == b't',
                    transmission,
                    data: payload.to_vec(),
                });
                if !more_chunks {
                    let done = self.kitty_gfx.take().expect("pending");
                    self.finalize_kitty(done);
                }
            }
            // Put: display a previously transmitted image at the cursor.
            b'p' => self.place_kitty_image(image_id),
            // Query (test-load, do not store). Direct transmission is always
            // accepted, so acknowledge unconditionally.
            b'q' => self.respond_kitty(image_id, "OK"),
            // Delete: `d=A` clears everything, otherwise delete `i=<id>`.
            b'd' => {
                if delete == b'A' {
                    self.sixel_images.clear();
                    self.kitty_images.clear();
                    self.mark_all_dirty();
                } else {
                    self.delete_kitty_image(image_id);
                }
            }
            // Continuation chunk (carries only `m=`) or unknown action.
            _ => {
                if self.kitty_gfx.is_some() {
                    let pending = self.kitty_gfx.as_mut().expect("checked");
                    pending.data.extend_from_slice(payload);
                    if !more_chunks {
                        let done = self.kitty_gfx.take().expect("pending");
                        self.finalize_kitty(done);
                    }
                }
            }
        }
    }

    /// Queue a kitty graphics acknowledgement: `ESC _ G i=<id>;OK ST` on
    /// success, `...;ENOENT:<msg>` on failure.
    fn respond_kitty(&mut self, image_id: u32, status: &str) {
        let reply = format!("\x1b_Gi={image_id};{status}\x1b\\");
        self.respond(reply.as_bytes());
    }

    /// Display a previously transmitted kitty image (`a=p,i=<id>`) at the
    /// cursor, cloning the stored pixels into a fresh placement.
    fn place_kitty_image(&mut self, image_id: u32) {
        let Some(stored) = self.kitty_images.iter().find(|k| k.id == image_id) else {
            log::debug!("Kitty graphics: put for unknown id {image_id}");
            self.respond_kitty(image_id, "ENOENT:no such image");
            return;
        };
        let image = stored.image.clone();
        let col = self.cursor.col;
        let row = self.cursor.row;
        let id = self.next_sixel_id;
        self.next_sixel_id = self.next_sixel_id.wrapping_add(1);
        self.sixel_images.push(crate::sixel::SixelPlacement {
            id,
            col,
            row,
            image,
            image_id,
        });
        while self.sixel_images.len() > crate::sixel::MAX_LIVE_SIXELS {
            self.sixel_images.remove(0);
        }
        self.mark_all_dirty();
        self.respond_kitty(image_id, "OK");
        log::debug!("Kitty graphics: placed stored image #{id} at ({col},{row})");
    }

    /// Delete a stored kitty image and any placements that came from it
    /// (`a=d,i=<id>`).
    fn delete_kitty_image(&mut self, image_id: u32) {
        self.kitty_images.retain(|k| k.id != image_id);
        self.sixel_images.retain(|p| p.image_id != image_id);
        self.mark_all_dirty();
    }

    /// Decode a completed kitty graphics transmission, then either store it
    /// under its client id (`a=t`) or place it at the cursor (`a=T`). Both
    /// reuse the sixel placement/render path.
    fn finalize_kitty(&mut self, pending: KittyGfxPending) {
        use base64::Engine as _;
        let Ok(payload) = base64::engine::general_purpose::STANDARD.decode(&pending.data) else {
            log::debug!("Kitty graphics: base64 decode failed");
            if pending.transmit_only {
                self.respond_kitty(pending.image_id, "EIO:base64 decode failed");
            }
            return;
        };
        // Direct (`t=d`) data is the image itself; `t=f`/`t=t` carry a
        // base64-encoded file path to read the image from (SSH-friendly).
        let raw = match pending.transmission {
            b'f' | b't' => {
                let path = String::from_utf8_lossy(&payload).into_owned();
                match read_graphics_file(&path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        log::debug!("Kitty graphics: {e}");
                        if pending.transmit_only {
                            self.respond_kitty(pending.image_id, &format!("EIO:{e}"));
                        }
                        return;
                    }
                }
            }
            _ => payload,
        };
        let (width, height, rgba) = match pending.format {
            24 => {
                // RGB24 (3 bytes/px) → RGBA8, opaque alpha.
                let mut out =
                    Vec::with_capacity(pending.width as usize * pending.height as usize * 4);
                for chunk in raw.chunks_exact(3) {
                    out.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
                }
                (pending.width, pending.height, out)
            }
            32 => (pending.width, pending.height, raw), // RGBA8 — the default.
            100 => match decode_png_rgba(&raw) {
                Some(decoded) => decoded,
                None => {
                    log::debug!("Kitty graphics: PNG (f=100) decode failed");
                    if pending.transmit_only {
                        self.respond_kitty(pending.image_id, "EIO:png decode failed");
                    }
                    return;
                }
            },
            other => {
                log::debug!("Kitty graphics: unsupported format {other}");
                if pending.transmit_only {
                    self.respond_kitty(pending.image_id, "EIO:unsupported format");
                }
                return;
            }
        };
        let expected = width as usize * height as usize * 4;
        if rgba.len() != expected {
            log::debug!(
                "Kitty graphics: size mismatch ({} vs {expected})",
                rgba.len()
            );
            if pending.transmit_only {
                self.respond_kitty(pending.image_id, "EIO:size mismatch");
            }
            return;
        }

        let image = crate::sixel::SixelImage {
            width,
            height,
            rgba,
        };

        if pending.transmit_only {
            // Store under the client id (replacing any prior image with the
            // same id) and acknowledge; no display.
            if let Some(existing) = self
                .kitty_images
                .iter_mut()
                .find(|k| k.id == pending.image_id)
            {
                existing.image = image;
            } else {
                self.kitty_images.push(KittyImage {
                    id: pending.image_id,
                    image,
                });
            }
            self.respond_kitty(pending.image_id, "OK");
            return;
        }

        // `a=T`: place at the cursor.
        let col = self.cursor.col;
        let row = self.cursor.row;
        let id = self.next_sixel_id;
        self.next_sixel_id = self.next_sixel_id.wrapping_add(1);
        self.sixel_images.push(crate::sixel::SixelPlacement {
            id,
            col,
            row,
            image,
            image_id: pending.image_id,
        });
        while self.sixel_images.len() > crate::sixel::MAX_LIVE_SIXELS {
            self.sixel_images.remove(0);
        }
        self.mark_all_dirty();
        log::debug!("Kitty graphics: placed image #{id} at ({col},{row})");
    }

    /// Take the pending notification text (OSC 9 / OSC 9;4), leaving None.
    pub fn take_notification(&mut self) -> Option<String> {
        std::mem::take(&mut self.notification)
    }

    /// Record the terminal's cell size in pixels. Used to compute the
    /// post-sixel cursor advance (`CSI P q` images occupy whole cells).
    pub fn set_cell_size(&mut self, w: u32, h: u32) {
        self.cell_w = w.max(1);
        self.cell_h = h.max(1);
    }

    /// Set (or clear) the inline video frame. `None` stops playback. Bumping
    /// `video_frame_version` lets the renderer re-upload only on change.
    pub fn set_video_frame(&mut self, frame: Option<crate::sixel::SixelImage>) {
        self.video_frame = frame;
        self.video_frame_version = self.video_frame_version.wrapping_add(1);
        self.mark_all_dirty();
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
            66 => self.keypad_app,             // DECNKM
            67 => self.backarrow_del,          // DECBKM
            69 => self.left_right_margins,     // DECLRMM
            1000 => self.mouse_mode == MouseMode::Normal,
            1002 => self.mouse_mode == MouseMode::ButtonEvent,
            1003 => self.mouse_mode == MouseMode::AnyEvent,
            1006 => self.mouse_encoding == MouseEncoding::SGR,
            1049 => self.alt_active, // alt screen
            2004 => self.bracketed_paste,
            1004 => self.focus_reporting,
            2026 => self.synchronized_output,
            2048 => self.in_band_resize,
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
        &self.cells()[row][col]
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

    /// True when the view is scrolled up at all (integer or fractional).
    pub fn is_scrolled(&self) -> bool {
        self.scrollback_offset > 0 || self.scroll_fraction > 0.0
    }

    /// Smooth-scroll view mapping: `(view_offset, remaining_fraction)` where
    /// `view_offset` is the number of scrollback lines to skip at the top
    /// (ceil of the fractional position) and `remaining_fraction` is how much
    /// of the top line is clipped (0.0..1.0, 0 when on an integer boundary).
    pub fn smooth_view(&self) -> (usize, f32) {
        let total = self.scrollback_offset as f32 + self.scroll_fraction;
        let ceil = total.ceil() as usize;
        let view_offset = ceil.min(self.scrollback.len());
        (view_offset, (total.ceil() - total).clamp(0.0, 1.0))
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

    /// Build an immutable [`GridSnapshot`] of the current state. Row handles
    /// are shared Arcs (copy-on-write), so this copies O(rows + scrollback)
    /// pointers — never cell bytes — and is safe to call on every batch.
    pub fn snapshot(&self) -> GridSnapshot {
        GridSnapshot {
            cols: self.cols,
            rows: self.rows,
            cells: self.cells().to_vec(),
            scrollback: self.scrollback.clone(),
            line_modes: self.line_modes.clone(),
            scrollback_offset: self.scrollback_offset,
            scroll_fraction: self.scroll_fraction,
            cursor: self.cursor,
            cursor_visible: self.cursor_visible,
            cursor_shape: self.cursor_shape,
            screen_reverse: self.screen_reverse,
            synchronized_output: self.synchronized_output,
            palette: self.palette.clone(),
            sixel_images: self.sixel_images.clone(),
            mouse_mode: self.mouse_mode,
            mouse_encoding: self.mouse_encoding,
            bracketed_paste: self.bracketed_paste,
            kitty_flags: self.kitty_flags,
            modify_other_keys: self.modify_other_keys,
            application_cursor_keys: self.application_cursor_keys,
            keypad_app: self.keypad_app,
            backarrow_del: self.backarrow_del,
            focus_reporting: self.focus_reporting,
            generation: 0,
            blank_cell: Cell::default(),
        }
    }

    fn cell_mut(&mut self, col: usize, row: usize) -> &mut Cell {
        // Writing a cell makes the row a private, non-blank row: if the slot
        // still points at the shared blank this copies it once (the copy is
        // the price of the first write; the fast path in `print_ascii_run`
        // rebuilds full-row writes instead).
        self.row_is_blank[row] = false;
        let row_cells = Arc::make_mut(&mut self.cells_mut()[row]);
        &mut row_cells[col]
    }

    // -----------------------------------------------------------------------
    // Shell integration (OSC 133) — prompt markers
    // -----------------------------------------------------------------------

    /// Marker stored for a row. 0 = none, 1 = prompt start (OSC 133;A/E),
    /// 2 = command start (OSC 133;B), 3 = command output (OSC 133;C/D).
    fn set_row_marker(&mut self, row: usize, marker: u8) {
        if row < self.rows {
            self.shell_markers[row] = marker;
        }
    }

    /// The combined marker stream: scrollback markers (oldest first) followed
    /// by visible-grid markers. An index into this stream is a stable address
    /// for a row even after it scrolls off the visible grid.
    pub fn marker_stream(&self) -> impl Iterator<Item = u8> + '_ {
        self.shell_scrollback_markers
            .iter()
            .copied()
            .chain(self.shell_markers.iter().copied())
    }

    /// Combined-stream length (scrollback + visible rows).
    pub fn marker_stream_len(&self) -> usize {
        self.scrollback.len() + self.rows
    }

    /// Index of the previous prompt marker strictly before `from` (a
    /// combined-stream index), or None.
    pub fn prev_prompt(&self, from: usize) -> Option<usize> {
        if from == 0 {
            return None; // nothing strictly before the stream start
        }
        let collected: Vec<u8> = self.marker_stream().collect();
        let mut idx = from - 1;
        loop {
            if let Some(marker) = collected.get(idx) {
                if *marker == 1 {
                    return Some(idx);
                }
            } else {
                return None;
            }
            if idx == 0 {
                return None;
            }
            idx -= 1;
        }
    }

    /// Index of the next prompt marker strictly after `from` (a combined
    /// stream index), or None.
    pub fn next_prompt(&self, from: usize) -> Option<usize> {
        let collected: Vec<u8> = self.marker_stream().collect();
        (from + 1..collected.len()).find(|&i| collected[i] == 1)
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
        let mut source_lines: Vec<Vec<Cell>> =
            self.scrollback.iter().map(|l| l.as_ref().clone()).collect();
        source_lines.extend(self.cells_primary.iter().map(|l| l.as_ref().clone()));
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
        // The shared blank must match the new width before padding rows are
        // built from it (otherwise the rebuilt grid mixes row widths).
        self.blank_row = Arc::new(vec![Cell::default(); new_cols]);

        let visible_lines = &reflowed[visible_start..reflowed.len().min(visible_start + new_rows)];
        let mut new_primary: Vec<Arc<Vec<Cell>>> =
            visible_lines.iter().map(|l| Arc::new(l.clone())).collect();
        while new_primary.len() < new_rows {
            new_primary.push(self.blank_row.clone());
        }

        let scrollback_start = visible_start.saturating_sub(self.scrollback_capacity);
        self.scrollback = reflowed[scrollback_start..visible_start]
            .iter()
            .map(|l| Arc::new(l.clone()))
            .collect();

        // Resize the alternate screen in place, preserving its visible top-left
        // content. Alternate-screen applications generally redraw immediately,
        // but losing it during a transient resize causes visible corruption.
        let mut new_alt: Vec<Arc<Vec<Cell>>> =
            (0..new_rows).map(|_| self.blank_row.clone()).collect();
        for row in 0..old_rows.min(new_rows) {
            let copy_cols = old_cols.min(new_cols);
            let src = &self.cells_alt[row][..copy_cols];
            Arc::make_mut(&mut new_alt[row])[..copy_cols].clone_from_slice(src);
        }

        self.cols = new_cols;
        self.rows = new_rows;
        self.cells_primary = new_primary;
        self.cells_alt = new_alt;
        // Recompute blank flags from the rebuilt screens (blank padding rows
        // point at the shared blank).
        self.row_is_blank = self
            .cells()
            .iter()
            .map(|r| Arc::ptr_eq(r, &self.blank_row))
            .collect();
        self.row_blank_dirty = vec![false; new_rows];
        self.line_modes.resize(new_rows, 0);
        self.line_modes.truncate(new_rows);
        // Reflow re-orders rows, so shell markers cannot stay aligned; reset
        // them. In-flight sixel images are viewport-relative and are dropped.
        self.shell_markers = vec![0; new_rows];
        self.shell_scrollback_markers.clear();
        self.sixel_images.clear();
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

    /// Switch the terminal between 80/132-column modes (DECCOLM ?3 / DECSCPP
    /// `CSI Ps | ~`). Per VT100 spec: reflow to the new width, clear the
    /// screen, home the cursor, and reset the margins. Also surfaces a
    /// `window_resize_request` for the app to mirror in the real window.
    pub fn set_columns(&mut self, new_cols: usize) {
        if new_cols == self.cols {
            // Still a full reset of the screen state per spec.
            self.active_fg = Color::Default;
            self.active_bg = Color::Default;
            self.active_attrs = Attrs::default();
            self.erase_in_display(2);
            self.scroll_top = 0;
            self.scroll_bottom = self.rows - 1;
            self.cursor = Cursor::default();
            return;
        }
        let new_cols = new_cols.clamp(2, 512);
        self.resize(WinSize {
            cols: new_cols as u16,
            rows: self.rows as u16,
        });
        // Spec side effects: clear, home cursor, reset margins.
        self.active_fg = Color::Default;
        self.active_bg = Color::Default;
        self.active_attrs = Attrs::default();
        self.erase_in_display(2);
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.scroll_left = 0;
        self.scroll_right = self.cols - 1;
        self.left_right_margins = false;
        self.cursor = Cursor::default();
        self.window_resize_request = Some(WinSize {
            cols: new_cols as u16,
            rows: self.rows as u16,
        });
    }

    /// DECSTR — soft terminal reset (`CSI ! p`). Returns the terminal to its
    /// power-up state: modes off, SGR default, screen cleared, cursor home,
    /// margins reset, tab stops re-initialized. Unlike RIS it keeps the
    /// scrollback (xterm behaviour).
    pub fn soft_reset(&mut self) {
        self.active_fg = Color::Default;
        self.active_bg = Color::Default;
        self.active_attrs = Attrs::default();
        self.autowrap = true;
        self.origin_mode = false;
        self.screen_reverse = false;
        self.insert_mode = false;
        self.cursor_visible = true;
        self.cursor_shape = 0;
        self.application_cursor_keys = false;
        self.keypad_app = false;
        self.backarrow_del = false;
        self.bracketed_paste = false;
        self.focus_reporting = false;
        self.synchronized_output = false;
        self.in_band_resize = false;
        self.mouse_mode = MouseMode::None;
        self.mouse_encoding = MouseEncoding::X10;
        self.tab_stops = default_tab_stops(self.cols);
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.scroll_left = 0;
        self.scroll_right = self.cols - 1;
        self.left_right_margins = false;
        self.erase_in_display(2);
        self.cursor = Cursor::default();
        self.mark_all_dirty();
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

    /// Scroll up by a fractional line amount (for smooth scrolling).
    pub fn smooth_scroll_up(&mut self, amount: f32) {
        let total = (self.scrollback_offset as f32 + self.scroll_fraction + amount)
            .min(self.scrollback.len() as f32);
        self.scrollback_offset = total.floor() as usize;
        self.scroll_fraction = total - total.floor();
        if self.scrollback_offset >= self.scrollback.len() {
            self.scroll_fraction = 0.0;
        }
    }

    /// Scroll down by a fractional line amount (for smooth scrolling).
    pub fn smooth_scroll_down(&mut self, amount: f32) {
        let total = (self.scrollback_offset as f32 + self.scroll_fraction - amount).max(0.0);
        self.scrollback_offset = total.floor() as usize;
        self.scroll_fraction = total - total.floor();
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

    /// Check if any cell is dirty (blank rows report via their row flag).
    pub fn has_dirty_cells(&self) -> bool {
        for row in 0..self.rows {
            if self.row_is_blank[row] {
                if self.row_blank_dirty[row] {
                    return true;
                }
                continue;
            }
            for col in 0..self.cols {
                if self.cell(col, row).dirty {
                    return true;
                }
            }
        }
        false
    }

    /// Get positions of all dirty cells and clear their dirty flags
    /// Returns a vector of (row, col) pairs. Blank rows are reported via
    /// `row_blank_dirty` without touching the shared blank row's cells.
    pub fn take_dirty_cells(&mut self) -> Vec<(usize, usize)> {
        let mut dirty = Vec::new();
        for row in 0..self.rows {
            if self.row_is_blank[row] {
                if self.row_blank_dirty[row] {
                    self.row_blank_dirty[row] = false;
                    for col in 0..self.cols {
                        dirty.push((row, col));
                    }
                }
                continue;
            }
            let cols = self.cols;
            let row_cells = Arc::make_mut(&mut self.cells_mut()[row]);
            for col in 0..cols {
                if row_cells[col].dirty {
                    dirty.push((row, col));
                    row_cells[col].dirty = false;
                }
            }
        }
        dirty
    }

    /// Mark all cells as dirty (full redraw). Blank rows get their row flag
    /// set instead of being cloned out of the shared blank.
    pub fn mark_all_dirty(&mut self) {
        for row in 0..self.rows {
            if self.row_is_blank[row] {
                self.row_blank_dirty[row] = true;
                continue;
            }
            for cell in Arc::make_mut(&mut self.cells_mut()[row]).iter_mut() {
                cell.dirty = true;
            }
        }
    }

    /// Mark a specific cell as dirty. For a blank row this marks the whole
    /// row (coarse but correct, and it never clones the shared blank).
    pub fn mark_dirty(&mut self, col: usize, row: usize) {
        if col < self.cols && row < self.rows {
            if self.row_is_blank[row] {
                self.row_blank_dirty[row] = true;
                return;
            }
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
                // Box<str> is immutable, so rebuild the tail when appending.
                // Combining clusters are short and rare; the alloc is fine.
                let tail = match cell.combining.take() {
                    Some(existing) => {
                        let mut s = existing.to_string();
                        s.push(ch);
                        s.into_boxed_str()
                    }
                    None => ch.to_string().into_boxed_str(),
                };
                cell.combining = Some(tail);
                cell.dirty = true;
            }
            return;
        }

        // Wrap if at end of line. DECAWM (?7) controls this: with autowrap
        // off, text clamps to the last cell(s) and overwrites them (T3-3).
        // DECSLRM (?69) bounds the line to the left/right margins: wrap goes
        // to the left margin, clamping to the right margin (VT420).
        let left = self.left_margin();
        let right = self.right_margin();
        if self.cursor.col + width > right + 1 {
            if self.autowrap {
                self.cursor.col = left;
                self.cursor.row += 1;
            } else {
                self.cursor.col = (right + 1).saturating_sub(width).max(left);
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
            self.row_is_blank[row] = false;
            let row_cells = Arc::make_mut(&mut self.cells_mut()[row]);
            for c in (col + width..cols).rev() {
                row_cells[c] = row_cells[c - width].clone();
                row_cells[c].dirty = true;
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

        // Bail when left/right margins are active — the wrap and write
        // bounds differ from the full line; the per-byte path handles them.
        if self.scroll_left != 0 || self.scroll_right != self.cols - 1 {
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
                    // LF — line feed. Batch consecutive LFs into a single
                    // scroll: runs of blank lines (`\n\n\n`) are common in
                    // real output and each one would otherwise pay the full
                    // scroll cost (rotate + blank + marker shift).
                    let mut n = 1;
                    while idx + n < bytes.len() && bytes[idx + n] == 0x0a {
                        n += 1;
                    }
                    let new_row = self.cursor.row + n;
                    if new_row > scroll_bottom {
                        self.scroll_up(new_row - scroll_bottom);
                        self.cursor.row = scroll_bottom;
                    } else {
                        self.cursor.row = new_row;
                    }
                    self.last_char = None;
                    consumed += n;
                    idx += n;
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

                // Write the chunk directly into the row's cells array.
                let col = self.cursor.col;
                let row = self.cursor.row;
                {
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
                    if col == 0 && take == cols {
                        // Full-row write: rebuild the row in one pass instead
                        // of Arc::make_mut cloning the (possibly shared blank)
                        // row first — halves the per-line cell traffic under
                        // `cat bigfile`.
                        self.row_is_blank[row] = false;
                        let mut new_row = Vec::with_capacity(cols);
                        for i in 0..take {
                            let b = run[run_idx + i];
                            let mut cell = template.clone();
                            cell.ch = b as char;
                            new_row.push(cell);
                        }
                        self.cells_mut()[row] = Arc::new(new_row);
                    } else {
                        self.row_is_blank[row] = false;
                        let row_cells = Arc::make_mut(&mut self.cells_mut()[row]);
                        // Clone the template and override the char. LLVM turns
                        // the clone into a tight vectorized copy; hand-written
                        // field stores benchmark slower.
                        for i in 0..take {
                            let b = run[run_idx + i];
                            let mut cell = template.clone();
                            cell.ch = b as char;
                            row_cells[col + i] = cell;
                        }
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

    /// Effective right margin: `scroll_right` while DECLRMM is active,
    /// otherwise the last column.
    #[inline]
    pub fn right_margin(&self) -> usize {
        if self.left_right_margins {
            self.scroll_right.min(self.cols - 1)
        } else {
            self.cols - 1
        }
    }

    /// Effective left margin: `scroll_left` while DECLRMM is active,
    /// otherwise column 0.
    #[inline]
    pub fn left_margin(&self) -> usize {
        if self.left_right_margins {
            self.scroll_left.min(self.cols - 1)
        } else {
            0
        }
    }

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
        let mark_dirty = !self.bulk_output;

        // Push top line(s) into scrollback (primary screen, full-region only).
        // Rows are `Arc<Vec<Cell>>`, so the scrollback gets a shared handle —
        // a refcount bump, no per-cell copying — and the grid slot is replaced
        // by the rotate + blank below (hot path under heavy output).
        if !self.alt_active && top == self.scroll_top {
            for r in top..top + n {
                if r < self.rows {
                    self.scrollback.push_back(self.cells_primary[r].clone());
                    // Shell-integration markers travel with their rows.
                    let marker = self.shell_markers.get(r).copied().unwrap_or(0);
                    self.shell_scrollback_markers.push_back(marker);
                }
            }
            // Enforce scrollback limit (O(1) eviction with VecDeque, T1-7).
            // The evicted Arc is dropped — no allocation to recycle.
            while self.scrollback.len() > self.scrollback_capacity {
                self.scrollback.pop_front();
                self.shell_scrollback_markers.pop_front();
            }
        }

        // Rotate row handles up — O(rows) pointer moves, no per-cell copies.
        {
            let cells = self.cells_mut();
            if n < bot - top + 1 {
                cells[top..=bot].rotate_left(n);
            }
        }
        // Blank flags shift with the rows, and the vacated bottom n rows
        // become blank slots pointing at the shared blank (a refcount bump —
        // no allocation, no memset on the scroll path).
        if top <= bot {
            if n < bot - top + 1 {
                for r in top..=bot - n {
                    self.shell_markers[r] = self.shell_markers[r + n];
                    self.row_is_blank[r] = self.row_is_blank[r + n];
                    self.row_blank_dirty[r] = self.row_blank_dirty[r + n];
                }
            }
            for r in (bot + 1 - n).max(top)..=bot {
                self.cells_mut()[r] = self.blank_row.clone();
                self.shell_markers[r] = 0;
                self.row_is_blank[r] = true;
                self.row_blank_dirty[r] = true;
            }
        }

        // Mark scrolled region dirty in bulk (skip in bulk_output mode).
        // Blank slots are flagged per-row so the shared blank is never
        // cloned just to set a dirty bit.
        if mark_dirty {
            for r in top..=bot {
                if self.row_is_blank[r] {
                    self.row_blank_dirty[r] = true;
                    continue;
                }
                for cell in Arc::make_mut(&mut self.cells_mut()[r]).iter_mut() {
                    cell.dirty = true;
                }
            }
        }
        // Sixel images travel with their rows: shift placements up by `n`,
        // dropping any whose top row scrolled off the region top.
        self.shift_sixel_rows(top, bot, n, true);
    }

    /// Shift sixel placements inside the scroll band `[top, bot]` along with
    /// the rows: up by `n` (LF scroll / DL) or down by `n` (IL). Placements
    /// whose top row leaves the band are dropped — they scrolled into history
    /// (or were discarded by DL/IL semantics, which never save). Placements
    /// outside the band are untouched. For region scrolls with `top > 0` the
    /// band's top rows land *above* the region (still on screen), so `up`
    /// only drops rows that pass the screen top.
    fn shift_sixel_rows(&mut self, top: usize, bot: usize, n: usize, up: bool) {
        if self.sixel_images.is_empty() {
            return;
        }
        self.sixel_images.retain_mut(|p| {
            if p.row < top || p.row > bot {
                return true; // outside the scrolled band
            }
            if up {
                if p.row < n {
                    return false; // scrolled off the region/screen top
                }
                p.row -= n;
            } else {
                if p.row + n > bot {
                    return false; // pushed past the region bottom (IL discard)
                }
                p.row += n;
            }
            true
        });
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

        let mark_dirty = !self.bulk_output;
        // Rotate row handles down — O(rows) pointer moves, no per-cell copies.
        {
            let cells = self.cells_mut();
            if n < bot - top + 1 {
                cells[top..=bot].rotate_right(n);
            }
        }
        // Blank flags shift with the rows, and the vacated top n rows become
        // blank slots pointing at the shared blank (refcount bump, no alloc).
        if top <= bot {
            if n < bot - top + 1 {
                for r in (top..=bot - n).rev() {
                    self.shell_markers[r + n] = self.shell_markers[r];
                    self.row_is_blank[r + n] = self.row_is_blank[r];
                    self.row_blank_dirty[r + n] = self.row_blank_dirty[r];
                }
            }
            for r in top..(top + n).min(bot + 1) {
                self.cells_mut()[r] = self.blank_row.clone();
                self.shell_markers[r] = 0;
                self.row_is_blank[r] = true;
                self.row_blank_dirty[r] = true;
            }
        }

        // Mark scrolled region dirty in bulk (skip in bulk_output mode).
        // Blank slots are flagged per-row so the shared blank is never
        // cloned just to set a dirty bit.
        if mark_dirty {
            for r in top..=bot {
                if self.row_is_blank[r] {
                    self.row_blank_dirty[r] = true;
                    continue;
                }
                for cell in Arc::make_mut(&mut self.cells_mut()[r]).iter_mut() {
                    cell.dirty = true;
                }
            }
        }
        // Sixel images travel with their rows: shift placements down by `n`,
        // dropping any pushed past the region bottom (IL semantics).
        self.shift_sixel_rows(top, bot, n, false);
    }

    // -----------------------------------------------------------------------
    // Erase operations
    // -----------------------------------------------------------------------

    fn erase_in_display(&mut self, mode: u16) {
        self.erase_display_impl(mode, false);
    }

    /// DECSED — selective erase in display: skips cells whose DECSCA
    /// protection is set.
    fn erase_in_display_selective(&mut self, mode: u16) {
        self.erase_display_impl(mode, true);
    }

    /// Core ED implementation. Horizontally bounded by the DECSLRM margins
    /// when DECLRMM is active; `selective` skips protected cells.
    fn erase_display_impl(&mut self, mode: u16, selective: bool) {
        let cursor = self.cursor;
        let rows = self.rows;
        let left = self.left_margin();
        let right = self.right_margin();
        let (start_row, start_col, end_row, end_col) = match mode {
            0 => {
                // From cursor to end of screen (bounded by the right margin).
                (cursor.row, cursor.col.min(right + 1), rows - 1, right + 1)
            }
            1 => {
                // From start to cursor (inclusive). Clamp: the cursor can sit
                // one past the right margin (pending wrap after filling a row).
                (0, left, cursor.row, (cursor.col + 1).clamp(left, right + 1))
            }
            2 | 3 => {
                // Entire screen. Mode 3 also wipes the scrollback history
                // (T3-13) so the user can't scroll up into cleared content.
                if mode == 3 {
                    self.scrollback.clear();
                    self.scrollback_offset = 0;
                }
                (0, left, rows - 1, right + 1)
            }
            _ => return,
        };

        let fg = self.active_fg;
        let bg = self.active_bg;
        for r in start_row..=end_row {
            self.row_is_blank[r] = false;
            let row_cells = Arc::make_mut(&mut self.cells_mut()[r]);
            let (cs, ce) = if r == start_row && r == end_row {
                (start_col, end_col)
            } else if r == start_row {
                (start_col, right + 1)
            } else if r == end_row {
                (left, end_col)
            } else {
                (left, right + 1)
            };
            for c in cs..ce {
                if selective && row_cells[c].attrs.protected() {
                    continue;
                }
                row_cells[c] = Cell {
                    fg,
                    bg,
                    dirty: true,
                    ..Cell::default()
                };
            }
        }
        // Sixel placements whose top-left cell is inside the erased rectangle
        // are removed (mode 2/3 covers the whole screen).
        if mode == 2 || mode == 3 {
            self.sixel_images.clear();
        } else {
            let (er, ec) = (start_row, start_col);
            let (fr, fc) = (end_row, end_col.saturating_sub(1)); // end_col exclusive
            self.sixel_images
                .retain(|p| !(p.row >= er && p.row <= fr && p.col >= ec && p.col <= fc));
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        self.erase_line_impl(mode, false);
    }

    /// DECSEL — selective erase in line: skips protected cells.
    fn erase_in_line_selective(&mut self, mode: u16) {
        self.erase_line_impl(mode, true);
    }

    /// Core EL implementation. Bounded by the DECSLRM margins when DECLRMM
    /// is active; `selective` skips protected cells.
    fn erase_line_impl(&mut self, mode: u16, selective: bool) {
        let cursor = self.cursor;
        let left = self.left_margin();
        let right = self.right_margin();
        let (start_col, end_col) = match mode {
            0 => (cursor.col.min(right + 1), right + 1), // cursor to right margin
            // Mode 1 erases left margin→cursor inclusive; the cursor can sit
            // one past the right margin (pending wrap), so clamp (T3-16).
            1 => (left, (cursor.col + 1).clamp(left, right + 1)),
            2 => (left, right + 1), // entire line between the margins
            _ => return,
        };

        let fg = self.active_fg;
        let bg = self.active_bg;
        let row = cursor.row;
        self.row_is_blank[row] = false;
        let row_cells = Arc::make_mut(&mut self.cells_mut()[row]);
        for c in start_col..end_col {
            if selective && row_cells[c].attrs.protected() {
                continue;
            }
            row_cells[c] = Cell {
                fg,
                bg,
                dirty: true,
                ..Cell::default()
            };
        }
        // Drop sixel placements starting on the erased line inside the range
        // (a full-line EL2 clears any image anchored on that row).
        self.sixel_images
            .retain(|p| !(p.row == row && p.col >= start_col && p.col < end_col));
    }

    /// Fill a rectangular area (DECFRA / DECERA / DECSERA): rows
    /// `top..=bottom` and columns `left..=right`, all 1-based and shifted by
    /// the origin in DECOM mode, clamped to the screen. Cells are replaced
    /// with `fill` using the current SGR (DECERA/DECSERA pre-set SGR to
    /// defaults); `selective` (DECSERA) leaves protected cells intact.
    fn fill_rect(
        &mut self,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
        fill: u16,
        selective: bool,
    ) {
        let origin = if self.origin_mode {
            self.scroll_top.min(self.rows.saturating_sub(1))
        } else {
            0
        };
        let r0 = top.saturating_sub(1) + origin;
        let r1 = bottom.saturating_sub(1) + origin;
        let c0 = left.saturating_sub(1);
        let c1 = right.saturating_sub(1);
        let r0 = r0.min(self.rows - 1);
        let r1 = r1.min(self.rows - 1);
        let c0 = c0.min(self.cols - 1);
        let c1 = c1.min(self.cols - 1);
        if r0 > r1 || c0 > c1 {
            return;
        }
        let ch = char::from_u32(fill as u32).unwrap_or(' ');
        let fg = self.active_fg;
        let bg = self.active_bg;
        let attrs = self.active_attrs;
        for r in r0..=r1 {
            self.row_is_blank[r] = false;
            let row_cells = Arc::make_mut(&mut self.cells_mut()[r]);
            for c in c0..=c1 {
                if selective && row_cells[c].attrs.protected() {
                    continue;
                }
                row_cells[c] = Cell {
                    ch,
                    fg,
                    bg,
                    attrs,
                    dirty: true,
                    ..Cell::default()
                };
            }
        }
    }

    /// Insert `n` blank columns at the cursor (DECIC). Every row shifts its
    /// content from the cursor column right; the rightmost columns fall off.
    fn insert_columns(&mut self, n: usize) {
        let bound = self.right_margin() + 1;
        let col = self.cursor.col.min(bound);
        if col >= bound {
            return;
        }
        let n = n.min(bound - col);
        let fg = self.active_fg;
        let bg = self.active_bg;
        let attrs = self.active_attrs;
        for r in 0..self.rows {
            self.row_is_blank[r] = false;
            let row_cells = Arc::make_mut(&mut self.cells_mut()[r]);
            for c in (col..bound).rev() {
                if c + n < bound {
                    row_cells[c + n] = row_cells[c].clone();
                }
                row_cells[c].dirty = true;
            }
            for c in col..(col + n).min(bound) {
                row_cells[c] = Cell {
                    ch: ' ',
                    fg,
                    bg,
                    attrs,
                    dirty: true,
                    ..Cell::default()
                };
            }
        }
    }

    /// Delete `n` columns at the cursor (DECDC). Every row shifts its content
    /// left from the cursor column; the rightmost columns become blank.
    fn delete_columns(&mut self, n: usize) {
        let bound = self.right_margin() + 1;
        let col = self.cursor.col.min(bound);
        if col >= bound {
            return;
        }
        let n = n.min(bound - col);
        let fg = self.active_fg;
        let bg = self.active_bg;
        let attrs = self.active_attrs;
        for r in 0..self.rows {
            self.row_is_blank[r] = false;
            let row_cells = Arc::make_mut(&mut self.cells_mut()[r]);
            for c in col..(bound - n) {
                row_cells[c] = row_cells[c + n].clone();
                row_cells[c].dirty = true;
            }
            for c in (bound - n)..bound {
                row_cells[c] = Cell {
                    ch: ' ',
                    fg,
                    bg,
                    attrs,
                    dirty: true,
                    ..Cell::default()
                };
            }
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
                    // DECSCA protection survives an SGR reset (VT420 keeps
                    // the two attributes independent).
                    let protected = self.active_attrs.protected();
                    self.active_attrs = Attrs::default();
                    self.active_attrs.set_protected(protected);
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
            // CUF — cursor forward (bounded by the right margin in DECLRMM).
            (_, b'C') => {
                let n = p0.max(1);
                self.cursor.col = (self.cursor.col + n).min(self.right_margin());
            }
            // CUB — cursor backward (bounded by the left margin in DECLRMM).
            (_, b'D') => {
                let n = p0.max(1);
                self.cursor.col = self.cursor.col.saturating_sub(n).max(self.left_margin());
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
            // CHA — cursor horizontal absolute. In DECOM the column is
            // relative to the left margin (VT420).
            (_, b'G') => {
                self.cursor.col = if self.origin_mode {
                    (p0.saturating_sub(1) + self.left_margin()).min(self.right_margin())
                } else {
                    p0.saturating_sub(1).min(self.cols - 1)
                };
            }
            // CUP / HVP — cursor position. In DECOM (?6) the position is
            // relative to the origin — the intersection of the top and left
            // margins — and clamped inside it (T3-2, VT420).
            (_, b'H') | (_, b'f') => {
                let row = if self.origin_mode {
                    (p0.saturating_sub(1) + self.scroll_top).min(self.scroll_bottom)
                } else {
                    p0.saturating_sub(1).min(self.rows - 1)
                };
                self.cursor.row = row;
                self.cursor.col = if self.origin_mode {
                    (p1.saturating_sub(1) + self.left_margin()).min(self.right_margin())
                } else {
                    p1.saturating_sub(1).min(self.cols - 1)
                };
            }
            // DECSED — selective erase in display: like ED but skips cells
            // whose DECSCA protection is set. Must precede the catch-all.
            (b"?", b'J') => self.erase_in_display_selective(p0 as u16),
            // DECSEL — selective erase in line.
            (b"?", b'K') => self.erase_in_line_selective(p0 as u16),
            // ED — erase in display.
            (_, b'J') => self.erase_in_display(p0 as u16),
            // EL — erase in line.
            (_, b'K') => self.erase_in_line(p0 as u16),
            // ICH — insert characters (T3-10). Shifts the row right, leaving
            // `n` blank cells at the cursor; remaining cells fall off the end
            // (bounded by the right margin in DECLRMM).
            (_, b'@') => {
                let n = p0.max(1);
                let row = self.cursor.row;
                let col = self.cursor.col;
                let right = self.right_margin();
                let bound = right + 1;
                self.row_is_blank[row] = false;
                let row_cells = Arc::make_mut(&mut self.cells_mut()[row]);
                // Shift everything at/after `col` right by `n`, working from
                // the right edge so we don't clobber source cells.
                for c in (col..bound).rev() {
                    if c + n < bound {
                        row_cells[c + n] = row_cells[c].clone();
                    }
                    if c >= col {
                        row_cells[c].dirty = true;
                    }
                }
                // Blank the freshly-inserted cells at the cursor.
                for c in col..(col + n).min(bound) {
                    row_cells[c] = Cell::default();
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
            // CHT — cursor forward tabulation. `CSI Ps I` advances to the
            // `Ps`-th next tab stop (default 1); past the last stop → the
            // last column (xterm behaviour, mirrors HT).
            (_, b'I') => {
                let n = p0.max(1);
                let mut col = self.cursor.col;
                for _ in 0..n {
                    let mut next = self.cols - 1;
                    let mut found = false;
                    for c in (col + 1)..self.cols {
                        if self.tab_stops.get(c).copied().unwrap_or(false) {
                            next = c;
                            found = true;
                            break;
                        }
                    }
                    col = next;
                    if !found {
                        break;
                    }
                }
                self.cursor.col = col;
            }
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
            // DCH — delete characters (bounded by the right margin).
            (_, b'P') => {
                let n = p0.max(1);
                let row = self.cursor.row;
                let col = self.cursor.col;
                let bound = self.right_margin() + 1;
                self.row_is_blank[row] = false;
                let row_cells = Arc::make_mut(&mut self.cells_mut()[row]);
                for c in col..bound {
                    if c + n < bound {
                        row_cells[c] = row_cells[c + n].clone();
                    } else {
                        row_cells[c] = Cell::default();
                    }
                    row_cells[c].dirty = true;
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
                self.row_is_blank[row] = false;
                let row_cells = Arc::make_mut(&mut self.cells_mut()[row]);
                for c in col..end {
                    row_cells[c] = Cell {
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
            // DECSTBM — set top/bottom margins (scroll region). Per VT420,
            // setting them resets the left/right margins to full width.
            (_, b'r') => {
                let top = p0.saturating_sub(1);
                let bot = if p1 == 0 { self.rows - 1 } else { p1 - 1 };
                if top < bot && bot < self.rows {
                    self.scroll_top = top;
                    self.scroll_bottom = bot;
                }
                self.scroll_left = 0;
                self.scroll_right = self.cols - 1;
                self.cursor = Cursor::default(); // home cursor
            }
            // DECSLRM — set left/right margins: CSI Pl;Pr s. With DECLRMM
            // (?69) off this resets them to full width (VT420). Setting them
            // resets the top/bottom margins and homes the cursor into the
            // margin region. The bare `CSI s` (no params) stays DECSC — save
            // cursor (matched below).
            (b"", b's') if p0 != 0 && params.len() >= 2 => {
                let left = p0.saturating_sub(1);
                let right = p1.saturating_sub(1);
                self.scroll_left = if self.left_right_margins {
                    left.min(self.cols - 1)
                } else {
                    0
                };
                self.scroll_right = if self.left_right_margins {
                    right.min(self.cols - 1).max(self.scroll_left)
                } else {
                    self.cols - 1
                };
                self.scroll_top = 0;
                self.scroll_bottom = self.rows - 1;
                self.cursor.row = self.scroll_top;
                self.cursor.col = self.scroll_left;
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
                            // DECCOLM — 80/132 column switch (T3-5): set → 80
                            // columns, reset → 132 (VT100 convention). Resizes
                            // the grid and requests the window to follow.
                            self.set_columns(if set { 80 } else { 132 });
                        }
                        5 => {
                            // DECSCNM — screen reverse video (T3-4).
                            self.screen_reverse = set;
                            self.mark_all_dirty();
                        }
                        6 => {
                            // DECOM — origin mode (T3-2). Setting or resetting
                            // homes the cursor — to the origin (the top-left
                            // of the scroll region and left margin) when set.
                            self.origin_mode = set;
                            self.cursor.col = if set { self.left_margin() } else { 0 };
                            self.cursor.row = if set { self.scroll_top } else { 0 };
                        }
                        7 => {
                            // DECAWM — autowrap (T3-3), defaults ON.
                            self.autowrap = set;
                        }
                        25 => {
                            self.cursor_visible = set;
                        }
                        66 => {
                            // DECNKM — keypad application mode (numeric off).
                            self.keypad_app = set;
                        }
                        67 => {
                            // DECBKM — backarrow key sends DEL when set, BS
                            // when reset (default).
                            self.backarrow_del = set;
                        }
                        69 => {
                            // DECLRMM — left/right margin mode. Enabling keeps
                            // the current margins; disabling returns to full
                            // width (VT420).
                            self.left_right_margins = set;
                            if !set {
                                self.scroll_left = 0;
                                self.scroll_right = self.cols - 1;
                            }
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
                                // Clear alternate screen (all rows share the
                                // blank; flags say they need a redraw).
                                self.cells_alt = vec![self.blank_row.clone(); self.rows];
                                self.row_is_blank = vec![true; self.rows];
                                self.row_blank_dirty = vec![true; self.rows];
                                self.cursor = Cursor::default();
                                // Sixels on the previous screen must not bleed
                                // into the alt screen (shared placement list).
                                self.sixel_images.clear();
                                // Kitty keyboard stacks are per-screen: save the
                                // primary screen's state, activate the alt
                                // screen's saved state.
                                self.kitty_flags_primary = self.kitty_flags;
                                self.kitty_stack_primary = std::mem::take(&mut self.kitty_stack);
                                self.kitty_flags = self.kitty_flags_alt;
                                self.kitty_stack = std::mem::take(&mut self.kitty_stack_alt);
                            } else if !set && self.alt_active {
                                self.alt_active = false;
                                // Restore cursor from the position saved at alt-screen entry
                                self.cursor = self.alt_saved_cursor;
                                // Conservative flags: treat primary rows as
                                // non-blank. A genuinely blank primary row
                                // (still shared) gets cloned once on the next
                                // dirty scan — a negligible cost for an
                                // alt-screen excursion, and never incorrect.
                                self.row_is_blank = vec![false; self.rows];
                                self.row_blank_dirty = vec![false; self.rows];
                                // Drop images drawn while the alt screen was up.
                                self.sixel_images.clear();
                                // Restore the primary screen's kitty keyboard
                                // state; stash the alt screen's for next time.
                                self.kitty_flags_alt = self.kitty_flags;
                                self.kitty_stack_alt = std::mem::take(&mut self.kitty_stack);
                                self.kitty_flags = self.kitty_flags_primary;
                                self.kitty_stack = std::mem::take(&mut self.kitty_stack_primary);
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
                        2048 => {
                            // In-band resize notification (terminal-wg): the
                            // app emits `CSI 4 ; rows ; cols t` on resize so
                            // tmux/neovim can redraw without polling.
                            self.in_band_resize = set;
                            log::debug!(
                                "In-band resize reporting: {}",
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
            // DECREQTPARM — request terminal parameters: CSI Ps x. Reports
            // the VT220 default (parity none, 8 bits, 112.5 baud ×4 = 9600,
            // 0 transmit bits, 0 receive bits) — the response xterm sends.
            // Request 0 = "will send a report", 1 = "no change requested".
            (b"", b'x') => {
                let request = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
                let reply_type = if request == 1 { 3 } else { 2 };
                let resp = format!("\x1b[{};1;1;112;112;1;0x", reply_type);
                self.respond(resp.as_bytes());
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
            // DECSTR — soft terminal reset: CSI ! p.
            (b"!", b'p') => {
                self.soft_reset();
            }
            // DECSCPP — set columns per page: CSI Ps $ |. 80 or 132 (0 = no
            // change). Shares the DECCOLM machinery.
            (b"$", b'|') => {
                let cols = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
                if cols == 80 || cols == 132 {
                    self.set_columns(cols as usize);
                }
            }
            // DECSCA — select character attributes: CSI Ps " q. Ps 0/1 =
            // unprotected (default), 2 = protected. Protection is sticky and
            // independent of SGR: protected cells survive selective erase
            // (DECSED/DECSEL/DECSERA). It rides on the active SGR attrs bit.
            (b"\"", b'q') => {
                let n = p0;
                self.active_attrs.set_protected(n == 2);
            }
            // DECFRA — fill rectangular area: CSI Ps;Pt;Pl;Pp;Pv $ x. Fills
            // the rectangle (top/left/bottom/right, 1-based, cursor-relative
            // in origin mode) with the character Ps (space if omitted), using
            // the current SGR.
            (b"$", b'x') => {
                let flat: Vec<u16> = params
                    .iter()
                    .map(|p| p.first().copied().unwrap_or(0))
                    .collect();
                let fill = flat.first().copied().unwrap_or(b' ' as u16);
                let top = flat.get(1).copied().unwrap_or(1);
                let left = flat.get(2).copied().unwrap_or(1);
                let bottom = flat.get(3).copied().unwrap_or(1);
                let right = flat.get(4).copied().unwrap_or(1);
                self.fill_rect(
                    top as usize,
                    left as usize,
                    bottom as usize,
                    right as usize,
                    fill as u16,
                    false,
                );
            }
            // DECERA — erase rectangular area: CSI Pt;Pl;Pp;Pv $ z. Replaces
            // the rectangle's characters and attributes with default blanks.
            (b"$", b'z') => {
                let flat: Vec<u16> = params
                    .iter()
                    .map(|p| p.first().copied().unwrap_or(0))
                    .collect();
                let top = flat.first().copied().unwrap_or(1);
                let left = flat.get(1).copied().unwrap_or(1);
                let bottom = flat.get(2).copied().unwrap_or(1);
                let right = flat.get(3).copied().unwrap_or(1);
                let old_fg = self.active_fg;
                let old_bg = self.active_bg;
                let old_attrs = self.active_attrs;
                self.active_fg = Color::Default;
                self.active_bg = Color::Default;
                self.active_attrs = Attrs::default();
                self.fill_rect(
                    top as usize,
                    left as usize,
                    bottom as usize,
                    right as usize,
                    b' ' as u16,
                    false,
                );
                self.active_fg = old_fg;
                self.active_bg = old_bg;
                self.active_attrs = old_attrs;
            }
            // DECSERA — selective erase rectangular area: CSI Pt;Pl;Pp;Pv $ {.
            // Like DECERA but protected (DECSCA) cells are left intact.
            (b"$", b'{') => {
                let flat: Vec<u16> = params
                    .iter()
                    .map(|p| p.first().copied().unwrap_or(0))
                    .collect();
                let top = flat.first().copied().unwrap_or(1);
                let left = flat.get(1).copied().unwrap_or(1);
                let bottom = flat.get(2).copied().unwrap_or(1);
                let right = flat.get(3).copied().unwrap_or(1);
                let old_fg = self.active_fg;
                let old_bg = self.active_bg;
                let old_attrs = self.active_attrs;
                self.active_fg = Color::Default;
                self.active_bg = Color::Default;
                self.active_attrs = Attrs::default();
                self.fill_rect(
                    top as usize,
                    left as usize,
                    bottom as usize,
                    right as usize,
                    b' ' as u16,
                    true,
                );
                self.active_fg = old_fg;
                self.active_bg = old_bg;
                self.active_attrs = old_attrs;
            }
            // DECIC — insert columns: CSI Ps ' }. Inserts Ps blank columns at
            // the cursor, shifting content right off the right margin.
            (b"'", b'}') => {
                let n = params
                    .first()
                    .and_then(|p| p.first())
                    .copied()
                    .unwrap_or(1)
                    .max(1) as usize;
                self.insert_columns(n);
            }
            // DECDC — delete columns: CSI Ps ' ~. Deletes Ps columns at the
            // cursor, pulling content left and blanking the right edge.
            (b"'", b'~') => {
                let n = params
                    .first()
                    .and_then(|p| p.first())
                    .copied()
                    .unwrap_or(1)
                    .max(1) as usize;
                self.delete_columns(n);
            }
            // Kitty keyboard protocol (CSI u) progressive enhancement.
            // `CSI > flags u` pushes the current flags and sets new flags
            // (default 0) — the quickstart `CSI > 1 u` enables disambiguation.
            // `CSI < n u` pops n entries (default 1).
            // `CSI = flags ; mode u` applies flags with mode semantics.
            // `CSI ? u` queries the current flags (replies `CSI ? flags u`).
            (b">", b'u') => {
                let flags = params
                    .first()
                    .and_then(|group| group.first())
                    .copied()
                    .unwrap_or(0) as u8;
                self.kitty_stack.push(self.kitty_flags);
                self.kitty_flags = flags;
            }
            (b"<", b'u') => {
                let count = params
                    .first()
                    .and_then(|group| group.first())
                    .copied()
                    .unwrap_or(1)
                    .max(1) as usize;
                for _ in 0..count {
                    self.kitty_flags = self.kitty_stack.pop().unwrap_or(0);
                }
            }
            (b"=", b'u') => {
                let groups: Vec<u16> = params
                    .iter()
                    .map(|group| group.first().copied().unwrap_or(0))
                    .collect();
                let flags = groups.first().copied().unwrap_or(0) as u8;
                let mode = groups.get(1).copied().unwrap_or(1);
                self.kitty_flags = match mode {
                    // mode 2: set bits only (unset bits unchanged)
                    2 => self.kitty_flags | flags,
                    // mode 3: reset bits only (unset bits unchanged)
                    3 => self.kitty_flags & !flags,
                    // mode 1 (default): replace flags wholesale
                    _ => flags,
                };
            }
            (b"?", b'u') => {
                self.respond(format!("\x1b[?{}u", self.kitty_flags).as_bytes());
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
            // OSC 7 — Set current working directory (shell integration).
            // Format: OSC 7 ; file://host/path  (VS Code / tmux convention).
            // The raw URI is stored; the app can percent-decode when needed.
            7 => {
                if params.len() >= 2 {
                    let uri = std::str::from_utf8(&params[1]).unwrap_or("");
                    if uri.is_empty() {
                        self.cwd = None;
                    } else {
                        self.cwd = Some(uri.to_string());
                        log::debug!("OSC 7: cwd = {}", uri);
                    }
                }
            }
            // OSC 9 — Desktop notification: OSC 9 ; text
            // OSC 9 ; 4 ; title ; message — progress/notification form
            // (we surface the message; actual desktop integration is the app's
            // job, gated by the bell/notification policy).
            9 => {
                let message = if params.len() >= 4 && params[1] == b"4" {
                    Some(params[3].to_vec())
                } else if params.len() >= 2 {
                    Some(params[1].to_vec())
                } else {
                    None
                };
                if let Some(msg) = message {
                    if let Ok(s) = String::from_utf8(msg) {
                        if s.is_empty() {
                            self.notification = None;
                        } else {
                            log::debug!("OSC 9: notification = {}", s);
                            self.notification = Some(s);
                        }
                    }
                }
            }
            // OSC 133 — Shell integration (FinalTerm/VS Code protocol):
            //   A = prompt start, E = prompt (clean) start
            //   B = command start
            //   C = end of command output
            //   D = command finished (optional ; exit status)
            // Rows are marked so the app can jump between prompts.
            133 => {
                let row = self.cursor.row;
                if let Some(kind) = params.get(1) {
                    if kind == b"A" || kind == b"E" {
                        // Prompt start.
                        self.set_row_marker(row, 1);
                    } else if kind == b"B" {
                        // Command start.
                        self.set_row_marker(row, 2);
                    } else if kind == b"C" || kind == b"D" {
                        // End of output / command finished.
                        self.set_row_marker(row, 3);
                    }
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
            for cell in line.iter() {
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

            Action::ApcDispatch { data } => {
                self.handle_apc(&data);
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
                        for r in 0..rows {
                            self.row_is_blank[r] = false;
                            let row = Arc::make_mut(&mut self.cells_mut()[r]);
                            for cell in row.iter_mut() {
                                cell.ch = 'E';
                                cell.fg = Color::Default;
                                cell.bg = Color::Default;
                                cell.attrs = Attrs::default();
                                cell.dirty = true;
                                cell.wide_filler = false;
                                cell.hyperlink_id = 0;
                            }
                        }
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
                    // DECPAM — keypad application mode (ESC =).
                    (_, b'=') => {
                        self.keypad_app = true;
                    }
                    // DECPNM — keypad numeric mode (ESC >).
                    (_, b'>') => {
                        self.keypad_app = false;
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
                // Sixel graphics: DCS ... q with no `$` intermediate. The
                // intro parameters (pan/pad) are permitted and ignored.
                self.dcs_sixel = final_byte == b'q' && !self.dcs_request;
            }
            Action::Put(byte) => {
                // Accumulate DCS data bytes (DECRQSS sends the query string,
                // sixel sends the encoded image). Sixel gets a larger cap.
                let cap = if self.dcs_sixel {
                    MAX_DCS_LEN
                } else {
                    MAX_OSC_DCS_LEN
                };
                if self.dcs_buf.len() < cap {
                    self.dcs_buf.push(byte);
                }
            }
            Action::Unhook => {
                // On DCS end, answer a DECRQSS request we recognized, or
                // decode and place a completed sixel image.
                if self.dcs_request {
                    self.answer_decrqss();
                } else if self.dcs_sixel {
                    self.place_sixel();
                }
                self.dcs_request = false;
                self.dcs_sixel = false;
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
    fn test_kitty_graphics_places_rgb_image() {
        use base64::Engine as _;
        let mut g = make_grid(80, 24);
        // 2x1 RGB24 image (f=24): red, green.
        let raw = [255u8, 0, 0, 0, 255, 0];
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        let mut seq = format!("\x1b_Ga=T,f=24,s=2,v=1;{b64}").into_bytes();
        seq.extend_from_slice(b"\x1b\\");
        feed(&mut g, &seq);

        assert_eq!(g.sixel_images.len(), 1);
        let p = &g.sixel_images[0];
        assert_eq!(p.image.width, 2);
        assert_eq!(p.image.height, 1);
        assert_eq!(p.image.rgba, vec![255, 0, 0, 255, 0, 255, 0, 255]);
        assert_eq!((p.col, p.row), (0, 0));
    }

    #[test]
    fn test_kitty_graphics_places_rgba_image() {
        use base64::Engine as _;
        let mut g = make_grid(80, 24);
        // 2x1 RGBA image (f=32, the default): red@255, green@128.
        let raw = [255u8, 0, 0, 255, 0, 255, 0, 128];
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        let mut seq = format!("\x1b_Ga=T,f=32,s=2,v=1;{b64}").into_bytes();
        seq.extend_from_slice(b"\x1b\\");
        feed(&mut g, &seq);

        assert_eq!(g.sixel_images.len(), 1);
        let p = &g.sixel_images[0];
        assert_eq!(p.image.rgba, vec![255, 0, 0, 255, 0, 255, 0, 128]);
    }

    #[test]
    fn test_kitty_graphics_chunked() {
        use base64::Engine as _;
        let mut g = make_grid(80, 24);
        let raw = [255u8, 0, 0, 0, 255, 0];
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        let (c1, c2) = b64.split_at(b64.len() / 2);
        let mut seq = format!("\x1b_Ga=T,f=24,s=2,v=1,m=1;{c1}").into_bytes();
        seq.extend_from_slice(b"\x1b\\");
        seq.extend_from_slice(format!("\x1b_Gm=0;{c2}").as_bytes());
        seq.extend_from_slice(b"\x1b\\");
        feed(&mut g, &seq);

        assert_eq!(g.sixel_images.len(), 1);
        assert_eq!(
            g.sixel_images[0].image.rgba,
            vec![255, 0, 0, 255, 0, 255, 0, 255]
        );
    }

    #[test]
    fn test_kitty_graphics_png() {
        use base64::Engine as _;
        let mut g = make_grid(80, 24);
        // Encode a 2x1 RGB PNG with the `png` crate, then transmit it as f=100.
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, 2, 1);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().expect("png header");
            writer
                .write_image_data(&[255, 0, 0, 0, 255, 0])
                .expect("png data");
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        let mut seq = format!("\x1b_Ga=T,f=100;{b64}").into_bytes();
        seq.extend_from_slice(b"\x1b\\");
        feed(&mut g, &seq);

        assert_eq!(g.sixel_images.len(), 1);
        let p = &g.sixel_images[0];
        assert_eq!((p.image.width, p.image.height), (2, 1));
        assert_eq!(p.image.rgba, vec![255, 0, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn test_kitty_graphics_transmit_put_delete() {
        use base64::Engine as _;
        let mut g = make_grid(80, 24);
        let raw = [255u8, 0, 0, 255]; // 1x1 RGBA red
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);

        // Transmit only (a=t,i=7): stores the image, replies OK, no placement.
        let mut seq = format!("\x1b_Ga=t,i=7,f=32,s=1,v=1;{b64}").into_bytes();
        seq.extend_from_slice(b"\x1b\\");
        feed(&mut g, &seq);
        assert_eq!(g.sixel_images.len(), 0);
        assert_eq!(g.kitty_images.len(), 1);
        assert_eq!(g.take_responses(), vec![b"\x1b_Gi=7;OK\x1b\\".to_vec()]);

        // Put (a=p,i=7): displays the stored image and replies OK.
        let mut seq = b"\x1b_Ga=p,i=7;\x1b\\".to_vec();
        feed(&mut g, &seq);
        assert_eq!(g.sixel_images.len(), 1);
        assert_eq!(g.sixel_images[0].image_id, 7);
        assert_eq!(g.take_responses(), vec![b"\x1b_Gi=7;OK\x1b\\".to_vec()]);

        // Put an unknown id replies ENOENT and places nothing.
        feed(&mut g, b"\x1b_Ga=p,i=999;\x1b\\");
        assert_eq!(g.sixel_images.len(), 1);
        assert_eq!(
            g.take_responses(),
            vec![b"\x1b_Gi=999;ENOENT:no such image\x1b\\".to_vec()]
        );

        // Delete (a=d,i=7) removes the stored image and its placement.
        feed(&mut g, b"\x1b_Ga=d,d=I,i=7;\x1b\\");
        assert!(g.kitty_images.is_empty());
        assert!(g.sixel_images.is_empty());
    }

    #[test]
    fn test_kitty_graphics_query() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b_Ga=q,i=1;\x1b\\");
        assert_eq!(g.take_responses(), vec![b"\x1b_Gi=1;OK\x1b\\".to_vec()]);
        assert!(g.kitty_images.is_empty());
        assert!(g.sixel_images.is_empty());
    }

    #[test]
    fn test_kitty_graphics_file_transmission() {
        use base64::Engine as _;
        let mut g = make_grid(80, 24);
        // Write a 1x1 RGBA red image to a temp file.
        let dir = std::env::temp_dir().join(format!("kitty-gfx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("img.rgba");
        std::fs::write(&path, [255u8, 0, 0, 255]).expect("write");
        let path_str = path.to_string_lossy();
        let b64 = base64::engine::general_purpose::STANDARD.encode(path_str.as_bytes());

        // t=f,f=32,s=1,v=1 → read the file as raw RGBA.
        let mut seq = format!("\x1b_Ga=T,f=32,s=1,v=1,t=f;{b64}").into_bytes();
        seq.extend_from_slice(b"\x1b\\");
        feed(&mut g, &seq);
        assert_eq!(g.sixel_images.len(), 1);
        assert_eq!(g.sixel_images[0].image.rgba, vec![255, 0, 0, 255]);

        // Sensitive path is refused.
        let blocked = base64::engine::general_purpose::STANDARD.encode(b"/proc/self/mem");
        let mut seq = format!("\x1b_Ga=t,i=9,f=32,s=1,v=1,t=f;{blocked}").into_bytes();
        seq.extend_from_slice(b"\x1b\\");
        feed(&mut g, &seq);
        assert!(g.kitty_images.is_empty(), "sensitive path must be refused");
        assert!(g.take_responses()[0].windows(3).any(|w| w == b"EIO"));

        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn test_consecutive_lf_batched_into_single_scroll() {
        // The batch path (advance_bytes) must collapse a run of LFs into one
        // scroll instead of scrolling per-LF.
        let mut g = make_grid(10, 3);
        g.cursor = crate::grid::Cursor { col: 0, row: 2 };
        let mut p = crate::parser::Parser::new();
        p.advance_bytes(&mut g, b"\n\n\n\n");
        // 4 LFs from the bottom of a 3-row screen scroll exactly 3 lines
        // (the region clamps), and the cursor stays at the bottom.
        assert_eq!(g.cursor.row, 2);
        assert_eq!(g.scrollback.len(), 3);
        // All scrolled-off rows were blank (grid started empty).
        for line in g.scrollback.iter() {
            assert!(line.iter().all(|c| c.ch == ' '));
        }
    }

    #[test]
    fn test_erase_line_at_pending_wrap_does_not_panic() {
        // Fuzz-found: after filling a row the cursor sits one column past the
        // end (pending wrap). EL mode 1 from that state used to index out of
        // bounds; it must clamp to the row length.
        let mut g = make_grid(10, 3);
        feed(&mut g, b"abcdefghij"); // fills row 0, cursor.col == 10
        assert_eq!(g.cursor.col, 10);
        feed(&mut g, b"\x1b[1K"); // EL mode 1 — must not panic
        assert_eq!(g.cursor.col, 10);
        // ED mode 1 has the same shape.
        feed(&mut g, b"\x1b[1J");
        assert_eq!(g.cursor.col, 10);
    }

    #[test]
    fn test_scrollback_eviction_recycles_blanks() {
        // Heavy scrolling past a tiny scrollback exercises the blank-row
        // recycle path: evicted rows are reset and reused, and the grid must
        // stay correct (bounded scrollback, newest lines kept, clean blanks).
        let mut g = make_grid(10, 3);
        g.scrollback_capacity = 2;
        let mut p = crate::parser::Parser::new();
        let mut payload = Vec::new();
        for i in 0..6 {
            payload.extend_from_slice(format!("LINE{i}\r\n").as_bytes());
        }
        p.advance_bytes(&mut g, &payload);
        // Scrollback bounded by capacity; oldest scrolled lines evicted.
        assert_eq!(g.scrollback.len(), 2);
        assert_eq!(g.scrollback[0][0].ch, 'L');
        assert_eq!(g.scrollback[0][4].ch, '2');
        assert_eq!(g.scrollback[1][4].ch, '3');
        // Visible rows hold the newest lines; the bottom is a clean blank.
        assert_eq!(g.cell(4, 0).ch, '4');
        assert_eq!(g.cell(4, 1).ch, '5');
        for c in 0..10 {
            assert_eq!(g.cell(c, 2).ch, ' ');
        }
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
    fn test_deccolm_switches_columns() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"text\x1b[?3h"); // DECCOLM set → 80 columns, cleared
        assert_eq!(g.cols, 80);
        assert_eq!(g.rows, 24);
        assert_eq!(g.cursor.col, 0, "DECCOLM should home the cursor");
        assert_eq!(
            g.line_to_string(0),
            "".to_owned() + &" ".repeat(80),
            "DECCOLM should clear"
        );
        feed(&mut g, b"\x1b[?3l"); // DECCOLM reset → 132 columns
        assert_eq!(g.cols, 132);
        assert_eq!(g.rows, 24);
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
    fn test_cht_forward_tab() {
        let mut g = make_grid(40, 3);
        feed(&mut g, b"\x1b[2I"); // CHT(2): col 0 → 8 → 16
        assert_eq!(g.cursor.col, 16);
        feed(&mut g, b"\x1b[2Z"); // CBT(2): back to col 0
        assert_eq!(g.cursor.col, 0);
        // Past the last stop → clamp to the last column.
        feed(&mut g, b"\x1b[9I");
        assert_eq!(g.cursor.col, 39);
    }

    // -- vt100.net research pass: DECSTR, DECREQTPARM, DECCOLM/DECSCPP,
    //    DECIC/DECDC, DECFRA/DECERA, DECNKM/DECBKM --

    #[test]
    fn test_decstr_soft_reset() {
        let mut g = make_grid(10, 6);
        feed(&mut g, b"\x1b[?7l\x1b[4h\x1b[5;1Hxy"); // autowrap off, insert on, cursor row 4
        assert!(!g.autowrap && g.insert_mode);
        feed(&mut g, b"\x1b[!p"); // DECSTR
        assert!(g.autowrap, "autowrap not restored");
        assert!(!g.insert_mode, "insert mode not cleared");
        assert_eq!(g.cursor.row, 0, "cursor not homed");
        assert_eq!(g.cursor.col, 0);
        assert_eq!(g.line_to_string(4), "          ", "screen not cleared");
    }

    #[test]
    fn test_decreqtparm_reports() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[x");
        assert_eq!(
            g.take_responses(),
            vec![b"\x1b[2;1;1;112;112;1;0x".to_vec()],
            "DECREQTPARM request 0 reply wrong"
        );
        feed(&mut g, b"\x1b[1x");
        assert_eq!(
            g.take_responses(),
            vec![b"\x1b[3;1;1;112;112;1;0x".to_vec()],
            "DECREQTPARM request 1 reply wrong"
        );
    }

    #[test]
    fn test_deccolm_and_decscpp_resize() {
        let mut g = make_grid(40, 4);
        feed(&mut g, b"hello\x1b[2;3H");
        feed(&mut g, b"\x1b[?3h"); // DECCOLM set → 80 columns
        assert_eq!(g.cols, 80, "DECCOLM did not switch to 80");
        assert_eq!(g.cursor.row, 0, "DECCOLM did not home cursor");
        assert_eq!(g.cursor.col, 0);
        assert_eq!(
            g.line_to_string(0),
            "".to_owned() + &" ".repeat(80),
            "DECCOLM did not clear"
        );
        feed(&mut g, b"\x1b[132$|"); // DECSCPP 132
        assert_eq!(g.cols, 132, "DECSCPP 132 failed");
        feed(&mut g, b"\x1b[?3l"); // DECCOLM reset → 132
        assert_eq!(g.cols, 132, "DECCOLM reset should stay 132 (already 132)");
        assert!(
            g.window_resize_request.is_some(),
            "window resize not requested"
        );
    }

    #[test]
    fn test_decic_and_decdc_columns() {
        let mut g = make_grid(8, 2);
        feed(&mut g, b"abcdef\x1b[4D"); // cursor at col 2
        feed(&mut g, b"\x1b['}"); // DECIC 1: insert blank column
        assert_eq!(
            g.line_to_string(0),
            "ab cdef ",
            "DECIC insert wrong: {:?}",
            g.line_to_string(0)
        );
        feed(&mut g, b"\x1b[2D\x1b['~"); // back to col 0, DECDC 1: delete col 0
        assert_eq!(
            g.line_to_string(0),
            "b cdef  ",
            "DECDC delete wrong: {:?}",
            g.line_to_string(0)
        );
    }

    #[test]
    fn test_decfra_fills_and_decera_erases() {
        let mut g = make_grid(8, 4);
        feed(&mut g, b"\x1b[65;2;2;3;4$x"); // DECFRA: fill rows 2-3, cols 2-4 with 'A'
        assert_eq!(g.cell(1, 1).ch, 'A', "DECFRA top-left");
        assert_eq!(g.cell(3, 2).ch, 'A', "DECFRA bottom-right");
        assert_eq!(g.cell(0, 0).ch, ' ', "DECFRA leaked outside rect");
        feed(&mut g, b"\x1b[1;1;2;3$z"); // DECERA: erase rows 1-2, cols 1-3
        assert_eq!(g.cell(0, 0).ch, ' ', "DECERA top-left not erased");
        // (col 2, row 1) was filled by DECFRA and lies inside the erase
        // rectangle → blank now; (col 3, row 1) is outside it → still 'A'.
        assert_eq!(g.cell(2, 1).ch, ' ', "DECERA did not erase filled cells");
        assert_eq!(g.cell(3, 1).ch, 'A', "DECERA over-erased");
    }

    #[test]
    fn test_decpam_decnkm_and_decbkm() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[?66h");
        assert!(g.keypad_app, "DECNKM set failed");
        feed(&mut g, b"\x1b>"); // DECPNM
        assert!(!g.keypad_app, "DECPNM clear failed");
        feed(&mut g, b"\x1b="); // DECPAM
        assert!(g.keypad_app, "DECPAM set failed");
        feed(&mut g, b"\x1b[?67h");
        assert!(g.backarrow_del, "DECBKM set failed");
        feed(&mut g, b"\x1b[?67$p");
        assert_eq!(
            g.take_responses(),
            vec![b"\x1b[?67;1$y".to_vec()],
            "DECRQM mode 67 mismatch"
        );
    }

    #[test]
    fn test_decslrm_margins_bound_wrap_el_and_movement() {
        let mut g = make_grid(8, 3);
        feed(&mut g, b"\x1b[?69h\x1b[3;6s"); // LRMM on, margins cols 3..6 (0-based 2..5)
        assert!(g.left_right_margins);
        assert_eq!((g.scroll_left, g.scroll_right), (2, 5));
        assert_eq!(
            (g.cursor.col, g.cursor.row),
            (2, 0),
            "DECSLRM should home into the margins"
        );
        feed(&mut g, b"abcdefgh");
        // 'abcd' fills cols 2-5 of row 0; 'e' wraps to the left margin of row 1.
        assert_eq!(g.line_to_string(0), "  abcd  ");
        assert_eq!(g.line_to_string(1), "  efgh  ");
        // CUF/CUB are bounded by the margins.
        feed(&mut g, b"\x1b[3;1H"); // cursor to (2, 0)
        feed(&mut g, b"\x1b[9C"); // CUF 9 → clamped at right margin (col 5)
        assert_eq!(g.cursor.col, 5);
        feed(&mut g, b"\x1b[9D"); // CUB 9 → clamped at left margin (col 2)
        assert_eq!(g.cursor.col, 2);
        // EL 2 is bounded by the margins: only cols 2-5 of the row are wiped.
        feed(&mut g, b"\x1b[1;1H\x1b[2K");
        assert_eq!(g.line_to_string(0), "        ");
        // DECSTBM resets the left/right margins to full width.
        feed(&mut g, b"\x1b[2;2r");
        assert_eq!((g.scroll_left, g.scroll_right), (0, 7));
    }

    #[test]
    fn test_decsca_protects_against_selective_erase() {
        // DECSCA 2 protects: plain EL 2 wipes everything…
        let mut g1 = make_grid(8, 2);
        feed(&mut g1, b"\x1b[2\"qab\x1b[0\"qcd\x1b[1;1H\x1b[2K");
        assert_eq!(g1.line_to_string(0), "        ");

        // …but DECSEL (?K) skips protected cells.
        let mut g2 = make_grid(8, 2);
        feed(&mut g2, b"\x1b[2\"qab\x1b[0\"qcd\x1b[1;1H\x1b[?2K");
        assert_eq!(g2.line_to_string(0), "ab      ");

        // DECSED (?J) skips protected cells too (cursor → end of screen).
        let mut g3 = make_grid(8, 2);
        feed(&mut g3, b"\x1b[2\"qab\x1b[0\"qcd\r\nxy\x1b[1;1H\x1b[?J");
        assert_eq!(g3.line_to_string(0), "ab      ");
        assert_eq!(g3.line_to_string(1), "        ");

        // DECSERA ($ {) erases a rectangle but leaves protected cells.
        let mut g4 = make_grid(8, 2);
        feed(&mut g4, b"\x1b[2\"qA\x1b[0\"qB");
        feed(&mut g4, b"\x1b[1;1;2;2${");
        assert_eq!(g4.cell(0, 0).ch, 'A', "DECSERA erased a protected cell");
        assert_eq!(g4.cell(1, 0).ch, ' ', "DECSERA left an unprotected cell");
    }

    #[test]
    fn test_decstr_clears_protection() {
        let mut g = make_grid(8, 2);
        // DECSCA 2 protects; DECSCA and SGR are independent (VT420), so SGR
        // 0;1 restyles without losing protection.
        feed(&mut g, b"\x1b[2\"qA\x1b[0;1mB");
        assert!(g.cell(0, 0).attrs.protected(), "DECSCA 2 did not protect");
        assert!(g.cell(1, 0).attrs.bold(), "SGR 0;1 after DECSCA broke");
        assert!(
            g.cell(1, 0).attrs.protected(),
            "protection lost on SGR reset"
        );
        // DECSCA 0 clears protection for subsequent characters.
        feed(&mut g, b"\x1b[0\"qC");
        assert!(
            !g.cell(2, 0).attrs.protected(),
            "DECSCA 0 did not unprotect"
        );
        // DECSTR resets protection so selective erase wipes everything.
        feed(&mut g, b"\x1b[!p\x1b[?2K");
        assert_eq!(g.line_to_string(0), "        ");
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
        // Quickstart: push + enable disambiguation.
        feed(&mut g, b"\x1b[>1u");
        assert_eq!(g.kitty_flags, 0b1);
        // Pop restores the previous (zero) flags.
        feed(&mut g, b"\x1b[<u");
        assert_eq!(g.kitty_flags, 0);
        // Progressive enhancement set with mode semantics.
        feed(&mut g, b"\x1b[=5u"); // disambiguate + alternate keys
        assert_eq!(g.kitty_flags, 0b101);
        feed(&mut g, b"\x1b[=2;2u"); // mode 2: set event-types bit
        assert_eq!(g.kitty_flags, 0b111);
        feed(&mut g, b"\x1b[=4;3u"); // mode 3: reset alternate-keys bit
        assert_eq!(g.kitty_flags, 0b011);
        // Query replies with the current flags.
        feed(&mut g, b"\x1b[?u");
        assert_eq!(g.take_responses(), vec![b"\x1b[?3u".to_vec()]);
        // Nested push/pop.
        feed(&mut g, b"\x1b[>0u");
        assert_eq!(g.kitty_flags, 0);
        feed(&mut g, b"\x1b[<u");
        assert_eq!(g.kitty_flags, 0b011);
        // modifyOtherKeys unaffected.
        feed(&mut g, b"\x1b[>4;1m");
        assert_eq!(g.modify_other_keys, 1);
        feed(&mut g, b"\x1b[>4;0m");
        assert_eq!(g.modify_other_keys, 0);
    }

    #[test]
    fn test_kitty_keyboard_flags_are_per_screen() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"\x1b[>1u"); // primary: disambiguate
        assert_eq!(g.kitty_flags, 0b1);
        feed(&mut g, b"\x1b[?1049h"); // enter alt screen
        assert_eq!(g.kitty_flags, 0, "alt screen starts with fresh flags");
        feed(&mut g, b"\x1b[>5u"); // alt: disambiguate + event types
        assert_eq!(g.kitty_flags, 0b101);
        feed(&mut g, b"\x1b[?1049l"); // exit alt screen
        assert_eq!(g.kitty_flags, 0b1, "primary flags restored on exit");
        feed(&mut g, b"\x1b[?1049h"); // re-enter alt
        assert_eq!(g.kitty_flags, 0b101, "alt flags remembered");
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

    // -----------------------------------------------------------------------
    // Shell integration (OSC 133) markers
    // -----------------------------------------------------------------------

    #[test]
    fn test_osc133_prompt_markers() {
        let mut g = make_grid(20, 6);
        feed(
            &mut g,
            b"\x1b]133;A\x07prompt>\r\n\x1b]133;B\x07ls\r\n\x1b]133;C\x07",
        );
        assert_eq!(g.shell_markers[0], 1); // prompt row
        assert_eq!(g.shell_markers[1], 2); // command row
        assert_eq!(g.shell_markers[2], 3); // output row
    }

    #[test]
    fn test_osc133_clean_prompt_and_exit() {
        let mut g = make_grid(20, 6);
        feed(&mut g, b"\x1b]133;E\x07\r\n\x1b]133;D;0\x07");
        assert_eq!(g.shell_markers[0], 1); // 133;E → prompt marker
        assert_eq!(g.shell_markers[1], 3); // 133;D → output marker
    }

    #[test]
    fn test_prompt_markers_follow_scroll() {
        let mut g = make_grid(10, 4);
        g.scrollback_capacity = 32;
        feed(&mut g, b"\x1b]133;A\x07");
        // Scroll the marked row into scrollback.
        feed(&mut g, b"a\r\nb\r\nc\r\nd\r\ne\r\n");
        assert_eq!(g.shell_scrollback_markers.len(), g.scrollback.len());
        assert_eq!(
            g.shell_scrollback_markers.iter().find(|m| **m == 1),
            Some(&1)
        );
        // The prompt is reachable by scanning backwards from the end.
        let stream_len = g.marker_stream_len();
        assert!(g.prev_prompt(stream_len).is_some());
        assert_eq!(g.prev_prompt(0), None);
    }

    #[test]
    fn test_prev_next_prompt_stream() {
        let mut g = make_grid(20, 5);
        feed(
            &mut g,
            b"\x1b]133;A\x07one\r\n\x1b]133;A\x07two\r\n\x1b]133;A\x07three",
        );
        // Prompts on rows 0, 1, 2 → stream indices 0, 1, 2 (no scrollback yet).
        assert_eq!(g.prev_prompt(3), Some(2));
        assert_eq!(g.prev_prompt(2), Some(1));
        assert_eq!(g.next_prompt(0), Some(1));
        assert_eq!(g.next_prompt(2), None);
    }

    // -----------------------------------------------------------------------
    // In-band resize (mode 2048), OSC 7/9, sixel
    // -----------------------------------------------------------------------

    #[test]
    fn test_mode_2048_resize_report() {
        let mut g = make_grid(10, 5);
        feed(&mut g, b"\x1b[?2048h");
        assert!(g.in_band_resize);
        g.resize_report();
        assert_eq!(g.take_responses(), vec![b"\x1b[4;5;10t".to_vec()]);
        feed(&mut g, b"\x1b[?2048l");
        g.resize_report();
        assert!(g.take_responses().is_empty());
    }

    #[test]
    fn test_decrqm_reports_2048() {
        let mut g = make_grid(10, 5);
        feed(&mut g, b"\x1b[?2048h\x1b[?2048$p");
        assert_eq!(g.take_responses(), vec![b"\x1b[?2048;1$y".to_vec()]);
        feed(&mut g, b"\x1b[?2048l\x1b[?2048$p");
        assert_eq!(g.take_responses(), vec![b"\x1b[?2048;2$y".to_vec()]);
    }

    #[test]
    fn test_osc7_cwd() {
        let mut g = make_grid(20, 3);
        feed(&mut g, b"\x1b]7;file:///home/user/projects\x07");
        assert_eq!(g.cwd.as_deref(), Some("file:///home/user/projects"));
    }

    #[test]
    fn test_osc9_notification() {
        let mut g = make_grid(20, 3);
        feed(&mut g, b"\x1b]9;hello world\x07");
        assert_eq!(g.take_notification().as_deref(), Some("hello world"));
        // Progress form: OSC 9;4;title;message
        feed(&mut g, b"\x1b]9;4;Task;done\x07");
        assert_eq!(g.take_notification().as_deref(), Some("done"));
        // Empty clears the pending notification.
        feed(&mut g, b"\x1b]9;\x07");
        assert_eq!(g.take_notification(), None);
    }

    #[test]
    fn test_sixel_dcs_places_image() {
        let mut g = make_grid(40, 10);
        g.set_cell_size(8, 16);
        // DCS q, blue columns at band 0 and band 1 (after LF), ST.
        feed(&mut g, b"\x1bPq#1~$~$~$~-~\x1b\\");
        assert_eq!(g.sixel_images.len(), 1);
        let img = &g.sixel_images[0];
        assert_eq!(img.col, 0);
        assert_eq!(img.row, 0);
        assert_eq!(img.image.width, 1);
        assert_eq!(img.image.height, 12);
        // Cursor advanced below the image: 12px / 16px cell → 1 row.
        assert_eq!(g.cursor.row, 1);
        assert_eq!(g.cursor.col, 0);
    }

    #[test]
    fn test_sixel_respects_cursor_position() {
        let mut g = make_grid(40, 10);
        g.set_cell_size(8, 16);
        feed(&mut g, b"abc\x1bPq#1~\x1b\\");
        assert_eq!(g.sixel_images[0].col, 3);
    }

    #[test]
    fn test_sixel_bad_payload_is_ignored() {
        let mut g = make_grid(40, 10);
        g.set_cell_size(8, 16);
        // Only control characters (CR/LF) — no graphic chars, nothing drawn.
        feed(&mut g, b"\x1bPq$-$-\x1b\\");
        assert!(g.sixel_images.is_empty());
        // And the cursor is untouched.
        assert_eq!(g.cursor.row, 0);
        assert_eq!(g.cursor.col, 0);
    }

    // -- Sixel lifecycle (scroll / clear / resize) --

    fn placement(id: u64, row: usize, col: usize) -> crate::sixel::SixelPlacement {
        crate::sixel::SixelPlacement {
            id,
            col,
            row,
            image: crate::sixel::SixelImage {
                width: 8,
                height: 8,
                rgba: vec![0u8; 8 * 8 * 4],
            },
            image_id: 0,
        }
    }

    #[test]
    fn test_sixel_scroll_shifts_rows_and_drops_off_top() {
        let mut g = make_grid(40, 6);
        g.sixel_images = vec![placement(1, 2, 0), placement(2, 0, 5)];
        g.scroll_up_from(0, 1);
        // The image at row 2 travels up with its content; the one at the very
        // top scrolled into history and is dropped.
        assert_eq!(g.sixel_images.len(), 1);
        assert_eq!(g.sixel_images[0].id, 1);
        assert_eq!(g.sixel_images[0].row, 1);
    }

    #[test]
    fn test_sixel_insert_shifts_down_and_drops_off_bottom() {
        let mut g = make_grid(40, 6);
        g.sixel_images = vec![placement(1, 2, 0), placement(2, 5, 0)];
        g.scroll_down_from(0, 1);
        // IL pushes content down; the bottom placement is discarded (IL
        // semantics never save) and the middle one moves down a row.
        assert_eq!(g.sixel_images.len(), 1);
        assert_eq!(g.sixel_images[0].id, 1);
        assert_eq!(g.sixel_images[0].row, 3);
    }

    #[test]
    fn test_sixel_erase_display_clears_screen_and_region() {
        let mut g = make_grid(40, 10);
        g.sixel_images = vec![placement(1, 2, 3), placement(2, 6, 0)];
        // ED 2 clears the whole screen.
        feed(&mut g, b"\x1b[2J");
        assert!(g.sixel_images.is_empty());

        g.sixel_images = vec![placement(1, 2, 3), placement(2, 6, 0)];
        // ED 0 erases cursor → end of screen; cursor is at row 5, so the
        // placement at row 2 survives while the one at row 6 is dropped.
        g.cursor.row = 5;
        g.cursor.col = 0;
        feed(&mut g, b"\x1b[0J");
        assert_eq!(g.sixel_images.len(), 1);
        assert_eq!(g.sixel_images[0].id, 1);
    }

    #[test]
    fn test_sixel_erase_line_removes_anchored_image() {
        let mut g = make_grid(40, 10);
        g.sixel_images = vec![placement(1, 2, 3)];
        // Cursor to row 2 (1-based 3;1H), then EL 2 wipes the whole line.
        feed(&mut g, b"\x1b[3;1H\x1b[2K");
        assert!(g.sixel_images.is_empty());
    }

    #[test]
    fn test_sixel_resize_drops_all_placements() {
        let mut g = make_grid(40, 6);
        g.sixel_images = vec![placement(1, 2, 0), placement(2, 5, 0)];
        g.resize(WinSize { cols: 30, rows: 4 });
        // Reflow re-orders rows, so placements (viewport-relative) can no
        // longer be positioned correctly and are dropped wholesale.
        assert!(g.sixel_images.is_empty());
    }

    #[test]
    fn test_sixel_ids_unique_and_list_capped() {
        let mut g = make_grid(40, 10);
        g.set_cell_size(8, 16);
        // 20 tiny one-pixel images — ids must be unique and the list capped.
        for _ in 0..20 {
            feed(&mut g, b"\x1bPq!1~!1~!1~!1~!1~!1~\x1b\\");
        }
        assert_eq!(g.sixel_images.len(), crate::sixel::MAX_LIVE_SIXELS);
        let mut ids: Vec<u64> = g.sixel_images.iter().map(|p| p.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), crate::sixel::MAX_LIVE_SIXELS);
    }
}
