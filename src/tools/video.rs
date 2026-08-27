use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

use super::{CommandProgressCallback, Tool, ToolCapability, ToolSafety};

const MAX_PROCESS_OUTPUT: usize = 32 * 1024;
const MAX_VIDEO_WIDTH: u32 = 8192;
const MAX_VIDEO_HEIGHT: u32 = 8192;
const MAX_VIDEO_PIXELS: u64 = 33_177_600;
const MAX_VIDEO_DURATION_SECONDS: f64 = 3_600.0;

fn inspect_schema() -> Value {
    serde_json::json!({
        "type":"object","additionalProperties":false,
        "properties":{"path":{"type":"string","minLength":1,"description":"Project-relative media path"}},
        "required":["path"]
    })
}

fn project_schema() -> Value {
    serde_json::json!({
        "type":"object","additionalProperties":false,
        "properties":{"project_path":{"type":"string","minLength":1,"description":"Project-relative JSON video composition path"}},
        "required":["project_path"]
    })
}

pub const INSPECT_MEDIA: Tool = Tool {
    name: "inspect_media",
    description: "Inspect a project-relative video or audio file with ffprobe and return concise typed stream metadata.",
    arguments: r#"{"path":"media/clip.mp4"}"#,
    handler: inspect_media_handler,
    requires_confirmation: false,
    schema: inspect_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

pub const VALIDATE_VIDEO_PROJECT: Tool = Tool {
    name: "validate_video_project",
    description: "Validate a declarative project-relative JSON video composition without rendering it. Returns timeline details, errors, and warnings.",
    arguments: r#"{"project_path":"video-project.json"}"#,
    handler: validate_handler,
    requires_confirmation: false,
    schema: project_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

pub const RENDER_VIDEO: Tool = Tool {
    name: "render_video",
    description: "Validate and render a declarative project-relative JSON video composition with FFmpeg. Supports ordered clips, trims, normalization, transitions, clip audio, and background music.",
    arguments: r#"{"project_path":"video-project.json"}"#,
    handler: render_handler,
    requires_confirmation: true,
    schema: project_schema,
    capabilities: &[
        ToolCapability::ReadWorkspace,
        ToolCapability::WriteWorkspace,
        ToolCapability::ExecuteCommands,
    ],
    safety: ToolSafety::WorkspaceMutation,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoErrorKind {
    InvalidArguments,
    MissingDependency,
    ProcessFailed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub(crate) struct VideoError {
    pub(crate) kind: VideoErrorKind,
    pub(crate) message: String,
}

impl VideoError {
    fn new(kind: VideoErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VideoProject {
    #[serde(default = "default_version")]
    version: u32,
    output: String,
    #[serde(default)]
    video: VideoSettings,
    clips: Vec<Clip>,
    #[serde(default)]
    transitions: Vec<Transition>,
    #[serde(default)]
    audio: AudioSettings,
}

fn default_version() -> u32 {
    1
}
fn default_width() -> u32 {
    1920
}
fn default_height() -> u32 {
    1080
}
fn default_fps() -> f64 {
    30.0
}
fn default_true() -> bool {
    true
}
fn default_volume() -> f64 {
    1.0
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct VideoSettings {
    width: u32,
    height: u32,
    fps: f64,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
            fps: default_fps(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Clip {
    path: String,
    #[serde(default)]
    trim: Option<Trim>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Trim {
    #[serde(default)]
    start: Option<f64>,
    #[serde(default)]
    end: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum TransitionType {
    Crossfade,
    Fade,
    WipeLeft,
    WipeRight,
    SlideLeft,
    SlideRight,
}

impl TransitionType {
    fn ffmpeg_name(&self) -> &'static str {
        match self {
            Self::Crossfade | Self::Fade => "fade",
            Self::WipeLeft => "wipeleft",
            Self::WipeRight => "wiperight",
            Self::SlideLeft => "slideleft",
            Self::SlideRight => "slideright",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Transition {
    after_clip: usize,
    #[serde(rename = "type")]
    kind: TransitionType,
    duration: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct AudioSettings {
    music: Option<Music>,
    keep_clip_audio: bool,
    clip_audio_volume: f64,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            music: None,
            keep_clip_audio: default_true(),
            clip_audio_volume: default_volume(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Music {
    path: String,
    #[serde(default = "default_volume")]
    volume: f64,
    #[serde(default)]
    fade_in: f64,
    #[serde(default)]
    fade_out: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct MediaMetadata {
    path: String,
    media_type: String,
    duration_seconds: f64,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<f64>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    audio_streams: usize,
    sample_rate: Option<u32>,
    channels: Option<u32>,
    has_video: bool,
    has_audio: bool,
    file_size_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct ProbeResult {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    #[serde(default)]
    format: ProbeFormat,
}

#[derive(Debug, Default, Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
    duration: Option<String>,
}

#[derive(Debug, Clone)]
struct ProcessOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

trait ProcessRunner {
    fn run(
        &self,
        program: &Path,
        args: &[OsString],
        cancel: Option<&CancellationToken>,
    ) -> Result<ProcessOutput, VideoError>;

    fn run_with_progress(
        &self,
        program: &Path,
        args: &[OsString],
        cancel: Option<&CancellationToken>,
        _progress: Option<&CommandProgressCallback>,
        _total_seconds: Option<f64>,
    ) -> Result<ProcessOutput, VideoError> {
        self.run(program, args, cancel)
    }
}

struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(
        &self,
        program: &Path,
        args: &[OsString],
        cancel: Option<&CancellationToken>,
    ) -> Result<ProcessOutput, VideoError> {
        self.run_internal(program, args, cancel, None, None)
    }

    fn run_with_progress(
        &self,
        program: &Path,
        args: &[OsString],
        cancel: Option<&CancellationToken>,
        progress: Option<&CommandProgressCallback>,
        total_seconds: Option<f64>,
    ) -> Result<ProcessOutput, VideoError> {
        self.run_internal(program, args, cancel, progress, total_seconds)
    }
}

impl SystemProcessRunner {
    fn run_internal(
        &self,
        program: &Path,
        args: &[OsString],
        cancel: Option<&CancellationToken>,
        progress: Option<&CommandProgressCallback>,
        total_seconds: Option<f64>,
    ) -> Result<ProcessOutput, VideoError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|error| {
            VideoError::new(VideoErrorKind::MissingDependency, format!("could not start '{}': {error}. Install FFmpeg and ensure ffmpeg and ffprobe are on PATH", program.display()))
        })?;
        let stdout = child.stdout.take().map(read_bounded);
        let stderr = child.stderr.take().map(|stream| match progress {
            Some(callback) => read_bounded_with_progress(stream, total_seconds, callback.clone()),
            None => read_bounded(stream),
        });
        loop {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                terminate_child(&mut child);
                let _ = stdout.map(|reader| reader.join());
                let _ = stderr.map(|reader| reader.join());
                return Err(VideoError::new(
                    VideoErrorKind::Cancelled,
                    "video operation cancelled; partial output was removed",
                ));
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Ok(ProcessOutput {
                        status,
                        stdout: stdout.and_then(|r| r.join().ok()).unwrap_or_default(),
                        stderr: stderr.and_then(|r| r.join().ok()).unwrap_or_default(),
                    });
                }
                Ok(None) => thread::sleep(Duration::from_millis(40)),
                Err(error) => {
                    terminate_child(&mut child);
                    return Err(VideoError::new(
                        VideoErrorKind::ProcessFailed,
                        format!("could not monitor media process: {error}"),
                    ));
                }
            }
        }
    }
}

fn terminate_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = child.kill();
    let _ = child.wait();
}

fn read_bounded<R: Read + Send + 'static>(mut stream: R) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = VecDeque::with_capacity(MAX_PROCESS_OUTPUT);
        let mut chunk = [0u8; 4096];
        loop {
            let read = match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            let excess = bytes
                .len()
                .saturating_add(read)
                .saturating_sub(MAX_PROCESS_OUTPUT);
            if excess > 0 {
                bytes.drain(..excess);
            }
            bytes.extend(&chunk[..read]);
        }
        let bytes: Vec<_> = bytes.into_iter().collect();
        String::from_utf8_lossy(&bytes).trim().to_string()
    })
}

fn read_bounded_with_progress<R: Read + Send + 'static>(
    mut stream: R,
    total_seconds: Option<f64>,
    callback: CommandProgressCallback,
) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = VecDeque::with_capacity(MAX_PROCESS_OUTPUT);
        let mut parser = FfmpegProgressParser::new(total_seconds);
        let mut chunk = [0u8; 4096];
        loop {
            let read = match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            let excess = bytes
                .len()
                .saturating_add(read)
                .saturating_sub(MAX_PROCESS_OUTPUT);
            if excess > 0 {
                bytes.drain(..excess);
            }
            bytes.extend(&chunk[..read]);
            parser.feed(&chunk[..read], &callback);
        }
        let bytes: Vec<_> = bytes.into_iter().collect();
        String::from_utf8_lossy(&bytes).trim().to_string()
    })
}

struct FfmpegProgressParser {
    pending: Vec<u8>,
    total_seconds: Option<f64>,
    out_time_seconds: f64,
    last_percent: Option<u8>,
}

impl FfmpegProgressParser {
    fn new(total_seconds: Option<f64>) -> Self {
        Self {
            pending: Vec::new(),
            total_seconds,
            out_time_seconds: 0.0,
            last_percent: None,
        }
    }

    fn feed(&mut self, bytes: &[u8], callback: &CommandProgressCallback) {
        self.pending.extend_from_slice(bytes);
        if self.pending.len() > 8192 {
            let excess = self.pending.len() - 8192;
            self.pending.drain(..excess);
        }
        while let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<_> = self.pending.drain(..=newline).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            if let Some(value) = line.strip_prefix("out_time_us=") {
                self.out_time_seconds = value
                    .parse::<f64>()
                    .ok()
                    .map_or(self.out_time_seconds, |value| value / 1_000_000.0);
            } else if let Some(value) = line.strip_prefix("out_time_ms=") {
                self.out_time_seconds = value
                    .parse::<f64>()
                    .ok()
                    .map_or(self.out_time_seconds, |value| value / 1_000_000.0);
            } else if let Some(value) = line.strip_prefix("out_time=")
                && let Some(seconds) = parse_progress_timestamp(value)
            {
                self.out_time_seconds = seconds;
            } else if line == "progress=end" {
                self.emit(100, callback);
            } else if line == "progress=continue" {
                let percent = self
                    .total_seconds
                    .filter(|total| total.is_finite() && *total > 0.0)
                    .map_or(0, |total| {
                        (self.out_time_seconds / total * 100.0)
                            .round()
                            .clamp(0.0, 100.0) as u8
                    });
                self.emit(percent, callback);
            }
        }
    }

    fn emit(&mut self, percent: u8, callback: &CommandProgressCallback) {
        if self.last_percent == Some(percent) {
            return;
        }
        self.last_percent = Some(percent);
        let total = self.total_seconds.unwrap_or_default();
        let message = format!(
            "render progress: {percent}% ({:.1}s/{total:.1}s)\n",
            self.out_time_seconds.max(0.0)
        );
        callback(message.as_bytes(), true);
    }
}

fn parse_progress_timestamp(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<f64>().ok()?;
    let minutes = parts.next()?.parse::<f64>().ok()?;
    let seconds = parts.next()?.parse::<f64>().ok()?;
    (hours >= 0.0 && minutes >= 0.0 && seconds >= 0.0)
        .then_some(hours * 3600.0 + minutes * 60.0 + seconds)
}

fn workspace_root() -> PathBuf {
    super::ACTIVE_WORKSPACE_ROOT
        .with(|root| root.borrow().clone())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn safe_relative(
    raw: &str,
    must_exist: bool,
    create_parent: bool,
) -> Result<(PathBuf, String), VideoError> {
    let value = raw.trim();
    let relative = Path::new(value);
    if value.is_empty()
        || relative.is_absolute()
        || relative.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            "media paths must be non-empty, project-relative, and must not contain '..'",
        ));
    }
    let root = fs::canonicalize(workspace_root()).map_err(|e| {
        VideoError::new(
            VideoErrorKind::InvalidArguments,
            format!("could not resolve project workspace: {e}"),
        )
    })?;
    let absolute = root.join(relative);
    if must_exist && !absolute.is_file() {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            format!("project file '{}' does not exist", value),
        ));
    }
    let parent = absolute
        .parent()
        .ok_or_else(|| VideoError::new(VideoErrorKind::InvalidArguments, "invalid project path"))?;
    let mut existing = parent;
    while !existing.exists() {
        existing = existing.parent().ok_or_else(|| {
            VideoError::new(
                VideoErrorKind::InvalidArguments,
                "could not resolve project directory",
            )
        })?;
    }
    if !fs::canonicalize(existing)
        .map_err(|e| VideoError::new(VideoErrorKind::InvalidArguments, e.to_string()))?
        .starts_with(&root)
    {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            "media path escapes the project workspace through a symbolic link",
        ));
    }
    if create_parent {
        fs::create_dir_all(parent).map_err(|e| {
            VideoError::new(
                VideoErrorKind::InvalidArguments,
                format!("could not create output directory: {e}"),
            )
        })?;
    }
    if absolute.exists()
        && !fs::canonicalize(&absolute)
            .map_err(|e| VideoError::new(VideoErrorKind::InvalidArguments, e.to_string()))?
            .starts_with(&root)
    {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            "media path escapes the project workspace through a symbolic link",
        ));
    }
    Ok((absolute, value.replace('\\', "/")))
}

