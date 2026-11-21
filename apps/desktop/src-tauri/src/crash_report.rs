//! Crash and error reporting to Sentry via the Envelope v7 HTTP API.
//!
//! Enabled by default (opt-out in Settings › General › Send crash reports,
//! disclosed once on first launch). Nothing is sent unless a DSN is baked in
//! at build time (`RUSTCODE_SENTRY_DSN`). Every payload is scrubbed for home
//! paths, prompt text, secrets and environment values before leaving the app,
//! and client-side deduplication and rate limiting guard the monthly quota.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Crash reporting is on by default; users may opt out without restarting.
pub const DEFAULT_ENABLED: bool = true;

const CONFIG_FILE: &str = "crash-reporting.json";
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);
const PANIC_TIMEOUT: Duration = Duration::from_secs(2);
const DEDUPE_WINDOW: Duration = Duration::from_secs(60);
const RATE_WINDOW: Duration = Duration::from_secs(300);
const RATE_MAX_EVENTS: usize = 10;
const MAX_TYPE_CHARS: usize = 128;
const MAX_MESSAGE_CHARS: usize = 512;
const MAX_STACK_CHARS: usize = 8192;
const MAX_STACK_FRAMES: usize = 64;

static GLOBAL: OnceLock<Arc<CrashReporter>> = OnceLock::new();
static EVENT_SEQ: AtomicU64 = AtomicU64::new(1);

/// Parsed Sentry DSN with the Envelope endpoint and public key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentryDsn {
    pub dsn: String,
    pub envelope_url: String,
    pub public_key: String,
}

/// Parses a standard Sentry DSN (`https://<key>@<host>[/<prefix>]/<project_id>`).
pub fn parse_dsn(raw: &str) -> Option<SentryDsn> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let url = reqwest::Url::parse(trimmed).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let public_key = url.username();
    if public_key.is_empty() {
        return None;
    }
    let host = url.host_str()?;
    let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    let project_id = *segments.last()?;
    if project_id.is_empty() || !project_id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    let prefix = if segments.len() > 1 {
        format!("/{}", segments[..segments.len() - 1].join("/"))
    } else {
        String::new()
    };
    let envelope_url = format!(
        "{}://{authority}{prefix}/api/{project_id}/envelope/",
        url.scheme()
    );
    Some(SentryDsn {
        dsn: trimmed.to_owned(),
        envelope_url,
        public_key: public_key.to_owned(),
    })
}

/// Build-time DSN injected via `RUSTCODE_SENTRY_DSN`; absent in dev builds.
pub fn configured_dsn() -> Option<SentryDsn> {
    option_env!("RUSTCODE_SENTRY_DSN").and_then(parse_dsn)
}

#[derive(Serialize, Deserialize)]
struct StoredPreference {
    enabled: bool,
}

fn config_path(config_dir: Option<&Path>) -> Option<PathBuf> {
    config_dir.map(|dir| dir.join(CONFIG_FILE))
}

fn load_enabled_from(config_dir: Option<&Path>) -> bool {
    let Some(path) = config_path(config_dir) else {
        return DEFAULT_ENABLED;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return DEFAULT_ENABLED;
    };
    serde_json::from_str::<StoredPreference>(&text)
        .map(|pref| pref.enabled)
        .unwrap_or(DEFAULT_ENABLED)
}

fn save_enabled_to(config_dir: Option<&Path>, enabled: bool) {
    let Some(dir) = config_dir else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if let Ok(text) = serde_json::to_string(&StoredPreference { enabled }) {
        let _ = std::fs::write(dir.join(CONFIG_FILE), text);
    }
}

/// Status returned to the webview for Settings and first-run notice state.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CrashReportStatus {
    pub enabled: bool,
    pub configured: bool,
}

#[derive(Clone, Debug)]
struct ScrubbedReport {
    source: String,
    platform: &'static str,
    error_type: String,
    message: String,
    stack: Option<String>,
}

#[derive(Default)]
struct RateLimiter {
    seen: HashMap<[u8; 32], Instant>,
    recent: VecDeque<Instant>,
}

