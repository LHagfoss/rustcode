//! Themes shared by the terminal UI and the desktop app (`docs/themes.md`).
//!
//! A theme is a TOML file: the terminal keys every theme has always had
//! (`bg`, `panel`, ... `hover_bg`) plus an optional `[desktop]` table of
//! semantic tokens for the desktop app. A token the file leaves out is
//! derived from the terminal keys ([`resolve_desktop`]), so every theme
//! styles both frontends. Built-in themes ship in `builtin/`; user themes
//! live in `<config>/themes/*.toml`, project themes in `.rustcode/themes/`.

pub mod cli;
pub(crate) mod watch;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Theme used when the config names none, and the fallback for a missing one.
pub const DEFAULT_THEME_NAME: &str = "rustcode";

/// Built-in themes as `(file name, TOML)`. Each is the legacy terminal part
/// followed by a `[desktop]` table, so a user copy written by an older
/// release (the terminal part alone) is recognised and upgraded.
pub const BUILTIN_THEMES: &[(&str, &str)] = &[
    ("rustcode.toml", include_str!("builtin/rustcode.toml")),
    (
        "catppuccin-mocha.toml",
        include_str!("builtin/catppuccin-mocha.toml"),
    ),
    ("default.toml", include_str!("builtin/default.toml")),
    ("rain.toml", include_str!("builtin/rain.toml")),
    ("cozy-rain.toml", include_str!("builtin/cozy-rain.toml")),
    ("light.toml", include_str!("builtin/light.toml")),
    ("nord.toml", include_str!("builtin/nord.toml")),
    ("dracula.toml", include_str!("builtin/dracula.toml")),
    ("tokyo-night.toml", include_str!("builtin/tokyo-night.toml")),
    ("sky.toml", include_str!("builtin/sky.toml")),
];

/// The terminal keys, in file order, with what each colours.
pub const TUI_KEYS: &[(&str, &str)] = &[
    (
        "bg",
        "terminal background (\"reset\" keeps the terminal's own)",
    ),
    ("panel", "panels, composer and cards"),
    ("element", "selected rows and raised elements"),
    ("text", "body text"),
    ("muted", "secondary text, hints"),
    ("primary", "accent: active items, the prompt mark"),
    ("secondary", "second accent"),
    ("green", "success, added lines"),
    ("selection", "selected text"),
    ("tip", "tips and warnings"),
    ("status_border", "status bar and box borders"),
    ("turn_separator", "line between turns"),
    ("notice_bg", "notice background"),
    ("hover_bg", "hovered rows"),
];

/// A colour as written in a theme file: `"#rrggbb"`, `"reset"` (also
/// `"none"`, `"transparent"`), or an `[r, g, b]` array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColorSpec {
    Hex(String),
    Rgb([u8; 3]),
}

/// A parsed colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeColor {
    /// The terminal's own colour.
    Reset,
    Rgb(u8, u8, u8),
}

type Rgb = [u8; 3];

impl ColorSpec {
    pub fn parse(&self) -> Result<ThemeColor, String> {
        match self {
            ColorSpec::Rgb([r, g, b]) => Ok(ThemeColor::Rgb(*r, *g, *b)),
            ColorSpec::Hex(hex) => parse_hex(hex),
        }
    }

    /// The colour, `Reset` when it does not parse (as the terminal UI has
    /// always treated a bad value).
    pub fn color(&self) -> ThemeColor {
        self.parse().unwrap_or(ThemeColor::Reset)
    }

    fn rgb(&self) -> Option<Rgb> {
        match self.parse() {
            Ok(ThemeColor::Rgb(r, g, b)) => Some([r, g, b]),
            _ => None,
        }
    }

    fn text(&self) -> String {
        match self {
            ColorSpec::Hex(hex) => hex.clone(),
            ColorSpec::Rgb(rgb) => hex(*rgb),
        }
    }
}

fn parse_hex(hex: &str) -> Result<ThemeColor, String> {
    let s = hex.trim_start_matches('#');
    if ["reset", "none", "transparent"]
        .iter()
        .any(|word| s.eq_ignore_ascii_case(word))
    {
        return Ok(ThemeColor::Reset);
    }
    if s.len() == 6 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(0);
        return Ok(ThemeColor::Rgb(byte(0), byte(2), byte(4)));
    }
    Err(format!(
        "'{hex}' is not a colour: use \"#rrggbb\" or \"reset\""
    ))
}

