//! Tab bar rendering for the terminal.

use crate::grid::Color;

/// A tab in the tab bar
#[derive(Debug, Clone)]
pub struct TabBarTab {
    /// Tab title
    pub title: String,
    /// Whether this tab is active
    pub active: bool,
    /// Tab index
    pub index: usize,
}

/// Tab bar renderer
pub struct TabBar {
    /// Tabs to display
    pub tabs: Vec<TabBarTab>,
    /// Height of the tab bar in pixels
    pub height: u32,
    /// Background color
    pub bg_color: Color,
    /// Active tab color
    pub active_color: Color,
    /// Inactive tab color
    pub inactive_color: Color,
    /// Text color
    pub text_color: Color,
}

impl TabBar {
    /// Create a new tab bar
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            height: 30, // Default height
            bg_color: Color::Rgb(24, 24, 37), // #181825
            active_color: Color::Rgb(137, 180, 250), // #89B4FA
            inactive_color: Color::Rgb(69, 71, 90), // #45475A
            text_color: Color::Rgb(205, 214, 244), // #CDD6F4
        }
    }

    /// Update tabs from tab manager
    pub fn update_tabs(&mut self, titles: &[&str], active_index: usize) {
        self.tabs = titles.iter().enumerate().map(|(i, &title)| {
            TabBarTab {
                title: title.to_string(),
                active: i == active_index,
                index: i,
            }
        }).collect();
    }

    /// Get tab bar height
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Check if a click is on a tab, return tab index
    pub fn tab_at_position(&self, x: f64, _y: f64, cell_width: u32) -> Option<usize> {
        let tab_width = 150; // pixels per tab
        let tab_index = (x / tab_width as f64) as usize;
        if tab_index < self.tabs.len() {
            Some(tab_index)
        } else {
            None
        }
    }

    /// Check if a click is on a close button
    pub fn close_button_at_position(&self, x: f64, y: f64, cell_width: u32) -> Option<usize> {
        let tab_width = 150;
        let close_button_width = 20;
        let tab_index = (x / tab_width as f64) as usize;
        
        if tab_index < self.tabs.len() {
            let tab_x = x % tab_width as f64;
            // Close button is at the right side of the tab
            if tab_x > (tab_width - close_button_width) as f64 {
                return Some(tab_index);
            }
        }
        None
    }

    /// Generate vertex data for rendering
    pub fn generate_vertices(&self, screen_width: u32) -> Vec<TabBarVertex> {
        let mut vertices = Vec::new();
        let tab_width = 150;
        
        for (i, tab) in self.tabs.iter().enumerate() {
            let x = i as f32 * tab_width as f32;
            let y = 0.0;
            let w = tab_width as f32;
            let h = self.height as f32;
            
            // Tab background
            let color = if tab.active {
                &self.active_color
            } else {
                &self.inactive_color
            };
            
            vertices.push(TabBarVertex {
                position: [x, y],
                size: [w, h],
                color: color_to_floats(color),
            });
            
            // Close button (X)
            let close_x = x + w - 25.0;
            let close_y = y + 5.0;
            vertices.push(TabBarVertex {
                position: [close_x, close_y],
                size: [15.0, 15.0],
                color: [0.7, 0.7, 0.7, 1.0],
            });
        }
        
        vertices
    }
}

/// Vertex data for tab bar rendering
#[derive(Debug, Clone, Copy)]
pub struct TabBarVertex {
    pub position: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
}

fn color_to_floats(color: &Color) -> [f32; 4] {
    match color {
        Color::Default => [0.0, 0.0, 0.0, 1.0],
        Color::Rgb(r, g, b) => [*r as f32 / 255.0, *g as f32 / 255.0, *b as f32 / 255.0, 1.0],
        Color::Indexed(i) => {
            // Simple fallback for indexed colors
            match i {
                0 => [0.0, 0.0, 0.0, 1.0],
                1 => [1.0, 0.0, 0.0, 1.0],
                2 => [0.0, 1.0, 0.0, 1.0],
                3 => [1.0, 1.0, 0.0, 1.0],
                4 => [0.0, 0.0, 1.0, 1.0],
                5 => [1.0, 0.0, 1.0, 1.0],
                6 => [0.0, 1.0, 1.0, 1.0],
                7 => [1.0, 1.0, 1.0, 1.0],
                _ => [0.5, 0.5, 0.5, 1.0],
            }
        }
    }
}

impl Default for TabBar {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tab_bar_new() {
        let tab_bar = TabBar::new();
        assert_eq!(tab_bar.height, 30);
        assert!(tab_bar.tabs.is_empty());
    }

    #[test]
    fn test_update_tabs() {
        let mut tab_bar = TabBar::new();
        let titles = vec!["Terminal 1", "Terminal 2"];
        tab_bar.update_tabs(&titles, 0);
        assert_eq!(tab_bar.tabs.len(), 2);
        assert!(tab_bar.tabs[0].active);
        assert!(!tab_bar.tabs[1].active);
    }

    #[test]
    fn test_tab_at_position() {
        let mut tab_bar = TabBar::new();
        let titles = vec!["Tab 1", "Tab 2"];
        tab_bar.update_tabs(&titles, 0);
        
        // Click on first tab
        assert_eq!(tab_bar.tab_at_position(50.0, 10.0, 8), Some(0));
        // Click on second tab
        assert_eq!(tab_bar.tab_at_position(200.0, 10.0, 8), Some(1));
        // Click outside tabs
        assert_eq!(tab_bar.tab_at_position(500.0, 10.0, 8), None);
    }
}
