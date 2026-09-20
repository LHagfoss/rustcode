use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

use super::{Tool, ToolCapability, ToolSafety};

const MAX_GIT_OUTPUT_BYTES: usize = 20_000;

fn working_directory() -> PathBuf {
    super::active_task_working_directory()
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|cwd| crate::memory::repository_root(Some(&cwd)).into())
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

fn truncate_output(text: &str) -> String {
    super::truncate_bytes(text, MAX_GIT_OUTPUT_BYTES)
}

fn run_git(args: &[&str]) -> Result<String, String> {
    let cwd = working_directory();
    let output = Command::new("git")
        .args(args)
        .current_dir(&cwd)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    let mut combined = String::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stdout.trim().is_empty() {
        combined.push_str(&stdout);
    }
    if !output.status.success() {
        if !stderr.trim().is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(&stderr);
        }
        return Err(truncate_output(&format!(
            "git {} failed (exit {}):\n{}",
            args.join(" "),
            output.status.code().unwrap_or(-1),
            combined.trim()
        )));
    }
    if combined.trim().is_empty() && !stderr.trim().is_empty() {
        combined.push_str(&stderr);
    }
    if combined.trim().is_empty() {
        combined.push_str("(no output)");
    }
    Ok(truncate_output(&combined))
}

fn git_status_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

pub const GIT_STATUS: Tool = Tool {
    name: "git_status",
    description: "Show `git status --short --branch` for the active workspace. Read-only inspection.",
    arguments: r#"{}"#,
    handler: git_status,
    requires_confirmation: false,
    schema: git_status_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

pub fn git_status(args: &Value) -> Result<String, String> {
    if !args.is_object() {
        return Err("arguments must be a JSON object".to_string());
    }
    run_git(&["status", "--short", "--branch"])
}

fn git_diff_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "staged": { "type": "boolean", "description": "Show staged (--cached) diff instead of working tree", "default": false },
            "path": { "type": "string", "description": "Optional path to limit the diff to" }
        },
        "additionalProperties": false
    })
}

pub const GIT_DIFF: Tool = Tool {
    name: "git_diff",
    description: "Show `git diff` for the active workspace, optionally staged or limited to a path. Read-only inspection.",
    arguments: r#"{"staged": "optional bool (default false)", "path": "optional path"}"#,
    handler: git_diff,
    requires_confirmation: false,
    schema: git_diff_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

pub fn git_diff(args: &Value) -> Result<String, String> {
    let staged = args
        .get("staged")
        .and_then(super::parse_json_bool)
        .unwrap_or(false);
    let path = args.get("path").and_then(Value::as_str).map(str::trim);
    if let Some(path) = path
        && (path.is_empty() || path.contains('\0'))
    {
        return Err("invalid 'path'".to_string());
    }
    let mut cmd: Vec<&str> = vec!["diff", "--no-color"];
    if staged {
        cmd.push("--cached");
    }
    if let Some(path) = path.filter(|p| !p.is_empty()) {
        cmd.push("--");
        cmd.push(path);
    }
    run_git(&cmd)
}

fn git_add_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "paths": { "type": "array", "items": { "type": "string" }, "description": "Explicit file paths to stage (never '.' or '-A')" }
        },
        "required": ["paths"]
    })
}

pub const GIT_ADD: Tool = Tool {
    name: "git_add",
    description: "Stage explicit file paths with `git add -- <paths>`. Broad staging ('.', '-A', '--all') is refused.",
    arguments: r#"{"paths": ["src/network.rs"]}"#,
    handler: git_add,
    requires_confirmation: true,
    schema: git_add_schema,
    capabilities: &[ToolCapability::WriteWorkspace],
    safety: ToolSafety::WorkspaceMutation,
};

pub fn git_add(args: &Value) -> Result<String, String> {
    let paths = super::coerce_array(args.get("paths").unwrap_or(&Value::Null))
        .ok_or("missing 'paths' array")?;
    if paths.is_empty() {
        return Err("'paths' must not be empty".to_string());
    }
    let mut owned: Vec<String> = Vec::new();
    for path in &paths {
        let path = path
            .as_str()
            .map(str::trim)
            .ok_or("each path must be a string")?;
        if path.is_empty()
            || matches!(path, "." | "-A" | "--all")
            || path.contains('\0')
            || path.contains("..")
        {
            return Err(format!("refusing to stage broad or unsafe path: '{path}'"));
        }
        owned.push(path.to_string());
    }
    // Reuse the shared shell policy so `git add .` style calls stay blocked
    // even if a provider smuggles them through this tool.
    let probe = format!("git add {}", owned.join(" "));
    if crate::tools::exec::reject_broad_git_stage(&probe).is_some() {
        return Err("Refusing broad git staging. Stage explicit feature paths.".to_string());
    }
    let mut cmd: Vec<&str> = vec!["add", "--"];
    cmd.extend(owned.iter().map(String::as_str));
    run_git(&cmd).map(|_| format!("Staged {} path(s).", owned.len()))
}

fn git_commit_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "message": { "type": "string", "description": "Commit message (single logical change, no -a/--all)" }
        },
        "required": ["message"]
    })
}

pub const GIT_COMMIT: Tool = Tool {
    name: "git_commit",
    description: "Create a commit with `git commit -m <message>`. Never stages everything; stage explicit paths with git_add first.",
    arguments: r#"{"message": "concise commit message"}"#,
    handler: git_commit,
    requires_confirmation: true,
    schema: git_commit_schema,
    capabilities: &[ToolCapability::WriteWorkspace],
    safety: ToolSafety::WorkspaceMutation,
};

pub fn git_commit(args: &Value) -> Result<String, String> {
    let message = args
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or("missing 'message'")?;
    if message.is_empty() {
        return Err("commit message must not be empty".to_string());
    }
    if message.len() > 2000 {
        return Err("commit message is too long (max 2000 chars)".to_string());
    }
    if message.contains("-a")
        && (message.contains("git commit -a") || message.contains("commit --all"))
    {
        return Err("do not smuggle -a/--all into the commit message".to_string());
    }
    run_git(&["commit", "-m", message]).map(|out| format!("Committed.\n{out}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_broad_stage_paths() {
        for bad in [".", "-A", "--all", ""] {
            let args = serde_json::json!({"paths": [bad]});
            assert!(git_add(&args).is_err(), "should reject {bad}");
        }
    }

    #[test]
    fn rejects_empty_commit_message() {
        assert!(git_commit(&serde_json::json!({})).is_err());
        assert!(git_commit(&serde_json::json!({"message": "  "})).is_err());
    }

    #[test]
    fn schemas_require_expected_fields() {
        assert!(git_add_schema()["required"].is_array());
        assert!(git_commit_schema()["required"].is_array());
    }

    #[test]
    fn git_tools_have_expected_safety_and_confirmation() {
        assert_eq!(GIT_STATUS.safety, super::super::ToolSafety::ReadOnly);
        assert_eq!(GIT_DIFF.safety, super::super::ToolSafety::ReadOnly);
        assert!(!GIT_STATUS.requires_confirmation);
        assert!(!GIT_DIFF.requires_confirmation);
        assert_eq!(GIT_ADD.safety, super::super::ToolSafety::WorkspaceMutation);
        assert_eq!(
            GIT_COMMIT.safety,
            super::super::ToolSafety::WorkspaceMutation
        );
        assert!(GIT_ADD.requires_confirmation);
        assert!(GIT_COMMIT.requires_confirmation);
    }
}
