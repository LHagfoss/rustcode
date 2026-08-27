//! Native local audio tools.
//!
//! Generation is deliberately process based: model runtimes stay outside the
//! RustCode process and can be installed, upgraded, or removed independently.

use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

use super::{Tool, ToolCapability, ToolSafety};

const MAX_BACKEND_LOG_BYTES: usize = 16 * 1024;

fn generation_schema(max_duration: u32) -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "prompt": { "type": "string", "minLength": 1 },
            "duration_seconds": { "type": "number", "exclusiveMinimum": 0, "maximum": max_duration },
            "output_path": { "type": "string", "minLength": 1, "description": "Project-relative output path, usually under assets/audio/" }
        },
        "required": ["prompt", "duration_seconds", "output_path"]
    })
}

fn sound_effect_schema() -> Value {
    generation_schema(300)
}

fn music_schema() -> Value {
    // The verified local music CLI currently supports up to 30 seconds.
    generation_schema(30)
}

fn inspect_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "path": { "type": "string", "minLength": 1, "description": "Project-relative audio path" }
        },
        "required": ["path"]
    })
}

pub const GENERATE_SOUND_EFFECT: Tool = Tool {
    name: "generate_sound_effect",
    description: "Generate a short local sound-effect asset for the project (for example a coin pickup, click, hit, or jump), then return the real project-relative path and audio metadata. Use this when audio materially improves a game or interactive app; do not invent an asset reference.",
    arguments: r#"{"prompt":"short sound description","duration_seconds":1.2,"output_path":"assets/audio/effect.wav"}"#,
    handler: generate_sound_effect,
    requires_confirmation: true,
    schema: sound_effect_schema,
    capabilities: &[ToolCapability::WriteWorkspace],
    safety: ToolSafety::WorkspaceMutation,
};

pub const GENERATE_MUSIC: Tool = Tool {
    name: "generate_music",
    description: "Generate an instrumental local background-music asset for the project and return the real project-relative path and audio metadata. Use this when music materially improves a game or interactive app; do not fabricate a file reference.",
    arguments: r#"{"prompt":"playful instrumental soundtrack","duration_seconds":30,"output_path":"assets/audio/theme.wav"}"#,
    handler: generate_music,
    requires_confirmation: true,
    schema: music_schema,
    capabilities: &[ToolCapability::WriteWorkspace],
    safety: ToolSafety::WorkspaceMutation,
};

pub const INSPECT_AUDIO: Tool = Tool {
    name: "inspect_audio",
    description: "Inspect a project-relative audio file and return concise metadata including format, duration, channels, sample rate, and file size.",
    arguments: r#"{"path":"assets/audio/theme.wav"}"#,
    handler: inspect_audio,
    requires_confirmation: false,
    schema: inspect_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioErrorKind {
    Disabled,
    InvalidPath,
    MissingBackend,
    BackendUnavailable,
    ModelUnavailable,
    GenerationFailed,
    Cancelled,
    InvalidAudio,
}

#[derive(Debug, Clone)]
pub(crate) struct AudioError {
    pub(crate) kind: AudioErrorKind,
    pub(crate) message: String,
}

impl AudioError {
    fn new(kind: AudioErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GenerationKind {
    Sfx,
    Music,
}

trait AudioGenerationBackend: Send + Sync {
    fn id(&self) -> &'static str;
    fn executable(&self) -> &'static str;
    fn command(
        &self,
        kind: GenerationKind,
        prompt: &str,
        duration: f64,
        output: &Path,
    ) -> Vec<OsString>;
}

struct MlxSpeechBackend;

impl AudioGenerationBackend for MlxSpeechBackend {
    fn id(&self) -> &'static str {
        "mlx-speech"
    }
    fn executable(&self) -> &'static str {
        "mlx-speech"
    }
    fn command(
        &self,
        _kind: GenerationKind,
        prompt: &str,
        duration: f64,
        output: &Path,
    ) -> Vec<OsString> {
        vec![
            "tts".into(),
            "--model".into(),
            "moss-sound-effect".into(),
            "--text".into(),
            prompt.into(),
            "--duration-seconds".into(),
            duration.to_string().into(),
            "-o".into(),
            output.as_os_str().to_os_string(),
        ]
    }
}

struct MusicgenBackend;

impl AudioGenerationBackend for MusicgenBackend {
    fn id(&self) -> &'static str {
        "musicgen-mlx"
    }
    fn executable(&self) -> &'static str {
        "musicgen-mlx"
    }
    fn command(
        &self,
        _kind: GenerationKind,
        prompt: &str,
        duration: f64,
        output: &Path,
    ) -> Vec<OsString> {
        vec![
            prompt.into(),
            "-o".into(),
            output.as_os_str().to_os_string(),
            "-d".into(),
            duration.to_string().into(),
            "--no-open".into(),
        ]
    }
}