fn executable_candidates(name: &str, pathext: Option<&str>) -> Vec<OsString> {
    let mut candidates = vec![OsString::from(name)];
    if Path::new(name).extension().is_none() {
        if let Some(pathext) = pathext {
            candidates.extend(pathext.split(';').filter_map(|extension| {
                let extension = extension.trim();
                (!extension.is_empty())
                    .then(|| OsString::from(format!("{name}{}", extension.to_ascii_lowercase())))
            }));
        }
    }
    candidates
}

fn executable(name: &str) -> Option<PathBuf> {
    let pathext = if cfg!(windows) {
        Some(std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()))
    } else {
        None
    };
    std::env::split_paths(&crate::network::augmented_path())
        .flat_map(|dir| {
            executable_candidates(name, pathext.as_deref())
                .into_iter()
                .map(move |candidate| dir.join(candidate))
        })
        .find(|path| path.is_file())
}

fn parse_rate(value: Option<&str>) -> Option<f64> {
    let value = value?;
    let mut parts = value.split('/');
    let numerator = parts.next()?.parse::<f64>().ok()?;
    let denominator = parts
        .next()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(1.0);
    (denominator != 0.0).then_some(numerator / denominator)
}

fn metadata_from_probe(
    path: String,
    file_size_bytes: u64,
    raw: &str,
) -> Result<MediaMetadata, VideoError> {
    let probe: ProbeResult = serde_json::from_str(raw).map_err(|e| {
        VideoError::new(
            VideoErrorKind::ProcessFailed,
            format!("ffprobe returned malformed JSON: {e}"),
        )
    })?;
    let video = probe
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"));
    let audios: Vec<_> = probe
        .streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("audio"))
        .collect();
    let audio = audios.first().copied();
    let duration_seconds = probe
        .format
        .duration
        .as_deref()
        .and_then(|v| v.parse().ok())
        .or_else(|| {
            probe
                .streams
                .iter()
                .filter_map(|s| s.duration.as_deref()?.parse::<f64>().ok())
                .reduce(f64::max)
        })
        .unwrap_or(0.0);
    let has_video = video.is_some();
    let has_audio = audio.is_some();
    Ok(MediaMetadata {
        path,
        media_type: match (has_video, has_audio) {
            (true, true) => "video",
            (true, false) => "video",
            (false, true) => "audio",
            _ => "unknown",
        }
        .into(),
        duration_seconds,
        width: video.and_then(|s| s.width),
        height: video.and_then(|s| s.height),
        fps: video.and_then(|s| {
            parse_rate(s.avg_frame_rate.as_deref())
                .or_else(|| parse_rate(s.r_frame_rate.as_deref()))
        }),
        video_codec: video.and_then(|s| s.codec_name.clone()),
        audio_codec: audio.and_then(|s| s.codec_name.clone()),
        audio_streams: audios.len(),
        sample_rate: audio.and_then(|s| s.sample_rate.as_deref()?.parse().ok()),
        channels: audio.and_then(|s| s.channels),
        has_video,
        has_audio,
        file_size_bytes,
    })
}

