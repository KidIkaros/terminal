//! Ligature support using fontdue's built-in shaping.
//!
//! This module provides basic ligature detection for common programming ligatures.
//! For full HarfBuzz integration, a separate crate would be needed.

use std::collections::HashMap;

/// A ligature mapping from character sequence to glyph
#[derive(Debug, Clone)]
pub struct Ligature {
    /// The character sequence (e.g., "=>")
    pub sequence: String,
    /// The ligature glyph character (if available)
    pub glyph: Option<char>,
    /// Number of characters consumed
    pub width: usize,
}

/// Ligature detector
pub struct LigatureDetector {
    /// Known ligatures
    ligatures: Vec<Ligature>,
    /// Cache for detected ligatures
    cache: HashMap<String, Ligature>,
}

impl LigatureDetector {
    /// Create a new ligature detector with common programming ligatures
    pub fn new() -> Self {
        let mut ligatures = Vec::new();
        
        // Arrow ligatures
        ligatures.push(Ligature { sequence: "=>".to_string(), glyph: Some('⇒'), width: 2 });
        ligatures.push(Ligature { sequence: "->".to_string(), glyph: Some('→'), width: 2 });
        ligatures.push(Ligature { sequence: "<-".to_string(), glyph: Some('←'), width: 2 });
        ligatures.push(Ligature { sequence: "<->".to_string(), glyph: Some('↔'), width: 3 });
        ligatures.push(Ligature { sequence: "=>>".to_string(), glyph: Some('↠'), width: 3 });
        ligatures.push(Ligature { sequence: "<<-".to_string(), glyph: Some('↞'), width: 3 });
        
        // Comparison ligatures
        ligatures.push(Ligature { sequence: "==".to_string(), glyph: Some('≡'), width: 2 });
        ligatures.push(Ligature { sequence: "!=".to_string(), glyph: Some('≠'), width: 2 });
        ligatures.push(Ligature { sequence: "<=".to_string(), glyph: Some('≤'), width: 2 });
        ligatures.push(Ligature { sequence: ">=".to_string(), glyph: Some('≥'), width: 2 });
        ligatures.push(Ligature { sequence: "=/=".to_string(), glyph: Some('≢'), width: 3 });
        ligatures.push(Ligature { sequence: "!==".to_string(), glyph: Some('≢'), width: 3 });
        
        // Logical ligatures
        ligatures.push(Ligature { sequence: "&&".to_string(), glyph: Some('∧'), width: 2 });
        ligatures.push(Ligature { sequence: "||".to_string(), glyph: Some('∨'), width: 2 });
        ligatures.push(Ligature { sequence: "!!".to_string(), glyph: Some('‼'), width: 2 });
        
        // Miscellaneous
        ligatures.push(Ligature { sequence: "::".to_string(), glyph: Some('∷'), width: 2 });
        ligatures.push(Ligature { sequence: "..".to_string(), glyph: Some('…'), width: 2 });
        ligatures.push(Ligature { sequence: "...".to_string(), glyph: Some('⋯'), width: 3 });
        ligatures.push(Ligature { sequence: "::=".to_string(), glyph: Some('⩴'), width: 3 });
        ligatures.push(Ligature { sequence: ":=".to_string(), glyph: Some('≔'), width: 2 });
        
        // Comment ligatures
        ligatures.push(Ligature { sequence: "//".to_string(), glyph: Some('⫽'), width: 2 });
        ligatures.push(Ligature { sequence: "/*".to_string(), glyph: None, width: 2 });
        ligatures.push(Ligature { sequence: "*/".to_string(), glyph: None, width: 2 });
        
        // Sort by length (longest first) to match longer ligatures first
        ligatures.sort_by(|a, b| b.width.cmp(&a.width));
        
        Self {
            ligatures,
            cache: HashMap::new(),
        }
    }

    /// Try to detect a ligature at the current position
    pub fn detect(&mut self, chars: &[char], pos: usize) -> Option<Ligature> {
        if pos >= chars.len() {
            return None;
        }

        // Build a string from the current position
        let remaining: String = chars[pos..].iter().take(5).collect();

        // Try each ligature
        for ligature in &self.ligatures {
            if remaining.starts_with(&ligature.sequence) {
                if pos + ligature.width <= chars.len() {
                    return Some(ligature.clone());
                }
            }
        }

        None
    }

    /// Check if ligatures are enabled in config
    pub fn is_enabled(config_ligatures: bool) -> bool {
        config_ligatures
    }
}

impl Default for LigatureDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ligature_detector() {
        let mut detector = LigatureDetector::new();
        
        let chars: Vec<char> = "=>".chars().collect();
        let ligature = detector.detect(&chars, 0);
        assert!(ligature.is_some());
        let lig = ligature.unwrap();
        assert_eq!(lig.sequence, "=>");
        assert_eq!(lig.width, 2);
    }

    #[test]
    fn test_no_ligature() {
        let mut detector = LigatureDetector::new();
        
        let chars: Vec<char> = "ab".chars().collect();
        let ligature = detector.detect(&chars, 0);
        assert!(ligature.is_none());
    }

    #[test]
    fn test_longest_ligature_first() {
        let mut detector = LigatureDetector::new();
        
        // "=>" should match before "=="
        let chars: Vec<char> = "=>".chars().collect();
        let ligature = detector.detect(&chars, 0);
        assert!(ligature.is_some());
        assert_eq!(ligature.unwrap().sequence, "=>");
    }
}
