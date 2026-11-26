//! Project worktree lifecycle scripts (`.rustcode/worktrees.toml`): sequential
//! `setup` and `teardown` hooks plus an on-demand `run` dev server process.
//!
//! Repo-defined commands are never executed silently. Before running any script
//! from a repository, the user must explicitly trust the project's config hash
//! in `<config_dir>/worktree-script-trust.json`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Relative path of the repo-checked-in worktree lifecycle config.
pub(crate) const CONFIG_REL_PATH: &str = ".rustcode/worktrees.toml";
/// File in the user's config directory recording trusted project script hashes.
const TRUST_FILE: &str = "worktree-script-trust.json";
/// Default timeout for a `setup` or `teardown` hook run.
const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 300;
/// Upper bound on hook timeout to prevent runaway scripts.
const MAX_HOOK_TIMEOUT_SECS: u64 = 900;
/// Maximum bytes kept from the tail of captured hook output.
const HOOK_OUTPUT_TAIL: usize = 8_000;
/// Maximum bytes kept from the tail of captured `run` process output.
const RUN_OUTPUT_TAIL: usize = 16_000;

/// Why a worktree script request failed; `reason` goes to `error.data.reason`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScriptError {
    pub reason: &'static str,
    pub message: String,
}

impl ScriptError {
    pub(crate) fn new(reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }

    pub(crate) fn into_acp(self) -> agent_client_protocol::Error {
        agent_client_protocol::Error::invalid_params()
            .data(json!({ "reason": self.reason, "message": self.message }))
    }
}

/// Parsed `.rustcode/worktrees.toml` configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorktreeScriptsConfig {
    #[serde(default, deserialize_with = "de_string_or_list")]
    pub setup: Vec<String>,
    #[serde(default, deserialize_with = "de_string_or_list")]
    pub teardown: Vec<String>,
    #[serde(default, deserialize_with = "de_string_or_list")]
    pub run: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default, alias = "timeout_secs")]
    pub timeout_secs: Option<u64>,
}

impl WorktreeScriptsConfig {
    /// Whether at least one lifecycle command is configured.
    pub(crate) fn has_any(&self) -> bool {
        !self.setup.is_empty() || !self.teardown.is_empty() || !self.run.is_empty()
    }

    /// Bounded timeout applied to `setup` and `teardown` hook runs.
    pub(crate) fn hook_timeout(&self) -> Duration {
        let secs = self
            .timeout_secs
            .unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS)
            .clamp(1, MAX_HOOK_TIMEOUT_SECS);
        Duration::from_secs(secs)
    }
}

fn de_string_or_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrList {
        One(String),
        Many(Vec<String>),
    }
    let parsed = Option::<StringOrList>::deserialize(deserializer)?;
    let raw = match parsed {
        None => Vec::new(),
        Some(StringOrList::One(single)) => vec![single],
        Some(StringOrList::Many(many)) => many,
    };
    Ok(raw
        .into_iter()
        .map(|item| item.trim().to_owned())
        .filter(|item| !item.is_empty())
        .collect())
}

/// Config loaded from disk together with its path and SHA-256 fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedConfig {
    pub path: PathBuf,
    pub sha256: String,
    pub config: WorktreeScriptsConfig,
}

/// Parse a `.rustcode/worktrees.toml` document from raw text.
pub(crate) fn parse_config(raw: &str) -> Result<WorktreeScriptsConfig, String> {
    toml::from_str::<WorktreeScriptsConfig>(raw)
        .map_err(|error| format!("invalid {CONFIG_REL_PATH}: {error}"))
}

/// Hex SHA-256 fingerprint of raw config bytes.
pub(crate) fn config_sha256(raw: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw);
    hex::encode(hasher.finalize())
}

/// Find `.rustcode/worktrees.toml` in `worktree` first, falling back to `repo`.
pub(crate) fn find_config_path(repo: &Path, worktree: &Path) -> Option<PathBuf> {
    let local = worktree.join(CONFIG_REL_PATH);
    if local.is_file() {
        return Some(local);
    }
    let root_cfg = repo.join(CONFIG_REL_PATH);
    if root_cfg.is_file() {
        return Some(root_cfg);
    }
    None
}

