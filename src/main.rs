mod clipboard;
mod config;
mod grid;
mod image;
mod ligatures;
mod mouse;
mod parser;
mod pty;
mod render;
mod search;
mod selection;
mod tab_bar;
mod tabs;
mod theme;

#[cfg(test)]
mod integration_tests;

use std::sync::Arc;

use winit::{
    application::ApplicationHandler,
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, KeyEvent, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

use clipboard::ClipboardManager;
use config::Config;
use grid::{Grid, WinSize};
use mouse::MouseButton;
use parser::Parser;
use render::{font::GlyphAtlas, pipeline::TerminalPipeline};
use search::SearchState;
use selection::{Selection, SelectionMode};

// ---------------------------------------------------------------------------
// Application (winit ApplicationHandler)
// ---------------------------------------------------------------------------

/// All state lives on the main thread — no shared state / mutex needed.
struct App {
    // Created on first `resumed` event
    window: Option<Arc<Window>>,
    pipeline: Option<TerminalPipeline>,
    atlas: Option<GlyphAtlas>,

    // Terminal state — parser + grid both single-threaded
    grid: Grid,
    parser: Parser,

    // PTY I/O
    pty_writer: Option<pty::PtyWriter>,
    pty_rx: Option<crossbeam_channel::Receiver<Vec<u8>>>,

    // Clipboard
    clipboard: ClipboardManager,
    modifiers: ModifiersState,

    // Mouse state
    mouse_position: (f64, f64),
    mouse_button_pressed: Option<MouseButton>,

    // Search state
    search: SearchState,

    // Selection state
    selection: Selection,

    // Cursor blink state
    cursor_visible: bool,
    last_cursor_blink: std::time::Instant,

    // Configuration
    config: Config,

    size: WinSize,
}

impl App {
    fn new() -> Self {
        let config = Config::load();
        let size = WinSize {
            cols: config.window.cols,
            rows: config.window.rows,
        };
        App {
            window: None,
            pipeline: None,
            atlas: None,
            grid: Grid::new(size),
            parser: Parser::new(),
            pty_writer: None,
            pty_rx: None,
            clipboard: ClipboardManager::new(),
            modifiers: ModifiersState::default(),
            mouse_position: (0.0, 0.0),
            mouse_button_pressed: None,
            search: SearchState::new(),
            selection: Selection::new(),
            cursor_visible: true,
            last_cursor_blink: std::time::Instant::now(),
            config,
            size,
        }
    }

    /// Drain all pending bytes from the PTY channel, parse them, update grid.
    fn drain_pty(&mut self) {
        // Collect chunks first so we can split the borrow cleanly
        let mut pending: Vec<Vec<u8>> = Vec::new();
        if let Some(rx) = &self.pty_rx {
            while let Ok(chunk) = rx.try_recv() {
                pending.push(chunk);
            }
        }
        for chunk in pending {
            for byte in chunk {
                self.parser.advance(&mut self.grid, byte);
            }
        }
    }

    fn write_to_pty(&self, data: &[u8]) {
        if let Some(w) = &self.pty_writer {
            w.write(data);
        }
    }

    /// Handle keyboard shortcuts (Ctrl+Shift+C, Ctrl+Shift+V, etc.)
    /// Returns true if the key was handled as a shortcut.
    fn handle_shortcut(&mut self, key: &Key, text: &Option<String>) -> bool {
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();

        // Handle search mode separately
        if self.search.active {
            return self.handle_search_key(key);
        }

        match key {
            // Ctrl+Shift+C — Copy selection to clipboard
            Key::Character(s) if s.as_str() == "C" && ctrl && shift => {
                if self.selection.active {
                    let lines = self.grid.all_lines();
                    let text = self.selection.extract_text(&lines, self.size.cols as usize);
                    if !text.is_empty() {
                        self.clipboard.copy(&text);
                        log::debug!("Copy: Ctrl+Shift+C ({} chars)", text.len());
                    }
                } else {
                    log::debug!("Copy: Ctrl+Shift+C (no selection)");
                }
                true
            }
            // Ctrl+Shift+V — Paste from clipboard
            Key::Character(s) if s.as_str() == "V" && ctrl && shift => {
                if let Some(text) = self.clipboard.paste() {
                    // Wrap in bracketed paste mode if enabled
                    if self.grid.bracketed_paste {
                        let mut data = Vec::new();
                        data.extend_from_slice(b"\x1b[200~");
                        data.extend_from_slice(text.as_bytes());
                        data.extend_from_slice(b"\x1b[201~");
                        self.write_to_pty(&data);
                    } else {
                        let bytes = text.into_bytes();
                        self.write_to_pty(&bytes);
                    }
                }
                log::debug!("Paste: Ctrl+Shift+V");
                true
            }
            // Ctrl+Shift+F — Open search bar
            Key::Character(s) if s.as_str() == "F" && ctrl && shift => {
                self.search.activate();
                log::debug!("Search: Ctrl+Shift+F");
                true
            }
            // Ctrl+R — Reverse search
            Key::Character(s) if s.as_str() == "r" && ctrl && !shift => {
                self.search.activate_reverse();
                log::debug!("Reverse Search: Ctrl+R");
                true
            }
            // Ctrl+Shift+T — New tab (TODO: full multi-PTY support)
            Key::Character(s) if s.as_str() == "T" && ctrl && shift => {
                log::debug!("New Tab: Ctrl+Shift+T (not fully implemented)");
                true
            }
            // Ctrl+Shift+W — Close tab (TODO: full multi-PTY support)
            Key::Character(s) if s.as_str() == "W" && ctrl && shift => {
                log::debug!("Close Tab: Ctrl+Shift+W (not fully implemented)");
                true
            }
            // Ctrl+PageDown — Next tab (TODO)
            Key::Named(NamedKey::PageDown) if ctrl && !shift => {
                log::debug!("Next Tab: Ctrl+PageDown (not fully implemented)");
                true
            }
            // Ctrl+PageUp — Previous tab (TODO)
            Key::Named(NamedKey::PageUp) if ctrl && !shift => {
                log::debug!("Previous Tab: Ctrl+PageUp (not fully implemented)");
                true
            }
            // Ctrl+Shift+A — Select all
            Key::Character(s) if s.as_str() == "A" && ctrl && shift => {
                self.selection.start_selection(0, 0, SelectionMode::Char);
                self.selection.update(self.size.rows as usize - 1, self.size.cols as usize - 1);
                self.selection.end_selection();
                log::debug!("Select All: Ctrl+Shift+A");
                true
            }
            // Ctrl+C — Send SIGINT (Ctrl+C is already handled by terminal)
            Key::Character(s) if s.as_str() == "c" && ctrl && !shift => {
                // Let Ctrl+C pass through to terminal for SIGINT
                false
            }
            // Ctrl+Z — Send SIGTSTP (Ctrl+Z is already handled by terminal)
            Key::Character(s) if s.as_str() == "z" && ctrl && !shift => {
                // Let Ctrl+Z pass through to terminal for SIGTSTP
                false
            }
            // Ctrl+D — Send EOF
            Key::Character(s) if s.as_str() == "d" && ctrl && !shift => {
                // Let Ctrl+D pass through to terminal for EOF
                false
            }
            // Ctrl+L — Clear screen
            Key::Character(s) if s.as_str() == "l" && ctrl && !shift => {
                // Let Ctrl+L pass through to terminal for clear screen
                false
            }
            // Ctrl+W — Delete word backward
            Key::Character(s) if s.as_str() == "w" && ctrl && !shift => {
                self.write_to_pty(&[0x17]);
                true
            }
            // Ctrl+U — Delete to beginning of line
            Key::Character(s) if s.as_str() == "u" && ctrl && !shift => {
                self.write_to_pty(&[0x15]);
                true
            }
            // Ctrl+K — Delete to end of line
            Key::Character(s) if s.as_str() == "k" && ctrl && !shift => {
                self.write_to_pty(&[0x0B]);
                true
            }
            // Ctrl+A — Beginning of line
            Key::Character(s) if s.as_str() == "a" && ctrl && !shift => {
                self.write_to_pty(&[0x01]);
                true
            }
            // Ctrl+E — End of line
            Key::Character(s) if s.as_str() == "e" && ctrl && !shift => {
                self.write_to_pty(&[0x05]);
                true
            }
            _ => false,
        }
    }

    /// Handle keyboard input while search mode is active
    fn handle_search_key(&mut self, key: &Key) -> bool {
        match key {
            // Escape — Close search
            Key::Named(NamedKey::Escape) => {
                self.search.deactivate();
                true
            }
            // Enter — Search for next match
            Key::Named(NamedKey::Enter) => {
                let lines = self.grid.all_lines();
                self.search.search(&lines);
                if let Some(m) = self.search.next() {
                    self.grid.mark_match_dirty(m.row, m.start_col, m.end_col);
                }
                true
            }
            // F3 or Ctrl+G — Find next
            Key::Named(NamedKey::F3) => {
                if let Some(m) = self.search.next() {
                    self.grid.mark_match_dirty(m.row, m.start_col, m.end_col);
                }
                true
            }
            Key::Character(s) if s.as_str() == "g" && self.modifiers.control_key() && !self.modifiers.shift_key() => {
                if let Some(m) = self.search.next() {
                    self.grid.mark_match_dirty(m.row, m.start_col, m.end_col);
                }
                true
            }
            // Shift+F3 or Ctrl+Shift+G — Find previous
            Key::Character(s) if s.as_str() == "g" && self.modifiers.control_key() && self.modifiers.shift_key() => {
                if let Some(m) = self.search.prev() {
                    self.grid.mark_match_dirty(m.row, m.start_col, m.end_col);
                }
                true
            }
            // Backspace — Remove last character from query
            Key::Named(NamedKey::Backspace) => {
                let new_len = self.search.query.len().saturating_sub(1);
                self.search.query.truncate(new_len);
                // Recompile and search
                let query = self.search.query.clone();
                self.search.update_query(&query);
                let lines = self.grid.all_lines();
                self.search.search(&lines);
                true
            }
            // Character input — Add to search query
            Key::Character(text) => {
                self.search.query.push_str(text);
                // Recompile and search
                let query = self.search.query.clone();
                self.search.update_query(&query);
                let lines = self.grid.all_lines();
                self.search.search(&lines);
                true
            }
            _ => false,
        }
    }

    /// Convert pixel coordinates to cell coordinates (1-based for CSI sequences).
    fn pixel_to_cell(&self, x: f64, y: f64) -> (u32, u32) {
        let cell_width = self.atlas.as_ref().map(|a| a.cell_width as f64).unwrap_or(8.0);
        let cell_height = self.atlas.as_ref().map(|a| a.cell_height as f64).unwrap_or(16.0);
        let col = (x / cell_width) as u32 + 1; // 1-based
        let row = (y / cell_height) as u32 + 1; // 1-based
        (col, row)
    }

    fn pixel_size(&self, atlas: &GlyphAtlas) -> (u32, u32) {
        (
            self.size.cols as u32 * atlas.cell_width,
            self.size.rows as u32 * atlas.cell_height,
        )
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let font_bytes = render::font::embedded_font();
        let atlas = GlyphAtlas::from_bytes(font_bytes, self.config.font.size);
        let (pw, ph) = self.pixel_size(&atlas);

        let attrs = Window::default_attributes()
            .with_title("terminal")
            .with_inner_size(PhysicalSize::new(pw, ph))
            .with_position(PhysicalPosition::new(100, 100))
            .with_resizable(true)
            .with_transparent(self.config.window.opacity < 1.0);

        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        self.window = Some(Arc::clone(&window));

        let pipeline = pollster::block_on(TerminalPipeline::new(Arc::clone(&window), &atlas, self.config.window.vsync));

        match pty::spawn_pty(self.size, &self.config.shell) {
            Ok((writer, _handle, rx)) => {
                self.pty_writer = Some(writer);
                self.pty_rx = Some(rx);
                // _handle is kept alive — Drop sends SIGHUP + waitpid on exit
            }
            Err(e) => {
                log::error!("PTY open failed: {e}");
                event_loop.exit();
                return;
            }
        }

        self.pipeline = Some(pipeline);
        self.atlas = Some(atlas);

        self.window.as_ref().unwrap().request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::Resized(size) => {
                if let Some(pipeline) = &mut self.pipeline {
                    pipeline.resize(size.width, size.height);
                }
                if let Some(atlas) = &self.atlas {
                    let new_cols = (size.width / atlas.cell_width.max(1)) as u16;
                    let new_rows = (size.height / atlas.cell_height.max(1)) as u16;
                    if new_cols > 0 && new_rows > 0 {
                        self.size = WinSize { cols: new_cols, rows: new_rows };
                        self.grid.resize(self.size);
                    }
                }
                if let Some(w) = &self.pty_writer {
                    w.resize(self.size);
                }
            }

            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_position = (position.x, position.y);
                
                // Report motion if mouse tracking is active
                if let Some(atlas) = &self.atlas {
                    let mode = self.grid.mouse_mode;
                    let button_pressed = self.mouse_button_pressed.is_some();
                    
                    if mouse::should_report_motion(mode, button_pressed) {
                        let col = (position.x / atlas.cell_width.max(1) as f64) as u32 + 1;
                        let row = (position.y / atlas.cell_height.max(1) as f64) as u32 + 1;
                        
                        let event = mouse::MouseEvent {
                            button: self.mouse_button_pressed.unwrap_or(mouse::MouseButton::Left),
                            event_type: mouse::MouseEventType::Motion,
                            col,
                            row,
                            shift: self.modifiers.shift_key(),
                            ctrl: self.modifiers.control_key(),
                            alt: self.modifiers.alt_key(),
                        };
                        
                        let encoded = event.encode(self.grid.mouse_encoding);
                        self.write_to_pty(encoded.as_bytes());
                    } else if self.selection.selecting {
                        // Update selection during drag
                        let col = (position.x / atlas.cell_width.max(1) as f64) as usize;
                        let row = (position.y / atlas.cell_height.max(1) as f64) as usize;
                        self.selection.update(row, col);
                    }
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(atlas) = &self.atlas {
                    let mode = self.grid.mouse_mode;
                    
                    if mouse::is_mouse_tracking_active(mode) {
                        let winit_button = match button {
                            winit::event::MouseButton::Left => Some(mouse::MouseButton::Left),
                            winit::event::MouseButton::Right => Some(mouse::MouseButton::Right),
                            winit::event::MouseButton::Middle => Some(mouse::MouseButton::Middle),
                            _ => None,
                        };
                        
                        let event_type = match state {
                            ElementState::Pressed => {
                                self.mouse_button_pressed = winit_button;
                                mouse::MouseEventType::Press
                            }
                            ElementState::Released => {
                                self.mouse_button_pressed = None;
                                mouse::MouseEventType::Release
                            }
                        };
                        
                        let col = (self.mouse_position.0 / atlas.cell_width.max(1) as f64) as u32 + 1;
                        let row = (self.mouse_position.1 / atlas.cell_height.max(1) as f64) as u32 + 1;
                        
                        let event = mouse::MouseEvent {
                            button: winit_button.unwrap_or(mouse::MouseButton::Left),
                            event_type,
                            col,
                            row,
                            shift: self.modifiers.shift_key(),
                            ctrl: self.modifiers.control_key(),
                            alt: self.modifiers.alt_key(),
                        };
                        
                        let encoded = event.encode(self.grid.mouse_encoding);
                        self.write_to_pty(encoded.as_bytes());
                    } else if button == winit::event::MouseButton::Left {
                        // Handle selection when mouse tracking is not active
                        let col = (self.mouse_position.0 / atlas.cell_width.max(1) as f64) as usize;
                        let row = (self.mouse_position.1 / atlas.cell_height.max(1) as f64) as usize;
                        
                        match state {
                            ElementState::Pressed => {
                                // Check for hyperlink at click position
                                let hyperlink_url = self.grid.get_hyperlink_at(col, row).map(|s| s.to_string());
                                if let Some(url) = hyperlink_url {
                                    log::debug!("Opening hyperlink: {}", url);
                                    // Open hyperlink in browser
                                    std::thread::spawn(move || {
                                        let _ = open::that(&url);
                                    });
                                }
                                // Start new selection
                                let mode = if self.modifiers.shift_key() {
                                    SelectionMode::Line
                                } else {
                                    SelectionMode::Char
                                };
                                self.selection.start_selection(row, col, mode);
                            }
                            ElementState::Released => {
                                // End selection
                                self.selection.end_selection();
                                // Copy to clipboard if there's a selection
                                if self.selection.active {
                                    let lines = self.grid.all_lines();
                                    let text = self.selection.extract_text(&lines, self.size.cols as usize);
                                    if !text.is_empty() {
                                        self.clipboard.copy(&text);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(atlas) = &self.atlas {
                    let mode = self.grid.mouse_mode;
                    
                    if mouse::is_mouse_tracking_active(mode) {
                        let (h, v) = match delta {
                            winit::event::MouseScrollDelta::LineDelta(h, v) => (h, v),
                            winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.x as f32, pos.y as f32),
                        };
                        
                        let col = (self.mouse_position.0 / atlas.cell_width.max(1) as f64) as u32 + 1;
                        let row = (self.mouse_position.1 / atlas.cell_height.max(1) as f64) as u32 + 1;
                        
                        // Determine scroll direction
                        let button = if v > 0.0 {
                            Some(mouse::MouseButton::WheelUp)
                        } else if v < 0.0 {
                            Some(mouse::MouseButton::WheelDown)
                        } else if h > 0.0 {
                            Some(mouse::MouseButton::WheelRight)
                        } else if h < 0.0 {
                            Some(mouse::MouseButton::WheelLeft)
                        } else {
                            None
                        };
                        
                        if let Some(btn) = button {
                            let event = mouse::MouseEvent {
                                button: btn,
                                event_type: mouse::MouseEventType::Press,
                                col,
                                row,
                                shift: self.modifiers.shift_key(),
                                ctrl: self.modifiers.control_key(),
                                alt: self.modifiers.alt_key(),
                            };
                            
                            let encoded = event.encode(self.grid.mouse_encoding);
                            self.write_to_pty(encoded.as_bytes());
                        }
                    }
                }
            }

            WindowEvent::KeyboardInput {
                event: KeyEvent { logical_key, state: ElementState::Pressed, text, .. },
                ..
            } => {
                // Handle keyboard shortcuts with modifiers
                let text_str = text.as_ref().map(|s| s.to_string());
                let handled = self.handle_shortcut(&logical_key, &text_str);
                if !handled {
                    // Check for scrollback navigation (Shift+PageUp/Down)
                    let shift = self.modifiers.shift_key();
                    match &logical_key {
                        Key::Named(NamedKey::PageUp) if shift => {
                            // Scroll up in scrollback
                            let scroll_amount = self.size.rows as usize;
                            self.grid.scrollback_offset = (self.grid.scrollback_offset + scroll_amount)
                                .min(self.grid.scrollback.len());
                            log::debug!("Scroll up: offset={}", self.grid.scrollback_offset);
                        }
                        Key::Named(NamedKey::PageDown) if shift => {
                            // Scroll down in scrollback
                            let scroll_amount = self.size.rows as usize;
                            self.grid.scrollback_offset = self.grid.scrollback_offset
                                .saturating_sub(scroll_amount);
                            log::debug!("Scroll down: offset={}", self.grid.scrollback_offset);
                        }
                        _ => {
                            // No shortcut matched, send to PTY
                            // Also reset scrollback offset on any key press
                            if self.grid.scrollback_offset > 0 || self.grid.scroll_fraction > 0.0 {
                                self.grid.reset_scroll();
                            }
                            let bytes = encode_key(&logical_key);
                            if !bytes.is_empty() {
                                self.write_to_pty(&bytes);
                            }
                        }
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                self.drain_pty();

                // Update cursor blink
                let blink_interval = std::time::Duration::from_millis(self.config.cursor_blink_ms);
                if self.config.cursor_blink_ms > 0 {
                    let now = std::time::Instant::now();
                    if now.duration_since(self.last_cursor_blink) >= blink_interval {
                        self.cursor_visible = !self.cursor_visible;
                        self.last_cursor_blink = now;
                    }
                } else {
                    self.cursor_visible = true;
                }

                if let (Some(pipeline), Some(atlas)) = (&mut self.pipeline, &mut self.atlas) {
                    pipeline.render(&self.grid, atlas, self.cursor_visible);
                }

                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {}
}

// ---------------------------------------------------------------------------
// Key encoding — keyboard events → PTY byte sequences
// ---------------------------------------------------------------------------

fn encode_key(key: &Key) -> Vec<u8> {
    match key {
        Key::Character(s) => s.as_str().as_bytes().to_vec(),

        Key::Named(named) => match named {
            NamedKey::Enter => b"\r".to_vec(),
            NamedKey::Backspace => b"\x7f".to_vec(),
            NamedKey::Tab => b"\t".to_vec(),
            NamedKey::Escape => b"\x1b".to_vec(),
            NamedKey::Space => b" ".to_vec(),
            NamedKey::ArrowUp => b"\x1b[A".to_vec(),
            NamedKey::ArrowDown => b"\x1b[B".to_vec(),
            NamedKey::ArrowRight => b"\x1b[C".to_vec(),
            NamedKey::ArrowLeft => b"\x1b[D".to_vec(),
            NamedKey::Home => b"\x1b[H".to_vec(),
            NamedKey::End => b"\x1b[F".to_vec(),
            NamedKey::PageUp => b"\x1b[5~".to_vec(),
            NamedKey::PageDown => b"\x1b[6~".to_vec(),
            NamedKey::Delete => b"\x1b[3~".to_vec(),
            NamedKey::Insert => b"\x1b[2~".to_vec(),
            NamedKey::F1 => b"\x1bOP".to_vec(),
            NamedKey::F2 => b"\x1bOQ".to_vec(),
            NamedKey::F3 => b"\x1bOR".to_vec(),
            NamedKey::F4 => b"\x1bOS".to_vec(),
            NamedKey::F5 => b"\x1b[15~".to_vec(),
            NamedKey::F6 => b"\x1b[17~".to_vec(),
            NamedKey::F7 => b"\x1b[18~".to_vec(),
            NamedKey::F8 => b"\x1b[19~".to_vec(),
            NamedKey::F9 => b"\x1b[20~".to_vec(),
            NamedKey::F10 => b"\x1b[21~".to_vec(),
            NamedKey::F11 => b"\x1b[23~".to_vec(),
            NamedKey::F12 => b"\x1b[24~".to_vec(),
            _ => vec![],
        },

        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    env_logger::init();

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new();
    event_loop.run_app(&mut app).expect("event loop run");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::{Key, NamedKey};

    #[test]
    fn test_encode_enter() {
        let result = encode_key(&Key::Named(NamedKey::Enter));
        assert_eq!(result, b"\r");
    }

    #[test]
    fn test_encode_backspace() {
        let result = encode_key(&Key::Named(NamedKey::Backspace));
        assert_eq!(result, b"\x7f");
    }

    #[test]
    fn test_encode_tab() {
        let result = encode_key(&Key::Named(NamedKey::Tab));
        assert_eq!(result, b"\t");
    }

    #[test]
    fn test_encode_escape() {
        let result = encode_key(&Key::Named(NamedKey::Escape));
        assert_eq!(result, b"\x1b");
    }

    #[test]
    fn test_encode_space() {
        let result = encode_key(&Key::Named(NamedKey::Space));
        assert_eq!(result, b" ");
    }

    #[test]
    fn test_encode_arrows() {
        assert_eq!(encode_key(&Key::Named(NamedKey::ArrowUp)), b"\x1b[A");
        assert_eq!(encode_key(&Key::Named(NamedKey::ArrowDown)), b"\x1b[B");
        assert_eq!(encode_key(&Key::Named(NamedKey::ArrowRight)), b"\x1b[C");
        assert_eq!(encode_key(&Key::Named(NamedKey::ArrowLeft)), b"\x1b[D");
    }

    #[test]
    fn test_encode_home_end() {
        assert_eq!(encode_key(&Key::Named(NamedKey::Home)), b"\x1b[H");
        assert_eq!(encode_key(&Key::Named(NamedKey::End)), b"\x1b[F");
    }

    #[test]
    fn test_encode_page_up_down() {
        assert_eq!(encode_key(&Key::Named(NamedKey::PageUp)), b"\x1b[5~");
        assert_eq!(encode_key(&Key::Named(NamedKey::PageDown)), b"\x1b[6~");
    }

    #[test]
    fn test_encode_delete_insert() {
        assert_eq!(encode_key(&Key::Named(NamedKey::Delete)), b"\x1b[3~");
        assert_eq!(encode_key(&Key::Named(NamedKey::Insert)), b"\x1b[2~");
    }

    #[test]
    fn test_encode_function_keys() {
        assert_eq!(encode_key(&Key::Named(NamedKey::F1)), b"\x1bOP");
        assert_eq!(encode_key(&Key::Named(NamedKey::F2)), b"\x1bOQ");
        assert_eq!(encode_key(&Key::Named(NamedKey::F3)), b"\x1bOR");
        assert_eq!(encode_key(&Key::Named(NamedKey::F4)), b"\x1bOS");
        assert_eq!(encode_key(&Key::Named(NamedKey::F5)), b"\x1b[15~");
        assert_eq!(encode_key(&Key::Named(NamedKey::F6)), b"\x1b[17~");
        assert_eq!(encode_key(&Key::Named(NamedKey::F7)), b"\x1b[18~");
        assert_eq!(encode_key(&Key::Named(NamedKey::F8)), b"\x1b[19~");
        assert_eq!(encode_key(&Key::Named(NamedKey::F9)), b"\x1b[20~");
        assert_eq!(encode_key(&Key::Named(NamedKey::F10)), b"\x1b[21~");
        assert_eq!(encode_key(&Key::Named(NamedKey::F11)), b"\x1b[23~");
        assert_eq!(encode_key(&Key::Named(NamedKey::F12)), b"\x1b[24~");
    }

    #[test]
    fn test_encode_character() {
        let result = encode_key(&Key::Character("a".into()));
        assert_eq!(result, b"a");
    }

    #[test]
    fn test_encode_character_capital() {
        let result = encode_key(&Key::Character("A".into()));
        assert_eq!(result, b"A");
    }

    #[test]
    fn test_encode_unknown_named_returns_empty() {
        // Some named keys aren't mapped yet
        let result = encode_key(&Key::Named(NamedKey::Super));
        assert!(result.is_empty());
    }

    #[test]
    fn test_encode_unmapped_returns_empty() {
        // Dead key / unmapped key type returns empty
        let result = encode_key(&Key::Character("".into()));
        assert!(result.is_empty());
    }
}
