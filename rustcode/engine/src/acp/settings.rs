//! `_rustcode/settings/get|set`: the server settings a client edits on one
//! machine or on all of them (docs/desktop.md › Settings on one machine or
//! all). One registry names every key, its JSON type and the value a client
//! reads when the machine has never set it; a key outside it is refused, so a
//! machine cannot be turned into a general-purpose store over ACP.
//!
//! The values are the client-hosted settings of a machine: the app applies
//! them when it starts or when it reads them. They live in
//! `<config-dir>/settings.json`, apart from `config.toml` because the engine
//! does not act on them yet and they must survive a client that never edits
//! `config.toml`. The harness default and the skill/MCP toggles already have
//! methods of their own (`_rustcode/harnesses/default`, `_rustcode/capabilities*`),
//! so they are not duplicated here.
//!
//! Offline machines keep their file; a client writes to a machine only while
//! its connection is open. `_rustcode/settings/changed` is not sent: a client
//! is the only writer of a value while it is connected, and the next read is
//! cheap.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::OnceLock;

/// The file these values live in, beside `config.toml`.
pub(crate) const SETTINGS_FILE: &str = "settings.json";

/// The type a key accepts. A client may send any of the three, but only the
/// one the registry declares is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingType {
    Boolean,
    Number,
    Text,
    /// A JSON array; its entries are read by the code that applies the key.
    List,
}

/// Every server setting, once: `(key, type, default)`. The order is the
/// order a `get` with no keys answers in; it is also the list a client may
/// use to check which keys a machine supports.
fn registry() -> &'static [(&'static str, SettingType, Value)] {
    static REGISTRY: OnceLock<Vec<(&'static str, SettingType, Value)>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        vec![
            ("default-permission-mode", SettingType::Text, json!("ask")),
            ("resume-after-restart", SettingType::Boolean, json!(true)),
            ("continue-after-limit", SettingType::Boolean, json!(true)),
            ("auto-compact-mode", SettingType::Text, json!("size")),
            (
                "auto-compact-threshold",
                SettingType::Number,
                json!(150_000),
            ),
            ("orchestration", SettingType::Boolean, json!(true)),
            ("delegates-total", SettingType::Number, json!(8)),
            ("delegates-per-thread", SettingType::Number, json!(4)),
            ("read-across-projects", SettingType::Boolean, json!(true)),
            ("delegate-presets", SettingType::List, json!([])),
            ("limit-fallback", SettingType::Boolean, json!(false)),
            ("limit-fallback-map", SettingType::List, json!([])),
            ("wake-batch-seconds", SettingType::Number, json!(60)),
        ]
    })
}

fn registry_entry(key: &str) -> Option<(&'static str, SettingType, &'static Value)> {
    registry()
        .iter()
        .find(|(known, ..)| *known == key)
        .map(|(known, kind, default)| (*known, *kind, default))
}

/// `_rustcode/settings/get`: every server setting, or only `keys`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/settings/get", response = serde_json::Value)]
pub(crate) struct GetSettingsRequest {
    /// The keys to read; absent reads all of them.
    #[serde(default)]
    pub keys: Option<Vec<String>>,
}

/// `_rustcode/settings/set`: merge these values into the machine's settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/settings/set", response = serde_json::Value)]
pub(crate) struct SetSettingsRequest {
    #[serde(default)]
    pub values: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SettingsFile {
    #[serde(default = "file_version")]
    version: u32,
    #[serde(default)]
    values: Map<String, Value>,
}

fn file_version() -> u32 {
    1
}

fn settings_path() -> Result<PathBuf, String> {
    crate::config::get_config_dir()
        .map(|dir| dir.join(SETTINGS_FILE))
        .ok_or_else(|| "could not determine the config directory".to_owned())
}

/// The stored values, with unknown or wrong-typed entries dropped: a file
/// edited by hand must not make a read fail.
fn read_values(path: &PathBuf) -> Map<String, Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Map::new(),
        Err(_) => return Map::new(),
    };
    let file: SettingsFile = serde_json::from_str(&text).unwrap_or_default();
    let mut values = Map::new();
    for (key, value) in file.values {
        if let Some((known, kind, _)) = registry_entry(&key)
            && value_matches(kind, &value)
        {
            values.insert(known.to_owned(), value);
        }
    }
    values
}

fn value_matches(kind: SettingType, value: &Value) -> bool {
    match kind {
        SettingType::Boolean => value.is_boolean(),
        SettingType::Number => value.as_f64().is_some_and(|n| n.is_finite()),
        SettingType::Text => value.is_string(),
        SettingType::List => value.is_array(),
    }
}

