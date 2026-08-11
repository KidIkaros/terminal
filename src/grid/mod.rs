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
    colors[0] = (0, 0, 0);         // Black
    colors[1] = (205, 49, 49);     // Red
    colors[2] = (13, 188, 121);    // Green
    colors[3] = (229, 229, 16);    // Yellow
    colors[4] = (36, 114, 200);    // Blue
    colors[5] = (188, 63, 188);    // Magenta
    colors[6] = (17, 168, 205);    // Cyan
    colors[7] = (229, 229, 229);   // White
    
    // Bright 8 colors (8-15)
    colors[8] = (102, 102, 102);   // Bright Black
    colors[9] = (241, 76, 76);     // Bright Red
    colors[10] = (35, 209, 139);   // Bright Green
    colors[11] = (245, 245, 67);   // Bright Yellow
    colors[12] = (59, 142, 234);   // Bright Blue
    colors[13] = (214, 112, 214);  // Bright Magenta
    colors[14] = (41, 184, 219);   // Bright Cyan
    colors[15] = (255, 255, 255);  // Bright White
    
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
            default_fg: (229, 229, 229), // Light gray
            default_bg: (30, 30, 46),   // Dark background
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attrs {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub blink_rapid: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
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
    pub hyperlink_id: Option<u32>,
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
            hyperlink_id: None,
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
    /// Legacy X10 encoding (default).
    #[default]
    X10,
    /// SGR extended encoding (DECSET 1006) — supports >223 columns.
    SGR,
}

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

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

    // Scrollback buffer (primary screen only)
    pub scrollback: Vec<Vec<Cell>>,
    pub scrollback_offset: usize, // 0 = no scroll, >0 = lines scrolled up
    /// Fractional scroll offset for smooth scrolling (0.0 to 1.0)
    pub scroll_fraction: f32,
    /// Maximum number of lines in scrollback buffer
    pub scrollback_capacity: usize,

    // DECTCEM cursor visibility
    pub cursor_visible: bool,

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

    // Bell state
    pub bell_pending: bool,

    // Hyperlink support (OSC 8)
    hyperlinks: std::collections::HashMap<u32, String>,
    active_hyperlink_id: Option<u32>,
    next_hyperlink_id: u32,
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
            scrollback: Vec::new(),
            scrollback_offset: 0,
            scroll_fraction: 0.0,
            scrollback_capacity: scrollback,
            cursor_visible: true,
            alt_saved_cursor: Cursor::default(),
            mouse_mode: MouseMode::None,
            mouse_encoding: MouseEncoding::X10,
            mouse_position: Cursor::default(),
            palette: ColorPalette::default(),
            bracketed_paste: false,
            bell_pending: false,
            hyperlinks: std::collections::HashMap::new(),
            active_hyperlink_id: None,
            next_hyperlink_id: 1,
        }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    fn cells(&self) -> &[Cell] {
        if self.alt_active { &self.cells_alt } else { &self.cells_primary }
    }

    fn cells_mut(&mut self) -> &mut Vec<Cell> {
        if self.alt_active { &mut self.cells_alt } else { &mut self.cells_primary }
    }

    fn idx(&self, col: usize, row: usize) -> usize {
        row * self.cols + col
    }

    pub fn cell(&self, col: usize, row: usize) -> &Cell {
        let i = self.idx(col, row);
        &self.cells()[i]
    }

    fn cell_mut(&mut self, col: usize, row: usize) -> &mut Cell {
        let i = self.idx(col, row);
        &mut self.cells_mut()[i]
    }

    // -----------------------------------------------------------------------
    // Grid resize — reflows content (best-effort, preserves last `rows` lines)
    // -----------------------------------------------------------------------

    pub fn resize(&mut self, size: WinSize) {
        let new_cols = size.cols as usize;
        let new_rows = size.rows as usize;

        let mut new_primary = vec![Cell::default(); new_cols * new_rows];
        let copy_rows = self.rows.min(new_rows);
        let copy_cols = self.cols.min(new_cols);

        for r in 0..copy_rows {
            for c in 0..copy_cols {
                let src = r * self.cols + c;
                let dst = r * new_cols + c;
                new_primary[dst] = self.cells_primary[src].clone();
            }
        }

        self.cols = new_cols;
        self.rows = new_rows;
        self.cells_primary = new_primary;
        self.cells_alt = vec![Cell::default(); new_cols * new_rows];
        self.scroll_top = 0;
        self.scroll_bottom = new_rows - 1;
        self.cursor.col = self.cursor.col.min(new_cols.saturating_sub(1));
        self.cursor.row = self.cursor.row.min(new_rows.saturating_sub(1));
        
        // Mark all cells dirty since the grid has been resized
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
            }
        }
        s
    }

    /// Get all visible lines as strings
    pub fn all_lines(&self) -> Vec<String> {
        (0..self.rows).map(|r| self.line_to_string(r)).collect()
    }

    /// Get scrollback lines (if any) + visible lines
    pub fn all_lines_with_scrollback(&self) -> Vec<String> {
        let mut lines = Vec::new();
        // Add scrollback lines if we have them
        // For now, just return visible lines
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
        let width = UnicodeWidthChar::width(ch).unwrap_or(1);

        // Wrap if at end of line
        if self.cursor.col + width > self.cols {
            self.cursor.col = 0;
            self.cursor.row += 1;
        }

        if self.cursor.row > self.scroll_bottom {
            self.scroll_up(1);
            self.cursor.row = self.scroll_bottom;
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

    // -----------------------------------------------------------------------
    // Scrolling
    // -----------------------------------------------------------------------

     fn scroll_up(&mut self, n: usize) {
        // Push top line(s) into scrollback (primary screen only)
        if !self.alt_active {
            for r in self.scroll_top..self.scroll_top + n {
                if r < self.rows {
                    let row: Vec<Cell> = (0..self.cols)
                        .map(|c| self.cells_primary[r * self.cols + c].clone())
                        .collect();
                    self.scrollback.push(row);
                }
            }
            // Enforce scrollback limit
            while self.scrollback.len() > self.scrollback_capacity {
                self.scrollback.remove(0);
            }
        }

        let top = self.scroll_top;
        let bot = self.scroll_bottom;
        let cols = self.cols;
        let cells = self.cells_mut();

        for _ in 0..n {
            for r in top..bot {
                for c in 0..cols {
                    cells[r * cols + c] = cells[(r + 1) * cols + c].clone();
                }
            }
            // Clear bottom line
            for c in 0..cols {
                cells[bot * cols + c] = Cell::default();
            }
        }
        
        // Mark all visible cells dirty after scroll
        for r in top..=bot {
            for c in 0..cols {
                self.mark_dirty(c, r);
            }
        }
    }

    fn scroll_down(&mut self, n: usize) {
        let top = self.scroll_top;
        let bot = self.scroll_bottom;
        let cols = self.cols;
        let cells = self.cells_mut();

        for _ in 0..n {
            for r in (top + 1..=bot).rev() {
                for c in 0..cols {
                    cells[r * cols + c] = cells[(r - 1) * cols + c].clone();
                }
            }
            // Clear top line
            for c in 0..cols {
                cells[top * cols + c] = Cell::default();
            }
        }
        
        // Mark all visible cells dirty after scroll
        for r in top..=bot {
            for c in 0..cols {
                self.mark_dirty(c, r);
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
                // Entire screen (3 also clears scrollback — ignored here)
                (0, self.cols * self.rows)
            }
            _ => return,
        };

        let fg = self.active_fg;
        let bg = self.active_bg;
        let cells = self.cells_mut();
        for i in start..end {
            cells[i] = Cell { fg, bg, dirty: true, ..Cell::default() };
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        let cursor = self.cursor;
        let (start_col, end_col) = match mode {
            0 => (cursor.col, self.cols),    // cursor to end of line
            1 => (0, cursor.col + 1),        // start to cursor
            2 => (0, self.cols),             // entire line
            _ => return,
        };

        let fg = self.active_fg;
        let bg = self.active_bg;
        let row = cursor.row;
        let cols = self.cols;
        let cells = self.cells_mut();
        for c in start_col..end_col {
            cells[row * cols + c] = Cell { fg, bg, dirty: true, ..Cell::default() };
        }
    }

    // -----------------------------------------------------------------------
    // SGR — Select Graphic Rendition
    // -----------------------------------------------------------------------

    fn apply_sgr(&mut self, params: &[Vec<u16>]) {
        let mut i = 0;
        // Flatten to a simple iterator over the primary parameter values
        let flat: Vec<u16> = params.iter().map(|p| p.first().copied().unwrap_or(0)).collect();

        while i < flat.len() {
            match flat[i] {
                0 => {
                    self.active_fg = Color::Default;
                    self.active_bg = Color::Default;
                    self.active_attrs = Attrs::default();
                }
                1 => self.active_attrs.bold = true,
                2 => self.active_attrs.dim = true,
                3 => self.active_attrs.italic = true,
                4 => self.active_attrs.underline = true,
                5 => self.active_attrs.blink = true,
                6 => self.active_attrs.blink_rapid = true,
                7 => self.active_attrs.inverse = true,
                8 => self.active_attrs.invisible = true,
                9 => self.active_attrs.strikethrough = true,
                22 => { self.active_attrs.bold = false; self.active_attrs.dim = false; }
                23 => self.active_attrs.italic = false,
                24 => self.active_attrs.underline = false,
                25 => self.active_attrs.blink = false,
                27 => self.active_attrs.inverse = false,
                28 => self.active_attrs.invisible = false,
                29 => self.active_attrs.strikethrough = false,
                // Standard foreground colors
                30..=37 => self.active_fg = Color::Indexed(flat[i] as u8 - 30),
                38 => {
                    // Extended foreground color
                    if i + 1 < flat.len() {
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
                }
                39 => self.active_fg = Color::Default,
                // Standard background colors
                40..=47 => self.active_bg = Color::Indexed(flat[i] as u8 - 40),
                48 => {
                    // Extended background color
                    if i + 1 < flat.len() {
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
                }
                49 => self.active_bg = Color::Default,
                // Bright foreground (xterm extension)
                90..=97 => self.active_fg = Color::Indexed(flat[i] as u8 - 90 + 8),
                // Bright background (xterm extension)
                100..=107 => self.active_bg = Color::Indexed(flat[i] as u8 - 100 + 8),
                _ => {}
            }
            i += 1;
        }
    }

    // -----------------------------------------------------------------------
    // Perform impl helper — CSI dispatch
    // -----------------------------------------------------------------------

    fn handle_csi(
        &mut self,
        params: &[Vec<u16>],
        intermediates: &[u8],
        final_byte: u8,
    ) {
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
            // CUP / HVP — cursor position
            (_, b'H') | (_, b'f') => {
                self.cursor.row = p0.saturating_sub(1).min(self.rows - 1);
                self.cursor.col = p1.saturating_sub(1).min(self.cols - 1);
            }
            // ED — erase in display
            (_, b'J') => self.erase_in_display(p0 as u16),
            // EL — erase in line
            (_, b'K') => self.erase_in_line(p0 as u16),
            // IL — insert lines
            (_, b'L') => {
                let n = p0.max(1);
                self.scroll_down(n);
            }
            // DL — delete lines
            (_, b'M') => {
                let n = p0.max(1);
                self.scroll_up(n);
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
            // SU — scroll up
            (_, b'S') => self.scroll_up(p0.max(1)),
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
                    cells[row * cells_len + c] = Cell { fg, bg, dirty: true, ..Cell::default() };
                }
            }
            // VPA — vertical position absolute
            (_, b'd') => {
                self.cursor.row = p0.saturating_sub(1).min(self.rows - 1);
            }
            // SGR — select graphic rendition
            (_, b'm') => self.apply_sgr(params),
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
            // DECSC — save cursor
            (b"", b'7') | (b"", b's') => {
                self.saved_cursor = SavedCursor {
                    cursor: self.cursor,
                    fg: self.active_fg,
                    bg: self.active_bg,
                    attrs: self.active_attrs,
                };
            }
            // DECRC — restore cursor
            (b"", b'8') | (b"", b'u') => {
                self.cursor = self.saved_cursor.cursor;
                self.active_fg = self.saved_cursor.fg;
                self.active_bg = self.saved_cursor.bg;
                self.active_attrs = self.saved_cursor.attrs;
            }
            // Private mode set/reset (?h / ?l)
            (b"?", b'h') | (b"?", b'l') => {
                let set = final_byte == b'h';
                for p in params {
                    let n = p.first().copied().unwrap_or(0);
                    match n {
                        25 => { self.cursor_visible = set; }
                        1000 => {
                            // Normal mouse tracking
                            self.mouse_mode = if set { MouseMode::Normal } else { MouseMode::None };
                        }
                        1002 => {
                            // Button-event mouse tracking
                            self.mouse_mode = if set { MouseMode::ButtonEvent } else { MouseMode::None };
                        }
                        1003 => {
                            // Any-event mouse tracking
                            self.mouse_mode = if set { MouseMode::AnyEvent } else { MouseMode::None };
                        }
                        1006 => {
                            // SGR extended mouse encoding
                            self.mouse_encoding = if set { MouseEncoding::SGR } else { MouseEncoding::X10 };
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
                            log::debug!("Bracketed paste: {}", if set { "enabled" } else { "disabled" });
                        }
                        _ => {}
                    }
                }
            }
            _ => {
                log::trace!(
                    "unhandled CSI: intermediates={:?} final={:?} params={:?}",
                    intermediates, final_byte as char, params
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // OSC dispatch — Operating System Command handling
    // -----------------------------------------------------------------------

    fn handle_osc(&mut self, params: &[Vec<u8>]) {
        // Get the OSC command (first parameter) - parse as number from ASCII bytes
        let cmd = params.first()
            .and_then(|p| std::str::from_utf8(p).ok())
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0);

        match cmd {
            // OSC 0 / 2 — Set window title
            0 | 2 => {
                // Title is everything after the command, joined by semicolons
                if params.len() > 1 {
                    let title_parts: Vec<&[u8]> = params[1..].iter().map(|p| p.as_slice()).collect();
                    let title = title_parts.join(&b';');
                    if let Ok(title_str) = std::str::from_utf8(&title) {
                        self.palette.set_title(title_str);
                    }
                }
            }
            // OSC 4 — Set/query color palette: OSC 4 ; index ; spec
            4 => {
                if params.len() >= 3 {
                    if let Ok(index_str) = std::str::from_utf8(&params[1]) {
                        if let Ok(index) = index_str.parse::<u8>() {
                            let spec = &params[2];
                            if let Ok(spec_str) = std::str::from_utf8(spec) {
                                if let Some((r, g, b)) = parse_color_spec(spec_str) {
                                    self.palette.set_color(index, r, g, b);
                                    log::debug!("OSC 4: Set color {} to ({}, {}, {})", index, r, g, b);
                                }
                            }
                        }
                    }
                }
            }
            // OSC 10 — Set/query foreground color: OSC 10 ; spec
            10 => {
                if params.len() >= 2 {
                    if let Ok(spec_str) = std::str::from_utf8(&params[1]) {
                        if let Some((r, g, b)) = parse_color_spec(spec_str) {
                            self.palette.set_fg(r, g, b);
                            log::debug!("OSC 10: Set foreground to ({}, {}, {})", r, g, b);
                        }
                    }
                }
            }
            // OSC 11 — Set/query background color: OSC 11 ; spec
            11 => {
                if params.len() >= 2 {
                    if let Ok(spec_str) = std::str::from_utf8(&params[1]) {
                        if let Some((r, g, b)) = parse_color_spec(spec_str) {
                            self.palette.set_bg(r, g, b);
                            log::debug!("OSC 11: Set background to ({}, {}, {})", r, g, b);
                        }
                    }
                }
            }
            // OSC 12 — Set/query cursor color: OSC 12 ; spec
            12 => {
                if params.len() >= 2 {
                    if let Ok(spec_str) = std::str::from_utf8(&params[1]) {
                        if let Some((r, g, b)) = parse_color_spec(spec_str) {
                            self.palette.set_cursor_color(r, g, b);
                            log::debug!("OSC 12: Set cursor color to ({}, {}, {})", r, g, b);
                        }
                    }
                }
            }
            // OSC 52 — Clipboard (handled by clipboard module)
            52 => {
                log::debug!("OSC 52 clipboard request");
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
                    let id = params_str.split(';')
                        .find(|p| p.starts_with("id="))
                        .and_then(|p| p[3..].parse::<u32>().ok());

                    if uri.is_empty() {
                        // Empty URI = end hyperlink
                        self.active_hyperlink_id = None;
                        log::debug!("OSC 8: End hyperlink");
                    } else {
                        // Start hyperlink
                        let link_id = id.unwrap_or(self.next_hyperlink_id);
                        self.hyperlinks.insert(link_id, uri.to_string());
                        self.active_hyperlink_id = Some(link_id);
                        if id.is_none() {
                            self.next_hyperlink_id = link_id + 1;
                        }
                        log::debug!("OSC 8: Start hyperlink id={}: {}", link_id, uri);
                    }
                } else if params.len() == 1 {
                    // Just OSC 8 ; = end hyperlink
                    self.active_hyperlink_id = None;
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
        cell.hyperlink_id.and_then(|id| self.hyperlinks.get(&id).map(|s| s.as_str()))
    }

    /// Get the hyperlink URL for a cell
    pub fn get_cell_hyperlink(&self, cell: &Cell) -> Option<&str> {
        cell.hyperlink_id.and_then(|id| self.hyperlinks.get(&id).map(|s| s.as_str()))
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
                    // HT — horizontal tab (advance to next 8-stop)
                    let next = (self.cursor.col / 8 + 1) * 8;
                    self.cursor.col = next.min(self.cols - 1);
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
                _ => {}
            },

            Action::CsiDispatch { params, intermediates, ignore: _, final_byte } => {
                self.handle_csi(&params, &intermediates, final_byte);
            }

            Action::OscDispatch { params } => {
                self.handle_osc(&params);
            }

            Action::EscDispatch { intermediates: _, ignore: _, final_byte } => {
                match final_byte {
                    b'7' => {
                        // DECSC
                        self.saved_cursor = SavedCursor {
                            cursor: self.cursor,
                            fg: self.active_fg,
                            bg: self.active_bg,
                            attrs: self.active_attrs,
                        };
                    }
                    b'8' => {
                        // DECRC
                        self.cursor = self.saved_cursor.cursor;
                        self.active_fg = self.saved_cursor.fg;
                        self.active_bg = self.saved_cursor.bg;
                        self.active_attrs = self.saved_cursor.attrs;
                    }
                    b'M' => {
                        // RI — reverse index (scroll down)
                        if self.cursor.row == self.scroll_top {
                            self.scroll_down(1);
                        } else {
                            self.cursor.row = self.cursor.row.saturating_sub(1);
                        }
                    }
                    b'c' => {
                        // RIS — full reset
                        *self = Grid::new(WinSize {
                            cols: self.cols as u16,
                            rows: self.rows as u16,
                        }, self.scrollback_capacity);
                        self.cursor_visible = true;
                    }
                    _ => {}
                }
            }

            Action::Hook { .. } | Action::Put(_) | Action::Unhook => {
                // DCS passthrough — ignore for now
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(g.cell(0, 0).attrs.bold);
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
        assert!(g.cell(0, 0).attrs.bold);
        assert_eq!(g.cell(0, 0).fg, Color::Indexed(1));
        // After reset, 'X' should have default attributes
        assert!(!g.cell(2, 0).attrs.bold);
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
        assert!(g.cell(0, 0).attrs.bold);
        assert_eq!(g.cell(0, 0).ch, 'B');

        feed(&mut g, b"\x1bc"); // RIS
        assert_eq!(g.cell(0, 0).ch, ' ');
        assert!(!g.cell(0, 0).attrs.bold);
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
}
