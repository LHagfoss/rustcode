use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerificationKind {
    Check,
    Test,
    Format,
    Lint,
    Build,
    Command,
}

impl VerificationKind {
    fn label(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Test => "test",
            Self::Format => "format",
            Self::Lint => "lint",
            Self::Build => "build",
            Self::Command => "command",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerificationEvidence {
    pub command: String,
    pub kind: VerificationKind,
    pub exit_code: Option<i32>,
    pub generation: u64,
    pub scope: VerificationScope,
}

/// What a verification command ran against, as far as the harness knows.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct VerificationScope {
    /// Project roots, canonical. Empty when unknown.
    roots: Vec<PathBuf>,
    /// The workspace root and the revision the command left it at, kept only
    /// when that root contains every other one.
    workspace: Option<(PathBuf, u64)>,
}

impl VerificationScope {
    pub(crate) fn new(
        workspace_root: Option<&Path>,
        project_root: Option<&Path>,
        workspace_generation: Option<u64>,
    ) -> Self {
        let roots: Vec<PathBuf> = workspace_root
            .into_iter()
            .chain(project_root)
            .map(canonical)
            .collect();
        let workspace = roots
            .first()
            .filter(|root| workspace_root.is_some() && roots.iter().all(|r| r.starts_with(root)))
            .cloned()
            .zip(workspace_generation);
        Self { roots, workspace }
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VerificationLedger {
    generation: u64,
    last: Option<VerificationEvidence>,
    explicit_last: Option<VerificationEvidence>,
    /// Scope given to the commands recorded next.
    scope: VerificationScope,
    /// The edit that last advanced `generation`, when its path is known.
    stale_path: Option<String>,
}

impl VerificationLedger {
    pub(crate) fn record_edit(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.stale_path = None;
    }

    /// Record an edit to known files. `project_roots` are the roots in use
    /// now. An edit outside those and outside every root the last successful
    /// verification covered changes nothing that verification checked, so it
    /// stays fresh (#1889). A path that cannot be resolved counts as inside.
    pub(crate) fn record_edit_to(&mut self, paths: &[PathBuf], project_roots: &[PathBuf]) {
        let outside = self.has_fresh_successful_verification()
            && !paths.is_empty()
            && self.last.as_ref().is_some_and(|evidence| {
                let roots: Vec<PathBuf> = evidence
                    .scope
                    .roots
                    .iter()
                    .chain(project_roots)
                    .map(|root| canonical(root))
                    .collect();
                !evidence.scope.roots.is_empty()
                    && paths.iter().all(|path| {
                        path.canonicalize()
                            .is_ok_and(|path| !roots.iter().any(|root| path.starts_with(root)))
                    })
            });
        if outside {
            return;
        }
        self.record_edit();
        self.stale_path = paths.first().map(|path| path.display().to_string());
    }

    /// Set what the commands recorded from here on ran against.
    pub(crate) fn set_scope(&mut self, scope: VerificationScope) {
        self.scope = scope;
    }

    /// Whether the workspace is still at the revision the last successful
    /// verification left it at: the condition under which the verification
    /// cache would answer the same command without running it. `current`
    /// reports a root's revision now.
    pub(crate) fn verified_workspace_is_unchanged(
        &self,
        current: impl Fn(&Path) -> Option<u64>,
    ) -> bool {
        self.last.as_ref().is_some_and(|evidence| {
            evidence.exit_code == Some(0)
                && evidence
                    .scope
                    .workspace
                    .as_ref()
                    .is_some_and(|(root, generation)| current(root) == Some(*generation))
        })
    }

    /// Why the finish gate has no verification to accept, for the model.
    pub(crate) fn missing_verification_reason(&self) -> String {
        match &self.stale_path {
            Some(path) => {
                format!("No verification command was run after the latest edit ({path}).")
            }
            None => "No verification command was run after the latest edit.".to_string(),
        }
    }

    pub(crate) fn record_command(&mut self, command: &str, exit_code: Option<i32>) {
        let Some(kind) = classify_command(command) else {
            return;
        };
        self.last = Some(self.evidence(command, kind, exit_code));
    }

    pub(crate) fn record_explicit_command(&mut self, command: &str, exit_code: Option<i32>) {
        let kind = classify_command(command).unwrap_or(VerificationKind::Command);
        let evidence = self.evidence(command, kind, exit_code);
        self.last = Some(evidence.clone());
        self.explicit_last = Some(evidence);
    }

    pub(crate) fn has_fresh_successful_verification(&self) -> bool {
        self.last.as_ref().is_some_and(|evidence| {
            evidence.generation == self.generation && evidence.exit_code == Some(0)
        })
    }

    pub(crate) fn last_failure(&self) -> Option<&VerificationEvidence> {
        self.last.as_ref().filter(|evidence| {
            evidence.generation == self.generation && evidence.exit_code != Some(0)
        })
    }

    /// True when the same successful verification is being requested again
    /// without an intervening edit. Repeating a clean check adds no evidence;
    /// callers can turn this into targeted recovery instead of another blind
    /// verification round.
    pub(crate) fn is_repeated_successful_command(
        &self,
        command: &str,
        exit_code: Option<i32>,
    ) -> bool {
        self.last.as_ref().is_some_and(|evidence| {
            evidence.generation == self.generation
                && evidence.exit_code == Some(0)
                && exit_code == Some(0)
                && evidence.command == command.trim()
        })
    }

    pub(crate) fn explicit_last_failure(&self) -> Option<&VerificationEvidence> {
        self.explicit_last.as_ref().filter(|evidence| {
            evidence.generation == self.generation && evidence.exit_code != Some(0)
        })
    }

    pub(crate) fn summary(&self) -> String {
        match self.last.as_ref() {
            Some(evidence) => format!(
                "{}: {} (exit code {})",
                evidence.kind.label(),
                evidence.command,
                evidence
                    .exit_code
                    .map_or_else(|| "unknown".to_string(), |code| code.to_string())
            ),
            None => "none recorded".to_string(),
        }
    }

    fn evidence(
        &self,
        command: &str,
        kind: VerificationKind,
        exit_code: Option<i32>,
    ) -> VerificationEvidence {
        VerificationEvidence {
            command: command.trim().to_string(),
            kind,
            exit_code,
            generation: self.generation,
            scope: self.scope.clone(),
        }
    }
}

fn classify_command(command: &str) -> Option<VerificationKind> {
    let normalized = command.to_ascii_lowercase();
    if normalized.contains("cargo fmt") || normalized.contains("rustfmt") {
        Some(VerificationKind::Format)
    } else if normalized.contains("cargo clippy") || normalized.contains("clippy") {
        Some(VerificationKind::Lint)
    } else if normalized.contains("cargo test")
        || normalized.contains("npm test")
        || normalized.contains("pytest")
        || normalized.contains("go test")
        || normalized.contains("bun test")
    {
        Some(VerificationKind::Test)
    } else if normalized.contains("cargo check") || normalized.contains("tsc ") {
        Some(VerificationKind::Check)
    } else if normalized.contains("cargo build") || normalized.contains("go build") {
        Some(VerificationKind::Build)
    } else {
        None
    }
}

pub(crate) fn is_verification_command(command: &str) -> bool {
    classify_command(command).is_some()
}

/// Return whether a command is the verification action the user asked for in
/// this turn. The request-level predicate is intentionally broad so it can
/// guide the model, but it must not make every incidental shell command
/// authoritative. In particular, `git diff` or `node -v` should not block a
/// completed task merely because the prompt also asked for checks.
pub(crate) fn is_explicit_verification_command(prompt: &str, command: &str) -> bool {
    if !is_explicit_verification_request(prompt) {
        return false;
    }
    if is_verification_command(command) {
        return true;
    }

    let normalized_command = command.trim().to_ascii_lowercase();
    if normalized_command.is_empty() {
        return false;
    }

    // Preserve support for arbitrary named checks when the user supplied the
    // command explicitly, most commonly as an inline shell snippet.
    let mut in_backticks = false;
    let mut candidate = String::new();
    for character in prompt.chars() {
        if character == '`' {
            if in_backticks
                && !candidate.trim().is_empty()
                && normalized_command.starts_with(&candidate.trim().to_ascii_lowercase())
            {
                return true;
            }
            in_backticks = !in_backticks;
            candidate.clear();
        } else if in_backticks {
            candidate.push(character);
        }
    }

    // A generic request such as "run the repository check" may not contain
    // the eventual executable name. Treat only command names that clearly
    // identify a verification action as authoritative in that case.
    let executable = normalized_command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .rsplit_once('/')
        .map_or(
            normalized_command
                .split_whitespace()
                .next()
                .unwrap_or_default(),
            |(_, name)| name,
        );
    matches!(
        executable,
        "check"
            | "check.sh"
            | "test"
            | "test.sh"
            | "lint"
            | "lint.sh"
            | "format"
            | "format.sh"
            | "fmt"
            | "verify"
            | "verify.sh"
            | "validate"
            | "validate.sh"
            | "build"
            | "build.sh"
    )
}

pub(crate) fn is_explicit_verification_request(prompt: &str) -> bool {
    let normalized = prompt.to_ascii_lowercase();
    if [
        "don't run",
        "do not run",
        "without running",
        "don't execute",
        "do not execute",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase))
    {
        return false;
    }

    let asks_to_run = ["run ", "rerun", "re-run", "execute "]
        .iter()
        .any(|phrase| normalized.contains(phrase));
    let asks_for_check = [
        "command",
        "check",
        "verify",
        "validate",
        "test",
        "lint",
        "build",
        "format",
        "verification",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase));
    let has_inline_command = normalized.contains('`');

