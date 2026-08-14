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

#[derive(Debug, Clone, Copy)]
pub struct ShapedGlyph {
    pub region: AtlasRegion,
    pub x_offset: f32,
    pub y_offset: f32,
    pub x_advance: f32,
}

pub const ATLAS_SIZE: u32 = 1024;

pub struct GlyphAtlas {
    font: Font,
    /// Dedicated bold/italic faces (loaded from config paths). None falls back
    /// to the primary face; bold is then synthesized via a faux-bold smear.
    bold_font: Option<Font>,
    italic_font: Option<Font>,
    /// Primary font bytes, kept for RustyBuzz cluster shaping.
    font_data: Vec<u8>,
    /// Fallback fonts for CJK, emoji, etc., loaded on first glyph miss.
    fallback_fonts: Vec<Font>,
    fallback_attempted: bool,
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
            bold_font: None,
            italic_font: None,
            font_data: font_bytes.to_vec(),
            fallback_fonts: Vec::new(),
            fallback_attempted: false,
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

    /// Set the dedicated bold face (loaded from `font.bold_path`).
    pub fn set_bold_font(&mut self, font_bytes: &[u8]) {
        if let Ok(font) = Font::from_bytes(font_bytes, FontSettings::default()) {
            self.bold_font = Some(font);
        }
    }

    /// Set the dedicated italic face (loaded from `font.italic_path`).
    pub fn set_italic_font(&mut self, font_bytes: &[u8]) {
        if let Ok(font) = Font::from_bytes(font_bytes, FontSettings::default()) {
            self.italic_font = Some(font);
        }
    }