fn write_values(path: &PathBuf, values: &Map<String, Value>) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
    }
    let file = SettingsFile {
        version: 1,
        values: values.clone(),
    };
    let text = serde_json::to_string_pretty(&file)
        .map_err(|error| format!("could not serialize settings: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text.as_bytes())
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    crate::atomic_file::replace_file(&temporary, path)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

/// Answer `_rustcode/settings/get`: the requested keys, with each registry
/// default where the machine has no value of its own, and `stored`, the keys
/// this file actually holds. A client seeds a missing key only, so a value
/// another machine wrote before this one first started is not overwritten.
pub(crate) fn read(request: GetSettingsRequest) -> Result<Value, agent_client_protocol::Error> {
    let stored = read_values(&settings_path().unwrap_or_default());
    let mut values = Map::new();
    let keys: Vec<&str> = match &request.keys {
        Some(keys) => {
            for key in keys {
                if registry_entry(key).is_none() {
                    return Err(agent_client_protocol::Error::invalid_params()
                        .data(format!("unknown setting '{key}'")));
                }
            }
            keys.iter().map(String::as_str).collect()
        }
        None => registry().iter().map(|(key, ..)| *key).collect(),
    };
    for key in keys {
        let (key, _, default) = registry_entry(key).expect("checked above");
        values.insert(
            key.to_owned(),
            stored.get(key).cloned().unwrap_or_else(|| default.clone()),
        );
    }
    let stored_keys: Vec<String> = stored.keys().cloned().collect();
    Ok(json!({ "values": values, "stored": stored_keys }))
}

/// One setting's current value, for engine code that has to apply a
/// client-hosted setting itself (#3422). The registry default applies where
/// the machine has not set it.
pub(crate) fn value(key: &str) -> Option<Value> {
    let answer = read(GetSettingsRequest {
        keys: Some(vec![key.to_owned()]),
    })
    .ok()?;
    answer.get("values")?.get(key).cloned()
}

/// Answer `_rustcode/settings/set`: merge the values, then read them back.
pub(crate) fn write(request: SetSettingsRequest) -> Result<Value, agent_client_protocol::Error> {
    let path = settings_path()
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error))?;
    let mut values = read_values(&path);
    for (key, value) in &request.values {
        let Some((known, kind, _)) = registry_entry(key) else {
            return Err(agent_client_protocol::Error::invalid_params()
                .data(format!("unknown setting '{key}'")));
        };
        if !value_matches(kind, value) {
            return Err(agent_client_protocol::Error::invalid_params()
                .data(format!("setting '{known}' has the wrong type")));
        }
        values.insert(known.to_owned(), value.clone());
    }
    write_values(&path, &values)
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error))?;
    read(GetSettingsRequest::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_answers_every_registry_default_before_anything_is_set() {
        let answer = read(GetSettingsRequest::default()).unwrap();
        let values = answer["values"].as_object().unwrap();
        assert_eq!(values.len(), registry().len());
        assert_eq!(answer["stored"], json!([]));
        assert_eq!(values["default-permission-mode"], json!("ask"));
        assert_eq!(values["resume-after-restart"], json!(true));
        assert_eq!(values["auto-compact-threshold"], json!(150_000));
    }

    #[test]
    fn set_merges_reads_back_and_keeps_the_rest_of_the_file() {
        let answer = write(SetSettingsRequest {
            values: Map::from_iter([
                ("orchestration".to_owned(), json!(false)),
                ("delegates-total".to_owned(), json!(12)),
            ]),
        })
        .unwrap();
        assert_eq!(answer["values"]["orchestration"], json!(false));
        assert_eq!(answer["values"]["delegates-total"], json!(12));
        assert_eq!(answer["values"]["default-permission-mode"], json!("ask"));
        let mut stored: Vec<&str> = answer["stored"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key.as_str().unwrap())
            .collect();
        stored.sort();
        assert_eq!(stored, ["delegates-total", "orchestration"]);

        let reread = read(GetSettingsRequest {
            keys: Some(vec!["orchestration".to_owned()]),
        })
        .unwrap();
        assert_eq!(
            reread["values"].as_object().unwrap().len(),
            1,
            "only the asked keys are answered"
        );
        assert_eq!(reread["values"]["orchestration"], json!(false));
        let mut reread_stored: Vec<&str> = reread["stored"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key.as_str().unwrap())
            .collect();
        reread_stored.sort();
        assert_eq!(
            reread_stored,
            ["delegates-total", "orchestration"],
            "stored names every key the file holds, whatever was asked"
        );
    }

    #[test]
    fn unknown_keys_and_wrong_types_are_refused_without_a_write() {
        let unknown = write(SetSettingsRequest {
            values: Map::from_iter([("theme".to_owned(), json!("dark"))]),
        });
        assert!(unknown.is_err());
        let wrong = write(SetSettingsRequest {
            values: Map::from_iter([("resume-after-restart".to_owned(), json!("yes"))]),
        });
        assert!(wrong.is_err());
        assert_eq!(
            read(GetSettingsRequest::default()).unwrap()["values"]["resume-after-restart"],
            json!(true)
        );
    }

    #[test]
    fn a_hand_edited_file_with_unknown_keys_still_reads() {
        let path = settings_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"version":1,"values":{"orchestration":false,"theme":"dark","delegates-total":"many"}}"#,
        )
        .unwrap();
        let values = read(GetSettingsRequest::default()).unwrap()["values"]
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(values["orchestration"], json!(false));
        assert_eq!(values["delegates-total"], json!(8));
        assert!(!values.contains_key("theme"));
        assert_eq!(
            read(GetSettingsRequest::default()).unwrap()["stored"],
            json!(["orchestration"])
        );
        let _ = std::fs::remove_file(path);
    }
}
