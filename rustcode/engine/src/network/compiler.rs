use super::TurnContext;
use super::events::ToolResult;
use crate::platform::{compiler_augmented_path, resolve_bin};
use regex::Regex;
use rustcode_tool_protocol::text::strip_ansi_escapes;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use tokio_util::sync::CancellationToken;

/// Success receipts are in-memory only: resume must establish fresh evidence.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct VerificationIdentity {
    root: PathBuf,
    generation: u64,
    command: String,
    inputs: [u8; 32],
    sandbox: String,
    checker_environment: bool,
}
static VERIFIED: LazyLock<Mutex<HashMap<VerificationIdentity, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) fn is_verification_command(command: &str) -> bool {
    // Reject shell composition/substitution and arbitrary scripts. Exact argv
    // identity distinguishes targeted tests, flags and checker configuration.
    let words: Vec<_> = command.split_whitespace().collect();
    !words.is_empty()
        && !words.iter().any(|word| {
            matches!(
                *word,
                "-h" | "--help"
                    | "-V"
                    | "--version"
                    | "--list"
                    | "--fix"
                    | "--write"
                    | "-w"
                    | "--apply"
                    | "--apply-unsafe"
                    | "--watch"
            )
        })
        && !command.chars().any(|c| ";&|<>`$\\\n\r\"'".contains(c))
        && matches!(
            words.as_slice(),
            ["cargo", "check" | "test" | "clippy", ..]
                | ["bunx" | "npx", "tsc", "--noEmit", ..]
                | ["bunx", "biome", "check", ..]
                | ["npx", "@biomejs/biome", "check", ..]
        )
}

pub(crate) fn verification_identity(
    root: &Path,
    command: &str,
    sandbox_mode: crate::config::SandboxMode,
) -> Option<VerificationIdentity> {
    verification_identity_with_environment(
        root,
        command,
        sandbox_mode,
        std::env::vars_os().collect(),
    )
}

fn compiler_verification_identity(
    root: &Path,
    command: &str,
    sandbox_mode: crate::config::SandboxMode,
) -> Option<VerificationIdentity> {
    let mut identity = verification_identity(root, command, sandbox_mode)?;
    // Automatic checkers augment PATH and install scratch/cache variables.
    // A normal shell check has a different effective environment, even when
    // the visible command is identical; do not share its receipt.
    identity.checker_environment = true;
    Some(identity)
}

fn verification_identity_with_environment(
    root: &Path,
    command: &str,
    sandbox_mode: crate::config::SandboxMode,
    mut environment: Vec<(std::ffi::OsString, std::ffi::OsString)>,
) -> Option<VerificationIdentity> {
    if !is_verification_command(command)
        || command.contains("--manifest-path")
        || command.contains("--config")
        || command.contains("--help")
    {
        return None;
    }
    let snapshot = crate::workspace_intelligence::snapshot(root).ok()?;
    if !snapshot.cache_safe {
        return None;
    }
    // A nested crate may depend on workspace state outside this root. A fresh
    // process is cheaper than incorrectly treating that state as verified.
    if snapshot
        .root
        .ancestors()
        .skip(1)
        .any(|p| p.join("Cargo.toml").exists())
    {
        return None;
    }
    if command.contains("tsc") || command.contains("biome") {
        // Dependency trees and inherited configs can change outside the scanned
        // source set. Until those inputs have their own revision service, run
        // these checks rather than publishing an incomplete cache identity.
        if snapshot
            .root
            .ancestors()
            .any(|p| p.join("node_modules").exists())
        {
            return None;
        }
        for name in ["tsconfig.json", "biome.json", "biome.jsonc"] {
            let path = snapshot.root.join(name);
            if path.exists() {
                let config: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
                if config.get("extends").is_some() || config.get("references").is_some() {
                    return None;
                }
            }
        }
    }
    let mut inputs = Sha256::new();
    environment.sort();
    for (key, value) in environment {
        inputs.update(key.as_encoded_bytes());
        inputs.update([0]);
        inputs.update(value.as_encoded_bytes());
        inputs.update([0]);
    }
    inputs.update(sandbox_mode.to_string().as_bytes());
    inputs.update(crate::workspace_intelligence::git_revision(&snapshot.root).to_le_bytes());
    inputs.update(compiler_augmented_path().as_bytes());
    for program in ["cargo", "rustc", "bunx", "npx"] {
        let path = resolve_bin(program);
        inputs.update(path.as_os_str().as_encoded_bytes());
        if let Ok(metadata) = std::fs::metadata(path) {
            inputs.update(format!("{:?}:{}", metadata.modified().ok(), metadata.len()).as_bytes());
        }
    }
    // Cargo reads ancestor and user configuration in addition to source files.
    let mut configs = Vec::new();
    for ancestor in snapshot.root.ancestors() {
        configs.extend([
            ancestor.join(".cargo/config"),
            ancestor.join(".cargo/config.toml"),
        ]);
    }
    if let Some(home) = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cargo")))
    {
        configs.extend([home.join("config"), home.join("config.toml")]);
    }
    for path in configs {
        inputs.update(path.as_os_str().as_encoded_bytes());
        if path.exists() {
            inputs.update(std::fs::read(path).ok()?);
        }
        inputs.update([0]);
    }
    // Local manifests with paths outside the scanned root are deliberately
    // uncached. That includes external lib/build paths and path dependencies.
    for path in snapshot
        .files
        .keys()
        .filter(|p| p.file_name().is_some_and(|n| n == "Cargo.toml"))
    {
        let manifest: toml::Value = std::fs::read_to_string(path).ok()?.parse().ok()?;
        if has_external_path(&manifest, path.parent()?, &snapshot.root) {
            return None;
        }
    }
    Some(VerificationIdentity {
        root: snapshot.root,
        generation: snapshot.generation,
        command: command.to_string(),
        inputs: inputs.finalize().into(),
        sandbox: sandbox_mode.to_string(),
        checker_environment: false,
    })
}

fn has_external_path(value: &toml::Value, cwd: &Path, root: &Path) -> bool {
    match value {
        toml::Value::Table(table) => table.iter().any(|(key, value)| {
            if key == "path"
                && let Some(path) = value.as_str()
            {
                let path = cwd.join(path);
                return !path.canonicalize().is_ok_and(|p| {
                    p.strip_prefix(root).is_ok_and(|relative| {
                        !relative.components().any(|component| {
                            matches!(
                                component.as_os_str().to_str(),
                                Some("target" | "node_modules" | ".git" | ".rustcode")
                            )
                        })
                    })
                });
            }
            has_external_path(value, cwd, root)
        }),
        toml::Value::Array(values) => values
            .iter()
            .any(|value| has_external_path(value, cwd, root)),
        _ => false,
    }
}

