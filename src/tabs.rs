use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::grid::{Grid, WinSize};
use crate::parser::Parser;
use crate::pty::{self, PtyHandle, PtyWriter};

use crossbeam_channel::Receiver;

/// A single tab containing its own terminal state and PTY.
pub struct Tab {
    /// Tab title (can be changed via OSC 0/2).
    pub title: String,
    /// Terminal grid for this tab.
    pub grid: Grid,
    /// VT parser for this tab.
    pub parser: Parser,
    /// Whether this tab is currently active.
    pub active: bool,
    /// Writer half of the PTY (None if the tab was constructed without spawning).
    pub pty_writer: Option<PtyWriter>,
    /// Child process handle — Drop sends SIGHUP + reaps.
    pub pty_handle: Option<PtyHandle>,
    /// Reader channel for PTY output.
    pub pty_rx: Option<Receiver<Vec<u8>>>,
    /// Whether the PTY reader thread should be reading. Set false while the
    /// tab is in the background so the kernel PTY buffer provides backpressure
    /// (the shell blocks on a full buffer) instead of growing the channel
    /// unboundedly. Toggled by [`TabManager::switch_to`].
    pub reading: Arc<AtomicBool>,
}

impl Tab {
    /// Create a new tab with the given size and scrollback, spawning a PTY.
    /// The `wake` callback is invoked by the reader thread on each data chunk
    /// (T5-2) so the event loop can drain promptly.
    pub fn spawn(
        title: &str,
        size: WinSize,
        scrollback: usize,
        argv: &[String],
        wake: pty::WakeCallback,
    ) -> Result<Self, pty::PtyError> {
        let (writer, handle, rx, reading) = pty::spawn_pty(size, argv, wake)?;
        Ok(Self {
            title: title.to_string(),
            grid: Grid::new(size, scrollback),
            parser: Parser::new(),
            active: false,
            pty_writer: Some(writer),
            pty_handle: Some(handle),
            pty_rx: Some(rx),
            reading,
        })
    }

