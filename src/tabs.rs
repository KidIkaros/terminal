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
}

impl Tab {
    /// Create a new tab with the given size and scrollback, spawning a PTY.
    pub fn spawn(
        title: &str,
        size: WinSize,
        scrollback: usize,
        shell: &str,
    ) -> Result<Self, pty::PtyError> {
        let (writer, handle, rx) = pty::spawn_pty(size, shell)?;
        Ok(Self {
            title: title.to_string(),
            grid: Grid::new(size, scrollback),
            parser: Parser::new(),
            active: false,
            pty_writer: Some(writer),
            pty_handle: Some(handle),
            pty_rx: Some(rx),
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
}

impl TabManager {
    /// Create a new tab manager with an initial tab (spawns its PTY).
    pub fn new(
        initial_size: WinSize,
        shell: &str,
        scrollback: usize,
    ) -> Result<Self, pty::PtyError> {
        let mut first = Tab::spawn("Terminal", initial_size, scrollback, shell)?;
        first.active = true;
        Ok(Self {
            tabs: vec![first],
            active_index: 0,
            default_size: initial_size,
            scrollback,
            shell: shell.to_string(),
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
        let tab = Tab::spawn(&title, self.default_size, self.scrollback, &self.shell)?;
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
        Some(self.active_index)
    }

    /// Switch to a specific tab by index.
    pub fn switch_to(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.tabs[self.active_index].active = false;
            self.active_index = index;
            self.tabs[self.active_index].active = true;
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
        manager.tabs.push(Tab::without_pty("T2", manager.default_size, manager.scrollback));
        manager.tabs.push(Tab::without_pty("T3", manager.default_size, manager.scrollback));

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
    fn test_resize_all_tabs() {
        let mut manager = make_manager();
        manager.tabs.push(Tab::without_pty("T2", manager.default_size, manager.scrollback));

        let new_size = WinSize { cols: 120, rows: 40 };
        manager.resize(new_size);

        assert_eq!(manager.default_size, new_size);
    }
}
