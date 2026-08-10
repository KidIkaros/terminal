//! Image rendering support for Kitty graphics protocol.
//!
//! The Kitty graphics protocol allows displaying images inline in the terminal.
//! Format: ESC_G ... ESC\
//!
//! Key commands:
//! - a=T: Display image from file
//! - a=t: Display image from file (transmit)
//! - a=p: Display previously transmitted image
//! - a=d: Delete image
//! - a=u: Transmit and display
//! - a=q: Query image status

use std::collections::HashMap;

/// Image placement position
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImagePlacement {
    /// Place at cursor position
    Cursor,
    /// Place at specific coordinates
    Coordinates(i32, i32),
    /// Place relative to cursor
    Relative(i32, i32),
}

/// An image loaded into the terminal
#[derive(Debug, Clone)]
pub struct TerminalImage {
    /// Image ID (assigned by terminal)
    pub id: u32,
    /// Image data (RGBA)
    pub data: Vec<u8>,
    /// Image width in pixels
    pub width: u32,
    /// Image height in pixels
    pub height: u32,
    /// Number of columns the image occupies
    pub cols: u32,
    /// Number of rows the image occupies
    pub rows: u32,
    /// Placement position
    pub placement: ImagePlacement,
}

/// Image storage for the terminal
pub struct ImageStore {
    /// Loaded images by ID
    images: HashMap<u32, TerminalImage>,
    /// Next available ID
    next_id: u32,
}

impl ImageStore {
    /// Create a new image store
    pub fn new() -> Self {
        Self {
            images: HashMap::new(),
            next_id: 1,
        }
    }

    /// Add a new image and return its ID
    pub fn add_image(&mut self, image: TerminalImage) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.images.insert(id, image);
        id
    }

    /// Get an image by ID
    pub fn get_image(&self, id: u32) -> Option<&TerminalImage> {
        self.images.get(&id)
    }

    /// Remove an image by ID
    pub fn remove_image(&mut self, id: u32) -> Option<TerminalImage> {
        self.images.remove(&id)
    }

    /// Clear all images
    pub fn clear(&mut self) {
        self.images.clear();
    }

    /// Get all images
    pub fn images(&self) -> &HashMap<u32, TerminalImage> {
        &self.images
    }
}

impl Default for ImageStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse Kitty graphics protocol command
pub fn parse_kitty_command(params: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();
    
    for part in params.split(',') {
        if let Some((key, value)) = part.split_once('=') {
            result.insert(key.to_string(), value.to_string());
        }
    }
    
    result
}

/// Create a Kitty graphics command
pub fn create_kitty_command(command: &str, data: Option<&[u8]>) -> String {
    if let Some(data) = data {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(data);
        format!("\x1b_G{},{}\x1b\\", command, encoded)
    } else {
        format!("\x1b_G{}\x1b\\", command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_store() {
        let mut store = ImageStore::new();
        
        let image = TerminalImage {
            id: 0,
            data: vec![0; 100],
            width: 10,
            height: 10,
            cols: 5,
            rows: 3,
            placement: ImagePlacement::Cursor,
        };
        
        let id = store.add_image(image);
        assert_eq!(id, 1);
        
        let retrieved = store.get_image(id);
        assert!(retrieved.is_some());
        
        store.remove_image(id);
        assert!(store.get_image(id).is_none());
    }

    #[test]
    fn test_parse_kitty_command() {
        let params = "a=T,i=1,t=f,s=1024,v=768";
        let result = parse_kitty_command(params);
        
        assert_eq!(result.get("a"), Some(&"T".to_string()));
        assert_eq!(result.get("i"), Some(&"1".to_string()));
        assert_eq!(result.get("t"), Some(&"f".to_string()));
        assert_eq!(result.get("s"), Some(&"1024".to_string()));
        assert_eq!(result.get("v"), Some(&"768".to_string()));
    }
}
