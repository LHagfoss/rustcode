//! `rustcode settings|skill|harness …` and `rustcode mcp enable|disable|restart`:
//! managing RustCode from a terminal (#3821).
//! The work is in `rustcode::manage`, which the `manage_settings` tool uses
//! too; this parses, prints and exits.

use crate::cli::{
    Commands, HarnessCommands, McpCommands, SessionCommands, SettingsCommands, ToggleCommands,
    WorktreeCommands,
};
use rustcode::manage::capabilities::Op;
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

/// The capability change or list a command asks for, and whether it wants JSON.
pub fn capability_op(command: &Commands) -> Option<(Op, bool)> {
    match command {
        Commands::Skill { command } => Some(match command {
            ToggleCommands::List { json } => (Op::List("skill".into()), *json),
            ToggleCommands::Enable { name } => (toggle("skill", name, true), false),
            ToggleCommands::Disable { name } => (toggle("skill", name, false), false),
        }),
        Commands::Harness { command } => Some(match command {
            HarnessCommands::List { json } => (Op::List("harness".into()), *json),
            HarnessCommands::Default {
                harness,
                model,
                effort,
            } => (
                Op::Default {
                    harness: harness.clone(),
                    model: model.clone(),
                    effort: effort.clone(),
                },
                false,
            ),
        }),
        _ => None,
    }
}

/// `rustcode mcp enable|disable|restart`; the other `mcp` commands edit the
/// config themselves.
pub fn mcp_op(command: &McpCommands) -> Option<Op> {
    match command {
        McpCommands::Enable { name } => Some(toggle("mcp", name, true)),
        McpCommands::Disable { name } => Some(toggle("mcp", name, false)),
        McpCommands::Restart { name } => Some(Op::Restart(name.clone())),
        _ => None,
    }
}

fn toggle(kind: &str, name: &str, enabled: bool) -> Op {
    Op::Toggle {
        kind: kind.to_owned(),
        name: name.to_owned(),
        enabled,
    }
}

pub async fn capability(op: Op, json: bool) -> Result<(), String> {
    let (answer, via) = rustcode::manage::capabilities::cli(&op).await?;
    if json {
        return print_json(&answer);
    }
    match &op {
        Op::List(kind) => print_list(kind, &answer),
        Op::Toggle {
            kind,
            name,
            enabled,
        } => {
            let what = if kind == "mcp" { "MCP server" } else { "Skill" };
            println!("{what} {name} is {}.", if *enabled { "on" } else { "off" });
        }
        Op::Restart(name) => println!("Restarted MCP server {name}."),
        Op::Default { .. } => {
            let default = &answer["default"];
            println!(
                "New threads start on {}{}.",
                default["harness"].as_str().unwrap_or("?"),
                default["model"]
                    .as_str()
                    .map(|model| format!(" with {model}"))
                    .unwrap_or_default()
            );
        }
        Op::Theme(name) => println!("Theme is {name}."),
    }
    if via == rustcode::manage::Via::File && op.is_write() {
        println!("No RustCode server is running, so this changed the config file directly.");
    }
    Ok(())
}

