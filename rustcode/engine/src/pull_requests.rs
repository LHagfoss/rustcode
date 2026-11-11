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

const LIST_FIELDS: &str = "number,title,author,state,isDraft,reviewDecision,statusCheckRollup,headRefName,baseRefName,headRefOid,headRepository,headRepositoryOwner,isCrossRepository,updatedAt,url,additions,deletions,changedFiles";
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
    watch_view_on(cwd, "", number).await
}

/// The same read for `repo` (`owner/name`) when one is named: `gh --repo`
/// answers without the folder being that repository, so a link (#3369) keeps
/// working after the worktree it was made in is gone.
pub(crate) async fn watch_view_on(cwd: &Path, repo: &str, number: u64) -> Result<Value, PullError> {
    let number = number.to_string();
    let mut args = vec!["pr", "view", &number, "--json", WATCH_FIELDS];
    if is_repository(repo) {
        args.extend(["--repo", repo]);
    }
    gh_json(cwd, &args).await
}

/// Whether `text` is an `owner/name` repository, the only spelling `gh --repo`
/// takes: two non-empty parts and no `.` segment.
pub(crate) fn is_repository(text: &str) -> bool {
    let text = text.trim();
    let mut parts = text.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !owner.is_empty()
        && !name.is_empty()
        && !matches!(owner, "." | "..")
        && !matches!(name, "." | "..")
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
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
    let mut command = crate::platform::tokio_command(crate::platform::resolve_bin("gh"));
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
                message: format!(
                    "The GitHub CLI (gh) is not installed. Install it ({}) and connect GitHub in Settings ▸ Connections.",
                    crate::install_hints::host_hint(crate::install_hints::Tool::GitHub)
                ),
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

// ------------------------------------------------ a delegate's pull request

/// The checks a merge never waits for: the advisory portability jobs in the
/// repository's CI, which run on another platform and never block a merge.
/// `scripts/pr-merge.sh` skips the same jobs.
pub const ADVISORY_CHECKS: [&str; 1] = ["portability coverage"];

/// Whether `name` is one of the advisory jobs a merge ignores.
pub fn advisory_check(name: &str) -> bool {
    let name = name.to_lowercase();
    ADVISORY_CHECKS
        .iter()
        .any(|advisory| name.contains(advisory))
}

/// The state of a delegate's checks, as one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CiState {
    /// The pull request has no checks at all.
    NoChecks,
    Passing,
    Pending,
    Failing,
}

impl CiState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoChecks => "none",
            Self::Passing => "passing",
            Self::Pending => "pending",
            Self::Failing => "failing",
        }
    }
}

/// What the checks say: failing beats pending beats passing, and a pull
/// request with no checks is neither.
pub fn ci_state(checks: &[Check]) -> CiState {
    if checks.is_empty() {
        return CiState::NoChecks;
    }
    if checks.iter().any(|check| check.status == "failed") {
        return CiState::Failing;
    }
    if checks.iter().any(|check| check.status == "pending") {
        return CiState::Pending;
    }
    CiState::Passing
}

/// The names of the checks that failed, in the order the host listed them.
pub fn failing_checks(checks: &[Check]) -> Vec<String> {
    checks
        .iter()
        .filter(|check| check.status == "failed")
        .map(|check| check.name.clone())
        .collect()
}

/// A delegate's pull request, as `thread_status` reports it: which one it is,
/// where it stands, and what its checks say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegatePull {
    pub number: u64,
    pub title: String,
    /// `open`, `closed` or `merged`.
    pub state: PullState,
    pub merged: bool,
    pub draft: bool,
    pub url: String,
    pub branch: String,
    /// The exact commit the pull request is at: cleanup proves the worktree's
    /// commits are part of it before honouring a merge.
    pub head_sha: String,
    pub ci: CiState,
    /// The checks that failed, by name: what the orchestrator tells the delegate.
    pub failing: Vec<String>,
    pub pending: usize,
    pub passed: usize,
}