fn hex([r, g, b]: Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

macro_rules! desktop_tokens {
    ($(($field:ident, $doc:literal)),* $(,)?) => {
        /// The `[desktop]` table as written: every entry optional.
        #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
        pub struct DesktopTable {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub mode: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub radius: Option<String>,
            $(
                #[serde(default, skip_serializing_if = "Option::is_none")]
                pub $field: Option<ColorSpec>,
            )*
        }

        impl DesktopTable {
            fn token(&self, key: &str) -> Option<&ColorSpec> {
                match key {
                    $(stringify!($field) => self.$field.as_ref(),)*
                    _ => None,
                }
            }
        }

        /// Every desktop token resolved to `#rrggbb`.
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        pub struct DesktopTokens {
            $(pub $field: String,)*
        }

        impl DesktopTokens {
            fn from_map(map: &BTreeMap<&'static str, Rgb>) -> Self {
                Self { $($field: hex(map[stringify!($field)]),)* }
            }

            pub fn get(&self, key: &str) -> Option<&str> {
                match key {
                    $(stringify!($field) => Some(&self.$field),)*
                    _ => None,
                }
            }
        }

        /// The desktop colour tokens, in file order, with what each colours.
        pub const TOKENS: &[(&str, &str)] = &[$((stringify!($field), $doc)),*];
    };
}

desktop_tokens!(
    (bg, "window and chat background"),
    (sidebar, "sidebar background"),
    (surface, "cards, composer, popovers"),
    (surface_2, "hovered and raised surfaces"),
    (surface_3, "selected rows, pressed controls"),
    (border, "hairlines between areas"),
    (border_strong, "control outlines, dividers that must read"),
    (text, "body text"),
    (text_muted, "secondary text, labels"),
    (text_faint, "placeholders, timestamps, disabled"),
    (accent, "primary buttons, links, active state"),
    (accent_text, "text on an accent fill"),
    (danger, "errors, destructive actions"),
    (success, "success, added lines"),
    (warning, "warnings, pending approvals"),
    (focus, "keyboard focus ring"),
    (selection, "selected text background"),
    (code_bg, "code blocks and terminal output"),
    (scrollbar, "scrollbar thumb"),
);

/// A theme file.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop: Option<DesktopTable>,
}

impl ThemeFile {
    pub fn tui_key(&self, key: &str) -> Option<&ColorSpec> {
        Some(match key {
            "bg" => &self.bg,
            "panel" => &self.panel,
            "element" => &self.element,
            "text" => &self.text,
            "muted" => &self.muted,
            "primary" => &self.primary,
            "secondary" => &self.secondary,
            "green" => &self.green,
            "selection" => &self.selection,
            "tip" => &self.tip,
            "status_border" => &self.status_border,
            "turn_separator" => &self.turn_separator,
            "notice_bg" => &self.notice_bg,
            "hover_bg" => &self.hover_bg,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Radius {
    Sharp,
    Default,
    Round,
}

impl Mode {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }
}

impl Radius {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "sharp" => Some(Self::Sharp),
            "default" => Some(Self::Default),
            "round" => Some(Self::Round),
            _ => None,
        }
    }
}

/// Where a theme comes from. A user file identical to a built-in counts as
/// the built-in (the copy `ensure_user_dir` wrote).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeSource {
    Builtin,
    User,
    Project,
}

/// The desktop side of a theme, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Desktop {
    pub mode: Mode,
    pub radius: Radius,
    pub tokens: DesktopTokens,
}

/// A loaded theme.
#[derive(Debug, Clone)]
pub struct Theme {
    pub file: ThemeFile,
    pub source: ThemeSource,
    pub path: Option<PathBuf>,
    pub desktop: Desktop,
}

impl Theme {
    pub fn new(file: ThemeFile, source: ThemeSource, path: Option<PathBuf>) -> Self {
        let desktop = resolve_desktop(&file);
        Self {
            file,
            source,
            path,
            desktop,
        }
    }

    pub fn name(&self) -> &str {
        &self.file.name
    }

    pub fn description(&self) -> &str {
        self.file.description.as_deref().unwrap_or(&self.file.name)
    }

    pub fn entry(&self) -> ThemeEntry {
        ThemeEntry {
            name: self.file.name.clone(),
            description: self.description().to_owned(),
            source: self.source,
            path: self.path.as_ref().map(|path| path.display().to_string()),
            mode: self.desktop.mode,
            tokens: self.desktop.tokens.clone(),
            radius: self.desktop.radius,
        }
    }
}

// ---------------------------------------------------------------------------
// Derivation

fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let channel = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    [
        channel(a[0], b[0]),
        channel(a[1], b[1]),
        channel(a[2], b[2]),
    ]
}

fn luminance(rgb: Rgb) -> f64 {
    let linear = |c: u8| {
        let c = c as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2])
}

/// WCAG contrast ratio of two colours.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The terminal UI's light test (`is_light_mode`).
fn is_light(rgb: Rgb) -> bool {
    (rgb[0] as u32 * 299 + rgb[1] as u32 * 587 + rgb[2] as u32 * 114) / 1000 > 140
}

const WHITE: Rgb = [0xff, 0xff, 0xff];
const BLACK: Rgb = [0, 0, 0];

/// Resolve the desktop side of `file`: each token from `[desktop]` when it
/// is a concrete colour, else derived from the terminal keys and the tokens
/// resolved before it (`docs/themes.md`, *Derivation*).
pub fn resolve_desktop(file: &ThemeFile) -> Desktop {
    let table = file.desktop.clone().unwrap_or_default();
    let tui = |key: &str| file.tui_key(key).and_then(ColorSpec::rgb);
    let set = |key: &str| table.token(key).and_then(ColorSpec::rgb);

    let mode = table
        .mode
        .as_deref()
        .and_then(Mode::parse)
        .unwrap_or_else(|| {
            let light = match set("bg").or_else(|| tui("bg")) {
                Some(bg) => is_light(bg),
                None => tui("text").is_some_and(|text| !is_light(text)),
            };
            if light { Mode::Light } else { Mode::Dark }
        });
    let dark = mode == Mode::Dark;
    let by_mode = |dark_value: Rgb, light_value: Rgb| if dark { dark_value } else { light_value };

    let mut map: BTreeMap<&'static str, Rgb> = BTreeMap::new();
    let put = |map: &mut BTreeMap<&'static str, Rgb>, key: &'static str, derived: Rgb| {
        let value = set(key).unwrap_or(derived);
        map.insert(key, value);
        value
    };

    let bg = put(
        &mut map,
        "bg",
        tui("bg").unwrap_or_else(|| {
            let panel = tui("panel").unwrap_or(by_mode([0x0e, 0x0e, 0x10], WHITE));
            if dark {
                mix(panel, BLACK, 0.25)
            } else {
                mix(panel, WHITE, 0.5)
            }
        }),
    );
    let text = put(
        &mut map,
        "text",
        tui("text").unwrap_or(by_mode([0xed, 0xed, 0xed], [0x1a, 0x1a, 0x1a])),
    );
    let step = |t: f64| mix(bg, text, t);
    put(&mut map, "sidebar", tui("panel").unwrap_or(step(0.02)));
    put(&mut map, "surface", step(0.04));
    put(&mut map, "surface_2", tui("hover_bg").unwrap_or(step(0.07)));
    put(&mut map, "surface_3", tui("element").unwrap_or(step(0.11)));
    let strong = tui("status_border");
    put(
        &mut map,
        "border",
        strong.map_or(step(0.08), |strong| mix(bg, strong, 0.6)),
    );
    put(&mut map, "border_strong", strong.unwrap_or(step(0.16)));
    let muted = put(
        &mut map,
        "text_muted",
        tui("muted").unwrap_or(mix(text, bg, 0.35)),
    );
    put(&mut map, "text_faint", mix(muted, bg, 0.4));
    let accent = put(
        &mut map,
        "accent",
        tui("primary").unwrap_or(by_mode([0x93, 0xa3, 0xe8], [0x3b, 0x5b, 0xdb])),
    );
    let on_accent = [WHITE, [0x0b, 0x0b, 0x0c]]
        .into_iter()
        .max_by(|a, b| contrast(*a, accent).total_cmp(&contrast(*b, accent)))
        .unwrap_or(WHITE);
    put(&mut map, "accent_text", on_accent);
    put(
        &mut map,
        "danger",
        by_mode([0xf2, 0x6d, 0x6d], [0xc6, 0x28, 0x28]),
    );
    put(
        &mut map,
        "success",
        tui("green").unwrap_or(by_mode([0x5f, 0xbf, 0x8a], [0x1f, 0x7a, 0x35])),
    );
    put(
        &mut map,
        "warning",
        tui("tip").unwrap_or(by_mode([0xe0, 0xb2, 0x5c], [0x9a, 0x5f, 0x12])),
    );
    put(&mut map, "focus", accent);
    put(&mut map, "selection", mix(bg, accent, 0.25));
    put(&mut map, "code_bg", step(0.025));
    put(&mut map, "scrollbar", step(0.15));

    Desktop {
        mode,
        radius: table
            .radius
            .as_deref()
            .and_then(Radius::parse)
            .unwrap_or(Radius::Default),
        tokens: DesktopTokens::from_map(&map),
    }
}