struct ProcessOutput {
    status: ExitStatus,
    stderr: String,
}

trait ProcessRunner {
    fn run(
        &self,
        program: &Path,
        args: &[OsString],
        cancel: Option<&CancellationToken>,
    ) -> Result<ProcessOutput, AudioError>;
}

struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(
        &self,
        program: &Path,
        args: &[OsString],
        cancel: Option<&CancellationToken>,
    ) -> Result<ProcessOutput, AudioError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .env("PATH", backend_search_path());
        let mut child = command.spawn().map_err(|error| {
            AudioError::new(AudioErrorKind::MissingBackend, format!("could not start audio backend '{}': {error}. Install the backend and ensure it is on PATH.", program.display()))
        })?;
        let stderr_reader = child.stderr.take().map(|mut stderr| {
            thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = stderr.read_to_end(&mut bytes);
                bytes.truncate(MAX_BACKEND_LOG_BYTES);
                String::from_utf8_lossy(&bytes).trim().to_string()
            })
        });

        loop {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                let _ = child.kill();
                let _ = child.wait();
                if let Some(reader) = stderr_reader {
                    let _ = reader.join();
                }
                return Err(AudioError::new(
                    AudioErrorKind::Cancelled,
                    "audio generation cancelled; partial output was removed",
                ));
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    let stderr = stderr_reader
                        .and_then(|reader| reader.join().ok())
                        .unwrap_or_default();
                    return Ok(ProcessOutput { status, stderr });
                }
                Ok(None) => thread::sleep(Duration::from_millis(40)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(reader) = stderr_reader {
                        let _ = reader.join();
                    }
                    return Err(AudioError::new(
                        AudioErrorKind::GenerationFailed,
                        format!("could not monitor audio backend: {error}"),
                    ));
                }
            }
        }
    }
}

fn backend_search_path() -> OsString {
    let mut search_dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        // Keep the documented RustCode venv and musicgen installation ahead
        // of a GUI-launched process's inherited PATH.
        search_dirs.push(PathBuf::from(&home).join(".local/share/rustcode/audio-venv/bin"));
        search_dirs.push(PathBuf::from(&home).join(".local/bin"));
    }
    search_dirs.extend(std::env::split_paths(&crate::network::augmented_path()));
    std::env::join_paths(search_dirs)
        .unwrap_or_else(|_| OsString::from(crate::network::augmented_path()))
}

