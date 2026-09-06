use super::TurnContext;
use super::events::ToolResult;
use super::text::strip_ansi_escapes;
use crate::platform::{compiler_augmented_path, resolve_bin};
use regex::Regex;
use std::sync::{Arc, LazyLock};
use tokio_util::sync::CancellationToken;

pub(crate) async fn run_compiler_check(
    cwd: &std::path::Path,
    cancel_token: &CancellationToken,
) -> Option<String> {
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
        return None;
    };
    run_compiler_command(
        cwd,
        command,
        cargo,
        std::time::Duration::from_secs(timeout),
        cancel_token,
    )
    .await
}

async fn run_compiler_command(
    cwd: &std::path::Path,
    command: &str,
    cargo: bool,
    timeout: std::time::Duration,
    cancel_token: &CancellationToken,
) -> Option<String> {
    let unverified = |reason: &str| {
        Some(format!(
            "__BUILD_UNVERIFIED__: `{command}` {reason}. The build was NOT verified — do not claim the task compiles."
        ))
    };
    if cancel_token.is_cancelled() {
        return unverified("was cancelled");
    }
    // A child token also stops the blocking worker if this async future is dropped.
    let worker_token = cancel_token.child_token();
    let _cancel_on_drop = worker_token.clone().drop_guard();
    let request = rustcode_command::CommandRequest {
        command: command.to_owned(),
        cwd: Some(cwd.to_path_buf()),
        env: vec![("PATH".into(), compiler_augmented_path().into())],
        timeout,
        process_group: true,
    };
    let output = tokio::task::spawn_blocking(move || {
        rustcode_command::run_with_timeout_cancellable(
            &request,
            None,
            Some(Arc::new(move || worker_token.is_cancelled())),
        )
    })
    .await;
    match output {
        Ok(Ok(output)) => compiler_output_diagnostics(command, cargo, &output),
        Ok(Err(error)) => unverified(&error),
        Err(error) => unverified(&format!("could not complete ({error})")),
    }
}

fn compiler_output_diagnostics(
    command: &str,
    cargo: bool,
    output: &rustcode_command::CommandOutput,
) -> Option<String> {
    let stdout = String::from_utf8_lossy(output.stdout.bytes());
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
            return Some(errors.join("\n"));
        }
    }
    if output.success {
        return None;
    }
    // Cargo's manifest, dependency, and toolchain failures often have no JSON
    // diagnostic. A nonzero exit must never be cached or reported as a pass.
    let stderr = String::from_utf8_lossy(output.stderr.bytes());
    let diagnostics = strip_ansi_escapes(&format!("{stdout}\n{stderr}"));
    let mut diagnostics = diagnostics.trim().to_owned();
    const MAX_DIAGNOSTIC_BYTES: usize = 16 * 1024;
    if diagnostics.len() > MAX_DIAGNOSTIC_BYTES {
        let mut end = MAX_DIAGNOSTIC_BYTES;
        while !diagnostics.is_char_boundary(end) {
            end -= 1;
        }
        diagnostics.truncate(end);
        diagnostics.push_str("\n[compiler diagnostics truncated]");
    }
    let status = output.exit_code.map_or_else(
        || "terminated without an exit code".to_owned(),
        |code| format!("exited with status {code}"),
    );
    Some(
        format!("`{command}` {status}.\n{diagnostics}")
            .trim_end()
            .to_owned(),
    )
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
    result
        .content
        .push_str("\n\nLSP/Compiler errors detected in workspace, please fix:\n");
    result
        .content
        .push_str(&compiler_diagnostics_with_snippets(diagnostics));
    result.metadata.error_kind = Some(crate::tools::ToolErrorKind::CompilerFailed);
    result.metadata.retryable = true;
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

    #[tokio::test]
    async fn stderr_only_cargo_failure_is_not_cached_as_passed() {
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
        assert!(result.contains("status 101"), "{result}");
        assert!(result.contains("not-a-version"), "{result}");
        assert!(!result.starts_with("__BUILD_UNVERIFIED__"));
        assert!(!dirty);
        assert_eq!(
            cached_compiler_check(project.path(), &mut dirty, &mut cache, &token).await,
            Some(result)
        );
    }

    #[tokio::test]
    async fn successful_cargo_check_is_cached_as_passed() {
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
        assert!(failure.contains("status 9"));
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
        assert_compiler_tree_cleanup(false).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn compiler_cancellation_kills_descendants() {
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