fn inspect_path(
    path: &Path,
    relative: String,
    cancel: Option<&CancellationToken>,
    runner: &dyn ProcessRunner,
    ffprobe: &Path,
) -> Result<MediaMetadata, VideoError> {
    let args = vec!["-v".into(), "error".into(), "-show_entries".into(), "format=duration:stream=codec_type,codec_name,width,height,avg_frame_rate,r_frame_rate,sample_rate,channels,duration".into(), "-of".into(), "json".into(), path.as_os_str().to_os_string()];
    let output = runner.run(ffprobe, &args, cancel)?;
    if !output.status.success() {
        return Err(VideoError::new(
            VideoErrorKind::ProcessFailed,
            format!(
                "ffprobe failed for '{}': {}",
                relative,
                process_detail(&output.stderr)
            ),
        ));
    }
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    metadata_from_probe(relative, size, &output.stdout)
}

fn process_detail(stderr: &str) -> String {
    if stderr.is_empty() {
        "no diagnostic output".into()
    } else {
        stderr.to_string()
    }
}

#[derive(Debug, Clone)]
struct Timeline {
    durations: Vec<f64>,
    transitions: HashMap<usize, (TransitionType, f64)>,
    total: f64,
}

fn validate(
    project: &VideoProject,
    metadata: &[MediaMetadata],
) -> Result<(Timeline, Vec<String>), VideoError> {
    if project.version != 1 {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            format!(
                "unsupported video project version {}; expected 1",
                project.version
            ),
        ));
    }
    if project.clips.is_empty() {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            "video project must contain at least one clip",
        ));
    }
    let output_path = normalized_relative(&project.output)?;
    for input in project
        .clips
        .iter()
        .map(|clip| clip.path.as_str())
        .chain(project.audio.music.iter().map(|music| music.path.as_str()))
    {
        if normalized_relative(input)? == output_path {
            return Err(VideoError::new(
                VideoErrorKind::InvalidArguments,
                "video output must not overwrite an input clip or music file",
            ));
        }
    }
    if project.video.width == 0
        || project.video.height == 0
        || project.video.width % 2 != 0
        || project.video.height % 2 != 0
    {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            "video width and height must be positive even numbers for H.264 output",
        ));
    }
    if project.video.width > MAX_VIDEO_WIDTH || project.video.height > MAX_VIDEO_HEIGHT {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            format!("video dimensions must be no more than {MAX_VIDEO_WIDTH}x{MAX_VIDEO_HEIGHT}"),
        ));
    }
    if u64::from(project.video.width) * u64::from(project.video.height) > MAX_VIDEO_PIXELS {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            format!("video resolution must not exceed {MAX_VIDEO_PIXELS} pixels"),
        ));
    }
    if !project.video.fps.is_finite() || project.video.fps <= 0.0 || project.video.fps > 240.0 {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            "video fps must be greater than 0 and no more than 240",
        ));
    }
    if metadata.len() != project.clips.len() || metadata.iter().any(|m| !m.has_video) {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            "every clip must contain a video stream",
        ));
    }
    let mut durations = Vec::new();
    for (index, (clip, media)) in project.clips.iter().zip(metadata).enumerate() {
        let start = clip.trim.as_ref().and_then(|t| t.start).unwrap_or(0.0);
        let end = clip
            .trim
            .as_ref()
            .and_then(|t| t.end)
            .unwrap_or(media.duration_seconds);
        if !start.is_finite()
            || !end.is_finite()
            || start < 0.0
            || end <= start
            || end > media.duration_seconds + 0.001
        {
            return Err(VideoError::new(
                VideoErrorKind::InvalidArguments,
                format!(
                    "clip {index} has an invalid trim range ({start}..{end}) for duration {}",
                    media.duration_seconds
                ),
            ));
        }
        durations.push(end - start);
    }
    let mut transitions = HashMap::new();
    for transition in &project.transitions {
        if transition.after_clip >= project.clips.len().saturating_sub(1) {
            return Err(VideoError::new(
                VideoErrorKind::InvalidArguments,
                format!(
                    "transition after_clip {} has no following clip",
                    transition.after_clip
                ),
            ));
        }
        if transitions.contains_key(&transition.after_clip) {
            return Err(VideoError::new(
                VideoErrorKind::InvalidArguments,
                format!(
                    "multiple transitions target boundary {}",
                    transition.after_clip
                ),
            ));
        }
        if !transition.duration.is_finite()
            || transition.duration <= 0.0
            || transition.duration
                >= durations[transition.after_clip].min(durations[transition.after_clip + 1])
        {
            return Err(VideoError::new(
                VideoErrorKind::InvalidArguments,
                format!(
                    "transition after clip {} must be positive and shorter than both adjacent clips",
                    transition.after_clip
                ),
            ));
        }
        transitions.insert(
            transition.after_clip,
            (transition.kind.clone(), transition.duration),
        );
    }
    for (name, volume) in [
        ("clip_audio_volume", project.audio.clip_audio_volume),
        (
            "music volume",
            project
                .audio
                .music
                .as_ref()
                .map(|m| m.volume)
                .unwrap_or(1.0),
        ),
    ] {
        if !volume.is_finite() || volume < 0.0 {
            return Err(VideoError::new(
                VideoErrorKind::InvalidArguments,
                format!("{name} must be a finite non-negative number"),
            ));
        }
    }
    if let Some(music) = &project.audio.music {
        if !music.fade_in.is_finite()
            || !music.fade_out.is_finite()
            || music.fade_in < 0.0
            || music.fade_out < 0.0
        {
            return Err(VideoError::new(
                VideoErrorKind::InvalidArguments,
                "music fades must be finite non-negative numbers",
            ));
        }
    }
    let total = durations.iter().sum::<f64>()
        - transitions
            .values()
            .map(|(_, duration)| duration)
            .sum::<f64>();
    if !total.is_finite() || total <= 0.0 || total > MAX_VIDEO_DURATION_SECONDS {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            format!(
                "video timeline duration must be greater than 0 and no more than {MAX_VIDEO_DURATION_SECONDS:.0} seconds"
            ),
        ));
    }
    let mut warnings = Vec::new();
    if project.audio.keep_clip_audio && metadata.iter().any(|m| !m.has_audio) {
        warnings.push(
            "one or more clips have no audio; silence will be inserted for those clips".into(),
        );
    }
    Ok((
        Timeline {
            durations,
            transitions,
            total,
        },
        warnings,
    ))
}

