//! Theme system — built-in themes and theme loading.

use crate::config::ColorConfig;

/// A terminal color theme.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    pub colors: ColorConfig,
}

impl Theme {
    /// Get all built-in themes.
    pub fn built_in_themes() -> Vec<Theme> {
        vec![
            Self::catppuccin_mocha(),
            Self::catppuccin_latte(),
            Self::catppuccin_frappe(),
            Self::catppuccin_macchiato(),
            Self::gruvbox_dark(),
            Self::gruvbox_light(),
            Self::dracula(),
            Self::tokyo_night(),
            Self::tokyo_night_storm(),
            Self::nord(),
            Self::solarized_dark(),
            Self::solarized_light(),
            Self::one_dark(),
            Self::monokai(),
            Self::github_dark(),
            Self::github_light(),
        ]
    }

    /// Find a theme by name (case-insensitive).
    pub fn find(name: &str) -> Option<Theme> {
        let name_lower = name.to_lowercase();
        Self::built_in_themes()
            .into_iter()
            .find(|t| t.name.to_lowercase() == name_lower)
    }

    /// Get theme names.
    pub fn theme_names() -> Vec<&'static str> {
        Self::built_in_themes().iter().map(|t| t.name).collect()
    }

    // -----------------------------------------------------------------------
    // Built-in themes
    // -----------------------------------------------------------------------

    pub fn catppuccin_mocha() -> Theme {
        Theme {
            name: "Catppuccin Mocha",
            colors: ColorConfig {
                background: "#1E1E2E".to_string(),
                foreground: "#CDD6F4".to_string(),
                cursor: "#F5E0DC".to_string(),
                cursor_text: "#1E1E2E".to_string(),
                selection_bg: "#585B70".to_string(),
                selection_fg: "#CDD6F4".to_string(),
                ansi: vec![
                    "#45475A".to_string(), // Black
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
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn catppuccin_latte() -> Theme {
        Theme {
            name: "Catppuccin Latte",
            colors: ColorConfig {
                background: "#EFF1F5".to_string(),
                foreground: "#4C4F69".to_string(),
                cursor: "#DC8A78".to_string(),
                cursor_text: "#EFF1F5".to_string(),
                selection_bg: "#CCD0DA".to_string(),
                selection_fg: "#4C4F69".to_string(),
                ansi: vec![
                    "#CCD0DA".to_string(), // Black
                    "#D20F39".to_string(), // Red
                    "#40A02B".to_string(), // Green
                    "#DF8E1D".to_string(), // Yellow
                    "#1E66F5".to_string(), // Blue
                    "#EA76CB".to_string(), // Magenta
                    "#179299".to_string(), // Cyan
                    "#ACB0BE".to_string(), // White
                    "#CCD0DA".to_string(), // Bright Black
                    "#D20F39".to_string(), // Bright Red
                    "#40A02B".to_string(), // Bright Green
                    "#DF8E1D".to_string(), // Bright Yellow
                    "#1E66F5".to_string(), // Bright Blue
                    "#EA76CB".to_string(), // Bright Magenta
                    "#179299".to_string(), // Bright Cyan
                    "#BCC0CC".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn catppuccin_frappe() -> Theme {
        Theme {
            name: "Catppuccin Frappe",
            colors: ColorConfig {
                background: "#303446".to_string(),
                foreground: "#C6D0F5".to_string(),
                cursor: "#F2D5CF".to_string(),
                cursor_text: "#303446".to_string(),
                selection_bg: "#414559".to_string(),
                selection_fg: "#C6D0F5".to_string(),
                ansi: vec![
                    "#51576D".to_string(), // Black
                    "#E78284".to_string(), // Red
                    "#A6D189".to_string(), // Green
                    "#E5C890".to_string(), // Yellow
                    "#8CAAEE".to_string(), // Blue
                    "#F4B8E4".to_string(), // Magenta
                    "#81C8BE".to_string(), // Cyan
                    "#B5BFE2".to_string(), // White
                    "#626880".to_string(), // Bright Black
                    "#E78284".to_string(), // Bright Red
                    "#A6D189".to_string(), // Bright Green
                    "#E5C890".to_string(), // Bright Yellow
                    "#8CAAEE".to_string(), // Bright Blue
                    "#F4B8E4".to_string(), // Bright Magenta
                    "#81C8BE".to_string(), // Bright Cyan
                    "#A5ADCE".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn catppuccin_macchiato() -> Theme {
        Theme {
            name: "Catppuccin Macchiato",
            colors: ColorConfig {
                background: "#24273A".to_string(),
                foreground: "#CAD3F5".to_string(),
                cursor: "#F4DBD6".to_string(),
                cursor_text: "#24273A".to_string(),
                selection_bg: "#363A4F".to_string(),
                selection_fg: "#CAD3F5".to_string(),
                ansi: vec![
                    "#494D64".to_string(), // Black
                    "#ED8796".to_string(), // Red
                    "#A6DA95".to_string(), // Green
                    "#EED49F".to_string(), // Yellow
                    "#8AADF4".to_string(), // Blue
                    "#F5BDE6".to_string(), // Magenta
                    "#8BD5CA".to_string(), // Cyan
                    "#B8C0E0".to_string(), // White
                    "#5B6078".to_string(), // Bright Black
                    "#ED8796".to_string(), // Bright Red
                    "#A6DA95".to_string(), // Bright Green
                    "#EED49F".to_string(), // Bright Yellow
                    "#8AADF4".to_string(), // Bright Blue
                    "#F5BDE6".to_string(), // Bright Magenta
                    "#8BD5CA".to_string(), // Bright Cyan
                    "#A5ADCB".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn gruvbox_dark() -> Theme {
        Theme {
            name: "Gruvbox Dark",
            colors: ColorConfig {
                background: "#282828".to_string(),
                foreground: "#EBDBB2".to_string(),
                cursor: "#EBDBB2".to_string(),
                cursor_text: "#282828".to_string(),
                selection_bg: "#504945".to_string(),
                selection_fg: "#EBDBB2".to_string(),
                ansi: vec![
                    "#282828".to_string(), // Black
                    "#CC241D".to_string(), // Red
                    "#98971A".to_string(), // Green
                    "#D79921".to_string(), // Yellow
                    "#458588".to_string(), // Blue
                    "#B16286".to_string(), // Magenta
                    "#689D6A".to_string(), // Cyan
                    "#A89984".to_string(), // White
                    "#928374".to_string(), // Bright Black
                    "#FB4934".to_string(), // Bright Red
                    "#B8BB26".to_string(), // Bright Green
                    "#FABD2F".to_string(), // Bright Yellow
                    "#83A598".to_string(), // Bright Blue
                    "#D3869B".to_string(), // Bright Magenta
                    "#8EC07C".to_string(), // Bright Cyan
                    "#EBDBB2".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn gruvbox_light() -> Theme {
        Theme {
            name: "Gruvbox Light",
            colors: ColorConfig {
                background: "#FBF1C7".to_string(),
                foreground: "#3C3836".to_string(),
                cursor: "#3C3836".to_string(),
                cursor_text: "#FBF1C7".to_string(),
                selection_bg: "#EBDBB2".to_string(),
                selection_fg: "#3C3836".to_string(),
                ansi: vec![
                    "#FBF1C7".to_string(), // Black
                    "#CC241D".to_string(), // Red
                    "#98971A".to_string(), // Green
                    "#D79921".to_string(), // Yellow
                    "#458588".to_string(), // Blue
                    "#B16286".to_string(), // Magenta
                    "#689D6A".to_string(), // Cyan
                    "#7C6F64".to_string(), // White
                    "#928374".to_string(), // Bright Black
                    "#9D0006".to_string(), // Bright Red
                    "#79740E".to_string(), // Bright Green
                    "#B57614".to_string(), // Bright Yellow
                    "#076678".to_string(), // Bright Blue
                    "#8F3F71".to_string(), // Bright Magenta
                    "#427B58".to_string(), // Bright Cyan
                    "#3C3836".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn dracula() -> Theme {
        Theme {
            name: "Dracula",
            colors: ColorConfig {
                background: "#282A36".to_string(),
                foreground: "#F8F8F2".to_string(),
                cursor: "#F8F8F2".to_string(),
                cursor_text: "#282A36".to_string(),
                selection_bg: "#44475A".to_string(),
                selection_fg: "#F8F8F2".to_string(),
                ansi: vec![
                    "#21222C".to_string(), // Black
                    "#FF5555".to_string(), // Red
                    "#50FA7B".to_string(), // Green
                    "#F1FA8C".to_string(), // Yellow
                    "#BD93F9".to_string(), // Blue
                    "#FF79C6".to_string(), // Magenta
                    "#8BE9FD".to_string(), // Cyan
                    "#F8F8F2".to_string(), // White
                    "#6272A4".to_string(), // Bright Black
                    "#FF6E6E".to_string(), // Bright Red
                    "#69FF94".to_string(), // Bright Green
                    "#FFFFA5".to_string(), // Bright Yellow
                    "#D6ACFF".to_string(), // Bright Blue
                    "#FF92DF".to_string(), // Bright Magenta
                    "#A4FFFF".to_string(), // Bright Cyan
                    "#FFFFFF".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn tokyo_night() -> Theme {
        Theme {
            name: "Tokyo Night",
            colors: ColorConfig {
                background: "#1A1B26".to_string(),
                foreground: "#C0CAF5".to_string(),
                cursor: "#C0CAF5".to_string(),
                cursor_text: "#1A1B26".to_string(),
                selection_bg: "#33467C".to_string(),
                selection_fg: "#C0CAF5".to_string(),
                ansi: vec![
                    "#15161E".to_string(), // Black
                    "#F7768E".to_string(), // Red
                    "#9ECE6A".to_string(), // Green
                    "#E0AF68".to_string(), // Yellow
                    "#7AA2F7".to_string(), // Blue
                    "#BB9AF7".to_string(), // Magenta
                    "#7DCFFF".to_string(), // Cyan
                    "#A9B1D6".to_string(), // White
                    "#414868".to_string(), // Bright Black
                    "#F7768E".to_string(), // Bright Red
                    "#9ECE6A".to_string(), // Bright Green
                    "#E0AF68".to_string(), // Bright Yellow
                    "#7AA2F7".to_string(), // Bright Blue
                    "#BB9AF7".to_string(), // Bright Magenta
                    "#7DCFFF".to_string(), // Bright Cyan
                    "#C0CAF5".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn tokyo_night_storm() -> Theme {
        Theme {
            name: "Tokyo Night Storm",
            colors: ColorConfig {
                background: "#24283B".to_string(),
                foreground: "#C0CAF5".to_string(),
                cursor: "#C0CAF5".to_string(),
                cursor_text: "#24283B".to_string(),
                selection_bg: "#33467C".to_string(),
                selection_fg: "#C0CAF5".to_string(),
                ansi: vec![
                    "#1D202F".to_string(), // Black
                    "#F7768E".to_string(), // Red
                    "#9ECE6A".to_string(), // Green
                    "#E0AF68".to_string(), // Yellow
                    "#7AA2F7".to_string(), // Blue
                    "#BB9AF7".to_string(), // Magenta
                    "#7DCFFF".to_string(), // Cyan
                    "#A9B1D6".to_string(), // White
                    "#414868".to_string(), // Bright Black
                    "#F7768E".to_string(), // Bright Red
                    "#9ECE6A".to_string(), // Bright Green
                    "#E0AF68".to_string(), // Bright Yellow
                    "#7AA2F7".to_string(), // Bright Blue
                    "#BB9AF7".to_string(), // Bright Magenta
                    "#7DCFFF".to_string(), // Bright Cyan
                    "#C0CAF5".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn nord() -> Theme {
        Theme {
            name: "Nord",
            colors: ColorConfig {
                background: "#2E3440".to_string(),
                foreground: "#D8DEE9".to_string(),
                cursor: "#D8DEE9".to_string(),
                cursor_text: "#2E3440".to_string(),
                selection_bg: "#434C5E".to_string(),
                selection_fg: "#D8DEE9".to_string(),
                ansi: vec![
                    "#3B4252".to_string(), // Black
                    "#BF616A".to_string(), // Red
                    "#A3BE8C".to_string(), // Green
                    "#EBCB8B".to_string(), // Yellow
                    "#81A1C1".to_string(), // Blue
                    "#B48EAD".to_string(), // Magenta
                    "#88C0D0".to_string(), // Cyan
                    "#E5E9F0".to_string(), // White
                    "#4C566A".to_string(), // Bright Black
                    "#BF616A".to_string(), // Bright Red
                    "#A3BE8C".to_string(), // Bright Green
                    "#EBCB8B".to_string(), // Bright Yellow
                    "#81A1C1".to_string(), // Bright Blue
                    "#B48EAD".to_string(), // Bright Magenta
                    "#88C0D0".to_string(), // Bright Cyan
                    "#ECEFF4".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn solarized_dark() -> Theme {
        Theme {
            name: "Solarized Dark",
            colors: ColorConfig {
                background: "#002B36".to_string(),
                foreground: "#839496".to_string(),
                cursor: "#839496".to_string(),
                cursor_text: "#002B36".to_string(),
                selection_bg: "#073642".to_string(),
                selection_fg: "#839496".to_string(),
                ansi: vec![
                    "#073642".to_string(), // Black
                    "#DC322F".to_string(), // Red
                    "#859900".to_string(), // Green
                    "#B58900".to_string(), // Yellow
                    "#268BD2".to_string(), // Blue
                    "#D33682".to_string(), // Magenta
                    "#2AA198".to_string(), // Cyan
                    "#EEE8D5".to_string(), // White
                    "#002B36".to_string(), // Bright Black
                    "#CB4B16".to_string(), // Bright Red
                    "#586E75".to_string(), // Bright Green
                    "#657B83".to_string(), // Bright Yellow
                    "#839496".to_string(), // Bright Blue
                    "#6C71C4".to_string(), // Bright Magenta
                    "#93A1A1".to_string(), // Bright Cyan
                    "#FDF6E3".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn solarized_light() -> Theme {
        Theme {
            name: "Solarized Light",
            colors: ColorConfig {
                background: "#FDF6E3".to_string(),
                foreground: "#657B83".to_string(),
                cursor: "#657B83".to_string(),
                cursor_text: "#FDF6E3".to_string(),
                selection_bg: "#EEE8D5".to_string(),
                selection_fg: "#657B83".to_string(),
                ansi: vec![
                    "#EEE8D5".to_string(), // Black
                    "#DC322F".to_string(), // Red
                    "#859900".to_string(), // Green
                    "#B58900".to_string(), // Yellow
                    "#268BD2".to_string(), // Blue
                    "#D33682".to_string(), // Magenta
                    "#2AA198".to_string(), // Cyan
                    "#586E75".to_string(), // White
                    "#FDF6E3".to_string(), // Bright Black
                    "#CB4B16".to_string(), // Bright Red
                    "#586E75".to_string(), // Bright Green
                    "#657B83".to_string(), // Bright Yellow
                    "#839496".to_string(), // Bright Blue
                    "#6C71C4".to_string(), // Bright Magenta
                    "#93A1A1".to_string(), // Bright Cyan
                    "#002B36".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn one_dark() -> Theme {
        Theme {
            name: "One Dark",
            colors: ColorConfig {
                background: "#282C34".to_string(),
                foreground: "#ABB2BF".to_string(),
                cursor: "#ABB2BF".to_string(),
                cursor_text: "#282C34".to_string(),
                selection_bg: "#3E4451".to_string(),
                selection_fg: "#ABB2BF".to_string(),
                ansi: vec![
                    "#282C34".to_string(), // Black
                    "#E06C75".to_string(), // Red
                    "#98C379".to_string(), // Green
                    "#E5C07B".to_string(), // Yellow
                    "#61AFEF".to_string(), // Blue
                    "#C678DD".to_string(), // Magenta
                    "#56B6C2".to_string(), // Cyan
                    "#ABB2BF".to_string(), // White
                    "#5C6370".to_string(), // Bright Black
                    "#E06C75".to_string(), // Bright Red
                    "#98C379".to_string(), // Bright Green
                    "#E5C07B".to_string(), // Bright Yellow
                    "#61AFEF".to_string(), // Bright Blue
                    "#C678DD".to_string(), // Bright Magenta
                    "#56B6C2".to_string(), // Bright Cyan
                    "#FFFFFF".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn monokai() -> Theme {
        Theme {
            name: "Monokai",
            colors: ColorConfig {
                background: "#272822".to_string(),
                foreground: "#F8F8F2".to_string(),
                cursor: "#F8F8F0".to_string(),
                cursor_text: "#272822".to_string(),
                selection_bg: "#49483E".to_string(),
                selection_fg: "#F8F8F2".to_string(),
                ansi: vec![
                    "#272822".to_string(), // Black
                    "#F92672".to_string(), // Red
                    "#A6E22E".to_string(), // Green
                    "#F4BF75".to_string(), // Yellow
                    "#66D9EF".to_string(), // Blue
                    "#AE81FF".to_string(), // Magenta
                    "#A1EFE4".to_string(), // Cyan
                    "#F8F8F2".to_string(), // White
                    "#75715E".to_string(), // Bright Black
                    "#F92672".to_string(), // Bright Red
                    "#A6E22E".to_string(), // Bright Green
                    "#F4BF75".to_string(), // Bright Yellow
                    "#66D9EF".to_string(), // Bright Blue
                    "#AE81FF".to_string(), // Bright Magenta
                    "#A1EFE4".to_string(), // Bright Cyan
                    "#F9F8F5".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn github_dark() -> Theme {
        Theme {
            name: "GitHub Dark",
            colors: ColorConfig {
                background: "#0D1117".to_string(),
                foreground: "#C9D1D9".to_string(),
                cursor: "#C9D1D9".to_string(),
                cursor_text: "#0D1117".to_string(),
                selection_bg: "#1F2A38".to_string(),
                selection_fg: "#C9D1D9".to_string(),
                ansi: vec![
                    "#484F58".to_string(), // Black
                    "#FF7B72".to_string(), // Red
                    "#3FB950".to_string(), // Green
                    "#D29922".to_string(), // Yellow
                    "#58A6FF".to_string(), // Blue
                    "#BC8CFF".to_string(), // Magenta
                    "#39D353".to_string(), // Cyan
                    "#C9D1D9".to_string(), // White
                    "#6E7681".to_string(), // Bright Black
                    "#FFA198".to_string(), // Bright Red
                    "#56D364".to_string(), // Bright Green
                    "#E3B341".to_string(), // Bright Yellow
                    "#79C0FF".to_string(), // Bright Blue
                    "#D2A8FF".to_string(), // Bright Magenta
                    "#56D364".to_string(), // Bright Cyan
                    "#F0F6FC".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }

    pub fn github_light() -> Theme {
        Theme {
            name: "GitHub Light",
            colors: ColorConfig {
                background: "#FFFFFF".to_string(),
                foreground: "#24292F".to_string(),
                cursor: "#24292F".to_string(),
                cursor_text: "#FFFFFF".to_string(),
                selection_bg: "#DDDFE1".to_string(),
                selection_fg: "#24292F".to_string(),
                ansi: vec![
                    "#6E7781".to_string(), // Black
                    "#CF222E".to_string(), // Red
                    "#116329".to_string(), // Green
                    "#4D2D00".to_string(), // Yellow
                    "#0969DA".to_string(), // Blue
                    "#8250DF".to_string(), // Magenta
                    "#1A7F37".to_string(), // Cyan
                    "#6E7781".to_string(), // White
                    "#6E7781".to_string(), // Bright Black
                    "#A40E26".to_string(), // Bright Red
                    "#1A7F37".to_string(), // Bright Green
                    "#633C01".to_string(), // Bright Yellow
                    "#0550AE".to_string(), // Bright Blue
                    "#8250DF".to_string(), // Bright Magenta
                    "#1A7F37".to_string(), // Bright Cyan
                    "#24292F".to_string(), // Bright White
                ],
                ..ColorConfig::default()
            },
        }
    }
}

/// Apply a theme to a ColorConfig.
pub fn apply_theme(colors: &mut ColorConfig, theme: &Theme) {
    *colors = theme.colors.clone();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_built_in_themes_exist() {
        let themes = Theme::built_in_themes();
        assert!(themes.len() >= 10);
    }

    #[test]
    fn test_find_theme() {
        assert!(Theme::find("Catppuccin Mocha").is_some());
        assert!(Theme::find("catppuccin mocha").is_some());
        assert!(Theme::find("DRACULA").is_some());
        assert!(Theme::find("nonexistent").is_none());
    }

    #[test]
    fn test_theme_names() {
        let names = Theme::theme_names();
        assert!(names.contains(&"Catppuccin Mocha"));
        assert!(names.contains(&"Dracula"));
        assert!(names.contains(&"Tokyo Night"));
    }

    #[test]
    fn test_apply_theme() {
        let mut colors = ColorConfig::default();
        let theme = Theme::dracula();
        apply_theme(&mut colors, &theme);
        assert_eq!(colors.background, "#282A36");
        assert_eq!(colors.foreground, "#F8F8F2");
    }
}