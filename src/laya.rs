use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::process::Stdio;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const DEFAULT_TIMEOUT_MS: u64 = 150;
const DEFAULT_STARTUP_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_MIN_CONFIDENCE: f32 = 0.98;
const DEFAULT_MAX_EXTRA_READ_ONLY_RECOVERIES: usize = 1;
const DEFAULT_PYTHON: &str = "python3";
const PROTOCOL_VERSION: u32 = 1;
const MAX_LINE_BYTES: usize = 16 * 1024;
const MAX_REQUEST_ID_BYTES: usize = 256;
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(0);

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
    #[serde(default = "default_startup_timeout_ms")]
    pub startup_timeout_ms: u64,
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
            startup_timeout_ms: DEFAULT_STARTUP_TIMEOUT_MS,
            min_confidence: DEFAULT_MIN_CONFIDENCE,
            max_extra_read_only_recoveries: DEFAULT_MAX_EXTRA_READ_ONLY_RECOVERIES,
        }
    }
}

impl LayaConfig {
    pub fn validation_error(&self) -> Option<&'static str> {
        if self.timeout_ms == 0 {
            Some("timeout_ms must be greater than zero")
        } else if self.startup_timeout_ms == 0 {
            Some("startup_timeout_ms must be greater than zero")
        } else if !self.min_confidence.is_finite() {
            Some("min_confidence must be finite")
        } else if !(0.0..=1.0).contains(&self.min_confidence) {
            Some("min_confidence must be between 0 and 1")
        } else {
            None
        }
    }

    /// Disable advisory evaluation when a persisted value cannot be used
    /// safely. The remaining values are retained for status diagnostics.
    pub fn fail_closed(mut self) -> Self {
        if self.validation_error().is_some() {
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
    PathsAvailable,
    UnsupportedProtocol,
    UnsupportedArchitecture,
    MissingPython,
    MissingAdapter,
    MissingModel,
}

impl fmt::Display for Availability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Disabled => "disabled",
            Self::PathsAvailable => "paths_available",
            Self::UnsupportedProtocol => "unsupported_protocol",
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
    pub protocol_version: u32,
    pub protocol_supported: bool,
    pub python: String,
    pub python_available: bool,
    pub adapter_configured: bool,
    pub adapter_available: bool,
    pub model_configured: bool,
    pub model_available: bool,
    pub paths_available: bool,
    pub runtime_ready: bool,
    pub failure_category: Option<String>,
    pub availability: Availability,
}

/// Pure, non-inference diagnostics for the local Laya prerequisites.
pub fn diagnose(config: &LayaConfig) -> LayaStatus {
    let python = config.python_executable().to_owned();
    let python_available = executable_available(&python);
    let adapter_configured = configured(config.adapter.as_deref());
    let adapter_available = readable_file(config.adapter.as_deref());
    let model_configured = configured(config.model.as_deref());
    let model_available = readable_path(config.model.as_deref());
    let availability = if config.mode == LayaMode::Off {
        Availability::Disabled
    } else if PROTOCOL_VERSION != 1 {
        Availability::UnsupportedProtocol
    } else if !supported_platform() {
        Availability::UnsupportedArchitecture
    } else if !python_available {
        Availability::MissingPython
    } else if !adapter_available {
        Availability::MissingAdapter
    } else if !model_available {
        Availability::MissingModel
    } else {
        Availability::PathsAvailable
    };

    LayaStatus {
        mode: config.mode,
        platform: platform_name().to_owned(),
        protocol_version: PROTOCOL_VERSION,
        protocol_supported: PROTOCOL_VERSION == 1,
        python,
        python_available,
        adapter_configured,
        adapter_available,
        model_configured,
        model_available,
        paths_available: python_available && adapter_available && model_available,
        runtime_ready: false,
        failure_category: None,
        availability,
    }
}