// ---------------------------------------------------------------------------
// Validation

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub level: Level,
    pub message: String,
}

impl Diagnostic {
    fn error(message: impl Into<String>) -> Self {
        Self {
            level: Level::Error,
            message: message.into(),
        }
    }

    fn warning(message: impl Into<String>) -> Self {
        Self {
            level: Level::Warning,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = match self.level {
            Level::Error => "error",
            Level::Warning => "warning",
        };
        write!(f, "{level}: {}", self.message)
    }
}

/// Minimum contrast of `text` on `bg` before validation warns (WCAG AA).
pub const MIN_TEXT_CONTRAST: f64 = 4.5;

/// Check a theme file's text. Errors make the file unusable or a value
/// ignored (a bad colour falls back as described in `docs/themes.md`);
/// warnings never stop it loading.
pub fn validate(text: &str) -> (Option<ThemeFile>, Vec<Diagnostic>) {
    let mut out = Vec::new();
    let table: toml::Table = match toml::from_str(text) {
        Ok(table) => table,
        Err(error) => {
            out.push(Diagnostic::error(format!(
                "invalid TOML: {}",
                error.message()
            )));
            return (None, out);
        }
    };
    for key in table.keys() {
        let known = matches!(key.as_str(), "name" | "description" | "desktop")
            || TUI_KEYS.iter().any(|(tui, _)| tui == key);
        if !known {
            out.push(Diagnostic::warning(format!(
                "unknown key `{key}` is ignored"
            )));
        }
    }
    match table.get("desktop") {
        Some(toml::Value::Table(desktop)) => {
            for key in desktop.keys() {
                let known = matches!(key.as_str(), "mode" | "radius")
                    || TOKENS.iter().any(|(token, _)| token == key);
                if !known {
                    out.push(Diagnostic::warning(format!(
                        "unknown key `desktop.{key}` is ignored"
                    )));
                }
            }
        }
        Some(_) => {
            out.push(Diagnostic::error("`desktop` must be a table ([desktop])"));
            return (None, out);
        }
        None => {}
    }
    let file: ThemeFile = match toml::from_str(text) {
        Ok(file) => file,
        Err(error) => {
            out.push(Diagnostic::error(error.message().to_owned()));
            return (None, out);
        }
    };
    if file.name.trim().is_empty() {
        out.push(Diagnostic::error("`name` is empty"));
    }
    for (key, _) in TUI_KEYS {
        if let Some(Err(error)) = file.tui_key(key).map(ColorSpec::parse) {
            out.push(Diagnostic::error(format!("`{key}`: {error}")));
        }
    }
    if let Some(desktop) = &file.desktop {
        for (key, _) in TOKENS {
            match desktop.token(key).map(ColorSpec::parse) {
                Some(Err(error)) => {
                    out.push(Diagnostic::error(format!("`desktop.{key}`: {error}")))
                }
                Some(Ok(ThemeColor::Reset)) => out.push(Diagnostic::error(format!(
                    "`desktop.{key}`: needs a colour, not \"reset\""
                ))),
                _ => {}
            }
        }
        if let Some(mode) = &desktop.mode
            && Mode::parse(mode).is_none()
        {
            out.push(Diagnostic::error(format!(
                "`desktop.mode` is '{mode}': use \"dark\" or \"light\""
            )));
        }
        if let Some(radius) = &desktop.radius
            && Radius::parse(radius).is_none()
        {
            out.push(Diagnostic::error(format!(
                "`desktop.radius` is '{radius}': use \"sharp\", \"default\" or \"round\""
            )));
        }
    }
    let resolved = resolve_desktop(&file);
    let rgb = |text: &str| ColorSpec::Hex(text.to_owned()).rgb().unwrap_or(BLACK);
    let ratio = contrast(rgb(&resolved.tokens.text), rgb(&resolved.tokens.bg));
    if ratio < MIN_TEXT_CONTRAST {
        out.push(Diagnostic::warning(format!(
            "desktop text on bg has contrast {ratio:.2}:1, below {MIN_TEXT_CONTRAST}:1"
        )));
    }
    (Some(file), out)
}

pub fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(|d| d.level == Level::Error)
}

// ---------------------------------------------------------------------------
// Loading

