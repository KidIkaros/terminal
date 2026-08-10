//! Clipboard support — system clipboard + OSC52 protocol.

use arboard::Clipboard;
use base64::Engine;

/// Clipboard manager handling system clipboard and OSC52.
pub struct ClipboardManager {
    clipboard: Option<Clipboard>,
}

impl ClipboardManager {
    pub fn new() -> Self {
        let clipboard = Clipboard::new().ok();
        ClipboardManager { clipboard }
    }

    /// Copy text to system clipboard.
    pub fn copy(&mut self, text: &str) -> bool {
        if let Some(clip) = &mut self.clipboard {
            if clip.set_text(text).is_ok() {
                return true;
            }
        }
        false
    }

    /// Paste text from system clipboard.
    pub fn paste(&mut self) -> Option<String> {
        if let Some(clip) = &mut self.clipboard {
            if let Ok(text) = clip.get_text() {
                return Some(text);
            }
        }
        None
    }

    /// Clear system clipboard.
    pub fn clear(&mut self) -> bool {
        if let Some(clip) = &mut self.clipboard {
            if clip.clear().is_ok() {
                return true;
            }
        }
        false
    }

    /// Generate OSC52 set clipboard escape sequence.
    /// Usage: write this string to PTY to set remote clipboard.
    pub fn osc52_set(content: &str) -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode(content.as_bytes());
        format!("\x1b]52;c;{}\x07", encoded)
    }

    /// Generate OSC52 query clipboard escape sequence.
    /// Usage: write this string to PTY to request clipboard contents.
    pub fn osc52_query() -> &'static str {
        "\x1b]52;c;?\x07"
    }

    /// Parse and handle OSC52 response from terminal.
    /// Returns the decoded clipboard content if valid.
    pub fn parse_osc52_response(params: &[Vec<u8>]) -> Option<String> {
        if params.len() >= 2 {
            let board = &params[0];
            let data = &params[1];
            
            // Check if it's clipboard buffer ('c') or primary ('p')
            if (board == b"c" || board == b"p") && data != b"?" {
                if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(data) {
                    if let Ok(text) = String::from_utf8(decoded) {
                        return Some(text);
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_osc52_set_format() {
        let result = ClipboardManager::osc52_set("Hello");
        // Should be ESC]52;c;<base64>BEL
        assert!(result.starts_with("\x1b]52;c;"));
        assert!(result.ends_with('\x07'));
    }

    #[test]
    fn test_osc52_query_format() {
        let result = ClipboardManager::osc52_query();
        assert_eq!(result, "\x1b]52;c;?\x07");
    }

    #[test]
    fn test_osc52_parse_response() {
        // "Hello" in base64 is "SGVsbG8="
        let params = vec![b"c".to_vec(), b"SGVsbG8=".to_vec()];
        let result = ClipboardManager::parse_osc52_response(&params);
        assert_eq!(result, Some("Hello".to_string()));
    }

    #[test]
    fn test_osc52_parse_invalid_base64() {
        let params = vec![b"c".to_vec(), b"!!!invalid!!!".to_vec()];
        let result = ClipboardManager::parse_osc52_response(&params);
        assert!(result.is_none());
    }

    #[test]
    fn test_osc52_parse_query() {
        // Query should return None (not actual content)
        let params = vec![b"c".to_vec(), b"?".to_vec()];
        let result = ClipboardManager::parse_osc52_response(&params);
        assert!(result.is_none());
    }

    #[test]
    fn test_osc52_parse_wrong_board() {
        let params = vec![b"x".to_vec(), b"SGVsbG8=".to_vec()];
        let result = ClipboardManager::parse_osc52_response(&params);
        assert!(result.is_none());
    }

    #[test]
    fn test_osc52_parse_utf8() {
        // "Hello 🌍" in base64
        let original = "Hello 🌍";
        let encoded = base64::engine::general_purpose::STANDARD.encode(original.as_bytes());
        let params = vec![b"c".to_vec(), encoded.into_bytes()];
        let result = ClipboardManager::parse_osc52_response(&params);
        assert_eq!(result, Some(original.to_string()));
    }
}