pub fn format_status(config: &LayaConfig) -> String {
    let status = diagnose(config);
    format!(
        "Laya mode: {}\nPlatform: {}\nProtocol: {} ({})\nPython executable: {} ({})\nAdapter: {}\nModel: {}\nPaths available: {}\nRuntime ready: {}\nFailure category: {}\nAvailability: {}",
        status.mode,
        status.platform,
        status.protocol_version,
        if status.protocol_supported {
            "supported"
        } else {
            "unsupported"
        },
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
        if status.paths_available { "yes" } else { "no" },
        if status.runtime_ready { "yes" } else { "no" },
        status.failure_category.as_deref().unwrap_or("none"),
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

fn readable_path(value: Option<&str>) -> bool {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return false;
    };
    let path = Path::new(value);
    (path.is_file() && std::fs::File::open(path).is_ok())
        || (path.is_dir() && std::fs::read_dir(path).is_ok())
}

fn executable_available(executable: &str) -> bool {
    let path = Path::new(executable);
    if path.components().count() > 1 {
        return is_executable_file(path);
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(executable))
        .any(|candidate| is_executable_file(&candidate))
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        path.metadata()
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
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
struct RuntimeState {
    process: Option<SidecarProcess>,
    started: bool,
    restart_available: bool,
    restart_used: bool,
    seen_ids: HashSet<String>,
    last_failure: Option<AdvisoryError>,
}

#[derive(Debug)]
struct SidecarProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

#[derive(Debug, Deserialize)]
struct ReadyMessage {
    protocol: u32,
    #[allow(dead_code)]
    backend: Option<String>,
    #[allow(dead_code)]
    model: Option<String>,
    #[allow(dead_code)]
    kinds: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct WireRequest<'a> {
    protocol: u32,
    id: &'a str,
    kind: AdvisoryKind,
    input: &'a serde_json::Value,
    deadline_ms: u64,
}

#[derive(Debug, Deserialize)]
struct WireResponse {
    protocol: u32,
    id: String,
    ok: bool,
    decision: Option<WireDecision>,
    error: Option<WireError>,
}

#[derive(Debug, Deserialize)]
struct WireDecision {
    label: String,
    confidence: Option<f32>,
    effects: Option<Vec<String>>,
    rationale_code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireError {
    category: String,
}

impl SidecarProcess {
    async fn evaluate(
        &mut self,
        request_line: &[u8],
        request_id: &str,
        timeout: Duration,
        min_confidence: f32,
        kind: AdvisoryKind,
    ) -> Result<AdvisoryDecision, AdvisoryError> {
        self.stdin
            .write_all(request_line)
            .await
            .map_err(|_| AdvisoryError::ProcessExit)?;
        self.stdin
            .flush()
            .await
            .map_err(|_| AdvisoryError::ProcessExit)?;

        let line = match tokio::time::timeout(timeout, read_bounded_line(&mut self.stdout)).await {
            Ok(result) => result?,
            Err(_) => return Err(AdvisoryError::Timeout),
        };
        let response: WireResponse =
            serde_json::from_slice(&line).map_err(|_| AdvisoryError::MalformedResponse)?;
        if response.protocol != PROTOCOL_VERSION || response.id != request_id {
            return Err(AdvisoryError::MalformedResponse);
        }
        if !response.ok {
            return Err(response
                .error
                .as_ref()
                .map(|error| match error.category.as_str() {
                    "invalid_request" => AdvisoryError::InvalidRequest,
                    "timeout" => AdvisoryError::Timeout,
                    "model_error" => AdvisoryError::ModelError,
                    "process_exit" => AdvisoryError::ProcessExit,
                    "unavailable" => AdvisoryError::Unavailable,
                    _ => AdvisoryError::MalformedResponse,
                })
                .unwrap_or(AdvisoryError::MalformedResponse));
        }

        let decision = response.decision.ok_or(AdvisoryError::MalformedResponse)?;
        let confidence = decision
            .confidence
            .ok_or(AdvisoryError::MalformedResponse)?;
        let effects = match decision.effects {
            Some(effects) if kind == AdvisoryKind::Repetition => {
                normalize_repetition_effects(effects)
            }
            Some(effects) => normalize_effects(effects),
            None if kind == AdvisoryKind::Repetition => Vec::new(),
            None => return Err(AdvisoryError::MalformedResponse),
        };
        if !confidence.is_finite()
            || !(0.0..=1.0).contains(&confidence)
            || !supported_label(&decision.label)
        {
            return Err(AdvisoryError::MalformedResponse);
        }
        if decision.label == "unknown" || confidence < min_confidence {
            return Err(AdvisoryError::ModelError);
        }

        Ok(AdvisoryDecision {
            label: decision.label,
            confidence,
            effects,
            rationale_code: decision.rationale_code,
        })
    }

    async fn terminate(mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

impl Drop for SidecarProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl LayaRuntime {
    pub fn new(config: LayaConfig) -> Self {
        Self {
            config: config.fail_closed(),
            state: Arc::new(tokio::sync::Mutex::new(RuntimeState::default())),
        }
    }

    pub fn config(&self) -> &LayaConfig {
        &self.config
    }

    pub(crate) fn next_request_id(&self, kind: AdvisoryKind) -> String {
        let sequence = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed) + 1;
        format!("rustcode-{kind:?}-{sequence}").to_lowercase()
    }

    pub fn status(&self) -> LayaStatus {
        let mut status = diagnose(&self.config);
        if let Ok(state) = self.state.try_lock() {
            status.runtime_ready = state.process.is_some();
            status.failure_category = state.last_failure.as_ref().map(advisory_error_category);
        }
        status
    }

    /// Evaluate through the process-lived advisory runtime. Off mode returns
    /// a disabled result without touching the lazy runtime state.
    pub async fn evaluate(
        &self,
        request: AdvisoryRequest,
    ) -> Result<AdvisoryDecision, AdvisoryError> {
        if self.config.mode == LayaMode::Off {
            return Err(AdvisoryError::Disabled);
        }

        let request_line = serialize_request(&request)?;
        let mut state = self.state.lock().await;
        if state.seen_ids.contains(&request.id) {
            return Err(AdvisoryError::InvalidRequest);
        }

        if state.process.is_none() {
            let retrying = state.started;
            if retrying && !state.restart_available {
                state.last_failure = Some(AdvisoryError::Unavailable);
                return Err(AdvisoryError::Unavailable);
            }
            state.started = true;
            if retrying {
                state.restart_available = false;
                state.restart_used = true;
            }
            match self.spawn_sidecar().await {
                Ok(process) => state.process = Some(process),
                Err(error) => {
                    state.last_failure = Some(error.clone());
                    state.restart_available = !retrying;
                    return Err(error);
                }
            }
        }

        state.seen_ids.insert(request.id.clone());
        let result = state
            .process
            .as_mut()
            .expect("sidecar process is initialized")
            .evaluate(
                &request_line,
                &request.id,
                self.timeout(),
                self.config.min_confidence,
                request.kind,
            )
            .await;
        if let Err(error) = &result {
            state.last_failure = Some(error.clone());
        }
        if result.as_ref().err().is_some_and(requires_restart) {
            if let Some(process) = state.process.take() {
                process.terminate().await;
            }
            state.restart_available = !state.restart_used;
        }
        result
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(self.config.timeout_ms.max(1))
    }

    fn startup_timeout(&self) -> Duration {
        Duration::from_millis(self.config.startup_timeout_ms.max(1))
    }

    async fn spawn_sidecar(&self) -> Result<SidecarProcess, AdvisoryError> {
        let adapter = self
            .config
            .adapter
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .ok_or(AdvisoryError::Unavailable)?;
        let model = self
            .config
            .model
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .ok_or(AdvisoryError::Unavailable)?;
        if !readable_file(Some(adapter)) || !readable_path(Some(model)) {
            return Err(AdvisoryError::Unavailable);
        }

        let mut child = Command::new(self.config.python_executable())
            .arg(adapter)
            .arg("--model")
            .arg(model)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| AdvisoryError::Unavailable)?;
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                let _ = child.start_kill();
                return Err(AdvisoryError::ProcessExit);
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                let _ = child.start_kill();
                return Err(AdvisoryError::ProcessExit);
            }
        };
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut stderr = BufReader::new(stderr);
                let mut sink = tokio::io::sink();
                let _ = tokio::io::copy(&mut stderr, &mut sink).await;
            });
        }

        let mut process = SidecarProcess {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        };
        let readiness = match tokio::time::timeout(
            self.startup_timeout(),
            read_bounded_line(&mut process.stdout),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                process.terminate().await;
                return Err(error);
            }
            Err(_) => {
                process.terminate().await;
                return Err(AdvisoryError::Timeout);
            }
        };
        let readiness: ReadyMessage = match serde_json::from_slice(&readiness) {
            Ok(readiness) => readiness,
            Err(_) => {
                process.terminate().await;
                return Err(AdvisoryError::MalformedResponse);
            }
        };
        if readiness.protocol != PROTOCOL_VERSION
            || readiness.backend.as_deref().is_none()
            || readiness.model.as_deref().is_none()
            || !readiness.kinds.as_ref().is_some_and(|kinds| {
                kinds.iter().any(|kind| kind == "shell_policy")
                    && kinds.iter().any(|kind| kind == "repetition")
            })
        {
            process.terminate().await;
            return Err(AdvisoryError::MalformedResponse);
        }
        Ok(process)
    }
}

