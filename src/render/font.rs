//! Font rasterization and glyph atlas — part of Layer 4.
//!
//! Uses `fontdue` (pure Rust) to rasterize glyphs into a grayscale bitmap,
//! then packs them into a GPU texture atlas via a simple shelf-packing
//! algorithm. The atlas is a single `wgpu::Texture` (format R8Unorm).
//!
//! ## Shelf-packing
//!
//! ```text
//! ┌───────────────────────────────┐  ← atlas (1024×1024)
//! │ [A][B][C][D]                  │  ← shelf 0, height = max glyph height in row
//! │ [E][F]                        │  ← shelf 1
//! │                               │
//! └───────────────────────────────┘
//! ```

use std::collections::HashMap;

use fontdue::{Font, FontSettings};

/// UV rectangle within the atlas (0.0–1.0 normalized).
#[derive(Debug, Clone, Copy)]
pub struct AtlasRegion {
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    /// Pixel metrics needed to position the glyph within its cell.
    pub metrics: GlyphMetrics,
}

#[derive(Debug, Clone, Copy)]
pub struct GlyphMetrics {
    pub width: u32,
    pub height: u32,
    /// Offset from the top of the cell to the top of the glyph bitmap.
    pub ymin: i32,
    /// Offset from the left of the cell to the left of the glyph bitmap.
    pub xmin: i32,
}

pub const ATLAS_SIZE: u32 = 1024;

pub struct GlyphAtlas {
    font: Font,
    /// Fallback fonts for CJK, emoji, etc.
    fallback_fonts: Vec<Font>,
    pub font_size: f32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub baseline: i32,

    /// Raw RGBA bytes of the atlas — updated on every cache miss, then
    /// re-uploaded to the GPU texture.
    pub bitmap: Vec<u8>,

    /// Map from (char, bold, italic) → atlas region.
    cache: HashMap<(char, bool, bool), AtlasRegion>,

    // Shelf packer state
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,

    /// True when new glyphs have been rasterized since last upload.
    dirty: bool,
}

impl GlyphAtlas {
    /// Load a TTF/OTF font from bytes and compute cell metrics at `font_size`.
    pub fn from_bytes(font_bytes: &[u8], font_size: f32) -> Self {
        let font =
            Font::from_bytes(font_bytes, FontSettings::default()).expect("invalid font data");

        // Measure the '█' full-block character to determine cell dimensions
        let (metrics, _) = font.rasterize('█', font_size);
        let cell_width = metrics.width.max(1) as u32;

        // Cell height from font metrics (ascent + descent)
        let line_metrics = font.horizontal_line_metrics(font_size).unwrap();
        let cell_height =
            (line_metrics.ascent - line_metrics.descent + line_metrics.line_gap).ceil() as u32;
        let baseline = line_metrics.ascent.ceil() as i32;

        GlyphAtlas {
            font,
            fallback_fonts: Vec::new(),
            font_size,
            cell_width,
            cell_height,
            baseline,
            bitmap: vec![0u8; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            cache: HashMap::new(),
            shelf_x: 0,
            shelf_y: 0,
            shelf_h: 0,
            dirty: false,
        }
    }

    /// Add a fallback font for CJK, emoji, or other characters not in the primary font.
    pub fn add_fallback_font(&mut self, font_bytes: &[u8]) {
        if let Ok(font) = Font::from_bytes(font_bytes, FontSettings::default()) {
            self.fallback_fonts.push(font);
        }
    }

    /// Look up a glyph in the cache, rasterizing and packing it if absent.
    /// Returns `None` if the glyph is a space or if the atlas is full.
    pub fn get_or_rasterize(
        &mut self,
        ch: char,
        bold: bool,
        italic: bool,
    ) -> Option<AtlasRegion> {
        // Space and control chars produce no glyph
        if ch == ' ' || (ch as u32) < 0x20 {
            return None;
        }

        let key = (ch, bold, italic);
        if let Some(region) = self.cache.get(&key) {
            return Some(*region);
        }

        // Try primary font first
        let (metrics, bitmap) = self.font.rasterize(ch, self.font_size);
        if metrics.width == 0 || metrics.height == 0 {
            // Try fallback fonts
            for fallback in &self.fallback_fonts {
                let (m, b) = fallback.rasterize(ch, self.font_size);
                if m.width > 0 && m.height > 0 {
                    return self.pack_glyph(ch, bold, italic, m, b);
                }
            }
            return None;
        }

        self.pack_glyph(ch, bold, italic, metrics, bitmap)
    }

    /// Pack a rasterized glyph into the atlas.
    fn pack_glyph(
        &mut self,
        ch: char,
        bold: bool,
        italic: bool,
        metrics: fontdue::Metrics,
        bitmap: Vec<u8>,
    ) -> Option<AtlasRegion> {
        let gw = metrics.width as u32;
        let gh = metrics.height as u32;
        let key = (ch, bold, italic);

        // Advance shelf if this glyph doesn't fit horizontally
        if self.shelf_x + gw > ATLAS_SIZE {
            self.shelf_y += self.shelf_h + 1;
            self.shelf_x = 0;
            self.shelf_h = 0;
        }

        // Atlas full?
        if self.shelf_y + gh > ATLAS_SIZE {
            log::warn!("glyph atlas is full — '{ch}' will not render");
            return None;
        }

        // Blit grayscale bitmap into the atlas
        for row in 0..gh {
            for col in 0..gw {
                let src_idx = (row * gw + col) as usize;
                let dst_x = self.shelf_x + col;
                let dst_y = self.shelf_y + row;
                let dst_idx = (dst_y * ATLAS_SIZE + dst_x) as usize;
                self.bitmap[dst_idx] = bitmap[src_idx];
            }
        }

        let uv_min = [
            self.shelf_x as f32 / ATLAS_SIZE as f32,
            self.shelf_y as f32 / ATLAS_SIZE as f32,
        ];
        let uv_max = [
            (self.shelf_x + gw) as f32 / ATLAS_SIZE as f32,
            (self.shelf_y + gh) as f32 / ATLAS_SIZE as f32,
        ];

        let glyph_metrics = GlyphMetrics {
            width: gw,
            height: gh,
            ymin: metrics.ymin,
            xmin: metrics.xmin,
        };

        // Advance packer
        self.shelf_x += gw + 1;
        if gh > self.shelf_h {
            self.shelf_h = gh;
        }

        let region = AtlasRegion { uv_min, uv_max, metrics: glyph_metrics };
        self.cache.insert(key, region);
        self.dirty = true;
        Some(region)
    }

    /// Returns true if new glyphs have been rasterized since last upload,
    /// then clears the flag.
    pub fn take_dirty(&mut self) -> bool {
        if self.dirty {
            self.dirty = false;
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in font — embedded at compile time so the binary is self-contained.
// We ship JetBrains Mono (SIL Open Font License).
// ---------------------------------------------------------------------------

/// Returns the bytes of the embedded monospace font.
pub fn embedded_font() -> &'static [u8] {
    // Embed JetBrains Mono Regular
    include_bytes!("../../fonts/JetBrainsMono-Regular.ttf")
}
