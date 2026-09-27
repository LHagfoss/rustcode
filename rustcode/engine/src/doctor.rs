//! `rustcode doctor` — environment diagnostics with optional `--fix`.
//!
//! Checks the local harness prerequisites without network access:
//! config directory, config load, required binaries (`git`, `rg`, `tmux`),
//! and skill directories. `--fix` creates missing directories; binaries are
//! never auto-installed (we print the install hint instead).

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
    pub fix_hint: Option<String>,
}

impl DoctorCheck {
    fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            ok: true,
            detail: detail.into(),
            fix_hint: None,
        }
    }

    fn fail(name: &'static str, detail: impl Into<String>, fix_hint: Option<String>) -> Self {
        Self {
            name,
            ok: false,
            detail: detail.into(),
            fix_hint,
        }
    }
}

fn which_binary(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn binary_version(binary: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(binary)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().next().map(|line| line.trim().to_string())
}

fn check_binary(name: &'static str, version_args: &[&str], install_hint: &str) -> DoctorCheck {
    match which_binary(name) {
        Some(path) => {
            let detail = binary_version(name, version_args)
                .map(|v| format!("{} ({})", path.display(), v))
                .unwrap_or_else(|| path.display().to_string());
            DoctorCheck::pass(name, detail)
        }
        None => DoctorCheck::fail(name, "not found in PATH", Some(install_hint.to_string())),
    }
}

fn ensure_dir(path: &PathBuf) -> bool {
    if path.is_dir() {
        return true;
    }
    std::fs::create_dir_all(path).is_ok() && path.is_dir()
}

/// Check each configured profile's API-key env var and report where it
/// resolved from (process env vs login shell vs dotfile vs missing).
/// Never prints values.
fn check_provider_keys(workspace: &std::path::Path) -> Vec<DoctorCheck> {
    let config = crate::config::load_config_for_workspace(workspace).2;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for profile in &config.models {
        let Some(env_name) = profile.api_key_env_name() else {
            continue;
        };
        if !seen.insert(env_name.to_string()) {
            continue;
        }
        match crate::shell_env::env_var_source(env_name) {
            crate::shell_env::EnvSource::Process => out.push(DoctorCheck::pass(
                "provider-key",
                format!("{env_name}: found in process env"),
            )),
            crate::shell_env::EnvSource::LoginShell => out.push(DoctorCheck::pass(
                "provider-key",
                format!("{env_name}: found via login shell (exported in shell rc, not in process env)"),
            )),
            crate::shell_env::EnvSource::Dotfile => out.push(DoctorCheck::pass(
                "provider-key",
                format!("{env_name}: found in shell dotfile (shell probe unavailable; parsed statically)"),
            )),
            crate::shell_env::EnvSource::Missing => out.push(DoctorCheck::fail(
                "provider-key",
                format!("{env_name}: missing (profile `{}` needs it)", profile.name),
                Some(format!(
                    "export {env_name}=<key> in ~/.zshenv (sourced by all zsh modes) or ~/.zprofile, then restart rustcode from a login shell; interactive-only ~/.zshrc exports are invisible to desktop/systemd/IDE launches"
                )),
            )),
        }
    }
    out
}

/// Run all checks. When `fix` is true, create missing config/skill
/// directories before reporting.
pub fn run_checks(fix: bool) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();

    // Config directory.
    match crate::config::get_config_dir() {
        Some(dir) => {
            if dir.is_dir() {
                checks.push(DoctorCheck::pass("config-dir", dir.display().to_string()));
            } else if fix && ensure_dir(&dir) {
                checks.push(DoctorCheck::pass(
                    "config-dir",
                    format!("{} (created by --fix)", dir.display()),
                ));
            } else {
                checks.push(DoctorCheck::fail(
                    "config-dir",
                    format!("missing: {}", dir.display()),
                    Some(format!(
                        "run `rustcode doctor --fix` or `mkdir -p {}`",
                        dir.display()
                    )),
                ));
            }
        }
        None => checks.push(DoctorCheck::fail(
            "config-dir",
            "configuration directory is unavailable",
            None,
        )),
    }

    // Config load.
    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (api_base, model_name, _) = crate::config::load_config_for_workspace(&workspace);
    checks.push(DoctorCheck::pass(
        "config-load",
        format!("model={model_name} base={api_base}"),
    ));

    // Provider API keys: report *where* each key resolved from so
    // "works in my terminal but not in rustcode" is diagnosable. Values are
    // never printed.
    checks.extend(check_provider_keys(&workspace));

    // Binaries.
    checks.push(check_binary(
        "git",
        &["--version"],
        "install git: https://git-scm.com/downloads",
    ));
    checks.push(check_binary(
        "rg",
        &["--version"],
        "install ripgrep: `brew install ripgrep` / `cargo install ripgrep`",
    ));
    checks.push(check_binary(
        "tmux",
        &["-V"],
        "install tmux for background tasks: `brew install tmux`",
    ));

    // Skill directories (informational; --fix creates the global one).
    let home = std::env::var("HOME").map(PathBuf::from).ok();
    if let Some(home) = home {
        let global = home.join(".config/rustcode/skills");
        if global.is_dir() {
            checks.push(DoctorCheck::pass(
                "skills-dir",
                global.display().to_string(),
            ));
        } else if fix && ensure_dir(&global) {
            checks.push(DoctorCheck::pass(
                "skills-dir",
                format!("{} (created by --fix)", global.display()),
            ));
        } else {
            checks.push(DoctorCheck::fail(
                "skills-dir",
                format!("missing: {}", global.display()),
                Some("run `rustcode doctor --fix` to create it".to_string()),
            ));
        }
        let local = workspace.join(".rustcode/skills");
        if local.is_dir() {
            checks.push(DoctorCheck::pass(
                "project-skills",
                local.display().to_string(),
            ));
        } else {
            // Missing project skills is fine — most repos don't have one.
            checks.push(DoctorCheck::pass(
                "project-skills",
                "no .rustcode/skills in this workspace (optional)".to_string(),
            ));
        }
    }

    checks
}