impl RateLimiter {
    fn allow_at(&mut self, fingerprint: [u8; 32], now: Instant) -> bool {
        self.seen
            .retain(|_, at| now.saturating_duration_since(*at) < DEDUPE_WINDOW);
        while let Some(&front) = self.recent.front() {
            if now.saturating_duration_since(front) >= RATE_WINDOW {
                self.recent.pop_front();
            } else {
                break;
            }
        }
        if self.seen.contains_key(&fingerprint) || self.recent.len() >= RATE_MAX_EVENTS {
            return false;
        }
        self.seen.insert(fingerprint, now);
        self.recent.push_back(now);
        true
    }
}

/// Shared crash reporter state for the Tauri shell, webview and engine supervisor.
pub struct CrashReporter {
    dsn: Option<SentryDsn>,
    enabled: AtomicBool,
    version: Mutex<String>,
    limiter: Mutex<RateLimiter>,
    config_dir: Option<PathBuf>,
    home_dir: Option<PathBuf>,
}

impl CrashReporter {
    fn new(
        dsn: Option<SentryDsn>,
        config_dir: Option<PathBuf>,
        home_dir: Option<PathBuf>,
        version: String,
    ) -> Self {
        let enabled = load_enabled_from(config_dir.as_deref());
        Self {
            dsn,
            enabled: AtomicBool::new(enabled),
            version: Mutex::new(version),
            limiter: Mutex::new(RateLimiter::default()),
            config_dir,
            home_dir,
        }
    }

    /// Returns the process-wide reporter, creating it on first use.
    pub fn global() -> Arc<Self> {
        Arc::clone(GLOBAL.get_or_init(|| {
            Arc::new(Self::new(
                configured_dsn(),
                crate::diagnostics::config_dir(),
                crate::diagnostics::home_dir(),
                env!("CARGO_PKG_VERSION").to_owned(),
            ))
        }))
    }

    /// Updates the release version once Tauri has read its bundle metadata.
    pub fn set_version(&self, version: String) {
        if !version.is_empty() {
            *self.version.lock().unwrap_or_else(|e| e.into_inner()) = version;
        }
    }

    pub fn status(&self) -> CrashReportStatus {
        CrashReportStatus {
            enabled: self.enabled.load(Ordering::SeqCst),
            configured: self.dsn.is_some(),
        }
    }

    pub fn set_enabled(&self, enabled: bool) -> CrashReportStatus {
        self.enabled.store(enabled, Ordering::SeqCst);
        save_enabled_to(self.config_dir.as_deref(), enabled);
        self.status()
    }

    fn prepare_envelope(
        &self,
        source: &str,
        platform: &'static str,
        error_type: &str,
        message: &str,
        stack: Option<&str>,
    ) -> Option<(SentryDsn, String, String)> {
        if !self.enabled.load(Ordering::SeqCst) {
            return None;
        }
        let dsn = self.dsn.clone()?;
        let home = self.home_dir.as_deref();
        let clean_type = truncate_chars(&scrub(error_type, home), MAX_TYPE_CHARS);
        let clean_type = if clean_type.is_empty() {
            "Error".to_owned()
        } else {
            clean_type
        };
        let clean_message = truncate_chars(&scrub(message, home), MAX_MESSAGE_CHARS);
        let clean_stack = stack
            .map(|s| truncate_chars(&scrub(s, home), MAX_STACK_CHARS))
            .filter(|s| !s.trim().is_empty());

        let fingerprint =
            fingerprint_for(source, &clean_type, &clean_message, clean_stack.as_deref());
        {
            let mut limiter = self.limiter.lock().unwrap_or_else(|e| e.into_inner());
            if !limiter.allow_at(fingerprint, Instant::now()) {
                return None;
            }
        }

        let report = ScrubbedReport {
            source: source.to_owned(),
            platform,
            error_type: clean_type,
            message: clean_message,
            stack: clean_stack,
        };
        let version = self
            .version
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let event_id = generate_event_id(&report.message);
        let sent_at = chrono::Utc::now().to_rfc3339();
        let envelope = build_envelope(&dsn, &event_id, &sent_at, &version, &report);
        Some((dsn, envelope, version))
    }

    fn send_async(
        &self,
        source: &str,
        platform: &'static str,
        error_type: &str,
        message: &str,
        stack: Option<&str>,
    ) {
        let Some((dsn, envelope, version)) =
            self.prepare_envelope(source, platform, error_type, message, stack)
        else {
            return;
        };
        tauri::async_runtime::spawn(async move {
            let _ = send_envelope(&dsn, &envelope, &version, HTTP_TIMEOUT).await;
        });
    }

