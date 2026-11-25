//! Config files that could not be read, with the parser's own message, so
//! `rustcode doctor`, Settings and a refused save can say what is wrong
//! instead of the engine quietly running on built-in defaults.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// One config file the engine skipped because it could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigLoadIssue {
    pub path: PathBuf,
    /// `user` for `config.toml` in the config directory, `project` for a
    /// repository's `.rustcode/config.toml`.
    pub scope: &'static str,
    /// The read or parse error, naming the file and, for TOML, the line and column.
    pub message: String,
}

/// Every config file that failed to load: the user config, then the project
/// files that apply to `workspace`, in precedence order.
pub fn config_load_issues(workspace: Option<&Path>) -> Vec<ConfigLoadIssue> {
    let mut issues: Vec<ConfigLoadIssue> = super::get_config_dir()
        .and_then(|dir| user_config_issue(&dir))
        .into_iter()
        .collect();
    if let Some(workspace) = workspace {
        for path in super::project_config_paths(workspace) {
            if let Err(message) = super::read_toml_config(&path) {
                issues.push(ConfigLoadIssue {
                    path,
                    scope: "project",
                    message,
                });
            }
        }
    }
    issues
}

/// The user config in `dir`, when it exists and cannot be read.
pub fn user_config_issue(dir: &Path) -> Option<ConfigLoadIssue> {
    let path = dir.join(super::CONFIG_TOML_FILE);
    if !path.exists() {
        return None;
    }
    super::read_toml_config(&path)
        .err()
        .map(|message| ConfigLoadIssue {
            path,
            scope: "user",
            message,
        })
}

/// Why a save was refused while the loaded configuration is invalid.
pub(super) fn invalid_config_error(dir: Option<&Path>) -> String {
    match dir.and_then(user_config_issue) {
        Some(issue) => format!(
            "settings were not saved because {} could not be read ({}); fix the file, then try again",
            issue.path.display(),
            issue.message
        ),
        None => "configuration is invalid".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_or_valid_user_config_is_no_issue() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(user_config_issue(dir.path()), None);
        std::fs::write(dir.path().join("config.toml"), "version = 1\n").unwrap();
        assert_eq!(user_config_issue(dir.path()), None);
    }

    #[test]
    fn a_broken_user_config_names_the_file_and_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "version = 1\nmax_tool_rounds = \"oops\n",
        )
        .unwrap();
        let issue = user_config_issue(dir.path()).expect("an issue");
        assert_eq!(issue.scope, "user");
        assert!(issue.message.contains("config.toml"), "{}", issue.message);
        assert!(issue.message.contains("line 2"), "{}", issue.message);
    }

    #[test]
    fn a_too_new_format_is_an_issue() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "version = 999\n").unwrap();
        let issue = user_config_issue(dir.path()).expect("an issue");
        assert!(issue.message.contains("unsupported config format version"));
    }

    #[test]
    fn a_refused_save_names_the_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[default\n").unwrap();
        let message = invalid_config_error(Some(dir.path()));
        assert!(
            message.contains("config.toml could not be read"),
            "{message}"
        );
        assert_eq!(invalid_config_error(None), "configuration is invalid");
    }

    #[test]
    fn a_broken_project_config_is_its_own_issue() {
        let workspace = tempfile::tempdir().unwrap();
        let project = workspace.path().join(".rustcode");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("config.toml"), "sandbox_mode = [\n").unwrap();
        let issues = config_load_issues(Some(workspace.path()));
        let project_issue = issues
            .iter()
            .find(|issue| issue.scope == "project")
            .expect("a project issue");
        assert!(project_issue.path.ends_with(".rustcode/config.toml"));
    }
}
