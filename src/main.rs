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
use grid::WinSize;
use mouse::MouseButton;
use render::{font::GlyphAtlas, pipeline::{RenderParams, TerminalPipeline}};
use search::SearchState;
use selection::{Selection, SelectionMode};
use tab_bar::TabBar;
use tabs::TabManager;
use theme::Theme;

// ---------------------------------------------------------------------------
// Application (winit ApplicationHandler)
// ---------------------------------------------------------------------------

/// All state lives on the main thread — no shared state / mutex needed.
struct App {
    // Created on first `resumed` event
    window: Option<Arc<Window>>,
    pipeline: Option<TerminalPipeline>,
    atlas: Option<GlyphAtlas>,

    // Tab management — each tab owns its own grid, parser, and PTY.
    tab_manager: Option<TabManager>,
    tab_bar: TabBar,
    show_tab_bar: bool,

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

    // Set when the last tab is closed — signals the event loop to exit.
    should_quit: bool,

    // Configuration
    config: Config,

    size: WinSize,
}

impl App {
    fn new() -> Self {
        let mut config = Config::load();

        // Load theme if specified in config
        if let Some(ref theme_name) = config.colors.theme {
            if let Some(theme) = Theme::find(theme_name) {
                config.colors = theme.colors;
            }
        }

        let size = WinSize {
            cols: config.window.cols,
            rows: config.window.rows,
        };
        App {
            window: None,
            pipeline: None,
            atlas: None,
            // TabManager is spawned in `resumed` so PTY errors can be handled.
            tab_manager: None,
            tab_bar: TabBar::new(),
            show_tab_bar: config.tabs.show_tab_bar,
            clipboard: ClipboardManager::new(),
            modifiers: ModifiersState::default(),
            mouse_position: (0.0, f64::NEG_INFINITY),
            mouse_button_pressed: None,
            search: SearchState::new(),
            selection: Selection::new(),
            cursor_visible: true,
            last_cursor_blink: std::time::Instant::now(),
            should_quit: false,
            config,
            size,
        }
    }

    /// Refresh the tab bar from the current tab manager state.
    fn refresh_tab_bar(&mut self) {
        if let Some(tm) = &self.tab_manager {
            let titles = tm.titles();
            let active = tm.active_index();
            self.tab_bar.update_tabs(&titles, active);
        }
    }

    // --- Active tab accessors (return defaults when tab_manager is not yet
    //     initialized, i.e. before `resumed`). ---

    fn active_mouse_mode(&self) -> grid::MouseMode {
        self.tab_manager.as_ref()
            .map(|tm| tm.active().grid.mouse_mode)
            .unwrap_or_default()
    }

    fn active_mouse_encoding(&self) -> grid::MouseEncoding {
        self.tab_manager.as_ref()
            .map(|tm| tm.active().grid.mouse_encoding)
            .unwrap_or_default()
    }

    fn active_all_lines(&self) -> Vec<String> {
        self.tab_manager.as_ref()
            .map(|tm| tm.active().grid.all_lines())
            .unwrap_or_default()
    }

    fn active_bracketed_paste(&self) -> bool {
        self.tab_manager.as_ref()
            .map(|tm| tm.active().grid.bracketed_paste)
            .unwrap_or(false)
    }

    /// Drain all pending bytes from the active tab's PTY channel, parse them,
    /// update the grid. Returns true if any data was processed.
    fn drain_pty(&mut self) -> bool {
        let Some(tm) = &mut self.tab_manager else { return false };
        let tab = tm.active_mut();

        // Capture the title before parsing so we can detect OSC title changes.
        let title_before = tab.grid.palette.title.clone();

        // Collect chunks first so we can split the borrow cleanly
        let mut pending: Vec<Vec<u8>> = Vec::new();
        if let Some(rx) = &tab.pty_rx {
            while let Ok(chunk) = rx.try_recv() {
                pending.push(chunk);
            }
        }
        let had_data = !pending.is_empty();
        for chunk in pending {
            for byte in chunk {
                tab.parser.advance(&mut tab.grid, byte);
            }
        }
        if had_data {
            tab.grid.mark_all_dirty();

            // If the shell set a title via OSC 0/2, update the tab manager + tab bar.
            let title_after = &tab.grid.palette.title;
            if title_after != &title_before && !title_after.is_empty() {
                let new_title = title_after.clone();
                let active_idx = self.tab_manager.as_ref().map(|tm| tm.active_index()).unwrap_or(0);
                if let Some(tm) = &mut self.tab_manager {
                    tm.set_active_title(&new_title);
                }
                self.tab_bar.update_tabs(
                    &self.tab_manager.as_ref().map(|tm| tm.titles()).unwrap_or_default(),
                    active_idx,
                );
            }
        }
        had_data
    }