/// Load and parse `.rustcode/worktrees.toml` for `(repo, worktree)`.
pub(crate) fn load_config(repo: &Path, worktree: &Path) -> Result<Option<LoadedConfig>, String> {
    let Some(path) = find_config_path(repo, worktree) else {
        return Ok(None);
    };
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let raw = String::from_utf8(bytes.clone())
        .map_err(|error| format!("{} is not valid UTF-8: {error}", path.display()))?;
    let config = parse_config(&raw)?;
    Ok(Some(LoadedConfig {
        path,
        sha256: config_sha256(&bytes),
        config,
    }))
}

/// Resolve the optional relative `cwd` inside `worktree`, rejecting traversal outside it.
pub(crate) fn resolve_exec_dir(
    worktree: &Path,
    subdir: Option<&str>,
) -> Result<PathBuf, ScriptError> {
    let Some(sub) = subdir.map(str::trim).filter(|sub| !sub.is_empty()) else {
        return Ok(worktree.to_path_buf());
    };
    let rel = Path::new(sub);
    if rel.is_absolute()
        || rel
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ScriptError::new(
            "invalid",
            format!("script cwd '{sub}' must be a relative path inside the worktree"),
        ));
    }
    let resolved = worktree.join(rel);
    if !resolved.is_dir() {
        return Err(ScriptError::new(
            "missing",
            format!("script cwd '{}' does not exist", resolved.display()),
        ));
    }
    Ok(resolved)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TrustStore {
    #[serde(default)]
    projects: BTreeMap<String, TrustedProject>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TrustedProject {
    sha256: String,
    approved_at: String,
}

fn default_trust_path() -> Option<PathBuf> {
    crate::config::get_config_dir().map(|dir| dir.join(TRUST_FILE))
}

fn canonical_key(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn load_trust_store(trust_path: &Path) -> TrustStore {
    std::fs::read_to_string(trust_path)
        .ok()
        .and_then(|text| serde_json::from_str::<TrustStore>(&text).ok())
        .unwrap_or_default()
}

fn save_trust_store(trust_path: &Path, store: &TrustStore) -> Result<(), ScriptError> {
    let text = serde_json::to_string_pretty(store)
        .map_err(|error| ScriptError::new("failed", error.to_string()))?;
    crate::atomic_file::write_atomic(trust_path, text.as_bytes())
        .map_err(|error| ScriptError::new("failed", error.to_string()))
}

/// Whether `repo` has an explicit trust entry matching `sha256` in `trust_path`.
pub(crate) fn is_trusted_in(trust_path: &Path, repo: &Path, sha256: &str) -> bool {
    let key = canonical_key(repo);
    load_trust_store(trust_path)
        .projects
        .get(&key)
        .is_some_and(|entry| entry.sha256 == sha256)
}

/// Record or revoke trust for `repo` in `trust_path`.
pub(crate) fn set_trust_in(
    trust_path: &Path,
    repo: &Path,
    sha256: &str,
    trusted: bool,
) -> Result<(), ScriptError> {
    let mut store = load_trust_store(trust_path);
    let key = canonical_key(repo);
    if trusted {
        store.projects.insert(
            key,
            TrustedProject {
                sha256: sha256.to_owned(),
                approved_at: chrono::Utc::now().to_rfc3339(),
            },
        );
    } else {
        store.projects.remove(&key);
    }
    save_trust_store(trust_path, &store)
}

fn is_trusted(repo: &Path, sha256: &str) -> bool {
    default_trust_path().is_some_and(|path| is_trusted_in(&path, repo, sha256))
}

/// Status of a worktree's `setup` hook execution.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetupRecord {
    pub status: String,
    #[serde(default)]
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl SetupRecord {
    fn idle() -> Self {
        Self {
            status: "idle".to_owned(),
            ..Self::default()
        }
    }
}

static SETUP_STATES: LazyLock<Mutex<HashMap<PathBuf, SetupRecord>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn setup_record_file(worktree: &Path) -> Option<PathBuf> {
    let root = crate::worktrees::root().and_then(|dir| std::fs::canonicalize(dir).ok())?;
    let canonical = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
    if !canonical.starts_with(&root) || canonical == root {
        return None;
    }
    let name = canonical.file_name()?.to_string_lossy().into_owned();
    Some(canonical.parent()?.join(format!(".{name}.setup.json")))
}

fn save_setup_record(worktree: &Path, record: SetupRecord) {
    let key = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
    if let Some(file) = setup_record_file(&key)
        && let Ok(text) = serde_json::to_string_pretty(&record)
    {
        let _ = crate::atomic_file::write_atomic(&file, text.as_bytes());
    }
    let mut states = SETUP_STATES
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    states.insert(key, record);
}

fn load_setup_record(worktree: &Path) -> SetupRecord {
    let key = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
    {
        let states = SETUP_STATES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(record) = states.get(&key) {
            return record.clone();
        }
    }
    if let Some(file) = setup_record_file(&key)
        && let Ok(text) = std::fs::read_to_string(&file)
        && let Ok(record) = serde_json::from_str::<SetupRecord>(&text)
    {
        return record;
    }
    SetupRecord::idle()
}

fn clear_setup_record(worktree: &Path) {
    let key = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
    if let Some(file) = setup_record_file(&key) {
        let _ = std::fs::remove_file(file);
    }
    let mut states = SETUP_STATES
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    states.remove(&key);
}

/// Keep the last `limit` bytes of `text`, cut at a UTF-8 character boundary.
fn tail_bytes(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

fn append_tail(buf: &mut String, chunk: &str, limit: usize) {
    buf.push_str(chunk);
    if buf.len() > limit {
        let mut start = buf.len() - limit;
        while !buf.is_char_boundary(start) {
            start += 1;
        }
        buf.drain(..start);
    }
}

/// Environment variables exposed to every worktree lifecycle command.
pub(crate) fn script_env(repo: &Path, worktree: &Path) -> Vec<(String, String)> {
    let root = canonical_key(repo);
    let wt_path = canonical_key(worktree);
    let wt_name = worktree
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "worktree".to_owned());
    vec![
        ("RUSTCODE_ROOT_PATH".to_owned(), root),
        ("RUSTCODE_WORKTREE_NAME".to_owned(), wt_name),
        ("RUSTCODE_WORKTREE_PATH".to_owned(), wt_path),
        (
            "PATH".to_owned(),
            crate::platform::compiler_augmented_path(),
        ),
    ]
}

fn shell_command(command_text: &str) -> std::process::Command {
    #[cfg(unix)]
    {
        let mut cmd = crate::platform::command("sh");
        cmd.args(["-c", command_text]);
        cmd
    }
    #[cfg(windows)]
    {
        let mut cmd = crate::platform::command("cmd");
        cmd.args(["/C", command_text]);
        cmd
    }
}

fn set_own_process_group(cmd: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
    }
}

fn terminate_process(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        if pid > 0 {
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Outcome of running a sequence of shell commands (`setup` or `teardown`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HookOutcome {
    pub success: bool,
    pub output: String,
    pub failed_command: Option<String>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Run `commands` sequentially in `exec_dir` with lifecycle env vars and bounded `timeout`.
pub(crate) fn run_hook_sequence(
    repo: &Path,
    worktree: &Path,
    exec_dir: &Path,
    commands: &[String],
    timeout: Duration,
) -> HookOutcome {
    let env = script_env(repo, worktree);
    let deadline = Instant::now() + timeout;
    let mut combined = String::new();
    for command_text in commands {
        if Instant::now() >= deadline {
            append_tail(
                &mut combined,
                &format!("\n$ {command_text}\nHook timed out.\n"),
                HOOK_OUTPUT_TAIL,
            );
            return HookOutcome {
                success: false,
                output: tail_bytes(combined.trim(), HOOK_OUTPUT_TAIL),
                failed_command: Some(command_text.clone()),
                exit_code: None,
                timed_out: true,
            };
        }
        append_tail(
            &mut combined,
            &format!("$ {command_text}\n"),
            HOOK_OUTPUT_TAIL,
        );
        let mut cmd = shell_command(command_text);
        cmd.current_dir(exec_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        set_own_process_group(&mut cmd);
        for (key, value) in &env {
            cmd.env(key, value);
        }
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                append_tail(
                    &mut combined,
                    &format!("Failed to start command: {error}\n"),
                    HOOK_OUTPUT_TAIL,
                );
                return HookOutcome {
                    success: false,
                    output: tail_bytes(combined.trim(), HOOK_OUTPUT_TAIL),
                    failed_command: Some(command_text.clone()),
                    exit_code: None,
                    timed_out: false,
                };
            }
        };
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let out_thread = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(pipe) = stdout.as_mut() {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        });
        let err_thread = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(pipe) = stderr.as_mut() {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        });
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() >= deadline => {
                    timed_out = true;
                    terminate_process(&mut child);
                    break Err("Hook timed out".to_owned());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(15)),
                Err(error) => break Err(error.to_string()),
            }
        };
        let out_text = String::from_utf8_lossy(&out_thread.join().unwrap_or_default()).into_owned();
        let err_text = String::from_utf8_lossy(&err_thread.join().unwrap_or_default()).into_owned();
        if !out_text.is_empty() {
            append_tail(&mut combined, &out_text, HOOK_OUTPUT_TAIL);
            if !out_text.ends_with('\n') {
                combined.push('\n');
            }
        }
        if !err_text.is_empty() {
            append_tail(&mut combined, &err_text, HOOK_OUTPUT_TAIL);
            if !err_text.ends_with('\n') {
                combined.push('\n');
            }
        }
        match status {
            Ok(exit) if exit.success() => {}
            Ok(exit) => {
                return HookOutcome {
                    success: false,
                    output: tail_bytes(combined.trim(), HOOK_OUTPUT_TAIL),
                    failed_command: Some(command_text.clone()),
                    exit_code: exit.code(),
                    timed_out: false,
                };
            }
            Err(message) => {
                append_tail(&mut combined, &format!("{message}\n"), HOOK_OUTPUT_TAIL);
                return HookOutcome {
                    success: false,
                    output: tail_bytes(combined.trim(), HOOK_OUTPUT_TAIL),
                    failed_command: Some(command_text.clone()),
                    exit_code: None,
                    timed_out,
                };
            }
        }
    }
    HookOutcome {
        success: true,
        output: tail_bytes(combined.trim(), HOOK_OUTPUT_TAIL),
        failed_command: None,
        exit_code: Some(0),
        timed_out: false,
    }
}

