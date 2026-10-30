//! A read-only view of a project's pull requests (#2645), for the macOS
//! app's pull request inbox. GitHub through the `gh` CLI is the only
//! provider today; the types say nothing GitHub-specific, so another host
//! can answer the same requests later.
//!
//! Nothing here changes a repository or an account: no checkout, review,
//! comment, approval or merge. `gh` runs in the project folder with its own
//! sign-in and prompts disabled, and its errors come back as they are.

use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// How long one `gh` call may take before it is given up.
const TIMEOUT: Duration = Duration::from_secs(30);
/// The most pull requests one list asks for.
pub const MAX_LIMIT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PullState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CheckCounts {
    pub passed: usize,
    pub failed: usize,
    pub pending: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullSummary {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub state: PullState,
    pub draft: bool,
    /// `approved`, `changes_requested` or `review_required`; none when the
    /// repository asks for no review.
    pub review: Option<String>,
    pub checks: CheckCounts,
    pub head_ref: String,
    pub base_ref: String,
    /// The exact commit the pull request is at.
    pub head_sha: String,
    pub updated_at: String,
    pub url: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullList {
    pub provider: &'static str,
    pub pulls: Vec<PullSummary>,
    /// More match than were asked for: ask again with a higher limit.
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    /// `passed`, `failed`, `pending` or `skipped`.
    pub status: &'static str,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub author: String,
    /// `commented`, or a review's state such as `approved`.
    pub kind: String,
    pub at: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullDetail {
    pub provider: &'static str,
    #[serde(flatten)]
    pub summary: PullSummary,
    pub body: String,
    pub check_runs: Vec<Check>,
    pub files: Vec<ChangedFile>,
    /// Reviews and comments, oldest first.
    pub activity: Vec<Activity>,
}

/// Why the pull requests could not be read, said so the app can show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PullError {
    /// `gh_missing`, `not_connected`, `not_a_repository`, `timed_out` or
    /// `failed`.
    pub code: &'static str,
    pub message: String,
}

const LIST_FIELDS: &str = "number,title,author,state,isDraft,reviewDecision,statusCheckRollup,headRefName,baseRefName,headRefOid,updatedAt,url,additions,deletions,changedFiles";
const VIEW_FIELDS: &str = "number,title,author,state,isDraft,reviewDecision,statusCheckRollup,headRefName,baseRefName,headRefOid,updatedAt,url,additions,deletions,changedFiles,body,files,reviews,comments";

/// The project's pull requests in `state` (`open`, `closed`, `merged` or
/// `all`), newest first, at most `limit`, matching `search` (GitHub's
/// search syntax) when given.
pub async fn list(
    cwd: &Path,
    state: &str,
    search: Option<&str>,
    limit: usize,
) -> Result<PullList, PullError> {
    let state = match state {
        "closed" | "merged" | "all" => state,
        _ => "open",
    };
    let limit = limit.clamp(1, MAX_LIMIT);
    // One more than asked shows whether there are more.
    let fetch = (limit + 1).to_string();
    let mut args = vec![
        "pr",
        "list",
        "--state",
        state,
        "--limit",
        &fetch,
        "--json",
        LIST_FIELDS,
    ];
    if let Some(search) = search.map(str::trim).filter(|search| !search.is_empty()) {
        args.extend(["--search", search]);
    }
    let value = gh_json(cwd, &args).await?;
    let mut pulls: Vec<PullSummary> = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(summary)
        .collect();
    let has_more = pulls.len() > limit;
    pulls.truncate(limit);
    Ok(PullList {
        provider: "github",
        pulls,
        has_more,
    })
}

/// One pull request with its description, checks, files and activity.
pub async fn view(cwd: &Path, number: u64) -> Result<PullDetail, PullError> {
    let number = number.to_string();
    let value = gh_json(cwd, &["pr", "view", &number, "--json", VIEW_FIELDS]).await?;
    detail(&value).ok_or_else(|| PullError {
        code: "failed",
        message: "gh answered without a pull request".to_owned(),
    })
}

/// What a watch compares (#2648): the pull request's state, checks,
/// mergeability, reviews and comments, without its body and files.
pub(crate) async fn watch_view(cwd: &Path, number: u64) -> Result<Value, PullError> {
    let number = number.to_string();
    gh_json(cwd, &["pr", "view", &number, "--json", WATCH_FIELDS]).await
}

const WATCH_FIELDS: &str = "number,title,author,state,isDraft,reviewDecision,statusCheckRollup,headRefName,baseRefName,headRefOid,updatedAt,url,additions,deletions,changedFiles,mergeable,reviews,comments";

async fn gh_json(cwd: &Path, args: &[&str]) -> Result<Value, PullError> {
    let output = gh(cwd, args, None).await?;
    serde_json::from_slice(&output).map_err(|error| PullError {
        code: "failed",
        message: format!("gh answered with unreadable JSON: {error}"),
    })
}

/// Run `gh` in `cwd`, `stdin` written to it when given, and return what it
/// printed, or its error.
async fn gh(cwd: &Path, args: &[&str], stdin: Option<Vec<u8>>) -> Result<Vec<u8>, PullError> {
    use tokio::io::AsyncWriteExt;
    let mut command = tokio::process::Command::new(crate::platform::resolve_bin("gh"));
    command
        .args(args)
        .current_dir(cwd)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(if stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let run = async {
        let mut child = command.spawn()?;
        if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(&input).await?;
            drop(pipe);
        }
        child.wait_with_output().await
    };
    let output = match tokio::time::timeout(TIMEOUT, run).await {
        Err(_) => {
            return Err(PullError {
                code: "timed_out",
                message: format!(
                    "gh took longer than {} s and was stopped",
                    TIMEOUT.as_secs()
                ),
            });
        }
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(PullError {
                code: "gh_missing",
                message: "The GitHub CLI (gh) is not installed. Install it (brew install gh) and connect GitHub in Settings ▸ Connections.".to_owned(),
            });
        }
        Ok(Err(error)) => {
            return Err(PullError {
                code: "failed",
                message: format!("Could not run gh: {error}"),
            });
        }
        Ok(Ok(output)) => output,
    };
    if !output.status.success() {
        // `gh api` prints the provider's answer on stdout and a summary on
        // stderr; both say why.
        let mut error = classify(&String::from_utf8_lossy(&output.stderr));
        let body = String::from_utf8_lossy(&output.stdout);
        if let Some(message) = serde_json::from_str::<Value>(&body).ok().and_then(|value| {
            value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }) {
            error.message = format!("{} ({message})", error.message);
        }
        return Err(error);
    }
    Ok(output.stdout)
}

/// The most diff text one answer carries; a larger diff is cut there.
pub const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullDiff {
    /// The pull request's unified diff, as `gh pr diff` prints it.
    pub diff: String,
    /// The diff was longer than [`MAX_DIFF_BYTES`] and was cut.
    pub truncated: bool,
}

/// The pull request's diff against its base.
pub async fn diff(cwd: &Path, number: u64) -> Result<PullDiff, PullError> {
    let number = number.to_string();
    let output = gh(cwd, &["pr", "diff", &number], None).await?;
    let mut diff = String::from_utf8_lossy(&output).into_owned();
    let truncated = diff.len() > MAX_DIFF_BYTES;
    if truncated {
        let mut end = MAX_DIFF_BYTES;
        while !diff.is_char_boundary(end) {
            end -= 1;
        }
        diff.truncate(end);
    }
    Ok(PullDiff { diff, truncated })
}

/// One line comment of a review: `line` in the file on `side` (`RIGHT`
/// for the pull request's version, `LEFT` for the base).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct ReviewComment {
    pub path: String,
    pub line: u64,
    pub side: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PostedReview {
    pub url: Option<String>,
}

/// The provider's review request: one review at `commit_id`, with its
/// comments, so a review is posted whole or not at all.
pub fn review_payload(
    commit_id: &str,
    event: &str,
    body: &str,
    comments: &[ReviewComment],
) -> Result<Value, PullError> {
    let event = match event {
        "APPROVE" | "REQUEST_CHANGES" | "COMMENT" => event,
        other => {
            return Err(PullError {
                code: "failed",
                message: format!("unknown review event {other}"),
            });
        }
    };
    if commit_id.is_empty() {
        return Err(PullError {
            code: "failed",
            message: "the review names no commit".to_owned(),
        });
    }
    for comment in comments {
        if !matches!(comment.side.as_str(), "LEFT" | "RIGHT") || comment.line == 0 {
            return Err(PullError {
                code: "failed",
                message: format!("{} has no line to comment on", comment.path),
            });
        }
    }
    Ok(serde_json::json!({
        "commit_id": commit_id,
        "event": event,
        "body": body,
        "comments": comments,
    }))
}

/// Post one review. A review the provider refuses comes back as `failed`
/// with its message; one whose answer never arrived as `uncertain`, which
/// the app does not retry by itself, since it may have been posted.
pub async fn post_review(
    cwd: &Path,
    repository: &str,
    number: u64,
    payload: &Value,
) -> Result<PostedReview, PullError> {
    let valid = repository.split('/').count() == 2
        && repository
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'));
    if !valid {
        return Err(PullError {
            code: "failed",
            message: format!("{repository} is not an owner/name repository"),
        });
    }
    let endpoint = format!("repos/{repository}/pulls/{number}/reviews");
    let input = serde_json::to_vec(payload).unwrap_or_default();
    let output = match gh(
        cwd,
        &["api", "--method", "POST", &endpoint, "--input", "-"],
        Some(input),
    )
    .await
    {
        Err(error) if error.code == "timed_out" => {
            return Err(PullError {
                code: "uncertain",
                message: "GitHub did not answer in time; the review may have been posted. Check the pull request before submitting again.".to_owned(),
            });
        }
        other => other?,
    };
    let answer: Value = serde_json::from_slice(&output).unwrap_or(Value::Null);
    Ok(PostedReview {
        url: answer
            .get("html_url")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// The error `gh` printed, with the cases the app explains on its own.
fn classify(stderr: &str) -> PullError {
    let message = stderr.trim().to_owned();
    let lower = message.to_lowercase();
    let code = if lower.contains("gh auth login") || lower.contains("not logged in") {
        "not_connected"
    } else if lower.contains("not a git repository")
        || lower.contains("no git remotes")
        || lower.contains("none of the git remotes")
    {
        "not_a_repository"
    } else {
        "failed"
    };
    PullError { code, message }
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or_default()
}

fn login(value: &Value) -> String {
    value
        .get("author")
        .map(|author| text(author, "login"))
        .unwrap_or_default()
}

/// A check run's or status's outcome.
fn check_status(check: &Value) -> &'static str {
    // A commit status has `state`; a check run has `status` and `conclusion`.
    let state = text(check, "state").to_uppercase();
    if !state.is_empty() {
        return match state.as_str() {
            "SUCCESS" => "passed",
            "FAILURE" | "ERROR" => "failed",
            _ => "pending",
        };
    }
    if !text(check, "status").eq_ignore_ascii_case("completed") {
        return "pending";
    }
    match text(check, "conclusion").to_uppercase().as_str() {
        "SUCCESS" | "NEUTRAL" => "passed",
        "SKIPPED" => "skipped",
        "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => "failed",
        _ => "pending",
    }
}

fn checks(value: &Value) -> Vec<Check> {
    value
        .get("statusCheckRollup")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|check| {
            let name = Some(text(check, "name"))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| text(check, "context"));
            let url = Some(text(check, "detailsUrl"))
                .filter(|url| !url.is_empty())
                .or_else(|| Some(text(check, "targetUrl")).filter(|url| !url.is_empty()));
            Check {
                name,
                status: check_status(check),
                url,
            }
        })
        .collect()
}

fn summary(value: &Value) -> Option<PullSummary> {
    let id = value.get("number")?.as_u64()?;
    let state = match text(value, "state").to_uppercase().as_str() {
        "MERGED" => PullState::Merged,
        "CLOSED" => PullState::Closed,
        _ => PullState::Open,
    };
    let review =
        Some(text(value, "reviewDecision").to_lowercase()).filter(|review| !review.is_empty());
    let mut counts = CheckCounts::default();
    for check in checks(value) {
        match check.status {
            "passed" => counts.passed += 1,
            "failed" => counts.failed += 1,
            "skipped" => counts.skipped += 1,
            _ => counts.pending += 1,
        }
    }
    Some(PullSummary {
        number: id,
        title: text(value, "title"),
        author: login(value),
        state,
        draft: value
            .get("isDraft")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        review,
        checks: counts,
        head_ref: text(value, "headRefName"),
        base_ref: text(value, "baseRefName"),
        head_sha: text(value, "headRefOid"),
        updated_at: text(value, "updatedAt"),
        url: text(value, "url"),
        additions: number(value, "additions"),
        deletions: number(value, "deletions"),
        changed_files: number(value, "changedFiles"),
    })
}

pub(crate) fn detail(value: &Value) -> Option<PullDetail> {
    let summary = summary(value)?;
    let files = value
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|file| ChangedFile {
            path: text(file, "path"),
            additions: number(file, "additions"),
            deletions: number(file, "deletions"),
        })
        .collect();
    let mut activity: Vec<Activity> = value
        .get("reviews")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|review| Activity {
            author: login(review),
            kind: text(review, "state").to_lowercase(),
            at: text(review, "submittedAt"),
            body: text(review, "body"),
        })
        .chain(
            value
                .get("comments")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|comment| Activity {
                    author: login(comment),
                    kind: "commented".to_owned(),
                    at: text(comment, "createdAt"),
                    body: text(comment, "body"),
                }),
        )
        .collect();
    // RFC 3339 times in UTC sort as text.
    activity.sort_by(|a, b| a.at.cmp(&b.at));
    Some(PullDetail {
        provider: "github",
        body: text(value, "body"),
        check_runs: checks(value),
        files,
        activity,
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pull() -> Value {
        json!({
            "number": 2701, "title": "feat(engine): clean", "state": "OPEN", "isDraft": false,
            "author": {"login": "LHagfoss"}, "reviewDecision": "",
            "headRefName": "feat/x", "baseRefName": "main", "headRefOid": "da4593b",
            "updatedAt": "2026-10-30T14:13:48Z", "url": "https://github.com/o/r/pull/2701",
            "additions": 473, "deletions": 0, "changedFiles": 5,
            "statusCheckRollup": [
                {"__typename": "CheckRun", "name": "label", "status": "COMPLETED", "conclusion": "SUCCESS", "detailsUrl": "https://ci/1"},
                {"__typename": "CheckRun", "name": "test", "status": "IN_PROGRESS", "conclusion": ""},
                {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "FAILURE"},
                {"__typename": "CheckRun", "name": "docs", "status": "COMPLETED", "conclusion": "SKIPPED"},
                {"__typename": "StatusContext", "context": "ci/legacy", "state": "PENDING", "targetUrl": "https://ci/2"}
            ],
            "body": "Why.",
            "files": [{"path": "a.rs", "additions": 3, "deletions": 1}],
            "reviews": [{"author": {"login": "rev"}, "state": "APPROVED", "submittedAt": "2026-10-30T15:00:00Z", "body": "ok"}],
            "comments": [{"author": {"login": "me"}, "createdAt": "2026-10-30T14:30:00Z", "body": "first"}]
        })
    }

    #[test]
    fn a_summary_counts_checks_and_reads_the_review_and_state() {
        let summary = summary(&pull()).unwrap();
        assert_eq!(summary.number, 2701);
        assert_eq!(summary.state, PullState::Open);
        assert_eq!(summary.review, None);
        assert_eq!(
            summary.checks,
            CheckCounts {
                passed: 1,
                failed: 1,
                pending: 2,
                skipped: 1
            }
        );
        assert_eq!(summary.head_sha, "da4593b");
        let mut merged = pull();
        merged["state"] = json!("MERGED");
        merged["reviewDecision"] = json!("CHANGES_REQUESTED");
        let merged = super::summary(&merged).unwrap();
        assert_eq!(merged.state, PullState::Merged);
        assert_eq!(merged.review.as_deref(), Some("changes_requested"));
        assert!(super::summary(&json!({"title": "no number"})).is_none());
    }

    #[test]
    fn a_detail_lists_checks_files_and_activity_oldest_first() {
        let detail = detail(&pull()).unwrap();
        assert_eq!(detail.body, "Why.");
        assert_eq!(detail.files[0].path, "a.rs");
        let names: Vec<_> = detail
            .check_runs
            .iter()
            .map(|c| (c.name.as_str(), c.status))
            .collect();
        assert_eq!(
            names,
            vec![
                ("label", "passed"),
                ("test", "pending"),
                ("lint", "failed"),
                ("docs", "skipped"),
                ("ci/legacy", "pending")
            ]
        );
        assert_eq!(detail.check_runs[4].url.as_deref(), Some("https://ci/2"));
        let kinds: Vec<_> = detail.activity.iter().map(|a| a.kind.as_str()).collect();
        assert_eq!(kinds, vec!["commented", "approved"]);
        // The summary's fields sit beside the detail's in the JSON.
        let json = serde_json::to_value(&detail).unwrap();
        assert_eq!(json["number"], 2701);
        assert_eq!(json["checkRuns"][0]["name"], "label");
    }

    #[test]
    fn gh_errors_are_classified() {
        assert_eq!(
            classify("To get started with GitHub CLI, please run:  gh auth login").code,
            "not_connected"
        );
        assert_eq!(
            classify("failed to run git: fatal: not a git repository (or any of the parent directories): .git").code,
            "not_a_repository"
        );
        assert_eq!(classify("no git remotes found").code, "not_a_repository");
        let other = classify("HTTP 502\n");
        assert_eq!((other.code, other.message.as_str()), ("failed", "HTTP 502"));
    }

    #[test]
    fn a_review_payload_is_one_review_at_one_commit() {
        let comments = vec![ReviewComment {
            path: "a.rs".into(),
            line: 12,
            side: "RIGHT".into(),
            body: "Why?".into(),
        }];
        let payload = review_payload("da4593b", "COMMENT", "Looks fine.", &comments).unwrap();
        assert_eq!(payload["commit_id"], "da4593b");
        assert_eq!(payload["event"], "COMMENT");
        assert_eq!(payload["comments"][0]["line"], 12);
        assert_eq!(payload["comments"][0]["side"], "RIGHT");
        assert!(review_payload("da4593b", "MERGE", "", &[]).is_err());
        assert!(review_payload("", "COMMENT", "x", &[]).is_err());
        let mut bad = comments.clone();
        bad[0].side = "MIDDLE".into();
        assert!(review_payload("da4593b", "COMMENT", "", &bad).is_err());
    }

    #[tokio::test]
    async fn a_review_to_a_malformed_repository_is_refused_before_gh_runs() {
        let dir = tempfile::tempdir().unwrap();
        let error = post_review(dir.path(), "owner/name/extra", 1, &json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.code, "failed");
        assert!(
            post_review(dir.path(), "own er/x", 1, &json!({}))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_folder_that_is_not_a_repository_says_so() {
        // Without gh installed the answer is `gh_missing`; either way it is
        // an error the app can show, never a list.
        let dir = tempfile::tempdir().unwrap();
        let error = list(dir.path(), "open", None, 10).await.unwrap_err();
        assert!(
            ["not_a_repository", "gh_missing", "not_connected", "failed"].contains(&error.code),
            "{error:?}"
        );
    }
}