fn normalized_relative(raw: &str) -> Result<PathBuf, VideoError> {
    let path = Path::new(raw.trim());
    if raw.trim().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            "media paths must be non-empty, project-relative, and must not contain '..'",
        ));
    }
    Ok(path
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect())
}

fn load_project(args: &Value) -> Result<(VideoProject, PathBuf), VideoError> {
    let raw = args
        .get("project_path")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            VideoError::new(VideoErrorKind::InvalidArguments, "missing 'project_path'")
        })?;
    if Path::new(raw)
        .extension()
        .and_then(|v| v.to_str())
        .is_none_or(|v| !v.eq_ignore_ascii_case("json"))
    {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            "video project path must use the .json extension",
        ));
    }
    let (path, _) = safe_relative(raw, true, false)?;
    let bytes = fs::read(&path).map_err(|e| {
        VideoError::new(
            VideoErrorKind::InvalidArguments,
            format!("could not read video project: {e}"),
        )
    })?;
    let project = serde_json::from_slice(&bytes).map_err(|e| {
        VideoError::new(
            VideoErrorKind::InvalidArguments,
            format!("invalid video project JSON: {e}"),
        )
    })?;
    Ok((project, path))
}

/// Build a lightweight confirmation preview without probing media or creating
/// output directories. Media durations are shown only when every clip has an
/// explicit trim range.
pub(crate) fn render_confirmation_preview(
    args: &Value,
    root_override: Option<&Path>,
) -> Option<String> {
    let raw_path = args.get("project_path").and_then(Value::as_str)?;
    let relative_path = normalized_relative(raw_path).ok()?;
    let root = root_override
        .map(Path::to_path_buf)
        .unwrap_or_else(workspace_root);
    let project_path = root.join(relative_path);
    let bytes = fs::read(&project_path).ok()?;
    let project: VideoProject = serde_json::from_slice(&bytes).ok()?;
    let output = normalized_relative(&project.output).ok()?;
    let duration = requested_duration(&project)
        .map(|duration| format!("{duration:.1}s"))
        .unwrap_or_else(|| "unavailable until media validation".into());

    Some(format!(
        "Project: {}\nOutput: {}\nClips: {}\nResolution: {}x{} @ {} FPS\nDuration: {duration}",
        project_path.display(),
        root.join(output).display(),
        project.clips.len(),
        project.video.width,
        project.video.height,
        number(project.video.fps),
    ))
}

fn requested_duration(project: &VideoProject) -> Option<f64> {
    let durations = project.clips.iter().map(|clip| {
        let trim = clip.trim.as_ref()?;
        let start = trim.start.unwrap_or(0.0);
        let end = trim.end?;
        (start.is_finite() && end.is_finite() && end > start).then_some(end - start)
    });
    let durations: Vec<f64> = durations.collect::<Option<_>>()?;
    let transition_duration = project
        .transitions
        .iter()
        .map(|transition| transition.duration)
        .sum::<f64>();
    let total = durations.into_iter().sum::<f64>() - transition_duration;
    (total.is_finite() && total > 0.0).then_some(total)
}

fn inspect_inputs(
    project: &VideoProject,
    cancel: Option<&CancellationToken>,
    runner: &dyn ProcessRunner,
    ffprobe: &Path,
) -> Result<Vec<MediaMetadata>, VideoError> {
    project
        .clips
        .iter()
        .map(|clip| {
            let (path, relative) = safe_relative(&clip.path, true, false)?;
            inspect_path(&path, relative, cancel, runner, ffprobe)
        })
        .collect()
}

fn validate_media_paths(project: &VideoProject) -> Result<(), VideoError> {
    if let Some(music) = &project.audio.music {
        safe_relative(&music.path, true, false).map_err(|error| {
            VideoError::new(
                error.kind,
                format!("background music file is invalid: {}", error.message),
            )
        })?;
    }
    Ok(())
}

fn inspect_project_media(
    project: &VideoProject,
    cancel: Option<&CancellationToken>,
    runner: &dyn ProcessRunner,
    ffprobe: &Path,
) -> Result<Vec<MediaMetadata>, VideoError> {
    let metadata = inspect_inputs(project, cancel, runner, ffprobe)?;
    if let Some(music) = &project.audio.music {
        let (path, relative) = safe_relative(&music.path, true, false)?;
        let music_meta = inspect_path(&path, relative, cancel, runner, ffprobe)?;
        if !music_meta.has_audio {
            return Err(VideoError::new(
                VideoErrorKind::InvalidArguments,
                "background music file has no audio stream",
            ));
        }
    }
    Ok(metadata)
}

