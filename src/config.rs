//! Configuration file support — TOML-based config with defaults.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Terminal configuration loaded from TOML file.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    /// Font settings.
    #[serde(default)]
    pub font: FontConfig,

    /// Window settings.
    #[serde(default)]
    pub window: WindowConfig,

    /// Color settings.
    #[serde(default)]
    pub colors: ColorConfig,

    /// Keyboard settings.
    #[serde(default)]
    pub keyboard: KeyboardConfig,

    /// Shell to run.
    #[serde(default = "default_shell")]
    pub shell: String,

    /// Scrollback lines.
    #[serde(default = "default_scrollback")]
    pub scrollback: usize,

    /// Enable mouse reporting by default.
    #[serde(default)]
    pub mouse_reporting: bool,

    /// Cursor blink interval in milliseconds (0 = no blink).
    #[serde(default = "default_cursor_blink")]
    pub cursor_blink_ms: u64,

    /// SGR text blink interval in milliseconds (0 = no blink; rendered solid).
    #[serde(default = "default_text_blink")]
    pub text_blink_ms: u64,

    /// Feedback style for the BEL control character.
    #[serde(default)]
    pub bell: BellStyle,

    /// Cursor style.
    #[serde(default = "default_cursor_style")]
    pub cursor_style: CursorStyle,

    /// Tab bar settings.
    #[serde(default)]
    pub tabs: TabsConfig,

    /// Reduce cursor and UI motion for accessibility.
    #[serde(default)]
    pub reduced_motion: bool,

    /// Security and privacy policy for terminal-host interactions.
    #[serde(default)]
    pub security: SecurityConfig,
}

/// Security and privacy policy for terminal-host interactions.
///
/// Following the shitty/pg83 "locked down by default" posture: applications
/// cannot read the clipboard or drive host-window behaviour unless explicitly
/// allowed. Writing the clipboard and setting the window title are the two
/// conveniences kept on by default.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SecurityConfig {
    /// Allow applications to write to the system clipboard via OSC 52.
    #[serde(default = "default_true")]
    pub osc52_write: bool,

    /// Allow applications to read the system clipboard via OSC 52 queries.
    /// Off by default — a hostile prompt could otherwise exfiltrate secrets.
    #[serde(default)]
    pub osc52_read: bool,

    /// Allow OSC 0/2 to change the window title.
    #[serde(default = "default_true")]
    pub window_title: bool,

    /// URI schemes that hyperlinks may open (OSC 8 + plain-text detection).
    /// A configured list replaces the default outright.
    #[serde(default = "default_uri_schemes")]
    pub uri_schemes: Vec<String>,
}

/// Cursor style options.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    Block,
    Bar,
    Underline,
}

impl Default for CursorStyle {
    fn default() -> Self {
        CursorStyle::Block
    }
}

/// Feedback style for the BEL (0x07) control character.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BellStyle {
    /// Briefly tint the window (visual flash).
    Flash,
    /// Emit an audible beep through the host terminal.
    Audible,
    /// No feedback.
    None,
}