pub(crate) fn cached_verification(identity: &VerificationIdentity) -> Option<String> {
    let cache = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    let output = cache.get(identity)?;
    crate::logger::operational_event(
        "verification.cache",
        serde_json::json!({
            "generation": identity.generation, "command": identity.command, "reused": true
        }),
    );
    Some(format!(
        "[CACHED VERIFICATION]\n{}: successful verification for workspace generation {}\n{}",
        identity.command, identity.generation, output
    ))
}

/// Call only after complete, uncancelled success, using an identity captured
/// before execution. Revalidate afterwards so edits during the check cannot
/// produce completion evidence for a different generation.
pub(crate) fn record_verification(identity: VerificationIdentity, output: String) {
    let mode = match identity.sandbox.as_str() {
        "trusted" => crate::config::SandboxMode::Trusted,
        "read_only" => crate::config::SandboxMode::ReadOnly,
        "workspace_write" => crate::config::SandboxMode::WorkspaceWrite,
        "workspace_write_network" => crate::config::SandboxMode::WorkspaceWriteNetwork,
        _ => return,
    };
    let current = if identity.checker_environment {
        compiler_verification_identity(&identity.root, &identity.command, mode)
    } else {
        verification_identity(&identity.root, &identity.command, mode)
    };
    if current.as_ref() != Some(&identity) {
        return;
    }
    let mut cache = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 256 {
        cache.clear();
    }
    cache.insert(identity, output);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompilerCheckOutcome {
    Passed,
    SourceDiagnostics { output: String, fingerprint: String },
    UnverifiedInfrastructure { reason: String },
}

impl CompilerCheckOutcome {
    fn legacy_output(&self, command: &str) -> Option<String> {
        match self {
            Self::Passed => None,
            Self::SourceDiagnostics { output, .. } => Some(output.clone()),
            Self::UnverifiedInfrastructure { reason } => Some(format!(
                "__BUILD_UNVERIFIED__: `{command}` {reason}. The build could not be verified. Check the checker environment or sandbox, then rerun verification."
            )),
        }
    }
}

pub(crate) async fn run_compiler_check(
    cwd: &std::path::Path,
    cancel_token: &CancellationToken,
    sandbox_mode: crate::config::SandboxMode,
) -> Option<String> {
    let (outcome, command) = run_compiler_check_with_command(cwd, cancel_token, sandbox_mode).await;
    outcome.legacy_output(command)
}

async fn run_compiler_check_with_command(
    cwd: &std::path::Path,
    cancel_token: &CancellationToken,
    sandbox_mode: crate::config::SandboxMode,
) -> (CompilerCheckOutcome, &'static str) {
    let (command, cargo, timeout) = if cwd.join("Cargo.toml").exists() {
        ("cargo check --message-format=json", true, 120)
    } else if cwd.join("biome.json").exists() || cwd.join("biome.jsonc").exists() {
        let command = if resolve_bin("bunx").exists() {
            "bunx biome check ."
        } else {
            "npx @biomejs/biome check ."
        };
        (command, false, 60)
    } else if cwd.join("tsconfig.json").exists() {
        let command = if resolve_bin("bunx").exists() {
            "bunx tsc --noEmit"
        } else {
            "npx tsc --noEmit"
        };
        (command, false, 60)
    } else {
        return (CompilerCheckOutcome::Passed, "compiler check");
    };
    (
        run_compiler_command_outcome(
            cwd,
            command,
            cargo,
            std::time::Duration::from_secs(timeout),
            cancel_token,
            sandbox_mode,
        )
        .await,
        command,
    )
}

#[cfg(test)]
async fn run_compiler_command(
    cwd: &std::path::Path,
    command: &str,
    cargo: bool,
    timeout: std::time::Duration,
    cancel_token: &CancellationToken,
) -> Option<String> {
    run_compiler_command_outcome(
        cwd,
        command,
        cargo,
        timeout,
        cancel_token,
        crate::config::SandboxMode::default(),
    )
    .await
    .legacy_output(command)
}

pub(super) async fn run_compiler_command_outcome(
    cwd: &std::path::Path,
    command: &str,
    cargo: bool,
    timeout: std::time::Duration,
    cancel_token: &CancellationToken,
    sandbox_mode: crate::config::SandboxMode,
) -> CompilerCheckOutcome {
    let unverified = |reason: String| CompilerCheckOutcome::UnverifiedInfrastructure { reason };
    if cancel_token.is_cancelled() {
        return record_unverified_event(unverified("was cancelled".to_string()), command);
    }
    let identity = compiler_verification_identity(cwd, command, sandbox_mode);
    if let Some(identity) = &identity
        && cached_verification(identity).is_some()
        && !cancel_token.is_cancelled()
    {
        return CompilerCheckOutcome::Passed;
    }
    let (scratch_container, scratch_path) = match create_compiler_scratch(cwd) {
        Ok(scratch) => scratch,
        Err(error) => {
            return record_unverified_event(unverified(error), command);
        }
    };
    let writable_roots = [cwd.to_path_buf(), scratch_path.clone()];
    let session_scratch_roots = [scratch_path.clone()];
    let command_for_exec = if sandbox_mode.is_trusted() {
        crate::tools::exec::sandbox::passthrough_command(command)
    } else {
        match crate::tools::exec::sandbox::command(
            command,
            crate::tools::exec::sandbox::SandboxPolicy {
                command_cwd: Some(cwd),
                workspace_root: Some(cwd),
                writable_roots: &writable_roots,
                session_scratch_roots: &session_scratch_roots,
                one_shot_writable_roots: &[],
                write_access: sandbox_mode.allows_workspace_write(),
                network_access: sandbox_mode.allows_network(),
            },
        ) {
            Ok(command) => command,
            Err(error) => return record_unverified_event(unverified(error.to_string()), command),
        }
    };
    // A child token also stops the blocking worker if this async future is dropped.
    let worker_token = cancel_token.child_token();
    let _cancel_on_drop = worker_token.clone().drop_guard();
    let request = rustcode_command::CommandRequest {
        command: command_for_exec.command,
        status_command: None,
        sandboxed_shell: true,
        cwd: Some(cwd.to_path_buf()),
        env: vec![
            ("PATH".into(), compiler_augmented_path().into()),
            ("TMPDIR".into(), scratch_path.clone().into_os_string()),
            ("TMP".into(), scratch_path.clone().into_os_string()),
            ("TEMP".into(), scratch_path.clone().into_os_string()),
            (
                "XDG_CACHE_HOME".into(),
                scratch_path.join("cache").into_os_string(),
            ),
            (
                "NPM_CONFIG_CACHE".into(),
                scratch_path.join("npm-cache").into_os_string(),
            ),
            (
                "BUN_INSTALL_CACHE_DIR".into(),
                scratch_path.join("bun-cache").into_os_string(),
            ),
        ],
        timeout,
        process_group: true,
        inherited_fds: command_for_exec.inherited_fds,
    };
    let output = spawn_compiler_worker(scratch_container, move || {
        rustcode_command::run_with_timeout_cancellable(
            &request,
            None,
            Some(Arc::new(move || worker_token.is_cancelled())),
        )
    })
    .await;
    let outcome = match output {
        Ok(Ok(output)) => classify_compiler_output(
            command,
            cargo,
            output.success,
            output.exit_code,
            &String::from_utf8_lossy(output.stdout.bytes()),
            &String::from_utf8_lossy(output.stderr.bytes()),
        ),
        Ok(Err(error)) => unverified(error.to_string()),
        Err(error) => unverified(format!("could not complete ({error})")),
    };
    if outcome == CompilerCheckOutcome::Passed
        && !cancel_token.is_cancelled()
        && let Some(identity) = identity
        && compiler_verification_identity(cwd, command, sandbox_mode).as_ref() == Some(&identity)
    {
        record_verification(identity, String::new());
    }
    record_unverified_event(outcome, command)
}

fn create_compiler_scratch(cwd: &Path) -> Result<(tempfile::TempDir, PathBuf), String> {
    let workspace = cwd.canonicalize().map_err(|error| {
        format!(
            "could not resolve checker workspace '{}' ({error})",
            cwd.display()
        )
    })?;
    create_compiler_scratch_in(&workspace, &compiler_scratch_bases())
}

fn compiler_scratch_bases() -> Vec<PathBuf> {
    let mut bases = vec![std::env::temp_dir()];
    #[cfg(unix)]
    bases.extend([PathBuf::from("/tmp"), PathBuf::from("/var/tmp")]);
    #[cfg(windows)]
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        bases.push(PathBuf::from(system_root).join("Temp"));
    }
    bases
}