    (asks_for_check || has_inline_command)
        && (asks_to_run || normalized.trim_start().starts_with("check "))
}

pub(crate) fn requires_verification(changed_paths: &std::collections::BTreeSet<String>) -> bool {
    if changed_paths.is_empty() {
        return true;
    }
    // Conservative default-to-verify: any file that is not explicitly confirmed
    // to be documentation or a non-code asset requires verification.
    changed_paths
        .iter()
        .any(|path| !is_documentation_or_asset(path))
}

fn is_documentation_or_asset(path: &str) -> bool {
    let p = std::path::Path::new(path);
    let file_name = p
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // Never skip verification for known build manifests, dependency locks, or build scripts
    if matches!(
        file_name.as_str(),
        "cargo.toml"
            | "cargo.lock"
            | "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "tsconfig.json"
            | "makefile"
            | "gnumakefile"
            | "dockerfile"
            | "containerfile"
            | "justfile"
            | "procfile"
            | "rakefile"
            | "gemfile"
            | "gemfile.lock"
            | "cmakelists.txt"
            | "pyproject.toml"
            | "requirements.txt"
            | "pipfile"
            | "pipfile.lock"
            | "go.mod"
            | "go.sum"
            | "build.gradle"
            | "settings.gradle"
            | "pom.xml"
    ) {
        return false;
    }

    if matches!(
        file_name.as_str(),
        "readme"
            | "readme.md"
            | "changelog"
            | "changelog.md"
            | "license"
            | "license.md"
            | "contributing.md"
            | "agents.md"
            | "claude.md"
            | ".gitignore"
            | ".gitattributes"
            | ".editorconfig"
    ) {
        return true;
    }

    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    matches!(
        ext.as_str(),
        "md" | "markdown"
            | "txt"
            | "rst"
            | "adoc"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "svg"
            | "ico"
            | "webp"
            | "bmp"
            | "mp3"
            | "wav"
            | "ogg"
            | "csv"
            | "tsv"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        VerificationLedger, VerificationScope, is_explicit_verification_request,
        requires_verification,
    };

    #[test]
    fn explicit_arbitrary_command_failure_is_authoritative() {
        assert!(is_explicit_verification_request(
            "Please run `markdownlint --config .markdownlint.json README.md` and report whether it passes."
        ));

        let mut ledger = VerificationLedger::default();
        ledger.record_explicit_command(
            "markdownlint --config .markdownlint.json README.md",
            Some(1),
        );

        assert_eq!(
            ledger
                .explicit_last_failure()
                .map(|evidence| evidence.command.as_str()),
            Some("markdownlint --config .markdownlint.json README.md")
        );
    }

    #[test]
    fn incidental_diff_does_not_become_explicit_verification_failure() {
        let prompt =
            "Audit the project, run it if possible, verify the result, and fix real issues.";
        assert!(is_explicit_verification_request(prompt));
        assert!(!super::is_explicit_verification_command(
            prompt,
            "git diff ."
        ));
        assert!(super::is_explicit_verification_command(
            prompt,
            "cargo test --all"
        ));
    }

    #[test]
    fn inline_arbitrary_check_command_is_explicit() {
        assert!(super::is_explicit_verification_command(
            "Run `markdownlint README.md` and report whether it passes.",
            "markdownlint README.md"
        ));
        assert!(!super::is_explicit_verification_command(
            "Run `markdownlint README.md` and report whether it passes.",
            "git diff --stat"
        ));
    }

    #[test]
    fn incidental_unknown_command_failure_is_not_authoritative_verification() {
        let mut ledger = VerificationLedger::default();
        ledger.record_command("which markdownlint", Some(1));

        assert!(ledger.explicit_last_failure().is_none());
    }

    #[test]
    fn explicit_verification_request_is_detected_without_a_command_allowlist() {
        assert!(is_explicit_verification_request(
            "Run the custom repository check and tell me if it passes."
        ));
        assert!(is_explicit_verification_request(
            "Please run `custom-tool --strict` and report the result."
        ));
        assert!(is_explicit_verification_request(
            "Rerun the check after editing."
        ));
        assert!(!is_explicit_verification_request(
            "Investigate the documentation issue and explain what you find."
        ));
    }

    #[test]
    fn verification_becomes_stale_after_a_later_edit() {
        let mut ledger = VerificationLedger::default();
        ledger.record_command("cargo test", Some(0));
        assert!(ledger.has_fresh_successful_verification());

        ledger.record_edit();

        assert!(!ledger.has_fresh_successful_verification());
    }

    #[test]
    fn edit_outside_the_verified_roots_keeps_verification_fresh() {
        let project = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let source = project.path().join("lib.rs");
        let config = elsewhere.path().join("config.toml");
        std::fs::write(&source, "").unwrap();
        std::fs::write(&config, "").unwrap();
        let roots = [project.path().to_path_buf()];

        let mut ledger = VerificationLedger::default();
        ledger.record_edit_to(std::slice::from_ref(&source), &roots);
        ledger.set_scope(VerificationScope::new(Some(project.path()), None, None));
        ledger.record_command("cargo test", Some(0));

        ledger.record_edit_to(std::slice::from_ref(&config), &roots);
        assert!(ledger.has_fresh_successful_verification());

        // An edit that also touches the project, or one in a project that the
        // turn has since moved to, still needs its own verification.
        let mut both = ledger.clone();
        both.record_edit_to(&[config.clone(), source.clone()], &roots);
        assert!(!both.has_fresh_successful_verification());
        let mut moved = ledger.clone();
        moved.record_edit_to(
            std::slice::from_ref(&config),
            &[elsewhere.path().to_path_buf()],
        );
        assert!(!moved.has_fresh_successful_verification());

        ledger.record_edit_to(std::slice::from_ref(&source), &roots);
        assert!(!ledger.has_fresh_successful_verification());
        let reason = ledger.missing_verification_reason();
        assert!(reason.contains(&source.display().to_string()), "{reason}");
    }

    #[test]
    fn outside_edit_is_stale_without_a_verified_scope_or_a_resolvable_path() {
        let project = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let config = elsewhere.path().join("config.toml");
        std::fs::write(&config, "").unwrap();

        // The roots of the verification are unknown.
        let mut unscoped = VerificationLedger::default();
        unscoped.record_command("cargo test", Some(0));
        unscoped.record_edit_to(std::slice::from_ref(&config), &[]);
        assert!(!unscoped.has_fresh_successful_verification());

        let mut ledger = VerificationLedger::default();
        ledger.set_scope(VerificationScope::new(Some(project.path()), None, None));
        ledger.record_command("cargo test", Some(0));
        // `..` back into the project is inside it; a missing file is unknown.
        let through = elsewhere
            .path()
            .join("..")
            .join(project.path().file_name().unwrap());
        assert!(!through.starts_with(project.path()));
        let mut inside = ledger.clone();
        std::fs::write(project.path().join("lib.rs"), "").unwrap();
        assert!(through.join("lib.rs").exists());
        inside.record_edit_to(&[through.join("lib.rs")], &[]);
        assert!(!inside.has_fresh_successful_verification());
        ledger.record_edit_to(&[elsewhere.path().join("missing.toml")], &[]);
        assert!(!ledger.has_fresh_successful_verification());
        // A failed verification is never kept fresh.
        let mut failed = VerificationLedger::default();
        failed.set_scope(VerificationScope::new(Some(project.path()), None, None));
        failed.record_command("cargo test", Some(1));
        failed.record_edit_to(std::slice::from_ref(&config), &[]);
        assert!(failed.last_failure().is_none());
        assert!(!failed.has_fresh_successful_verification());
    }

    #[test]
    fn unchanged_workspace_generation_satisfies_a_stale_ledger() {
        let project = tempfile::tempdir().unwrap();
        let source = project.path().join("lib.rs");
        std::fs::write(&source, "first").unwrap();
        let current = |root: &std::path::Path| {
            crate::workspace_intelligence::snapshot(root)
                .ok()
                .map(|snapshot| snapshot.generation)
        };

        let mut ledger = VerificationLedger::default();
        ledger.set_scope(VerificationScope::new(
            Some(project.path()),
            None,
            current(project.path()),
        ));
        ledger.record_command("cargo test", Some(0));
        // An edit the ledger cannot place, which left the workspace as it was.
        ledger.record_edit();
        assert!(!ledger.has_fresh_successful_verification());
        assert!(ledger.verified_workspace_is_unchanged(current));

        std::fs::write(&source, "second").unwrap();
        assert!(!ledger.verified_workspace_is_unchanged(current));
    }

    #[test]
    fn unchanged_generation_needs_a_successful_run_inside_the_workspace() {
        let project = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let same = |_: &std::path::Path| Some(7);

        let mut failed = VerificationLedger::default();
        failed.set_scope(VerificationScope::new(Some(project.path()), None, Some(7)));
        failed.record_command("cargo test", Some(1));
        assert!(!failed.verified_workspace_is_unchanged(same));

        // A project root outside the workspace has no revision on record.
        let mut outside = VerificationLedger::default();
        outside.set_scope(VerificationScope::new(
            Some(project.path()),
            Some(other.path()),
            Some(7),
        ));
        outside.record_command("cargo test", Some(0));
        assert!(!outside.verified_workspace_is_unchanged(same));

        let mut unknown = VerificationLedger::default();
        unknown.record_command("cargo test", Some(0));
        assert!(!unknown.verified_workspace_is_unchanged(same));
    }

    #[test]
    fn failed_verification_is_not_evidence_of_a_clean_workspace() {
        let mut ledger = VerificationLedger::default();
        ledger.record_edit();
        ledger.record_command("cargo fmt --check", Some(1));

        assert!(!ledger.has_fresh_successful_verification());
        assert_eq!(
            ledger.last_failure().map(|e| e.command.as_str()),
            Some("cargo fmt --check")
        );
    }

    #[test]
    fn repeated_successful_verification_is_detected_until_an_edit() {
        let mut ledger = VerificationLedger::default();
        ledger.record_command("cargo check --tests", Some(0));
        assert!(ledger.is_repeated_successful_command("cargo check --tests", Some(0)));
        ledger.record_edit();
        assert!(!ledger.is_repeated_successful_command("cargo check --tests", Some(0)));
    }

    #[test]
    fn requires_verification_identifies_code_vs_doc_edits() {
        let mut non_code = std::collections::BTreeSet::new();
        non_code.insert("README.md".to_string());
        non_code.insert("docs/architecture.txt".to_string());
        non_code.insert("assets/logo.png".to_string());
        assert!(!requires_verification(&non_code));

        let mut with_code = non_code.clone();
        with_code.insert("src/main.rs".to_string());
        assert!(requires_verification(&with_code));
    }

    #[test]
    fn requires_verification_for_manifests_and_extensionless_build_files() {
        // Manifest files must require verification
        for manifest in [
            "Cargo.toml",
            "Cargo.lock",
            "package.json",
            "tsconfig.json",
            "pyproject.toml",
            "go.mod",
            "CMakeLists.txt",
            "requirements.txt",
        ] {
            let mut set = std::collections::BTreeSet::new();
            set.insert(manifest.to_string());
            assert!(
                requires_verification(&set),
                "manifest '{manifest}' must require verification"
            );
        }

        // Extensionless build files must require verification
        for build_file in ["Makefile", "Dockerfile", "Containerfile", "Justfile"] {
            let mut set = std::collections::BTreeSet::new();
            set.insert(build_file.to_string());
            assert!(
                requires_verification(&set),
                "build file '{build_file}' must require verification"
            );
        }
    }
}
