use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use terminal::clipboard::ClipboardManager;
use terminal::config::Config;
use terminal::grid::WinSize;
use terminal::mouse::MouseButton;
use terminal::render::{
    font::GlyphAtlas,
    pipeline::{RenderParams, TerminalPipeline},
};
use terminal::search::SearchState;
use terminal::selection::{Selection, SelectionMode};
use terminal::tab_bar::TabBar;
use terminal::tabs::TabManager;
use terminal::theme::Theme;
// Re-import modules so bare paths like `grid::MouseMode` resolve.
use terminal::{
    clipboard, config, grid, mouse, parser, pty, render, search, selection, tab_bar, tabs, theme,
};
use winit::{
    application::ApplicationHandler,
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, KeyEvent, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

/// Custom event sent by PTY reader threads to wake the event loop (T5-2).
#[derive(Debug, Clone)]
enum UserEvent {
    /// PTY data is available on the channel — drain and redraw.
    PtyData,
}

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
    /// T5-2: proxy to wake the event loop from PTY reader threads.
    event_proxy: Option<EventLoopProxy<UserEvent>>,
    /// Coalesces PTY wake events during a single output burst.
    pty_wake_pending: Arc<AtomicBool>,
    /// Optional command to run instead of the default shell (set by `-e`).
    initial_command: Option<String>,
    startup_started: std::time::Instant,
    first_frame_logged: bool,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>, initial_command: Option<String>) -> Self {
        let mut config = Config::load();

        // Load theme if specified in config
        if let Some(ref theme_name) = config.colors.theme {
            if let Some(theme) = Theme::find(theme_name) {
                config.colors = theme.colors;
            }
        }
        if config.reduced_motion {
            config.cursor_blink_ms = 0;
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
            event_proxy: Some(proxy),
            pty_wake_pending: Arc::new(AtomicBool::new(false)),
            initial_command,
            startup_started: std::time::Instant::now(),
            first_frame_logged: false,
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
        self.tab_manager
            .as_ref()
            .map(|tm| tm.active().grid.mouse_mode)
            .unwrap_or_default()
    }

    fn active_mouse_encoding(&self) -> grid::MouseEncoding {
        self.tab_manager
            .as_ref()
            .map(|tm| tm.active().grid.mouse_encoding)
            .unwrap_or_default()
    }

    fn active_all_lines(&self) -> Vec<String> {
        self.tab_manager
            .as_ref()
            .map(|tm| tm.active().grid.all_lines())
            .unwrap_or_default()
    }

    fn active_bracketed_paste(&self) -> bool {
        self.tab_manager
            .as_ref()
            .map(|tm| tm.active().grid.bracketed_paste)
            .unwrap_or(false)
    }

    /// Drain all pending bytes from the active tab's PTY channel, parse them,
    /// update the grid. Returns true if any data was processed.
    fn drain_pty(&mut self) -> bool {
        let Some(tm) = &mut self.tab_manager else {
            return false;
        };
        let tab = tm.active_mut();

        // Capture the title before parsing so we can detect OSC title changes.
        let title_before = tab.grid.palette.title.clone();

        // Collect chunks first so we can split the borrow cleanly. A
        // disconnected receiver means the child shell exited; preserve the
        // final bytes already queued, then let the app close cleanly.
        let mut pending: Vec<Vec<u8>> = Vec::new();
        let mut channel_closed = false;
        if let Some(rx) = &tab.pty_rx {
            loop {
                match rx.try_recv() {
                    Ok(chunk) => pending.push(chunk),
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        channel_closed = true;
                        break;
                    }
                }
            }
        }
        let had_data = !pending.is_empty();
        if had_data {
            // Bulk mode: skip per-cell dirty bookkeeping during the batch.
            // Scrolling output would otherwise mark the whole visible region
            // dirty on every line feed (O(rows×cols) per scroll). One
            // mark_all_dirty() below covers the same cells for rendering.
            tab.grid.bulk_output = true;
        }
        for chunk in pending {
            tab.parser.advance_bytes(&mut tab.grid, &chunk);
        }
        if had_data {
            tab.grid.bulk_output = false;
            // The parser marks changed cells dirty during print/scroll/erase
            // operations; bulk mode deferred that, so mark the grid once.
            tab.grid.mark_all_dirty();

            // Device-query responses (T2): DA/DSR/CPR/DECRQM/OSC color
            // replies queued by the grid during parsing. Write them straight
            // to the PTY (tab is already mutably borrowed, so use its writer
            // rather than write_to_pty(&self)).
            for response in tab.grid.take_responses() {
                if let Some(w) = &tab.pty_writer {
                    w.write(&response);
                }
            }

            // OSC 52 clipboard drain (T1-6): apply set requests, then answer
            // queries with the current system clipboard contents. The reply is
            // written through tab.pty_writer directly (tab is already mutably
            // borrowed) — write_to_pty(&self) would conflict with the borrow.
            //
            // Security policy (locked down by default): writes are gated by
            // `security.osc52_write`, reads by `security.osc52_read` — a
            // hostile prompt must not silently read the clipboard.
            if let Some(text) = tab.grid.clipboard_set.take() {
                if !self.config.security.osc52_write {
                    log::warn!("OSC 52 write blocked by security policy");
                } else if text.is_empty() {
                    self.clipboard.clear();
                    log::debug!("OSC 52: cleared system clipboard");
                } else {
                    self.clipboard.copy(&text);
                    log::debug!("OSC 52: set system clipboard ({} chars)", text.len());
                }
            }
            if tab.grid.clipboard_query_requested {
                tab.grid.clipboard_query_requested = false;
                if !self.config.security.osc52_read {
                    log::warn!("OSC 52 query blocked by security policy");
                } else if let Some(contents) = self.clipboard.paste() {
                    let response = ClipboardManager::osc52_set(&contents);
                    if let Some(w) = &tab.pty_writer {
                        w.write(response.as_bytes());
                    }
                    log::debug!("OSC 52: replied to clipboard query");
                }
            }

            // DECCOLM (?3) / DECSCPP: the terminal asked to switch column
            // count (80/132). Mirror it in the real window so the renderer's
            // cell geometry stays consistent; the resulting Resized event
            // re-syncs the grid to the same size.
            if let Some(size) = tab.grid.window_resize_request.take() {
                if let (Some(w), Some(atlas)) = (&self.window, &self.atlas) {
                    // Inline tab_bar_height() (a method would conflict with
                    // the active-tab borrow held for this block).
                    let tb_h = if self.show_tab_bar {
                        self.config
                            .tabs
                            .height
                            .max(tab_bar::TabBar::HIT_SIZE as u32)
                    } else {
                        0
                    };
                    let width = (size.cols as u32) * atlas.cell_width.max(1);
                    let height = (size.rows as u32) * atlas.cell_height.max(1) + tb_h;
                    let _ = w.request_inner_size(winit::dpi::PhysicalSize::new(
                        width.max(1),
                        height.max(1),
                    ));
                    log::debug!(
                        "DECCOLM/DECSCPP: requested {}x{} window",
                        size.cols,
                        size.rows
                    );
                }
            }

            // Bell (BEL): surfaced to the log; a visual flash is future work.
            if tab.grid.take_bell() {
                log::debug!("bell: visual feedback requested by application");
            }
            // OSC 9 notifications: surface via the desktop notification
            // daemon (notify-send/libnotify). Fire-and-forget — notify-send
            // exits on its own — and fall back to the log when the binary is
            // missing (headless session, non-Linux desktop).
            if let Some(msg) = tab.grid.take_notification() {
                match std::process::Command::new("notify-send")
                    .args(["-a", "terminal", "-i", "utilities-terminal"])
                    .arg(&msg)
                    .spawn()
                {
                    Ok(_) => log::debug!("notification: {}", msg),
                    Err(e) => log::info!("notification (notify-send unavailable): {} ({})", msg, e),
                }
            }

            // If the shell set a title via OSC 0/2, update the tab manager + tab bar.
            let title_after = &tab.grid.palette.title;
            if title_after != &title_before && !title_after.is_empty() {
                let new_title = title_after.clone();
                let active_idx = self
                    .tab_manager
                    .as_ref()
                    .map(|tm| tm.active_index())
                    .unwrap_or(0);
                if let Some(tm) = &mut self.tab_manager {
                    tm.set_active_title(&new_title);
                }
                self.tab_bar.update_tabs(
                    &self
                        .tab_manager
                        .as_ref()
                        .map(|tm| tm.titles())
                        .unwrap_or_default(),
                    active_idx,
                );
            }
        }
        if channel_closed {
            self.should_quit = true;
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
                self.selection
                    .update(self.size.rows as usize - 1, self.size.cols as usize - 1);
                self.selection.end_selection();
                log::debug!("Select All: Ctrl+Shift+A");
                true
            }
            // NOTE: Ctrl+C/Z/D/L/W/U/K/A/E are intentionally NOT handled here —
            // they fall through to encode_key() + the Ctrl+letter → C0 control
            // code mapping in KeyboardInput, which sends 0x03, 0x1a, 0x04,
            // 0x0c, 0x17, 0x15, 0x0b, 0x01, 0x05 to the PTY respectively.
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
                let idx = self
                    .tab_manager
                    .as_ref()
                    .map(|tm| tm.active_index())
                    .unwrap_or(0);
                self.refresh_tab_bar();
                log::debug!("Switched to next tab (index={})", idx);
                true
            }
            // Ctrl+PageUp — Previous tab
            Key::Named(NamedKey::PageUp) if ctrl && !shift => {
                if let Some(tm) = &mut self.tab_manager {
                    tm.prev();
                }
                let idx = self
                    .tab_manager
                    .as_ref()
                    .map(|tm| tm.active_index())
                    .unwrap_or(0);
                self.refresh_tab_bar();
                log::debug!("Switched to prev tab (index={})", idx);
                true
            }
            // Ctrl+Shift+1..9 — Switch to tab N
            Key::Character(s)
                if ctrl
                    && shift
                    && matches!(
                        s.as_str(),
                        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"
                    ) =>
            {
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
                        tm.active_mut()
                            .grid
                            .mark_match_dirty(m.row, m.start_col, m.end_col);
                    }
                }
                true
            }
            // F3 or Ctrl+G — Find next
            Key::Named(NamedKey::F3) => {
                if let Some(m) = self.search.next() {
                    if let Some(tm) = &mut self.tab_manager {
                        tm.active_mut()
                            .grid
                            .mark_match_dirty(m.row, m.start_col, m.end_col);
                    }
                }
                true
            }
            Key::Character(s)
                if s.as_str() == "g"
                    && self.modifiers.control_key()
                    && !self.modifiers.shift_key() =>
            {
                if let Some(m) = self.search.next() {
                    if let Some(tm) = &mut self.tab_manager {
                        tm.active_mut()
                            .grid
                            .mark_match_dirty(m.row, m.start_col, m.end_col);
                    }
                }
                true
            }
            // Shift+F3 or Ctrl+Shift+G — Find previous
            Key::Character(s)
                if s.as_str() == "g"
                    && self.modifiers.control_key()
                    && self.modifiers.shift_key() =>
            {
                if let Some(m) = self.search.prev() {
                    if let Some(tm) = &mut self.tab_manager {
                        tm.active_mut()
                            .grid
                            .mark_match_dirty(m.row, m.start_col, m.end_col);
                    }
                }
                true
            }
            // Backspace — Remove last character from query
            Key::Named(NamedKey::Backspace) => {
                remove_last_query_char(&mut self.search.query);
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
        let cell_width = self
            .atlas
            .as_ref()
            .map(|a| a.cell_width as f64)
            .unwrap_or(8.0);
        let cell_height = self
            .atlas
            .as_ref()
            .map(|a| a.cell_height as f64)
            .unwrap_or(16.0);
        let col = (x / cell_width) as u32 + 1; // 1-based
        let row = (y / cell_height) as u32 + 1; // 1-based
        (col, row)
    }

    fn pixel_size(&self, atlas: &GlyphAtlas) -> (u32, u32) {
        let tb_h = if self.show_tab_bar {
            self.config
                .tabs
                .height
                .max(tab_bar::TabBar::HIT_SIZE as u32)
        } else {
            0
        };
        (
            self.size.cols as u32 * atlas.cell_width,
            self.size.rows as u32 * atlas.cell_height + tb_h,
        )
    }

    /// Current tab bar height in pixels (0 if hidden).
    fn tab_bar_height(&self) -> u32 {
        if self.show_tab_bar {
            self.config
                .tabs
                .height
                .max(tab_bar::TabBar::HIT_SIZE as u32)
        } else {
            0
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let trace_startup = std::env::var_os("TERMINAL_STARTUP_TRACE").is_some();
        let startup_started = self.startup_started;
        let trace = |stage: &str| {
            if trace_startup {
                log::info!(
                    "startup stage={stage} elapsed_ms={:.3}",
                    startup_started.elapsed().as_secs_f64() * 1000.0
                );
            }
        };

        trace("resumed");
        let font_bytes = render::font::embedded_font();
        let atlas = GlyphAtlas::from_bytes(font_bytes, self.config.font.size);
        trace("font");
        let (pw, ph) = self.pixel_size(&atlas);

        let attrs = Window::default_attributes()
            .with_title(&self.config.window.title)
            .with_inner_size(PhysicalSize::new(pw, ph))
            .with_position(PhysicalPosition::new(100, 100))
            .with_resizable(true)
            .with_transparent(self.config.window.opacity < 1.0);

        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        self.window = Some(Arc::clone(&window));
        trace("window");

        let pipeline = pollster::block_on(TerminalPipeline::new(
            Arc::clone(&window),
            &atlas,
            self.config.window.vsync,
        ));
        trace("gpu_pipeline");

        // Spawn the initial tab (which spawns the first PTY).
        // T5-2: pass an EventLoopProxy-backed wake callback so the reader
        // thread can wake the event loop immediately on PTY data.
        let proxy = self.event_proxy.clone();
        let wake_pending = Arc::clone(&self.pty_wake_pending);
        let wake_factory: Box<dyn Fn() -> pty::WakeCallback + Send + Sync> = match &proxy {
            Some(p) => {
                let p = p.clone();
                let wake_pending = Arc::clone(&wake_pending);
                Box::new(move || {
                    let p = p.clone();
                    let wake_pending = Arc::clone(&wake_pending);
                    Box::new(move || {
                        let _ = schedule_pty_wake(&wake_pending, || {
                            p.send_event(UserEvent::PtyData).is_ok()
                        });
                    }) as pty::WakeCallback
                })
            }
            None => Box::new(|| Box::new(|| {}) as pty::WakeCallback),
        };
        let tm_result = if let Some(ref cmd) = self.initial_command {
            TabManager::new_with_command(
                self.size,
                &self.config.shell,
                self.config.scrollback,
                cmd,
                wake_factory,
            )
        } else {
            TabManager::new(
                self.size,
                &self.config.shell,
                self.config.scrollback,
                wake_factory,
            )
        };
        match tm_result {
            Ok(mut tm) => {
                trace("pty_ready");
                // Record the real cell size so sixel cursor advances and
                // image spans use the correct geometry.
                tm.set_cell_size(atlas.cell_width, atlas.cell_height);
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

    /// T5-2: PTY reader thread pokes the event loop via EventLoopProxy.
    /// Drain the channel and request a redraw — no more 16ms polling.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyData => {
                self.pty_wake_pending.store(false, Ordering::Release);
                let had_data = self.drain_pty();
                if self.should_quit {
                    _event_loop.exit();
                    return;
                }
                let sync_active = self
                    .tab_manager
                    .as_ref()
                    .map(|tm| tm.active().grid.synchronized_output)
                    .unwrap_or(false);
                if had_data && !sync_active {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            // T4-3: focus reporting (?1004). vim/neovim use this to update
            // the statusline when the terminal gains/loses focus.
            WindowEvent::Focused(focused) => {
                if let Some(tm) = &mut self.tab_manager {
                    let grid = &mut tm.active_mut().grid;
                    if focused {
                        grid.focus_in();
                    } else {
                        grid.focus_out();
                    }
                    for response in grid.take_responses() {
                        self.write_to_pty(&response);
                    }
                }
            }

            WindowEvent::Resized(size) => {
                if let Some(pipeline) = &mut self.pipeline {
                    pipeline.resize(size.width, size.height);
                }
                if let Some(atlas) = &self.atlas {
                    let tb_h = self.tab_bar_height();
                    let new_cols = (size.width / atlas.cell_width.max(1)) as u16;
                    let new_rows =
                        ((size.height.saturating_sub(tb_h)) / atlas.cell_height.max(1)) as u16;
                    if new_cols > 0 && new_rows > 0 {
                        self.size = WinSize {
                            cols: new_cols,
                            rows: new_rows,
                        };
                        let mut responses = Vec::new();
                        if let Some(tm) = &mut self.tab_manager {
                            tm.resize(self.size);
                            // In-band resize notification (mode 2048): tell
                            // the application the new size so tmux/neovim
                            // redraw without polling.
                            tm.active_mut().grid.resize_report();
                            responses = tm.active_mut().grid.take_responses();
                        }
                        for response in responses {
                            self.write_to_pty(&response);
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
                if self.show_tab_bar {
                    let screen_width = self
                        .window
                        .as_ref()
                        .map(|window| window.inner_size().width)
                        .unwrap_or_default();
                    self.tab_bar
                        .update_hover(position.x, position.y, screen_width);
                }

                // Report motion if mouse tracking is active
                if let Some(atlas) = &self.atlas {
                    let mode = self.active_mouse_mode();
                    let button_pressed = self.mouse_button_pressed.is_some();

                    if mouse::should_report_motion(mode, button_pressed) {
                        let col = (position.x / atlas.cell_width.max(1) as f64) as u32 + 1;
                        let row = ((position.y - self.tab_bar_height() as f64)
                            / atlas.cell_height.max(1) as f64)
                            as u32
                            + 1;

                        let event = mouse::MouseEvent {
                            button: self
                                .mouse_button_pressed
                                .unwrap_or(mouse::MouseButton::Left),
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
                        let row = ((position.y - self.tab_bar_height() as f64)
                            / atlas.cell_height.max(1) as f64)
                            as usize;

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
                if button == winit::event::MouseButton::Left && self.show_tab_bar {
                    let tb_h = self.tab_bar_height();
                    let (mx, my) = self.mouse_position;
                    let screen_width = self
                        .window
                        .as_ref()
                        .map(|window| window.inner_size().width)
                        .unwrap_or_default();
                    let target = if my < tb_h as f64 {
                        self.tab_bar.target_at_position(mx, my, screen_width)
                    } else {
                        None
                    };
                    self.tab_bar.update_pressed(match state {
                        ElementState::Pressed => target,
                        ElementState::Released => None,
                    });

                    if state == ElementState::Pressed && my < tb_h as f64 {
                        let cell_width = self.atlas.as_ref().map(|a| a.cell_width).unwrap_or(8);
                        let screen_width = self
                            .window
                            .as_ref()
                            .map(|w| w.inner_size().width)
                            .unwrap_or(0);

                        // Check close button first
                        if let Some(idx) = self.tab_bar.close_button_at_position(mx, my, cell_width)
                        {
                            let close_result =
                                self.tab_manager.as_mut().map(|tm| tm.close_tab(idx));
                            if matches!(close_result, Some(None)) {
                                self.should_quit = true;
                            }
                            self.refresh_tab_bar();
                        } else if self
                            .tab_bar
                            .new_tab_button_at_position(mx, my, screen_width)
                        {
                            match self.tab_manager.as_mut().map(|tm| tm.new_tab()) {
                                Some(Ok(_)) => {
                                    self.refresh_tab_bar();
                                }
                                Some(Err(e)) => log::error!("Failed to spawn new tab: {e}"),
                                None => {}
                            }
                        } else if self.tab_bar.search_button_at_position(mx, my, screen_width) {
                            self.search.activate();
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

                        let col =
                            (self.mouse_position.0 / atlas.cell_width.max(1) as f64) as u32 + 1;
                        let row = ((self.mouse_position.1 - self.tab_bar_height() as f64)
                            / atlas.cell_height.max(1) as f64)
                            as u32
                            + 1;

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
                        let row = ((self.mouse_position.1 - self.tab_bar_height() as f64)
                            / atlas.cell_height.max(1) as f64)
                            as usize;

                        match state {
                            ElementState::Pressed => {
                                // Check for hyperlink at click position
                                let hyperlink_url = self
                                    .tab_manager
                                    .as_ref()
                                    .and_then(|tm| tm.active().grid.get_hyperlink_at(col, row))
                                    .map(|s| s.to_string());
                                if let Some(url) = hyperlink_url {
                                    if hyperlink_is_allowed(&url, &self.config.security.uri_schemes)
                                    {
                                        log::debug!("Opening hyperlink: {}", url);
                                        // Open only explicitly supported web links.
                                        std::thread::spawn(move || {
                                            let _ = open::that(&url);
                                        });
                                    } else {
                                        log::warn!("Blocked unsupported hyperlink scheme");
                                    }
                                }
                                // Start new selection: Alt = rectangular (block)
                                // selection, Shift = line selection, else char.
                                let mode = if self.modifiers.alt_key() {
                                    SelectionMode::Rectangular
                                } else if self.modifiers.shift_key() {
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
                                    let text = self
                                        .selection
                                        .extract_text(&lines, self.size.cols as usize);
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
                            winit::event::MouseScrollDelta::PixelDelta(pos) => {
                                (pos.x as f32, pos.y as f32)
                            }
                        };

                        let col =
                            (self.mouse_position.0 / atlas.cell_width.max(1) as f64) as u32 + 1;
                        let row = ((self.mouse_position.1 - self.tab_bar_height() as f64)
                            / atlas.cell_height.max(1) as f64)
                            as u32
                            + 1;

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
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        text,
                        repeat,
                        ..
                    },
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
                    let ctrl = self.modifiers.control_key();
                    match &logical_key {
                        // Ctrl+Shift+Up/Down — jump between shell prompts
                        // (OSC 133 shell-integration markers). When the target
                        // prompt is in scrollback, scroll it to the top of the
                        // viewport; when visible, move the cursor to it.
                        Key::Named(NamedKey::ArrowUp) if ctrl && shift => {
                            if let Some(tm) = &mut self.tab_manager {
                                let grid = &mut tm.active_mut().grid;
                                let sb_len = grid.scrollback.len();
                                let from = sb_len + grid.cursor.row;
                                if let Some(idx) = grid.prev_prompt(from) {
                                    if idx < sb_len {
                                        grid.scrollback_offset = sb_len - idx;
                                    } else {
                                        grid.reset_scroll();
                                        grid.cursor.row = idx - sb_len;
                                        grid.mark_all_dirty();
                                    }
                                }
                            }
                        }
                        Key::Named(NamedKey::ArrowDown) if ctrl && shift => {
                            if let Some(tm) = &mut self.tab_manager {
                                let grid = &mut tm.active_mut().grid;
                                let sb_len = grid.scrollback.len();
                                let from = sb_len + grid.cursor.row;
                                if let Some(idx) = grid.next_prompt(from) {
                                    if idx < sb_len {
                                        grid.scrollback_offset = sb_len - idx;
                                    } else {
                                        grid.reset_scroll();
                                        grid.cursor.row = idx - sb_len;
                                        grid.mark_all_dirty();
                                    }
                                } else {
                                    // No further prompt — return to the live
                                    // view at the shell cursor.
                                    grid.reset_scroll();
                                    grid.mark_all_dirty();
                                }
                            }
                        }
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
                                grid.scrollback_offset =
                                    grid.scrollback_offset.saturating_sub(scroll_amount);
                                log::debug!("Scroll down: offset={}", grid.scrollback_offset);
                            }
                        }
                        _ => {
                            // No shortcut matched, send to PTY.
                            // Also reset scrollback offset on any key press
                            if let Some(tm) = &mut self.tab_manager {
                                let grid = &mut tm.active_mut().grid;
                                if grid.scrollback_offset > 0 || grid.scroll_fraction > 0.0 {
                                    grid.reset_scroll();
                                }
                            }
                            let (kitty_flags, modify_other_keys) = self
                                .tab_manager
                                .as_ref()
                                .map(|tm| {
                                    let grid = &tm.active().grid;
                                    (grid.kitty_flags, grid.modify_other_keys)
                                })
                                .unwrap_or((0, 0));
                            let event_type = if repeat { 2 } else { 1 };
                            let mut bytes = encode_protocol_key(
                                &logical_key,
                                &self.modifiers,
                                text.as_deref(),
                                kitty_flags,
                                event_type,
                                modify_other_keys,
                            );
                            // Ctrl+letter → C0 control code (SIGINT etc.). This
                            // only applies in legacy mode: the kitty disambiguate
                            // / all-keys enhancements already report these as
                            // `CSI cp;mod u` escape codes.
                            if kitty_flags & (KITTY_DISAMBIGUATE | KITTY_ALL_KEYS) == 0
                                && self.modifiers.control_key()
                                && !shift
                                && bytes.len() == 1
                            {
                                if let Some(code) = ctrl_code(bytes[0]) {
                                    bytes[0] = code;
                                }
                            }
                            // DECCKM (?1): application cursor keys send SS3
                            // sequences instead of CSI (T3-1). vim/less/nano
                            // rely on this — but only in legacy mode (kitty's
                            // canonical forms ignore the cursor-key mode).
                            if kitty_flags & (KITTY_DISAMBIGUATE | KITTY_ALL_KEYS) == 0 {
                                if let Some(tm) = &self.tab_manager {
                                    if tm.active().grid.application_cursor_keys {
                                        app_cursor_remap(&mut bytes);
                                    }
                                }
                            }
                            if !bytes.is_empty() {
                                self.write_to_pty(&bytes);
                            }
                        }
                    }
                }

                // T4-6: reset cursor blink on keypress — real terminals
                // restart the blink cycle so the cursor is solid immediately
                // after input and only begins blinking after the idle period.
                if self.config.cursor_blink_ms > 0 {
                    self.cursor_visible = true;
                    self.last_cursor_blink = std::time::Instant::now();
                }

                // Request redraw after keyboard input
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }

            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Released,
                        text,
                        ..
                    },
                ..
            } => {
                // Kitty keyboard protocol — report key-release events when the
                // application requested event types. Plain text and legacy
                // Enter/Tab/Backspace keys produce raw bytes rather than an
                // escape code, so gate on the encoded form being an escape
                // sequence (per the spec, those need the all-keys enhancement).
                let kitty_flags = self
                    .tab_manager
                    .as_ref()
                    .map(|tm| tm.active().grid.kitty_flags)
                    .unwrap_or(0);
                if kitty_flags & KITTY_EVENT_TYPES != 0 {
                    let modify_other_keys = self
                        .tab_manager
                        .as_ref()
                        .map(|tm| tm.active().grid.modify_other_keys)
                        .unwrap_or(0);
                    let bytes = encode_protocol_key(
                        &logical_key,
                        &self.modifiers,
                        text.as_deref(),
                        kitty_flags,
                        3, // release
                        modify_other_keys,
                    );
                    if bytes.first() == Some(&0x1b) && bytes.len() > 1 {
                        self.write_to_pty(&bytes);
                    }
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
                let tb_ref = if tb_height > 0 {
                    Some(&mut self.tab_bar)
                } else {
                    None
                };

                if let (Some(pipeline), Some(atlas), Some(tm)) =
                    (&mut self.pipeline, &mut self.atlas, &mut self.tab_manager)
                {
                    let tab = tm.active_mut();
                    pipeline.render(RenderParams {
                        grid: &mut tab.grid,
                        atlas,
                        cursor_visible: self.cursor_visible,
                        colors: &self.config.colors,
                        selection: &self.selection,
                        search: Some(&mut self.search),
                        tab_bar: tb_ref,
                        tab_bar_height: tb_height as f32,
                    });
                }

                if !self.first_frame_logged {
                    self.first_frame_logged = true;
                    if std::env::var_os("TERMINAL_STARTUP_TRACE").is_some() {
                        log::info!(
                            "startup stage=first_frame elapsed_ms={:.3}",
                            self.startup_started.elapsed().as_secs_f64() * 1000.0
                        );
                    }
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
        // T5-2: PTY reader threads wake the loop via EventLoopProxy, so we
        // can use ControlFlow::Wait instead of polling at 60 Hz. The only
        // remaining reason to wake periodically is cursor blinking.
        if self.config.cursor_blink_ms > 0 {
            let now = std::time::Instant::now();
            let blink_interval = std::time::Duration::from_millis(self.config.cursor_blink_ms);
            let next_blink = self.last_cursor_blink + blink_interval;
            if now >= next_blink {
                self.cursor_visible = !self.cursor_visible;
                self.last_cursor_blink = now;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            // Wake at the next blink tick.
            event_loop.set_control_flow(ControlFlow::WaitUntil(next_blink));
        } else {
            // No blinking — pure wait, woken only by PTY/input/resize events.
            event_loop.set_control_flow(ControlFlow::Wait);
        }

        // Drain any PTY data that arrived between events (safety net for
        // data that came in after the last user_event but before we sleep).
        let had_data = self.drain_pty();
        if self.should_quit {
            event_loop.exit();
            return;
        }
        let sync_active = self
            .tab_manager
            .as_ref()
            .map(|tm| tm.active().grid.synchronized_output)
            .unwrap_or(false);
        if had_data && !sync_active {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Event scheduling helpers
// ---------------------------------------------------------------------------

fn remove_last_query_char(query: &mut String) {
    query.pop();
}

/// Schedule at most one wake event while a PTY burst is pending.
fn schedule_pty_wake(pending: &AtomicBool, send: impl FnOnce() -> bool) -> bool {
    if pending.swap(true, Ordering::AcqRel) {
        return false;
    }
    if send() {
        true
    } else {
        pending.store(false, Ordering::Release);
        false
    }
}

// ---------------------------------------------------------------------------
// Hyperlink policy
// ---------------------------------------------------------------------------

/// Permit URLs without embedded control characters whose scheme is on the
/// configured allowlist. OSC 8 / plain-text content is terminal input and
/// must not be allowed to invoke arbitrary URI handlers.
fn hyperlink_is_allowed(url: &str, schemes: &[String]) -> bool {
    if url.chars().any(|ch| ch.is_control()) {
        return false;
    }
    let Some(scheme) = url.split_once("://").map(|(s, _)| s) else {
        return false;
    };
    schemes.iter().any(|s| s == scheme)
}

// ---------------------------------------------------------------------------
// Key encoding — keyboard events → PTY byte sequences
// ---------------------------------------------------------------------------

/// Map an ASCII byte to its Ctrl-modified C0 control code.
/// Returns None if the byte has no Ctrl combination.
fn ctrl_code(b: u8) -> Option<u8> {
    if b.is_ascii_lowercase() {
        Some(b & 0x1f) // a-z → 0x01-0x1a
    } else {
        match b {
            b'@' => Some(0x00),  // Ctrl+@ = NUL
            b'[' => Some(0x1b),  // Ctrl+[ = ESC
            b'\\' => Some(0x1c), // Ctrl+\ = FS
            b']' => Some(0x1d),  // Ctrl+] = GS
            b'^' => Some(0x1e),  // Ctrl+^ = RS
            b'_' => Some(0x1f),  // Ctrl+_ = US
            _ => None,
        }
    }
}

/// Kitty keyboard protocol progressive-enhancement flags.
const KITTY_DISAMBIGUATE: u8 = 0b0_0001;
const KITTY_EVENT_TYPES: u8 = 0b0_0010;
const KITTY_ALTERNATE: u8 = 0b0_0100;
const KITTY_ALL_KEYS: u8 = 0b0_1000;
const KITTY_ASSOC_TEXT: u8 = 0b1_0000;

/// Kitty modifier bits: shift=1, alt=2, ctrl=4, super=8.
fn kitty_modifier_bits(m: &ModifiersState) -> u8 {
    (m.shift_key() as u8)
        | ((m.alt_key() as u8) << 1)
        | ((m.control_key() as u8) << 2)
        | ((m.super_key() as u8) << 3)
}

/// Kitty modifier parameter = 1 + the modifier bit-field.
fn kitty_modifier(m: &ModifiersState) -> u8 {
    1 + kitty_modifier_bits(m)
}

/// Build a kitty `CSI ... u` escape code.
///
/// `code` is the unshifted Unicode codepoint (or a functional key code);
/// `shifted` is the alternate shifted codepoint sent when alternate-key
/// reporting is requested and Shift is held; `text` is the associated text
/// (report-associated-text enhancement).
fn kitty_csi_u(
    code: u32,
    shifted: Option<u32>,
    kmod: u8,
    event_type: u8,
    flags: u8,
    text: Option<&str>,
) -> Vec<u8> {
    use std::fmt::Write;
    let report_events = flags & KITTY_EVENT_TYPES != 0;
    let assoc_text = flags & KITTY_ASSOC_TEXT != 0;
    let mut s = String::from("\x1b[");
    let _ = write!(s, "{}", code);
    if let Some(sh) = shifted {
        let _ = write!(s, ":{}", sh);
    }
    let need_mod = kmod != 1 || (report_events && event_type != 1);
    if need_mod {
        let _ = write!(s, ";{}", kmod);
        if report_events && event_type != 1 {
            let _ = write!(s, ":{}", event_type);
        }
    }
    if assoc_text {
        if let Some(t) = text {
            let mut first = true;
            for ch in t.chars() {
                if !ch.is_control() {
                    s.push(if first { ';' } else { ':' });
                    let _ = write!(s, "{}", ch as u32);
                    first = false;
                }
            }
        }
    }
    s.push('u');
    s.into_bytes()
}

/// Encode a functional (named) key in kitty canonical form:
/// `CSI 1;mod LETTER` (arrows/Home/End/F1–F4) or `CSI num;mod ~` (the rest).
fn kitty_functional_key(named: &NamedKey, kmod: u8, event_type: u8, flags: u8) -> Option<Vec<u8>> {
    use std::fmt::Write;
    let report_events = flags & KITTY_EVENT_TYPES != 0;
    let (letter, tilde_num): (Option<u8>, Option<u16>) = match named {
        NamedKey::ArrowUp => (Some(b'A'), None),
        NamedKey::ArrowDown => (Some(b'B'), None),
        NamedKey::ArrowRight => (Some(b'C'), None),
        NamedKey::ArrowLeft => (Some(b'D'), None),
        NamedKey::Home => (Some(b'H'), None),
        NamedKey::End => (Some(b'F'), None),
        NamedKey::F1 => (Some(b'P'), None),
        NamedKey::F2 => (Some(b'Q'), None),
        NamedKey::F3 => (Some(b'R'), None),
        NamedKey::F4 => (Some(b'S'), None),
        NamedKey::Insert => (None, Some(2)),
        NamedKey::Delete => (None, Some(3)),
        NamedKey::PageUp => (None, Some(5)),
        NamedKey::PageDown => (None, Some(6)),
        NamedKey::F5 => (None, Some(15)),
        NamedKey::F6 => (None, Some(17)),
        NamedKey::F7 => (None, Some(18)),
        NamedKey::F8 => (None, Some(19)),
        NamedKey::F9 => (None, Some(20)),
        NamedKey::F10 => (None, Some(21)),
        NamedKey::F11 => (None, Some(23)),
        NamedKey::F12 => (None, Some(24)),
        _ => return None,
    };

    let mut s = vec![0x1b, b'['];
    let mod_suffix = if kmod != 1 || (report_events && event_type != 1) {
        let mut buf = String::new();
        let _ = write!(buf, "{}", kmod);
        if report_events && event_type != 1 {
            let _ = write!(buf, ":{}", event_type);
        }
        Some(buf)
    } else {
        None
    };
    if let Some(letter) = letter {
        if let Some(m) = &mod_suffix {
            s.extend_from_slice(b"1;");
            s.extend_from_slice(m.as_bytes());
        }
        s.push(letter);
    } else if let Some(num) = tilde_num {
        s.extend_from_slice(num.to_string().as_bytes());
        if let Some(m) = &mod_suffix {
            s.push(b';');
            s.extend_from_slice(m.as_bytes());
        }
        s.push(b'~');
    }
    Some(s)
}

/// Encode a key event into the bytes sent to the PTY, honoring the negotiated
/// Kitty keyboard protocol enhancements and xterm modifyOtherKeys, falling
/// back to the legacy xterm encoder otherwise.
///
/// `event_type` is 1 (press), 2 (repeat) or 3 (release); it is only reported
/// when the event-types enhancement is active.
fn encode_protocol_key(
    key: &Key,
    modifiers: &ModifiersState,
    text: Option<&str>,
    flags: u8,
    event_type: u8,
    modify_other_keys: u8,
) -> Vec<u8> {
    let disambig = flags & KITTY_DISAMBIGUATE != 0;
    let all_keys = flags & KITTY_ALL_KEYS != 0;
    let kitty_active = disambig || all_keys;
    let kmod = kitty_modifier(modifiers);

    match key {
        Key::Character(s) => {
            let s = s.as_str();
            let Some(ch) = s.chars().next() else {
                return encode_key(key, modifiers);
            };
            // The unshifted codepoint is always reported (kitty rule).
            let code = ch.to_ascii_lowercase() as u32;
            // Alternate (shifted) key for shortcut matching.
            let shifted = if flags & KITTY_ALTERNATE != 0 && modifiers.shift_key() {
                let upper = ch.to_ascii_uppercase();
                (upper as u32 != code).then_some(upper as u32)
            } else {
                None
            };
            // Report as an escape code when the app asked for disambiguation
            // of modified keys, or all keys as escape codes.
            let as_escape = all_keys || (disambig && kmod != 1);
            if as_escape {
                return kitty_csi_u(code, shifted, kmod, event_type, flags, text);
            }
            // Legacy: modifyOtherKeys, then plain text.
            let xmod = modifier_param(modifiers);
            if modify_other_keys > 0 && xmod != 1 {
                return format!("\x1b[27;{};{}~", xmod, code).into_bytes();
            }
            encode_key(key, modifiers)
        }
        Key::Named(named) => match named {
            NamedKey::Enter => {
                if all_keys {
                    kitty_csi_u(13, None, kmod, event_type, flags, text)
                } else {
                    b"\r".to_vec()
                }
            }
            NamedKey::Tab => {
                if all_keys {
                    kitty_csi_u(9, None, kmod, event_type, flags, text)
                } else if modifiers.shift_key() {
                    b"\x1b[Z".to_vec()
                } else if disambig && kmod != 1 {
                    kitty_csi_u(9, None, kmod, event_type, flags, text)
                } else {
                    b"\t".to_vec()
                }
            }
            NamedKey::Backspace => {
                if all_keys {
                    kitty_csi_u(127, None, kmod, event_type, flags, text)
                } else {
                    b"\x7f".to_vec()
                }
            }
            NamedKey::Escape => {
                if kitty_active {
                    kitty_csi_u(27, None, kmod, event_type, flags, text)
                } else {
                    b"\x1b".to_vec()
                }
            }
            other => {
                if kitty_active {
                    if let Some(bytes) = kitty_functional_key(other, kmod, event_type, flags) {
                        return bytes;
                    }
                }
                encode_key(key, modifiers)
            }
        },
        _ => encode_key(key, modifiers),
    }
}

/// Encode a key into the bytes sent to the PTY.
///
/// T3-16: when Shift/Alt/Ctrl are held, navigation keys emit modified
/// CSI sequences (`ESC [ 1 ; P A` for Ctrl+Up, etc.) instead of the bare
/// unmodified form. Modifier bit P = 1 + (Shift?1) + (Alt?2) + (Ctrl?4):
/// 2=Shift, 3=Alt, 4=Alt+Shift, 5=Ctrl, 6=Ctrl+Shift, 7=Ctrl+Alt,
/// 8=Ctrl+Alt+Shift — the xterm / readline convention. Alt+character is
/// sent as the `ESC` prefix (`ESC c`) so apps (and the Alt modifier detection)
/// see a literal `\x1b` in front of the char.
fn encode_key(key: &Key, modifiers: &ModifiersState) -> Vec<u8> {
    // Alt + printable character → ESC prefix (T3-16 Alt prefix).
    if modifiers.alt_key() {
        if let Key::Character(s) = key {
            let mut b = vec![0x1b];
            b.extend_from_slice(s.as_str().as_bytes());
            return b;
        }
    }

    let p = modifier_param(modifiers);

    match key {
        Key::Character(s) => s.as_str().as_bytes().to_vec(),

        Key::Named(named) => match named {
            NamedKey::Enter => b"\r".to_vec(),
            NamedKey::Backspace => b"\x7f".to_vec(),
            NamedKey::Tab => {
                // Shift+Tab is CBT (`CSI Z`); plain Tab stays a C0 HT.
                if modifiers.shift_key() {
                    b"\x1b[Z".to_vec()
                } else {
                    b"\t".to_vec()
                }
            }
            NamedKey::Escape => b"\x1b".to_vec(),
            NamedKey::Space => b" ".to_vec(),
            NamedKey::ArrowUp => modified_csi(b'A', p),
            NamedKey::ArrowDown => modified_csi(b'B', p),
            NamedKey::ArrowRight => modified_csi(b'C', p),
            NamedKey::ArrowLeft => modified_csi(b'D', p),
            NamedKey::Home => home_end(b'H', p),
            NamedKey::End => home_end(b'F', p),
            // Tilde-terminated keys: number before `~` then optional `;P`.
            NamedKey::PageUp => tilde_seq(5, p),
            NamedKey::PageDown => tilde_seq(6, p),
            NamedKey::Delete => tilde_seq(3, p),
            NamedKey::Insert => tilde_seq(2, p),
            // Function keys — F1–F4 use SS3/CSI `P..S`, F5–F12 use `~`.
            NamedKey::F1 => fn_key(b'P', p),
            NamedKey::F2 => fn_key(b'Q', p),
            NamedKey::F3 => fn_key(b'R', p),
            NamedKey::F4 => fn_key(b'S', p),
            NamedKey::F5 => tilde_seq(15, p),
            NamedKey::F6 => tilde_seq(17, p),
            NamedKey::F7 => tilde_seq(18, p),
            NamedKey::F8 => tilde_seq(19, p),
            NamedKey::F9 => tilde_seq(20, p),
            NamedKey::F10 => tilde_seq(21, p),
            NamedKey::F11 => tilde_seq(23, p),
            NamedKey::F12 => tilde_seq(24, p),
            _ => vec![],
        },

        _ => vec![],
    }
}

/// Modifier parameter for CSI sequences (1 = no modifier).
#[inline]
fn modifier_param(m: &ModifiersState) -> u8 {
    1 + (m.shift_key() as u8) + (m.alt_key() as u8) * 2 + (m.control_key() as u8) * 4
}

/// Arrow key: unmodified `ESC [ X`, modified `ESC [ 1 ; P X`.
fn modified_csi(final_byte: u8, p: u8) -> Vec<u8> {
    if p == 1 {
        vec![0x1b, b'[', final_byte]
    } else {
        format!("\x1b[1;{}", p)
            .into_bytes()
            .into_iter()
            .chain(std::iter::once(final_byte))
            .collect()
    }
}

/// Home/End: unmodified `ESC [ H`/`ESC [ F`, modified `ESC [ 1 ; P H/F`.
fn home_end(final_byte: u8, p: u8) -> Vec<u8> {
    if p == 1 {
        vec![0x1b, b'[', final_byte]
    } else {
        format!("\x1b[1;{}", p)
            .into_bytes()
            .into_iter()
            .chain(std::iter::once(final_byte))
            .collect()
    }
}

/// Tilde-terminated key: unmodified `ESC [ N ~`, modified `ESC [ N ; P ~`.
/// `code` is the numeric key code (e.g. `5` for PageUp, `15` for F5).
fn tilde_seq(code: u16, p: u8) -> Vec<u8> {
    let mut v = vec![0x1b, b'['];
    v.extend_from_slice(code.to_string().as_bytes());
    if p != 1 {
        v.extend_from_slice(format!(";{}", p).as_bytes());
    }
    v.push(b'~');
    v
}

/// Function key F1–F4: unmodified SS3 `ESC O X`, modified CSI `ESC [ 1 ; P X`.
fn fn_key(x: u8, p: u8) -> Vec<u8> {
    if p == 1 {
        vec![0x1b, b'O', x]
    } else {
        format!("\x1b[1;{}", p)
            .into_bytes()
            .into_iter()
            .chain(std::iter::once(x))
            .collect()
    }
}

/// DECCKM (?1) — application cursor keys: arrows and Home/End switch from
/// CSI (ESC [) to SS3 (ESC O). Called only when the grid reports the mode
/// set. (T3-1)
fn app_cursor_remap(bytes: &mut Vec<u8>) {
    if bytes.len() == 3 && bytes[0] == 0x1b && bytes[1] == b'[' {
        match bytes[2] {
            b'A' | b'B' | b'C' | b'D' | b'H' | b'F' => {
                bytes[1] = b'O';
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    env_logger::init();

    // Minimal CLI parsing: `terminal [-e command] [--cols N] [--rows N]`
    let mut initial_command: Option<String> = None;
    let mut override_cols: Option<u16> = None;
    let mut override_rows: Option<u16> = None;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-e" | "--command" => {
                i += 1;
                if i < args.len() {
                    initial_command = Some(args[i].clone());
                }
            }
            "--cols" => {
                i += 1;
                if i < args.len() {
                    override_cols = args[i].parse().ok();
                }
            }
            "--rows" => {
                i += 1;
                if i < args.len() {
                    override_rows = args[i].parse().ok();
                }
            }
            _ => {}
        }
        i += 1;
    }

    // T5-2: EventLoop with a custom user event so PTY reader threads can
    // wake the loop via EventLoopProxy instead of 16ms polling.
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("event loop");
    let proxy = event_loop.create_proxy();

    let mut app = App::new(proxy, initial_command);
    // Apply CLI geometry overrides (take precedence over config).
    if let Some(cols) = override_cols {
        app.size.cols = cols;
        app.config.window.cols = cols;
    }
    if let Some(rows) = override_rows {
        app.size.rows = rows;
        app.config.window.rows = rows;
    }
    event_loop.run_app(&mut app).expect("event loop run");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::{Key, NamedKey};

    fn none() -> ModifiersState {
        ModifiersState::default()
    }

    #[test]
    fn test_search_backspace_removes_one_unicode_scalar() {
        let mut query = "café界".to_string();
        remove_last_query_char(&mut query);
        assert_eq!(query, "café");
        remove_last_query_char(&mut query);
        assert_eq!(query, "caf");
    }

    #[test]
    fn test_pty_wake_scheduler_coalesces_until_cleared() {
        let pending = AtomicBool::new(false);
        let mut sends = 0;
        assert!(schedule_pty_wake(&pending, || {
            sends += 1;
            true
        }));
        assert!(!schedule_pty_wake(&pending, || {
            sends += 1;
            true
        }));
        assert_eq!(sends, 1);
        pending.store(false, Ordering::Release);
        assert!(schedule_pty_wake(&pending, || true));
    }

    #[test]
    fn test_pty_wake_scheduler_clears_gate_when_send_fails() {
        let pending = AtomicBool::new(false);
        assert!(!schedule_pty_wake(&pending, || false));
        assert!(!pending.load(Ordering::Acquire));
    }

    #[test]
    fn test_hyperlink_policy_default_schemes() {
        let schemes: Vec<String> = vec!["http".into(), "https".into()];
        assert!(hyperlink_is_allowed("https://example.com/path", &schemes));
        assert!(hyperlink_is_allowed("http://example.com", &schemes));
        assert!(!hyperlink_is_allowed("file:///etc/passwd", &schemes));
        assert!(!hyperlink_is_allowed("javascript:alert(1)", &schemes));
        assert!(!hyperlink_is_allowed(
            "https://example.com/\nnext",
            &schemes
        ));
        assert!(!hyperlink_is_allowed("example.com", &schemes)); // no scheme
    }

    #[test]
    fn test_hyperlink_policy_custom_schemes() {
        let schemes: Vec<String> = vec!["gemini".into(), "mailto".into()];
        assert!(hyperlink_is_allowed(
            "gemini://geminiprotocol.net",
            &schemes
        ));
        assert!(hyperlink_is_allowed("mailto://x@y.com", &schemes));
        assert!(!hyperlink_is_allowed("https://example.com", &schemes));
    }

    #[test]
    fn test_protocol_key_encoding() {
        let modifiers = ModifiersState::CONTROL;
        assert_eq!(
            encode_protocol_key(
                &Key::Character("x".into()),
                &modifiers,
                None,
                KITTY_DISAMBIGUATE,
                1,
                0,
            ),
            b"\x1b[120;5u"
        );
        assert_eq!(
            encode_protocol_key(&Key::Character("x".into()), &modifiers, None, 0, 1, 1),
            b"\x1b[27;5;120~"
        );
    }

    #[test]
    fn test_kitty_escape_disambiguation() {
        assert_eq!(
            encode_protocol_key(
                &Key::Named(NamedKey::Escape),
                &none(),
                None,
                KITTY_DISAMBIGUATE,
                1,
                0,
            ),
            b"\x1b[27u"
        );
        // Legacy: raw ESC byte.
        assert_eq!(
            encode_protocol_key(&Key::Named(NamedKey::Escape), &none(), None, 0, 1, 0),
            b"\x1b"
        );
    }

    #[test]
    fn test_kitty_ctrl_shift_alt_disambiguation() {
        let ctrl = ModifiersState::CONTROL;
        // ctrl+c → CSI 99;5 u, not the SIGINT byte 0x03.
        assert_eq!(
            encode_protocol_key(
                &Key::Character("c".into()),
                &ctrl,
                None,
                KITTY_DISAMBIGUATE,
                1,
                0,
            ),
            b"\x1b[99;5u"
        );
        // Unshifted codepoint reported: shift+a → code 97.
        let shift = ModifiersState::SHIFT;
        assert_eq!(
            encode_protocol_key(
                &Key::Character("A".into()),
                &shift,
                Some("A"),
                KITTY_DISAMBIGUATE,
                1,
                0,
            ),
            b"\x1b[97;2u"
        );
        // alt+a → CSI 97;3 u.
        let alt = ModifiersState::ALT;
        assert_eq!(
            encode_protocol_key(
                &Key::Character("a".into()),
                &alt,
                None,
                KITTY_DISAMBIGUATE,
                1,
                0,
            ),
            b"\x1b[97;3u"
        );
    }

    #[test]
    fn test_kitty_event_types() {
        let flags = KITTY_DISAMBIGUATE | KITTY_EVENT_TYPES;
        let ctrl = ModifiersState::CONTROL;
        assert_eq!(
            encode_protocol_key(&Key::Character("x".into()), &ctrl, None, flags, 2, 0),
            b"\x1b[120;5:2u"
        );
        assert_eq!(
            encode_protocol_key(&Key::Character("x".into()), &ctrl, None, flags, 3, 0),
            b"\x1b[120;5:3u"
        );
    }

    #[test]
    fn test_kitty_all_keys_mode() {
        assert_eq!(
            encode_protocol_key(
                &Key::Character("a".into()),
                &none(),
                Some("a"),
                KITTY_ALL_KEYS,
                1,
                0,
            ),
            b"\x1b[97u"
        );
        assert_eq!(
            encode_protocol_key(
                &Key::Named(NamedKey::Enter),
                &none(),
                None,
                KITTY_ALL_KEYS,
                1,
                0,
            ),
            b"\x1b[13u"
        );
        assert_eq!(
            encode_protocol_key(
                &Key::Named(NamedKey::Tab),
                &none(),
                None,
                KITTY_ALL_KEYS,
                1,
                0,
            ),
            b"\x1b[9u"
        );
        assert_eq!(
            encode_protocol_key(
                &Key::Named(NamedKey::Backspace),
                &none(),
                None,
                KITTY_ALL_KEYS,
                1,
                0,
            ),
            b"\x1b[127u"
        );
    }

    #[test]
    fn test_kitty_associated_text() {
        let flags = KITTY_ALL_KEYS | KITTY_ASSOC_TEXT;
        let shift = ModifiersState::SHIFT;
        assert_eq!(
            encode_protocol_key(&Key::Character("A".into()), &shift, Some("A"), flags, 1, 0,),
            b"\x1b[97;2;65u"
        );
    }

    #[test]
    fn test_kitty_alternate_keys() {
        let flags = KITTY_DISAMBIGUATE | KITTY_ALTERNATE;
        let shift = ModifiersState::SHIFT;
        assert_eq!(
            encode_protocol_key(&Key::Character("A".into()), &shift, Some("A"), flags, 1, 0,),
            b"\x1b[97:65;2u"
        );
    }

    #[test]
    fn test_kitty_functional_keys() {
        let flags = KITTY_DISAMBIGUATE;
        assert_eq!(
            encode_protocol_key(&Key::Named(NamedKey::ArrowUp), &none(), None, flags, 1, 0),
            b"\x1b[A"
        );
        let ctrl = ModifiersState::CONTROL;
        assert_eq!(
            encode_protocol_key(&Key::Named(NamedKey::ArrowUp), &ctrl, None, flags, 1, 0),
            b"\x1b[1;5A"
        );
        assert_eq!(
            encode_protocol_key(&Key::Named(NamedKey::F1), &none(), None, flags, 1, 0),
            b"\x1b[P"
        );
        assert_eq!(
            encode_protocol_key(&Key::Named(NamedKey::PageUp), &none(), None, flags, 1, 0),
            b"\x1b[5~"
        );
        let ev_flags = KITTY_DISAMBIGUATE | KITTY_EVENT_TYPES;
        assert_eq!(
            encode_protocol_key(&Key::Named(NamedKey::ArrowUp), &ctrl, None, ev_flags, 3, 0),
            b"\x1b[1;5:3A"
        );
    }

    #[test]
    fn test_kitty_legacy_unchanged_when_disabled() {
        // With no kitty flags, behaviour is identical to the legacy encoder.
        let ctrl = ModifiersState::CONTROL;
        assert_eq!(
            encode_protocol_key(&Key::Character("c".into()), &ctrl, None, 0, 1, 0),
            b"c"
        );
        let alt = ModifiersState::ALT;
        assert_eq!(
            encode_protocol_key(&Key::Character("a".into()), &alt, None, 0, 1, 0),
            b"\x1ba"
        );
    }

    #[test]
    fn test_encode_enter() {
        let result = encode_key(&Key::Named(NamedKey::Enter), &none());
        assert_eq!(result, b"\r");
    }

    #[test]
    fn test_encode_backspace() {
        let result = encode_key(&Key::Named(NamedKey::Backspace), &none());
        assert_eq!(result, b"\x7f");
    }

    #[test]
    fn test_encode_tab() {
        let result = encode_key(&Key::Named(NamedKey::Tab), &none());
        assert_eq!(result, b"\t");
    }

    #[test]
    fn test_encode_escape() {
        let result = encode_key(&Key::Named(NamedKey::Escape), &none());
        assert_eq!(result, b"\x1b");
    }

    #[test]
    fn test_encode_space() {
        let result = encode_key(&Key::Named(NamedKey::Space), &none());
        assert_eq!(result, b" ");
    }

    #[test]
    fn test_encode_arrows() {
        assert_eq!(
            encode_key(&Key::Named(NamedKey::ArrowUp), &none()),
            b"\x1b[A"
        );
        assert_eq!(
            encode_key(&Key::Named(NamedKey::ArrowDown), &none()),
            b"\x1b[B"
        );
        assert_eq!(
            encode_key(&Key::Named(NamedKey::ArrowRight), &none()),
            b"\x1b[C"
        );
        assert_eq!(
            encode_key(&Key::Named(NamedKey::ArrowLeft), &none()),
            b"\x1b[D"
        );
    }

    #[test]
    fn test_encode_home_end() {
        assert_eq!(encode_key(&Key::Named(NamedKey::Home), &none()), b"\x1b[H");
        assert_eq!(encode_key(&Key::Named(NamedKey::End), &none()), b"\x1b[F");
    }

    #[test]
    fn test_encode_page_up_down() {
        assert_eq!(
            encode_key(&Key::Named(NamedKey::PageUp), &none()),
            b"\x1b[5~"
        );
        assert_eq!(
            encode_key(&Key::Named(NamedKey::PageDown), &none()),
            b"\x1b[6~"
        );
    }

    #[test]
    fn test_encode_delete_insert() {
        assert_eq!(
            encode_key(&Key::Named(NamedKey::Delete), &none()),
            b"\x1b[3~"
        );
        assert_eq!(
            encode_key(&Key::Named(NamedKey::Insert), &none()),
            b"\x1b[2~"
        );
    }

    #[test]
    fn test_encode_function_keys() {
        assert_eq!(encode_key(&Key::Named(NamedKey::F1), &none()), b"\x1bOP");
        assert_eq!(encode_key(&Key::Named(NamedKey::F2), &none()), b"\x1bOQ");
        assert_eq!(encode_key(&Key::Named(NamedKey::F3), &none()), b"\x1bOR");
        assert_eq!(encode_key(&Key::Named(NamedKey::F4), &none()), b"\x1bOS");
        assert_eq!(encode_key(&Key::Named(NamedKey::F5), &none()), b"\x1b[15~");
        assert_eq!(encode_key(&Key::Named(NamedKey::F6), &none()), b"\x1b[17~");
        assert_eq!(encode_key(&Key::Named(NamedKey::F7), &none()), b"\x1b[18~");
        assert_eq!(encode_key(&Key::Named(NamedKey::F8), &none()), b"\x1b[19~");
        assert_eq!(encode_key(&Key::Named(NamedKey::F9), &none()), b"\x1b[20~");
        assert_eq!(encode_key(&Key::Named(NamedKey::F10), &none()), b"\x1b[21~");
        assert_eq!(encode_key(&Key::Named(NamedKey::F11), &none()), b"\x1b[23~");
        assert_eq!(encode_key(&Key::Named(NamedKey::F12), &none()), b"\x1b[24~");
    }

    #[test]
    fn test_encode_character() {
        let result = encode_key(&Key::Character("a".into()), &none());
        assert_eq!(result, b"a");
    }

    #[test]
    fn test_encode_character_capital() {
        let result = encode_key(&Key::Character("A".into()), &none());
        assert_eq!(result, b"A");
    }

    // -- Ctrl+letter → C0 control codes (T1-1) --

    #[test]
    fn test_ctrl_code_letters() {
        assert_eq!(ctrl_code(b'a'), Some(0x01)); // Ctrl+A
        assert_eq!(ctrl_code(b'c'), Some(0x03)); // Ctrl+C = SIGINT
        assert_eq!(ctrl_code(b'd'), Some(0x04)); // Ctrl+D = EOF
        assert_eq!(ctrl_code(b'l'), Some(0x0c)); // Ctrl+L
        assert_eq!(ctrl_code(b'z'), Some(0x1a)); // Ctrl+Z = SIGTSTP
    }

    #[test]
    fn test_ctrl_code_symbols() {
        assert_eq!(ctrl_code(b'@'), Some(0x00)); // Ctrl+@ = NUL
        assert_eq!(ctrl_code(b'['), Some(0x1b)); // Ctrl+[ = ESC
        assert_eq!(ctrl_code(b'\\'), Some(0x1c)); // Ctrl+\ = FS
        assert_eq!(ctrl_code(b']'), Some(0x1d)); // Ctrl+] = GS
        assert_eq!(ctrl_code(b'^'), Some(0x1e)); // Ctrl+^ = RS
        assert_eq!(ctrl_code(b'_'), Some(0x1f)); // Ctrl+_ = US
    }

    #[test]
    fn test_ctrl_code_none_for_unmapped() {
        assert_eq!(ctrl_code(b'A'), None); // uppercase not Ctrl-combined here
        assert_eq!(ctrl_code(b'1'), None);
        assert_eq!(ctrl_code(b' '), None);
    }

    #[test]
    fn test_encode_unknown_named_returns_empty() {
        // Some named keys aren't mapped yet
        let result = encode_key(&Key::Named(NamedKey::Super), &none());
        assert!(result.is_empty());
    }

    #[test]
    fn test_encode_unmapped_returns_empty() {
        // Dead key / unmapped key type returns empty
        let result = encode_key(&Key::Character("".into()), &none());
        assert!(result.is_empty());
    }
}