fn available_executable(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().count() > 1 {
        return path.is_file().then(|| path.to_path_buf());
    }
    std::env::split_paths(&backend_search_path())
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn configured_backend(
    kind: GenerationKind,
    config: &crate::config::AudioConfig,
) -> Result<Box<dyn AudioGenerationBackend>, AudioError> {
    configured_backend_with_probe(kind, config, |name| available_executable(name).is_some())
}

fn configured_backend_with_probe(
    kind: GenerationKind,
    config: &crate::config::AudioConfig,
    probe: impl Fn(&str) -> bool,
) -> Result<Box<dyn AudioGenerationBackend>, AudioError> {
    let requested = match kind {
        GenerationKind::Sfx => &config.sfx_backend,
        GenerationKind::Music => &config.music_backend,
    };
    let backends: Vec<Box<dyn AudioGenerationBackend>> = match kind {
        GenerationKind::Sfx => vec![Box::new(MlxSpeechBackend)],
        GenerationKind::Music => vec![Box::new(MusicgenBackend)],
    };
    if requested.eq_ignore_ascii_case("auto") {
        return backends.into_iter().find(|backend| probe(backend.executable())).ok_or_else(|| {
            let expected = match kind { GenerationKind::Sfx => "mlx-speech", GenerationKind::Music => "musicgen-mlx" };
            AudioError::new(AudioErrorKind::MissingBackend, format!("no local audio backend was found. Install {expected} and ensure its executable is on PATH; see the local audio setup docs."))
        });
    }
    backends.into_iter().find(|backend| backend.id().eq_ignore_ascii_case(requested)).ok_or_else(|| {
        AudioError::new(AudioErrorKind::BackendUnavailable, format!("audio backend '{requested}' is not supported; use 'auto' or a supported local backend"))
    })
}

fn workspace_root() -> PathBuf {
    super::ACTIVE_WORKSPACE_ROOT
        .with(|root| root.borrow().clone())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn project_path(raw: &str) -> Result<(PathBuf, String), AudioError> {
    project_path_with_parent_creation(raw, true)
}

fn project_path_with_parent_creation(
    raw: &str,
    create_parent: bool,
) -> Result<(PathBuf, String), AudioError> {
    let input = raw.trim();
    if input.is_empty() {
        return Err(AudioError::new(
            AudioErrorKind::InvalidPath,
            "audio path must not be empty",
        ));
    }
    let relative = Path::new(input);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Err(AudioError::new(
            AudioErrorKind::InvalidPath,
            "audio output paths must be project-relative and must not contain '..' or an absolute prefix",
        ));
    }
    if relative
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| !extension.eq_ignore_ascii_case("wav"))
        .unwrap_or(true)
    {
        return Err(AudioError::new(
            AudioErrorKind::InvalidPath,
            "audio paths must use the .wav format",
        ));
    }
    let root = fs::canonicalize(workspace_root()).map_err(|error| {
        AudioError::new(
            AudioErrorKind::InvalidPath,
            format!("could not resolve project workspace: {error}"),
        )
    })?;
    let absolute = root.join(relative);
    if let Some(parent) = absolute.parent() {
        let mut existing = parent;
        while !existing.exists() {
            existing = existing.parent().ok_or_else(|| {
                AudioError::new(
                    AudioErrorKind::InvalidPath,
                    "could not resolve audio output directory",
                )
            })?;
        }
        let canonical_existing = fs::canonicalize(existing).map_err(|error| {
            AudioError::new(
                AudioErrorKind::InvalidPath,
                format!("could not resolve audio output directory: {error}"),
            )
        })?;
        if !canonical_existing.starts_with(&root) {
            return Err(AudioError::new(
                AudioErrorKind::InvalidPath,
                "audio output directory escapes the project workspace",
            ));
        }
        if create_parent {
            fs::create_dir_all(parent).map_err(|error| {
                AudioError::new(
                    AudioErrorKind::InvalidPath,
                    format!(
                        "could not create audio output directory '{}': {error}",
                        parent.display()
                    ),
                )
            })?;
        }
        if !parent.exists() {
            return Err(AudioError::new(
                AudioErrorKind::InvalidPath,
                "audio output directory does not exist",
            ));
        }
        let canonical_parent = fs::canonicalize(parent).map_err(|error| {
            AudioError::new(
                AudioErrorKind::InvalidPath,
                format!("could not resolve audio output directory: {error}"),
            )
        })?;
        if !canonical_parent.starts_with(&root) {
            return Err(AudioError::new(
                AudioErrorKind::InvalidPath,
                "audio output directory escapes the project workspace",
            ));
        }
    }
    if absolute.exists() {
        let canonical = fs::canonicalize(&absolute).map_err(|error| {
            AudioError::new(
                AudioErrorKind::InvalidPath,
                format!("could not resolve audio output path: {error}"),
            )
        })?;
        if !canonical.starts_with(&root) {
            return Err(AudioError::new(
                AudioErrorKind::InvalidPath,
                "audio output path escapes the project workspace",
            ));
        }
    }
    Ok((absolute, input.replace('\\', "/")))
}

fn duration(args: &Value) -> Result<f64, AudioError> {
    let value = args
        .get("duration_seconds")
        .and_then(Value::as_f64)
        .ok_or_else(|| {
            AudioError::new(
                AudioErrorKind::InvalidPath,
                "missing or invalid 'duration_seconds'",
            )
        })?;
    if !(value.is_finite() && value > 0.0 && value <= 300.0) {
        return Err(AudioError::new(
            AudioErrorKind::InvalidPath,
            "duration_seconds must be greater than 0 and no more than 300",
        ));
    }
    Ok(value)
}

