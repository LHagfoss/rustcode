//! `github`: read issues, pull requests, CI checks and run logs through the
//! GitHub CLI's sign-in (Settings > Connections). Read-only, so it needs no
//! approval; anything that changes GitHub still goes through `run_command`.
//! Output is compact text built from `gh ... --json`, a fraction of the size
//! of the CLI's pretty output or the web page.

use serde_json::Value;
use std::process::Command;

use super::{Tool, ToolCapability, ToolSafety};

const MAX_OUTPUT_BYTES: usize = 24_000;
/// Characters of an issue or PR body, and of each comment.
const BODY_CHARS: usize = 4_000;
const COMMENT_CHARS: usize = 1_200;
/// Comments shown, newest last.
const MAX_COMMENTS: usize = 12;
const DEFAULT_LIMIT: u64 = 20;
const MAX_LIMIT: u64 = 100;

const ACTIONS: &[&str] = &[
    "issue",
    "pr",
    "pr_diff",
    "checks",
    "list_issues",
    "list_prs",
    "run_log",
];

fn github_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ACTIONS },
            "repo": { "type": "string", "description": "owner/name; default: this workspace" },
            "number": { "type": "integer", "description": "Issue/PR number, or run id for run_log" },
            "query": { "type": "string", "description": "list_*: search terms" },
            "state": { "type": "string", "enum": ["open", "closed", "merged", "all"] },
            "limit": { "type": "integer" }
        },
        "required": ["action"],
        "additionalProperties": false
    })
}

pub const GITHUB: Tool = Tool {
    name: "github",
    description: "Read GitHub via the user's gh sign-in: an issue or PR with comments, a PR diff or CI checks, issue/PR lists, a run's failed log. Read-only; prefer it over gh in run_command for reading.",
    arguments: r#"{"action": "issue|pr|pr_diff|checks|list_issues|list_prs|run_log", "repo": "optional owner/name", "number": 123, "query": "optional search terms", "state": "open|closed|merged|all", "limit": 20}"#,
    handler: github,
    requires_confirmation: false,
    schema: github_schema,
    capabilities: &[ToolCapability::Network],
    safety: ToolSafety::ReadOnly,
};

pub fn github(args: &Value) -> Result<String, String> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .ok_or("missing 'action' argument")?;
    let repo = args
        .get("repo")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|repo| !repo.is_empty());
    if let Some(repo) = repo
        && (repo.starts_with('-') || repo.split('/').count() != 2)
    {
        return Err(format!("'repo' must be owner/name, got '{repo}'"));
    }
    let number = || {
        args.get("number")
            .and_then(Value::as_u64)
            .map(|n| n.to_string())
            .ok_or_else(|| format!("'{action}' needs a 'number'"))
    };
    let state = args.get("state").and_then(Value::as_str).unwrap_or("open");
    if !["open", "closed", "merged", "all"].contains(&state) {
        return Err(format!("unknown state '{state}'"));
    }
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT)
        .to_string();
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();

    let text = match action {
        "issue" => {
            let n = number()?;
            let json = gh_json(
                &[
                    "issue",
                    "view",
                    &n,
                    "--json",
                    "number,title,state,author,labels,assignees,body,comments,url,createdAt",
                ],
                repo,
            )?;
            format_item(&json, false)
        }
        "pr" => {
            let n = number()?;
            let json = gh_json(
                &[
                    "pr",
                    "view",
                    &n,
                    "--json",
                    "number,title,state,author,labels,body,comments,reviews,url,isDraft,baseRefName,headRefName,mergeable,reviewDecision,additions,deletions,files,statusCheckRollup",
                ],
                repo,
            )?;
            format_item(&json, true)
        }
        "pr_diff" => run_gh(&["pr", "diff", &number()?], repo, false)?,
        "checks" => run_gh(&["pr", "checks", &number()?], repo, true)?,
        "run_log" => {
            let log = run_gh(&["run", "view", &number()?, "--log-failed"], repo, false)?;
            // The end of a failed log holds the error; keep that part.
            tail_bytes(&log, MAX_OUTPUT_BYTES)
        }
        "list_issues" | "list_prs" => {
            let kind = if action == "list_issues" {
                "issue"
            } else {
                "pr"
            };
            if kind == "issue" && state == "merged" {
                return Err("issues have no 'merged' state".into());
            }
            let mut list = vec![
                kind,
                "list",
                "--state",
                state,
                "--limit",
                &limit,
                "--json",
                "number,title,state,author,labels,updatedAt",
            ];
            if !query.is_empty() {
                list.extend(["--search", query]);
            }
            format_list(&gh_json(&list, repo)?)
        }
        other => {
            return Err(format!(
                "unknown action '{other}'; use one of {}",
                ACTIONS.join(", ")
            ));
        }
    };
    Ok(super::truncate_bytes(&text, MAX_OUTPUT_BYTES))
}