fn number(value: f64) -> String {
    format!("{value:.6}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn compile_ffmpeg(
    project: &VideoProject,
    timeline: &Timeline,
    metadata: &[MediaMetadata],
    output: &Path,
) -> Result<Vec<OsString>, VideoError> {
    let mut args: Vec<OsString> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:2".into(),
    ];
    for clip in &project.clips {
        let (path, _) = safe_relative(&clip.path, true, false)?;
        args.extend(["-i".into(), path.into_os_string()]);
    }
    if let Some(music) = &project.audio.music {
        let (path, _) = safe_relative(&music.path, true, false)?;
        args.extend([
            "-stream_loop".into(),
            "-1".into(),
            "-i".into(),
            path.into_os_string(),
        ]);
    }
    let mut filters = Vec::new();
    for (index, (clip, duration)) in project.clips.iter().zip(&timeline.durations).enumerate() {
        let start = clip.trim.as_ref().and_then(|t| t.start).unwrap_or(0.0);
        filters.push(format!("[{index}:v]trim=start={}:duration={},setpts=PTS-STARTPTS,scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={},format=yuv420p[v{index}]", number(start), number(*duration), project.video.width, project.video.height, project.video.width, project.video.height, number(project.video.fps)));
        if project.audio.keep_clip_audio {
            if metadata[index].has_audio {
                filters.push(format!("[{index}:a]atrim=start={}:duration={},asetpts=PTS-STARTPTS,aresample=48000,aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo,volume={},apad,atrim=duration={}[a{index}]", number(start), number(*duration), number(project.audio.clip_audio_volume), number(*duration)));
            } else {
                filters.push(format!(
                    "anullsrc=r=48000:cl=stereo,atrim=duration={}[a{index}]",
                    number(*duration)
                ));
            }
        }
    }
    let mut video_label = "v0".to_string();
    let mut audio_label = "a0".to_string();
    let mut current_duration = timeline.durations[0];
    for index in 0..project.clips.len() - 1 {
        let next_video = format!("v{}", index + 1);
        let out_video = format!("vc{}", index + 1);
        if let Some((kind, duration)) = timeline.transitions.get(&index) {
            let offset = current_duration - duration;
            filters.push(format!("[{video_label}][{next_video}]xfade=transition={}:duration={}:offset={}[{out_video}]", kind.ffmpeg_name(), number(*duration), number(offset)));
            if project.audio.keep_clip_audio {
                let next_audio = format!("a{}", index + 1);
                let out_audio = format!("ac{}", index + 1);
                filters.push(format!(
                    "[{audio_label}][{next_audio}]acrossfade=d={}:c1=tri:c2=tri[{out_audio}]",
                    number(*duration)
                ));
                audio_label = out_audio;
            }
            current_duration += timeline.durations[index + 1] - duration;
        } else {
            filters.push(format!(
                "[{video_label}][{next_video}]concat=n=2:v=1:a=0[{out_video}]"
            ));
            if project.audio.keep_clip_audio {
                let next_audio = format!("a{}", index + 1);
                let out_audio = format!("ac{}", index + 1);
                filters.push(format!(
                    "[{audio_label}][{next_audio}]concat=n=2:v=0:a=1[{out_audio}]"
                ));
                audio_label = out_audio;
            }
            current_duration += timeline.durations[index + 1];
        }
        video_label = out_video;
    }
    filters.push(format!(
        "[{video_label}]trim=duration={},setpts=PTS-STARTPTS[vout]",
        number(timeline.total)
    ));
    let mut audio_output = None;
    if let Some(music) = &project.audio.music {
        let input = project.clips.len();
        let mut chain = format!(
            "[{input}:a]atrim=duration={},asetpts=PTS-STARTPTS,aresample=48000,aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo,volume={}",
            number(timeline.total),
            number(music.volume)
        );
        if music.fade_in > 0.0 {
            chain.push_str(&format!(
                ",afade=t=in:st=0:d={}",
                number(music.fade_in.min(timeline.total))
            ));
        }
        if music.fade_out > 0.0 {
            let duration = music.fade_out.min(timeline.total);
            chain.push_str(&format!(
                ",afade=t=out:st={}:d={}",
                number(timeline.total - duration),
                number(duration)
            ));
        }
        chain.push_str("[music]");
        filters.push(chain);
        if project.audio.keep_clip_audio {
            filters.push(format!("[{audio_label}][music]amix=inputs=2:duration=first:dropout_transition=0,atrim=duration={}[aout]", number(timeline.total)));
        } else {
            filters.push(format!(
                "[music]atrim=duration={}[aout]",
                number(timeline.total)
            ));
        }
        audio_output = Some("aout");
    } else if project.audio.keep_clip_audio {
        filters.push(format!(
            "[{audio_label}]atrim=duration={}[aout]",
            number(timeline.total)
        ));
        audio_output = Some("aout");
    }
    args.extend([
        "-filter_complex".into(),
        filters.join(";").into(),
        "-map".into(),
        "[vout]".into(),
    ]);
    if let Some(label) = audio_output {
        args.extend(["-map".into(), format!("[{label}]").into()]);
    }
    args.extend([
        "-c:v".into(),
        "libx264".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-r".into(),
        number(project.video.fps).into(),
        "-movflags".into(),
        "+faststart".into(),
    ]);
    if audio_output.is_some() {
        args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "192k".into()]);
    }
    args.extend([
        "-t".into(),
        number(timeline.total).into(),
        output.as_os_str().to_os_string(),
    ]);
    Ok(args)
}

fn temp_output(output: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    output.with_file_name(format!(
        ".{}.rustcode-{stamp}-{}.tmp.mp4",
        output
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("video.mp4"),
        std::process::id()
    ))
}

fn inspect_media(
    args: &Value,
    cancel: Option<&CancellationToken>,
    runner: &dyn ProcessRunner,
) -> Result<String, VideoError> {
    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| VideoError::new(VideoErrorKind::InvalidArguments, "missing 'path'"))?;
    let (path, relative) = safe_relative(raw, true, false)?;
    let ffprobe = executable("ffprobe").ok_or_else(|| {
        VideoError::new(
            VideoErrorKind::MissingDependency,
            "ffprobe is unavailable; install FFmpeg and ensure ffprobe is on PATH",
        )
    })?;
    let metadata = inspect_path(&path, relative, cancel, runner, &ffprobe)?;
    serde_json::to_string(&metadata)
        .map_err(|e| VideoError::new(VideoErrorKind::ProcessFailed, e.to_string()))
}

fn validate_project(
    args: &Value,
    cancel: Option<&CancellationToken>,
    runner: &dyn ProcessRunner,
) -> Result<String, VideoError> {
    let (project, _) = load_project(args)?;
    validate_media_paths(&project)?;
    let ffprobe = executable("ffprobe").ok_or_else(|| {
        VideoError::new(
            VideoErrorKind::MissingDependency,
            "ffprobe is unavailable; install FFmpeg and ensure ffprobe is on PATH",
        )
    })?;
    let metadata = inspect_project_media(&project, cancel, runner, &ffprobe)?;
    let (timeline, warnings) = validate(&project, &metadata)?;
    Ok(serde_json::json!({"valid":true,"duration_seconds":timeline.total,"resolution":{"width":project.video.width,"height":project.video.height},"fps":project.video.fps,"warnings":warnings}).to_string())
}