fn requires_restart(error: &AdvisoryError) -> bool {
    matches!(
        error,
        AdvisoryError::Timeout | AdvisoryError::MalformedResponse | AdvisoryError::ProcessExit
    )
}

fn advisory_error_category(error: &AdvisoryError) -> String {
    match error {
        AdvisoryError::Disabled => "disabled",
        AdvisoryError::Unavailable => "unavailable",
        AdvisoryError::InvalidRequest => "invalid_request",
        AdvisoryError::Timeout => "timeout",
        AdvisoryError::MalformedResponse => "malformed_response",
        AdvisoryError::ModelError => "model_error",
        AdvisoryError::ProcessExit => "process_exit",
    }
    .to_owned()
}

fn serialize_request(request: &AdvisoryRequest) -> Result<Vec<u8>, AdvisoryError> {
    if request.id.is_empty() || request.id.len() > MAX_REQUEST_ID_BYTES {
        return Err(AdvisoryError::InvalidRequest);
    }
    let deadline_ms = request.deadline.as_millis().min(u64::MAX as u128) as u64;
    let mut line = serde_json::to_vec(&WireRequest {
        protocol: PROTOCOL_VERSION,
        id: &request.id,
        kind: request.kind,
        input: &request.input,
        deadline_ms,
    })
    .map_err(|_| AdvisoryError::InvalidRequest)?;
    if line.len() + 1 > MAX_LINE_BYTES {
        return Err(AdvisoryError::InvalidRequest);
    }
    line.push(b'\n');
    Ok(line)
}

