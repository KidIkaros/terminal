use std::sync::atomic::Ordering;

use crate::engine::{Command, EngineHandle};
use crate::grid::WinSize;
use crate::pty::{self, PtyError};

/// A single tab: an engine thread owning its grid, parser, and PTY, plus
/// app-side metadata (title, activity). The engine publishes immutable
/// snapshots; the app renders from them and drives the engine by commands.
pub struct Tab {
    /// Tab title (can be changed via OSC 0/2).
    pub title: String,
    /// Background engine (grid + parser + PTY) for this tab.
    pub engine: EngineHandle,
    /// Whether this tab is currently active.
    pub active: bool,
}

impl Tab {
    /// Create a new tab with the given size and scrollback, spawning a PTY.
    /// `wake` is invoked by the engine after each parse batch so the event
    /// loop can redraw promptly.
    pub fn spawn(
        title: &str,
        size: WinSize,
        scrollback: usize,
        cell_size: (u32, u32),
        argv: &[String],
        wake: pty::WakeCallback,
        drain_budget: usize,
    ) -> Result<Self, PtyError> {
        let engine =
            EngineHandle::spawn(title, size, scrollback, cell_size, argv, wake, drain_budget)?;
        Ok(Self {
            title: title.to_string(),
            engine,
            active: false,
        })
    }