fn gh_json(args: &[&str], repo: Option<&str>) -> Result<Value, String> {
    let text = run_gh(args, repo, false)?;
    serde_json::from_str(&text).map_err(|error| format!("gh returned unreadable JSON: {error}"))
}

/// Run `gh` in the workspace. `pending_ok` keeps the output of `gh pr checks`,
/// which exits non-zero while checks fail or are still running.
fn run_gh(args: &[&str], repo: Option<&str>, pending_ok: bool) -> Result<String, String> {
    let mut command = Command::new(crate::platform::resolve_bin("gh"));
    command.args(args);
    if let Some(repo) = repo {
        command.args(["--repo", repo]);
    }
    let output = command
        .current_dir(super::git::working_directory())
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => {
                "The GitHub CLI (gh) is not installed. Ask the user to install it (brew install gh) and connect GitHub in Settings > Connections.".to_owned()
            }
            _ => format!("failed to run gh: {error}"),
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() || (pending_ok && !stdout.trim().is_empty()) {
        return Ok(if stdout.trim().is_empty() {
            "(no output)".into()
        } else {
            stdout
        });
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut message = format!(
        "gh {} failed: {}",
        args[..2.min(args.len())].join(" "),
        stderr.trim()
    );
    if stderr.contains("gh auth login") || stderr.contains("not logged in") {
        message.push_str(
            "\nGitHub is not connected. Ask the user to connect it in Settings > Connections.",
        );
    }
    Err(super::truncate_bytes(&message, 4_000))
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn login(value: &Value) -> &str {
    value
        .get("author")
        .map_or("", |author| text(author, "login"))
}

fn names(value: &Value, key: &str, field: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| text(item, field).to_owned())
        .filter(|name| !name.is_empty())
        .collect()
}

fn clip(text: &str, chars: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(chars) {
        Some((end, _)) => format!(
            "{}… [{} more characters]",
            &text[..end],
            text[end..].chars().count()
        ),
        None => text.to_owned(),
    }
}

fn tail_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("[{} earlier bytes omitted]\n{}", start, &text[start..])
}

