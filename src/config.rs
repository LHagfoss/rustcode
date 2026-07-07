use crate::app::ChatMessage;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_CONTEXT_TOKENS: u32 = 2048;

const CONFIG_FILE: &str = "config.toml";
const HISTORY_FILE: &str = "history.json";
const HISTORY_LIMIT: usize = 50;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ModelProfile {
    pub name: String,
    pub url: String,
    pub model: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub default: String,
    pub models: Vec<ModelProfile>,
    #[serde(default)]
    pub latest_model: Option<String>,
    #[serde(default)]
    pub latest_url: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            default: "apple-fm".to_string(),
            models: vec![
                ModelProfile {
                    name: "apple-fm".to_string(),
                    url: "http://127.0.0.1:1976/v1/chat/completions".to_string(),
                    model: "system".to_string(),
                },
                ModelProfile {
                    name: "ollama".to_string(),
                    url: "http://127.0.0.1:11434/v1/chat/completions".to_string(),
                    model: "llama3.2:latest".to_string(),
                },
            ],
            latest_model: None,
            latest_url: None,
        }
    }
}

pub fn get_config_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let config_root = PathBuf::from(home).join(".config");
    let dir = config_root.join("rustcode");

    if !dir.exists() {
        let legacy = config_root.join("fmr");
        if legacy.exists() && fs::rename(&legacy, &dir).is_ok() {
            let old_history = dir.join("fmr_history.json");
            if old_history.exists() {
                let _ = fs::rename(&old_history, dir.join(HISTORY_FILE));
            }
        }
    }

    Some(dir)
}

fn default_endpoint(config: &AppConfig) -> (String, String) {
    let profile = config.models[0].clone();
    (profile.url, profile.model)
}

pub fn load_config() -> (String, String, AppConfig) {
    match get_config_dir() {
        Some(dir) => load_config_from(&dir),
        None => {
            let config = AppConfig::default();
            let (url, model) = default_endpoint(&config);
            (url, model, config)
        }
    }
}

fn load_config_from(dir: &Path) -> (String, String, AppConfig) {
    let default_config = AppConfig::default();

    let file_path = dir.join(CONFIG_FILE);
    if !file_path.exists() {
        save_config_to(dir, &default_config);
        let (url, model) = default_endpoint(&default_config);
        return (url, model, default_config);
    }

    let Ok(content) = fs::read_to_string(&file_path) else {
        let (url, model) = default_endpoint(&default_config);
        return (url, model, default_config);
    };

    let config = match toml::from_str::<AppConfig>(&content) {
        Ok(c) => c,
        Err(_) => default_config,
    };

    let (url, model) =
        if let (Some(l_url), Some(l_model)) = (&config.latest_url, &config.latest_model) {
            (l_url.clone(), l_model.clone())
        } else {
            config
                .models
                .iter()
                .find(|m| m.name == config.default)
                .or_else(|| config.models.first())
                .map(|p| (p.url.clone(), p.model.clone()))
                .unwrap_or_else(|| default_endpoint(&AppConfig::default()))
        };

    (url, model, config)
}

pub fn save_entire_config(config: &AppConfig) {
    if let Some(dir) = get_config_dir() {
        save_config_to(&dir, config);
    }
}

fn save_config_to(dir: &Path, config: &AppConfig) {
    let _ = fs::create_dir_all(dir);
    if let Ok(toml_str) = toml::to_string_pretty(config) {
        let _ = fs::write(dir.join(CONFIG_FILE), toml_str);
    }
}

pub fn load_history() -> Vec<ChatMessage> {
    match get_config_dir() {
        Some(dir) => load_history_from(&dir),
        None => Vec::new(),
    }
}

fn load_history_from(dir: &Path) -> Vec<ChatMessage> {
    let file_path = dir.join(HISTORY_FILE);
    let Ok(content) = fs::read_to_string(&file_path) else {
        return Vec::new();
    };
    let Ok(mut history) = serde_json::from_str::<Vec<ChatMessage>>(&content) else {
        return Vec::new();
    };
    if history.len() > HISTORY_LIMIT {
        history.drain(..history.len() - HISTORY_LIMIT);
    }
    history
}

pub fn save_history(history: &[ChatMessage]) {
    if let Some(dir) = get_config_dir() {
        save_history_to(&dir, history);
    }
}

fn save_history_to(dir: &Path, history: &[ChatMessage]) {
    let _ = fs::create_dir_all(dir);
    let start_idx = history.len().saturating_sub(HISTORY_LIMIT);
    if let Ok(json_str) = serde_json::to_string_pretty(&history[start_idx..]) {
        let _ = fs::write(dir.join(HISTORY_FILE), json_str);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("rustcode-tests").join(format!(
            "{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_config_save_load() {
        let dir = temp_dir("config");
        let mut config = AppConfig::default();
        config.default = "ollama".to_string();
        save_config_to(&dir, &config);

        let (url, model, loaded) = load_config_from(&dir);
        assert_eq!(loaded.default, "ollama");
        let expected = &loaded.models.iter().find(|m| m.name == "ollama").unwrap();
        assert_eq!(url, expected.url);
        assert_eq!(model, expected.model);
    }

    #[test]
    fn test_latest_model_overrides_default() {
        let dir = temp_dir("latest");
        let mut config = AppConfig::default();
        config.latest_url = Some("http://example.com/v1/chat/completions".to_string());
        config.latest_model = Some("custom:latest".to_string());
        save_config_to(&dir, &config);

        let (url, model, _) = load_config_from(&dir);
        assert_eq!(url, "http://example.com/v1/chat/completions");
        assert_eq!(model, "custom:latest");
    }

    #[test]
    fn test_history_save_load() {
        let dir = temp_dir("history");
        let msgs = vec![
            ChatMessage::new("user", "Hello"),
            ChatMessage::new("assistant", "Hi there"),
        ];
        save_history_to(&dir, &msgs);
        let loaded = load_history_from(&dir);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].role, "user");
        assert_eq!(loaded[0].content, "Hello");
        assert_eq!(loaded[1].role, "assistant");
        assert_eq!(loaded[1].content, "Hi there");
    }

    #[test]
    fn test_history_capped_at_limit() {
        let dir = temp_dir("history-cap");
        let msgs: Vec<ChatMessage> = (0..80)
            .map(|i| ChatMessage::new("user", format!("msg {}", i)))
            .collect();
        save_history_to(&dir, &msgs);
        let loaded = load_history_from(&dir);
        assert_eq!(loaded.len(), HISTORY_LIMIT);
        assert_eq!(loaded[0].content, "msg 30");
    }
}
