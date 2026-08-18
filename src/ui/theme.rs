use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColorSpec {
    Hex(String),
    Rgb([u8; 3]),
}

impl ColorSpec {
    pub fn to_color(&self) -> Color {
        match self {
            ColorSpec::Rgb([r, g, b]) => Color::Rgb(*r, *g, *b),
            ColorSpec::Hex(hex) => parse_hex_color(hex).unwrap_or(Color::Reset),
        }
    }
}

fn parse_hex_color(hex: &str) -> Option<Color> {
    let s = hex.trim_start_matches('#');
    if s.eq_ignore_ascii_case("reset")
        || s.eq_ignore_ascii_case("none")
        || s.eq_ignore_ascii_case("transparent")
    {
        return Some(Color::Reset);
    }
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Color::Rgb(r, g, b))
    } else {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeFile {
    pub name: String,
    pub description: Option<String>,
    pub bg: ColorSpec,
    pub panel: ColorSpec,
    pub element: ColorSpec,
    pub text: ColorSpec,
    pub muted: ColorSpec,
    pub primary: ColorSpec,
    pub secondary: ColorSpec,
    pub green: ColorSpec,
    pub selection: ColorSpec,
    pub tip: ColorSpec,
    pub status_border: ColorSpec,
    pub turn_separator: ColorSpec,
    pub notice_bg: ColorSpec,
    pub hover_bg: ColorSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePalette {
    pub name: String,
    pub description: String,
    pub bg: Color,
    pub panel: Color,
    pub element: Color,
    pub text: Color,
    pub muted: Color,
    pub primary: Color,
    pub secondary: Color,
    pub green: Color,
    pub selection: Color,
    pub tip: Color,
    pub status_border: Color,
    pub turn_separator: Color,
    pub notice_bg: Color,
    pub hover_bg: Color,
}

impl From<&ThemeFile> for ThemePalette {
    fn from(f: &ThemeFile) -> Self {
        Self {
            name: f.name.clone(),
            description: f.description.clone().unwrap_or_else(|| f.name.clone()),
            bg: f.bg.to_color(),
            panel: f.panel.to_color(),
            element: f.element.to_color(),
            text: f.text.to_color(),
            muted: f.muted.to_color(),
            primary: f.primary.to_color(),
            secondary: f.secondary.to_color(),
            green: f.green.to_color(),
            selection: f.selection.to_color(),
            tip: f.tip.to_color(),
            status_border: f.status_border.to_color(),
            turn_separator: f.turn_separator.to_color(),
            notice_bg: f.notice_bg.to_color(),
            hover_bg: f.hover_bg.to_color(),
        }
    }
}

static BUILTIN_THEMES: &[(&str, &str)] = &[
    (
        "default.toml",
        r##"name = "default"
description = "Default dark palette (Cozy Rain)"
bg = "reset"
panel = "#15171a"
element = "#22262a"
text = "#f0e5de"
muted = "#88929a"
primary = "#ec6e5d"
secondary = "#3c5865"
green = "#a6e3a1"
selection = "#f0e5de"
tip = "#e0a96d"
status_border = "#3c5865"
turn_separator = "#5a707e"
notice_bg = "reset"
hover_bg = "#2b3035"
"##,
    ),
    (
        "rain.toml",
        r##"name = "rain"
description = "Cozy Rain warm dark palette"
bg = "#15171a"
panel = "#1a1d20"
element = "#22262a"
text = "#f0e5de"
muted = "#88929a"
primary = "#ec6e5d"
secondary = "#3c5865"
green = "#a6e3a1"
selection = "#f0e5de"
tip = "#e0a96d"
status_border = "#3c5865"
turn_separator = "#5a707e"
notice_bg = "reset"
hover_bg = "#2b3035"
"##,
    ),
    (
        "cozy-rain.toml",
        r##"name = "cozy-rain"
description = "Cozy Rain warm dark palette"
bg = "#15171a"
panel = "#1a1d20"
element = "#22262a"
text = "#f0e5de"
muted = "#88929a"
primary = "#ec6e5d"
secondary = "#3c5865"
green = "#a6e3a1"
selection = "#f0e5de"
tip = "#e0a96d"
status_border = "#3c5865"
turn_separator = "#5a707e"
notice_bg = "reset"
hover_bg = "#2b3035"
"##,
    ),
    (
        "light.toml",
        r##"name = "light"
description = "Clean light mode text and border palette"
bg = "#f5f7fa"
panel = "#e6eaf0"
element = "#d7dde4"
text = "#1e232a"
muted = "#5e6872"
primary = "#d74130"
secondary = "#2d6e91"
green = "#239b41"
selection = "#1e1e1e"
tip = "#be781e"
status_border = "#aab3bc"
turn_separator = "#b9bec3"
notice_bg = "reset"
hover_bg = "#d2d8e0"
"##,
    ),
    (
        "nord.toml",
        r##"name = "nord"
description = "Arctic nord text and border palette"
bg = "#2e3440"
panel = "#3b4252"
element = "#434c5e"
text = "#eceff4"
muted = "#d8dee9"
primary = "#88c0d0"
secondary = "#81a1c1"
green = "#a3be8c"
selection = "#eceff4"
tip = "#ebcb8b"
status_border = "#4c566a"
turn_separator = "#5e6b84"
notice_bg = "reset"
hover_bg = "#4c566a"
"##,
    ),
    (
        "dracula.toml",
        r##"name = "dracula"
description = "Vibrant dracula text and border palette"
bg = "#282a36"
panel = "#44475a"
element = "#6272a4"
text = "#f8f8f2"
muted = "#6272a4"
primary = "#ff79c6"
secondary = "#bd93f9"
green = "#50fa7b"
selection = "#f8f8f2"
tip = "#f1fa8c"
status_border = "#6272a4"
turn_separator = "#6272a4"
notice_bg = "reset"
hover_bg = "#44475a"
"##,
    ),
    (
        "tokyo-night.toml",
        r##"name = "tokyo-night"
description = "Tokyo night text and border palette"
bg = "#1a1b26"
panel = "#24283b"
element = "#292e42"
text = "#c0caf5"
muted = "#565f89"
primary = "#f7768e"
secondary = "#7aa2f7"
green = "#9ece6a"
selection = "#c0caf5"
tip = "#e0af68"
status_border = "#414868"
turn_separator = "#565f89"
notice_bg = "reset"
hover_bg = "#292e42"
"##,
    ),
    (
        "sky.toml",
        r##"name = "sky"
description = "Summer sky azure and meadow green palette"
bg = "reset"
panel = "#162032"
element = "#1e2c44"
text = "#f0f6fc"
muted = "#7890a8"
primary = "#3894f0"
secondary = "#88c438"
green = "#88c438"
selection = "#f0f6fc"
tip = "#ffd152"
status_border = "#2a3d5c"
turn_separator = "#48658a"
notice_bg = "reset"
hover_bg = "#253754"
"##,
    ),
];

pub fn get_themes_dir() -> Option<PathBuf> {
    let config_dir = crate::config::get_config_dir()?;
    Some(config_dir.join("themes"))
}

pub fn ensure_themes_dir() -> Option<PathBuf> {
    let themes_dir = get_themes_dir()?;
    if !themes_dir.exists() {
        let _ = fs::create_dir_all(&themes_dir);
    }

    for (filename, content) in BUILTIN_THEMES {
        let file_path = themes_dir.join(filename);
        let _ = fs::write(file_path, content);
    }

    Some(themes_dir)
}

pub fn load_available_themes() -> Vec<ThemePalette> {
    let mut themes = Vec::new();
    let themes_dir = ensure_themes_dir();

    if let Some(dir) = themes_dir {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Ok(file_struct) = toml::from_str::<ThemeFile>(&content) {
                            themes.push(ThemePalette::from(&file_struct));
                        }
                    }
                }
            }
        }
    }

    if themes.is_empty() {
        for (_, content) in BUILTIN_THEMES {
            if let Ok(file_struct) = toml::from_str::<ThemeFile>(content) {
                themes.push(ThemePalette::from(&file_struct));
            }
        }
    }

    themes.sort_by(|a, b| {
        if a.name == "default" {
            std::cmp::Ordering::Less
        } else if b.name == "default" {
            std::cmp::Ordering::Greater
        } else {
            a.name.cmp(&b.name)
        }
    });

    themes
}

