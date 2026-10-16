//! The model profile and reasoning effort new sessions start with: the
//! config's default profile and that profile's effort. The TUI's `/model` and
//! `/effort` set them as they switch; `rustcode defaults` reads and sets them
//! for frontends such as the macOS app.

use crate::config::AppConfig;
use serde::Serialize;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ModelDefaults {
    /// Default model profile name.
    pub model: String,
    /// Its reasoning effort; "default" leaves it to the provider.
    pub effort: String,
    pub models: Vec<ModelChoice>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ModelChoice {
    pub name: String,
    pub model: String,
    /// Effort choices, "default" first. A single entry means no choice.
    pub efforts: Vec<String>,
}

pub fn report(config: &AppConfig) -> ModelDefaults {
    let model = config.default.big().to_owned();
    let effort = config
        .models
        .iter()
        .find(|profile| profile.name == model)
        .and_then(|profile| profile.reasoning_effort.clone())
        .unwrap_or_else(|| "default".to_owned());
    ModelDefaults {
        model,
        effort,
        models: config
            .models
            .iter()
            .map(|profile| ModelChoice {
                name: profile.name.clone(),
                model: profile.model.clone(),
                efforts: crate::remote::projection::reasoning_efforts(profile),
            })
            .collect(),
    }
}

/// Change the default profile and/or its effort in `config`. An effort is
/// checked against the (new) default profile's choices.
pub fn apply(
    config: &mut AppConfig,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<(), String> {
    if let Some(model) = model {
        if !config.models.iter().any(|profile| profile.name == model) {
            return Err(format!("unknown model profile: {model}"));
        }
        config.default.set_big(model.to_owned());
    }
    if let Some(effort) = effort {
        let name = config.default.big().to_owned();
        let profile = config
            .models
            .iter_mut()
            .find(|profile| profile.name == name)
            .ok_or_else(|| format!("the default model profile {name} is not configured"))?;
        if !crate::remote::projection::reasoning_efforts(profile)
            .iter()
            .any(|choice| choice == effort)
        {
            return Err(format!("{name} does not support reasoning effort {effort}"));
        }
        profile.reasoning_effort = (effort != "default").then(|| effort.to_owned());
    }
    Ok(())
}

/// `apply`, then write the config file.
pub fn save(
    config: &mut AppConfig,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<(), String> {
    apply(config, model, effort)?;
    if model.is_some() || effort.is_some() {
        crate::config::save_entire_config_result(config)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ModelProfile;

    fn config() -> AppConfig {
        let mut config = AppConfig::default();
        config.models = vec![
            ModelProfile {
                name: "fast".into(),
                model: "fast-1".into(),
                ..Default::default()
            },
            ModelProfile {
                name: "deep".into(),
                model: "deep-1".into(),
                supports_reasoning_effort: Some(true),
                reasoning_efforts: Some(vec!["low".into(), "high".into()]),
                ..Default::default()
            },
        ];
        config.default.set_big("fast".into());
        config
    }

    #[test]
    fn reports_and_sets_the_default_model_and_effort() {
        let mut config = config();
        assert_eq!(report(&config).model, "fast");
        assert_eq!(report(&config).effort, "default");
        apply(&mut config, Some("deep"), Some("high")).unwrap();
        let defaults = report(&config);
        assert_eq!(
            (defaults.model.as_str(), defaults.effort.as_str()),
            ("deep", "high")
        );
        assert_eq!(defaults.models[1].efforts, ["default", "low", "high"]);
        apply(&mut config, None, Some("default")).unwrap();
        assert_eq!(report(&config).effort, "default");
    }

    #[test]
    fn rejects_unknown_profiles_and_unsupported_efforts() {
        let mut config = config();
        assert!(apply(&mut config, Some("missing"), None).is_err());
        assert!(apply(&mut config, None, Some("high")).is_err());
        assert!(apply(&mut config, Some("deep"), Some("xhigh")).is_err());
        assert_eq!(config.default.big(), "deep");
    }
}