struct ManagedRun {
    child: Child,
    pid: u32,
    command: String,
    started_at: String,
    output: Arc<Mutex<String>>,
    exit_code: Option<i32>,
    exited: bool,
}

impl ManagedRun {
    fn poll(&mut self) {
        if self.exited {
            return;
        }
        if let Ok(Some(status)) = self.child.try_wait() {
            self.exited = true;
            self.exit_code = status.code();
        }
    }

    fn snapshot(&mut self) -> Value {
        self.poll();
        let output = self
            .output
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let status = if !self.exited {
            "running"
        } else if self.exit_code == Some(0) {
            "stopped"
        } else {
            "failed"
        };
        json!({
            "status": status,
            "pid": if self.exited { Value::Null } else { json!(self.pid) },
            "command": self.command,
            "exitCode": self.exit_code,
            "output": output,
            "startedAt": self.started_at,
        })
    }
}

static RUN_PROCESSES: LazyLock<Mutex<HashMap<PathBuf, ManagedRun>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn spawn_output_collector(pipe: impl Read + Send + 'static, sink: Arc<Mutex<String>>) {
    std::thread::spawn(move || {
        let mut reader = pipe;
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]);
                    let mut guard = sink.lock().unwrap_or_else(|error| error.into_inner());
                    append_tail(&mut guard, &chunk, RUN_OUTPUT_TAIL);
                }
            }
        }
    });
}

