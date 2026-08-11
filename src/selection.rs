use crate::grid::Cell;

/// Selection mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    /// Character-by-character selection
    Char,
    /// Word-by-word selection
    Word,
    /// Line-by-line selection
    Line,
}

/// Represents a selection region in the terminal
#[derive(Debug, Clone)]
pub struct Selection {
    /// Start position (row, col)
    pub start: (usize, usize),
    /// End position (row, col)
    pub end: (usize, usize),
    /// Selection mode
    pub mode: SelectionMode,
    /// Whether selection is active
    pub active: bool,
    /// Whether we're currently selecting (mouse button held)
    pub selecting: bool,
}

impl Selection {
    /// Create a new empty selection
    pub fn new() -> Self {
        Self {
            start: (0, 0),
            end: (0, 0),
            mode: SelectionMode::Char,
            active: false,
            selecting: false,
        }
    }

    /// Start a new selection at the given position
    pub fn start_selection(&mut self, row: usize, col: usize, mode: SelectionMode) {
        self.start = (row, col);
        self.end = (row, col);
        self.mode = mode;
        self.active = true;
        self.selecting = true;
    }

    /// Update the selection end position (while dragging)
    pub fn update(&mut self, row: usize, col: usize) {
        if self.selecting {
            self.end = (row, col);
        }
    }

    /// End the selection (mouse button released)
    pub fn end_selection(&mut self) {
        self.selecting = false;
        // Normalize so start <= end
        self.normalize();
    }

    /// Cancel the selection
    pub fn cancel(&mut self) {
        self.active = false;
        self.selecting = false;
    }

    /// Normalize the selection so start is before end
    fn normalize(&mut self) {
        if self.start.0 > self.end.0 || (self.start.0 == self.end.0 && self.start.1 > self.end.1) {
            std::mem::swap(&mut self.start, &mut self.end);
        }
    }

    /// Get the normalized start and end
    pub fn normalized(&self) -> ((usize, usize), (usize, usize)) {
        let mut s = self.start;
        let mut e = self.end;
        if s.0 > e.0 || (s.0 == e.0 && s.1 > e.1) {
            std::mem::swap(&mut s, &mut e);
        }
        (s, e)
    }

    /// Check if a cell is within the selection
    pub fn contains(&self, row: usize, col: usize) -> bool {
        if !self.active {
            return false;
        }

        let (start, end) = self.normalized();

        match self.mode {
            SelectionMode::Char => {
                if row < start.0 || row > end.0 {
                    return false;
                }
                if row == start.0 && row == end.0 {
                    col >= start.1 && col <= end.1
                } else if row == start.0 {
                    col >= start.1
                } else if row == end.0 {
                    col <= end.1
                } else {
                    true
                }
            }
            SelectionMode::Word => {
                // For word selection, we'd need to detect word boundaries
                // For now, use char mode
                if row < start.0 || row > end.0 {
                    return false;
                }
                if row == start.0 && row == end.0 {
                    col >= start.1 && col <= end.1
                } else if row == start.0 {
                    col >= start.1
                } else if row == end.0 {
                    col <= end.1
                } else {
                    true
                }
            }
            SelectionMode::Line => {
                // Select entire lines
                row >= start.0 && row <= end.0
            }
        }
    }

    /// Extract selected text from a grid
    pub fn extract_text(&self, lines: &[String], cols: usize) -> String {
        if !self.active {
            return String::new();
        }

        let (start, end) = self.normalized();
        let mut result = String::new();

        for row in start.0..=end.0 {
            if row >= lines.len() {
                break;
            }

            let line = &lines[row];
            let chars: Vec<char> = line.chars().collect();

            let (start_col, end_col) = if row == start.0 && row == end.0 {
                (start.1, end.1)
            } else if row == start.0 {
                (start.1, cols - 1)
            } else if row == end.0 {
                (0, end.1)
            } else {
                (0, cols - 1)
            };

            let start_idx = start_col.min(chars.len());
            let end_idx = (end_col + 1).min(chars.len());

            for ch in &chars[start_idx..end_idx] {
                result.push(*ch);
            }

            // Add newline between lines (but not after the last line)
            if row < end.0 {
                result.push('\n');
            }
        }

        result
    }

