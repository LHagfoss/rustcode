use std::path::{Path, PathBuf};

const MAX_TREE_ENTRIES: usize = 60;
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
    out.push_str(&format!(
        "- Today's date: {}\n",
        now.format("%A %Y-%m-%d")
    ));

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
            let shown: Vec<&str> = lines.iter().take(40).copied().collect();
            out.push_str(&format!("- Working tree: {} changed path(s)\n", lines.len()));
            out.push_str("```\n");
            out.push_str(&shown.join("\n"));
            if lines.len() > shown.len() {
                out.push_str(&format!("\n... ({} more)", lines.len() - shown.len()));
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
    Some(String::from_utf8_lossy(&out.stdout).to_string())
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
        out.push_str(&format!("\n... ({} more entries)", total - shown.len()));
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

