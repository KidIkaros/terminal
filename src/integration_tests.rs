//! Integration tests for the terminal emulator.

#[cfg(test)]
mod integration_tests {
    use crate::grid::{Grid, WinSize};
    use crate::parser::Parser;

    /// Test basic terminal output
    #[test]
    fn test_basic_output() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Print "Hello"
        for byte in b"Hello" {
            parser.advance(&mut grid, *byte);
        }

        // Check that "Hello" is in the grid
        let line = grid.line_to_string(0);
        assert!(line.contains("Hello"));
    }

    /// Test cursor movement
    #[test]
    fn test_cursor_movement() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Move cursor to position 5, 3
        parser.advance(&mut grid, b'\x1b');
        parser.advance(&mut grid, b'[');
        parser.advance(&mut grid, b'4');
        parser.advance(&mut grid, b';');
        parser.advance(&mut grid, b'6');
        parser.advance(&mut grid, b'H');

        assert_eq!(grid.cursor.row, 3);
        assert_eq!(grid.cursor.col, 5);
    }

    /// Test SGR attributes
    #[test]
    fn test_sgr_bold() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Enable bold
        parser.advance(&mut grid, b'\x1b');
        parser.advance(&mut grid, b'[');
        parser.advance(&mut grid, b'1');
        parser.advance(&mut grid, b'm');

        // Print a character
        parser.advance(&mut grid, b'A');

        let cell = grid.cell(0, 0);
        assert!(cell.attrs.bold);
    }

    /// Test colors
    #[test]
    fn test_sgr_color() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Set red foreground
        parser.advance(&mut grid, b'\x1b');
        parser.advance(&mut grid, b'[');
        parser.advance(&mut grid, b'3');
        parser.advance(&mut grid, b'1');
        parser.advance(&mut grid, b'm');

        // Print a character
        parser.advance(&mut grid, b'B');

        let cell = grid.cell(0, 0);
        // Check that foreground is set (not default)
        assert!(matches!(cell.fg, crate::grid::Color::Indexed(1)));
    }

    /// Test scrollback
    #[test]
    fn test_scrollback() {
        let size = WinSize { cols: 20, rows: 5 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Fill the screen and overflow
        for i in 0..10 {
            let line = format!("Line {}\r\n", i);
            for byte in line.bytes() {
                parser.advance(&mut grid, byte);
            }
        }

        // Should have some scrollback
        assert!(!grid.scrollback.is_empty());
    }

    /// Test alternate screen
    #[test]
    fn test_alternate_screen() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Print to primary screen
        for byte in b"Primary" {
            parser.advance(&mut grid, *byte);
        }

        // Switch to alternate screen
        parser.advance(&mut grid, b'\x1b');
        parser.advance(&mut grid, b'[');
        parser.advance(&mut grid, b'?');
        parser.advance(&mut grid, b'1');
        parser.advance(&mut grid, b'0');
        parser.advance(&mut grid, b'4');
        parser.advance(&mut grid, b'9');
        parser.advance(&mut grid, b'h');

        // Print to alternate screen
        for byte in b"Alternate" {
            parser.advance(&mut grid, *byte);
        }

        let line = grid.line_to_string(0);
        assert!(line.contains("Alternate"));

        // Switch back to primary screen
        parser.advance(&mut grid, b'\x1b');
        parser.advance(&mut grid, b'[');
        parser.advance(&mut grid, b'?');
        parser.advance(&mut grid, b'1');
        parser.advance(&mut grid, b'0');
        parser.advance(&mut grid, b'4');
        parser.advance(&mut grid, b'9');
        parser.advance(&mut grid, b'l');

        let line = grid.line_to_string(0);
        assert!(line.contains("Primary"));
    }

    /// Test line wrapping
    #[test]
    fn test_line_wrapping() {
        let size = WinSize { cols: 10, rows: 5 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Print more characters than cols
        for byte in b"Hello World" {
            parser.advance(&mut grid, *byte);
        }

        // Should wrap to next line
        assert!(grid.cursor.row >= 1);
    }

    /// Test window title (OSC 0/2)
    #[test]
    fn test_window_title() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Set window title
        let seq = b"\x1b]0;My Title\x07";
        for byte in seq {
            parser.advance(&mut grid, *byte);
        }

        assert_eq!(grid.palette.title, "My Title");
    }

    /// Test bracketed paste
    #[test]
    fn test_bracketed_paste() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Enable bracketed paste
        parser.advance(&mut grid, b'\x1b');
        parser.advance(&mut grid, b'[');
        parser.advance(&mut grid, b'?');
        parser.advance(&mut grid, b'2');
        parser.advance(&mut grid, b'0');
        parser.advance(&mut grid, b'0');
        parser.advance(&mut grid, b'4');
        parser.advance(&mut grid, b'h');

        assert!(grid.bracketed_paste);

        // Disable bracketed paste
        parser.advance(&mut grid, b'\x1b');
        parser.advance(&mut grid, b'[');
        parser.advance(&mut grid, b'?');
        parser.advance(&mut grid, b'2');
        parser.advance(&mut grid, b'0');
        parser.advance(&mut grid, b'0');
        parser.advance(&mut grid, b'4');
        parser.advance(&mut grid, b'l');

        assert!(!grid.bracketed_paste);
    }

    /// Test bell
    #[test]
    fn test_bell() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Send BEL
        parser.advance(&mut grid, b'\x07');

        assert!(grid.bell_pending);
    }

    /// Test tab handling
    #[test]
    fn test_tab() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Print a tab
        parser.advance(&mut grid, b'\t');

        // Tab should move cursor to next tab stop (typically every 8 columns)
        assert!(grid.cursor.col > 0);
    }

    /// Test backspace
    #[test]
    fn test_backspace() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Print a character
        parser.advance(&mut grid, b'A');
        assert_eq!(grid.cursor.col, 1);

        // Backspace (0x08) - not DEL (0x7f)
        parser.advance(&mut grid, b'\x08');
        assert_eq!(grid.cursor.col, 0);
    }

    /// Test carriage return
    #[test]
    fn test_carriage_return() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Move to column 10
        for _ in 0..10 {
            parser.advance(&mut grid, b'A');
        }
        assert_eq!(grid.cursor.col, 10);

        // Carriage return
        parser.advance(&mut grid, b'\r');
        assert_eq!(grid.cursor.col, 0);
    }

    /// Test line feed
    #[test]
    fn test_line_feed() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        assert_eq!(grid.cursor.row, 0);

        // Line feed
        parser.advance(&mut grid, b'\n');
        assert_eq!(grid.cursor.row, 1);
    }

    /// Test color palette (OSC 4)
    #[test]
    fn test_color_palette() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Set color 1 (red) to blue using #RRGGBB format
        let seq = b"\x1b]4;1;#0000FF\x07";
        for byte in seq {
            parser.advance(&mut grid, *byte);
        }

        let color = grid.palette.get_color(1);
        assert_eq!(color, Some((0, 0, 255)));
    }

    /// Test cursor visibility (DECTCEM)
    #[test]
    fn test_cursor_visibility() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Hide cursor
        parser.advance(&mut grid, b'\x1b');
        parser.advance(&mut grid, b'[');
        parser.advance(&mut grid, b'?');
        parser.advance(&mut grid, b'2');
        parser.advance(&mut grid, b'5');
        parser.advance(&mut grid, b'l');

        assert!(!grid.cursor_visible);

        // Show cursor
        parser.advance(&mut grid, b'\x1b');
        parser.advance(&mut grid, b'[');
        parser.advance(&mut grid, b'?');
        parser.advance(&mut grid, b'2');
        parser.advance(&mut grid, b'5');
        parser.advance(&mut grid, b'h');

        assert!(grid.cursor_visible);
    }

    /// Test mouse mode
    #[test]
    fn test_mouse_mode() {
        use crate::grid::MouseMode;

        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Enable normal mouse tracking
        parser.advance(&mut grid, b'\x1b');
        parser.advance(&mut grid, b'[');
        parser.advance(&mut grid, b'?');
        parser.advance(&mut grid, b'1');
        parser.advance(&mut grid, b'0');
        parser.advance(&mut grid, b'0');
        parser.advance(&mut grid, b'0');
        parser.advance(&mut grid, b'h');

        assert_eq!(grid.mouse_mode, MouseMode::Normal);
    }

    /// Test selection
    #[test]
    fn test_selection() {
        use crate::selection::{Selection, SelectionMode};

        let mut sel = Selection::new();
        sel.start_selection(0, 0, SelectionMode::Char);
        sel.update(0, 5);
        sel.end_selection();

        assert!(sel.active);
        assert!(sel.contains(0, 0));
        assert!(sel.contains(0, 5));
        assert!(!sel.contains(0, 6));
    }

    /// Test search
    #[test]
    fn test_search() {
        use crate::search::SearchState;

        let mut search = SearchState::new();
        search.activate();
        search.update_query("test");

        let lines = vec![
            "this is a test".to_string(),
            "another test here".to_string(),
        ];

        search.search(&lines);
        assert_eq!(search.match_count(), 2);
    }

    /// Test tabs
    #[test]
    fn test_tabs() {
        use crate::tabs::TabManager;

        let size = WinSize { cols: 80, rows: 24 };
        let mut manager = TabManager::new(size);

        assert_eq!(manager.len(), 1);

        manager.new_tab();
        assert_eq!(manager.len(), 2);

        manager.switch_to(1);
        assert_eq!(manager.active_index(), 1);

        manager.prev();
        assert_eq!(manager.active_index(), 0);
    }

    /// Test config loading
    #[test]
    fn test_config() {
        use crate::config::Config;

        let config = Config::load();
        // Should have default values
        assert!(config.font.size > 0.0);
        assert!(config.window.cols > 0);
        assert!(config.window.rows > 0);
    }

    /// Test theme system
    #[test]
    fn test_themes() {
        use crate::theme::Theme;

        let names = Theme::theme_names();
        assert!(!names.is_empty());

        let theme = Theme::find("Catppuccin Mocha");
        assert!(theme.is_some());
    }

    /// Test hyperlinks (OSC 8)
    #[test]
    fn test_hyperlinks() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut grid = Grid::new(size);
        let mut parser = Parser::new();

        // Start hyperlink
        let seq = b"\x1b]8;id=1;https://example.com\x07";
        for byte in seq {
            parser.advance(&mut grid, *byte);
        }

        // Print some text
        for byte in b"Link" {
            parser.advance(&mut grid, *byte);
        }

        // End hyperlink
        parser.advance(&mut grid, b'\x1b');
        parser.advance(&mut grid, b']');
        parser.advance(&mut grid, b'8');
        parser.advance(&mut grid, b';');
        parser.advance(&mut grid, b';');
        parser.advance(&mut grid, b'\x07');

        // Check that the cells have hyperlink IDs
        let cell = grid.cell(0, 0);
        assert!(cell.hyperlink_id.is_some());
    }

    /// Test OSC 52 (clipboard)
    #[test]
    fn test_osc52() {
        use crate::clipboard::ClipboardManager;

        let mut clipboard = ClipboardManager::new();

        // Test copy
        clipboard.copy("test text");
        assert_eq!(clipboard.paste(), Some("test text".to_string()));
    }

    /// Test mouse event encoding
    #[test]
    fn test_mouse_encoding() {
        use crate::grid::MouseEncoding;
        use crate::mouse::{MouseEvent, MouseEventType, MouseButton};

        let event = MouseEvent {
            button: MouseButton::Left,
            event_type: MouseEventType::Press,
            col: 10,
            row: 5,
            shift: false,
            ctrl: false,
            alt: false,
        };

        let x10 = event.encode(MouseEncoding::X10);
        assert!(!x10.is_empty());

        let sgr = event.encode(MouseEncoding::SGR);
        assert!(!sgr.is_empty());
    }
}