pub fn format_report(checks: &[DoctorCheck]) -> String {
    let mut out = String::from("rustcode doctor\n");
    for check in checks {
        let status = if check.ok { "ok  " } else { "FAIL" };
        out.push_str(&format!("  [{status}] {}: {}", check.name, check.detail));
        if !check.ok
            && let Some(hint) = &check.fix_hint
        {
            out.push_str(&format!("\n         hint: {hint}"));
        }
        out.push('\n');
    }
    let failures = checks.iter().filter(|c| !c.ok).count();
    if failures == 0 {
        out.push_str("All checks passed.\n");
    } else {
        out.push_str(&format!(
            "{failures} check(s) failed. Re-run with `--fix` to create missing directories.\n"
        ));
    }
    out
}

/// Entry point for `rustcode doctor [--fix]`. Returns a process exit code.
pub fn run_doctor(fix: bool) -> i32 {
    let checks = run_checks(fix);
    print!("{}", format_report(&checks));
    if checks.iter().all(|c| c.ok) { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_marks_pass_and_fail() {
        let checks = vec![
            DoctorCheck::pass("a", "fine"),
            DoctorCheck::fail("b", "broken", Some("fix it".to_string())),
        ];
        let report = format_report(&checks);
        assert!(report.contains("[ok  ] a: fine"));
        assert!(report.contains("[FAIL] b: broken"));
        assert!(report.contains("hint: fix it"));
        assert!(report.contains("1 check(s) failed"));
    }

    #[test]
    fn report_all_pass() {
        let checks = vec![DoctorCheck::pass("a", "fine")];
        assert!(format_report(&checks).contains("All checks passed."));
    }

    #[test]
    fn missing_binary_reports_install_hint() {
        let check = check_binary(
            "rustcode-definitely-missing-binary-xyz",
            &["--version"],
            "install it somehow",
        );
        assert!(!check.ok);
        assert_eq!(check.fix_hint.as_deref(), Some("install it somehow"));
    }

    #[test]
    fn run_checks_does_not_panic() {
        let checks = run_checks(false);
        assert!(checks.iter().any(|c| c.name == "config-load"));
        assert!(checks.iter().any(|c| c.name == "git"));
    }
}
