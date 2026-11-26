//! Read-only listing of a project's GitHub issues through the `gh` CLI,
//! reusing the process, timeout, and error classification helpers from
//! `crate::pull_requests`.

use serde::Serialize;
use serde_json::Value;
use std::path::Path;

use crate::pull_requests::{MAX_LIMIT, PullError, gh};

const ISSUE_FIELDS: &str = "number,title,author,state,labels,assignees,body,updatedAt,url";

/// State of a GitHub issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueState {
    Open,
    Closed,
}

/// One label attached to an issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueLabel {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// One GitHub issue with enough detail to inspect and seed a worktree thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueSummary {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub state: IssueState,
    pub labels: Vec<IssueLabel>,
    pub assignees: Vec<String>,
    pub body: String,
    pub updated_at: String,
    pub url: String,
}

/// Page of issues returned by `list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueList {
    pub provider: &'static str,
    pub issues: Vec<IssueSummary>,
    pub has_more: bool,
}

fn normalized_state(state: &str) -> &'static str {
    match state {
        "closed" => "closed",
        "all" => "all",
        _ => "open",
    }
}

fn build_list_args(
    state: &str,
    search: Option<&str>,
    labels: &[String],
    fetch: &str,
) -> Vec<String> {
    let mut args = vec![
        "issue".to_owned(),
        "list".to_owned(),
        "--state".to_owned(),
        normalized_state(state).to_owned(),
        "--limit".to_owned(),
        fetch.to_owned(),
        "--json".to_owned(),
        ISSUE_FIELDS.to_owned(),
    ];
    if let Some(search) = search.map(str::trim).filter(|s| !s.is_empty()) {
        args.push("--search".to_owned());
        args.push(search.to_owned());
    }
    for label in labels.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        args.push("--label".to_owned());
        args.push(label.to_owned());
    }
    args
}

fn field_str(item: &Value, key: &str) -> String {
    item.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn login_of(value: &Value) -> Option<String> {
    value
        .get("login")
        .and_then(Value::as_str)
        .or_else(|| value.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn parse_label(value: &Value) -> Option<IssueLabel> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| value.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_owned();
    let color = value
        .get("color")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Some(IssueLabel { name, color })
}

/// Parse one issue object from `gh issue list --json`.
pub(crate) fn parse_issue(item: &Value) -> Option<IssueSummary> {
    let number = item.get("number").and_then(Value::as_u64)?;
    let state = match item
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "closed" => IssueState::Closed,
        _ => IssueState::Open,
    };
    let author = item.get("author").and_then(login_of).unwrap_or_default();
    let labels = item
        .get("labels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_label)
        .collect();
    let assignees = item
        .get("assignees")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(login_of)
        .collect();
    Some(IssueSummary {
        number,
        title: field_str(item, "title"),
        author,
        state,
        labels,
        assignees,
        body: field_str(item, "body"),
        updated_at: field_str(item, "updatedAt"),
        url: field_str(item, "url"),
    })
}

/// Parse a `gh issue list --json` array and cap it to `limit`, recording whether more existed.
pub(crate) fn parse_issue_list(value: &Value, limit: usize) -> IssueList {
    let mut issues: Vec<IssueSummary> = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(parse_issue)
        .collect();
    let has_more = issues.len() > limit;
    issues.truncate(limit);
    IssueList {
        provider: "github",
        issues,
        has_more,
    }
}

/// List a project's GitHub issues in `state` (`open`, `closed`, or `all`),
/// optionally narrowed by `search` and `labels`, capped at `limit`.
pub async fn list(
    cwd: &Path,
    state: &str,
    search: Option<&str>,
    labels: &[String],
    limit: usize,
) -> Result<IssueList, PullError> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let fetch = (limit + 1).to_string();
    let owned_args = build_list_args(state, search, labels, &fetch);
    let arg_refs: Vec<&str> = owned_args.iter().map(String::as_str).collect();
    let output = gh(cwd, &arg_refs, None).await?;
    let value: Value = serde_json::from_slice(&output).map_err(|error| PullError {
        code: "failed",
        message: format!("gh answered with unreadable JSON: {error}"),
    })?;
    Ok(parse_issue_list(&value, limit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_gh_issue_list_json_with_labels_assignees_and_has_more() {
        let payload = json!([
            {
                "number": 4475,
                "title": "Browse GitHub issues",
                "author": { "login": "lucas" },
                "state": "OPEN",
                "labels": [
                    { "name": "desktop", "color": "1d76db" },
                    { "name": "enhancement", "color": "" }
                ],
                "assignees": [
                    { "login": "alice" },
                    { "login": "bob" }
                ],
                "body": "Add an issues browser and fan-out action.",
                "updatedAt": "2026-11-26T09:00:00Z",
                "url": "https://github.com/LHagfoss/rustcode/issues/4475"
            },
            {
                "number": 4474,
                "title": "Worktree scripts",
                "author": null,
                "state": "CLOSED",
                "labels": [],
                "assignees": [],
                "body": null,
                "updatedAt": "2026-11-25T12:00:00Z",
                "url": "https://github.com/LHagfoss/rustcode/issues/4474"
            },
            {
                "title": "Malformed without number"
            }
        ]);

        let list = parse_issue_list(&payload, 1);
        assert_eq!(list.provider, "github");
        assert!(list.has_more);
        assert_eq!(list.issues.len(), 1);
        assert_eq!(
            list.issues[0],
            IssueSummary {
                number: 4475,
                title: "Browse GitHub issues".to_owned(),
                author: "lucas".to_owned(),
                state: IssueState::Open,
                labels: vec![
                    IssueLabel {
                        name: "desktop".to_owned(),
                        color: Some("1d76db".to_owned()),
                    },
                    IssueLabel {
                        name: "enhancement".to_owned(),
                        color: None,
                    },
                ],
                assignees: vec!["alice".to_owned(), "bob".to_owned()],
                body: "Add an issues browser and fan-out action.".to_owned(),
                updated_at: "2026-11-26T09:00:00Z".to_owned(),
                url: "https://github.com/LHagfoss/rustcode/issues/4475".to_owned(),
            }
        );

        let full = parse_issue_list(&payload, 10);
        assert!(!full.has_more);
        assert_eq!(full.issues.len(), 2);
        assert_eq!(full.issues[1].state, IssueState::Closed);
        assert_eq!(full.issues[1].author, "");
        assert_eq!(full.issues[1].body, "");
    }

    #[test]
    fn builds_gh_issue_list_args_with_filters_and_state_fallback() {
        let labels = vec![
            "bug".to_owned(),
            "   ".to_owned(),
            " good first issue ".to_owned(),
        ];
        let args = build_list_args("weird", Some("  worktree  "), &labels, "31");
        assert_eq!(
            args,
            vec![
                "issue",
                "list",
                "--state",
                "open",
                "--limit",
                "31",
                "--json",
                ISSUE_FIELDS,
                "--search",
                "worktree",
                "--label",
                "bug",
                "--label",
                "good first issue",
            ]
        );
    }

    #[test]
    fn builds_closed_and_all_states_without_empty_search() {
        let closed = build_list_args("closed", Some("   "), &[], "11");
        assert_eq!(closed[3], "closed");
        assert!(!closed.iter().any(|arg| arg == "--search"));

        let all = build_list_args("all", None, &[], "11");
        assert_eq!(all[3], "all");
    }
}