fn supported_label(label: &str) -> bool {
    matches!(
        label,
        "read_only" | "novel_evidence" | "confirmatory_evidence" | "no_new_information" | "unknown"
    )
}

fn normalize_effects(effects: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for effect in effects {
        let effect = effect.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        let effect = match effect.as_str() {
            "read" => "read_only",
            "workspace_mutation" | "mutating" => "mutation",
            "process" => "process_control",
            "network" | "external" => "network_or_external",
            "read_only" | "mutation" | "process_control" | "network_or_external" | "unknown" => {
                effect.as_str()
            }
            _ => "unknown",
        };
        if !normalized.iter().any(|item| item == effect) {
            normalized.push(effect.to_owned());
        }
    }
    if normalized.is_empty() {
        normalized.push("unknown".to_owned());
    }
    normalized
}

fn normalize_repetition_effects(effects: Vec<String>) -> Vec<String> {
    if effects.is_empty() {
        Vec::new()
    } else {
        normalize_effects(effects)
    }
}

async fn read_bounded_line<R>(reader: &mut R) -> Result<Vec<u8>, AdvisoryError>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    reader
        .take((MAX_LINE_BYTES + 1) as u64)
        .read_until(b'\n', &mut line)
        .await
        .map_err(|_| AdvisoryError::ProcessExit)?;
    if line.is_empty() {
        return Err(AdvisoryError::ProcessExit);
    }
    if line.len() > MAX_LINE_BYTES || !line.ends_with(b"\n") {
        return Err(AdvisoryError::MalformedResponse);
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(line)
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

fn default_startup_timeout_ms() -> u64 {
    DEFAULT_STARTUP_TIMEOUT_MS
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

    const FAKE_SIDECAR: &str = r#"#!/bin/sh
model_path="$2"
mode=$(basename "$model_path")
marker="$model_path.marker"
if [ "$mode" = "slow-readiness" ]; then
    sleep 0.2
fi
printf '%s\n' '{"protocol":1,"backend":"fake","model":"fixture","kinds":["shell_policy","repetition"]}'
if [ "$mode" = "exit" ]; then
    exit 17
fi
if [ "$mode" = "recover" ] && [ ! -e "$marker" ]; then
    : > "$marker"
    exit 23
fi
if [ "$mode" = "fail-twice" ]; then
    if [ ! -e "$marker.1" ]; then
        : > "$marker.1"
        exit 23
    elif [ ! -e "$marker.2" ]; then
        : > "$marker.2"
        exit 23
    fi
fi

while IFS= read -r request; do
    id=$(printf '%s' "$request" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
    case "$mode" in
        success|recover|slow-readiness)
            printf '{"protocol":1,"id":"%s","ok":true,"decision":{"label":"read_only","confidence":0.999,"effects":["read_only"],"rationale_code":"fixture"}}\n' "$id"
            ;;
        fail-twice)
            printf '{"protocol":1,"id":"%s","ok":true,"decision":{"label":"read_only","confidence":0.999,"effects":["read_only"],"rationale_code":"fixture"}}\n' "$id"
            ;;
        repetition)
            printf '{"protocol":1,"id":"%s","ok":true,"decision":{"label":"novel_evidence","confidence":0.999,"effects":["read_only"],"rationale_code":"fixture"}}\n' "$id"
            ;;
        repetition-no-effects)
            printf '{"protocol":1,"id":"%s","ok":true,"decision":{"label":"novel_evidence","confidence":0.999,"rationale_code":"fixture"}}\n' "$id"
            ;;
        semantic-then-success)
            if [ ! -e "$marker" ]; then
                : > "$marker"
                printf '{"protocol":1,"id":"%s","ok":true,"decision":{"label":"unknown","confidence":0.999,"effects":["unknown"],"rationale_code":"fixture"}}\n' "$id"
            else
                printf '{"protocol":1,"id":"%s","ok":true,"decision":{"label":"read_only","confidence":0.999,"effects":["read_only"],"rationale_code":"fixture"}}\n' "$id"
            fi
            ;;
        low-confidence)
            printf '{"protocol":1,"id":"%s","ok":true,"decision":{"label":"read_only","confidence":0.5,"effects":["read_only"],"rationale_code":"fixture"}}\n' "$id"
            ;;
        unknown)
            printf '{"protocol":1,"id":"%s","ok":true,"decision":{"label":"unknown","confidence":0.999,"effects":["unknown"],"rationale_code":"fixture"}}\n' "$id"
            ;;
        wrong-id)
            printf '%s\n' '{"protocol":1,"id":"wrong-id","ok":true,"decision":{"label":"read_only","confidence":0.999,"effects":["read_only"]}}'
            ;;
        serialize)
            sleep 0.1
            printf '{"protocol":1,"id":"%s","ok":true,"decision":{"label":"read_only","confidence":0.999,"effects":["read_only"],"rationale_code":"fixture"}}\n' "$id"
            ;;
        malformed)
            printf '%s\n' 'not-json'
            ;;
        unknown-version)
            printf '{"protocol":2,"id":"%s","ok":true,"decision":{"label":"read_only","confidence":0.999,"effects":["read_only"]}}\n' "$id"
            ;;
        missing-confidence)
            printf '{"protocol":1,"id":"%s","ok":true,"decision":{"label":"read_only","effects":["read_only"]}}\n' "$id"
            ;;
        unsupported-label)
            printf '{"protocol":1,"id":"%s","ok":true,"decision":{"label":"allow","confidence":0.999,"effects":["read_only"]}}\n' "$id"
            ;;
        timeout)
            sleep 1
            ;;
        oversized)
            printf '%s' '{"protocol":1,"id":"'
            printf '%s' "$id"
            printf '%s' '","ok":true,"decision":{"label":"read_only","confidence":0.999,"effects":["read_only"],"rationale_code":"'
            head -c 20000 /dev/zero | tr '\000' x
            printf '%s\n' '"}}'
            ;;
        *)
            exit 19
            ;;
    esac
