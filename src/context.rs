use std::path::{Path, PathBuf};

/// Snapshot of environment context for delta diffing.
/// Stores the last-rendered context so subsequent rounds only send changes.
#[derive(Clone, Default)]
pub struct ContextSnapshot {
    cwd: String,
    platform: String,
    date: String,
    git_branch: Option<String>,
    git_status_summary: Option<String>,
    tree_entries: Vec<String>,
    agent_doc: Option<String>,
}

impl ContextSnapshot {
    /// Capture the current environment state.
    pub fn capture() -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "(unknown)".to_string());
        let platform = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
        let date = chrono::Local::now().format("%A %Y-%m-%d").to_string();

        let git_branch = run_git(std::path::Path::new(&cwd), &["rev-parse", "--abbrev-ref", "HEAD"])
            .map(|s| s.trim().to_string());
        let git_status_summary = run_git(std::path::Path::new(&cwd), &["status", "--short"])
            .map(|s| {
                let count = s.trim().lines().count();
                if count == 0 { "clean".to_string() } else { format!("{} changed", count) }
            });

        let tree_entries = std::fs::read_dir(&cwd)
            .ok()
            .map(|entries| {
                let mut names: Vec<String> = entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        if name.starts_with('.') { return None; }
                        let mut n = name;
                        if e.file_type().ok().map(|t| t.is_dir()).unwrap_or(false) {
                            n.push('/');
                        }
                        Some(n)
                    })
                    .collect();
                names.sort();
                names.truncate(MAX_TREE_ENTRIES);
                names
            })
            .unwrap_or_default();

        let agent_doc = load_agent_doc(&cwd);

        Self { cwd, platform, date, git_branch, git_status_summary, tree_entries, agent_doc }
    }

    /// Produce a delta description of what changed between the previous snapshot and now.
    /// Returns None if nothing changed.
    pub fn diff(&self, current: &ContextSnapshot) -> Option<String> {
        let mut changes = Vec::new();

        if self.cwd != current.cwd {
            changes.push(format!("Working directory changed: {} -> {}", self.cwd, current.cwd));
        }
        if self.date != current.date {
            changes.push(format!("Date changed: {}", current.date));
        }
        if self.git_branch != current.git_branch {
            changes.push(format!("Git branch changed: {:?} -> {:?}", self.git_branch, current.git_branch));
        }
        if self.git_status_summary != current.git_status_summary {
            changes.push(format!("Git status: {}",
                current.git_status_summary.as_deref().unwrap_or("unknown")));
        }
        if self.tree_entries != current.tree_entries {
            let added: Vec<&String> = current.tree_entries.iter()
                .filter(|e| !self.tree_entries.contains(e)).collect();
            let removed: Vec<&String> = self.tree_entries.iter()
                .filter(|e| !current.tree_entries.contains(e)).collect();
            if !added.is_empty() {
                changes.push(format!("New files/dirs: {}", added.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
            }
            if !removed.is_empty() {
                changes.push(format!("Removed files/dirs: {}", removed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
            }
        }
        if self.agent_doc != current.agent_doc {
            changes.push("Project instructions (AGENTS.md) changed.".to_string());
        }

        if changes.is_empty() {
            None
        } else {
            Some(format!("# Environment Updates\n{}", changes.join("\n")))
        }
    }
}

const MAX_TREE_ENTRIES: usize = 30;
const MAX_AGENTS_BYTES: usize = 8000;

pub fn environment_context() -> String {
    let mut out = String::new();
    out.push_str("# Environment\n\n");

    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "(unknown)".to_string());
    out.push_str(&format!("- Working directory: {cwd}\n"));

    out.push_str(&format!(
        "- Platform: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));

    let now = chrono::Local::now();
    out.push_str(&format!("- Today's date: {}\n", now.format("%A %Y-%m-%d")));

    if let Some(git) = git_context(&cwd) {
        out.push_str(&git);
    }

    if let Some(tree) = top_level_tree(&cwd) {
        out.push_str(&tree);
    }

    if let Some(agent_doc) = load_agent_doc(&cwd) {
        out.push_str("\n# Project instructions (AGENTS.md)\n\n");
        out.push_str(&agent_doc);
        out.push('\n');
    }

    out
}

fn git_context(cwd: &str) -> Option<String> {
    let cwd_path = Path::new(cwd);
    if !cwd_path.join(".git").exists() && !is_inside_git_worktree(cwd_path) {
        return None;
    }

    let mut out = String::from("\n## Git\n\n");

    if let Some(branch) = run_git(cwd_path, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        out.push_str(&format!("- Current branch: {}\n", branch.trim()));
    }

    if let Some(status) = run_git(cwd_path, &["status", "--short"]) {
        let trimmed = status.trim();
        if trimmed.is_empty() {
            out.push_str("- Working tree: clean\n");
        } else {
            let lines: Vec<&str> = trimmed.lines().collect();
            let shown: Vec<&str> = lines.iter().take(20).copied().collect();
            out.push_str(&format!(
                "- Working tree: {} changed path(s)\n",
                lines.len()
            ));
            out.push_str("```\n");
            out.push_str(&shown.join("\n"));
            if lines.len() > shown.len() {
                out.push_str(&format!("\n... ({} more changed files truncated to save context)", lines.len() - shown.len()));
            }
            out.push_str("\n```\n");
        }
    }

    if let Some(head) = run_git(cwd_path, &["log", "--oneline", "-5"]) {
        out.push_str("- Recent commits:\n```\n");
        out.push_str(head.trim());
        out.push_str("\n```\n");
    }

    Some(out)
}

fn is_inside_git_worktree(path: &Path) -> bool {
    run_git(path, &["rev-parse", "--is-inside-work-tree"])
        .map(|s| s.trim() == "true")
        .unwrap_or(false)
}

fn run_git(path: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout_str = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<&str> = stdout_str.lines().collect();
    if lines.len() > 50 {
        let truncated = lines[..50].join("\n");
        let truncated_count = lines.len() - 50;
        Some(format!(
            "{}\n... (truncated {} lines of git output to prune context) ...\n",
            truncated, truncated_count
        ))
    } else {
        Some(stdout_str)
    }
}

fn top_level_tree(cwd: &str) -> Option<String> {
    let path = Path::new(cwd);
    let entries = std::fs::read_dir(path).ok()?;
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            let mut n = name;
            if e.file_type().ok().map(|t| t.is_dir()).unwrap_or(false) {
                n.push('/');
            }
            Some(n)
        })
        .collect();
    names.sort();
    let total = names.len();
    if total == 0 {
        return None;
    }
    let shown: Vec<String> = names.into_iter().take(MAX_TREE_ENTRIES).collect();
    let mut out = String::from("\n## Working directory tree (top level)\n\n");
    out.push_str(&shown.join("\n"));
    if total > shown.len() {
        out.push_str(&format!("\n... ({} more entries truncated to save context)", total - shown.len()));
    }
    out.push('\n');
    Some(out)
}

fn load_agent_doc(cwd: &str) -> Option<String> {
    let candidates: [PathBuf; 2] = [
        PathBuf::from(cwd).join("AGENTS.md"),
        PathBuf::from(cwd).join("CLAUDE.md"),
    ];
    for c in candidates {
        if let Ok(content) = std::fs::read_to_string(&c) {
            let mut s = content;
            if s.len() > MAX_AGENTS_BYTES {
                s.truncate(MAX_AGENTS_BYTES);
                s.push_str("\n... (truncated)");
            }
            return Some(s);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_context_includes_cwd() {
        let ctx = environment_context();
        assert!(ctx.starts_with("# Environment"));
        assert!(ctx.contains("Working directory:"));
        assert!(ctx.contains("Platform:"));
        assert!(ctx.contains("Today's date:"));
    }

    #[test]
    fn top_level_tree_lists_entries() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("rustcode-ctx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("a.txt"), "x").unwrap();
        std::fs::create_dir(path.join("sub")).unwrap();
        let tree = top_level_tree(path.to_str().unwrap()).unwrap();
        assert!(tree.contains("a.txt"));
        assert!(tree.contains("sub/"));
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn top_level_tree_empty_dir_is_none() {
        let dir = std::env::temp_dir().join(format!("rustcode-ctx-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(top_level_tree(dir.to_str().unwrap()).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
