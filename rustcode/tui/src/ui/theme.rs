//! Terminal rendering of the shared themes (`rustcode::controller::themes`,
//! `docs/themes.md`): the files, built-ins and parsing live in the engine.

use ratatui::style::Color;
pub use rustcode::controller::themes::{ColorSpec, DEFAULT_THEME_NAME, ThemeFile};
use std::path::PathBuf;

fn to_color(spec: &ColorSpec) -> Color {
    match spec.color() {
        rustcode::controller::themes::ThemeColor::Reset => Color::Reset,
        rustcode::controller::themes::ThemeColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
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
            bg: to_color(&f.bg),
            panel: to_color(&f.panel),
            element: to_color(&f.element),
            text: to_color(&f.text),
            muted: to_color(&f.muted),
            primary: to_color(&f.primary),
            secondary: to_color(&f.secondary),
            green: to_color(&f.green),
            selection: to_color(&f.selection),
            tip: to_color(&f.tip),
            status_border: to_color(&f.status_border),
            turn_separator: to_color(&f.turn_separator),
            notice_bg: to_color(&f.notice_bg),
            hover_bg: to_color(&f.hover_bg),
        }
    }
}

pub fn ensure_themes_dir() -> Option<PathBuf> {
    rustcode::controller::themes::ensure_user_dir()
}

/// Every theme (built-in, user, and the current project's), the default
/// first. Writes the built-ins to the user themes directory first, as the
/// terminal UI always has.
pub fn load_available_themes() -> Vec<ThemePalette> {
    ensure_themes_dir();
    let workspace = std::env::current_dir().ok();
    rustcode::controller::themes::load_themes(workspace.as_deref())
        .iter()
        .map(|theme| ThemePalette::from(&theme.file))
        .collect()
}

pub fn get_palette(name: &str) -> ThemePalette {
    // The render tests predate the Catppuccin default and assert the legacy
    // palette's colors for a state that leaves the theme at its default.
    #[cfg(test)]
    let name = if name == DEFAULT_THEME_NAME {
        "default"
    } else {
        name
    };
    let themes = load_available_themes();
    themes
        .into_iter()
        .find(|t| t.name.eq_ignore_ascii_case(name))
        .unwrap_or_else(|| {
            if let Some(theme) = rustcode::controller::themes::find(
                &rustcode::controller::themes::builtin_themes(),
                DEFAULT_THEME_NAME,
            ) {
                return ThemePalette::from(&theme.file);
            }
            ThemePalette {
                name: DEFAULT_THEME_NAME.to_string(),
                description: "Built-in fallback palette".to_string(),
                bg: Color::Reset,
                panel: Color::Rgb(24, 24, 37),
                element: Color::Rgb(49, 50, 68),
                text: Color::Rgb(205, 214, 244),
                muted: Color::Rgb(147, 153, 178),
                primary: Color::Rgb(137, 180, 250),
                secondary: Color::Rgb(116, 199, 236),
                green: Color::Rgb(166, 227, 161),
                selection: Color::Rgb(205, 214, 244),
                tip: Color::Rgb(249, 226, 175),
                status_border: Color::Rgb(69, 71, 90),
                turn_separator: Color::Rgb(88, 91, 112),
                notice_bg: Color::Reset,
                hover_bg: Color::Rgb(49, 50, 68),
            }
        })
}

/// The palette the next frame renders with.
///
/// In the app it is process-global: `render_with_transcript_snapshot` re-applies
/// the configured theme on every frame and every render path reads it back.
///
/// Under `cfg(test)` it is thread-local instead. `cargo test` runs hundreds of
/// render tests in parallel and each of them re-applies its own state's theme,
/// so a shared slot let a render test on one thread overwrite the palette a
/// theme test on another thread had just set, halfway through its assertion.
/// `THEME_TEST_LOCK` cannot prevent that: it only serialises the tests that take
/// it, and the ordinary render tests have no reason to.
#[cfg(not(test))]
static ACTIVE_THEME: std::sync::RwLock<Option<ThemePalette>> = std::sync::RwLock::new(None);