/// A pull request from a `gh pr list --json` answer, with its checks mapped.
pub fn delegate_pull(value: &Value) -> Option<DelegatePull> {
    let summary = summary(value)?;
    // The advisory portability jobs never hold a merge, so they are not part of
    // the state either: a failing one would contradict `merge_when_green`.
    let checks: Vec<Check> = checks(value)
        .into_iter()
        .filter(|check| !advisory_check(&check.name))
        .collect();
    Some(DelegatePull {
        number: summary.number,
        title: summary.title,
        state: summary.state,
        merged: summary.state == PullState::Merged,
        draft: summary.draft,
        url: summary.url,
        branch: summary.head_ref,
        head_sha: summary.head_sha,
        ci: ci_state(&checks),
        failing: failing_checks(&checks),
        pending: checks
            .iter()
            .filter(|check| check.status == "pending")
            .count(),
        passed: checks
            .iter()
            .filter(|check| check.status == "passed")
            .count(),
    })
}

/// The branch a folder is on, or `None` when it is not a git checkout.
pub fn current_branch(cwd: &Path) -> Option<String> {
    crate::worktrees::git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .map(|branch| branch.trim().to_owned())
        .filter(|branch| !branch.is_empty() && branch != "HEAD")
}

/// Whether a `gh pr list` entry's head is this repository, not a same-named
/// branch on someone's fork: `gh pr list --head` matches both. `None` means gh
/// did not say enough to tell, which is never taken as "ours". The pull
/// request's URL names the repository it is open against, so the head's
/// `nameWithOwner` has to equal it; on a host whose URL is not GitHub's,
/// `isCrossRepository` alone says the head is that repository.
fn head_is_this_repository(value: &Value) -> Option<bool> {
    let cross = value.get("isCrossRepository")?.as_bool()?;
    let head = value
        .get("headRepository")?
        .get("nameWithOwner")?
        .as_str()?;
    let owner = value.get("headRepositoryOwner")?.get("login")?.as_str()?;
    let (head_owner, _) = head.split_once('/')?;
    if head_owner != owner || cross {
        return Some(false);
    }
    Some(repository_of(&text(value, "url")).is_none_or(|base| base == head))
}

/// The pull request to report for one branch: only one whose head is this
/// repository (a fork's same-named branch is not the delegate's), an open one
/// first (a branch reused after its old pull request merged), then a merged one,
/// then closed. An entry gh could not place in a repository refuses the whole
/// answer instead of a guess, so a fork's pull request is never reported as
/// this repository's.
fn choose_for_branch(pulls: &[Value]) -> Result<Option<DelegatePull>, PullError> {
    let rank = |value: &Value| match text(value, "state").to_uppercase().as_str() {
        "OPEN" => 0,
        "MERGED" => 1,
        _ => 2,
    };
    let mut candidates: Vec<(u8, usize, &Value)> = Vec::new();
    for (index, value) in pulls.iter().enumerate() {
        let ours = head_is_this_repository(value).ok_or_else(|| PullError {
            code: "failed",
            message: "gh did not say which repository a matching pull request's head is in; \
                      refusing to guess between this repository and a fork"
                .to_owned(),
        })?;
        if ours {
            candidates.push((rank(value), index, value));
        }
    }
    Ok(candidates
        .into_iter()
        .min_by_key(|(rank, index, _)| (*rank, *index))
        .and_then(|(_, _, value)| delegate_pull(value)))
}

/// The newest pull request whose head is `branch`: an open one first (a branch
/// reused after its old pull request merged), then a merged one, then closed.
pub async fn for_branch(cwd: &Path, branch: &str) -> Result<Option<DelegatePull>, PullError> {
    let value = gh_json(
        cwd,
        &[
            "pr",
            "list",
            "--head",
            branch,
            "--state",
            "all",
            "--limit",
            "10",
            "--json",
            LIST_FIELDS,
        ],
    )
    .await?;
    let pulls: Vec<Value> = value.as_array().cloned().unwrap_or_default();
    choose_for_branch(&pulls)
}

/// How often a merge re-reads the checks, and how long it waits for them. The
/// cap is 25 minutes, like `wait_session`: an MCP client aborts a call that
/// stays silent for about half an hour, and `pending` lets the caller ask again.
pub const MERGE_POLL_SECS: u64 = 20;
pub const MAX_MERGE_WAIT_SECS: u64 = 25 * 60;

/// The `gh pr merge` arguments: squash and delete the remote branch, with
/// `--repo` so gh leaves the local branch (checked out in the delegate's
/// worktree) alone.
pub(crate) fn merge_args(number: u64, repository: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "pr".to_owned(),
        "merge".to_owned(),
        number.to_string(),
        "--squash".to_owned(),
        "--delete-branch".to_owned(),
    ];
    if let Some(repository) = repository {
        args.push("--repo".to_owned());
        args.push(repository.to_owned());
    }
    args
}