pub fn get_palette(name: &str) -> ThemePalette {
    let themes = load_available_themes();
    themes
        .into_iter()
        .find(|t| t.name.eq_ignore_ascii_case(name))
        .unwrap_or_else(|| ThemePalette {
            name: "default".to_string(),
            description: "Default dark palette (Cozy Rain)".to_string(),
            bg: Color::Rgb(21, 23, 26),
            panel: Color::Rgb(21, 23, 26),
            element: Color::Rgb(34, 38, 42),
            text: Color::Rgb(240, 229, 222),
            muted: Color::Rgb(136, 146, 154),
            primary: Color::Rgb(236, 110, 93),
            secondary: Color::Rgb(60, 88, 101),
            green: Color::Rgb(166, 227, 161),
            selection: Color::Rgb(240, 229, 222),
            tip: Color::Rgb(224, 169, 109),
            status_border: Color::Rgb(60, 88, 101),
            turn_separator: Color::Rgb(90, 112, 126),
            notice_bg: Color::Reset,
            hover_bg: Color::Rgb(43, 48, 53),
        })
}

use std::sync::RwLock;

static ACTIVE_THEME: RwLock<Option<ThemePalette>> = RwLock::new(None);

pub fn set_active_theme(name: &str) {
    let palette = get_palette(name);
    if let Ok(mut guard) = ACTIVE_THEME.write() {
        *guard = Some(palette);
    }
}