fn create_compiler_scratch_in(
    workspace: &Path,
    candidate_bases: &[PathBuf],
) -> Result<(tempfile::TempDir, PathBuf), String> {
    let workspace = workspace.canonicalize().map_err(|error| {
        format!(
            "could not resolve checker workspace '{}' ({error})",
            workspace.display()
        )
    })?;
    let mut failures = Vec::new();
    for candidate in candidate_bases {
        let base = match candidate.canonicalize() {
            Ok(base) if base.is_dir() => base,
            Ok(_) => continue,
            Err(error) => {
                failures.push(format!("{}: {error}", candidate.display()));
                continue;
            }
        };
        if base.starts_with(&workspace) {
            failures.push(format!(
                "{} is inside the checker workspace",
                base.display()
            ));
            continue;
        }
        let container = match tempfile::Builder::new()
            .prefix("rustcode-compiler-check-")
            .tempdir_in(&base)
        {
            Ok(container) => container,
            Err(error) => {
                failures.push(format!("{}: {error}", base.display()));
                continue;
            }
        };
        let scratch_candidate = container.path().join("sandbox");
        if let Err(error) = std::fs::create_dir(&scratch_candidate) {
            failures.push(format!("{}: {error}", scratch_candidate.display()));
            continue;
        }
        let scratch = match scratch_candidate.canonicalize() {
            Ok(scratch) => scratch,
            Err(error) => {
                failures.push(format!("{}: {error}", scratch_candidate.display()));
                continue;
            }
        };
        if scratch == workspace || scratch.starts_with(&workspace) {
            failures.push(format!(
                "{} resolves inside the checker workspace",
                scratch.display()
            ));
            continue;
        }
        return Ok((container, scratch));
    }
    let detail = if failures.is_empty() {
        "no usable temporary base was available".to_string()
    } else {
        failures.join("; ")
    };
    Err(format!(
        "could not create checker scratch outside workspace '{}': {detail}",
        workspace.display()
    ))
}

fn spawn_compiler_worker<T>(
    scratch: tempfile::TempDir,
    worker: impl FnOnce() -> T + Send + 'static,
) -> tokio::task::JoinHandle<T>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let _scratch = scratch;
        worker()
    })
}

fn record_unverified_event(outcome: CompilerCheckOutcome, command: &str) -> CompilerCheckOutcome {
    if let CompilerCheckOutcome::UnverifiedInfrastructure { reason } = &outcome {
        crate::logger::operational_event(
            "compiler.check_unverified",
            serde_json::json!({
                "command": command,
                "reason": reason,
                "recovery": "resolve checker startup, sandbox, workspace, or temporary-directory access and rerun verification",
            }),
        );
    }
    outcome
}