/// `owner/name` from a pull request's URL, for `gh --repo`.
pub(crate) fn repository_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/');
    let owner = parts.next().filter(|part| !part.is_empty())?;
    let name = parts.next().filter(|part| !part.is_empty())?;
    if parts.next()? != "pull" {
        return None;
    }
    let number = parts.next()?;
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) || parts.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

/// What `merge_when_green` did: merged, or why it stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeOutcome {
    pub number: u64,
    /// `merged`, `failing`, `closed`, `no_checks`, `pending` (still waiting when
    /// the time ran out) or `failed`.
    pub state: &'static str,
    pub merged: bool,
    /// The checks that failed, when `state` is `failing`.
    pub failing: Vec<String>,
    pub url: String,
    pub waited_secs: u64,
    pub message: String,
}

/// What one look at the checks says to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PollAction {
    Wait,
    Merge,
    StopFailing,
    StopNoChecks,
}

/// The polling rules of `scripts/pr-merge.sh`, as a state machine: two quiet
/// rounds catch a path-filtered job that registers late, a failing check stops
/// it, and a pull request with no checks stops after two empty rounds instead
/// of waiting out the whole budget for checks that will never appear.
#[derive(Debug, Default)]
pub(crate) struct MergePoll {
    quiet: u32,
    empty: u32,
}

impl MergePoll {
    pub(crate) fn observe(&mut self, checks: &[Check]) -> PollAction {
        if checks.iter().any(|check| check.status == "failed") {
            return PollAction::StopFailing;
        }
        if checks.is_empty() {
            self.quiet = 0;
            self.empty += 1;
            return if self.empty >= 2 {
                PollAction::StopNoChecks
            } else {
                PollAction::Wait
            };
        }
        self.empty = 0;
        if checks.iter().any(|check| check.status == "pending") {
            self.quiet = 0;
            return PollAction::Wait;
        }
        self.quiet += 1;
        if self.quiet >= 2 {
            PollAction::Merge
        } else {
            PollAction::Wait
        }
    }
}

/// The merges running now, by the id their caller gave (the orchestration call
/// id): a call that was dropped (the harness stopped waiting, the MCP server
/// answered `pending`) cancels its merge, so one that was reported as not
/// merged can never merge afterwards.
static MERGES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, tokio_util::sync::CancellationToken>>,
> = std::sync::LazyLock::new(Default::default);

/// Start tracking a merge under `id`; the returned token ends it.
pub fn register_merge(id: &str) -> tokio_util::sync::CancellationToken {
    let token = tokio_util::sync::CancellationToken::new();
    MERGES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(id.to_owned(), token.clone());
    token
}

/// Stop the merge tracked under `id`, if any is still running.
pub fn cancel_merge(id: &str) -> bool {
    let token = MERGES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(id)
        .cloned();
    token.inspect(|token| token.cancel()).is_some()
}

/// Forgets a merge when it ends, however it ends.
pub struct MergeGuard(pub Option<String>);

impl Drop for MergeGuard {
    fn drop(&mut self) {
        if let Some(id) = self.0.take() {
            MERGES
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&id);
        }
    }
}

fn cancelled() -> PullError {
    PullError {
        code: "cancelled",
        message: "the call that asked for this merge was dropped; nothing was merged".to_owned(),
    }
}