pub fn active_palette() -> ThemePalette {
    if let Ok(guard) = ACTIVE_THEME.read() {
        if let Some(p) = guard.as_ref() {
            return p.clone();
        }
    }
    get_palette("default")
}

pub fn color_bg() -> Color {
    active_palette().bg
}
pub fn color_panel() -> Color {
    active_palette().panel
}
pub fn color_element() -> Color {
    active_palette().element
}
pub fn color_text() -> Color {
    active_palette().text
}
pub fn color_muted() -> Color {
    active_palette().muted
}
pub fn color_primary() -> Color {
    active_palette().primary
}
pub fn color_secondary() -> Color {
    active_palette().secondary
}
pub fn color_green() -> Color {
    active_palette().green
}
pub fn color_selection() -> Color {
    active_palette().selection
}
pub fn color_tip() -> Color {
    active_palette().tip
}
#[allow(dead_code)]
pub fn color_status_border() -> Color {
    active_palette().status_border
}
pub fn color_turn_separator() -> Color {
    active_palette().turn_separator
}
pub fn color_hover_bg() -> Color {
    active_palette().hover_bg
}

pub fn is_light_mode() -> bool {
    let c = active_palette().bg;
    match c {
        Color::Rgb(r, g, b) => (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000 > 140,
        Color::White | Color::Gray | Color::Yellow | Color::LightYellow => true,
        _ => false,
    }
}

#[allow(dead_code)]
pub fn color_diff_add_bg() -> Color {
    if is_light_mode() {
        Color::Rgb(225, 245, 225)
    } else {
        Color::Rgb(24, 40, 24)
    }
}

pub fn color_diff_add_fg() -> Color {
    if is_light_mode() {
        Color::Rgb(30, 120, 40)
    } else {
        Color::Rgb(160, 240, 160)
    }
}

#[allow(dead_code)]
pub fn color_diff_remove_bg() -> Color {
    if is_light_mode() {
        Color::Rgb(252, 230, 230)
    } else {
        Color::Rgb(48, 20, 20)
    }
}

pub fn color_diff_remove_fg() -> Color {
    if is_light_mode() {
        Color::Rgb(180, 40, 40)
    } else {
        Color::Rgb(240, 150, 150)
    }
}

#[allow(dead_code)]
pub fn color_diff_absent_bg() -> Color {
    if is_light_mode() {
        Color::Rgb(235, 240, 245)
    } else {
        Color::Rgb(22, 22, 26)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_themes_define_visible_chat_surfaces() {
        for (_, content) in BUILTIN_THEMES {
            let file = toml::from_str::<ThemeFile>(content).unwrap();
            let palette = ThemePalette::from(&file);
            assert_ne!(palette.panel, Color::Reset);
        }
    }
}