    /// Create a tab without spawning a PTY (used in tests).
    pub fn without_pty(title: &str, size: WinSize, scrollback: usize) -> Self {
        Self {
            title: title.to_string(),
            grid: Grid::new(size, scrollback),
            parser: Parser::new(),
            active: false,
            pty_writer: None,
            pty_handle: None,
            pty_rx: None,
            reading: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Manages multiple tabs, each with its own PTY/shell.
pub struct TabManager {
    /// List of tabs.
    tabs: Vec<Tab>,
    /// Index of the currently active tab.
    active_index: usize,
    /// Default size for new tabs.
    default_size: WinSize,
    /// Scrollback capacity for new tabs.
    scrollback: usize,
    /// Shell path for new tabs.
    shell: String,
    /// Wake callback cloned for each new tab's reader thread (T5-2).
    wake_factory: Box<dyn Fn() -> pty::WakeCallback + Send + Sync>,
    /// Terminal cell size in pixels (width, height); inherited by new tabs
    /// so sixel cursor advances use the correct geometry.
    cell_size: (u32, u32),
}

impl TabManager {
    /// Create a new tab manager with an initial tab (spawns its PTY).
    /// `wake_factory` produces a fresh wake callback for each tab's reader
    /// thread (T5-2).
    pub fn new(
        initial_size: WinSize,
        shell: &str,
        scrollback: usize,
        wake_factory: Box<dyn Fn() -> pty::WakeCallback + Send + Sync>,
    ) -> Result<Self, pty::PtyError> {
        let argv = vec![shell.to_string()];
        let wake = wake_factory();
        let mut first = Tab::spawn("Terminal", initial_size, scrollback, &argv, wake)?;
        first.active = true;
        Ok(Self {
            tabs: vec![first],
            active_index: 0,
            default_size: initial_size,
            scrollback,
            shell: shell.to_string(),
            wake_factory,
            cell_size: (8, 16),
        })
    }

    /// Like `new` but runs `command` via `sh -c` in the initial tab instead
    /// of the default shell. Subsequent tabs (Ctrl+T) still use `shell`.
    pub fn new_with_command(
        initial_size: WinSize,
        shell: &str,
        scrollback: usize,
        command: &str,
        wake_factory: Box<dyn Fn() -> pty::WakeCallback + Send + Sync>,
    ) -> Result<Self, pty::PtyError> {
        let argv = vec!["sh".to_string(), "-c".to_string(), command.to_string()];
        let wake = wake_factory();
        let mut first = Tab::spawn("Terminal", initial_size, scrollback, &argv, wake)?;
        first.active = true;
        Ok(Self {
            tabs: vec![first],
            active_index: 0,
            default_size: initial_size,
            scrollback,
            shell: shell.to_string(),
            wake_factory,
            cell_size: (8, 16),
        })
    }

    /// Get the number of tabs.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Check if there are no tabs.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Get the currently active tab.
    pub fn active(&self) -> &Tab {
        &self.tabs[self.active_index]
    }

    /// Get a mutable reference to the currently active tab.
    pub fn active_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_index]
    }

    /// Get the active tab index.
    pub fn active_index(&self) -> usize {
        self.active_index
    }

    /// Get all tabs (for rendering tab bar).
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Create a new tab (spawns a PTY) and return its index.
    pub fn new_tab(&mut self) -> Result<usize, pty::PtyError> {
        let title = format!("Terminal {}", self.tabs.len() + 1);
        let wake = (self.wake_factory)();
        let argv = vec![self.shell.clone()];
        let mut tab = Tab::spawn(&title, self.default_size, self.scrollback, &argv, wake)?;
        tab.grid.set_cell_size(self.cell_size.0, self.cell_size.1);
        self.tabs.push(tab);
        let new_index = self.tabs.len() - 1;
        self.switch_to(new_index);
        Ok(new_index)
    }

    /// Close the current tab. Returns the new active index, or `None` if this
    /// was the last tab (caller should close the window). Dropping the `Tab`
    /// drops its `PtyHandle`, which sends SIGHUP + reaps the child.
    pub fn close_current(&mut self) -> Option<usize> {
        if self.tabs.len() == 1 {
            return None;
        }

        self.tabs.remove(self.active_index);

        if self.active_index >= self.tabs.len() {
            self.active_index = self.tabs.len() - 1;
        }

        self.tabs[self.active_index].active = true;
        self.tabs[self.active_index]
            .reading
            .store(true, Ordering::Release);
        Some(self.active_index)
    }

    /// Close a specific tab by index.
    pub fn close_tab(&mut self, index: usize) -> Option<usize> {
        if index >= self.tabs.len() || self.tabs.len() == 1 {
            return None;
        }

        self.tabs.remove(index);

        if self.active_index >= self.tabs.len() {
            self.active_index = self.tabs.len() - 1;
        } else if self.active_index > index {
            self.active_index -= 1;
        }

        self.tabs[self.active_index].active = true;
        self.tabs[self.active_index]
            .reading
            .store(true, Ordering::Release);
        Some(self.active_index)
    }

    /// Switch to a specific tab by index.
    pub fn switch_to(&mut self, index: usize) {
        if index < self.tabs.len() {
            // Pause the outgoing tab's reader so it stops pulling output; the
            // incoming tab's reader resumes so its (blocked) writer unblocks.
            self.tabs[self.active_index]
                .reading
                .store(false, Ordering::Release);
            self.tabs[self.active_index].active = false;
            self.active_index = index;
            self.tabs[self.active_index].active = true;
            self.tabs[self.active_index]
                .reading
                .store(true, Ordering::Release);
        }
    }

    /// Switch to the next tab (wraps around).
    pub fn next(&mut self) {
        if self.tabs.len() > 1 {
            let next_index = (self.active_index + 1) % self.tabs.len();
            self.switch_to(next_index);
        }
    }

    /// Switch to the previous tab (wraps around).
    pub fn prev(&mut self) {
        if self.tabs.len() > 1 {
            let prev_index = if self.active_index == 0 {
                self.tabs.len() - 1
            } else {
                self.active_index - 1
            };
            self.switch_to(prev_index);
        }
    }

    /// Update the title of a tab.
    pub fn set_title(&mut self, index: usize, title: &str) {
        if index < self.tabs.len() {
            self.tabs[index].title = title.to_string();
        }
    }

    /// Update the title of the active tab.
    pub fn set_active_title(&mut self, title: &str) {
        self.set_title(self.active_index, title);
    }

    /// Resize all tabs (grids + PTY windows).
    pub fn resize(&mut self, size: WinSize) {
        self.default_size = size;
        for tab in &mut self.tabs {
            tab.grid.resize(size);
            if let Some(w) = &tab.pty_writer {
                w.resize(size);
            }
        }
    }

    /// Get tab titles for rendering.
    pub fn titles(&self) -> Vec<&str> {
        self.tabs.iter().map(|t| t.title.as_str()).collect()
    }

    /// Record the terminal's cell size (pixels) for all tabs; new tabs
    /// inherit it via [`TabManager::new_tab`].
    pub fn set_cell_size(&mut self, w: u32, h: u32) {
        self.cell_size = (w.max(1), h.max(1));
        for tab in &mut self.tabs {
            tab.grid.set_cell_size(self.cell_size.0, self.cell_size.1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manager() -> TabManager {
        // Build a manager without spawning real PTYs by constructing tabs
        // manually and using a private constructor path.
        let size = WinSize { cols: 80, rows: 24 };
        let mut tabs = Vec::new();
        let mut first = Tab::without_pty("Terminal", size, 1000);
        first.active = true;
        tabs.push(first);
        TabManager {
            tabs,
            active_index: 0,
            default_size: size,
            scrollback: 1000,
            shell: "/bin/bash".to_string(),
            wake_factory: Box::new(|| Box::new(|| {}) as pty::WakeCallback),
            cell_size: (8, 16),
        }
    }

    #[test]
    fn test_tab_manager_new() {
        let manager = make_manager();
        assert_eq!(manager.len(), 1);
        assert_eq!(manager.active_index(), 0);
        assert_eq!(manager.active().title, "Terminal");
    }

    #[test]
    fn test_new_tab_logic() {
        let mut manager = make_manager();
        // Simulate adding a tab without PTY spawn.
        let tab = Tab::without_pty("Terminal 2", manager.default_size, manager.scrollback);
        manager.tabs.push(tab);
        manager.switch_to(1);
        assert_eq!(manager.len(), 2);
        assert_eq!(manager.active_index(), 1);
        assert_eq!(manager.active().title, "Terminal 2");
    }

    #[test]
    fn test_close_tab() {
        let mut manager = make_manager();
        let t2 = Tab::without_pty("T2", manager.default_size, manager.scrollback);
        let t3 = Tab::without_pty("T3", manager.default_size, manager.scrollback);
        manager.tabs.push(t2);
        manager.tabs.push(t3);
        assert_eq!(manager.len(), 3);

        // Close the middle tab
        manager.switch_to(1);
        let result = manager.close_current();
        assert!(result.is_some());
        assert_eq!(manager.len(), 2);
        assert_eq!(manager.active_index(), 1);
    }

    #[test]
    fn test_close_last_tab() {
        let mut manager = make_manager();

        let result = manager.close_current();
        assert!(result.is_none());
        assert_eq!(manager.len(), 1);
    }

    #[test]
    fn test_switch_tabs() {
        let mut manager = make_manager();
        manager.tabs.push(Tab::without_pty(
            "T2",
            manager.default_size,
            manager.scrollback,
        ));
        manager.tabs.push(Tab::without_pty(
            "T3",
            manager.default_size,
            manager.scrollback,
        ));

        manager.switch_to(2);
        assert_eq!(manager.active_index(), 2);

        manager.next();
        assert_eq!(manager.active_index(), 0);

        manager.prev();
        assert_eq!(manager.active_index(), 2);
    }

    #[test]
    fn test_set_title() {
        let mut manager = make_manager();

        manager.set_active_title("My Tab");
        assert_eq!(manager.active().title, "My Tab");

        manager.set_title(0, "Renamed");
        assert_eq!(manager.active().title, "Renamed");
    }

    #[test]
    fn test_switch_to_pauses_and_resumes_pty_readers() {
        let mut manager = make_manager();
        manager.tabs.push(Tab::without_pty(
            "T2",
            manager.default_size,
            manager.scrollback,
        ));

        // First tab starts reading.
        manager.tabs[0].reading.store(true, Ordering::Release);

        manager.switch_to(1);
        assert!(!manager.tabs[0].reading.load(Ordering::Acquire));
        assert!(manager.tabs[1].reading.load(Ordering::Acquire));

        manager.switch_to(0);
        assert!(manager.tabs[0].reading.load(Ordering::Acquire));
        assert!(!manager.tabs[1].reading.load(Ordering::Acquire));
    }

    #[test]
    fn test_close_tab_resumes_reader() {
        let mut manager = make_manager();
        manager.tabs.push(Tab::without_pty(
            "T2",
            manager.default_size,
            manager.scrollback,
        ));
        manager.switch_to(1);
        assert!(!manager.tabs[0].reading.load(Ordering::Acquire));
        assert!(manager.tabs[1].reading.load(Ordering::Acquire));

        // Close the active tab → the remaining tab becomes active and resumes.
        manager.close_current();
        assert!(manager.tabs[0].reading.load(Ordering::Acquire));
    }

    #[test]
    fn test_resize_all_tabs() {
        let mut manager = make_manager();
        manager.tabs.push(Tab::without_pty(
            "T2",
            manager.default_size,
            manager.scrollback,
        ));

        let new_size = WinSize {
            cols: 120,
            rows: 40,
        };
        manager.resize(new_size);

        assert_eq!(manager.default_size, new_size);
    }
}