fn render_project(
    args: &Value,
    cancel: Option<&CancellationToken>,
    runner: &dyn ProcessRunner,
    progress: Option<&CommandProgressCallback>,
) -> Result<String, VideoError> {
    let (project, _) = load_project(args)?;
    validate_media_paths(&project)?;
    if Path::new(&project.output)
        .extension()
        .and_then(|v| v.to_str())
        .is_none_or(|v| !v.eq_ignore_ascii_case("mp4"))
    {
        return Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            "video output must use the .mp4 extension",
        ));
    }
    let ffprobe = executable("ffprobe").ok_or_else(|| {
        VideoError::new(
            VideoErrorKind::MissingDependency,
            "ffprobe is unavailable; install FFmpeg and ensure ffprobe is on PATH",
        )
    })?;
    let ffmpeg = executable("ffmpeg").ok_or_else(|| {
        VideoError::new(
            VideoErrorKind::MissingDependency,
            "ffmpeg is unavailable; install FFmpeg and ensure ffmpeg is on PATH",
        )
    })?;
    let metadata = inspect_project_media(&project, cancel, runner, &ffprobe)?;
    let (timeline, warnings) = validate(&project, &metadata)?;
    let (output, relative) = safe_relative(&project.output, false, true)?;
    let temporary = temp_output(&output);
    let command = compile_ffmpeg(&project, &timeline, &metadata, &temporary)?;
    let result =
        runner.run_with_progress(&ffmpeg, &command, cancel, progress, Some(timeline.total));
    let process = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = fs::remove_file(&temporary);
            return Err(e);
        }
    };
    if !process.status.success() {
        let _ = fs::remove_file(&temporary);
        return Err(VideoError::new(
            VideoErrorKind::ProcessFailed,
            format!("FFmpeg render failed: {}", process_detail(&process.stderr)),
        ));
    }
    let rendered =
        inspect_path(&temporary, relative.clone(), cancel, runner, &ffprobe).map_err(|e| {
            let _ = fs::remove_file(&temporary);
            e
        })?;
    crate::atomic_file::replace_file(&temporary, &output).map_err(|e| {
        let _ = fs::remove_file(&temporary);
        VideoError::new(
            VideoErrorKind::ProcessFailed,
            format!("could not finalize rendered video: {e}"),
        )
    })?;
    Ok(serde_json::json!({"status":"rendered","output_path":relative,"duration_seconds":rendered.duration_seconds,"resolution":{"width":rendered.width,"height":rendered.height},"file_size_bytes":rendered.file_size_bytes,"warnings":warnings}).to_string())
}

pub(crate) fn execute_with_cancel(
    name: &str,
    args: &Value,
    cancel: Option<CancellationToken>,
) -> Result<String, VideoError> {
    execute_with_cancel_and_progress(name, args, cancel, None)
}

pub(crate) fn execute_with_cancel_and_progress(
    name: &str,
    args: &Value,
    cancel: Option<CancellationToken>,
    progress: Option<CommandProgressCallback>,
) -> Result<String, VideoError> {
    match name {
        "inspect_media" => inspect_media(args, cancel.as_ref(), &SystemProcessRunner),
        "validate_video_project" => validate_project(args, cancel.as_ref(), &SystemProcessRunner),
        "render_video" => render_project(
            args,
            cancel.as_ref(),
            &SystemProcessRunner,
            progress.as_ref(),
        ),
        _ => Err(VideoError::new(
            VideoErrorKind::InvalidArguments,
            "unknown video tool",
        )),
    }
}

fn inspect_media_handler(args: &Value) -> Result<String, String> {
    execute_with_cancel("inspect_media", args, None).map_err(|e| e.message)
}
fn validate_handler(args: &Value) -> Result<String, String> {
    execute_with_cancel("validate_video_project", args, None).map_err(|e| e.message)
}
fn render_handler(args: &Value) -> Result<String, String> {
    execute_with_cancel("render_video", args, None).map_err(|e| e.message)
}

