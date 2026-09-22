use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 150;
const DEFAULT_MIN_CONFIDENCE: f32 = 0.98;
const DEFAULT_MAX_EXTRA_READ_ONLY_RECOVERIES: usize = 1;
const DEFAULT_PYTHON: &str = "python3";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LayaMode {
    Off,
    Shadow,
    Relaxed,
}

impl Default for LayaMode {
    fn default() -> Self {
        Self::Off
    }
}

impl fmt::Display for LayaMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Off => "off",
            Self::Shadow => "shadow",
            Self::Relaxed => "relaxed",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayaConfig {
    #[serde(default)]
    pub mode: LayaMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f32,
    #[serde(default = "default_max_extra_read_only_recoveries")]
    pub max_extra_read_only_recoveries: usize,
}

impl Default for LayaConfig {
    fn default() -> Self {
        Self {
            mode: LayaMode::Off,
            python: None,
            adapter: None,
            model: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            min_confidence: DEFAULT_MIN_CONFIDENCE,
            max_extra_read_only_recoveries: DEFAULT_MAX_EXTRA_READ_ONLY_RECOVERIES,
        }
    }
}

impl LayaConfig {
    /// Disable advisory evaluation when a persisted value cannot be used
    /// safely. The remaining values are retained for status diagnostics.
    pub fn fail_closed(mut self) -> Self {
        if self.timeout_ms == 0
            || !self.min_confidence.is_finite()
            || !(0.0..=1.0).contains(&self.min_confidence)
        {
            self.mode = LayaMode::Off;
        }
        self
    }

    pub fn python_executable(&self) -> &str {
        self.python.as_deref().unwrap_or(DEFAULT_PYTHON)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdvisoryKind {
    ShellPolicy,
    Repetition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdvisoryRequest {
    pub id: String,
    pub kind: AdvisoryKind,
    pub input: serde_json::Value,
    #[serde(with = "serde_millis")]
    pub deadline: Duration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdvisoryDecision {
    pub label: String,
    pub confidence: f32,
    pub effects: Vec<String>,
    pub rationale_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdvisoryError {
    Disabled,
    Unavailable,
    InvalidRequest,
    Timeout,
    MalformedResponse,
    ModelError,
    ProcessExit,
}

impl fmt::Display for AdvisoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Disabled => "Laya advisory evaluation is disabled",
            Self::Unavailable => "Laya advisory service is unavailable",
            Self::InvalidRequest => "Laya advisory request is invalid",
            Self::Timeout => "Laya advisory evaluation timed out",
            Self::MalformedResponse => "Laya advisory response was malformed",
            Self::ModelError => "Laya advisory model failed",
            Self::ProcessExit => "Laya advisory process exited",
        })
    }
}

impl std::error::Error for AdvisoryError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Disabled,
    Ready,
    UnsupportedArchitecture,
    MissingPython,
    MissingAdapter,
    MissingModel,
}

impl fmt::Display for Availability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Disabled => "disabled",
            Self::Ready => "ready",
            Self::UnsupportedArchitecture => "unsupported_architecture",
            Self::MissingPython => "missing_python",
            Self::MissingAdapter => "missing_adapter",
            Self::MissingModel => "missing_model",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayaStatus {
    pub mode: LayaMode,
    pub platform: String,
    pub python: String,
    pub python_available: bool,
    pub adapter_configured: bool,
    pub adapter_available: bool,
    pub model_configured: bool,
    pub model_available: bool,
    pub availability: Availability,
}

/// Pure, non-inference diagnostics for the local Laya prerequisites.
pub fn diagnose(config: &LayaConfig) -> LayaStatus {
    let python = config.python_executable().to_owned();
    let python_available = executable_available(&python);
    let adapter_configured = configured(config.adapter.as_deref());
    let adapter_available = readable_file(config.adapter.as_deref());
    let model_configured = configured(config.model.as_deref());
    let model_available = readable_file(config.model.as_deref());
    let availability = if config.mode == LayaMode::Off {
        Availability::Disabled
    } else if !supported_platform() {
        Availability::UnsupportedArchitecture
    } else if !python_available {
        Availability::MissingPython
    } else if !adapter_available {
        Availability::MissingAdapter
    } else if !model_available {
        Availability::MissingModel
    } else {
        Availability::Ready
    };

    LayaStatus {
        mode: config.mode,
        platform: platform_name().to_owned(),
        python,
        python_available,
        adapter_configured,
        adapter_available,
        model_configured,
        model_available,
        availability,
    }
}

pub fn format_status(config: &LayaConfig) -> String {
    let status = diagnose(config);
    format!(
        "Laya mode: {}\nPlatform: {}\nPython executable: {} ({})\nAdapter: {}\nModel: {}\nAvailability: {}",
        status.mode,
        status.platform,
        status.python,
        if status.python_available {
            "available"
        } else {
            "missing"
        },
        if status.adapter_available {
            "available"
        } else if status.adapter_configured {
            "configured, missing"
        } else {
            "not configured"
        },
        if status.model_available {
            "available"
        } else if status.model_configured {
            "configured, missing"
        } else {
            "not configured"
        },
        status.availability,
    )
}

