//! `[harness_defaults]` and `[[harnesses]]` in the user config
//! (`docs/harnesses.md`). Both are optional: a config without them keeps
//! working and starts every thread on the built-in harness.

use super::HarnessKind;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub onboarded: bool,
}

/// One `[[harnesses]]` entry. A known harness needs only `id` and
/// `enabled`; a custom one also names its `command`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<HarnessKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// An entry that leaves it out is on: listing a harness means wanting it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

impl HarnessEntry {
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
}

/// What `_rustcode/harnesses/set` asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessChange {
    pub id: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<HarnessKind>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Apply `change` to `harnesses`.
pub(crate) fn apply_change(
    harnesses: &mut Vec<HarnessEntry>,
    change: HarnessChange,
) -> Result<(), String> {
    let id = change.id.trim().to_owned();
    if !valid_id(&id) {
        return Err(format!(
            "invalid harness id '{id}': use letters, digits, '-', '_' or '.'"
        ));
    }
    if let Some(command) = &change.command
        && command
            .first()
            .is_none_or(|program| program.trim().is_empty())
    {
        return Err("command must name a program".to_owned());
    }
    let known = super::known(&id);
    if id == super::BUILTIN_ID {
        if change.enabled == Some(false) {
            return Err("the built-in RustCode harness cannot be turned off".to_owned());
        }
        if change.command.is_some() || change.kind.is_some() {
            return Err("the built-in RustCode harness has no command".to_owned());
        }
        return Ok(());
    }
    if known.is_some()
        && change
            .kind
            .is_some_and(|kind| Some(kind) != known.map(|k| k.kind))
    {
        return Err(format!("the kind of '{id}' is fixed"));
    }
    if let Some(kind) = change.kind
        && kind == HarnessKind::Builtin
    {
        return Err("only RustCode is a built-in harness".to_owned());
    }
    match harnesses.iter_mut().find(|entry| entry.id == id) {
        Some(entry) => {
            if let Some(enabled) = change.enabled {
                entry.enabled = Some(enabled);
            }
            if let Some(command) = change.command {
                entry.command = Some(command);
            }
            if let Some(name) = change.name.filter(|name| !name.trim().is_empty()) {
                entry.name = Some(name);
            }
            if known.is_none()
                && let Some(kind) = change.kind
            {
                entry.kind = Some(kind);
            }
        }
        None => {
            if known.is_none() && change.command.is_none() {
                return Err(format!("a custom harness '{id}' needs a command"));
            }
            harnesses.push(HarnessEntry {
                id: id.clone(),
                name: change.name.filter(|name| !name.trim().is_empty()),
                kind: if known.is_some() {
                    None
                } else {
                    Some(change.kind.unwrap_or(HarnessKind::Acp))
                },
                command: change.command,
                enabled: Some(change.enabled.unwrap_or(true)),
            });
        }
    }
    Ok(())
}

pub(crate) fn remove(harnesses: &mut Vec<HarnessEntry>, id: &str) -> Result<(), String> {
    if id == super::BUILTIN_ID || super::known(id).is_some() {
        return Err(format!(
            "'{id}' is a built-in entry; turn it off instead of removing it"
        ));
    }
    let before = harnesses.len();
    harnesses.retain(|entry| entry.id != id);
    if harnesses.len() == before {
        return Err(format!("unknown harness '{id}'"));
    }
    Ok(())
}

/// The user config directory, or an error a client can show.
pub(crate) fn config_dir() -> Result<std::path::PathBuf, String> {
    crate::config::get_config_dir().ok_or_else(|| "no config directory".to_owned())
}

pub(crate) fn load_in(dir: &Path) -> (HarnessDefaults, Vec<HarnessEntry>) {
    let (_, _, config) = crate::config::load_config_from(dir);
    (config.harness_defaults, config.harnesses)
}