fn classify_compiler_output(
    command: &str,
    cargo: bool,
    success: bool,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> CompilerCheckOutcome {
    let normalized = strip_ansi_escapes(&format!("{stdout}\n{stderr}"));
    let lower = normalized.to_lowercase();
    if lower.contains("permissiondenied")
        || lower.contains("permission denied")
        || lower.contains("unable to write files to tempdir")
        || lower.contains("command not found")
        || lower.contains("no such file or directory")
        || lower.contains("failed to start")
        || lower.contains("could not find executable")
    {
        return CompilerCheckOutcome::UnverifiedInfrastructure {
            reason: normalized.trim().to_owned(),
        };
    }
    if cargo {
        let errors = stdout
            .lines()
            .filter_map(|line| {
                let value: serde_json::Value = serde_json::from_str(line).ok()?;
                if value.get("reason")?.as_str()? != "compiler-message" {
                    return None;
                }
                let message = value.get("message")?;
                (message.get("level")?.as_str()? == "error")
                    .then(|| message.get("rendered")?.as_str().map(strip_ansi_escapes))
                    .flatten()
            })
            .collect::<Vec<_>>();
        if !errors.is_empty() {
            let diagnostics = errors.join("\n");
            return CompilerCheckOutcome::SourceDiagnostics {
                fingerprint: fingerprint_compiler_diagnostics(&diagnostics),
                output: diagnostics,
            };
        }
    }
    if success {
        return CompilerCheckOutcome::Passed;
    }
    let mut diagnostics = normalized.trim().to_owned();
    if diagnostics.is_empty() {
        return CompilerCheckOutcome::UnverifiedInfrastructure {
            reason: format!("`{command}` exited unsuccessfully without output"),
        };
    }
    if !has_recognized_source_diagnostic(&diagnostics) {
        let status = exit_code.map_or_else(
            || "terminated without an exit code".to_owned(),
            |code| format!("exited with status {code}"),
        );
        return CompilerCheckOutcome::UnverifiedInfrastructure {
            reason: format!(
                "`{command}` {status}; output did not match a known compiler or linter diagnostic:\n{diagnostics}"
            ),
        };
    }
    const MAX_DIAGNOSTIC_BYTES: usize = 16 * 1024;
    if diagnostics.len() > MAX_DIAGNOSTIC_BYTES {
        let mut end = MAX_DIAGNOSTIC_BYTES;
        while !diagnostics.is_char_boundary(end) {
            end -= 1;
        }
        diagnostics.truncate(end);
        diagnostics.push_str("\n[compiler diagnostics truncated]");
    }
    let status = exit_code.map_or_else(
        || "terminated without an exit code".to_owned(),
        |code| format!("exited with status {code}"),
    );
    let rendered = format!("`{command}` {status}.\n{diagnostics}")
        .trim_end()
        .to_owned();
    CompilerCheckOutcome::SourceDiagnostics {
        fingerprint: fingerprint_compiler_diagnostics(&rendered),
        output: rendered,
    }
}

fn has_recognized_source_diagnostic(output: &str) -> bool {
    static TYPESCRIPT_DIAGNOSTIC: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(.+\.(?i:tsx?|jsx?))\((\d+),(\d+)\):\s*error\s+TS\d+\b").unwrap()
    });
    static BIOME_DIAGNOSTIC: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(.+\.(?i:tsx?|jsx?|mjs|cjs|jsonc?|css|scss|html|vue|svelte|astro|graphql|gql|ya?ml)):\d+:\d+\s+(?:lint|assist|format)/",
        )
        .unwrap()
    });
    static RUST_SOURCE_LOCATION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*-->\s+\S+\.rs:\d+:\d+").unwrap());
    let lower = output.to_lowercase();
    let has_rust_source_location = output
        .lines()
        .any(|line| RUST_SOURCE_LOCATION.is_match(line));
    let rust_diagnostic = has_rust_source_location
        && (lower.contains("error[")
            || lower
                .lines()
                .any(|line| line.trim_start().starts_with("error:")));
    let typescript_diagnostic = output.lines().any(|line| {
        TYPESCRIPT_DIAGNOSTIC
            .captures(line)
            .is_some_and(|captures| {
                captures
                    .get(1)
                    .is_some_and(|path| is_plausible_source_path(path.as_str()))
            })
    });
    let biome_diagnostic = output.lines().any(|line| {
        BIOME_DIAGNOSTIC.captures(line).is_some_and(|captures| {
            captures
                .get(1)
                .is_some_and(|path| is_plausible_source_path(path.as_str()))
        })
    });
    rust_diagnostic || typescript_diagnostic || biome_diagnostic
}

fn is_plausible_source_path(path: &str) -> bool {
    if path.is_empty() || path != path.trim() {
        return false;
    }
    let is_windows_absolute = path.as_bytes().get(1) == Some(&b':')
        && path
            .as_bytes()
            .get(2)
            .is_some_and(|separator| matches!(separator, b'/' | b'\\'));
    if path.contains(':') && !is_windows_absolute {
        return false;
    }
    if !path.chars().any(char::is_whitespace) {
        return true;
    }
    if path.starts_with('/') || is_windows_absolute {
        return true;
    }
    if !path.contains(['/', '\\']) {
        return true;
    }
    let first_component = path.split(['/', '\\']).next().unwrap_or_default();
    !first_component.is_empty()
        && !first_component.chars().any(char::is_whitespace)
        && path.contains(['/', '\\'])
}

fn fingerprint_compiler_diagnostics(diagnostics: &str) -> String {
    normalize_diagnostic_fingerprint(&compiler_diagnostics_with_snippets(diagnostics))
}

fn normalize_diagnostic_fingerprint(diagnostics: &str) -> String {
    diagnostics.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) async fn cached_compiler_check(
    root: &std::path::Path,
    dirty: &mut bool,
    cache: &mut Option<(std::path::PathBuf, Option<String>)>,
    cancel_token: &CancellationToken,
    sandbox_mode: crate::config::SandboxMode,
) -> Option<String> {
    // The turn-local tuple remains a compatibility projection. Validity is
    // checked at execution using workspace/config/environment identity.
    if !cancel_token.is_cancelled() {
        let command = if root.join("Cargo.toml").exists() {
            Some("cargo check --message-format=json")
        } else if root.join("biome.json").exists() || root.join("biome.jsonc").exists() {
            Some(if resolve_bin("bunx").exists() {
                "bunx biome check ."
            } else {
                "npx @biomejs/biome check ."
            })
        } else if root.join("tsconfig.json").exists() {
            Some(if resolve_bin("bunx").exists() {
                "bunx tsc --noEmit"
            } else {
                "npx tsc --noEmit"
            })
        } else {
            None
        };
        if let Some(identity) =
            command.and_then(|command| compiler_verification_identity(root, command, sandbox_mode))
            && cached_verification(&identity).is_some()
            && !cancel_token.is_cancelled()
        {
            *dirty = false;
            *cache = Some((root.to_path_buf(), None));
            return None;
        }
    }
    let result = run_compiler_check(root, cancel_token, sandbox_mode).await;
    if result.is_some() || cancel_token.is_cancelled() {
        *dirty = true;
        *cache = None;
        return result;
    }
    *cache = Some((root.to_path_buf(), result.clone()));
    *dirty = false;
    result
}

