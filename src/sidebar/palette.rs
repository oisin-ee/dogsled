//! Herdr's colours, read from its own `[theme]` so the sidebar matches the frame around it.
//!
//! Order follows Herdr 0.9.5 (`src/config/theme.rs`, `src/app/state.rs`): `name` picks a
//! built-in, then `[theme.custom]` overrides single tokens. Only `catppuccin` and
//! `catppuccin-latte` are built in here; any other theme name draws with Herdr's `terminal`
//! ANSI palette. `auto_switch` is not followed: the sidebar cannot see the host appearance.

use ratatui::style::Color;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub accent: Color,
    pub surface0: Color,
    pub surface1: Color,
    pub overlay0: Color,
    pub overlay1: Color,
    pub text: Color,
    pub subtext0: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub teal: Color,
    /// Text on a coloured pill. Herdr has no such token; this is Catppuccin's `crust`.
    pub crust: Color,
}

impl Palette {
    pub const fn catppuccin() -> Self {
        Self {
            accent: Color::Rgb(137, 180, 250),
            surface0: Color::Rgb(49, 50, 68),
            surface1: Color::Rgb(69, 71, 90),
            overlay0: Color::Rgb(108, 112, 134),
            overlay1: Color::Rgb(127, 132, 156),
            text: Color::Rgb(205, 214, 244),
            subtext0: Color::Rgb(166, 173, 200),
            green: Color::Rgb(166, 227, 161),
            yellow: Color::Rgb(249, 226, 175),
            red: Color::Rgb(243, 139, 168),
            teal: Color::Rgb(148, 226, 213),
            crust: Color::Rgb(17, 17, 27),
        }
    }

    pub const fn catppuccin_latte() -> Self {
        Self {
            accent: Color::Rgb(30, 102, 245),
            surface0: Color::Rgb(204, 208, 218),
            surface1: Color::Rgb(188, 192, 204),
            overlay0: Color::Rgb(156, 160, 176),
            overlay1: Color::Rgb(140, 143, 161),
            text: Color::Rgb(76, 79, 105),
            subtext0: Color::Rgb(108, 111, 133),
            green: Color::Rgb(64, 160, 43),
            yellow: Color::Rgb(223, 142, 29),
            red: Color::Rgb(210, 15, 57),
            teal: Color::Rgb(23, 146, 153),
            crust: Color::Rgb(220, 224, 232),
        }
    }

    /// Herdr's `terminal` theme: the terminal's own 16 colours.
    pub const fn terminal() -> Self {
        Self {
            accent: Color::Blue,
            surface0: Color::Reset,
            surface1: Color::DarkGray,
            overlay0: Color::Gray,
            overlay1: Color::White,
            text: Color::Reset,
            subtext0: Color::Gray,
            green: Color::Green,
            yellow: Color::Yellow,
            red: Color::LightRed,
            teal: Color::Cyan,
            crust: Color::Black,
        }
    }

    /// The palette Herdr would draw with for this `config.toml` text. Unreadable or invalid
    /// text gives Herdr's default, `catppuccin`.
    pub fn from_config(text: &str) -> Self {
        let Ok(config) = text.parse::<toml::Table>() else {
            return Self::catppuccin();
        };
        let theme = config.get("theme").and_then(toml::Value::as_table);
        let mut palette = match theme.and_then(|theme| theme.get("name")?.as_str()) {
            None => Self::catppuccin(),
            Some(name) => match name.to_lowercase().replace([' ', '_'], "-").as_str() {
                "catppuccin" | "catppuccin-mocha" => Self::catppuccin(),
                "catppuccin-latte" | "latte" | "light" => Self::catppuccin_latte(),
                _ => Self::terminal(),
            },
        };
        if let Some(custom) = theme.and_then(|theme| theme.get("custom")?.as_table()) {
            let set = |slot: &mut Color, key: &str| {
                if let Some(value) = custom.get(key).and_then(toml::Value::as_str) {
                    *slot = parse_color(value);
                }
            };
            set(&mut palette.accent, "accent");
            set(&mut palette.surface0, "surface0");
            set(&mut palette.surface1, "surface1");
            set(&mut palette.overlay0, "overlay0");
            set(&mut palette.overlay1, "overlay1");
            set(&mut palette.text, "text");
            set(&mut palette.subtext0, "subtext0");
            set(&mut palette.green, "green");
            set(&mut palette.yellow, "yellow");
            set(&mut palette.red, "red");
            set(&mut palette.teal, "teal");
        }
        palette
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map_or_else(|_| Self::catppuccin(), |text| Self::from_config(&text))
    }