pub(crate) fn map_error_kind(kind: VideoErrorKind) -> super::ToolErrorKind {
    match kind {
        VideoErrorKind::InvalidArguments => super::ToolErrorKind::InvalidArguments,
        VideoErrorKind::MissingDependency => super::ToolErrorKind::UnavailableDependency,
        VideoErrorKind::ProcessFailed => super::ToolErrorKind::CommandFailed,
        VideoErrorKind::Cancelled => super::ToolErrorKind::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;

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

    struct ProbeRunner;

    impl ProcessRunner for ProbeRunner {
        fn run(
            &self,
            _program: &Path,
            args: &[OsString],
            _cancel: Option<&CancellationToken>,
        ) -> Result<ProcessOutput, VideoError> {
            let path = args.last().unwrap().to_string_lossy();
            let stdout = if path.ends_with("music.wav") {
                r#"{"streams":[{"codec_type":"video"}],"format":{"duration":"1"}}"#
            } else {
                r#"{"streams":[{"codec_type":"video"},{"codec_type":"audio"}],"format":{"duration":"1"}}"#
            };
            Ok(ProcessOutput {
                status: exit_status(0),
                stdout: stdout.into(),
                stderr: String::new(),
            })
        }
    }

    fn metadata(duration: f64, audio: bool) -> MediaMetadata {
        MediaMetadata {
            path: "clip.mp4".into(),
            media_type: "video".into(),
            duration_seconds: duration,
            width: Some(1280),
            height: Some(720),
            fps: Some(30.0),
            video_codec: Some("h264".into()),
            audio_codec: audio.then(|| "aac".into()),
            audio_streams: usize::from(audio),
            sample_rate: audio.then_some(48000),
            channels: audio.then_some(2),
            has_video: true,
            has_audio: audio,
            file_size_bytes: 1,
        }
    }

    fn project() -> VideoProject {
        serde_json::from_value(serde_json::json!({"output":"output/final.mp4","clips":[{"path":"media/a.mp4"},{"path":"media/b.mp4"},{"path":"media/c.mp4"}]})).unwrap()
    }

    #[test]
    fn confirmation_preview_reports_resolved_render_details() {
        let dir = tempfile::TempDir::new().unwrap();
        let project_path = dir.path().join("video-project.json");
        fs::write(
            &project_path,
            serde_json::json!({
                "output":"output/final.mp4",
                "video":{"width":1280,"height":720,"fps":24},
                "clips":[
                    {"path":"media/a.mp4","trim":{"start":1,"end":5}},
                    {"path":"media/b.mp4","trim":{"start":0,"end":3}}
                ]
            })
            .to_string(),
        )
        .unwrap();

        let preview = render_confirmation_preview(
            &serde_json::json!({"project_path":"video-project.json"}),
            Some(dir.path()),
        )
        .unwrap();

        assert!(preview.contains("Project: "));
        assert!(preview.contains("Output: "));
        assert!(preview.contains("Clips: 2"));
        assert!(preview.contains("Resolution: 1280x720 @ 24 FPS"));
        assert!(preview.contains("Duration: 7.0s"));
    }

    #[test]
    fn validation_rejects_excessive_dimensions_and_duration() {
        let mut value = project();
        value.video.width = MAX_VIDEO_WIDTH + 2;
        let error = validate(
            &value,
            &[
                metadata(10.0, true),
                metadata(10.0, true),
                metadata(10.0, true),
            ],
        )
        .unwrap_err();
        assert!(error.message.contains("dimensions"));

        let mut value = project();
        value.video.width = MAX_VIDEO_WIDTH;
        value.video.height = MAX_VIDEO_HEIGHT;
        let error = validate(
            &value,
            &[
                metadata(10.0, true),
                metadata(10.0, true),
                metadata(10.0, true),
            ],
        )
        .unwrap_err();
        assert!(error.message.contains("pixels"));

        let value = project();
        let error = validate(
            &value,
            &[
                metadata(MAX_VIDEO_DURATION_SECONDS + 1.0, true),
                metadata(10.0, true),
                metadata(10.0, true),
            ],
        )
        .unwrap_err();
        assert!(error.message.contains("timeline duration"));
    }

    #[test]
    fn process_output_capture_is_bounded_and_keeps_the_tail() {
        let mut input = b"HEAD_MARKER".to_vec();
        input.extend(std::iter::repeat_n(b'x', MAX_PROCESS_OUTPUT * 4));
        input.extend_from_slice(b"TAIL_MARKER");

        let output = read_bounded(std::io::Cursor::new(input)).join().unwrap();

        assert!(output.len() <= MAX_PROCESS_OUTPUT);
        assert!(output.ends_with("TAIL_MARKER"));
    }

    #[test]
    fn process_output_capture_keeps_small_output_intact() {
        let output = read_bounded(std::io::Cursor::new(br#"{"streams":[]}"#.to_vec()))
            .join()
            .unwrap();

        assert_eq!(output, r#"{"streams":[]}"#);
    }

    #[cfg(unix)]
    #[test]
    fn process_runner_drains_large_output_before_waiting() {
        let output = SystemProcessRunner
            .run(
                Path::new("/bin/sh"),
                &["-c".into(), "head -c 200000 /dev/zero".into()],
                None,
            )
            .unwrap();

        assert!(output.status.success());
        assert!(output.stdout.len() <= MAX_PROCESS_OUTPUT);
    }

    #[test]
    fn ffmpeg_progress_parser_emits_stable_percentage_updates() {
        let messages = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = messages.clone();
        let callback: CommandProgressCallback = std::sync::Arc::new(move |bytes, _| {
            captured
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(bytes).into_owned());
        });
        let mut parser = FfmpegProgressParser::new(Some(2.0));
        parser.feed(b"out_time_us=500000\nprogress=cont", &callback);
        parser.feed(b"inue\nout_time_us=500000\nprogress=continue\n", &callback);
        parser.feed(
            b"out_time=00:00:01.500\nprogress=continue\nprogress=end\n",
            &callback,
        );

        let messages = messages.lock().unwrap();
        assert_eq!(messages.len(), 3);
        assert!(messages[0].contains("25%"));
        assert!(messages[1].contains("75%"));
        assert!(messages[2].contains("100%"));
    }

    #[test]
    fn defaults_are_stable() {
        let value = project();
        assert_eq!(value.version, 1);
        assert_eq!(value.video.width, 1920);
        assert_eq!(value.video.height, 1080);
        assert_eq!(value.video.fps, 30.0);
        assert!(value.audio.keep_clip_audio);
        assert_eq!(value.audio.clip_audio_volume, 1.0);
    }

    #[test]
    fn transition_overlap_reduces_timeline() {
        let mut value = project();
        value.transitions = vec![
            Transition {
                after_clip: 0,
                kind: TransitionType::Crossfade,
                duration: 0.5,
            },
            Transition {
                after_clip: 1,
                kind: TransitionType::Fade,
                duration: 0.5,
            },
        ];
        let (timeline, _) = validate(
            &value,
            &[
                metadata(10.0, true),
                metadata(10.0, true),
                metadata(10.0, true),
            ],
        )
        .unwrap();
        assert_eq!(timeline.total, 29.0);
    }

    #[test]
    fn rejects_invalid_trim_transition_and_index() {
        let mut value = project();
        value.clips[0].trim = Some(Trim {
            start: Some(8.0),
            end: Some(2.0),
        });
        assert!(
            validate(
                &value,
                &[
                    metadata(10.0, true),
                    metadata(10.0, true),
                    metadata(10.0, true)
                ]
            )
            .is_err()
        );
        let mut value = project();
        value.transitions.push(Transition {
            after_clip: 2,
            kind: TransitionType::Crossfade,
            duration: 0.5,
        });
        assert!(
            validate(
                &value,
                &[
                    metadata(10.0, true),
                    metadata(10.0, true),
                    metadata(10.0, true)
                ]
            )
            .is_err()
        );
        let mut value = project();
        value.transitions.push(Transition {
            after_clip: 0,
            kind: TransitionType::Crossfade,
            duration: 10.0,
        });
        assert!(
            validate(
                &value,
                &[
                    metadata(10.0, true),
                    metadata(10.0, true),
                    metadata(10.0, true)
                ]
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_output_that_overwrites_an_input() {
        let mut value = project();
        value.output = "media/./a.mp4".into();
        let error = validate(
            &value,
            &[
                metadata(10.0, true),
                metadata(10.0, true),
                metadata(10.0, true),
            ],
        )
        .unwrap_err();
        assert!(error.message.contains("must not overwrite"));
    }

    #[test]
    fn parses_concise_probe_metadata() {
        let raw = r#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"30000/1001"},{"codec_type":"audio","codec_name":"aac","sample_rate":"48000","channels":2}],"format":{"duration":"12.5"}}"#;
        let value = metadata_from_probe("media/a.mp4".into(), 42, raw).unwrap();
        assert!(value.has_video);
        assert!(value.has_audio);
        assert_eq!(value.duration_seconds, 12.5);
        assert!((value.fps.unwrap() - 29.970).abs() < 0.001);
        assert_eq!(value.audio_streams, 1);
    }

    #[test]
    fn ffmpeg_graph_has_offsets_normalization_and_music_fades() {
        let mut value = project();
        value.transitions.push(Transition {
            after_clip: 0,
            kind: TransitionType::WipeLeft,
            duration: 0.5,
        });
        value.audio.music = Some(Music {
            path: "media/music.wav".into(),
            volume: 0.2,
            fade_in: 1.0,
            fade_out: 2.0,
        });
        let media = [
            metadata(10.0, true),
            metadata(10.0, false),
            metadata(10.0, true),
        ];
        let (timeline, _) = validate(&value, &media).unwrap();
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("media")).unwrap();
        for name in ["a.mp4", "b.mp4", "c.mp4", "music.wav"] {
            fs::write(dir.path().join("media").join(name), b"x").unwrap();
        }
        super::super::set_active_workspace_root(Some(dir.path().to_path_buf()));
        let args = compile_ffmpeg(&value, &timeline, &media, &dir.path().join("out.mp4")).unwrap();
        let joined = args
            .iter()
            .map(|v| v.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("xfade=transition=wipeleft:duration=0.5:offset=9.5"));
        assert!(joined.contains("scale=1920:1080"));
        assert!(joined.contains("anullsrc"));
        assert!(joined.contains("volume=1,apad,atrim=duration=10[a0]"));
        assert!(joined.contains("volume=0.2"));
        assert!(joined.contains("afade=t=out:st=27.5:d=2"));
        super::super::set_active_workspace_root(None);
    }

    #[test]
    fn project_paths_reject_traversal() {
        let dir = tempfile::TempDir::new().unwrap();
        super::super::set_active_workspace_root(Some(dir.path().to_path_buf()));
        assert!(safe_relative("../outside.mp4", false, false).is_err());
        assert!(safe_relative("media/../../outside.mp4", false, false).is_err());
        super::super::set_active_workspace_root(None);
    }

    #[test]
    fn executable_candidates_include_windows_extensions() {
        let candidates = executable_candidates("ffmpeg", Some(".COM;.EXE;.BAT"));
        assert_eq!(
            candidates,
            vec![
                OsString::from("ffmpeg"),
                OsString::from("ffmpeg.com"),
                OsString::from("ffmpeg.exe"),
                OsString::from("ffmpeg.bat"),
            ]
        );
    }

    #[test]
    fn validation_rejects_missing_music_file_before_dependency_lookup() {
        let dir = tempfile::TempDir::new().unwrap();
        super::super::set_active_workspace_root(Some(dir.path().to_path_buf()));
        let mut value = project();
        value.audio.music = Some(Music {
            path: "media/missing.wav".into(),
            volume: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
        });
        fs::write(
            dir.path().join("video-project.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        let error = validate_project(
            &serde_json::json!({"project_path":"video-project.json"}),
            None,
            &SystemProcessRunner,
        )
        .unwrap_err();
        super::super::set_active_workspace_root(None);
        assert!(error.message.contains("music"));
    }

    #[test]
    fn project_media_preflight_rejects_music_without_audio() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("media")).unwrap();
        for name in ["a.mp4", "b.mp4", "c.mp4", "music.wav"] {
            fs::write(dir.path().join("media").join(name), b"media").unwrap();
        }
        let mut value = project();
        value.audio.music = Some(Music {
            path: "media/music.wav".into(),
            volume: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
        });
        super::super::set_active_workspace_root(Some(dir.path().to_path_buf()));
        let error =
            inspect_project_media(&value, None, &ProbeRunner, Path::new("ffprobe")).unwrap_err();
        super::super::set_active_workspace_root(None);
        assert_eq!(error.kind, VideoErrorKind::InvalidArguments);
        assert_eq!(error.message, "background music file has no audio stream");
    }

    #[test]
    fn malformed_and_unknown_json_are_rejected() {
        assert!(serde_json::from_str::<VideoProject>("{").is_err());
        assert!(
            serde_json::from_value::<VideoProject>(
                serde_json::json!({"output":"x.mp4","clips":[],"raw_ffmpeg":"-i x"})
            )
            .is_err()
        );
    }

    #[test]
    fn missing_dependency_has_typed_error() {
        let error = SystemProcessRunner
            .run(Path::new("/definitely/missing/ffprobe"), &[], None)
            .unwrap_err();
        assert_eq!(error.kind, VideoErrorKind::MissingDependency);
        assert!(error.message.contains("Install FFmpeg"));
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_kills_external_process() {
        use std::sync::mpsc;
        let token = CancellationToken::new();
        let worker_token = token.clone();
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            tx.send(SystemProcessRunner.run(
                Path::new("/bin/sh"),
                &["-c".into(), "sleep 10".into()],
                Some(&worker_token),
            ))
            .unwrap();
        });
        thread::sleep(Duration::from_millis(100));
        token.cancel();
        let result = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        assert_eq!(result.unwrap_err().kind, VideoErrorKind::Cancelled);
    }

    #[test]
    fn renders_tiny_crossfade_when_ffmpeg_is_available() {
        let require_ffmpeg = std::env::var_os("RUSTCODE_REQUIRE_FFMPEG_TESTS").is_some();
        let Some(ffmpeg) = executable("ffmpeg") else {
            assert!(
                !require_ffmpeg,
                "ffmpeg is required for this integration test"
            );
            return;
        };
        let Some(_ffprobe) = executable("ffprobe") else {
            assert!(
                !require_ffmpeg,
                "ffprobe is required for this integration test"
            );
            return;
        };
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("media")).unwrap();
        for (name, color) in [("a.mp4", "red"), ("b.mp4", "blue")] {
            let status = Command::new(&ffmpeg)
                .args([
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("color=c={color}:s=64x64:d=0.5:r=10"),
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                ])
                .arg(dir.path().join("media").join(name))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success());
        }
        fs::write(
            dir.path().join("video-project.json"),
            serde_json::to_vec(&serde_json::json!({
                "output":"output/final.mp4",
                "video":{"width":64,"height":64,"fps":10},
                "clips":[{"path":"media/a.mp4"},{"path":"media/b.mp4"}],
                "transitions":[{"after_clip":0,"type":"crossfade","duration":0.1}],
                "audio":{"keep_clip_audio":false}
            }))
            .unwrap(),
        )
        .unwrap();
        super::super::set_active_workspace_root(Some(dir.path().to_path_buf()));
        let result = render_project(
            &serde_json::json!({"project_path":"video-project.json"}),
            None,
            &SystemProcessRunner,
            None,
        )
        .unwrap();
        super::super::set_active_workspace_root(None);
        let result: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(result["status"], "rendered");
        assert_eq!(result["resolution"]["width"], 64);
        assert!(dir.path().join("output/final.mp4").is_file());
    }

    #[test]
    fn renders_short_clip_audio_to_full_timeline_when_ffmpeg_is_available() {
        let require_ffmpeg = std::env::var_os("RUSTCODE_REQUIRE_FFMPEG_TESTS").is_some();
        let Some(ffmpeg) = executable("ffmpeg") else {
            assert!(
                !require_ffmpeg,
                "ffmpeg is required for this integration test"
            );
            return;
        };
        let Some(ffprobe) = executable("ffprobe") else {
            assert!(
                !require_ffmpeg,
                "ffprobe is required for this integration test"
            );
            return;
        };
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("media")).unwrap();
        for (name, color) in [("a.mp4", "red"), ("b.mp4", "blue")] {
            let status = Command::new(&ffmpeg)
                .args([
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("color=c={color}:s=64x64:d=1:r=10"),
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=0.2",
                    "-map",
                    "0:v",
                    "-map",
                    "1:a",
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "aac",
                    "-t",
                    "1",
                ])
                .arg(dir.path().join("media").join(name))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success());
        }
        fs::write(
            dir.path().join("video-project.json"),
            serde_json::to_vec(&serde_json::json!({
                "output":"output/final.mp4",
                "video":{"width":64,"height":64,"fps":10},
                "clips":[{"path":"media/a.mp4"},{"path":"media/b.mp4"}]
            }))
            .unwrap(),
        )
        .unwrap();
        let output = dir.path().join("output/final.mp4");
        super::super::set_active_workspace_root(Some(dir.path().to_path_buf()));
        render_project(
            &serde_json::json!({"project_path":"video-project.json"}),
            None,
            &SystemProcessRunner,
            None,
        )
        .unwrap();
        super::super::set_active_workspace_root(None);

        let probe = Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "a:0",
                "-show_entries",
                "stream=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(output)
            .output()
            .unwrap();
        assert!(probe.status.success());
        let duration: f64 = String::from_utf8_lossy(&probe.stdout)
            .trim()
            .parse()
            .unwrap();
        assert!(duration >= 1.8, "audio duration was only {duration}s");
    }
}