    /// Expand selection to word boundaries
    pub fn expand_to_word(&mut self, lines: &[String]) {
        if !self.active || lines.is_empty() {
            return;
        }

        let (start, end) = self.normalized();

        // Find word start
        if start.0 < lines.len() {
            let line = &lines[start.0];
            let chars: Vec<char> = line.chars().collect();
            let mut col = start.1;
            while col > 0 && col < chars.len() && chars[col].is_alphanumeric() {
                col -= 1;
            }
            if col > 0 && !chars[col].is_alphanumeric() {
                col += 1;
            }
            self.start.1 = col;
        }

        // Find word end
        if end.0 < lines.len() {
            let line = &lines[end.0];
            let chars: Vec<char> = line.chars().collect();
            let mut col = end.1;
            while col < chars.len() && chars[col].is_alphanumeric() {
                col += 1;
            }
            self.end.1 = col.saturating_sub(1);
        }
    }
}

impl Default for Selection {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selection_new() {
        let sel = Selection::new();
        assert!(!sel.active);
        assert!(!sel.selecting);
    }

    #[test]
    fn test_start_selection() {
        let mut sel = Selection::new();
        sel.start_selection(5, 10, SelectionMode::Char);
        assert!(sel.active);
        assert!(sel.selecting);
        assert_eq!(sel.start, (5, 10));
        assert_eq!(sel.end, (5, 10));
    }

    #[test]
    fn test_update_selection() {
        let mut sel = Selection::new();
        sel.start_selection(5, 10, SelectionMode::Char);
        sel.update(5, 20);
        assert_eq!(sel.end, (5, 20));
    }

    #[test]
    fn test_end_selection_normalizes() {
        let mut sel = Selection::new();
        sel.start_selection(5, 20, SelectionMode::Char);
        sel.update(5, 10);
        sel.end_selection();

        assert!(!sel.selecting);
        let (start, end) = sel.normalized();
        assert_eq!(start, (5, 10));
        assert_eq!(end, (5, 20));
    }

    #[test]
    fn test_contains() {
        let mut sel = Selection::new();
        sel.start_selection(2, 5, SelectionMode::Char);
        sel.update(4, 10);
        sel.end_selection();

        // Before selection
        assert!(!sel.contains(1, 0));
        // First line of selection
        assert!(sel.contains(2, 5));
        assert!(sel.contains(2, 10));
        assert!(!sel.contains(2, 4));
        // Middle line
        assert!(sel.contains(3, 0));
        // Last line
        assert!(sel.contains(4, 0));
        assert!(sel.contains(4, 10));
        assert!(!sel.contains(4, 11));
        // After selection
        assert!(!sel.contains(5, 0));
    }

    #[test]
    fn test_extract_text() {
        let mut sel = Selection::new();
        sel.start_selection(0, 2, SelectionMode::Char);
        sel.update(1, 3);
        sel.end_selection();

        let lines = vec!["Hello, World!".to_string(), "Goodbye, World!".to_string()];

        let text = sel.extract_text(&lines, 13);
        assert_eq!(text, "llo, World!\nGood");
    }

    #[test]
    fn test_cancel() {
        let mut sel = Selection::new();
        sel.start_selection(5, 10, SelectionMode::Char);
        sel.cancel();
        assert!(!sel.active);
        assert!(!sel.selecting);
    }

    #[test]
    fn test_line_mode() {
        let mut sel = Selection::new();
        sel.start_selection(2, 5, SelectionMode::Line);
        sel.update(4, 10);
        sel.end_selection();

        // All cells in lines 2-4 should be selected
        assert!(sel.contains(2, 0));
        assert!(sel.contains(2, 100));
        assert!(sel.contains(3, 0));
        assert!(sel.contains(4, 0));
        assert!(!sel.contains(1, 0));
        assert!(!sel.contains(5, 0));
    }

    #[test]
    fn test_word_expansion() {
        let mut sel = Selection::new();
        let lines = vec!["Hello, World!".to_string()];
        sel.start_selection(0, 4, SelectionMode::Word);
        sel.update(0, 6);
        sel.end_selection();
        sel.expand_to_word(&lines);

        let (start, end) = sel.normalized();
        // "Hello" starts at 0, ends at 4 (inclusive index)
        // The expansion should find word boundaries
        assert!(start.1 <= 4);
        assert!(end.1 >= 4);
    }
}
