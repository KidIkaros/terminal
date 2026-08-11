use regex::Regex;

/// Represents a search result in the terminal grid
#[derive(Debug, Clone, PartialEq)]
pub struct SearchMatch {
    pub row: usize,
    pub start_col: usize,
    pub end_col: usize,
}

/// Search mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Forward search (Ctrl+F)
    Forward,
    /// Reverse search (Ctrl+R) - searches history/output
    Reverse,
}

/// Search state
#[derive(Debug)]
pub struct SearchState {
    /// Whether search mode is active
    pub active: bool,
    /// Current search query
    pub query: String,
    /// Compiled regex (if query is valid)
    pub regex: Option<Regex>,
    /// All matches found
    pub matches: Vec<SearchMatch>,
    /// Index of currently highlighted match
    pub current_match: usize,
    /// Whether search is case-sensitive
    pub case_sensitive: bool,
    /// Whether to use regex mode
    pub regex_mode: bool,
    /// Search mode (forward or reverse)
    pub mode: SearchMode,
    /// Search history for reverse search
    pub search_history: Vec<String>,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            active: false,
            query: String::new(),
            regex: None,
            matches: Vec::new(),
            current_match: 0,
            case_sensitive: false,
            regex_mode: false,
            mode: SearchMode::Forward,
            search_history: Vec::new(),
        }
    }

    /// Activate search mode
    pub fn activate(&mut self) {
        self.active = true;
        self.mode = SearchMode::Forward;
        self.query.clear();
        self.regex = None;
        self.matches.clear();
        self.current_match = 0;
    }

    /// Activate reverse search mode
    pub fn activate_reverse(&mut self) {
        self.active = true;
        self.mode = SearchMode::Reverse;
        self.query.clear();
        self.regex = None;
        self.matches.clear();
        self.current_match = 0;
    }

    /// Deactivate search mode
    pub fn deactivate(&mut self) {
        self.active = false;
        self.query.clear();
        self.regex = None;
        self.matches.clear();
        self.current_match = 0;
    }

    /// Update search query and recompile regex
    pub fn update_query(&mut self, query: &str) {
        self.query = query.to_string();
        self.compile_regex();
        self.current_match = 0;
    }

    /// Compile the current query as a regex
    fn compile_regex(&mut self) {
        if self.query.is_empty() {
            self.regex = None;
            return;
        }

        let pattern = if self.regex_mode {
            self.query.clone()
        } else {
            // Escape special regex characters for literal search
            regex::escape(&self.query)
        };

        let mut builder = regex::RegexBuilder::new(&pattern);
        if !self.case_sensitive {
            builder.case_insensitive(true);
        }

        self.regex = builder.build().ok();
    }

    /// Toggle case sensitivity
    pub fn toggle_case_sensitive(&mut self) {
        self.case_sensitive = !self.case_sensitive;
        self.compile_regex();
    }

    /// Toggle regex mode
    pub fn toggle_regex_mode(&mut self) {
        self.regex_mode = !self.regex_mode;
        self.compile_regex();
    }

    /// Search through terminal lines and populate matches
    pub fn search(&mut self, lines: &[String]) {
        self.matches.clear();
        self.current_match = 0;

        let regex = match &self.regex {
            Some(r) => r,
            None => return,
        };

        for (row, line) in lines.iter().enumerate() {
            for mat in regex.find_iter(line) {
                self.matches.push(SearchMatch {
                    row,
                    start_col: mat.start(),
                    end_col: mat.end(),
                });
            }
        }
    }

    /// Search through all lines including scrollback (for reverse search)
    pub fn search_with_scrollback(&mut self, scrollback: &[Vec<String>], visible_lines: &[String]) {
        self.matches.clear();
        self.current_match = 0;

        let regex = match &self.regex {
            Some(r) => r,
            None => return,
        };

        // Search scrollback first (oldest to newest)
        for (scroll_idx, lines) in scrollback.iter().enumerate() {
            for (row, line) in lines.iter().enumerate() {
                for mat in regex.find_iter(line) {
                    self.matches.push(SearchMatch {
                        row: scroll_idx * 1000 + row, // Offset by scroll index
                        start_col: mat.start(),
                        end_col: mat.end(),
                    });
                }
            }
        }

        // Then search visible lines
        let offset = scrollback.len() * 1000;
        for (row, line) in visible_lines.iter().enumerate() {
            for mat in regex.find_iter(line) {
                self.matches.push(SearchMatch {
                    row: offset + row,
                    start_col: mat.start(),
                    end_col: mat.end(),
                });
            }
        }

        // For reverse search, start from the end
        if self.mode == SearchMode::Reverse && !self.matches.is_empty() {
            self.current_match = self.matches.len() - 1;
        }
    }

    /// Get current match (if any)
    pub fn current(&self) -> Option<&SearchMatch> {
        self.matches.get(self.current_match)
    }

    /// Move to next match
    pub fn next(&mut self) -> Option<&SearchMatch> {
        if self.matches.is_empty() {
            return None;
        }
        self.current_match = (self.current_match + 1) % self.matches.len();
        self.current()
    }

    /// Move to previous match
    pub fn prev(&mut self) -> Option<&SearchMatch> {
        if self.matches.is_empty() {
            return None;
        }
        if self.current_match == 0 {
            self.current_match = self.matches.len() - 1;
        } else {
            self.current_match -= 1;
        }
        self.current()
    }

    /// Get total number of matches
    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// Get current match index (1-based for display)
    pub fn current_index(&self) -> usize {
        if self.matches.is_empty() {
            0
        } else {
            self.current_match + 1
        }
    }
}