fn temp_output(path: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("audio.wav");
    path.with_file_name(format!(
        ".{name}.rustcode-{stamp}-{}.tmp.wav",
        std::process::id()
    ))
}

fn generate(
    kind: GenerationKind,
    args: &Value,
    cancel: Option<&CancellationToken>,
    runner: &dyn ProcessRunner,
) -> Result<String, AudioError> {
    let prompt = args
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| AudioError::new(AudioErrorKind::InvalidPath, "missing or empty 'prompt'"))?;
    let duration = duration(args)?;
    if kind == GenerationKind::Music && duration > 30.0 {
        return Err(AudioError::new(
            AudioErrorKind::InvalidPath,
            "music generation currently supports durations up to 30 seconds",
        ));
    }
    let raw_path = args
        .get("output_path")
        .and_then(Value::as_str)
        .ok_or_else(|| AudioError::new(AudioErrorKind::InvalidPath, "missing 'output_path'"))?;
    let (output, relative) = project_path(raw_path)?;
    let root = workspace_root();
    let config = crate::config::load_config_for_workspace(&root).2.audio;
    if !config.enabled {
        return Err(AudioError::new(
            AudioErrorKind::Disabled,
            "audio generation is disabled in configuration ([audio].enabled = false)",
        ));
    }
    let backend = configured_backend(kind, &config)?;
    let executable = available_executable(backend.executable()).ok_or_else(|| {
        AudioError::new(
            AudioErrorKind::MissingBackend,
            format!(
                "backend '{}' is not installed or is not on PATH",
                backend.id()
            ),
        )
    })?;
    generate_with_selected_backend(
        kind,
        prompt,
        duration,
        output,
        relative,
        cancel,
        runner,
        backend.as_ref(),
        &executable,
    )
}

fn generate_with_selected_backend(
    kind: GenerationKind,
    prompt: &str,
    duration: f64,
    output: PathBuf,
    relative: String,
    cancel: Option<&CancellationToken>,
    runner: &dyn ProcessRunner,
    backend: &dyn AudioGenerationBackend,
    executable: &Path,
) -> Result<String, AudioError> {
    let temporary = temp_output(&output);
    let args_for_backend = backend.command(kind, prompt, duration, &temporary);
    let result = runner.run(executable, &args_for_backend, cancel);
    let process = match result {
        Ok(process) => process,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    if !process.status.success() {
        let detail = if process.stderr.is_empty() {
            String::new()
        } else {
            format!(" Backend output: {}", process.stderr)
        };
        let kind = if process.stderr.to_ascii_lowercase().contains("model") {
            AudioErrorKind::ModelUnavailable
        } else {
            AudioErrorKind::GenerationFailed
        };
        let _ = fs::remove_file(&temporary);
        return Err(AudioError::new(
            kind,
            format!(
                "audio backend '{}' failed with {}.{detail}",
                backend.id(),
                process.status
            ),
        ));
    }
    let metadata = match inspect_path(&temporary) {
        Ok(metadata) => metadata,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(AudioError::new(
                AudioErrorKind::InvalidAudio,
                format!(
                    "audio backend produced an invalid output: {}",
                    error.message
                ),
            ));
        }
    };
    if let Err(error) = crate::atomic_file::replace_file(&temporary, &output) {
        let _ = fs::remove_file(&temporary);
        return Err(AudioError::new(
            AudioErrorKind::GenerationFailed,
            format!(
                "could not finalize generated audio '{}': {error}",
                output.display()
            ),
        ));
    }
    Ok(serde_json::json!({ "path": relative, "format": metadata.format, "duration_seconds": metadata.duration_seconds, "channels": metadata.channels, "sample_rate": metadata.sample_rate, "file_size_bytes": metadata.file_size_bytes }).to_string())
}

pub(crate) fn generate_with_cancel(
    kind: GenerationKind,
    args: &Value,
    cancel: Option<CancellationToken>,
) -> Result<String, AudioError> {
    generate(kind, args, cancel.as_ref(), &SystemProcessRunner)
}

fn generate_sound_effect(args: &Value) -> Result<String, String> {
    generate_with_cancel(GenerationKind::Sfx, args, None).map_err(|error| error.message)
}
fn generate_music(args: &Value) -> Result<String, String> {
    generate_with_cancel(GenerationKind::Music, args, None).map_err(|error| error.message)
}