/// `<config>/themes`.
pub fn user_themes_dir() -> Option<PathBuf> {
    Some(crate::config::get_config_dir()?.join("themes"))
}

/// The project theme directories of `workspace`: each ancestor's
/// `.rustcode/themes`, farthest first, so a closer one wins.
pub fn project_themes_dirs(workspace: &Path) -> Vec<PathBuf> {
    let workspace = fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let mut dirs: Vec<PathBuf> = workspace
        .ancestors()
        .map(|ancestor| {
            ancestor
                .join(crate::config::PROJECT_CONFIG_DIR)
                .join("themes")
        })
        .filter(|dir| dir.is_dir())
        .collect();
    // The user config dir may sit under the workspace (`~/.rustcode` is not
    // it, but a test or a custom RUSTCODE_CONFIG_DIR could be).
    if let Some(user) = user_themes_dir() {
        dirs.retain(|dir| *dir != user);
    }
    dirs.reverse();
    dirs
}

/// The terminal part of a built-in (what older releases wrote to disk).
fn legacy_builtin(content: &str) -> Option<&str> {
    content
        .find("\n\n[desktop]")
        .map(|index| &content[..index + 1])
}

/// Create the user themes directory and write each built-in that is not
/// there. A copy an older release wrote (the terminal part alone, unedited)
/// is upgraded to the current built-in; an edited one is never touched.
pub fn ensure_user_dir() -> Option<PathBuf> {
    let dir = user_themes_dir()?;
    let _ = fs::create_dir_all(&dir);
    for (filename, content) in BUILTIN_THEMES {
        let path = dir.join(filename);
        match fs::read_to_string(&path) {
            Ok(existing) => {
                if legacy_builtin(content) == Some(existing.as_str()) {
                    let _ = crate::worktrees::write_atomic(&path, content.as_bytes());
                }
            }
            Err(_) => {
                if !path.exists() {
                    let _ = fs::write(&path, content);
                }
            }
        }
    }
    Some(dir)
}

fn builtin_named(name: &str) -> Option<&'static str> {
    BUILTIN_THEMES
        .iter()
        .map(|(_, content)| *content)
        .find(|content| {
            toml::from_str::<ThemeFile>(content)
                .is_ok_and(|file| file.name.eq_ignore_ascii_case(name))
        })
}

pub fn builtin_themes() -> Vec<Theme> {
    BUILTIN_THEMES
        .iter()
        .filter_map(|(_, content)| toml::from_str::<ThemeFile>(content).ok())
        .map(|file| Theme::new(file, ThemeSource::Builtin, None))
        .collect()
}

/// The `.toml` files of `dir`, by file name.
pub fn theme_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("toml"))
        .collect();
    files.sort();
    files
}

fn load_dir(dir: &Path, source: ThemeSource, into: &mut BTreeMap<String, Theme>) {
    for path in theme_files(dir) {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(file) = toml::from_str::<ThemeFile>(&content) else {
            continue;
        };
        let source = if source == ThemeSource::User
            && builtin_named(&file.name).is_some_and(|builtin| builtin == content)
        {
            ThemeSource::Builtin
        } else {
            source
        };
        into.insert(
            file.name.to_ascii_lowercase(),
            Theme::new(file, source, Some(path)),
        );
    }
}

/// Every theme: built-ins, then user themes, then project themes of
/// `workspace` (closer wins), one per name (case-insensitive). The default
/// theme comes first, the rest by name. Files that do not parse are left out.
pub fn load_themes(workspace: Option<&Path>) -> Vec<Theme> {
    let mut by_name = BTreeMap::new();
    for theme in builtin_themes() {
        by_name.insert(theme.file.name.to_ascii_lowercase(), theme);
    }
    if let Some(dir) = user_themes_dir() {
        load_dir(&dir, ThemeSource::User, &mut by_name);
    }
    if let Some(workspace) = workspace {
        for dir in project_themes_dirs(workspace) {
            load_dir(&dir, ThemeSource::Project, &mut by_name);
        }
    }
    let mut themes: Vec<Theme> = by_name.into_values().collect();
    themes.sort_by(|a, b| {
        let default = |t: &Theme| t.file.name != DEFAULT_THEME_NAME;
        default(a)
            .cmp(&default(b))
            .then_with(|| a.file.name.cmp(&b.file.name))
    });
    themes
}

pub fn find<'a>(themes: &'a [Theme], name: &str) -> Option<&'a Theme> {
    themes
        .iter()
        .find(|theme| theme.file.name.eq_ignore_ascii_case(name))
}