    /// Look up a glyph in the cache, rasterizing and packing it if absent.
    /// Returns `None` if the glyph is a space or if the atlas is full.
    pub fn get_or_rasterize(&mut self, ch: char, bold: bool, italic: bool) -> Option<AtlasRegion> {
        // Space and control chars produce no glyph
        if ch == ' ' || (ch as u32) < 0x20 {
            return None;
        }

        let key = (ch, bold, italic);
        if let Some(region) = self.cache.get(&key) {
            return Some(*region);
        }

        // Select the face: dedicated italic/bold fonts when loaded, otherwise
        // the primary face (bold is then synthesized with a faux-bold smear).
        let face = if italic {
            self.italic_font.as_ref().unwrap_or(&self.font)
        } else if bold {
            self.bold_font.as_ref().unwrap_or(&self.font)
        } else {
            &self.font
        };

        let (metrics, mut bitmap) = face.rasterize(ch, self.font_size);
        if bold && self.bold_font.is_none() && metrics.width > 0 {
            apply_faux_bold(&mut bitmap, metrics.width as usize, metrics.height as usize);
        }
        if italic && self.italic_font.is_none() && metrics.width > 0 {
            apply_faux_italic(&mut bitmap, metrics.width as usize, metrics.height as usize);
        }
        if metrics.width == 0 || metrics.height == 0 {
            self.ensure_fallback_fonts();
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

        let region = AtlasRegion {
            uv_min,
            uv_max,
            metrics: glyph_metrics,
        };
        self.cache.insert(key, region);
        self.dirty = true;
        Some(region)
    }

    /// Shape a cluster with RustyBuzz and rasterize the resulting glyph IDs.
    /// This provides real ligature/mark positioning for the embedded primary
    /// font; fallback shaping remains a later multi-font extension.
    pub fn shape_cluster(&mut self, text: &str, bold: bool, italic: bool) -> Vec<ShapedGlyph> {
        if text.is_empty() {
            return Vec::new();
        }
        let Some(face) = rustybuzz::Face::from_slice(&self.font_data, 0) else {
            return Vec::new();
        };
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        let shaped = rustybuzz::shape(&face, &[], buffer);
        shaped
            .glyph_infos()
            .iter()
            .zip(shaped.glyph_positions())
            .filter_map(|(info, position)| {
                let glyph_index = u16::try_from(info.glyph_id).ok()?;
                let (metrics, bitmap) = self.font.rasterize_indexed(glyph_index, self.font_size);
                if metrics.width == 0 || metrics.height == 0 {
                    return None;
                }
                let region = self.pack_glyph(
                    char::from_u32(info.glyph_id).unwrap_or(' '),
                    bold,
                    italic,
                    metrics,
                    bitmap,
                )?;
                Some(ShapedGlyph {
                    region,
                    x_offset: position.x_offset as f32 / 64.0,
                    y_offset: position.y_offset as f32 / 64.0,
                    x_advance: position.x_advance as f32 / 64.0,
                })
            })
            .collect()
    }

    fn ensure_fallback_fonts(&mut self) {
        if self.fallback_attempted {
            return;
        }
        self.fallback_attempted = true;
        load_fallback_fonts(self);
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

/// Load fallback fonts for CJK, emoji, etc. Search the XDG config dir first
/// (stable across working directories), then the current directory for a
/// vendored copy.
pub fn load_fallback_fonts(atlas: &mut GlyphAtlas) {
    let cjk_font_paths = [
        "fonts/NotoSansCJK-Regular.otf",
        "fonts/NotoSansCJKsc-Regular.otf",
        "fonts/NotoSansSC-Regular.otf",
    ];

    let mut search_dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Some(config_dir) = dirs::config_dir() {
        search_dirs.push(config_dir.join("terminal"));
    }
    search_dirs.push(std::path::PathBuf::from("."));

    for dir in &search_dirs {
        for rel in &cjk_font_paths {
            let path = dir.join(rel);
            if let Ok(bytes) = std::fs::read(&path) {
                atlas.add_fallback_font(&bytes);
                log::info!("Loaded fallback font from {}", path.display());
                return;
            }
        }
    }

    // No CJK fallback font found - log warning but don't crash
    log::warn!("No CJK fallback font found — CJK and emoji characters may not render correctly");
}

/// Resolve the primary font bytes from config: an explicit `path` wins, then a
/// `family` looked up via fontconfig (`fc-match`), falling back to the embedded
/// font so the binary stays self-contained.
pub fn load_primary_font(fc: &crate::config::FontConfig) -> Vec<u8> {
    if let Some(path) = &fc.path {
        match std::fs::read(path) {
            Ok(bytes) => {
                log::info!("Loaded font from {}", path);
                return bytes;
            }
            Err(e) => log::warn!("Font path {} unreadable: {e}", path),
        }
    }

    if fc.family != crate::config::EMBEDDED_FONT_FAMILY {
        if let Some(bytes) = resolve_family(&fc.family) {
            log::info!("Loaded font family '{}' via fontconfig", fc.family);
            return bytes;
        }
        log::warn!(
            "Font family '{}' not found — using embedded font",
            fc.family
        );
    }

    embedded_font().to_vec()
}

/// Resolve a family name to font bytes via `fc-match` (fontconfig). Returns
/// None when fontconfig is unavailable or the family can't be resolved.
fn resolve_family(family: &str) -> Option<Vec<u8>> {
    let out = std::process::Command::new("fc-match")
        .args(["--format=%{file}", family])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        return None;
    }
    std::fs::read(path).ok()
}

/// Synthesize bold by smearing each pixel one column rightward (faux bold),
/// used only when no dedicated bold face is loaded.
fn apply_faux_bold(bitmap: &mut [u8], width: usize, height: usize) {
    if width < 2 {
        return;
    }
    for row in 0..height {
        let base = row * width;
        for col in (0..width - 1).rev() {
            let v = bitmap[base + col];
            if v > bitmap[base + col + 1] {
                bitmap[base + col + 1] = v;
            }
        }
    }
}

/// Synthesize italic by shearing the glyph (top rows shift right, bottom rows
/// stay), used only when no dedicated italic face is loaded. Operates within
/// the existing width, clipping the slanted right edge by up to a few pixels.
fn apply_faux_italic(bitmap: &mut [u8], width: usize, height: usize) {
    if width < 2 || height < 2 {
        return;
    }
    let skew = (height / 8).clamp(1, 4);
    let mut out = vec![0u8; width * height];
    for row in 0..height {
        // Top leans right; bottom is the pivot.
        let offset = (skew * (height - 1 - row)) / (height - 1);
        for col in 0..width {
            let dst = col + offset;
            if dst < width {
                out[row * width + dst] = bitmap[row * width + col];
            }
        }
    }
    bitmap.copy_from_slice(&out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaping_returns_positioned_glyphs_for_complex_text() {
        let mut atlas = GlyphAtlas::from_bytes(embedded_font(), 15.0);
        let shaped = atlas.shape_cluster("e\u{301}", false, false);
        assert!(!shaped.is_empty());
        assert!(shaped.iter().any(|glyph| glyph.region.metrics.width > 0));
    }

    #[test]
    fn fallback_fonts_are_lazy_until_a_missing_glyph_is_requested() {
        let mut atlas = GlyphAtlas::from_bytes(embedded_font(), 15.0);
        assert!(!atlas.fallback_attempted);
        atlas.ensure_fallback_fonts();
        assert!(atlas.fallback_attempted);
    }

    #[test]
    fn faux_bold_smears_pixels_rightward() {
        // 3x2 bitmap with a single lit pixel on the left edge of each row.
        let mut bitmap = vec![200, 0, 0, 150, 0, 0];
        apply_faux_bold(&mut bitmap, 3, 2);
        assert_eq!(bitmap, vec![200, 200, 0, 150, 150, 0]);
    }

    #[test]
    fn faux_italic_shears_top_rows_right() {
        // A single lit pixel in the left column of a tall bitmap: after
        // shearing, the top pixel shifts right while the bottom stays put.
        let mut bitmap = vec![200, 0, 0, 150, 0, 0];
        apply_faux_italic(&mut bitmap, 3, 2);
        // Top row (row 0) shifts right by `skew`; bottom row stays.
        assert_eq!(bitmap[0], 0, "top pixel moved");
        assert_eq!(bitmap[1], 200, "top pixel sheared right");
        assert_eq!(bitmap[3], 150, "bottom pixel unmoved");
    }

    #[test]
    fn primary_font_falls_back_to_embedded_for_default_family() {
        let fc = crate::config::FontConfig {
            family: crate::config::EMBEDDED_FONT_FAMILY.to_string(),
            size: 15.0,
            path: None,
            bold_path: None,
            italic_path: None,
            ligatures: true,
        };
        let bytes = load_primary_font(&fc);
        assert_eq!(bytes, embedded_font());
    }
}