#[derive(Debug, Clone, PartialEq)]
struct AudioMetadata {
    format: String,
    duration_seconds: f64,
    channels: u16,
    sample_rate: u32,
    file_size_bytes: u64,
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    bytes
        .get(offset..offset + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
}
fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn inspect_path(path: &Path) -> Result<AudioMetadata, AudioError> {
    let bytes = fs::read(path).map_err(|error| {
        AudioError::new(
            AudioErrorKind::InvalidAudio,
            format!("could not read audio file: {error}"),
        )
    })?;
    let file_size_bytes = bytes.len() as u64;
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if extension != "wav" && extension != "wave" {
        return Err(AudioError::new(
            AudioErrorKind::InvalidAudio,
            "audio inspection currently supports WAV files",
        ));
    }
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(AudioError::new(
            AudioErrorKind::InvalidAudio,
            "file is not a RIFF/WAVE audio file",
        ));
    }
    let mut cursor = 12;
    let mut channels = None;
    let mut sample_rate = None;
    let mut byte_rate = None;
    let mut data_bytes = None;
    while cursor + 8 <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let size = read_u32(&bytes, cursor + 4).unwrap_or(0) as usize;
        let start = cursor + 8;
        let end = start.saturating_add(size).min(bytes.len());
        if id == b"fmt " && end >= start + 12 {
            channels = read_u16(&bytes, start + 2);
            sample_rate = read_u32(&bytes, start + 4);
            byte_rate = read_u32(&bytes, start + 8);
        } else if id == b"data" {
            data_bytes = Some(size.min(bytes.len().saturating_sub(start)));
        }
        cursor = start.saturating_add(size + (size & 1));
    }
    let channels = channels.ok_or_else(|| {
        AudioError::new(AudioErrorKind::InvalidAudio, "WAV file has no fmt chunk")
    })?;
    let sample_rate = sample_rate.ok_or_else(|| {
        AudioError::new(AudioErrorKind::InvalidAudio, "WAV file has no sample rate")
    })?;
    let byte_rate = byte_rate.filter(|rate| *rate > 0).ok_or_else(|| {
        AudioError::new(
            AudioErrorKind::InvalidAudio,
            "WAV file has no usable byte rate",
        )
    })?;
    let data_bytes = data_bytes.ok_or_else(|| {
        AudioError::new(AudioErrorKind::InvalidAudio, "WAV file has no data chunk")
    })?;
    Ok(AudioMetadata {
        format: "wav".to_string(),
        duration_seconds: data_bytes as f64 / byte_rate as f64,
        channels,
        sample_rate,
        file_size_bytes,
    })
}

fn inspect_audio(args: &Value) -> Result<String, String> {
    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or("missing 'path' argument")?;
    let (path, relative) =
        project_path_with_parent_creation(raw, false).map_err(|error| error.message)?;
    let metadata = inspect_path(&path).map_err(|error| error.message)?;
    Ok(serde_json::json!({ "path": relative, "format": metadata.format, "duration_seconds": metadata.duration_seconds, "channels": metadata.channels, "sample_rate": metadata.sample_rate, "file_size_bytes": metadata.file_size_bytes }).to_string())
}