    pub fn load_default() -> Self {
        Self::load(&config_path())
    }
}

/// `$XDG_CONFIG_HOME/herdr/config.toml`, else `~/.config/herdr/config.toml`.
pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("herdr").join("config.toml")
}

/// Herdr's `parse_color`: `#rrggbb`, `#rgb`, `rgb(r, g, b)`, names, and the reset aliases.
/// Anything else is cyan, as in Herdr.
pub fn parse_color(value: &str) -> Color {
    let value = value.trim().to_lowercase();
    if matches!(value.as_str(), "reset" | "default" | "none" | "transparent") {
        return Color::Reset;
    }
    if let Some(hex) = value.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .filter_map(|c| c.to_digit(16).and_then(|d| u8::try_from(d).ok()))
            .collect();
        if digits.len() == hex.len() {
            match digits[..] {
                [r1, r0, g1, g0, b1, b0] => {
                    return Color::Rgb(r1 * 16 + r0, g1 * 16 + g0, b1 * 16 + b0);
                }
                [r, g, b] => return Color::Rgb(r * 17, g * 17, b * 17),
                _ => {}
            }
        }
    }
    if let Some(inner) = value.strip_prefix("rgb(").and_then(|v| v.strip_suffix(')')) {
        let parts: Vec<_> = inner.split(',').map(|p| p.trim().parse::<u8>()).collect();
        if let [Ok(r), Ok(g), Ok(b)] = parts[..] {
            return Color::Rgb(r, g, b);
        }
    }
    match value.as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" | "purple" => Color::Magenta,
        "white" => Color::White,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        _ => Color::Cyan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_theme_is_catppuccin_and_latte_is_built_in() {
        assert_eq!(Palette::from_config(""), Palette::catppuccin());
        assert_eq!(
            Palette::from_config("[theme]\nname = \"catppuccin\""),
            Palette::catppuccin()
        );
        assert_eq!(
            Palette::from_config("[theme]\nname = \"catppuccin-latte\""),
            Palette::catppuccin_latte()
        );
        assert_eq!(
            Palette::from_config("[theme]\nname = \"Latte\""),
            Palette::catppuccin_latte()
        );
    }

    #[test]
    fn other_theme_names_use_the_terminal_palette() {
        assert_eq!(
            Palette::from_config("[theme]\nname = \"dracula\""),
            Palette::terminal()
        );
        assert_eq!(
            Palette::from_config("[theme]\nname = \"nonsense\""),
            Palette::terminal()
        );
    }

    #[test]
    fn custom_tokens_override_the_base_in_herdrs_order() {
        let palette = Palette::from_config(
            "[theme]\nname = \"catppuccin-latte\"\n[theme.custom]\naccent = \"#f5c2e7\"\nred = \"rgb(255, 85, 85)\"\ngreen = \"#0f0\"\nteal = \"reset\"\n",
        );
        assert_eq!(palette.accent, Color::Rgb(0xf5, 0xc2, 0xe7));
        assert_eq!(palette.red, Color::Rgb(255, 85, 85));
        assert_eq!(palette.green, Color::Rgb(0, 255, 0));
        assert_eq!(palette.teal, Color::Reset);
        assert_eq!(palette.text, Palette::catppuccin_latte().text);
    }

    #[test]
    fn invalid_toml_and_missing_files_give_the_default() {
        assert_eq!(Palette::from_config("[theme"), Palette::catppuccin());
        assert_eq!(
            Palette::load(Path::new("/nonexistent/herdr.toml")),
            Palette::catppuccin()
        );
    }

    #[test]
    fn colour_parsing_matches_herdr() {
        assert_eq!(parse_color(" #ABC "), Color::Rgb(170, 187, 204));
        assert_eq!(parse_color("DarkGrey"), Color::DarkGray);
        assert_eq!(parse_color("#12"), Color::Cyan);
        assert_eq!(parse_color("rgb(1,2,300)"), Color::Cyan);
    }
}
