//! `rustcode theme ...`: each command's output as text, for the terminal
//! frontend to print. An `Err` is the message to print before exiting 1.

use super::{
    BUILTIN_THEMES, Theme, ThemeSource, ensure_user_dir, find, has_errors, load_themes,
    project_themes_dirs, theme_files, user_themes_dir, validate,
};
use std::path::{Path, PathBuf};

fn source_label(source: ThemeSource) -> &'static str {
    match source {
        ThemeSource::Builtin => "built-in",
        ThemeSource::User => "user",
        ThemeSource::Project => "project",
    }
}

/// `theme list`: one theme per line, `*` marking the active one.
pub fn list(workspace: Option<&Path>) -> String {
    let themes = load_themes(workspace);
    let active = super::active_name(&themes, workspace);
    let width = themes.iter().map(|t| t.name().len()).max().unwrap_or(0);
    themes
        .iter()
        .map(|theme| {
            let mark = if theme.name() == active { '*' } else { ' ' };
            format!(
                "{mark} {:<width$}  {:<8}  {}\n",
                theme.name(),
                source_label(theme.source),
                theme.description()
            )
        })
        .collect()
}

fn named<'a>(themes: &'a [Theme], name: &str) -> Result<&'a Theme, String> {
    find(themes, name).ok_or_else(|| format!("no theme named '{name}' (see `rustcode theme list`)"))
}

/// `theme show <name> [--json]`: the resolved theme, as the desktop app
/// gets it in `_rustcode/themes`.
pub fn show(name: &str, json: bool, workspace: Option<&Path>) -> Result<String, String> {
    let themes = load_themes(workspace);
    let theme = named(&themes, name)?;
    let entry = theme.entry();
    if json {
        return serde_json::to_string_pretty(&entry)
            .map(|text| text + "\n")
            .map_err(|error| error.to_string());
    }
    let mut out = format!(
        "{}  ({})\n{}\n",
        entry.name,
        source_label(entry.source),
        entry.description
    );
    if let Some(path) = &entry.path {
        out.push_str(&format!("path: {path}\n"));
    }
    out.push_str(&format!(
        "mode: {}  radius: {}\n",
        serde_json::to_value(entry.mode)
            .unwrap_or_default()
            .as_str()
            .unwrap_or(""),
        serde_json::to_value(entry.radius)
            .unwrap_or_default()
            .as_str()
            .unwrap_or("")
    ));
    let explicit = theme.file.desktop.as_ref();
    for (key, _) in super::TOKENS {
        let value = entry.tokens.get(key).unwrap_or_default();
        let derived = explicit
            .and_then(|table| table.token(key))
            .and_then(super::ColorSpec::rgb)
            .is_none();
        out.push_str(&format!(
            "  {key:<14} {value}{}\n",
            if derived { "  (derived)" } else { "" }
        ));
    }
    Ok(out)
}

/// `theme set <name>`.
pub fn set(name: &str, workspace: Option<&Path>) -> Result<String, String> {
    let snapshot = super::set_active(name, workspace)?;
    Ok(format!("Active theme: {}\n", snapshot.active))
}

/// `theme new <name> [--from <theme>]`: prints the new file's path.
pub fn new(name: &str, from: Option<&str>, workspace: Option<&Path>) -> Result<String, String> {
    let path = super::create(name, from, workspace)?;
    Ok(format!("{}\n", path.display()))
}

/// `theme path [<name>]`: the user themes directory, or a theme's file.
pub fn path(name: Option<&str>, workspace: Option<&Path>) -> Result<String, String> {
    let Some(name) = name else {
        let dir = ensure_user_dir().ok_or("could not determine the config directory")?;
        return Ok(format!("{}\n", dir.display()));
    };
    let themes = load_themes(workspace);
    let theme = named(&themes, name)?;
    match &theme.path {
        Some(path) => Ok(format!("{}\n", path.display())),
        None => Err(format!(
            "'{}' is built in and has no file; copy it with `rustcode theme new <name> --from {}`",
            theme.name(),
            theme.name()
        )),
    }
}

/// What `theme validate` checks: `(label, text)` of each file.
fn targets(
    target: Option<&str>,
    workspace: Option<&Path>,
) -> Result<Vec<(String, String)>, String> {
    let read = |path: &Path| {
        std::fs::read_to_string(path)
            .map(|text| (path.display().to_string(), text))
            .map_err(|error| format!("failed to read {}: {error}", path.display()))
    };
    match target {
        Some(target) => {
            let as_path = PathBuf::from(target);
            if target.ends_with(".toml") || target.contains(std::path::MAIN_SEPARATOR) {
                return Ok(vec![read(&as_path)?]);
            }
            let themes = load_themes(workspace);
            let theme = named(&themes, target)?;
            match &theme.path {
                Some(path) => Ok(vec![read(path)?]),
                None => BUILTIN_THEMES
                    .iter()
                    .find(|(file, _)| file.trim_end_matches(".toml") == theme.name())
                    .map(|(file, text)| vec![(format!("<built-in>/{file}"), (*text).to_owned())])
                    .ok_or_else(|| format!("no file for '{target}'")),
            }
        }
        None => {
            let mut out: Vec<(String, String)> = BUILTIN_THEMES
                .iter()
                .map(|(file, text)| (format!("<built-in>/{file}"), (*text).to_owned()))
                .collect();
            let mut dirs: Vec<PathBuf> = user_themes_dir().into_iter().collect();
            if let Some(workspace) = workspace {
                dirs.extend(project_themes_dirs(workspace));
            }
            for dir in dirs {
                for file in theme_files(&dir) {
                    out.push(read(&file)?);
                }
            }
            Ok(out)
        }
    }
}

/// `theme validate [<name>|<path>]`: every problem, and whether all files
/// are free of errors (warnings pass).
pub fn validate_command(target: Option<&str>, workspace: Option<&Path>) -> (String, bool) {
    let targets = match targets(target, workspace) {
        Ok(targets) => targets,
        Err(error) => return (format!("error: {error}\n"), false),
    };
    let mut out = String::new();
    let mut ok = true;
    for (label, text) in targets {
        let (_, diagnostics) = validate(&text);
        if diagnostics.is_empty() {
            out.push_str(&format!("{label}: ok\n"));
            continue;
        }
        ok &= !has_errors(&diagnostics);
        out.push_str(&format!("{label}:\n"));
        for diagnostic in diagnostics {
            out.push_str(&format!("  {diagnostic}\n"));
        }
    }
    (out, ok)
}