pub(crate) fn map_error_kind(kind: AudioErrorKind) -> super::ToolErrorKind {
    match kind {
        AudioErrorKind::Disabled
        | AudioErrorKind::MissingBackend
        | AudioErrorKind::BackendUnavailable
        | AudioErrorKind::ModelUnavailable => super::ToolErrorKind::UnavailableDependency,
        AudioErrorKind::InvalidPath => super::ToolErrorKind::InvalidArguments,
        AudioErrorKind::Cancelled => super::ToolErrorKind::Cancelled,
        AudioErrorKind::GenerationFailed | AudioErrorKind::InvalidAudio => {
            super::ToolErrorKind::CommandFailed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;
    #[cfg(unix)]
    use std::sync::mpsc;
    use tempfile::TempDir;

    fn exit_status(code: i32) -> ExitStatus {
        #[cfg(unix)]
        {
            ExitStatus::from_raw(code << 8)
        }
        #[cfg(windows)]
        {
            ExitStatus::from_raw(code as u32)
        }
    }

    struct MockRunner {
        status: ExitStatus,
        output: Option<Vec<u8>>,
        cancel: bool,
    }
    impl ProcessRunner for MockRunner {
        fn run(
            &self,
            _program: &Path,
            args: &[OsString],
            _cancel: Option<&CancellationToken>,
        ) -> Result<ProcessOutput, AudioError> {
            if let Some(output) = &self.output {
                let path = args.last().unwrap();
                fs::write(Path::new(path), output).unwrap();
            }
            if self.cancel {
                return Err(AudioError::new(AudioErrorKind::Cancelled, "cancelled"));
            }
            Ok(ProcessOutput {
                status: self.status,
                stderr: String::new(),
            })
        }
    }

    fn wav() -> Vec<u8> {
        let data = vec![0u8; 8];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36u32 + data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&8000u32.to_le_bytes());
        bytes.extend_from_slice(&16000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        bytes
    }

    #[test]
    fn rejects_parent_traversal() {
        assert!(project_path("assets/../outside.wav").is_err());
    }

    #[test]
    fn inspects_wav_metadata() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a.wav");
        fs::write(&path, wav()).unwrap();
        let metadata = inspect_path(&path).unwrap();
        assert_eq!(metadata.channels, 1);
        assert_eq!(metadata.sample_rate, 8000);
        assert_eq!(metadata.duration_seconds, 0.0005);
    }

    #[test]
    fn mocked_generation_creates_directory_and_structured_result() {
        let dir = TempDir::new().unwrap();
        super::super::set_active_workspace_root(Some(dir.path().to_path_buf()));
        let args = serde_json::json!({"prompt":"coin","duration_seconds":1.0,"output_path":"assets/audio/coin.wav"});
        let (output, relative) = project_path("assets/audio/coin.wav").unwrap();
        let runner = MockRunner {
            status: exit_status(0),
            output: Some(wav()),
            cancel: false,
        };
        let result = generate_with_selected_backend(
            GenerationKind::Sfx,
            "coin",
            1.0,
            output,
            relative,
            None,
            &runner,
            &MlxSpeechBackend,
            Path::new("mock"),
        )
        .unwrap();
        let result: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(result["format"], "wav");
        assert!(dir.path().join("assets/audio/coin.wav").exists());
        assert_eq!(args["prompt"], "coin");
        super::super::set_active_workspace_root(None);
    }

    #[test]
    fn selected_backend_returns_structured_result_and_handles_spaces() {
        let dir = TempDir::new().unwrap();
        super::super::set_active_workspace_root(Some(dir.path().to_path_buf()));
        let (output, relative) = project_path("assets/audio/my coin sound.wav").unwrap();
        let runner = MockRunner {
            status: exit_status(0),
            output: Some(wav()),
            cancel: false,
        };
        let result = generate_with_selected_backend(
            GenerationKind::Sfx,
            "coin pickup",
            1.0,
            output,
            relative,
            None,
            &runner,
            &MlxSpeechBackend,
            Path::new("mock executable with spaces"),
        )
        .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["path"], "assets/audio/my coin sound.wav");
        assert_eq!(value["format"], "wav");
        assert!(dir.path().join("assets/audio/my coin sound.wav").exists());
        super::super::set_active_workspace_root(None);
    }

    #[test]
    fn nonzero_and_cancelled_generation_remove_partial_output() {
        let dir = TempDir::new().unwrap();
        super::super::set_active_workspace_root(Some(dir.path().to_path_buf()));
        let (output, relative) = project_path("assets/audio/fail.wav").unwrap();
        let failed = MockRunner {
            status: exit_status(1),
            output: Some(wav()),
            cancel: false,
        };
        let error = generate_with_selected_backend(
            GenerationKind::Sfx,
            "fail",
            1.0,
            output.clone(),
            relative.clone(),
            None,
            &failed,
            &MlxSpeechBackend,
            Path::new("mock"),
        )
        .unwrap_err();
        assert_eq!(error.kind, AudioErrorKind::GenerationFailed);
        assert!(!output.exists());
        let cancelled = MockRunner {
            status: exit_status(0),
            output: Some(wav()),
            cancel: true,
        };
        let error = generate_with_selected_backend(
            GenerationKind::Sfx,
            "cancel",
            1.0,
            output.clone(),
            relative,
            None,
            &cancelled,
            &MlxSpeechBackend,
            Path::new("mock"),
        )
        .unwrap_err();
        assert_eq!(error.kind, AudioErrorKind::Cancelled);
        assert!(!output.exists());
        super::super::set_active_workspace_root(None);
    }

    #[test]
    fn inspect_audio_does_not_create_missing_directories() {
        let dir = TempDir::new().unwrap();
        super::super::set_active_workspace_root(Some(dir.path().to_path_buf()));
        let result = inspect_audio(&serde_json::json!({"path":"missing/audio.wav"}));
        assert!(result.is_err());
        assert!(!dir.path().join("missing").exists());
        super::super::set_active_workspace_root(None);
    }

    #[test]
    fn tool_schemas_validate_typed_arguments() {
        let valid = super::super::ToolCall {
            name: "generate_music".to_string(),
            arguments: serde_json::json!({
                "prompt": "quiet menu music",
                "duration_seconds": 30.0,
                "output_path": "assets/audio/menu.wav"
            }),
            call_id: None,
        };
        assert!(super::super::validate_tool_calls(&[valid]).is_ok());
        let invalid = super::super::ToolCall {
            name: "generate_music".to_string(),
            arguments: serde_json::json!({
                "prompt": "quiet menu music",
                "duration_seconds": "30",
                "output_path": "assets/audio/menu.wav"
            }),
            call_id: None,
        };
        assert!(super::super::validate_tool_calls(&[invalid]).is_err());
    }

    #[test]
    fn missing_executable_is_reported_as_dependency_error() {
        let error = match SystemProcessRunner.run(
            Path::new("/definitely/missing/rustcode-audio"),
            &[],
            None,
        ) {
            Ok(_) => panic!("missing executable unexpectedly started"),
            Err(error) => error,
        };
        assert_eq!(error.kind, AudioErrorKind::MissingBackend);
        assert!(error.message.contains("Install"));
    }

    #[test]
    fn auto_backend_selection_uses_the_first_probed_backend() {
        let config = crate::config::AudioConfig::default();
        let selected = configured_backend_with_probe(GenerationKind::Sfx, &config, |name| {
            name == "mlx-speech"
        })
        .unwrap();
        assert_eq!(selected.id(), "mlx-speech");
        let missing = match configured_backend_with_probe(GenerationKind::Sfx, &config, |_| false) {
            Ok(_) => panic!("auto selection unexpectedly found a backend"),
            Err(error) => error,
        };
        assert_eq!(missing.kind, AudioErrorKind::MissingBackend);
    }

    #[cfg(unix)]
    struct SlowShellBackend;
    #[cfg(unix)]
    impl AudioGenerationBackend for SlowShellBackend {
        fn id(&self) -> &'static str {
            "test"
        }
        fn executable(&self) -> &'static str {
            "sh"
        }
        fn command(
            &self,
            _kind: GenerationKind,
            _prompt: &str,
            _duration: f64,
            output: &Path,
        ) -> Vec<OsString> {
            vec![
                "-c".into(),
                "for arg do out=\"$arg\"; done; printf partial > \"$out\"; sleep 5 2>/dev/null"
                    .into(),
                "rustcode-test".into(),
                output.as_os_str().to_os_string(),
            ]
        }
    }

    #[cfg(unix)]
    #[test]
    fn system_runner_kills_cancelled_child_and_generation_removes_temp_file() {
        let dir = TempDir::new().unwrap();
        let output = dir.path().join("assets/audio/cancel.wav");
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        let relative = "assets/audio/cancel.wav".to_string();
        let token = CancellationToken::new();
        let worker_token = token.clone();
        let (tx, rx) = mpsc::channel();
        let worker_output = output.clone();
        let worker = std::thread::spawn(move || {
            let result = generate_with_selected_backend(
                GenerationKind::Sfx,
                "cancel",
                1.0,
                worker_output,
                relative,
                Some(&worker_token),
                &SystemProcessRunner,
                &SlowShellBackend,
                Path::new("/bin/sh"),
            );
            tx.send(result).unwrap();
        });
        std::thread::sleep(Duration::from_millis(150));
        token.cancel();
        let result = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        assert_eq!(result.unwrap_err().kind, AudioErrorKind::Cancelled);
        assert!(!output.exists());
        assert!(
            fs::read_dir(output.parent().unwrap())
                .unwrap()
                .next()
                .is_none()
        );
    }
}
