use super::TurnContext;
use super::events::ToolResult;
use super::text::strip_ansi_escapes;
use regex::Regex;
use std::sync::LazyLock;

fn augmented_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let common = [
        format!("{home}/.cargo/bin"),
        format!("{home}/.bun/bin"),
        format!("{home}/.nvm/versions/node/current/bin"),
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
    ];
    let mut dirs = common.to_vec();
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if !dirs.contains(&dir.to_string()) {
                dirs.push(dir.to_string());
            }
        }
    }
    dirs.join(":")
}

pub(crate) async fn run_compiler_check(cwd: &std::path::Path) -> Option<String> {
    if cwd.join("Cargo.toml").exists() {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.args(["-c", "cargo check --message-format=json"])
            .current_dir(cwd)
            .env("PATH", augmented_path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                dbg_log!("Could not spawn cargo check ({e}), skipping compiler check");
                return Some(format!(
                    "__BUILD_UNVERIFIED__: could not run `cargo check` ({e}). \
                     The build was NOT verified — do not claim the task compiles."
                ));
            }
        };

        let timeout_duration = std::time::Duration::from_secs(120);
        let output_res = tokio::time::timeout(timeout_duration, child.wait_with_output()).await;

        let output = match output_res {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                dbg_log!("cargo check failed to run ({e}), skipping compiler check");
                return Some(format!(
                    "__BUILD_UNVERIFIED__: `cargo check` failed to run ({e}). \
                     The build was NOT verified."
                ));
            }
            Err(_) => {
                dbg_log!("cargo check timed out, skipping compiler check");
                return Some(
                    "__BUILD_UNVERIFIED__: `cargo check` timed out. \
                     The build was NOT verified."
                        .to_string(),
                );
            }
        };

        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let mut errors = Vec::new();

        for line in stdout_str.lines() {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line)
                && val.get("reason").and_then(|r| r.as_str()) == Some("compiler-message")
                && let Some(msg) = val.get("message")
                && let Some(level) = msg.get("level").and_then(|l| l.as_str())
                && level == "error"
                && let Some(rendered) = msg.get("rendered").and_then(|r| r.as_str())
            {
                errors.push(strip_ansi_escapes(rendered));
            }
        }

        if !errors.is_empty() {
            return Some(errors.join("\n"));
        }
    } else if cwd.join("biome.json").exists() || cwd.join("biome.jsonc").exists() {
        let (runner, bin_arg) = if super::resolve_bin("bunx").exists() {
            (super::resolve_bin("bunx"), "biome")
        } else {
            (super::resolve_bin("npx"), "@biomejs/biome")
        };

        let mut cmd = tokio::process::Command::new(runner);
        cmd.args([bin_arg, "check", "."])
            .current_dir(cwd)
            .env("PATH", augmented_path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                dbg_log!("Could not spawn biome check ({e}), skipping compiler check");
                return None;
            }
        };

        let timeout_duration = std::time::Duration::from_secs(60);
        let output_res = tokio::time::timeout(timeout_duration, child.wait_with_output()).await;

        let output = match output_res {
            Ok(Ok(out)) => out,
            Ok(Err(_)) | Err(_) => return None,
        };

        if !output.status.success() {
            let stdout_str = String::from_utf8_lossy(&output.stdout);
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{stdout_str}\n{stderr_str}");
            let trimmed = combined.trim();
            if !trimmed.is_empty() {
                return Some(strip_ansi_escapes(trimmed));
            }
        }
    } else if cwd.join("tsconfig.json").exists() {
        let (runner, bin_arg) = if super::resolve_bin("bunx").exists() {
            (super::resolve_bin("bunx"), "tsc")
        } else {
            (super::resolve_bin("npx"), "tsc")
        };

        let mut cmd = tokio::process::Command::new(runner);
        cmd.args([bin_arg, "--noEmit"])
            .current_dir(cwd)
            .env("PATH", augmented_path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                dbg_log!("Could not spawn {bin_arg} ({e}), skipping compiler check");
                return None;
            }
        };

        let timeout_duration = std::time::Duration::from_secs(60);
        let output_res = tokio::time::timeout(timeout_duration, child.wait_with_output()).await;

        let output = match output_res {
            Ok(Ok(out)) => out,
            Ok(Err(_)) | Err(_) => return None,
        };

        if !output.status.success() {
            let stdout_str = String::from_utf8_lossy(&output.stdout);
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{stdout_str}\n{stderr_str}");
            let trimmed = combined.trim();
            if !trimmed.is_empty() {
                return Some(strip_ansi_escapes(trimmed));
            }
        }
    }

    None
}

pub(crate) async fn cached_compiler_check(
    root: &std::path::Path,
    dirty: &mut bool,
    cache: &mut Option<(std::path::PathBuf, Option<String>)>,
) -> Option<String> {
    if !*dirty
        && let Some((cached_root, cached_result)) = cache.as_ref()
        && cached_root == root
    {
        dbg_log!("Compiler check: reusing cached result (tree unchanged since last check)");
        return cached_result.clone();
    }
    let result = run_compiler_check(root).await;
    *cache = Some((root.to_path_buf(), result.clone()));
    *dirty = false;
    result
}

pub(crate) fn append_compiler_diagnostics(result: &mut ToolResult, diagnostics: &str) {
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
            if ctx.last_compiler_diagnostic_fingerprint.as_deref()
                == Some(fingerprint.as_str()) =>
        {
            ctx.consecutive_compiler_diagnostics += 1;
        }
        Some(fingerprint) => {
            ctx.last_compiler_diagnostic_fingerprint = Some(fingerprint);
            ctx.consecutive_compiler_diagnostics = 1;
        }
        None => {
            ctx.last_compiler_diagnostic_fingerprint = None;
            ctx.consecutive_compiler_diagnostics = 0;
        }
    }
}