fn print_list(kind: &str, answer: &Value) {
    let on = |value: &Value| {
        if value.as_bool() == Some(true) {
            "on"
        } else {
            "off"
        }
    };
    match kind {
        "skill" => {
            let rows: Vec<Vec<String>> = answer["skills"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|skill| {
                    vec![
                        shown(&skill["name"]),
                        on(&skill["enabled"]).to_owned(),
                        shown(&skill["source"]),
                        shown(&skill["description"]),
                    ]
                })
                .collect();
            crate::run::print_rows(&["SKILL", "STATE", "FROM", "DESCRIPTION"], &rows);
        }
        _ => {
            let default = answer["default"]["harness"].clone();
            let rows: Vec<Vec<String>> = answer["harnesses"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|row| {
                    let accounts: Vec<String> = row["accounts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(shown)
                        .collect();
                    vec![
                        format!(
                            "{}{}",
                            shown(&row["id"]),
                            if row["id"] == default { " *" } else { "" }
                        ),
                        shown(&row["name"]),
                        if row["installed"].as_bool() == Some(true) {
                            "installed".to_owned()
                        } else {
                            "missing".to_owned()
                        },
                        shown(&row["auth"]),
                        accounts.join(", "),
                    ]
                })
                .collect();
            crate::run::print_rows(&["HARNESS", "NAME", "INSTALLED", "AUTH", "ACCOUNTS"], &rows);
            println!("* new threads start here");
        }
    }
}

/// `rustcode threads move|rename|pin|unpin`.
pub async fn thread(command: &SessionCommands) -> Result<(), String> {
    let saved = rustcode::config::session_ids();
    let resolve = |raw: &str| crate::run::resolve_thread_id(raw, &saved);
    match command {
        SessionCommands::Move {
            id,
            harness,
            account,
            model,
        } => {
            let id = resolve(id)?;
            rustcode::manage::threads::move_thread(
                &id,
                harness,
                account.as_deref(),
                model.as_deref(),
            )
            .await?;
            println!(
                "{id} continues on {harness}{}{}.",
                account
                    .as_deref()
                    .map(|account| format!(" ({account})"))
                    .unwrap_or_default(),
                model
                    .as_deref()
                    .map(|model| format!(" with {model}"))
                    .unwrap_or_default()
            );
        }
        SessionCommands::Rename { id, title } => {
            let id = resolve(id)?;
            let title = title.trim();
            if title.is_empty() {
                return Err("the title is empty".to_owned());
            }
            rustcode::config::save_session_title(&id, title);
            println!("renamed {id} {title}");
        }
        SessionCommands::Pin { ids } | SessionCommands::Unpin { ids } => {
            let pinned = matches!(command, SessionCommands::Pin { .. });
            for raw in ids {
                let id = resolve(raw)?;
                rustcode::manage::threads::set_pinned(&id, pinned)?;
                println!("{} {id}", if pinned { "pinned" } else { "unpinned" });
            }
        }
        _ => unreachable!("the other thread commands run in run.rs"),
    }
    Ok(())
}

/// `rustcode worktree …`.
pub fn worktree(command: &WorktreeCommands) -> Result<(), String> {
    let here = || std::env::current_dir().map_err(|error| error.to_string());
    match command {
        WorktreeCommands::List { repo, json } => {
            let repo = match repo {
                Some(repo) => repo.clone(),
                None => here()?.into(),
            };
            let listed = rustcode::manage::threads::worktrees(&repo)?;
            if *json {
                return print_json(&listed);
            }
            let rows: Vec<Vec<String>> = listed["worktrees"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|row| {
                    let threads: Vec<String> = row["session_ids"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(shown)
                        .collect();
                    let state = match (row["dirty"].as_bool(), row["ahead"].as_u64()) {
                        (Some(true), _) => "uncommitted".to_owned(),
                        (_, Some(ahead)) if ahead > 0 => format!("{ahead} ahead"),
                        (Some(false), _) => "clean".to_owned(),
                        _ => "unknown".to_owned(),
                    };
                    vec![
                        shown(&row["branch"]),
                        state,
                        shown(&row["path"]),
                        threads.join(", "),
                    ]
                })
                .collect();
            if rows.is_empty() {
                println!("No RustCode worktrees for {}.", repo.display());
            } else {
                crate::run::print_rows(&["BRANCH", "STATE", "PATH", "THREADS"], &rows);
            }
            Ok(())
        }
        WorktreeCommands::Status { path } => {
            print_json(&rustcode::manage::threads::worktree_state(path))
        }
        WorktreeCommands::Remove { path, force, yes } => {
            if *force
                && !*yes
                && !confirm(&format!(
                    "Remove {} with any uncommitted or unpushed work in it? [y/N] ",
                    path.display()
                ))
            {
                return Err("not removed; pass --yes to confirm --force".to_owned());
            }
            rustcode::manage::threads::remove_worktree(path, *force)?;
            println!("Removed {}.", path.display());
            Ok(())
        }
        WorktreeCommands::Prune { repo } => {
            let repo = match repo {
                Some(repo) => repo.clone(),
                None => here()?.into(),
            };
            let said = rustcode::manage::threads::prune_worktrees(&repo)?;
            println!(
                "{}",
                if said.is_empty() {
                    "Nothing to prune."
                } else {
                    &said
                }
            );
            Ok(())
        }
    }
}

/// Ask on a terminal; no terminal is a no.
fn confirm(question: &str) -> bool {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return false;
    }
    print!("{question}");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}
