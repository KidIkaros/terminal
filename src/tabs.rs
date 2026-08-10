use crate::grid::{Grid, WinSize};
use crate::parser::Parser;

/// A single tab containing its own terminal state
pub struct Tab {
    /// Tab title (can be changed via OSC)
    pub title: String,
    /// Terminal grid for this tab
    pub grid: Grid,
    /// VT parser for this tab
    pub parser: Parser,
    /// Whether this tab is currently active
    pub active: bool,
}

impl Tab {
    /// Create a new tab with the given size
    pub fn new(title: &str, size: WinSize) -> Self {
        Self {
            title: title.to_string(),
            grid: Grid::new(size),
            parser: Parser::new(),
            active: false,
        }
    }
}

/// Manages multiple tabs
pub struct TabManager {
    /// List of tabs
    tabs: Vec<Tab>,
    /// Index of the currently active tab
    active_index: usize,
    /// Default size for new tabs
    default_size: WinSize,
}

impl TabManager {
    /// Create a new tab manager with an initial tab
    pub fn new(initial_size: WinSize) -> Self {
        let mut tabs = Vec::new();
        tabs.push(Tab::new("Terminal", initial_size));
        tabs[0].active = true;

        Self {
            tabs,
            active_index: 0,
            default_size: initial_size,
        }
    }

    /// Get the number of tabs
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Check if there are no tabs
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Get the currently active tab
    pub fn active(&self) -> &Tab {
        &self.tabs[self.active_index]
    }

    /// Get a mutable reference to the currently active tab
    pub fn active_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_index]
    }

    /// Get the active tab index
    pub fn active_index(&self) -> usize {
        self.active_index
    }

    /// Get all tabs (for rendering tab bar)
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Create a new tab and return its index
    pub fn new_tab(&mut self) -> usize {
        let title = format!("Terminal {}", self.tabs.len() + 1);
        let tab = Tab::new(&title, self.default_size);
        self.tabs.push(tab);
        let new_index = self.tabs.len() - 1;
        self.switch_to(new_index);
        new_index
    }

    /// Close the current tab, returns the new active index
    /// Returns None if this was the last tab (caller should close the window)
    pub fn close_current(&mut self) -> Option<usize> {
        if self.tabs.len() == 1 {
            return None;
        }

        self.tabs.remove(self.active_index);

        // Adjust active index if needed
        if self.active_index >= self.tabs.len() {
            self.active_index = self.tabs.len() - 1;
        }

        self.tabs[self.active_index].active = true;
        Some(self.active_index)
    }

    /// Close a specific tab by index
    pub fn close_tab(&mut self, index: usize) -> Option<usize> {
        if index >= self.tabs.len() || self.tabs.len() == 1 {
            return None;
        }

        self.tabs.remove(index);

        // Adjust active index
        if self.active_index >= self.tabs.len() {
            self.active_index = self.tabs.len() - 1;
        } else if self.active_index > index {
            self.active_index -= 1;
        }

        self.tabs[self.active_index].active = true;
        Some(self.active_index)
    }

    /// Switch to a specific tab by index
    pub fn switch_to(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.tabs[self.active_index].active = false;
            self.active_index = index;
            self.tabs[self.active_index].active = true;
        }
    }

    /// Switch to the next tab
    pub fn next(&mut self) {
        if self.tabs.len() > 1 {
            let next_index = (self.active_index + 1) % self.tabs.len();
            self.switch_to(next_index);
        }
    }

    /// Switch to the previous tab
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

    /// Update the title of a tab
    pub fn set_title(&mut self, index: usize, title: &str) {
        if index < self.tabs.len() {
            self.tabs[index].title = title.to_string();
        }
    }

    /// Update the title of the active tab
    pub fn set_active_title(&mut self, title: &str) {
        self.set_title(self.active_index, title);
    }

    /// Resize all tabs
    pub fn resize(&mut self, size: WinSize) {
        self.default_size = size;
        for tab in &mut self.tabs {
            tab.grid.resize(size);
        }
    }

    /// Get tab titles for rendering
    pub fn titles(&self) -> Vec<&str> {
        self.tabs.iter().map(|t| t.title.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tab_manager_new() {
        let size = WinSize { cols: 80, rows: 24 };
        let manager = TabManager::new(size);
        assert_eq!(manager.len(), 1);
        assert_eq!(manager.active_index(), 0);
        assert_eq!(manager.active().title, "Terminal");
    }

    #[test]
    fn test_new_tab() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut manager = TabManager::new(size);
        
        let idx = manager.new_tab();
        assert_eq!(idx, 1);
        assert_eq!(manager.len(), 2);
        assert_eq!(manager.active_index(), 1);
        assert_eq!(manager.active().title, "Terminal 2");
    }

    #[test]
    fn test_close_tab() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut manager = TabManager::new(size);
        manager.new_tab();
        manager.new_tab();
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
        let size = WinSize { cols: 80, rows: 24 };
        let mut manager = TabManager::new(size);
        
        let result = manager.close_current();
        assert!(result.is_none());
        assert_eq!(manager.len(), 1);
    }

    #[test]
    fn test_switch_tabs() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut manager = TabManager::new(size);
        manager.new_tab();
        manager.new_tab();

        manager.switch_to(2);
        assert_eq!(manager.active_index(), 2);

        manager.next();
        assert_eq!(manager.active_index(), 0);

        manager.prev();
        assert_eq!(manager.active_index(), 2);
    }

    #[test]
    fn test_set_title() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut manager = TabManager::new(size);
        
        manager.set_active_title("My Tab");
        assert_eq!(manager.active().title, "My Tab");

        manager.set_title(0, "Renamed");
        assert_eq!(manager.active().title, "Renamed");
    }

    #[test]
    fn test_resize_all_tabs() {
        let size = WinSize { cols: 80, rows: 24 };
        let mut manager = TabManager::new(size);
        manager.new_tab();

        let new_size = WinSize { cols: 120, rows: 40 };
        manager.resize(new_size);

        assert_eq!(manager.default_size, new_size);
        // Both tabs should be resized (we can't directly check grid size without exposing it)
    }
}