    fn send_blocking(
        &self,
        source: &str,
        platform: &'static str,
        error_type: &str,
        message: &str,
        stack: Option<&str>,
    ) {
        let Some((dsn, envelope, version)) =
            self.prepare_envelope(source, platform, error_type, message, stack)
        else {
            return;
        };
        let handle = std::thread::spawn(move || {
            tauri::async_runtime::block_on(async move {
                let _ = tokio::time::timeout(
                    PANIC_TIMEOUT,
                    send_envelope(&dsn, &envelope, &version, PANIC_TIMEOUT),
                )
                .await;
            });
        });
        let _ = handle.join();
    }
}

/// Installs a panic hook for the Tauri shell that forwards scrubbed panics when enabled.
pub fn install_panic_hook() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let reporter = CrashReporter::global();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                .unwrap_or("<non-string panic payload>");
            let location = info
                .location()
                .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
                .unwrap_or_else(|| "<unknown location>".to_owned());
            let thread = std::thread::current()
                .name()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{:?}", std::thread::current().id()));
            let message = format!("{payload} at {location} (thread={thread})");
            let backtrace = std::backtrace::Backtrace::force_capture().to_string();
            reporter.send_blocking("shell", "rust", "Panic", &message, Some(&backtrace));
            previous(info);
        }));
    });
}

/// Reports an unexpected engine sidecar exit or panic to Sentry when enabled.
pub fn report_engine_crash(reason: &str, recent_stderr: &[String]) {
    let reporter = CrashReporter::global();
    if let Some((headline, stack)) = extract_engine_panic(recent_stderr) {
        reporter.send_async("engine", "rust", "EnginePanic", &headline, Some(&stack));
    } else {
        reporter.send_async("engine", "rust", "EngineCrash", reason, None);
    }
}

/// Extracts Rust panic lines and stack frames from captured engine stderr output.
pub fn extract_engine_panic(lines: &[String]) -> Option<(String, String)> {
    let start = lines
        .iter()
        .position(|line| line.contains("panicked at") || line.contains("[PANIC]"))?;
    let headline = lines[start].trim().to_owned();
    let collected: Vec<&str> = lines[start..].iter().take(40).map(|s| s.as_str()).collect();
    Some((headline, collected.join("\n")))
}

/// Returns whether crash reporting is enabled and whether a build DSN is present.
#[tauri::command]
pub fn crash_report_status() -> CrashReportStatus {
    CrashReporter::global().status()
}

/// Enables or disables crash reporting immediately without requiring a restart.
#[tauri::command]
pub fn crash_report_set_enabled(enabled: bool) -> CrashReportStatus {
    CrashReporter::global().set_enabled(enabled)
}

/// Receives a scrubbed webview crash report and forwards it when enabled.
#[tauri::command]
pub async fn crash_report_send(
    source: String,
    name: String,
    message: String,
    stack: Option<String>,
    component_stack: Option<String>,
) {
    let combined_stack = match (stack, component_stack) {
        (Some(s), Some(c)) if !c.trim().is_empty() => Some(format!("{s}\n\nComponent stack:\n{c}")),
        (Some(s), _) => Some(s),
        (None, Some(c)) if !c.trim().is_empty() => Some(c),
        (None, _) => None,
    };
    let clean_source = match source.as_str() {
        "webview" | "react" => source.as_str(),
        _ => "webview",
    };
    CrashReporter::global().send_async(
        clean_source,
        "javascript",
        &name,
        &message,
        combined_stack.as_deref(),
    );
}