    fn write_to_pty(&self, data: &[u8]) {
        if let Some(tm) = &self.tab_manager {
            if let Some(w) = &tm.active().pty_writer {
                w.write(data);
            }
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
                    let lines = self.active_all_lines();
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
                    let bracketed = self.active_bracketed_paste();
                    if bracketed {
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
            // Ctrl+Shift+T — New tab
            Key::Character(s) if s.as_str() == "T" && ctrl && shift => {
                let spawn_result = self.tab_manager.as_mut().map(|tm| tm.new_tab());
                match spawn_result {
                    Some(Ok(_)) => {
                        let count = self.tab_manager.as_ref().map(|tm| tm.len()).unwrap_or(0);
                        self.refresh_tab_bar();
                        log::debug!("New tab created (count={})", count);
                    }
                    Some(Err(e)) => log::error!("Failed to spawn new tab: {e}"),
                    None => {}
                }
                true
            }
            // Ctrl+Shift+W — Close current tab
            Key::Character(s) if s.as_str() == "W" && ctrl && shift => {
                let close_result = self.tab_manager.as_mut().map(|tm| tm.close_current());
                match close_result {
                    Some(None) => {
                        // Last tab closed — signal the event loop to exit
                        self.should_quit = true;
                        log::debug!("Closed last tab — exiting");
                    }
                    Some(Some(_)) => {
                        let remaining = self.tab_manager.as_ref().map(|tm| tm.len()).unwrap_or(0);
                        self.refresh_tab_bar();
                        log::debug!("Closed tab (remaining={})", remaining);
                    }
                    None => {}
                }
                true
            }
            // Ctrl+PageDown — Next tab
            Key::Named(NamedKey::PageDown) if ctrl && !shift => {
                if let Some(tm) = &mut self.tab_manager {
                    tm.next();
                }
                let idx = self.tab_manager.as_ref().map(|tm| tm.active_index()).unwrap_or(0);
                self.refresh_tab_bar();
                log::debug!("Switched to next tab (index={})", idx);
                true
            }
            // Ctrl+PageUp — Previous tab
            Key::Named(NamedKey::PageUp) if ctrl && !shift => {
                if let Some(tm) = &mut self.tab_manager {
                    tm.prev();
                }
                let idx = self.tab_manager.as_ref().map(|tm| tm.active_index()).unwrap_or(0);
                self.refresh_tab_bar();
                log::debug!("Switched to prev tab (index={})", idx);
                true
            }
            // Ctrl+Shift+1..9 — Switch to tab N
            Key::Character(s) if ctrl && shift && matches!(s.as_str(), "1"|"2"|"3"|"4"|"5"|"6"|"7"|"8"|"9") => {
                if let Ok(n) = s.parse::<usize>() {
                    let len = self.tab_manager.as_ref().map(|tm| tm.len()).unwrap_or(0);
                    if n >= 1 && n <= len {
                        if let Some(tm) = &mut self.tab_manager {
                            tm.switch_to(n - 1);
                        }
                        self.refresh_tab_bar();
                        log::debug!("Switched to tab {}", n);
                    }
                }
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
                let lines = self.active_all_lines();
                self.search.search(&lines);
                if let Some(m) = self.search.next() {
                    if let Some(tm) = &mut self.tab_manager {
                        tm.active_mut().grid.mark_match_dirty(m.row, m.start_col, m.end_col);
                    }
                }
                true
            }
            // F3 or Ctrl+G — Find next
            Key::Named(NamedKey::F3) => {
                if let Some(m) = self.search.next() {
                    if let Some(tm) = &mut self.tab_manager {
                        tm.active_mut().grid.mark_match_dirty(m.row, m.start_col, m.end_col);
                    }
                }
                true
            }
            Key::Character(s) if s.as_str() == "g" && self.modifiers.control_key() && !self.modifiers.shift_key() => {
                if let Some(m) = self.search.next() {
                    if let Some(tm) = &mut self.tab_manager {
                        tm.active_mut().grid.mark_match_dirty(m.row, m.start_col, m.end_col);
                    }
                }
                true
            }
            // Shift+F3 or Ctrl+Shift+G — Find previous
            Key::Character(s) if s.as_str() == "g" && self.modifiers.control_key() && self.modifiers.shift_key() => {
                if let Some(m) = self.search.prev() {
                    if let Some(tm) = &mut self.tab_manager {
                        tm.active_mut().grid.mark_match_dirty(m.row, m.start_col, m.end_col);
                    }
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
                let lines = self.active_all_lines();
                self.search.search(&lines);
                true
            }
            // Character input — Add to search query
            Key::Character(text) => {
                self.search.query.push_str(text);
                // Recompile and search
                let query = self.search.query.clone();
                self.search.update_query(&query);
                let lines = self.active_all_lines();
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
        // Tab bar only shows with 2+ tabs; initial window has 1 tab so no offset.
        (
            self.size.cols as u32 * atlas.cell_width,
            self.size.rows as u32 * atlas.cell_height,
        )
    }

    /// Current tab bar height in pixels (0 if hidden or only one tab).
    fn tab_bar_height(&self) -> u32 {
        let tab_count = self.tab_manager.as_ref().map(|tm| tm.len()).unwrap_or(1);
        if self.show_tab_bar && tab_count > 1 {
            self.config.tabs.height
        } else {
            0
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let font_bytes = render::font::embedded_font();
        let mut atlas = GlyphAtlas::from_bytes(font_bytes, self.config.font.size);
        render::font::load_fallback_fonts(&mut atlas);
        let (pw, ph) = self.pixel_size(&atlas);

        let attrs = Window::default_attributes()
            .with_title(&self.config.window.title)
            .with_inner_size(PhysicalSize::new(pw, ph))
            .with_position(PhysicalPosition::new(100, 100))
            .with_resizable(true)
            .with_transparent(self.config.window.opacity < 1.0);

        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        self.window = Some(Arc::clone(&window));

        let pipeline = pollster::block_on(TerminalPipeline::new(Arc::clone(&window), &atlas, self.config.window.vsync));

        // Spawn the initial tab (which spawns the first PTY).
        match TabManager::new(self.size, &self.config.shell, self.config.scrollback) {
            Ok(mut tm) => {
                // Mark all cells dirty so the first frame renders the full grid.
                tm.active_mut().grid.mark_all_dirty();
                self.tab_manager = Some(tm);
                self.refresh_tab_bar();
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
                    let tb_h = self.tab_bar_height();
                    let new_cols = (size.width / atlas.cell_width.max(1)) as u16;
                    let new_rows = ((size.height.saturating_sub(tb_h)) / atlas.cell_height.max(1)) as u16;
                    if new_cols > 0 && new_rows > 0 {
                        self.size = WinSize { cols: new_cols, rows: new_rows };
                        if let Some(tm) = &mut self.tab_manager {
                            tm.resize(self.size);
                        }
                    }
                }

                // Request redraw after resize
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }

            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_position = (position.x, position.y);

                // Report motion if mouse tracking is active
                if let Some(atlas) = &self.atlas {
                    let mode = self.active_mouse_mode();
                    let button_pressed = self.mouse_button_pressed.is_some();

                    if mouse::should_report_motion(mode, button_pressed) {
                        let col = (position.x / atlas.cell_width.max(1) as f64) as u32 + 1;
                        let row = ((position.y - self.tab_bar_height() as f64) / atlas.cell_height.max(1) as f64) as u32 + 1;

                        let event = mouse::MouseEvent {
                            button: self.mouse_button_pressed.unwrap_or(mouse::MouseButton::Left),
                            event_type: mouse::MouseEventType::Motion,
                            col,
                            row,
                            shift: self.modifiers.shift_key(),
                            ctrl: self.modifiers.control_key(),
                            alt: self.modifiers.alt_key(),
                        };

                        let encoding = self.active_mouse_encoding();
                        let encoded = event.encode(encoding);
                        self.write_to_pty(encoded.as_bytes());
                    } else if self.selection.selecting {
                        // Update selection during drag
                        let col = (position.x / atlas.cell_width.max(1) as f64) as usize;
                        let row = ((position.y - self.tab_bar_height() as f64) / atlas.cell_height.max(1) as f64) as usize;

                        // Mark previously selected cells dirty
                        let (old_start, old_end) = self.selection.normalized();
                        if let Some(tm) = &mut self.tab_manager {
                            let grid = &mut tm.active_mut().grid;
                            for r in old_start.0..=old_end.0 {
                                for c in 0..self.size.cols as usize {
                                    if self.selection.contains(r, c) {
                                        grid.mark_dirty(c, r);
                                    }
                                }
                            }
                        }

                        self.selection.update(row, col);

                        // Mark newly selected cells dirty
                        if let Some(tm) = &mut self.tab_manager {
                            let grid = &mut tm.active_mut().grid;
                            for r in old_start.0.min(row)..=old_end.0.max(row) {
                                for c in 0..self.size.cols as usize {
                                    if self.selection.contains(r, c) {
                                        grid.mark_dirty(c, r);
                                    }
                                }
                            }
                        }
                    }
                }

                // Request redraw after mouse movement (selection update)
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                // Tab bar click handling — check before terminal mouse logic
                if button == winit::event::MouseButton::Left
                    && state == ElementState::Pressed
                    && self.show_tab_bar
                {
                    let tb_h = self.tab_bar_height();
                    let (mx, my) = self.mouse_position;
                    if my < tb_h as f64 {
                        let cell_width = self.atlas.as_ref().map(|a| a.cell_width).unwrap_or(8);
                        // Check close button first
                        if let Some(idx) = self.tab_bar.close_button_at_position(mx, my, cell_width) {
                            let close_result = self.tab_manager.as_mut().map(|tm| tm.close_tab(idx));
                            if matches!(close_result, Some(None)) {
                                self.should_quit = true;
                            }
                            self.refresh_tab_bar();
                        } else if let Some(idx) = self.tab_bar.tab_at_position(mx, my, cell_width) {
                            if let Some(tm) = &mut self.tab_manager {
                                tm.switch_to(idx);
                            }
                            self.refresh_tab_bar();
                        }
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                }

                if let Some(atlas) = &self.atlas {
                    let mode = self.active_mouse_mode();

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
                        let row = ((self.mouse_position.1 - self.tab_bar_height() as f64) / atlas.cell_height.max(1) as f64) as u32 + 1;

                        let event = mouse::MouseEvent {
                            button: winit_button.unwrap_or(mouse::MouseButton::Left),
                            event_type,
                            col,
                            row,
                            shift: self.modifiers.shift_key(),
                            ctrl: self.modifiers.control_key(),
                            alt: self.modifiers.alt_key(),
                        };

                        let encoding = self.active_mouse_encoding();
                        let encoded = event.encode(encoding);
                        self.write_to_pty(encoded.as_bytes());
                    } else if button == winit::event::MouseButton::Left {
                        // Handle selection when mouse tracking is not active
                        let col = (self.mouse_position.0 / atlas.cell_width.max(1) as f64) as usize;
                        let row = ((self.mouse_position.1 - self.tab_bar_height() as f64) / atlas.cell_height.max(1) as f64) as usize;

                        match state {
                            ElementState::Pressed => {
                                // Check for hyperlink at click position
                                let hyperlink_url = self.tab_manager.as_ref()
                                    .and_then(|tm| tm.active().grid.get_hyperlink_at(col, row))
                                    .map(|s| s.to_string());
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
                                    let lines = self.active_all_lines();
                                    let text = self.selection.extract_text(&lines, self.size.cols as usize);
                                    if !text.is_empty() {
                                        self.clipboard.copy(&text);
                                    }
                                }
                            }
                        }
                    }
                }

                // Request redraw after mouse button events
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(atlas) = &self.atlas {
                    let mode = self.active_mouse_mode();

                    if mouse::is_mouse_tracking_active(mode) {
                        let (h, v) = match delta {
                            winit::event::MouseScrollDelta::LineDelta(h, v) => (h, v),
                            winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.x as f32, pos.y as f32),
                        };

                        let col = (self.mouse_position.0 / atlas.cell_width.max(1) as f64) as u32 + 1;
                        let row = ((self.mouse_position.1 - self.tab_bar_height() as f64) / atlas.cell_height.max(1) as f64) as u32 + 1;

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

                            let encoding = self.active_mouse_encoding();
                            let encoded = event.encode(encoding);
                            self.write_to_pty(encoded.as_bytes());
                        }
                    }
                }

                // Request redraw after mouse wheel
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }

            WindowEvent::KeyboardInput {
                event: KeyEvent { logical_key, state: ElementState::Pressed, text, .. },
                ..
            } => {
                // Handle keyboard shortcuts with modifiers
                let text_str = text.as_ref().map(|s| s.to_string());
                let handled = self.handle_shortcut(&logical_key, &text_str);

                // Check if a shortcut requested app exit (e.g. closing the last tab)
                if self.should_quit {
                    event_loop.exit();
                    return;
                }

                if !handled {
                    // Check for scrollback navigation (Shift+PageUp/Down)
                    let shift = self.modifiers.shift_key();
                    match &logical_key {
                        Key::Named(NamedKey::PageUp) if shift => {
                            // Scroll up in scrollback
                            let scroll_amount = self.size.rows as usize;
                            if let Some(tm) = &mut self.tab_manager {
                                let grid = &mut tm.active_mut().grid;
                                grid.scrollback_offset = (grid.scrollback_offset + scroll_amount)
                                    .min(grid.scrollback.len());
                                log::debug!("Scroll up: offset={}", grid.scrollback_offset);
                            }
                        }
                        Key::Named(NamedKey::PageDown) if shift => {
                            // Scroll down in scrollback
                            let scroll_amount = self.size.rows as usize;
                            if let Some(tm) = &mut self.tab_manager {
                                let grid = &mut tm.active_mut().grid;
                                grid.scrollback_offset = grid.scrollback_offset
                                    .saturating_sub(scroll_amount);
                                log::debug!("Scroll down: offset={}", grid.scrollback_offset);
                            }
                        }
                        _ => {
                            // No shortcut matched, send to PTY
                            // Also reset scrollback offset on any key press
                            if let Some(tm) = &mut self.tab_manager {
                                let grid = &mut tm.active_mut().grid;
                                if grid.scrollback_offset > 0 || grid.scroll_fraction > 0.0 {
                                    grid.reset_scroll();
                                }
                            }
                            let bytes = encode_key(&logical_key);
                            if !bytes.is_empty() {
                                self.write_to_pty(&bytes);
                            }
                        }
                    }
                }

                // Request redraw after keyboard input
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }

            WindowEvent::RedrawRequested => {
                // Update cursor blink (already handled in about_to_wait, but keep here for safety)
                let blink_interval = std::time::Duration::from_millis(self.config.cursor_blink_ms);
                let mut needs_redraw = false;
                if self.config.cursor_blink_ms > 0 {
                    let now = std::time::Instant::now();
                    if now.duration_since(self.last_cursor_blink) >= blink_interval {
                        self.cursor_visible = !self.cursor_visible;
                        self.last_cursor_blink = now;
                        needs_redraw = true;
                    }
                } else {
                    self.cursor_visible = true;
                }

                // Compute tab bar height before borrowing tab_manager mutably.
                let tb_height = self.tab_bar_height();
                let tb_ref = if tb_height > 0 { Some(&self.tab_bar) } else { None };

                if let (Some(pipeline), Some(atlas), Some(tm)) = (&mut self.pipeline, &mut self.atlas, &mut self.tab_manager) {
                    let tab = tm.active_mut();
                    pipeline.render(RenderParams {
                        grid: &mut tab.grid,
                        atlas,
                        cursor_visible: self.cursor_visible,
                        colors: &self.config.colors,
                        selection: &self.selection,
                        tab_bar: tb_ref,
                        tab_bar_height: tb_height as f32,
                    });
                }

                if let Some(w) = &self.window {
                    // Update window title if OSC changed it
                    if let Some(tm) = &self.tab_manager {
                        let title = &tm.active().grid.palette.title;
                        if !title.is_empty() {
                            w.set_title(title);
                        }
                    }
                    // Only request redraw if we have data or cursor blinked
                    if needs_redraw {
                        w.request_redraw();
                    }
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Keep the event loop waking periodically so PTY data is drained promptly
        // without busy-waiting.
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            std::time::Instant::now() + std::time::Duration::from_millis(16),
        ));

        // Check for PTY data when about to wait — request redraw if data available
        let had_data = self.drain_pty();
        if had_data {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
        
        // Check cursor blink timer
        if self.config.cursor_blink_ms > 0 {
            let now = std::time::Instant::now();
            let blink_interval = std::time::Duration::from_millis(self.config.cursor_blink_ms);
            if now.duration_since(self.last_cursor_blink) >= blink_interval {
                self.cursor_visible = !self.cursor_visible;
                self.last_cursor_blink = now;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
        }
    }
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
    // Wake periodically to drain PTY data. 16ms gives ~60Hz idle refresh rate
    // without the busy-wait of ControlFlow::Poll.
    event_loop.set_control_flow(ControlFlow::WaitUntil(
        std::time::Instant::now() + std::time::Duration::from_millis(16),
    ));

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
