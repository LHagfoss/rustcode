use super::TurnContext;
use super::events::ToolResult;
use super::text::strip_ansi_escapes;
use crate::platform::{compiler_augmented_path, resolve_bin};
use regex::Regex;
use std::sync::{Arc, LazyLock};
use tokio_util::sync::CancellationToken;

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
) -> Option<String> {
    let (outcome, command) = run_compiler_check_with_command(cwd, cancel_token).await;
    outcome.legacy_output(command)
}

async fn run_compiler_check_with_command(
    cwd: &std::path::Path,
    cancel_token: &CancellationToken,
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
    run_compiler_command_outcome(cwd, command, cargo, timeout, cancel_token)
        .await
        .legacy_output(command)
}

pub(super) async fn run_compiler_command_outcome(
    cwd: &std::path::Path,
    command: &str,
    cargo: bool,
    timeout: std::time::Duration,
    cancel_token: &CancellationToken,
) -> CompilerCheckOutcome {
    let unverified = |reason: String| CompilerCheckOutcome::UnverifiedInfrastructure { reason };
    if cancel_token.is_cancelled() {
        return record_unverified_event(unverified("was cancelled".to_string()), command);
    }
    let scratch_container = match tempfile::Builder::new()
        .prefix("rustcode-compiler-check-")
        .tempdir()
    {
        Ok(scratch) => scratch,
        Err(error) => {
            return record_unverified_event(
                unverified(format!(
                    "could not create checker scratch directory ({error})"
                )),
                command,
            );
        }
    };
    let scratch_path = scratch_container.path().join("sandbox");
    if let Err(error) = std::fs::create_dir(&scratch_path) {
        return record_unverified_event(
            unverified(format!(
                "could not create checker scratch directory ({error})"
            )),
            command,
        );
    }
    let writable_roots = [cwd.to_path_buf(), scratch_path.clone()];
    let session_scratch_roots = [scratch_path.clone()];
    let command_for_exec = match crate::tools::exec::sandbox::command(
        command,
        crate::tools::exec::sandbox::SandboxPolicy {
            command_cwd: Some(cwd),
            workspace_root: Some(cwd),
            writable_roots: &writable_roots,
            session_scratch_roots: &session_scratch_roots,
            one_shot_writable_roots: &[],
            write_access: true,
            network_access: false,
        },
    ) {
        Ok(command) => command,
        Err(error) => return record_unverified_event(unverified(error.to_string()), command),
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
    let output = tokio::task::spawn_blocking(move || {
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
    record_unverified_event(outcome, command)
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
    static BIOME_LOCATION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\S+:\d+:\d+\s+(?:lint|assist|format)/").unwrap());
    static RUST_SOURCE_LOCATION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*-->\s+\S+\.rs:\d+:\d+").unwrap());
    let lower = output.to_lowercase();
    let has_location = !compiler_diagnostic_locations(output).is_empty();
    let has_rust_source_location = output
        .lines()
        .any(|line| RUST_SOURCE_LOCATION.is_match(line));
    let rust_diagnostic = has_rust_source_location
        && (lower.contains("error[")
            || lower
                .lines()
                .any(|line| line.trim_start().starts_with("error:")));
    let typescript_diagnostic = lower.contains("error ts") && has_location;
    let biome_diagnostic = output.lines().any(|line| BIOME_LOCATION.is_match(line));
    rust_diagnostic || typescript_diagnostic || biome_diagnostic
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
) -> Option<String> {
    if !cancel_token.is_cancelled()
        && !*dirty
        && let Some((cached_root, cached_result)) = cache.as_ref()
        && cached_root == root
    {
        dbg_log!("Compiler check: reusing cached result (tree unchanged since last check)");
        return cached_result.clone();
    }
    let result = run_compiler_check(root, cancel_token).await;
    if result
        .as_deref()
        .is_some_and(|text| text.starts_with("__BUILD_UNVERIFIED__"))
    {
        *dirty = true;
        *cache = None;
        return result;
    }
    *cache = Some((root.to_path_buf(), result.clone()));
    *dirty = false;
    result
}

pub(crate) fn append_compiler_diagnostics(result: &mut ToolResult, diagnostics: &str) {
    if diagnostics.starts_with("__BUILD_UNVERIFIED__") {
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
            result.metadata.error_kind = Some(crate::tools::ToolErrorKind::CompilerFailed);
            result.metadata.retryable = true;
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
        LazyLock::new(|| Regex::new(r"^(\S+)\((\d+),(\d+)\):").unwrap());
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
        let result = cached_compiler_check(project.path(), &mut dirty, &mut cache, &token)
            .await
            .unwrap();
        assert!(result.starts_with("__BUILD_UNVERIFIED__"), "{result}");
        assert!(result.contains("status 101"), "{result}");
        assert!(result.contains("not-a-version"), "{result}");
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
            cached_compiler_check(project.path(), &mut dirty, &mut cache, &token)
                .await
                .is_none()
        );
        assert!(!dirty);
        assert_eq!(cache, Some((project.path().to_owned(), None)));
    }

    #[tokio::test]
    async fn cancelled_check_does_not_reuse_clean_cache() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        let token = CancellationToken::new();
        token.cancel();
        let mut dirty = false;
        let mut cache = Some((project.path().to_owned(), None));
        let result = cached_compiler_check(project.path(), &mut dirty, &mut cache, &token)
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