pub(crate) fn append_compiler_diagnostics(result: &mut ToolResult, diagnostics: &str) {
    if diagnostics.starts_with("__BUILD_UNVERIFIED__")
        || diagnostics.starts_with("[CACHED VERIFICATION]")
    {
        result.content.push_str("\n\n");
        result.content.push_str(diagnostics);
        return;
    }
    append_compiler_outcome(
        result,
        &CompilerCheckOutcome::SourceDiagnostics {
            output: diagnostics.to_string(),
            fingerprint: normalize_diagnostic_fingerprint(diagnostics),
        },
    );
}

pub(crate) fn append_compiler_outcome(result: &mut ToolResult, outcome: &CompilerCheckOutcome) {
    match outcome {
        CompilerCheckOutcome::Passed => {}
        CompilerCheckOutcome::SourceDiagnostics { output, .. } => {
            result
                .content
                .push_str("\n\nLSP/Compiler errors detected in workspace, please fix:\n");
            result
                .content
                .push_str(&compiler_diagnostics_with_snippets(output));
            // Reported next to the result, not as its status: the edit this
            // is appended to succeeded (#1887).
        }
        CompilerCheckOutcome::UnverifiedInfrastructure { reason } => {
            result.content.push_str("\n\n");
            result.content.push_str("__BUILD_UNVERIFIED__: ");
            result.content.push_str(reason);
            result.content.push_str(". The build could not be verified. Check the checker environment or sandbox, then rerun verification.");
        }
    }
}

fn compiler_diagnostic_locations(diagnostics: &str) -> Vec<(String, usize, usize)> {
    static TYPESCRIPT_LOCATION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(.+\.(?i:tsx?|jsx?))\((\d+),(\d+)\):").unwrap());
    static RUST_LOCATION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*-->\s+(\S+):(\d+):(\d+)").unwrap());

    diagnostics
        .lines()
        .filter_map(|line| {
            let captures = TYPESCRIPT_LOCATION
                .captures(line)
                .or_else(|| RUST_LOCATION.captures(line))?;
            Some((
                captures.get(1)?.as_str().to_string(),
                captures.get(2)?.as_str().parse().ok()?,
                captures.get(3)?.as_str().parse().ok()?,
            ))
        })
        .collect()
}

pub(crate) fn compiler_diagnostics_with_snippets(diagnostics: &str) -> String {
    let mut enriched = diagnostics.to_string();
    let mut seen = std::collections::BTreeSet::new();
    for (path, line, column) in compiler_diagnostic_locations(diagnostics)
        .into_iter()
        .take(4)
    {
        if line == 0 || !seen.insert((path.clone(), line, column)) {
            continue;
        }
        let resolved = crate::tools::resolve_tool_path(&path);
        let Ok(source) = std::fs::read_to_string(resolved) else {
            continue;
        };
        let lines = source.lines().collect::<Vec<_>>();
        if line > lines.len() {
            continue;
        }
        let start = line.saturating_sub(2).max(1);
        let end = (line + 2).min(lines.len());
        enriched.push_str(&format!("\n\n[compiler context: {path}:{line}:{column}]\n"));
        for number in start..=end {
            enriched.push_str(&format!("{number}: {}\n", lines[number - 1]));
        }
    }
    enriched
}

const COMPILER_DIAGNOSTIC_MARKER: &str = "LSP/Compiler errors detected in workspace, please fix:";

#[cfg(test)]
mod compiler_execution_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn compiler_scratch_skips_a_project_local_temp_base() {
        let project = tempfile::tempdir().unwrap();
        let local_temp = project.path().join("tmp");
        std::fs::create_dir(&local_temp).unwrap();
        let external_temp = tempfile::tempdir().unwrap();

        let (container, scratch_path) = create_compiler_scratch_in(
            project.path(),
            &[local_temp, external_temp.path().to_path_buf()],
        )
        .unwrap();