#[cfg(test)]
thread_local! {
    static ACTIVE_THEME: std::cell::RefCell<Option<ThemePalette>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(not(test))]
fn active_theme() -> Option<ThemePalette> {
    ACTIVE_THEME.read().ok().and_then(|guard| guard.clone())
}

#[cfg(test)]
fn active_theme() -> Option<ThemePalette> {
    ACTIVE_THEME.with(|slot| slot.borrow().clone())
}

#[cfg(not(test))]
fn store_active_theme(palette: ThemePalette) {
    if let Ok(mut guard) = ACTIVE_THEME.write() {
        *guard = Some(palette);
    }
}

#[cfg(test)]
fn store_active_theme(palette: ThemePalette) {
    ACTIVE_THEME.with(|slot| *slot.borrow_mut() = Some(palette));
}

pub fn set_active_theme(name: &str) {
    // Every frame re-applies the configured theme. Loading a palette reads the
    // themes directory, so an unchanged name must not pay for that again.
    #[cfg(not(test))]
    if let Ok(guard) = ACTIVE_THEME.read()
        && guard
            .as_ref()
            .is_some_and(|palette| palette.name.eq_ignore_ascii_case(name))
    {
        return;
    }
    store_active_theme(get_palette(name));
}

/// Palette for `name`, served from the active theme when that is the one
/// asked for. Renderers that need a named theme every frame use this instead of
/// `get_palette`, which reads the themes directory on each call.
pub fn palette_for(name: &str) -> ThemePalette {
    if let Some(palette) = active_theme()
        && palette.name.eq_ignore_ascii_case(name)
    {
        return palette;
    }
    get_palette(name)
}

pub fn active_palette() -> ThemePalette {
    if let Some(palette) = active_theme() {
        return palette;
    }
    get_palette(DEFAULT_THEME_NAME)
}

pub fn color_bg() -> Color {
    active_palette().bg
}
pub fn color_panel() -> Color {
    active_palette().panel
}
#[cfg(test)]
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
pub fn color_tip() -> Color {
    active_palette().tip
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

#[cfg(test)]
mod tests {
    use super::*;
    use rustcode::controller::themes::BUILTIN_THEMES;

    #[test]
    fn builtin_themes_define_visible_chat_surfaces() {
        for (_, content) in BUILTIN_THEMES {
            let file = toml::from_str::<ThemeFile>(content).unwrap();
            let palette = ThemePalette::from(&file);
            assert_ne!(palette.panel, Color::Reset);
        }
    }

    #[test]
    fn default_theme_is_the_builtin_rustcode_theme() {
        let names = BUILTIN_THEMES
            .iter()
            .map(|(_, content)| toml::from_str::<ThemeFile>(content).unwrap().name)
            .collect::<Vec<_>>();
        assert!(names.iter().any(|name| name == DEFAULT_THEME_NAME));
        assert_eq!(DEFAULT_THEME_NAME, "rustcode");
        assert!(names.iter().any(|name| name == "catppuccin-mocha"));
    }

    #[test]
    fn existing_theme_files_render_as_before() {
        // A file from before the desktop table, with its exact colours.
        let file: ThemeFile = toml::from_str(
            r##"name = "mine"
bg = "reset"
panel = "#181825"
element = [49, 50, 68]
text = "#cdd6f4"
muted = "#9399b2"
primary = "#89b4fa"
secondary = "#74c7ec"
green = "#a6e3a1"
selection = "#cdd6f4"
tip = "#f9e2af"
status_border = "#45475a"
turn_separator = "bad"
notice_bg = "none"
hover_bg = "#313244"
"##,
        )
        .unwrap();
        let palette = ThemePalette::from(&file);
        assert_eq!(palette.description, "mine");
        assert_eq!(palette.bg, Color::Reset);
        assert_eq!(palette.panel, Color::Rgb(24, 24, 37));
        assert_eq!(palette.element, Color::Rgb(49, 50, 68));
        assert_eq!(palette.turn_separator, Color::Reset, "a bad value");
        assert_eq!(palette.notice_bg, Color::Reset);
    }
}
