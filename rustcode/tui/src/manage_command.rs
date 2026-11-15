//! `rustcode settings …`: RustCode's server settings from a terminal (#3821).
//! The work is in `rustcode::manage`, which the `manage_settings` tool uses
//! too; this parses, prints and exits.

use crate::cli::SettingsCommands;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub async fn settings(command: &SettingsCommands) -> Result<(), String> {
    match command {
        SettingsCommands::List { project, json } => {
            let project = folder(project.as_deref())?;
            let listed = rustcode::manage::cli_list(project.as_deref()).await?;
            if *json {
                return print_json(&listed);
            }
            let rows: Vec<Vec<String>> = listed["settings"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|row| {
                    vec![
                        row["key"].as_str().unwrap_or_default().to_owned(),
                        shown(&row["value"]),
                        row["source"].as_str().unwrap_or_default().to_owned(),
                        row["label"].as_str().unwrap_or_default().to_owned(),
                    ]
                })
                .collect();
            crate::run::print_rows(&["KEY", "VALUE", "FROM", "SETTING"], &rows);
            Ok(())
        }
        SettingsCommands::Get { key, project, json } => {
            let project = folder(project.as_deref())?;
            let row = rustcode::manage::cli_get(key, project.as_deref()).await?;
            if *json {
                return print_json(&row);
            }
            println!(
                "{}: {}",
                row["key"].as_str().unwrap_or(key),
                shown(&row["value"])
            );
            println!("  {}", row["label"].as_str().unwrap_or_default());
            println!(
                "  from {}, default {}, {}",
                row["source"].as_str().unwrap_or_default(),
                shown(&row["default"]),
                row["type"].as_str().unwrap_or_default()
            );
            if let Some(choices) = row["choices"].as_array() {
                let choices: Vec<&str> = choices.iter().filter_map(Value::as_str).collect();
                println!("  one of {}", choices.join(", "));
            }
            Ok(())
        }
        SettingsCommands::Set {
            key,
            value,
            project,
            json,
        } => {
            let project = folder(project.as_deref())?;
            let value = rustcode::manage::parse_value(key, value)?;
            let row = rustcode::manage::cli_write(key, Some(value), project.as_deref()).await?;
            print_written(&row, *json)
        }
        SettingsCommands::Reset { key, project, json } => {
            let project = folder(project.as_deref())?;
            let row = rustcode::manage::cli_write(key, None, project.as_deref()).await?;
            print_written(&row, *json)
        }
    }
}

/// A project folder as the settings name it: absolute, from where the
/// command runs.
fn folder(path: Option<&Path>) -> Result<Option<String>, String> {
    let Some(path) = path else {
        return Ok(None);
    };
    let absolute: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("could not read the current folder: {error}"))?
            .join(path)
    };
    let resolved = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    Ok(Some(resolved.to_string_lossy().into_owned()))
}

fn print_written(row: &Value, json: bool) -> Result<(), String> {
    if json {
        return print_json(row);
    }
    let key = row["key"].as_str().unwrap_or_default();
    let before = shown(&row["before"]);
    let now = shown(&row["value"]);
    if before == now {
        println!("{key}: {now} (unchanged)");
    } else {
        println!("{key}: {before} → {now}");
    }
    if row["via"] == "file" {
        println!("No RustCode server is running, so this wrote settings.json directly.");
    }
    Ok(())
}

fn print_json(value: &Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn shown(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "-".to_owned(),
        other => other.to_string(),
    }
}