pub(crate) fn edit_in<T>(
    dir: &Path,
    edit: impl FnOnce(&mut HarnessDefaults, &mut Vec<HarnessEntry>) -> Result<T, String>,
) -> Result<T, String> {
    crate::config::edit_user_config_in(dir, |config| {
        edit(&mut config.harness_defaults, &mut config.harnesses)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_and_old_configs_still_load() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "theme = \"dark\"\n").unwrap();
        let (defaults, harnesses) = load_in(dir.path());
        assert_eq!(defaults, HarnessDefaults::default());
        assert!(harnesses.is_empty());

        edit_in(dir.path(), |defaults, harnesses| {
            defaults.harness = Some("codex".into());
            defaults.model = Some("gpt-6-luna".into());
            defaults.effort = Some("low".into());
            defaults.onboarded = true;
            apply_change(
                harnesses,
                HarnessChange {
                    id: "codex".into(),
                    enabled: Some(true),
                    ..Default::default()
                },
            )?;
            apply_change(
                harnesses,
                HarnessChange {
                    id: "my-agent".into(),
                    name: Some("My agent".into()),
                    command: Some(vec!["my-agent".into(), "--acp".into()]),
                    ..Default::default()
                },
            )
        })
        .unwrap();

        let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(text.contains("[harness_defaults]"), "{text}");
        assert!(text.contains("[[harnesses]]"), "{text}");
        assert!(text.contains("theme = \"dark\""), "{text}");

        let (defaults, harnesses) = load_in(dir.path());
        assert_eq!(defaults.harness.as_deref(), Some("codex"));
        assert_eq!(defaults.effort.as_deref(), Some("low"));
        assert!(defaults.onboarded);
        assert_eq!(
            harnesses,
            vec![
                HarnessEntry {
                    id: "codex".into(),
                    enabled: Some(true),
                    ..Default::default()
                },
                HarnessEntry {
                    id: "my-agent".into(),
                    name: Some("My agent".into()),
                    kind: Some(HarnessKind::Acp),
                    command: Some(vec!["my-agent".into(), "--acp".into()]),
                    enabled: Some(true),
                },
            ]
        );

        edit_in(dir.path(), |_, harnesses| remove(harnesses, "my-agent")).unwrap();
        assert_eq!(load_in(dir.path()).1.len(), 1);
    }

    #[test]
    fn documented_toml_parses() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[harness_defaults]
harness = "codex"
model = "gpt-6-luna"
effort = "low"
onboarded = true

[[harnesses]]
id = "codex"
enabled = true

[[harnesses]]
id = "my-agent"
name = "My agent"
kind = "acp"
command = ["my-agent", "--acp"]
enabled = true
"#,
        )
        .unwrap();
        let (defaults, harnesses) = load_in(dir.path());
        assert!(defaults.onboarded);
        assert_eq!(harnesses.len(), 2);
        assert_eq!(harnesses[1].kind, Some(HarnessKind::Acp));
    }

    #[test]
    fn changes_are_validated() {
        let mut harnesses = Vec::new();
        assert!(
            apply_change(
                &mut harnesses,
                HarnessChange {
                    id: "rustcode".into(),
                    enabled: Some(false),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert!(
            apply_change(
                &mut harnesses,
                HarnessChange {
                    id: "custom".into(),
                    ..Default::default()
                }
            )
            .is_err(),
            "a custom harness needs a command"
        );
        assert!(
            apply_change(
                &mut harnesses,
                HarnessChange {
                    id: "bad id".into(),
                    command: Some(vec!["x".into()]),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert!(remove(&mut harnesses, "codex").is_err());
        assert!(remove(&mut harnesses, "nope").is_err());
    }

    #[test]
    fn project_config_cannot_add_harnesses() {
        let workspace = tempfile::tempdir().unwrap();
        let project = workspace.path().join(".rustcode");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("config.toml"),
            "[[harnesses]]\nid = \"evil\"\ncommand = [\"rm\", \"-rf\", \"/\"]\n",
        )
        .unwrap();
        let (_, _, config) = crate::config::load_config_for_workspace(workspace.path());
        assert!(config.harnesses.iter().all(|entry| entry.id != "evil"));
    }
}