    /// Create a tab without spawning a PTY (used in tests).
    pub fn without_pty(title: &str, size: WinSize, scrollback: usize) -> Self {
        Self {
            title: title.to_string(),
            engine: EngineHandle::idle(size, scrollback),
            active: false,
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
    /// Wake callback cloned for each new tab's engine (T5-2).
    wake_factory: Box<dyn Fn() -> pty::WakeCallback + Send + Sync>,
    /// Terminal cell size in pixels (width, height); inherited by new tabs
    /// so sixel cursor advances use the correct geometry.
    cell_size: (u32, u32),
    /// Per-tab PTY drain budget (env-overridable).
    drain_budget: usize,
}

impl TabManager {
    /// Create a new tab manager with an initial tab (spawns its PTY).
    /// `wake_factory` produces a fresh wake callback for each tab's engine.
    pub fn new(
        initial_size: WinSize,
        shell: &str,
        scrollback: usize,
        wake_factory: Box<dyn Fn() -> pty::WakeCallback + Send + Sync>,
        drain_budget: usize,
    ) -> Result<Self, PtyError> {
        let argv = vec![shell.to_string()];
        let wake = wake_factory();
        let mut first = Tab::spawn(
            "Terminal",
            initial_size,
            scrollback,
            (8, 16),
            &argv,
            wake,
            drain_budget,
        )?;
        first.active = true;
        Ok(Self {
            tabs: vec![first],
            active_index: 0,
            default_size: initial_size,
            scrollback,
            shell: shell.to_string(),
            wake_factory,
            cell_size: (8, 16),
            drain_budget,
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
        drain_budget: usize,
    ) -> Result<Self, PtyError> {
        let argv = vec!["sh".to_string(), "-c".to_string(), command.to_string()];
        let wake = wake_factory();
        let mut first = Tab::spawn(
            "Terminal",
            initial_size,
            scrollback,
            (8, 16),
            &argv,
            wake,
            drain_budget,
        )?;
        first.active = true;
        Ok(Self {
            tabs: vec![first],
            active_index: 0,
            default_size: initial_size,
            scrollback,
            shell: shell.to_string(),
            wake_factory,
            cell_size: (8, 16),
            drain_budget,
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
    pub fn new_tab(&mut self) -> Result<usize, PtyError> {
        let title = format!("Terminal {}", self.tabs.len() + 1);
        let wake = (self.wake_factory)();
        let argv = vec![self.shell.clone()];
        let tab = Tab::spawn(
            &title,
            self.default_size,
            self.scrollback,
            self.cell_size,
            &argv,
            wake,
            self.drain_budget,
        )?;
        self.tabs.push(tab);
        let new_index = self.tabs.len() - 1;
        self.switch_to(new_index);
        Ok(new_index)
    }

    /// Close the current tab. Returns the new active index, or `None` if this
    /// was the last tab (caller should close the window). Dropping the `Tab`
    /// sends `Quit` to the engine, which drops the PTY handle (SIGHUP + reap).
    pub fn close_current(&mut self) -> Option<usize> {
        if self.tabs.len() == 1 {
            return None;
        }

        let tab = self.tabs.remove(self.active_index);
        tab.engine.shutdown();

        if self.active_index >= self.tabs.len() {
            self.active_index = self.tabs.len() - 1;
        }

        self.tabs[self.active_index].active = true;
        self.tabs[self.active_index]
            .engine
            .reading
            .store(true, Ordering::Release);
        Some(self.active_index)
    }

    /// Close a specific tab by index.
    pub fn close_tab(&mut self, index: usize) -> Option<usize> {
        if index >= self.tabs.len() || self.tabs.len() == 1 {
            return None;
        }

        let tab = self.tabs.remove(index);
        tab.engine.shutdown();

        if self.active_index >= self.tabs.len() {
            self.active_index = self.tabs.len() - 1;
        } else if self.active_index > index {
            self.active_index -= 1;
        }

        self.tabs[self.active_index].active = true;
        self.tabs[self.active_index]
            .engine
            .reading
            .store(true, Ordering::Release);
        Some(self.active_index)
    }

    /// Switch to a specific tab by index.
    pub fn switch_to(&mut self, index: usize) {
        if index < self.tabs.len() {
            // Pause the outgoing tab's engine reader so it stops pulling
            // output; the incoming tab's reader resumes so its (blocked)
            // writer unblocks. Mirrors xterm/kitty background-pane behavior.
            self.tabs[self.active_index]
                .engine
                .reading
                .store(false, Ordering::Release);
            self.tabs[self.active_index].active = false;
            self.active_index = index;
            self.tabs[self.active_index].active = true;
            self.tabs[self.active_index]
                .engine
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
            tab.engine.send(Command::Resize(size));
        }
    }

    /// Get tab titles for rendering.
    pub fn titles(&self) -> Vec<&str> {
        self.tabs.iter().map(|t| t.title.as_str()).collect()
    }

    /// Record the terminal's cell size (pixels) for new tabs; existing tabs
    /// already received theirs at spawn.
    pub fn set_cell_size(&mut self, w: u32, h: u32) {
        self.cell_size = (w.max(1), h.max(1));
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
            drain_budget: 256 * 1024,
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
        manager.tabs[0]
            .engine
            .reading
            .store(true, Ordering::Release);

        manager.switch_to(1);
        assert!(!manager.tabs[0].engine.reading.load(Ordering::Acquire));
        assert!(manager.tabs[1].engine.reading.load(Ordering::Acquire));

        manager.switch_to(0);
        assert!(manager.tabs[0].engine.reading.load(Ordering::Acquire));
        assert!(!manager.tabs[1].engine.reading.load(Ordering::Acquire));
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
        assert!(!manager.tabs[0].engine.reading.load(Ordering::Acquire));
        assert!(manager.tabs[1].engine.reading.load(Ordering::Acquire));

        // Close the active tab → the remaining tab becomes active and resumes.
        manager.close_current();
        assert!(manager.tabs[0].engine.reading.load(Ordering::Acquire));
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

    #[test]
    fn test_engine_idle_snapshot_and_commands() {
        // The idle engine publishes a snapshot and services commands.
        let size = WinSize { cols: 20, rows: 5 };
        let engine = EngineHandle::idle(size, 100);
        let snap = engine.snapshot();
        assert_eq!(snap.rows, 5);
        assert_eq!(snap.cols, 20);
        assert_eq!(snap.cells.len(), 5);

        engine.send(Command::ScrollTo {
            offset: 3,
            fraction: 0.0,
        });
        // Command servicing is async; poll briefly for the snapshot update.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
        while std::time::Instant::now() < deadline {
            if engine.snapshot().scrollback_offset == 3 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("engine did not apply ScrollTo command");
    }
}