fn start_run_process(
    repo: &Path,
    worktree: &Path,
    config: &WorktreeScriptsConfig,
) -> Result<(), ScriptError> {
    if config.run.is_empty() {
        return Err(ScriptError::new(
            "invalid",
            "No run command is configured in .rustcode/worktrees.toml",
        ));
    }
    let exec_dir = resolve_exec_dir(worktree, config.cwd.as_deref())?;
    stop_run_process(worktree);
    let command_text = config.run.join(" && ");
    let mut cmd = shell_command(&command_text);
    cmd.current_dir(&exec_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    set_own_process_group(&mut cmd);
    for (key, value) in script_env(repo, worktree) {
        cmd.env(key, value);
    }
    let mut child = cmd.spawn().map_err(|error| {
        ScriptError::new("failed", format!("run script failed to start: {error}"))
    })?;
    let pid = child.id();
    let output = Arc::new(Mutex::new(String::new()));
    if let Some(stdout) = child.stdout.take() {
        spawn_output_collector(stdout, Arc::clone(&output));
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_output_collector(stderr, Arc::clone(&output));
    }
    let key = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
    let mut runs = RUN_PROCESSES
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    runs.insert(
        key,
        ManagedRun {
            child,
            pid,
            command: command_text,
            started_at: chrono::Utc::now().to_rfc3339(),
            output,
            exit_code: None,
            exited: false,
        },
    );
    Ok(())
}

/// Stop any managed `run` process started for `worktree`.
pub(crate) fn stop_run_process(worktree: &Path) {
    let key = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
    let removed = {
        let mut runs = RUN_PROCESSES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        runs.remove(&key)
    };
    if let Some(mut running) = removed
        && !running.exited
    {
        terminate_process(&mut running.child);
    }
}

fn run_snapshot(worktree: &Path) -> Value {
    let key = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
    let mut runs = RUN_PROCESSES
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    match runs.get_mut(&key) {
        Some(running) => running.snapshot(),
        None => json!({
            "status": "idle",
            "output": "",
        }),
    }
}

fn spawn_setup_background(repo: PathBuf, worktree: PathBuf, config: WorktreeScriptsConfig) {
    let started = SetupRecord {
        status: "running".to_owned(),
        output: String::new(),
        command: config.setup.first().cloned(),
        exit_code: None,
        updated_at: Some(chrono::Utc::now().to_rfc3339()),
    };
    save_setup_record(&worktree, started);
    std::thread::spawn(move || {
        let exec_dir = match resolve_exec_dir(&worktree, config.cwd.as_deref()) {
            Ok(dir) => dir,
            Err(error) => {
                save_setup_record(
                    &worktree,
                    SetupRecord {
                        status: "failed".to_owned(),
                        output: error.message,
                        command: config.setup.first().cloned(),
                        exit_code: None,
                        updated_at: Some(chrono::Utc::now().to_rfc3339()),
                    },
                );
                return;
            }
        };
        let outcome = run_hook_sequence(
            &repo,
            &worktree,
            &exec_dir,
            &config.setup,
            config.hook_timeout(),
        );
        save_setup_record(
            &worktree,
            SetupRecord {
                status: if outcome.success {
                    "succeeded".to_owned()
                } else {
                    "failed".to_owned()
                },
                output: outcome.output,
                command: outcome.failed_command,
                exit_code: outcome.exit_code,
                updated_at: Some(chrono::Utc::now().to_rfc3339()),
            },
        );
    });
}

/// Hook called right after `git worktree add`: runs trusted `setup` scripts in
/// the background or marks the worktree as awaiting user trust.
pub(crate) fn on_worktree_created(repo: &Path, worktree: &Path) {
    let Ok(Some(loaded)) = load_config(repo, worktree) else {
        return;
    };
    if loaded.config.setup.is_empty() {
        return;
    }
    if !is_trusted(repo, &loaded.sha256) {
        save_setup_record(
            worktree,
            SetupRecord {
                status: "needs_trust".to_owned(),
                output: String::new(),
                command: None,
                exit_code: None,
                updated_at: Some(chrono::Utc::now().to_rfc3339()),
            },
        );
        return;
    }
    spawn_setup_background(repo.to_path_buf(), worktree.to_path_buf(), loaded.config);
}

/// Hook called before removing a worktree: stops any managed `run` server, runs
/// trusted `teardown` commands (logging failures without blocking deletion),
/// and clears setup state.
pub(crate) fn on_worktree_removing(repo: &Path, worktree: &Path) {
    stop_run_process(worktree);
    if let Ok(Some(loaded)) = load_config(repo, worktree)
        && !loaded.config.teardown.is_empty()
        && is_trusted(repo, &loaded.sha256)
        && let Ok(exec_dir) = resolve_exec_dir(worktree, loaded.config.cwd.as_deref())
    {
        let outcome = run_hook_sequence(
            repo,
            worktree,
            &exec_dir,
            &loaded.config.teardown,
            loaded.config.hook_timeout(),
        );
        if !outcome.success {
            crate::dbg_log!(
                "[worktree_scripts] teardown failed in {}: {}",
                worktree.display(),
                outcome.output
            );
        }
    }
    clear_setup_record(worktree);
}

fn resolve_paths(path: &Path) -> Result<(PathBuf, PathBuf), ScriptError> {
    if !path.is_dir() {
        return Err(ScriptError::new(
            "missing",
            format!("{} does not exist", path.display()),
        ));
    }
    let worktree = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let repo = crate::worktrees::main_checkout(&worktree).unwrap_or_else(|| worktree.clone());
    Ok((repo, worktree))
}

/// Answer `_rustcode/worktrees/scripts/state` for `path`.
pub(crate) fn state(path: &Path) -> Result<Value, ScriptError> {
    let (repo, worktree) = resolve_paths(path)?;
    match load_config(&repo, &worktree) {
        Ok(None) => Ok(json!({
            "configured": false,
            "trusted": false,
            "setup": SetupRecord::idle(),
            "run": run_snapshot(&worktree),
        })),
        Ok(Some(loaded)) if !loaded.config.has_any() => Ok(json!({
            "configured": false,
            "configPath": loaded.path,
            "trusted": false,
            "setup": SetupRecord::idle(),
            "run": run_snapshot(&worktree),
        })),
        Ok(Some(loaded)) => {
            let trusted = is_trusted(&repo, &loaded.sha256);
            let mut setup = load_setup_record(&worktree);
            if !trusted && !loaded.config.setup.is_empty() && setup.status == "idle" {
                setup.status = "needs_trust".to_owned();
            } else if trusted && setup.status == "needs_trust" {
                setup.status = "idle".to_owned();
            }
            Ok(json!({
                "configured": true,
                "configPath": loaded.path,
                "sha256": loaded.sha256,
                "trusted": trusted,
                "config": {
                    "setup": loaded.config.setup,
                    "teardown": loaded.config.teardown,
                    "run": loaded.config.run,
                    "cwd": loaded.config.cwd,
                    "timeoutSecs": loaded.config.hook_timeout().as_secs(),
                },
                "setup": setup,
                "run": run_snapshot(&worktree),
            }))
        }
        Err(message) => Ok(json!({
            "configured": true,
            "configPath": find_config_path(&repo, &worktree),
            "trusted": false,
            "error": message,
            "setup": SetupRecord::idle(),
            "run": run_snapshot(&worktree),
        })),
    }
}

/// Answer `_rustcode/worktrees/scripts/trust`: record or revoke project script trust.
pub(crate) fn set_trust(
    path: &Path,
    expected_sha256: Option<&str>,
    trusted: bool,
) -> Result<Value, ScriptError> {
    let (repo, worktree) = resolve_paths(path)?;
    let Some(trust_path) = default_trust_path() else {
        return Err(ScriptError::new("failed", "no config directory"));
    };
    let loaded = load_config(&repo, &worktree)
        .map_err(|message| ScriptError::new("invalid", message))?
        .ok_or_else(|| {
            ScriptError::new("missing", format!("No {CONFIG_REL_PATH} found for project"))
        })?;
    if trusted
        && let Some(expected) = expected_sha256.filter(|sha| !sha.trim().is_empty())
        && expected != loaded.sha256
    {
        return Err(ScriptError::new(
            "stale",
            format!("{CONFIG_REL_PATH} changed since you reviewed it; reload and review again"),
        ));
    }
    set_trust_in(&trust_path, &repo, &loaded.sha256, trusted)?;
    if !trusted {
        stop_run_process(&worktree);
    }
    state(&worktree)
}

/// Answer `_rustcode/worktrees/scripts/action`: run `setup`, `teardown`, `run_start`, or `run_stop`.
pub(crate) fn run_action(path: &Path, action: &str) -> Result<Value, ScriptError> {
    let (repo, worktree) = resolve_paths(path)?;
    match action.trim() {
        "run_stop" => {
            stop_run_process(&worktree);
            state(&worktree)
        }
        action @ ("setup" | "teardown" | "run_start") => {
            let loaded = load_config(&repo, &worktree)
                .map_err(|message| ScriptError::new("invalid", message))?
                .ok_or_else(|| {
                    ScriptError::new("missing", format!("No {CONFIG_REL_PATH} found for project"))
                })?;
            if !is_trusted(&repo, &loaded.sha256) {
                return Err(ScriptError::new(
                    "untrusted",
                    format!("Trust {CONFIG_REL_PATH} for this project before running its scripts"),
                ));
            }
            match action {
                "setup" => {
                    if loaded.config.setup.is_empty() {
                        return Err(ScriptError::new(
                            "invalid",
                            "No setup commands are configured in .rustcode/worktrees.toml",
                        ));
                    }
                    spawn_setup_background(repo, worktree.clone(), loaded.config);
                    state(&worktree)
                }
                "teardown" => {
                    if loaded.config.teardown.is_empty() {
                        return Err(ScriptError::new(
                            "invalid",
                            "No teardown commands are configured in .rustcode/worktrees.toml",
                        ));
                    }
                    let exec_dir = resolve_exec_dir(&worktree, loaded.config.cwd.as_deref())?;
                    let outcome = run_hook_sequence(
                        &repo,
                        &worktree,
                        &exec_dir,
                        &loaded.config.teardown,
                        loaded.config.hook_timeout(),
                    );
                    if !outcome.success {
                        return Err(ScriptError::new("failed", outcome.output));
                    }
                    let mut answer = state(&worktree)?;
                    if let Some(obj) = answer.as_object_mut() {
                        obj.insert("teardownOutput".to_owned(), json!(outcome.output));
                    }
                    Ok(answer)
                }
                _ => {
                    start_run_process(&repo, &worktree, &loaded.config)?;
                    state(&worktree)
                }
            }
        }
        other => Err(ScriptError::new(
            "invalid",
            format!("Unknown worktree script action '{other}'"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_write_command(var: &str, file: &str) -> String {
        #[cfg(unix)]
        {
            format!("printf '%s' \"${var}\" > {file}")
        }
        #[cfg(windows)]
        {
            format!("echo %{var}%> {file}")
        }
    }

    #[test]
    fn parses_string_and_list_commands_with_bounded_timeout() {
        let parsed = parse_config(
            r#"
setup = ["echo one", "  ", "echo two"]
teardown = "echo bye"
run = "bun dev"
cwd = "apps/web"
timeout_secs = 9999
"#,
        )
        .unwrap();
        assert_eq!(parsed.setup, vec!["echo one", "echo two"]);
        assert_eq!(parsed.teardown, vec!["echo bye"]);
        assert_eq!(parsed.run, vec!["bun dev"]);
        assert_eq!(parsed.cwd.as_deref(), Some("apps/web"));
        assert_eq!(
            parsed.hook_timeout(),
            Duration::from_secs(MAX_HOOK_TIMEOUT_SECS)
        );

        let empty = parse_config("").unwrap();
        assert!(!empty.has_any());
        assert_eq!(
            empty.hook_timeout(),
            Duration::from_secs(DEFAULT_HOOK_TIMEOUT_SECS)
        );
    }

    #[test]
    fn rejects_parent_traversal_in_script_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_exec_dir(dir.path(), Some("../outside")).unwrap_err();
        assert_eq!(err.reason, "invalid");
    }

    #[test]
    fn trust_is_scoped_to_project_and_config_hash() {
        let dir = tempfile::tempdir().unwrap();
        let trust_path = dir.path().join("trust.json");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();

        let hash_v1 = config_sha256(b"setup = ['echo v1']");
        let hash_v2 = config_sha256(b"setup = ['echo v2']");
        assert!(!is_trusted_in(&trust_path, &repo, &hash_v1));

        set_trust_in(&trust_path, &repo, &hash_v1, true).unwrap();
        assert!(is_trusted_in(&trust_path, &repo, &hash_v1));
        assert!(!is_trusted_in(&trust_path, &repo, &hash_v2));

        set_trust_in(&trust_path, &repo, &hash_v1, false).unwrap();
        assert!(!is_trusted_in(&trust_path, &repo, &hash_v1));
    }

    #[test]
    fn hook_sequence_sets_env_vars_and_stops_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("main-repo");
        let wt = dir.path().join("feat-alpha");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&wt).unwrap();

        let outcome = run_hook_sequence(
            &repo,
            &wt,
            &wt,
            &[
                env_write_command("RUSTCODE_ROOT_PATH", "root.txt"),
                env_write_command("RUSTCODE_WORKTREE_NAME", "name.txt"),
            ],
            Duration::from_secs(10),
        );
        assert!(outcome.success, "output: {}", outcome.output);
        let root_recorded = std::fs::read_to_string(wt.join("root.txt")).unwrap();
        let name_recorded = std::fs::read_to_string(wt.join("name.txt")).unwrap();
        assert_eq!(root_recorded.trim(), canonical_key(&repo));
        assert_eq!(name_recorded.trim(), "feat-alpha");

        let failed = run_hook_sequence(
            &repo,
            &wt,
            &wt,
            &["exit 7".to_owned(), "echo should-not-run".to_owned()],
            Duration::from_secs(10),
        );
        assert!(!failed.success);
        assert_eq!(failed.exit_code, Some(7));
        assert_eq!(failed.failed_command.as_deref(), Some("exit 7"));
        assert!(!failed.output.contains("should-not-run"));
    }

    #[test]
    fn hook_sequence_enforces_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let sleep_cmd = if cfg!(windows) {
            "ping -n 5 127.0.0.1 > nul".to_owned()
        } else {
            "sleep 5".to_owned()
        };
        let outcome = run_hook_sequence(
            dir.path(),
            dir.path(),
            dir.path(),
            &[sleep_cmd],
            Duration::from_millis(120),
        );
        assert!(!outcome.success);
        assert!(outcome.timed_out);
    }

    #[test]
    fn load_config_prefers_worktree_over_repo() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = dir.path().join("wt");
        std::fs::create_dir_all(repo.join(".rustcode")).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(repo.join(CONFIG_REL_PATH), "setup = [\"echo repo\"]\n").unwrap();

        let from_repo = load_config(&repo, &wt).unwrap().unwrap();
        assert_eq!(from_repo.config.setup, vec!["echo repo"]);

        std::fs::create_dir_all(wt.join(".rustcode")).unwrap();
        std::fs::write(wt.join(CONFIG_REL_PATH), "setup = [\"echo wt\"]\n").unwrap();
        let from_wt = load_config(&repo, &wt).unwrap().unwrap();
        assert_eq!(from_wt.config.setup, vec!["echo wt"]);
    }
}