impl Default for BellStyle {
    fn default() -> Self {
        BellStyle::Flash
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FontConfig {
    /// Font family name.
    #[serde(default = "default_font_family")]
    pub family: String,

    /// Font size in points.
    #[serde(default = "default_font_size")]
    pub size: f32,

    /// Font file path (optional, overrides family).
    #[serde(default)]
    pub path: Option<String>,

    /// Bold font file path (optional).
    #[serde(default)]
    pub bold_path: Option<String>,

    /// Italic font file path (optional).
    #[serde(default)]
    pub italic_path: Option<String>,

    /// Enable font ligatures.
    #[serde(default = "default_true")]
    pub ligatures: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WindowConfig {
    /// Window title.
    #[serde(default = "default_title")]
    pub title: String,

    /// Default columns.
    #[serde(default = "default_cols")]
    pub cols: u16,

    /// Default rows.
    #[serde(default = "default_rows")]
    pub rows: u16,

    /// Window padding in pixels.
    #[serde(default)]
    pub padding: u32,

    /// Window opacity (0.0 - 1.0).
    #[serde(default = "default_opacity")]
    pub opacity: f64,

    /// Window decorations.
    #[serde(default = "default_true")]
    pub decorations: bool,

    /// Always on top.
    #[serde(default)]
    pub always_on_top: bool,

    /// Enable VSync (vertical sync).
    #[serde(default = "default_true")]
    pub vsync: bool,

    /// Background blur behind translucent windows (Wayland/macOS; no-op on
    /// X11).
    #[serde(default)]
    pub blur: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ColorConfig {
    /// Theme name to load (overrides individual color settings if set).
    #[serde(default)]
    pub theme: Option<String>,

    /// Background color (#RRGGBB or #RRGGBBAA).
    #[serde(default = "default_bg")]
    pub background: String,

    /// Foreground color.
    #[serde(default = "default_fg")]
    pub foreground: String,

    /// Cursor color.
    #[serde(default = "default_cursor_color")]
    pub cursor: String,

    /// Cursor text color (color of text under cursor).
    #[serde(default = "default_cursor_text")]
    pub cursor_text: String,

    /// Selection background color.
    #[serde(default = "default_selection_bg")]
    pub selection_bg: String,

    /// Selection foreground color.
    #[serde(default = "default_selection_fg")]
    pub selection_fg: String,

    /// ANSI color palette (16 colors).
    #[serde(default = "default_ansi_palette")]
    pub ansi: Vec<String>,

    /// Bold text color (optional, uses foreground if not set).
    #[serde(default)]
    pub bold: Option<String>,

    /// Dim text color (optional, uses foreground if not set).
    #[serde(default)]
    pub dim: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct KeyboardConfig {
    /// Custom key bindings.
    #[serde(default)]
    pub bindings: Vec<KeyBinding>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct KeyBinding {
    /// Key combination (e.g., "Ctrl+Shift+C").
    pub key: String,

    /// Action to perform.
    pub action: String,
}

/// Tab bar configuration.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TabsConfig {
    /// Show the tab bar when there is more than one tab.
    #[serde(default = "default_true")]
    pub show_tab_bar: bool,

    /// Tab bar height in pixels.
    #[serde(default = "default_tab_bar_height")]
    pub height: u32,
}

fn default_tab_bar_height() -> u32 {
    30
}

impl Default for TabsConfig {
    fn default() -> Self {
        TabsConfig {
            show_tab_bar: true,
            height: default_tab_bar_height(),
        }
    }
}

// Default value functions
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
}

fn default_scrollback() -> usize {
    10000
}

/// Family name of the font embedded in the binary. Used to decide whether a
/// configured `family` should trigger a system fontconfig lookup.
pub const EMBEDDED_FONT_FAMILY: &str = "JetBrains Mono";

fn default_font_family() -> String {
    EMBEDDED_FONT_FAMILY.to_string()
}

fn default_font_size() -> f32 {
    15.0
}

fn default_cols() -> u16 {
    80
}

fn default_rows() -> u16 {
    24
}

fn default_title() -> String {
    "Terminal".to_string()
}

fn default_opacity() -> f64 {
    1.0
}

fn default_cursor_blink() -> u64 {
    500
}

fn default_text_blink() -> u64 {
    500
}

fn default_cursor_style() -> CursorStyle {
    CursorStyle::Block
}

fn default_true() -> bool {
    true
}

fn default_uri_schemes() -> Vec<String> {
    vec!["http".to_string(), "https".to_string()]
}

impl Default for SecurityConfig {
    fn default() -> Self {
        SecurityConfig {
            osc52_write: true,
            osc52_read: false,
            window_title: true,
            uri_schemes: default_uri_schemes(),
        }
    }
}

fn default_bg() -> String {
    "#1E1E2E".to_string()
}

fn default_fg() -> String {
    "#CDD6F4".to_string()
}

fn default_cursor_color() -> String {
    "#89B4FA".to_string()
}

fn default_cursor_text() -> String {
    "#1E1E2E".to_string()
}

fn default_selection_bg() -> String {
    "#45475A".to_string()
}

fn default_selection_fg() -> String {
    "#CDD6F4".to_string()
}

/// Default ANSI color palette (Catppuccin Mocha).
fn default_ansi_palette() -> Vec<String> {
    vec![
        "#1E1E2E".to_string(), // Black
        "#F38BA8".to_string(), // Red
        "#A6E3A1".to_string(), // Green
        "#F9E2AF".to_string(), // Yellow
        "#89B4FA".to_string(), // Blue
        "#F5C2E7".to_string(), // Magenta
        "#94E2D5".to_string(), // Cyan
        "#BAC2DE".to_string(), // White
        "#585B70".to_string(), // Bright Black
        "#F38BA8".to_string(), // Bright Red
        "#A6E3A1".to_string(), // Bright Green
        "#F9E2AF".to_string(), // Bright Yellow
        "#89B4FA".to_string(), // Bright Blue
        "#F5C2E7".to_string(), // Bright Magenta
        "#94E2D5".to_string(), // Bright Cyan
        "#A6ADC8".to_string(), // Bright White
    ]
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig {
            family: default_font_family(),
            size: default_font_size(),
            path: None,
            bold_path: None,
            italic_path: None,
            ligatures: true,
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        WindowConfig {
            title: default_title(),
            cols: default_cols(),
            rows: default_rows(),
            padding: 0,
            opacity: default_opacity(),
            decorations: true,
            always_on_top: false,
            vsync: true,
            blur: false,
        }
    }
}

impl Default for ColorConfig {
    fn default() -> Self {
        ColorConfig {
            theme: None,
            background: default_bg(),
            foreground: default_fg(),
            cursor: default_cursor_color(),
            cursor_text: default_cursor_text(),
            selection_bg: default_selection_bg(),
            selection_fg: default_selection_fg(),
            ansi: default_ansi_palette(),
            bold: None,
            dim: None,
        }
    }
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        KeyboardConfig {
            bindings: Vec::new(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            font: FontConfig::default(),
            window: WindowConfig::default(),
            colors: ColorConfig::default(),
            keyboard: KeyboardConfig::default(),
            shell: default_shell(),
            scrollback: default_scrollback(),
            mouse_reporting: false,
            cursor_blink_ms: default_cursor_blink(),
            text_blink_ms: default_text_blink(),
            cursor_style: CursorStyle::default(),
            tabs: TabsConfig::default(),
            reduced_motion: false,
            bell: BellStyle::default(),
            security: SecurityConfig::default(),
        }
    }
}

impl Config {
    /// Load configuration from the default location.
    /// Priority: ./terminal.toml > ~/.config/terminal/config.toml > defaults
    pub fn load() -> Self {
        // Try current directory first
        if let Ok(config) = Self::from_file("terminal.toml") {
            log::info!("Loaded config from ./terminal.toml");
            return config;
        }

        // Try XDG config directory
        if let Some(config_dir) = dirs::config_dir() {
            let path = config_dir.join("terminal").join("config.toml");
            if let Ok(config) = Self::from_file(&path) {
                log::info!("Loaded config from {}", path.display());
                return config;
            }
        }

        log::info!("Using default configuration");
        Config::default()
    }

    /// Load configuration from a specific file path.
    pub fn from_file(path: impl Into<PathBuf>) -> Result<Self, Box<dyn std::error::Error>> {
        let path = path.into();
        let content = std::fs::read_to_string(&path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// Save configuration to a file.
    pub fn save(&self, path: impl Into<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
        let path = path.into();
        let content = toml::to_string_pretty(self)?;

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&path, content)?;
        log::info!("Saved config to {}", path.display());
        Ok(())
    }

    /// Generate default config file content.
    pub fn default_content() -> String {
        let config = Config::default();
        toml::to_string_pretty(&config).unwrap_or_default()
    }

    /// Parse a hex color string to RGB values.
    pub fn parse_color(hex: &str) -> Option<(u8, u8, u8)> {
        let hex = hex.trim_start_matches('#');
        match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some((r, g, b))
            }
            8 => {
                // #RRGGBBAA - ignore alpha
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some((r, g, b))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.font.size, 15.0);
        assert_eq!(config.window.cols, 80);
        assert_eq!(config.window.rows, 24);
        assert_eq!(config.scrollback, 10000);
        assert!(!config.mouse_reporting);
        assert!(!config.reduced_motion);
        assert_eq!(config.bell, BellStyle::Flash);
    }

    #[test]
    fn test_parse_bell_style() {
        let config: Config = toml::from_str("bell = \"audible\"\n").unwrap();
        assert_eq!(config.bell, BellStyle::Audible);
        let config: Config = toml::from_str("bell = \"none\"\n").unwrap();
        assert_eq!(config.bell, BellStyle::None);
    }

    #[test]
    fn test_parse_minimal_config() {
        let toml_str = r#"
[font]
size = 12.0

[window]
cols = 120
rows = 40

[colors]
background = '000000'
foreground = 'FFFFFF'
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.font.size, 12.0);
        assert_eq!(config.window.cols, 120);
        assert_eq!(config.window.rows, 40);
        assert_eq!(config.colors.background, "000000");
        assert_eq!(config.colors.foreground, "FFFFFF");
    }

    #[test]
    fn test_parse_empty_config() {
        let toml_str = "";
        let config: Config = toml::from_str(toml_str).unwrap();
        // All defaults should apply
        assert_eq!(config.font.size, 15.0);
        assert_eq!(config.window.cols, 80);
    }

    #[test]
    fn test_roundtrip() {
        let original = Config::default();
        let serialized = toml::to_string_pretty(&original).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(original.font.size, deserialized.font.size);
        assert_eq!(original.window.cols, deserialized.window.cols);
        assert_eq!(original.shell, deserialized.shell);
    }

    #[test]
    fn test_save_and_load() {
        let dir = std::env::temp_dir().join("terminal_config_test");
        let _ = std::fs::remove_dir_all(&dir);

        let path = dir.join("test.toml");
        let config = Config::default();
        config.save(&path).unwrap();

        let loaded = Config::from_file(&path).unwrap();
        assert_eq!(config.font.size, loaded.font.size);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_parse_color() {
        assert_eq!(Config::parse_color("#FF0000"), Some((255, 0, 0)));
        assert_eq!(Config::parse_color("#00FF00"), Some((0, 255, 0)));
        assert_eq!(Config::parse_color("#0000FF"), Some((0, 0, 255)));
        assert_eq!(Config::parse_color("FF0000"), Some((255, 0, 0)));
        assert_eq!(Config::parse_color("#1E1E2E"), Some((30, 30, 46)));
        assert_eq!(Config::parse_color("invalid"), None);
    }

    #[test]
    fn test_cursor_style() {
        let toml_str = r#"
cursor_style = "bar"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.cursor_style, CursorStyle::Bar);
    }

    #[test]
    fn test_security_defaults_locked_down() {
        let config = Config::default();
        assert!(config.security.osc52_write);
        assert!(!config.security.osc52_read);
        assert!(config.security.window_title);
        assert_eq!(config.security.uri_schemes, vec!["http", "https"]);
    }

    #[test]
    fn test_security_parse() {
        let toml_str = r#"
[security]
osc52_write = false
osc52_read = true
uri_schemes = ["https", "gemini"]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(!config.security.osc52_write);
        assert!(config.security.osc52_read);
        assert_eq!(config.security.uri_schemes, vec!["https", "gemini"]);
        assert!(config.security.window_title); // untouched → default true
    }

    #[test]
    fn test_security_roundtrip() {
        let original = Config::default();
        let serialized = toml::to_string_pretty(&original).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert!(!deserialized.security.osc52_read);
        assert!(deserialized.security.osc52_write);
    }
}