/// Wait for every check except the advisory ones, then squash-merge and delete
/// the branch, as `scripts/pr-merge.sh` does. Two quiet rounds in a row are
/// needed, so a path-filtered job that registers late is still caught. A
/// failing check stops it before anything merges, and so does a pull request
/// with no checks at all. `cancel` ends it with a `cancelled` error at any
/// point before `gh pr merge` starts.
pub async fn merge_when_green(
    cwd: &Path,
    number: u64,
    wait: Duration,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<MergeOutcome, PullError> {
    let started = std::time::Instant::now();
    let wait = wait.clamp(
        Duration::from_secs(MERGE_POLL_SECS),
        Duration::from_secs(MAX_MERGE_WAIT_SECS),
    );
    let mut poll = MergePoll::default();
    loop {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        let value = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(cancelled()),
            value = watch_view(cwd, number) => value?,
        };
        let Some(detail) = detail(&value) else {
            return Err(PullError {
                code: "failed",
                message: "gh answered without a pull request".to_owned(),
            });
        };
        let waited_secs = started.elapsed().as_secs();
        if detail.summary.state == PullState::Merged {
            return Ok(MergeOutcome {
                number,
                state: "merged",
                merged: true,
                failing: Vec::new(),
                url: detail.summary.url,
                waited_secs,
                message: "it was already merged".to_owned(),
            });
        }
        if detail.summary.state == PullState::Closed {
            return Ok(MergeOutcome {
                number,
                state: "closed",
                merged: false,
                failing: Vec::new(),
                url: detail.summary.url,
                waited_secs,
                message: "the pull request was closed without merging".to_owned(),
            });
        }
        let checks: Vec<Check> = detail
            .check_runs
            .into_iter()
            .filter(|check| !advisory_check(&check.name))
            .collect();
        let failing = failing_checks(&checks);
        if !failing.is_empty() {
            return Ok(MergeOutcome {
                number,
                state: "failing",
                merged: false,
                failing,
                url: detail.summary.url,
                waited_secs,
                message: "a check failed; nothing was merged".to_owned(),
            });
        }
        match poll.observe(&checks) {
            PollAction::Merge => {
                // The last point to stop: once `gh pr merge` runs it lands.
                if cancel.is_cancelled() {
                    return Err(cancelled());
                }
                let repository = repository_of(&detail.summary.url);
                let args = merge_args(number, repository.as_deref());
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                match gh(cwd, &args, None).await {
                    Ok(_) => {
                        return Ok(MergeOutcome {
                            number,
                            state: "merged",
                            merged: true,
                            failing: Vec::new(),
                            url: detail.summary.url,
                            waited_secs: started.elapsed().as_secs(),
                            message: "squash-merged and the remote branch was deleted".to_owned(),
                        });
                    }
                    Err(error) => {
                        // gh can still exit non-zero after the merge landed (a
                        // race on the branch delete). The pull request is the
                        // truth: if it merged, report that and keep the note.
                        let after = watch_view(cwd, number).await?;
                        let merged = crate::pull_requests::detail(&after)
                            .is_some_and(|after| after.summary.state == PullState::Merged);
                        if merged {
                            return Ok(MergeOutcome {
                                number,
                                state: "merged",
                                merged: true,
                                failing: Vec::new(),
                                url: detail.summary.url,
                                waited_secs: started.elapsed().as_secs(),
                                message: format!("squash-merged; gh reported: {}", error.message),
                            });
                        }
                        return Err(error);
                    }
                }
            }
            PollAction::StopNoChecks => {
                return Ok(MergeOutcome {
                    number,
                    state: "no_checks",
                    merged: false,
                    failing: Vec::new(),
                    url: detail.summary.url,
                    waited_secs: started.elapsed().as_secs(),
                    message: "the pull request has no checks to wait for; nothing was merged"
                        .to_owned(),
                });
            }
            PollAction::StopFailing | PollAction::Wait => {}
        }
        let elapsed = started.elapsed();
        if elapsed + Duration::from_secs(MERGE_POLL_SECS) > wait {
            return Ok(MergeOutcome {
                number,
                state: "pending",
                merged: false,
                failing: Vec::new(),
                url: detail.summary.url,
                waited_secs: elapsed.as_secs(),
                message: format!("checks were still pending after {} s", elapsed.as_secs()),
            });
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(cancelled()),
            _ = tokio::time::sleep(Duration::from_secs(MERGE_POLL_SECS)) => {}
        }
    }
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
        // A cancelled run is not a failing one: a newer run superseded it, or
        // someone stopped it, and `scripts/pr-merge.sh` does not block on it.
        "CANCELLED" => "skipped",
        "FAILURE" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => "failed",
        _ => "pending",
    }
}

/// When a check ran, for telling a superseded run from the newest one.
fn check_time(check: &Value) -> String {
    ["completedAt", "startedAt"]
        .iter()
        .find_map(|key| Some(text(check, key)).filter(|time| !time.is_empty()))
        .unwrap_or_default()
}

