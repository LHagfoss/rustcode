//! Configuration logic for the fmr TUI client.

use crate::app::ChatMessage;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Apple's on-device foundation model context-window limit (tokens).
pub const MAX_CONTEXT_TOKENS: u32 = 2048;

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
                    url: "http://100.90.28.23:11434/v1/chat/completions".to_string(),
                    model: "hrbrmstr/ornith-35b-fixed:latest".to_string(),
                },
            ],
            latest_model: None,
            latest_url: None,
        }
    }
}

pub fn get_config_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config").join("fmr"))
}

pub fn load_config() -> (String, String, AppConfig) {
    let default_config = AppConfig::default();

    let Some(dir) = get_config_dir() else {
        let default_profile = default_config.models[0].clone();
        return (default_profile.url, default_profile.model, default_config);
    };

    let file_path = dir.join("config.toml");
    if !file_path.exists() {
        let _ = fs::create_dir_all(&dir);
        if let Ok(toml_str) = toml::to_string_pretty(&default_config) {
            let _ = fs::write(&file_path, toml_str);
        }
        let default_profile = default_config.models[0].clone();
        return (default_profile.url, default_profile.model, default_config);
    }

    let Ok(content) = fs::read_to_string(&file_path) else {
        let default_profile = default_config.models[0].clone();
        return (default_profile.url, default_profile.model, default_config);
    };

    let config = match toml::from_str::<AppConfig>(&content) {
        Ok(c) => c,
        Err(_) => default_config,
    };

    let (url, model) = if let (Some(l_url), Some(l_model)) = (&config.latest_url, &config.latest_model) {
        (l_url.clone(), l_model.clone())
    } else {
        let profile = config
            .models
            .iter()
            .find(|m| m.name == config.default)
            .cloned()
            .unwrap_or_else(|| {
                config
                    .models
                    .first()
                    .cloned()
                    .unwrap_or_else(|| ModelProfile {
                        name: "apple-fm".to_string(),
                        url: "http://127.0.0.1:1976/v1/chat/completions".to_string(),
                        model: "system".to_string(),
                    })
            });
        (profile.url, profile.model)
    };

    (url, model, config)
}

pub fn save_entire_config(config: &AppConfig) {
    let Some(dir) = get_config_dir() else {
        return;
    };
    let file_path = dir.join("config.toml");
    let _ = fs::create_dir_all(&dir);
    if let Ok(toml_str) = toml::to_string_pretty(config) {
        let _ = fs::write(&file_path, toml_str);
    }
}

pub fn load_history() -> Vec<ChatMessage> {
    let Some(dir) = get_config_dir() else {
        return Vec::new();
    };
    let file_path = dir.join("fmr_history.json");
    if !file_path.exists() {
        return Vec::new();
    }
    let Ok(content) = fs::read_to_string(&file_path) else {
        return Vec::new();
    };
    let Ok(mut history) = serde_json::from_str::<Vec<ChatMessage>>(&content) else {
        return Vec::new();
    };
    // Keep last 50 messages
    if history.len() > 50 {
        history = history.drain(history.len() - 50..).collect();
    }
    history
}

pub fn save_history(history: &[ChatMessage]) {
    let Some(dir) = get_config_dir() else {
        return;
    };
    let file_path = dir.join("fmr_history.json");
    let _ = fs::create_dir_all(&dir);
    // Keep last 50 messages
    let start_idx = history.len().saturating_sub(50);
    let slice = &history[start_idx..];
    if let Ok(json_str) = serde_json::to_string_pretty(slice) {
        let _ = fs::write(&file_path, json_str);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_save_load() {
        let mut config = AppConfig::default();
        config.default = "ollama".to_string();
        save_entire_config(&config);
        let (url, model, loaded_config) = load_config();
        assert_eq!(loaded_config.default, "ollama");
        assert_eq!(url, "http://100.90.28.23:11434/v1/chat/completions");
        assert_eq!(model, "hrbrmstr/ornith-35b-fixed:latest");
    }

    #[test]
    fn test_history_save_load() {
        let msgs = vec![
            ChatMessage::new("user", "Hello"),
            ChatMessage::new("assistant", "Hi there"),
        ];
        save_history(&msgs);
        let loaded = load_history();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].role, "user");
        assert_eq!(loaded[0].content, "Hello");
        assert_eq!(loaded[1].role, "assistant");
        assert_eq!(loaded[1].content, "Hi there");
    }
}