fn configured(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}

fn readable_file(value: Option<&str>) -> bool {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return false;
    };
    let path = Path::new(value);
    path.is_file() && std::fs::File::open(path).is_ok()
}

fn executable_available(executable: &str) -> bool {
    let path = Path::new(executable);
    if path.components().count() > 1 {
        return path.is_file();
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(executable))
        .any(|candidate| candidate.is_file())
}

fn supported_platform() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

fn platform_name() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "macos-aarch64"
    } else {
        "unsupported"
    }
}

#[derive(Debug)]
pub struct LayaRuntime {
    config: LayaConfig,
    state: Arc<tokio::sync::Mutex<RuntimeState>>,
}

#[derive(Debug, Default)]
enum RuntimeState {
    #[default]
    NotStarted,
}

impl LayaRuntime {
    pub fn new(config: LayaConfig) -> Self {
        Self {
            config: config.fail_closed(),
            state: Arc::new(tokio::sync::Mutex::new(RuntimeState::NotStarted)),
        }
    }

    pub fn config(&self) -> &LayaConfig {
        &self.config
    }

    /// Evaluate through the process-lived advisory runtime. Off mode returns
    /// a disabled result without touching the lazy runtime state.
    pub async fn evaluate(
        &self,
        _request: AdvisoryRequest,
    ) -> Result<AdvisoryDecision, AdvisoryError> {
        if self.config.mode == LayaMode::Off {
            return Err(AdvisoryError::Disabled);
        }

        let _state = self.state.lock().await;
        Err(AdvisoryError::Unavailable)
    }
}

impl Clone for LayaRuntime {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            state: Arc::clone(&self.state),
        }
    }
}

fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

fn default_min_confidence() -> f32 {
    DEFAULT_MIN_CONFIDENCE
}

fn default_max_extra_read_only_recoveries() -> usize {
    DEFAULT_MAX_EXTRA_READ_ONLY_RECOVERIES
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn modes_serialize_as_lowercase_values() {
        assert_eq!(serde_json::to_string(&LayaMode::Off).unwrap(), "\"off\"");
        assert_eq!(
            serde_json::to_string(&LayaMode::Shadow).unwrap(),
            "\"shadow\""
        );
        assert_eq!(
            serde_json::to_string(&LayaMode::Relaxed).unwrap(),
            "\"relaxed\""
        );
    }

    #[tokio::test]
    async fn off_evaluation_is_a_disabled_no_advisory_result() {
        let runtime = LayaRuntime::new(LayaConfig::default());
        let result = runtime
            .evaluate(AdvisoryRequest {
                id: "off".to_owned(),
                kind: AdvisoryKind::ShellPolicy,
                input: serde_json::json!({}),
                deadline: Duration::from_millis(150),
            })
            .await;
        assert_eq!(result, Err(AdvisoryError::Disabled));
    }

    #[test]
    fn invalid_numeric_configuration_fails_closed() {
        let config = LayaConfig {
            mode: LayaMode::Relaxed,
            timeout_ms: 0,
            ..LayaConfig::default()
        };
        assert_eq!(config.fail_closed().mode, LayaMode::Off);
    }

    #[test]
    fn status_reports_a_missing_adapter_path_without_running_python() {
        let dir = TempDir::new().unwrap();
        let model = dir.path().join("model.gguf");
        std::fs::write(&model, b"local model").unwrap();
        let config = LayaConfig {
            mode: LayaMode::Shadow,
            python: Some(std::env::current_exe().unwrap().display().to_string()),
            adapter: Some(dir.path().join("missing-adapter.py").display().to_string()),
            model: Some(model.display().to_string()),
            ..LayaConfig::default()
        };

        let status = diagnose(&config);
        assert!(!status.adapter_available);
        assert!(status.model_available);
        if supported_platform() {
            assert_eq!(status.availability, Availability::MissingAdapter);
        }
    }

    #[test]
    fn status_reports_a_missing_model_path_without_running_python() {
        let dir = TempDir::new().unwrap();
        let adapter = dir.path().join("adapter.py");
        std::fs::write(&adapter, b"sidecar").unwrap();
        let config = LayaConfig {
            mode: LayaMode::Shadow,
            python: Some(std::env::current_exe().unwrap().display().to_string()),
            adapter: Some(adapter.display().to_string()),
            model: Some(dir.path().join("missing-model").display().to_string()),
            ..LayaConfig::default()
        };

        let status = diagnose(&config);
        assert!(status.adapter_available);
        assert!(!status.model_available);
        if supported_platform() {
            assert_eq!(status.availability, Availability::MissingModel);
        }
    }
}