fn checks(value: &Value) -> Vec<Check> {
    // A re-run leaves two entries under one name: the latest one is the state,
    // the older one is history. `statusCheckRollup` need not be sorted, so the
    // timestamp decides, and an entry without one never replaces a newer one.
    let mut latest: Vec<(String, Check)> = Vec::new();
    for check in value
        .get("statusCheckRollup")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = Some(text(check, "name"))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| text(check, "context"));
        let url = Some(text(check, "detailsUrl"))
            .filter(|url| !url.is_empty())
            .or_else(|| Some(text(check, "targetUrl")).filter(|url| !url.is_empty()));
        let at = check_time(check);
        let mapped = Check {
            name: name.clone(),
            status: check_status(check),
            url,
        };
        match latest.iter().position(|(_, check)| check.name == name) {
            Some(index) => {
                if at >= latest[index].0 {
                    latest[index].0 = at;
                    latest[index].1 = mapped;
                }
            }
            None => latest.push((at, mapped)),
        }
    }
    latest.into_iter().map(|(_, check)| check).collect()
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

    #[tokio::test]
    async fn a_cancelled_merge_stops_before_it_asks_github_or_merges() {
        let token = register_merge("call-merge-test");
        assert!(cancel_merge("call-merge-test"));
        // `gh` is never run: a cancelled merge returns before its first look.
        let outcome = merge_when_green(
            Path::new("/nonexistent"),
            1,
            Duration::from_secs(60),
            &token,
        )
        .await;
        let error = outcome.expect_err("a cancelled merge is not an outcome");
        assert_eq!(error.code, "cancelled");
        drop(MergeGuard(Some("call-merge-test".to_owned())));
        assert!(
            !cancel_merge("call-merge-test"),
            "a finished merge is forgotten"
        );
    }

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

    fn check(name: &str, status: &'static str) -> Check {
        Check {
            name: name.to_owned(),
            status,
            url: None,
        }
    }

    #[test]
    fn a_delegates_checks_map_to_pending_passing_and_failing() {
        assert_eq!(ci_state(&[]), CiState::NoChecks);
        assert_eq!(ci_state(&[]).as_str(), "none");
        assert_eq!(ci_state(&[check("test", "passed")]), CiState::Passing);
        assert_eq!(
            ci_state(&[check("test", "passed"), check("docs", "skipped")]),
            CiState::Passing,
            "a skipped check is not pending"
        );
        assert_eq!(
            ci_state(&[check("test", "passed"), check("build", "pending")]),
            CiState::Pending
        );
        // Failing wins over pending, so the names are reported at once.
        let mixed = [
            check("lint", "failed"),
            check("build", "pending"),
            check("test", "failed"),
        ];
        assert_eq!(ci_state(&mixed), CiState::Failing);
        assert_eq!(failing_checks(&mixed), ["lint", "test"]);
        assert_eq!(ci_state(&mixed).as_str(), "failing");
    }

    #[test]
    fn a_cancelled_run_is_not_a_failing_check() {
        let value = json!({
            "number": 1, "state": "OPEN", "statusCheckRollup": [
                {"__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "SUCCESS"},
                {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "CANCELLED"}
            ]
        });
        let state = delegate_pull(&value).unwrap();
        assert_eq!(state.failing, Vec::<String>::new());
        assert_eq!(state.ci, CiState::Passing);
    }

    #[test]
    fn a_superseded_check_run_of_the_same_name_uses_the_latest() {
        let value = json!({"statusCheckRollup": [
            {"__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "FAILURE",
             "completedAt": "2026-10-30T15:00:00Z"},
            {"__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "SUCCESS",
             "completedAt": "2026-10-30T16:00:00Z"}
        ]});
        let checks = checks(&value);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, "passed");
    }

    #[test]
    fn advisory_checks_are_not_part_of_the_reported_state() {
        let mut value = pull();
        value["statusCheckRollup"] = json!([
            {"__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"__typename": "CheckRun", "name": "Windows and macOS portability coverage",
             "status": "COMPLETED", "conclusion": "FAILURE"}
        ]);
        let state = delegate_pull(&value).unwrap();
        assert_eq!(state.ci, CiState::Passing);
        assert_eq!(state.failing, Vec::<String>::new());
        assert_eq!(state.passed, 1);
    }

    #[test]
    fn a_pull_request_with_no_checks_stops_instead_of_waiting() {
        let mut poll = MergePoll::default();
        assert_eq!(poll.observe(&[]), PollAction::Wait);
        assert_eq!(poll.observe(&[]), PollAction::StopNoChecks);
        // A failing check stops at once.
        let mut poll = MergePoll::default();
        assert_eq!(
            poll.observe(&[check("test", "failed")]),
            PollAction::StopFailing
        );
        // Pending resets the quiet rounds; two quiet rounds merge.
        let mut poll = MergePoll::default();
        assert_eq!(poll.observe(&[check("test", "passed")]), PollAction::Wait);
        assert_eq!(poll.observe(&[check("test", "pending")]), PollAction::Wait);
        assert_eq!(poll.observe(&[check("test", "passed")]), PollAction::Wait);
        assert_eq!(poll.observe(&[check("test", "passed")]), PollAction::Merge);
    }

    #[test]
    fn a_merge_gets_repo_so_it_never_deletes_the_local_branch() {
        let args = merge_args(3184, Some("LHagfoss/rustcode"));
        assert_eq!(
            args,
            [
                "pr",
                "merge",
                "3184",
                "--squash",
                "--delete-branch",
                "--repo",
                "LHagfoss/rustcode"
            ]
        );
        // Without a repository it still merges; only the local-cleanup guard is gone.
        assert_eq!(merge_args(1, None).len(), 5);
    }

    #[test]
    fn a_pull_request_url_names_the_repository_for_gh() {
        assert_eq!(
            repository_of("https://github.com/LHagfoss/rustcode/pull/3184").as_deref(),
            Some("LHagfoss/rustcode")
        );
        assert_eq!(repository_of("https://github.com/o/r/pull/1/files"), None);
        assert_eq!(repository_of("https://gitlab.com/o/r/pull/1"), None);
        assert_eq!(repository_of(""), None);
    }

    #[test]
    fn advisory_checks_never_hold_a_merge() {
        assert!(advisory_check("Windows and macOS portability coverage"));
        assert!(advisory_check("portability coverage (windows)"));
        assert!(!advisory_check("cargo test --workspace"));
        assert!(!advisory_check("clippy"));
    }

    #[test]
    fn a_branch_lookup_ignores_a_same_named_branch_on_a_fork() {
        let mut ours = pull();
        ours["state"] = json!("MERGED");
        ours["url"] = json!("https://github.com/LHagfoss/rustcode/pull/2701");
        ours["headRepository"] = json!({"name": "rustcode", "nameWithOwner": "LHagfoss/rustcode"});
        ours["headRepositoryOwner"] = json!({"login": "LHagfoss"});
        ours["isCrossRepository"] = json!(false);
        // An open fork pull request on a branch with the same name comes first
        // in gh's answer, but it is not this repository's branch.
        let mut fork = ours.clone();
        fork["number"] = json!(99);
        fork["state"] = json!("OPEN");
        fork["url"] = json!("https://github.com/LHagfoss/rustcode/pull/99");
        fork["headRepository"] = json!({"name": "rustcode", "nameWithOwner": "someone/rustcode"});
        fork["headRepositoryOwner"] = json!({"login": "someone"});
        fork["isCrossRepository"] = json!(true);
        let chosen = choose_for_branch(&[fork.clone(), ours]).unwrap().unwrap();
        assert_eq!(chosen.number, 2701);
        assert!(chosen.merged);
        // A fork-only match is no pull request for this repository's branch.
        assert!(choose_for_branch(&[fork]).unwrap().is_none());
    }

    #[test]
    fn a_branch_lookup_refuses_a_pull_request_it_cannot_place_in_a_repository() {
        // Without gh's repository fields a fork's same-named branch cannot be
        // told from this repository's, so it fails closed instead of guessing.
        let error = choose_for_branch(&[pull()]).unwrap_err();
        assert_eq!(error.code, "failed");
        assert!(
            error.message.contains("refusing to guess"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_delegates_pull_reports_its_state_and_failing_checks() {
        let state = delegate_pull(&pull()).unwrap();
        assert_eq!(state.number, 2701);
        assert_eq!(state.state, PullState::Open);
        assert!(!state.merged);
        assert_eq!(state.branch, "feat/x");
        assert_eq!(state.ci, CiState::Failing);
        assert_eq!(state.failing, ["lint"]);
        assert_eq!((state.passed, state.pending), (1, 2));
        assert_eq!(state.head_sha, "da4593b");
        let mut merged = pull();
        merged["state"] = json!("MERGED");
        let merged = delegate_pull(&merged).unwrap();
        assert_eq!(merged.state, PullState::Merged);
        assert!(merged.merged);
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