/// One issue or pull request as a header, its body and its comments.
fn format_item(item: &Value, is_pr: bool) -> String {
    let mut out = format!(
        "#{} {} [{}{}] by {}\n{}\n",
        item.get("number").and_then(Value::as_u64).unwrap_or(0),
        text(item, "title"),
        text(item, "state"),
        if item.get("isDraft").and_then(Value::as_bool) == Some(true) {
            ", draft"
        } else {
            ""
        },
        login(item),
        text(item, "url"),
    );
    let labels = names(item, "labels", "name");
    if !labels.is_empty() {
        out.push_str(&format!("labels: {}\n", labels.join(", ")));
    }
    let assignees = names(item, "assignees", "login");
    if !assignees.is_empty() {
        out.push_str(&format!("assignees: {}\n", assignees.join(", ")));
    }
    if is_pr {
        out.push_str(&format!(
            "{} <- {} · +{} -{} in {} files · mergeable: {} · review: {}\n",
            text(item, "baseRefName"),
            text(item, "headRefName"),
            item.get("additions").and_then(Value::as_u64).unwrap_or(0),
            item.get("deletions").and_then(Value::as_u64).unwrap_or(0),
            item.get("files")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            text(item, "mergeable").to_ascii_lowercase(),
            match text(item, "reviewDecision") {
                "" => "none".to_owned(),
                decision => decision.to_ascii_lowercase().replace('_', " "),
            },
        ));
        if let Some(checks) = item.get("statusCheckRollup").and_then(Value::as_array)
            && !checks.is_empty()
        {
            out.push_str(&format!("checks: {}\n", summarize_checks(checks)));
        }
    }
    let body = text(item, "body");
    out.push('\n');
    out.push_str(&if body.trim().is_empty() {
        "(no description)".to_owned()
    } else {
        clip(body, BODY_CHARS)
    });
    out.push('\n');

    let mut comments: Vec<(&str, String, &str)> = Vec::new();
    for comment in item
        .get("comments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        comments.push((
            text(comment, "createdAt"),
            login(comment).to_owned(),
            text(comment, "body"),
        ));
    }
    for review in item
        .get("reviews")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let state = text(review, "state").to_ascii_lowercase().replace('_', " ");
        let body = text(review, "body");
        if body.trim().is_empty() && state == "commented" {
            continue;
        }
        comments.push((
            text(review, "submittedAt"),
            format!("{} (review: {state})", login(review)),
            body,
        ));
    }
    comments.sort_by(|a, b| a.0.cmp(b.0));
    if !comments.is_empty() {
        let skipped = comments.len().saturating_sub(MAX_COMMENTS);
        out.push_str(&format!("\n--- {} comments", comments.len()));
        if skipped > 0 {
            out.push_str(&format!(", oldest {skipped} omitted"));
        }
        out.push_str(" ---\n");
        for (at, who, body) in &comments[skipped..] {
            let day = at.get(..10).unwrap_or(at);
            out.push_str(&format!("\n{who} · {day}\n{}\n", clip(body, COMMENT_CHARS)));
        }
    }
    out
}

/// "3 passed, 1 failed (lint), 1 pending (test)".
fn summarize_checks(checks: &[Value]) -> String {
    let (mut passed, mut failed, mut pending) = (0, Vec::new(), Vec::new());
    for check in checks {
        let name = match text(check, "name") {
            "" => text(check, "context"),
            name => name,
        };
        // Check runs report `conclusion`; commit statuses report `state`.
        let outcome = match text(check, "conclusion") {
            "" => text(check, "state"),
            conclusion => conclusion,
        };
        match outcome.to_ascii_uppercase().as_str() {
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => passed += 1,
            "" | "PENDING" | "EXPECTED" | "QUEUED" | "IN_PROGRESS" => pending.push(name),
            _ => failed.push(name),
        }
    }
    let mut parts = vec![format!("{passed} passed")];
    if !failed.is_empty() {
        parts.push(format!("{} failed ({})", failed.len(), failed.join(", ")));
    }
    if !pending.is_empty() {
        parts.push(format!(
            "{} pending ({})",
            pending.len(),
            pending.join(", ")
        ));
    }
    parts.join(", ")
}