async fn send_envelope(
    dsn: &SentryDsn,
    envelope: &str,
    version: &str,
    timeout: Duration,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout)
        .user_agent(format!("rustcode-desktop/{version}"))
        .build()
        .map_err(|e| e.to_string())?;
    let auth = format!(
        "Sentry sentry_version=7, sentry_client=rustcode-desktop/{version}, sentry_key={}",
        dsn.public_key
    );
    client
        .post(&dsn.envelope_url)
        .header("Content-Type", "application/x-sentry-envelope")
        .header("X-Sentry-Auth", auth)
        .body(envelope.to_owned())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn generate_event_id(seed: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = EVENT_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(seq.to_le_bytes());
    hasher.update(seed.as_bytes());
    let digest = hasher.finalize();
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

fn fingerprint_for(source: &str, error_type: &str, message: &str, stack: Option<&str>) -> [u8; 32] {
    let first_frame = stack
        .and_then(|s| s.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or("");
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    hasher.update(b"\0");
    hasher.update(error_type.as_bytes());
    hasher.update(b"\0");
    hasher.update(truncate_chars(message, 120).as_bytes());
    hasher.update(b"\0");
    hasher.update(truncate_chars(first_frame, 120).as_bytes());
    hasher.finalize().into()
}

fn build_envelope(
    dsn: &SentryDsn,
    event_id: &str,
    sent_at: &str,
    version: &str,
    report: &ScrubbedReport,
) -> String {
    let header = serde_json::json!({
        "event_id": event_id,
        "sent_at": sent_at,
        "dsn": dsn.dsn,
    });
    let item_header = serde_json::json!({ "type": "event" });

    let mut exception = serde_json::json!({
        "type": report.error_type,
        "value": report.message,
    });
    if let Some(stack) = &report.stack {
        let frames: Vec<serde_json::Value> = stack
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(MAX_STACK_FRAMES)
            .map(|line| serde_json::json!({ "function": line }))
            .collect();
        if !frames.is_empty() {
            let mut ordered = frames;
            ordered.reverse();
            exception["stacktrace"] = serde_json::json!({ "frames": ordered });
        }
    }

    let event = serde_json::json!({
        "event_id": event_id,
        "timestamp": sent_at,
        "level": "error",
        "platform": report.platform,
        "release": format!("rustcode-desktop@{version}"),
        "contexts": {
            "os": { "name": std::env::consts::OS },
            "device": { "arch": std::env::consts::ARCH },
            "runtime": { "name": report.source, "version": version },
        },
        "tags": {
            "source": report.source,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "exception": {
            "values": [exception],
        },
    });

    format!("{header}\n{item_header}\n{event}")
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let kept: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{kept}…")
    } else {
        kept
    }
}

/// Scrubs home directory paths, prompt fields, credentials and env values from text.
pub fn scrub(input: &str, home: Option<&Path>) -> String {
    let mut out = input.to_owned();
    if let Some(dir) = home {
        let native = dir.to_string_lossy();
        let trimmed = native.trim_end_matches(['/', '\\']);
        if trimmed.len() > 1 {
            out = out.replace(trimmed, "~");
            let slash = trimmed.replace('\\', "/");
            if slash != trimmed {
                out = out.replace(&slash, "~");
            }
        }
    }
    out = scrub_home_paths(&out);
    out = scrub_json_sensitive_fields(&out);
    out = scrub_auth_headers(&out);
    out = scrub_secret_tokens(&out);
    scrub_env_assignments(&out)
}

fn is_path_sep(b: u8) -> bool {
    b == b'/' || b == b'\\'
}

fn is_path_segment_char(b: u8) -> bool {
    !is_path_sep(b)
        && !b.is_ascii_whitespace()
        && !matches!(
            b,
            b'"' | b'\'' | b'`' | b')' | b']' | b'}' | b',' | b';' | b':'
        )
}

fn scrub_home_paths(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if let Some(end) = match_unix_home(bytes, i) {
            out.push('~');
            i = end;
            continue;
        }
        if let Some(end) = match_windows_home(bytes, i) {
            out.push('~');
            i = end;
            continue;
        }
        let ch = input[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn match_unix_home(bytes: &[u8], start: usize) -> Option<usize> {
    let rest = &bytes[start..];
    let prefix_len = if rest.starts_with(b"/Users/") {
        7
    } else if rest.starts_with(b"/home/") {
        6
    } else {
        return None;
    };
    let mut end = start + prefix_len;
    while end < bytes.len() && is_path_segment_char(bytes[end]) {
        end += 1;
    }
    (end > start + prefix_len).then_some(end)
}

fn match_windows_home(bytes: &[u8], start: usize) -> Option<usize> {
    let rest = &bytes[start..];
    if rest.len() < 10 || !rest[0].is_ascii_alphabetic() || rest[1] != b':' || !is_path_sep(rest[2])
    {
        return None;
    }
    let after_drive = &rest[3..];
    if after_drive.len() < 7
        || !after_drive[..5].eq_ignore_ascii_case(b"users")
        || !is_path_sep(after_drive[5])
    {
        return None;
    }
    let prefix_len = 9;
    let mut end = start + prefix_len;
    while end < bytes.len() && is_path_segment_char(bytes[end]) {
        end += 1;
    }
    (end > start + prefix_len).then_some(end)
}

const SENSITIVE_JSON_KEYS: &[&str] = &[
    "prompt",
    "prompts",
    "content",
    "text",
    "message",
    "messages",
    "input",
    "system",
    "authorization",
    "api_key",
    "apikey",
    "token",
    "access_token",
    "refresh_token",
    "secret",
    "password",
    "dsn",
    "cookie",
];

fn scrub_json_sensitive_fields(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            if let Some((key_end, key)) = read_json_string(input, i) {
                let lower = key.to_ascii_lowercase();
                if SENSITIVE_JSON_KEYS.contains(&lower.as_str()) {
                    let mut j = key_end;
                    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if j < bytes.len() && bytes[j] == b':' {
                        j += 1;
                        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                            j += 1;
                        }
                        if j < bytes.len() && bytes[j] == b'"' {
                            if let Some((val_end, _)) = read_json_string(input, j) {
                                out.push_str(&input[i..j]);
                                out.push_str("\"[REDACTED]\"");
                                i = val_end;
                                continue;
                            }
                        } else if j < bytes.len() && bytes[j] == b'[' {
                            if let Some(arr_end) = skip_balanced(bytes, j, b'[', b']') {
                                out.push_str(&input[i..j]);
                                out.push_str("\"[REDACTED]\"");
                                i = arr_end;
                                continue;
                            }
                        }
                    }
                }
            }
        }
        let ch = input[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn read_json_string(input: &str, start: usize) -> Option<(usize, &str)> {
    let bytes = input.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some((i + 1, &input[start + 1..i])),
            _ => i += 1,
        }
    }
    None
}

fn skip_balanced(bytes: &[u8], start: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_string = false;
            }
        } else if b == b'"' {
            in_string = true;
        } else if b == open {
            depth += 1;
        } else if b == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(i + 1);
            }
        }
        i += 1;
    }
    None
}

fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+' | b'/' | b'=')
}

fn scrub_auth_headers(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        let rest = &bytes[i..];
        let scheme = if rest.len() > 7 && rest[..7].eq_ignore_ascii_case(b"bearer ") {
            Some("Bearer ")
        } else if rest.len() > 6 && rest[..6].eq_ignore_ascii_case(b"basic ") {
            Some("Basic ")
        } else {
            None
        };
        if let Some(label) = scheme {
            let mut end = i + label.len();
            while end < bytes.len() && is_token_char(bytes[end]) {
                end += 1;
            }
            if end > i + label.len() {
                out.push_str(label);
                out.push_str("[REDACTED]");
                i = end;
                continue;
            }
        }
        let ch = input[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

const TOKEN_PREFIXES: &[&str] = &[
    "sk-ant-",
    "sk-proj-",
    "sk-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "github_pat_",
    "AIza",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxr-",
    "xoxs-",
    "AKIA",
];

fn scrub_secret_tokens(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        let boundary = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if boundary {
            if let Some(prefix) = TOKEN_PREFIXES.iter().find(|p| input[i..].starts_with(*p)) {
                let mut end = i + prefix.len();
                while end < bytes.len() && is_token_char(bytes[end]) {
                    end += 1;
                }
                if end - (i + prefix.len()) >= 8 {
                    out.push_str("[REDACTED]");
                    i = end;
                    continue;
                }
            }
        }
        let ch = input[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

const SINGLE_WORD_ENV_KEYS: &[&str] = &[
    "HOME",
    "USER",
    "PATH",
    "SHELL",
    "PWD",
    "LOGNAME",
    "USERNAME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "KEY",
    "DSN",
    "AUTH",
    "COOKIE",
];

fn is_env_key(key: &str) -> bool {
    if key.len() < 3 || !key.as_bytes()[0].is_ascii_uppercase() {
        return false;
    }
    if !key
        .bytes()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
    {
        return false;
    }
    key.contains('_') || SINGLE_WORD_ENV_KEYS.contains(&key)
}

fn is_env_value_char(b: u8) -> bool {
    !b.is_ascii_whitespace() && !matches!(b, b'"' | b'\'' | b',' | b';' | b')' | b']' | b'}')
}

fn scrub_env_assignments(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        let boundary = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
        if boundary && bytes[i].is_ascii_uppercase() {
            let mut key_end = i;
            while key_end < bytes.len()
                && (bytes[key_end].is_ascii_uppercase()
                    || bytes[key_end].is_ascii_digit()
                    || bytes[key_end] == b'_')
            {
                key_end += 1;
            }
            if key_end < bytes.len() && bytes[key_end] == b'=' {
                let key = &input[i..key_end];
                if is_env_key(key) {
                    let val_start = key_end + 1;
                    let mut val_end = val_start;
                    if val_start < bytes.len()
                        && (bytes[val_start] == b'"' || bytes[val_start] == b'\'')
                    {
                        let quote = bytes[val_start];
                        val_end = val_start + 1;
                        while val_end < bytes.len() && bytes[val_end] != quote {
                            if bytes[val_end] == b'\\' {
                                val_end += 1;
                            }
                            val_end += 1;
                        }
                        if val_end < bytes.len() {
                            val_end += 1;
                        }
                    } else {
                        while val_end < bytes.len() && is_env_value_char(bytes[val_end]) {
                            val_end += 1;
                        }
                    }
                    if val_end > val_start {
                        out.push_str(key);
                        out.push_str("=[REDACTED]");
                        i = val_end;
                        continue;
                    }
                }
            }
        }
        let ch = input[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_and_prefixed_sentry_dsns() {
        let parsed = parse_dsn("https://abc123@o450.ingest.us.sentry.io/987654").unwrap();
        assert_eq!(parsed.public_key, "abc123");
        assert_eq!(
            parsed.envelope_url,
            "https://o450.ingest.us.sentry.io/api/987654/envelope/"
        );

        let self_hosted = parse_dsn("https://k9@errors.example.com:8443/sub/42").unwrap();
        assert_eq!(
            self_hosted.envelope_url,
            "https://errors.example.com:8443/sub/api/42/envelope/"
        );

        assert_eq!(parse_dsn(""), None);
        assert_eq!(parse_dsn("   "), None);
        assert_eq!(parse_dsn("https://sentry.io/123"), None);
        assert_eq!(parse_dsn("https://key@sentry.io/not-a-number"), None);
        assert_eq!(parse_dsn("ftp://key@sentry.io/123"), None);
    }

    #[test]
    fn scrubs_home_paths_prompts_tokens_and_env_values() {
        let home = Path::new("/custom/me");
        let raw = concat!(
            "Error at /custom/me/code/secret.rs and /Users/alice/work/app.ts ",
            "and /home/bob/repo/main.rs and C:\\Users\\carol\\proj\\file.rs ",
            "with OPENAI_API_KEY=sk-proj-1234567890abcdef ",
            "HOME=/home/bob MY_CUSTOM_VAR=\"secret value\" ",
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig ",
            "ghp_1234567890abcdefghij ",
            r#"payload={"prompt":"fix my private repo","messages":[{"role":"user","content":"top secret"}],"code":500}"#
        );
        let scrubbed = scrub(raw, Some(home));

        assert!(!scrubbed.contains("/custom/me"));
        assert!(!scrubbed.contains("alice"));
        assert!(!scrubbed.contains("bob"));
        assert!(!scrubbed.contains("carol"));
        assert!(scrubbed.contains("~/code/secret.rs"));
        assert!(scrubbed.contains("~/work/app.ts"));
        assert!(scrubbed.contains("~/repo/main.rs"));
        assert!(scrubbed.contains("~\\proj\\file.rs"));
        assert!(!scrubbed.contains("sk-proj-1234567890abcdef"));
        assert!(!scrubbed.contains("secret value"));
        assert!(!scrubbed.contains("eyJhbGciOiJIUzI1NiJ9"));
        assert!(!scrubbed.contains("ghp_1234567890abcdefghij"));
        assert!(!scrubbed.contains("fix my private repo"));
        assert!(!scrubbed.contains("top secret"));
        assert!(scrubbed.contains("OPENAI_API_KEY=[REDACTED]"));
        assert!(scrubbed.contains("MY_CUSTOM_VAR=[REDACTED]"));
        assert!(scrubbed.contains("Bearer [REDACTED]"));
        assert!(scrubbed.contains(r#""prompt":"[REDACTED]""#));
        assert!(scrubbed.contains(r#""messages":"[REDACTED]""#));
        assert!(scrubbed.contains(r#""code":500"#));
    }

    #[test]
    fn builds_valid_sentry_envelope_without_pii() {
        let dsn = parse_dsn("https://pub@o1.ingest.sentry.io/42").unwrap();
        let report = ScrubbedReport {
            source: "webview".into(),
            platform: "javascript",
            error_type: "TypeError".into(),
            message: "Cannot read properties of undefined".into(),
            stack: Some("at render (~/app.tsx:10)\nat mount (~/main.tsx:4)".into()),
        };
        let envelope = build_envelope(
            &dsn,
            "0123456789abcdef0123456789abcdef",
            "2026-11-21T22:00:00Z",
            "1.50.0",
            &report,
        );
        let lines: Vec<&str> = envelope.lines().collect();
        assert_eq!(lines.len(), 3);

        let header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(header["dsn"], dsn.dsn);
        let item: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(item["type"], "event");
        let event: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(event["release"], "rustcode-desktop@1.50.0");
        assert_eq!(event["platform"], "javascript");
        assert_eq!(event["tags"]["source"], "webview");
        assert!(event.get("user").is_none());
        assert!(event.get("breadcrumbs").is_none());
        assert_eq!(event["exception"]["values"][0]["type"], "TypeError");
        assert_eq!(
            event["exception"]["values"][0]["stacktrace"]["frames"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn prepares_nothing_when_disabled_or_missing_dsn() {
        let dsn = parse_dsn("https://pub@o1.ingest.sentry.io/42").unwrap();
        let no_dsn = CrashReporter::new(None, None, None, "1.0.0".into());
        assert!(!no_dsn.status().configured);
        assert!(no_dsn.status().enabled);
        assert!(no_dsn
            .prepare_envelope("webview", "javascript", "Error", "boom", None)
            .is_none());

        let with_dsn = CrashReporter::new(Some(dsn), None, None, "1.0.0".into());
        assert!(with_dsn.status().configured);
        assert!(with_dsn
            .prepare_envelope("webview", "javascript", "Error", "boom", None)
            .is_some());

        with_dsn.set_enabled(false);
        assert!(!with_dsn.status().enabled);
        assert!(with_dsn
            .prepare_envelope("webview", "javascript", "Error", "another boom", None)
            .is_none());
    }

    #[test]
    fn deduplicates_and_rate_limits_bursts() {
        let mut limiter = RateLimiter::default();
        let start = Instant::now();
        let fp = fingerprint_for("webview", "Error", "same", None);
        assert!(limiter.allow_at(fp, start));
        assert!(!limiter.allow_at(fp, start + Duration::from_secs(10)));
        assert!(limiter.allow_at(fp, start + DEDUPE_WINDOW + Duration::from_secs(1)));

        let mut burst = RateLimiter::default();
        for idx in 0..RATE_MAX_EVENTS {
            let unique = fingerprint_for("webview", "Error", &format!("err-{idx}"), None);
            assert!(burst.allow_at(unique, start));
        }
        let overflow = fingerprint_for("webview", "Error", "err-overflow", None);
        assert!(!burst.allow_at(overflow, start + Duration::from_secs(1)));
        assert!(burst.allow_at(overflow, start + RATE_WINDOW + Duration::from_secs(1)));
    }

    #[test]
    fn persists_preference_and_defaults_to_enabled() {
        let dir = std::env::temp_dir().join(format!("rc-crash-pref-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load_enabled_from(Some(&dir)));

        save_enabled_to(Some(&dir), false);
        assert!(!load_enabled_from(Some(&dir)));

        save_enabled_to(Some(&dir), true);
        assert!(load_enabled_from(Some(&dir)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extracts_engine_panics_from_stderr() {
        let normal = vec!["desktop: started rc --acp".to_owned(), "ready".to_owned()];
        assert_eq!(extract_engine_panic(&normal), None);

        let panic = vec![
            "starting".to_owned(),
            "thread 'tokio-runtime-worker' panicked at rustcode/engine/src/acp.rs:42:5:".to_owned(),
            "index out of bounds".to_owned(),
            "stack backtrace:".to_owned(),
            "   0: rustcode_engine::acp::run".to_owned(),
        ];
        let (headline, stack) = extract_engine_panic(&panic).unwrap();
        assert!(headline.contains("panicked at"));
        assert!(stack.contains("rustcode_engine::acp::run"));
        assert!(!stack.contains("starting"));
    }
}