        assert!(
            !scratch_path.starts_with(project.path()),
            "scratch {} is inside project {}",
            scratch_path.display(),
            project.path().display()
        );
        assert!(
            scratch_path.starts_with(external_temp.path().canonicalize().unwrap()),
            "scratch {} did not use external base {}",
            scratch_path.display(),
            external_temp.path().display()
        );
        assert_eq!(
            scratch_path,
            container.path().join("sandbox").canonicalize().unwrap()
        );
        drop(container);
        assert!(!scratch_path.exists());
    }

    #[tokio::test]
    async fn compiler_worker_retains_scratch_until_blocking_work_finishes() {
        let scratch = tempfile::tempdir().unwrap();
        let scratch_path = scratch.path().to_path_buf();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = spawn_compiler_worker(scratch, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        tokio::task::spawn_blocking(move || started_rx.recv_timeout(Duration::from_secs(5)))
            .await
            .unwrap()
            .unwrap();
        worker.abort();
        assert!(
            scratch_path.exists(),
            "worker-owned scratch was removed early"
        );
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while scratch_path.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("scratch was not removed after blocking work finished");
    }

    #[test]
    fn compiler_output_classification_separates_pass_source_and_infrastructure() {
        let passed = classify_compiler_output("biome check .", false, true, Some(0), "", "");
        assert_eq!(passed, CompilerCheckOutcome::Passed);

        let source = classify_compiler_output(
            "biome check .",
            false,
            false,
            Some(1),
            "src/main.ts(3,1): error TS2322: wrong type",
            "",
        );
        assert!(matches!(
            source,
            CompilerCheckOutcome::SourceDiagnostics { .. }
        ));

        let infrastructure = classify_compiler_output(
            "bunx biome check .",
            false,
            false,
            Some(1),
            "",
            "error: unable to write files to tempdir: PermissionDenied",
        );
        assert!(matches!(
            infrastructure,
            CompilerCheckOutcome::UnverifiedInfrastructure { .. }
        ));

        let empty_failure = classify_compiler_output("tsc --noEmit", false, false, Some(1), "", "");
        assert!(matches!(
            empty_failure,
            CompilerCheckOutcome::UnverifiedInfrastructure { .. }
        ));

        let missing_executable = classify_compiler_output(
            "biome check .",
            false,
            false,
            Some(127),
            "sh: biome: command not found",
            "",
        );
        assert!(matches!(
            missing_executable,
            CompilerCheckOutcome::UnverifiedInfrastructure { .. }
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn default_compiler_commands_can_write_outside_the_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let marker = outside.path().join("checker-marker.txt");
        let command = format!("printf checked > '{}'", marker.display());
        let outcome = run_compiler_command_outcome(
            workspace.path(),
            &command,
            false,
            std::time::Duration::from_secs(5),
            &CancellationToken::new(),
            crate::config::SandboxMode::default(),
        )
        .await;
        assert_eq!(outcome, CompilerCheckOutcome::Passed);
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "checked");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn restricted_compiler_commands_still_enforce_writable_roots() {
        if !crate::tools::exec::sandbox::runtime_tests_available() {
            return;
        }
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let marker = outside.path().join("checker-marker.txt");
        let command = format!("printf checked > '{}'", marker.display());
        let outcome = run_compiler_command_outcome(
            workspace.path(),
            &command,
            false,
            std::time::Duration::from_secs(5),
            &CancellationToken::new(),
            crate::config::SandboxMode::WorkspaceWrite,
        )
        .await;
        assert!(matches!(
            outcome,
            CompilerCheckOutcome::UnverifiedInfrastructure { .. }
        ));
        assert!(!marker.exists());
    }

    #[test]
    fn only_recognized_source_diagnostics_affect_the_diagnostic_streak() {
        let rustc_json = r#"{"reason":"compiler-message","message":{"level":"error","rendered":"error[E0425]: cannot find value `missing` in this scope\n --> src/lib.rs:3:5\n"}}"#;
        let cases = [
            (
                "rustc JSON diagnostic",
                "cargo check",
                true,
                rustc_json,
                "",
                true,
            ),
            (
                "rustc rendered diagnostic",
                "rustc",
                false,
                "error[E0425]: cannot find value `missing` in this scope\n --> src/lib.rs:3:5",
                "",
                true,
            ),
            (
                "rustc uncoded syntax diagnostic",
                "rustc",
                false,
                "error: unexpected closing delimiter: `}`\n --> src/lib.rs:3:1",
                "",
                true,
            ),
            (
                "TypeScript diagnostic",
                "tsc --noEmit",
                false,
                "src/main.ts(3,1): error TS2322: wrong type",
                "",
                true,
            ),
            (
                "Biome lint diagnostic",
                "biome check .",
                false,
                "src/main.ts:3:1 lint/suspicious/noConsole ━━━━━\n× Avoid console output",
                "",
                true,
            ),
            (
                "manifest parse failure",
                "cargo check",
                true,
                "",
                "error: failed to parse manifest at Cargo.toml",
                false,
            ),
            (
                "dependency resolution failure",
                "cargo check",
                true,
                "",
                "error: failed to get `serde` as a dependency of package `app`",
                false,
            ),
            (
                "toolchain setup failure",
                "cargo check",
                true,
                "",
                "error: toolchain `nightly-x` is not installed",
                false,
            ),
            (
                "linter configuration failure",
                "biome check .",
                false,
                "",
                "Invalid configuration: unknown key `formatter`",
                false,
            ),
            (
                "network download failure",
                "cargo check",
                true,
                "",
                "error: failed to download from https://example.test; connection timed out",
                false,
            ),
            (
                "unknown nonzero failure",
                "compiler check",
                false,
                "process failed for an unknown reason",
                "",
                false,
            ),
        ];

        for (name, command, cargo, stdout, stderr, is_source) in cases {
            let outcome = classify_compiler_output(command, cargo, false, Some(1), stdout, stderr);
            assert_eq!(
                matches!(outcome, CompilerCheckOutcome::SourceDiagnostics { .. }),
                is_source,
                "incorrect classification for {name}: {outcome:?}"
            );

            let mut ctx = TurnContext::new();
            ctx.compiler.consecutive_diagnostics = 2;
            ctx.compiler.last_diagnostic_fingerprint = Some("previous source failure".into());
            update_compiler_outcome_streak(&mut ctx, &outcome);
            assert_eq!(
                ctx.compiler.consecutive_diagnostics,
                if is_source { 1 } else { 2 },
                "incorrect diagnostic accounting for {name}"
            );
            if !is_source {
                assert_eq!(
                    ctx.compiler.last_diagnostic_fingerprint.as_deref(),
                    Some("previous source failure"),
                    "infrastructure outcome cleared fingerprint for {name}"
                );
            }
        }
    }

    #[test]
    fn source_locations_accept_spaces_without_matching_infrastructure_lines() {
        assert!(has_recognized_source_diagnostic(
            "src/my file.ts(3,1): error TS2322: wrong type"
        ));
        assert_eq!(
            compiler_diagnostic_locations("src/my file.ts(3,1): error TS2322: wrong type"),
            vec![("src/my file.ts".to_owned(), 3, 1)]
        );
        assert!(has_recognized_source_diagnostic(
            "src/my file.ts:3:1 lint/suspicious/noConsole ━━━━━"
        ));
        assert!(has_recognized_source_diagnostic(
            "tests/my file.ts(3,1): error TS2322: wrong type"
        ));
        assert!(has_recognized_source_diagnostic(
            "crates/tool/src/my file.ts:3:1 lint/suspicious/noConsole"
        ));
        assert!(has_recognized_source_diagnostic(
            r"C:\repo\src\my file.ts(3,1): error TS2322: wrong type"
        ));
        assert!(has_recognized_source_diagnostic(
            r"C:\repo\src\my file.ts:3:1 lint/suspicious/noConsole"
        ));
        assert!(has_recognized_source_diagnostic(
            "my file.ts(3,1): error TS2322: wrong type"
        ));
        assert!(has_recognized_source_diagnostic(
            "my file.ts:3:1 lint/suspicious/noConsole"
        ));
        assert!(!has_recognized_source_diagnostic(
            "error: failed to create temporary directory at /tmp/my build: PermissionDenied"
        ));
        assert!(!has_recognized_source_diagnostic(
            "warning: cache at /tmp/my file.ts:3:1 could not be opened"
        ));
        assert!(!has_recognized_source_diagnostic(
            "error TS2322: wrong type\nfoo(3,1): PermissionDenied"
        ));
        assert!(!has_recognized_source_diagnostic(
            "error: failed to write temp file:3:1 lint/suspicious/noConsole"
        ));
        assert!(!has_recognized_source_diagnostic(
            "runner failed tests/my file.ts:3:1 lint/suspicious/noConsole"
        ));
        assert!(!has_recognized_source_diagnostic(
            "build failed because temporary files could not be written"
        ));
        assert!(!has_recognized_source_diagnostic(
            "foo(3,1): error TS2322: wrong type"
        ));
    }

    #[test]
    fn verification_identity_invalidates_generation_root_command_environment_and_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("lib.rs");
        std::fs::write(&source, "pub fn first() {}\n").unwrap();
        let mode = crate::config::SandboxMode::Trusted;
        let command = "cargo check";
        let initial = verification_identity(dir.path(), command, mode).unwrap();
        record_verification(initial.clone(), "successful output".into());
        assert!(
            cached_verification(&initial)
                .unwrap()
                .contains("[CACHED VERIFICATION]")
        );
        assert_ne!(
            Some(initial.clone()),
            verification_identity(dir.path(), "cargo test", mode)
        );
        assert_ne!(
            Some(initial.clone()),
            verification_identity(
                dir.path(),
                command,
                crate::config::SandboxMode::WorkspaceWrite
            )
        );
        let other = tempfile::tempdir().unwrap();
        assert_ne!(
            Some(initial.clone()),
            verification_identity(other.path(), command, mode)
        );
        let environment = verification_identity_with_environment(
            dir.path(),
            command,
            mode,
            vec![("RUSTFLAGS".into(), "-Dwarnings".into())],
        )
        .unwrap();
        assert_ne!(initial, environment);
        std::fs::create_dir(dir.path().join(".cargo")).unwrap();
        std::fs::write(
            dir.path().join(".cargo/config.toml"),
            "[build]\nrustflags = []\n",
        )
        .unwrap();
        let configured = verification_identity(dir.path(), command, mode).unwrap();
        assert_ne!(initial, configured);
        assert!(cached_verification(&configured).is_none());
        std::fs::write(&source, "pub fn other() {}\n").unwrap();
        assert!(
            cached_verification(&verification_identity(dir.path(), command, mode).unwrap())
                .is_none()
        );
        // Publishing a receipt after a concurrent edit must be rejected.
        record_verification(configured.clone(), "stale output".into());
        assert!(cached_verification(&configured).is_none());
    }

    #[test]
    fn automatic_checker_and_shell_environment_receipts_are_separate() {
        let dir = tempfile::tempdir().unwrap();
        let mode = crate::config::SandboxMode::Trusted;
        let command = "cargo check --message-format=json";
        let shell = verification_identity(dir.path(), command, mode).unwrap();
        let checker = compiler_verification_identity(dir.path(), command, mode).unwrap();
        assert_ne!(shell, checker);
        record_verification(shell.clone(), "shell output".into());
        assert!(cached_verification(&shell).is_some());
        assert!(cached_verification(&checker).is_none());
    }

    #[test]
    fn verification_identity_rejects_shell_composition_and_external_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let mode = crate::config::SandboxMode::Trusted;
        for command in [
            "cargo check && touch marker",
            "cargo test; echo success",
            "cargo check --manifest-path ../Cargo.toml",
            "cargo check --config x.toml",
            "echo success",
        ] {
            assert!(
                verification_identity(dir.path(), command, mode).is_none(),
                "{command}"
            );
        }
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[dependencies]\nexternal = { path = \"../external\" }\n",
        )
        .unwrap();
        assert!(verification_identity(dir.path(), "cargo check", mode).is_none());
    }

    #[tokio::test]
    async fn cancellation_does_not_reuse_success_receipts() {
        let dir = tempfile::tempdir().unwrap();
        let mode = crate::config::SandboxMode::Trusted;
        let command = "cargo check";
        let identity = compiler_verification_identity(dir.path(), command, mode).unwrap();
        record_verification(identity, String::new());
        let token = CancellationToken::new();
        token.cancel();
        let outcome = run_compiler_command_outcome(
            dir.path(),
            command,
            true,
            Duration::from_secs(2),
            &token,
            mode,
        )
        .await;
        assert!(matches!(
            outcome,
            CompilerCheckOutcome::UnverifiedInfrastructure { .. }
        ));
    }

    #[tokio::test]
    #[ignore = "manual repeated-verification benchmark"]
    async fn benchmark_repeated_verification() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"verification_benchmark\"\nversion = \"0.1.0\"\n[lib]\npath = \"lib.rs\"\n[workspace]\n").unwrap();
        std::fs::write(dir.path().join("lib.rs"), "pub fn example() {}\n").unwrap();
        let token = CancellationToken::new();
        let command = "cargo check --message-format=json";
        let cold = std::time::Instant::now();
        assert_eq!(
            run_compiler_command_outcome(
                dir.path(),
                command,
                true,
                Duration::from_secs(120),
                &token,
                crate::config::SandboxMode::Trusted
            )
            .await,
            CompilerCheckOutcome::Passed
        );
        eprintln!(
            "verification benchmark cold_ms={}",
            cold.elapsed().as_millis()
        );
        let repeat = std::time::Instant::now();
        for _ in 0..20 {
            assert_eq!(
                run_compiler_command_outcome(
                    dir.path(),
                    command,
                    true,
                    Duration::from_secs(120),
                    &token,
                    crate::config::SandboxMode::Trusted
                )
                .await,
                CompilerCheckOutcome::Passed
            );
        }
        eprintln!(
            "verification benchmark repeated_20_ms={}",
            repeat.elapsed().as_millis()
        );
    }

    #[tokio::test]
    async fn manifest_parse_failure_is_unverified_and_not_cached_as_passed() {
        if !crate::tools::exec::sandbox::runtime_tests_available() {
            return;
        }
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"invalid_fixture\"\nversion = \"not-a-version\"\n[workspace]\n",
        )
        .unwrap();
        let token = CancellationToken::new();
        let mut dirty = true;
        let mut cache = None;
        let result = cached_compiler_check(
            project.path(),
            &mut dirty,
            &mut cache,
            &token,
            crate::config::SandboxMode::default(),
        )
        .await
        .unwrap();
        assert!(result.starts_with("__BUILD_UNVERIFIED__"), "{result}");
        assert!(result.contains("status 101"), "{result}");
        assert!(result.contains("not-a-version"), "{result}");
        assert!(dirty);
        assert!(cache.is_none());
    }

    #[tokio::test]
    async fn source_failures_are_never_cached_as_verification() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("Cargo.toml"), "[package]\nname = \"invalid_source_fixture\"\nversion = \"0.1.0\"\n[lib]\npath = \"lib.rs\"\n[workspace]\n").unwrap();
        std::fs::write(
            project.path().join("lib.rs"),
            "pub fn invalid() { missing(); }\n",
        )
        .unwrap();
        let mut dirty = true;
        let mut cache = None;
        let result = cached_compiler_check(
            project.path(),
            &mut dirty,
            &mut cache,
            &CancellationToken::new(),
            crate::config::SandboxMode::Trusted,
        )
        .await;
        assert!(result.is_some());
        assert!(dirty);
        assert!(cache.is_none());
    }

    #[tokio::test]
    async fn successful_cargo_check_is_cached_as_passed() {
        if !crate::tools::exec::sandbox::runtime_tests_available() {
            return;
        }
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("Cargo.toml"), "[package]\nname = \"valid_compiler_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\npath = \"lib.rs\"\n[workspace]\n").unwrap();
        std::fs::write(project.path().join("lib.rs"), "pub fn valid() {}\n").unwrap();
        let token = CancellationToken::new();
        let mut dirty = true;
        let mut cache = None;
        assert!(
            cached_compiler_check(
                project.path(),
                &mut dirty,
                &mut cache,
                &token,
                crate::config::SandboxMode::default()
            )
            .await
            .is_none()
        );
        assert!(!dirty);
        assert_eq!(cache, Some((project.path().to_owned(), None)));

        // A shared-cache hit is still a successful check. The cache receipt is
        // operational metadata, not a compiler diagnostic for callers.
        let identity = compiler_verification_identity(
            project.path(),
            "cargo check --message-format=json",
            crate::config::SandboxMode::default(),
        )
        .unwrap();
        record_verification(identity, "cached compiler output".to_string());
        let mut cached_dirty = true;
        let mut cached_turn_cache = None;
        assert!(
            cached_compiler_check(
                project.path(),
                &mut cached_dirty,
                &mut cached_turn_cache,
                &token,
                crate::config::SandboxMode::default()
            )
            .await
            .is_none()
        );
        assert!(!cached_dirty);
        assert_eq!(cached_turn_cache, Some((project.path().to_owned(), None)));
    }

    #[tokio::test]
    async fn cancelled_check_does_not_reuse_clean_cache() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        let token = CancellationToken::new();
        token.cancel();
        let mut dirty = false;
        let mut cache = Some((project.path().to_owned(), None));
        let result = cached_compiler_check(
            project.path(),
            &mut dirty,
            &mut cache,
            &token,
            crate::config::SandboxMode::default(),
        )
        .await
        .unwrap();
        assert!(result.starts_with("__BUILD_UNVERIFIED__"));
        assert!(result.contains("cancelled"));
        assert!(dirty);
        assert!(cache.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nonzero_without_diagnostics_is_failure_and_zero_is_passed() {
        if !crate::tools::exec::sandbox::runtime_tests_available() {
            return;
        }
        let project = tempfile::tempdir().unwrap();
        let token = CancellationToken::new();
        let failure = run_compiler_command(
            project.path(),
            "exit 9",
            false,
            Duration::from_secs(5),
            &token,
        )
        .await
        .unwrap();
        assert!(failure.starts_with("__BUILD_UNVERIFIED__"), "{failure}");
        assert!(failure.contains("without output"));
        assert!(
            run_compiler_command(
                project.path(),
                "exit 0",
                false,
                Duration::from_secs(5),
                &token
            )
            .await
            .is_none()
        );
    }

    #[cfg(unix)]
    async fn assert_compiler_tree_cleanup(cancel: bool) {
        let project = tempfile::tempdir().unwrap();
        let token = CancellationToken::new();
        let timeout = if cancel {
            Duration::from_secs(10)
        } else {
            Duration::from_millis(500)
        };
        let command =
            "(sleep 2; printf survived > descendant-marker) & printf ready > started; wait";
        let execution = run_compiler_command(project.path(), command, false, timeout, &token);
        let trigger = async {
            if cancel {
                tokio::time::timeout(Duration::from_secs(5), async {
                    while !project.path().join("started").exists() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .unwrap();
                token.cancel();
            }
        };
        let (result, _) = tokio::time::timeout(Duration::from_secs(6), async {
            tokio::join!(execution, trigger)
        })
        .await
        .unwrap();
        let result = result.unwrap();
        assert!(result.starts_with("__BUILD_UNVERIFIED__"), "{result}");
        assert!(
            result.contains(if cancel { "cancelled" } else { "timed out" }),
            "{result}"
        );
        assert!(project.path().join("started").exists());
        tokio::time::sleep(Duration::from_millis(2200)).await;
        assert!(
            !project.path().join("descendant-marker").exists(),
            "compiler descendant survived termination"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn compiler_timeout_kills_descendants() {
        if !crate::tools::exec::sandbox::runtime_tests_available() {
            return;
        }
        assert_compiler_tree_cleanup(false).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn compiler_cancellation_kills_descendants() {
        if !crate::tools::exec::sandbox::runtime_tests_available() {
            return;
        }
        assert_compiler_tree_cleanup(true).await;
    }
}

pub(crate) fn compiler_diagnostic_fingerprint(content: &str) -> Option<String> {
    let diagnostics = content.split_once(COMPILER_DIAGNOSTIC_MARKER)?.1.trim();
    if diagnostics.is_empty() {
        return None;
    }
    let normalized = diagnostics.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalized.is_empty()).then_some(normalized)
}

pub(crate) fn update_compiler_diagnostic_streak(
    ctx: &mut TurnContext,
    fingerprint: Option<String>,
) {
    match fingerprint {
        Some(fingerprint)
            if ctx.compiler.last_diagnostic_fingerprint.as_deref()
                == Some(fingerprint.as_str()) =>
        {
            ctx.compiler.consecutive_diagnostics += 1;
        }
        Some(fingerprint) => {
            ctx.compiler.last_diagnostic_fingerprint = Some(fingerprint);
            ctx.compiler.consecutive_diagnostics = 1;
        }
        None => {
            ctx.compiler.last_diagnostic_fingerprint = None;
            ctx.compiler.consecutive_diagnostics = 0;
        }
    }
}

pub(crate) fn update_compiler_outcome_streak(
    ctx: &mut TurnContext,
    outcome: &CompilerCheckOutcome,
) {
    match outcome {
        CompilerCheckOutcome::Passed => update_compiler_diagnostic_streak(ctx, None),
        CompilerCheckOutcome::SourceDiagnostics { fingerprint, .. } => {
            update_compiler_diagnostic_streak(ctx, Some(fingerprint.clone()));
        }
        CompilerCheckOutcome::UnverifiedInfrastructure { .. } => {}
    }
}