fn format_list(list: &Value) -> String {
    let items = list.as_array().map(Vec::as_slice).unwrap_or_default();
    if items.is_empty() {
        return "No matches.".into();
    }
    items
        .iter()
        .map(|item| {
            let labels = names(item, "labels", "name");
            format!(
                "#{} {} [{}] {} · {}{}",
                item.get("number").and_then(Value::as_u64).unwrap_or(0),
                text(item, "title"),
                text(item, "state").to_ascii_lowercase(),
                login(item),
                text(item, "updatedAt").get(..10).unwrap_or(""),
                if labels.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", labels.join(", "))
                },
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issue_reads_as_header_body_and_comments() {
        let issue = serde_json::json!({
            "number": 7, "title": "Crash on start", "state": "OPEN", "url": "https://github.com/o/r/issues/7",
            "author": {"login": "amy"}, "labels": [{"name": "bug"}], "assignees": [],
            "body": "Steps to reproduce", "createdAt": "2026-10-01T00:00:00Z",
            "comments": [
                {"author": {"login": "zed"}, "body": "second", "createdAt": "2026-10-03T00:00:00Z"},
                {"author": {"login": "amy"}, "body": "first", "createdAt": "2026-10-02T00:00:00Z"}
            ]
        });
        let text = format_item(&issue, false);
        assert!(
            text.starts_with(
                "#7 Crash on start [OPEN] by amy\nhttps://github.com/o/r/issues/7\nlabels: bug\n"
            ),
            "{text}"
        );
        assert!(text.contains("Steps to reproduce"));
        let first = text.find("amy · 2026-10-02").unwrap();
        let second = text.find("zed · 2026-10-03").unwrap();
        assert!(first < second, "comments are oldest first: {text}");
    }

    #[test]
    fn a_pr_summarizes_branches_reviews_and_checks() {
        let pr = serde_json::json!({
            "number": 9, "title": "Fix", "state": "OPEN", "isDraft": true, "url": "u", "author": {"login": "amy"},
            "baseRefName": "main", "headRefName": "fix/x", "additions": 10, "deletions": 2,
            "files": [{}, {}], "mergeable": "MERGEABLE", "reviewDecision": "CHANGES_REQUESTED", "body": "",
            "comments": [], "reviews": [
                {"author": {"login": "zed"}, "state": "CHANGES_REQUESTED", "body": "Needs a test", "submittedAt": "2026-10-04T00:00:00Z"},
                {"author": {"login": "bot"}, "state": "COMMENTED", "body": "", "submittedAt": "2026-10-05T00:00:00Z"}
            ],
            "statusCheckRollup": [
                {"name": "build", "conclusion": "SUCCESS"},
                {"name": "lint", "conclusion": "FAILURE"},
                {"name": "test", "conclusion": "", "status": "IN_PROGRESS"},
                {"context": "ci/legacy", "state": "SUCCESS"}
            ]
        });
        let text = format_item(&pr, true);
        assert!(text.contains("[OPEN, draft]"), "{text}");
        assert!(text.contains("main <- fix/x · +10 -2 in 2 files · mergeable: mergeable · review: changes requested"), "{text}");
        assert!(
            text.contains("checks: 2 passed, 1 failed (lint), 1 pending (test)"),
            "{text}"
        );
        assert!(text.contains("(no description)"));
        assert!(
            text.contains("zed (review: changes requested) · 2026-10-04\nNeeds a test"),
            "{text}"
        );
        assert!(
            !text.contains("bot"),
            "empty comment-only reviews are dropped: {text}"
        );
    }

    #[test]
    fn lists_are_one_line_each() {
        let list = serde_json::json!([
            {"number": 1, "title": "A", "state": "OPEN", "author": {"login": "amy"}, "labels": [{"name": "bug"}], "updatedAt": "2026-10-01T10:00:00Z"},
            {"number": 2, "title": "B", "state": "CLOSED", "author": {"login": "zed"}, "labels": [], "updatedAt": "2026-10-02T10:00:00Z"}
        ]);
        assert_eq!(
            format_list(&list),
            "#1 A [open] amy · 2026-10-01 · bug\n#2 B [closed] zed · 2026-10-02"
        );
        assert_eq!(format_list(&serde_json::json!([])), "No matches.");
    }

    #[test]
    fn bad_arguments_fail_before_running_gh() {
        let err = |args: Value| github(&args).unwrap_err();
        assert!(err(serde_json::json!({})).contains("action"));
        assert!(err(serde_json::json!({"action": "issue"})).contains("number"));
        assert!(err(serde_json::json!({"action": "nope"})).contains("unknown action"));
        assert!(
            err(serde_json::json!({"action": "issue", "number": 1, "repo": "--web"}))
                .contains("owner/name")
        );
        assert!(
            err(serde_json::json!({"action": "list_issues", "state": "merged"})).contains("merged")
        );
    }

    #[test]
    fn long_text_is_clipped_with_a_count() {
        assert_eq!(clip("abcdef", 4), "abcd… [2 more characters]");
        assert_eq!(clip(" short ", 10), "short");
        assert_eq!(tail_bytes("abcdef", 3), "[3 earlier bytes omitted]\ndef");
    }
}