// ---------------------------------------------------------------------------
// Snapshot (`_rustcode/themes`)

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeEntry {
    pub name: String,
    pub description: String,
    pub source: ThemeSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub mode: Mode,
    pub tokens: DesktopTokens,
    pub radius: Radius,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemesSnapshot {
    pub active: String,
    pub themes: Vec<ThemeEntry>,
}

/// The theme the config names, as the theme list spells it; the default
/// when the config names a theme that does not exist.
pub fn active_name(themes: &[Theme], workspace: Option<&Path>) -> String {
    let configured = match workspace {
        Some(workspace) => crate::config::load_config_for_workspace(workspace).2.theme,
        None => crate::config::load_config().2.theme,
    };
    find(themes, &configured)
        .or_else(|| find(themes, DEFAULT_THEME_NAME))
        .map(|theme| theme.file.name.clone())
        .unwrap_or_else(|| DEFAULT_THEME_NAME.to_owned())
}

pub fn snapshot(workspace: Option<&Path>) -> ThemesSnapshot {
    let themes = load_themes(workspace);
    ThemesSnapshot {
        active: active_name(&themes, workspace),
        themes: themes.iter().map(Theme::entry).collect(),
    }
}

/// Make `name` the active theme in the user config, as `/theme` does.
pub fn set_active(name: &str, workspace: Option<&Path>) -> Result<ThemesSnapshot, String> {
    let themes = load_themes(workspace);
    let theme = find(&themes, name).ok_or_else(|| format!("no theme named '{name}'"))?;
    crate::config::save_theme(&theme.file.name)?;
    Ok(snapshot(workspace))
}

// ---------------------------------------------------------------------------
// New themes

/// A theme name usable as a file name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        && name.as_bytes()[0].is_ascii_alphanumeric()
}

/// A commented theme file named `name`, with `base`'s colours.
pub fn template(name: &str, base: &Theme) -> String {
    let mut out = String::new();
    out.push_str("# RustCode theme (docs/themes.md).\n");
    out.push_str("# Colours are \"#rrggbb\". Save and the desktop app restyles at once;\n");
    out.push_str(&format!(
        "# check it with `rustcode theme validate {name}`.\n"
    ));
    out.push_str(&format!("name = \"{name}\"\n"));
    out.push_str(&format!(
        "description = \"Based on {}\"\n\n",
        base.file.name
    ));
    out.push_str("# Terminal UI. \"reset\" keeps the terminal's own colour.\n");
    let pad = |key: &str, value: &str| format!("{key} = \"{value}\"");
    for (key, doc) in TUI_KEYS {
        let value = base
            .file
            .tui_key(key)
            .map(ColorSpec::text)
            .unwrap_or_default();
        out.push_str(&format!("{:<34} # {doc}\n", pad(key, &value)));
    }
    out.push_str("\n# Desktop app. A token left out is derived from the keys above.\n");
    out.push_str("[desktop]\n");
    let mode = match base.desktop.mode {
        Mode::Dark => "dark",
        Mode::Light => "light",
    };
    let radius = match base.desktop.radius {
        Radius::Sharp => "sharp",
        Radius::Default => "default",
        Radius::Round => "round",
    };
    out.push_str(&format!("{:<34} # dark | light\n", pad("mode", mode)));
    out.push_str(&format!(
        "{:<34} # sharp | default | round\n",
        pad("radius", radius)
    ));
    for (key, doc) in TOKENS {
        let value = base.desktop.tokens.get(key).unwrap_or_default();
        out.push_str(&format!("{:<34} # {doc}\n", pad(key, value)));
    }
    out
}

/// Write a new user theme `name` from `from` (the default theme when
/// `None`) and return its path.
pub fn create(name: &str, from: Option<&str>, workspace: Option<&Path>) -> Result<PathBuf, String> {
    if !valid_name(name) {
        return Err(format!(
            "'{name}' is not a theme name: use lowercase letters, digits, '-' and '_'"
        ));
    }
    let themes = load_themes(workspace);
    if find(&themes, name).is_some() {
        return Err(format!("a theme named '{name}' already exists"));
    }
    let from = from.unwrap_or(DEFAULT_THEME_NAME);
    let base = find(&themes, from).ok_or_else(|| format!("no theme named '{from}'"))?;
    let dir = ensure_user_dir().ok_or("could not determine the config directory")?;
    let path = dir.join(format!("{name}.toml"));
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    fs::write(&path, template(name, base))
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests;