done
"#;

    fn request(id: &str, kind: AdvisoryKind) -> AdvisoryRequest {
        AdvisoryRequest {
            id: id.to_owned(),
            kind,
            input: serde_json::json!({"fixture": true}),
            deadline: Duration::from_millis(150),
        }
    }

    fn fake_runtime(mode: &str) -> (TempDir, LayaRuntime) {
        fake_runtime_with_config(mode, LayaConfig::default())
    }

    fn fake_runtime_with_config(mode: &str, mut config: LayaConfig) -> (TempDir, LayaRuntime) {
        let dir = TempDir::new().unwrap();
        let adapter = dir.path().join("fake-sidecar.sh");
        let model = dir.path().join(mode);
        std::fs::write(&adapter, FAKE_SIDECAR).unwrap();
        std::fs::write(&model, b"fixture").unwrap();
        config.mode = LayaMode::Shadow;
        config.python = Some("sh".to_owned());
        config.adapter = Some(adapter.display().to_string());
        config.model = Some(model.display().to_string());
        if config.timeout_ms == DEFAULT_TIMEOUT_MS {
            config.timeout_ms = 50;
        }
        (dir, LayaRuntime::new(config))
    }

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

    #[tokio::test]
    async fn off_mode_does_not_spawn_a_sidecar() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("spawned");
        let adapter = dir.path().join("sidecar.sh");
        std::fs::write(&adapter, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
        let runtime = LayaRuntime::new(LayaConfig {
            adapter: Some(adapter.display().to_string()),
            model: Some(dir.path().join("model").display().to_string()),
            ..LayaConfig::default()
        });

        let result = runtime
            .evaluate(request("off", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(result, Err(AdvisoryError::Disabled));
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn sidecar_correlates_successful_shell_response_to_request_id() {
        let (_dir, runtime) = fake_runtime("success");

        let decision = runtime
            .evaluate(request("shell-17", AdvisoryKind::ShellPolicy))
            .await
            .unwrap();

        assert_eq!(decision.label, "read_only");
        assert_eq!(decision.confidence, 0.999);
        assert_eq!(decision.effects, vec!["read_only"]);
    }

    #[tokio::test]
    async fn sidecar_returns_successful_repetition_response() {
        let (_dir, runtime) = fake_runtime("repetition");

        let decision = runtime
            .evaluate(request("repeat-17", AdvisoryKind::Repetition))
            .await
            .unwrap();

        assert_eq!(decision.label, "novel_evidence");
    }

    #[tokio::test]
    async fn package_shaped_repetition_response_without_effects_is_valid() {
        let (_dir, runtime) = fake_runtime("repetition-no-effects");

        let decision = runtime
            .evaluate(request("repeat-no-effects", AdvisoryKind::Repetition))
            .await
            .unwrap();

        assert_eq!(decision.label, "novel_evidence");
        assert!(decision.effects.is_empty());
    }

    #[tokio::test]
    async fn malformed_json_is_not_an_allow_result() {
        let (_dir, runtime) = fake_runtime("malformed");

        let result = runtime
            .evaluate(request("malformed", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(result, Err(AdvisoryError::MalformedResponse));
    }

    #[tokio::test]
    async fn unknown_protocol_version_is_a_malformed_response() {
        let (_dir, runtime) = fake_runtime("unknown-version");

        let result = runtime
            .evaluate(request("unknown-version", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(result, Err(AdvisoryError::MalformedResponse));
    }

    #[tokio::test]
    async fn missing_confidence_is_not_an_allow_result() {
        let (_dir, runtime) = fake_runtime("missing-confidence");

        let result = runtime
            .evaluate(request("missing-confidence", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(result, Err(AdvisoryError::MalformedResponse));
    }

    #[tokio::test]
    async fn low_confidence_is_a_model_fallback_error() {
        let (_dir, runtime) = fake_runtime("low-confidence");

        let result = runtime
            .evaluate(request("low-confidence", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(result, Err(AdvisoryError::ModelError));
    }

    #[tokio::test]
    async fn semantic_fallback_keeps_a_healthy_sidecar_for_the_next_request() {
        let (_dir, runtime) = fake_runtime("semantic-then-success");

        assert_eq!(
            runtime
                .evaluate(request("semantic-first", AdvisoryKind::ShellPolicy))
                .await,
            Err(AdvisoryError::ModelError)
        );
        assert_eq!(
            runtime
                .evaluate(request("semantic-second", AdvisoryKind::ShellPolicy))
                .await
                .unwrap()
                .label,
            "read_only"
        );
    }

    #[tokio::test]
    async fn unknown_label_is_a_model_fallback_error() {
        let (_dir, runtime) = fake_runtime("unknown");

        let result = runtime
            .evaluate(request("unknown", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(result, Err(AdvisoryError::ModelError));
    }

    #[tokio::test]
    async fn unsupported_label_is_not_an_allow_result() {
        let (_dir, runtime) = fake_runtime("unsupported-label");

        let result = runtime
            .evaluate(request("unsupported-label", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(result, Err(AdvisoryError::MalformedResponse));
    }

    #[tokio::test]
    async fn duplicate_request_ids_are_rejected() {
        let (_dir, runtime) = fake_runtime("success");

        runtime
            .evaluate(request("duplicate", AdvisoryKind::ShellPolicy))
            .await
            .unwrap();
        let result = runtime
            .evaluate(request("duplicate", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(result, Err(AdvisoryError::InvalidRequest));
    }

    #[tokio::test]
    async fn missing_model_path_is_unavailable_without_spawning() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("spawned");
        let adapter = dir.path().join("sidecar.sh");
        std::fs::write(&adapter, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
        let runtime = LayaRuntime::new(LayaConfig {
            mode: LayaMode::Shadow,
            python: Some("sh".to_owned()),
            adapter: Some(adapter.display().to_string()),
            model: Some(dir.path().join("missing-model").display().to_string()),
            ..LayaConfig::default()
        });

        let result = runtime
            .evaluate(request("missing-model", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(result, Err(AdvisoryError::Unavailable));
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn sidecar_timeout_is_categorized() {
        let (_dir, runtime) = fake_runtime("timeout");

        let result = runtime
            .evaluate(request("timeout", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(result, Err(AdvisoryError::Timeout));
    }

    #[tokio::test]
    async fn sidecar_process_exit_is_categorized() {
        let (_dir, runtime) = fake_runtime("exit");

        let result = runtime
            .evaluate(request("exit", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(result, Err(AdvisoryError::ProcessExit));
    }

    #[tokio::test]
    async fn oversized_response_is_rejected() {
        let (_dir, runtime) = fake_runtime("oversized");

        let result = runtime
            .evaluate(request("oversized", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(result, Err(AdvisoryError::MalformedResponse));
    }

    #[tokio::test]
    async fn a_later_request_can_restart_once_after_process_failure() {
        let (_dir, runtime) = fake_runtime("recover");

        let first = runtime
            .evaluate(request("first", AdvisoryKind::ShellPolicy))
            .await;
        let second = runtime
            .evaluate(request("second", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(first, Err(AdvisoryError::ProcessExit));
        assert_eq!(second.unwrap().label, "read_only");
    }

    #[tokio::test]
    async fn restart_allowance_is_exhausted_after_two_process_failures() {
        let (_dir, runtime) = fake_runtime("fail-twice");

        let first = runtime
            .evaluate(request("first-failure", AdvisoryKind::ShellPolicy))
            .await;
        let second = runtime
            .evaluate(request("second-failure", AdvisoryKind::ShellPolicy))
            .await;
        let third = runtime
            .evaluate(request("after-exhaustion", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(first, Err(AdvisoryError::ProcessExit));
        assert_eq!(second, Err(AdvisoryError::ProcessExit));
        assert_eq!(third, Err(AdvisoryError::Unavailable));
    }

    #[tokio::test]
    async fn readiness_uses_the_startup_timeout_not_the_inference_timeout() {
        let (_dir, runtime) = fake_runtime_with_config(
            "slow-readiness",
            LayaConfig {
                timeout_ms: 20,
                startup_timeout_ms: 500,
                ..LayaConfig::default()
            },
        );

        let decision = runtime
            .evaluate(request("slow-start", AdvisoryKind::ShellPolicy))
            .await
            .unwrap();

        assert_eq!(decision.label, "read_only");
    }

    #[tokio::test]
    async fn wrong_response_id_is_malformed() {
        let (_dir, runtime) = fake_runtime("wrong-id");

        let result = runtime
            .evaluate(request("expected-id", AdvisoryKind::ShellPolicy))
            .await;

        assert_eq!(result, Err(AdvisoryError::MalformedResponse));
    }

    #[tokio::test]
    async fn concurrent_evaluations_are_serialized() {
        let (_dir, runtime) = fake_runtime_with_config(
            "serialize",
            LayaConfig {
                timeout_ms: 500,
                ..LayaConfig::default()
            },
        );
        let started = std::time::Instant::now();

        let (first, second) = tokio::join!(
            runtime.evaluate(request("concurrent-1", AdvisoryKind::ShellPolicy)),
            runtime.evaluate(request("concurrent-2", AdvisoryKind::ShellPolicy)),
        );

        assert_eq!(first.unwrap().label, "read_only");
        assert_eq!(second.unwrap().label, "read_only");
        assert!(started.elapsed() >= Duration::from_millis(180));
    }

    #[test]
    fn runtime_status_is_pure_and_reports_protocol() {
        let config = LayaConfig::default();
        let runtime = LayaRuntime::new(config.clone());

        assert_eq!(runtime.status(), diagnose(&config));
        assert_eq!(runtime.status().protocol_version, 1);
    }

    #[test]
    fn status_accepts_a_readable_model_directory() {
        let dir = TempDir::new().unwrap();
        let adapter = dir.path().join("adapter.py");
        let model = dir.path().join("model");
        std::fs::write(&adapter, b"sidecar").unwrap();
        std::fs::create_dir(&model).unwrap();
        let config = LayaConfig {
            mode: LayaMode::Shadow,
            python: Some(std::env::current_exe().unwrap().display().to_string()),
            adapter: Some(adapter.display().to_string()),
            model: Some(model.display().to_string()),
            ..LayaConfig::default()
        };

        let status = diagnose(&config);

        assert!(status.adapter_available);
        assert!(status.model_available);
    }

    #[test]
    fn status_names_missing_runtime_prerequisite_categories() {
        let status = format_status(&LayaConfig {
            mode: LayaMode::Shadow,
            python: Some("/missing/python".to_owned()),
            adapter: Some("/missing/adapter.py".to_owned()),
            model: Some("/missing/checkpoint".to_owned()),
            ..LayaConfig::default()
        });

        assert!(status.contains("Python executable: /missing/python (missing)"));
        assert!(status.contains("Adapter: configured, missing"));
        assert!(status.contains("Model: configured, missing"));
    }

    #[test]
    fn status_distinguishes_available_paths_from_an_unstarted_runtime() {
        let status = format_status(&LayaConfig {
            mode: LayaMode::Shadow,
            python: Some(std::env::current_exe().unwrap().display().to_string()),
            adapter: Some("/missing/adapter.py".to_owned()),
            model: Some("/missing/checkpoint".to_owned()),
            ..LayaConfig::default()
        });

        assert!(status.contains("Paths available:"));
        assert!(status.contains("Runtime ready: no"));
        assert!(status.contains("Failure category:"));
    }

    #[test]
    fn readme_documents_pinned_offline_laya_setup_and_disable_path() {
        let readme = include_str!("../README.md");
        for required in [
            "Apple Silicon",
            "Python 3.11+",
            "laya-mlx==0.2.0",
            "checkpoint",
            "does not install software",
            "rustcode laya disable",
            "mode = \"shadow\"",
        ] {
            assert!(
                readme.contains(required),
                "README is missing the Laya setup requirement: {required}"
            );
        }
    }

    #[test]
    fn status_rejects_a_non_executable_python_file() {
        let dir = TempDir::new().unwrap();
        let python = dir.path().join("python");
        let adapter = dir.path().join("adapter.py");
        let model = dir.path().join("model");
        std::fs::write(&python, b"not executable").unwrap();
        std::fs::write(&adapter, b"sidecar").unwrap();
        std::fs::create_dir(&model).unwrap();
        let status = diagnose(&LayaConfig {
            mode: LayaMode::Shadow,
            python: Some(python.display().to_string()),
            adapter: Some(adapter.display().to_string()),
            model: Some(model.display().to_string()),
            ..LayaConfig::default()
        });

        assert!(!status.python_available);
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