impl Default for SearchState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_state_new() {
        let state = SearchState::new();
        assert!(!state.active);
        assert!(state.query.is_empty());
        assert!(state.matches.is_empty());
    }

    #[test]
    fn test_activate_deactivate() {
        let mut state = SearchState::new();
        state.activate();
        assert!(state.active);

        state.deactivate();
        assert!(!state.active);
        assert!(state.query.is_empty());
    }

    #[test]
    fn test_simple_search() {
        let mut state = SearchState::new();
        state.activate();
        state.update_query("hello");

        let lines = vec![
            "hello world".to_string(),
            "foo bar".to_string(),
            "hello there".to_string(),
        ];

        state.search(&lines);
        assert_eq!(state.matches.len(), 2);
        assert_eq!(state.matches[0].row, 0);
        assert_eq!(state.matches[0].start_col, 0);
        assert_eq!(state.matches[0].end_col, 5);
        assert_eq!(state.matches[1].row, 2);
    }

    #[test]
    fn test_case_insensitive_search() {
        let mut state = SearchState::new();
        state.activate();
        state.update_query("hello");

        let lines = vec!["HELLO world".to_string(), "Hello there".to_string()];

        state.search(&lines);
        assert_eq!(state.matches.len(), 2);
    }

    #[test]
    fn test_case_sensitive_search() {
        let mut state = SearchState::new();
        state.activate();
        state.case_sensitive = true;
        state.update_query("hello");

        let lines = vec![
            "HELLO world".to_string(),
            "Hello there".to_string(),
            "hello world".to_string(),
        ];

        state.search(&lines);
        assert_eq!(state.matches.len(), 1);
        assert_eq!(state.matches[0].row, 2);
    }

    #[test]
    fn test_regex_search() {
        let mut state = SearchState::new();
        state.activate();
        state.regex_mode = true;
        state.update_query(r"\d+");

        let lines = vec![
            "foo 123 bar".to_string(),
            "no numbers".to_string(),
            "456 test".to_string(),
        ];

        state.search(&lines);
        assert_eq!(state.matches.len(), 2);
        assert_eq!(state.matches[0].start_col, 4);
        assert_eq!(state.matches[0].end_col, 7);
    }

    #[test]
    fn test_navigation() {
        let mut state = SearchState::new();
        state.activate();
        state.update_query("test");

        let lines = vec![
            "test one".to_string(),
            "test two".to_string(),
            "test three".to_string(),
        ];

        state.search(&lines);
        assert_eq!(state.matches.len(), 3);

        // Start at 0
        assert_eq!(state.current_match, 0);

        // Next goes to 1
        state.next();
        assert_eq!(state.current_match, 1);

        // Next goes to 2
        state.next();
        assert_eq!(state.current_match, 2);

        // Next wraps to 0
        state.next();
        assert_eq!(state.current_match, 0);

        // Prev wraps to 2
        state.prev();
        assert_eq!(state.current_match, 2);

        // Prev goes to 1
        state.prev();
        assert_eq!(state.current_match, 1);
    }

    #[test]
    fn test_empty_search() {
        let mut state = SearchState::new();
        state.activate();
        state.update_query("");

        let lines = vec!["hello world".to_string()];
        state.search(&lines);
        assert!(state.matches.is_empty());
    }
}
