#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerificationKind {
    Check,
    Test,
    Format,
    Lint,
    Build,
}

impl VerificationKind {
    fn label(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Test => "test",
            Self::Format => "format",
            Self::Lint => "lint",
            Self::Build => "build",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerificationEvidence {
    pub command: String,
    pub kind: VerificationKind,
    pub exit_code: Option<i32>,
    pub generation: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct VerificationLedger {
    generation: u64,
    last: Option<VerificationEvidence>,
}

impl VerificationLedger {
    pub(crate) fn record_edit(&mut self) {
        self.generation = self.generation.saturating_add(1);
    }

    pub(crate) fn record_command(&mut self, command: &str, exit_code: Option<i32>) {
        let Some(kind) = classify_command(command) else {
            return;
        };
        self.last = Some(VerificationEvidence {
            command: command.trim().to_string(),
            kind,
            exit_code,
            generation: self.generation,
        });
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

pub(crate) fn requires_verification(changed_paths: &std::collections::BTreeSet<String>) -> bool {
    if changed_paths.is_empty() {
        return true;
    }
    // Conservative default-to-verify: any file that is not explicitly confirmed
    // to be documentation or a non-code asset requires verification.
    changed_paths.iter().any(|path| !is_documentation_or_asset(path))
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
    use super::{VerificationLedger, requires_verification};

    #[test]
    fn verification_becomes_stale_after_a_later_edit() {
        let mut ledger = VerificationLedger::default();
        ledger.record_command("cargo test", Some(0));
        assert!(ledger.has_fresh_successful_verification());

        ledger.record_edit();

        assert!(!ledger.has_fresh_successful_verification());
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
