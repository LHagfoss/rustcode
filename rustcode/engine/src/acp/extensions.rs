//! `_rustcode/pulls/*`, `_rustcode/threads/*` and `_rustcode/warm` extension messages, and the pull-request watcher that feeds them.

use super::*;

/// Watch a pull request from a thread (#2648). Answers `{watch}` or
/// `{error: {code, message}}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/pulls/watch/start", response = serde_json::Value)]
pub(crate) struct PullWatchStartRequest {
    pub session_id: String,
    pub cwd: std::path::PathBuf,
    pub number: u64,
    #[serde(default)]
    pub interests: Option<crate::pull_watch::Interests>,
    #[serde(default)]
    pub link: bool,
    #[serde(default)]
    pub repo: String,
}

/// Stop watching; `remove` forgets the watch too. Answers `{watch}`, with
/// a null watch when there was none.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/pulls/watch/stop", response = serde_json::Value)]
pub(crate) struct PullWatchStopRequest {
    pub session_id: String,
    pub number: u64,
    #[serde(default)]
    pub remove: bool,
}

/// The watches, a thread's when `sessionId` is given. Answers `{watches}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/pulls/watches", response = serde_json::Value)]
pub(crate) struct PullWatchesRequest {
    /// Claim a linked PR’s pending events and advance its delivered cursor.
    #[serde(default)]
    pub take: Option<u64>,
    #[serde(default)]
    pub session_id: Option<String>,
}

/// A watch changed: checked, stopped, or with new events, which are only
/// in this message once. The app tells the reader; nothing acts on them.
/// A thread this engine started because a paired device asked for one
/// (`docs/remote.md`, *Threads started from a device*). The client lists it as
/// a thread of its own: a thread has no row in the store before its first turn,
/// so listing alone would not show it.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcNotification,
)]
#[notification(method = "_rustcode/threads/created")]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadCreatedNotification {
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
    pub session_id: String,
    pub cwd: std::path::PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The harness the thread runs on (`rustcode` for the built-in one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// A thread a client trashed (`_rustcode/threads/delete`): every client that lists threads drops
/// the row, so a machine's app and the app driving it from another computer agree on what is there.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcNotification,
)]
#[notification(method = "_rustcode/threads/deleted")]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadDeletedNotification {
    pub session_id: String,
}

/// The app trashed a thread's folder (its shell moves it to the trash): this is what tells every
/// other client, since no one else can see that folder go.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/threads/delete", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadDeleteRequest {
    pub session_id: String,
}

#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcNotification,
)]
#[notification(method = "_rustcode/pulls/watch")]
pub(crate) struct PullWatchNotification {
    pub watch: crate::pull_watch::Watch,
    pub events: Vec<crate::pull_watch::WatchEvent>,
}

/// Start checking watches, once per engine, for every client that asked about
/// them. Each round tries for the checking lease, so a second engine takes
/// over when the first one exits.
pub(super) fn start_pull_watching(clients: &clients::Clients) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let Some(store) = crate::pull_watch::WatchStore::default_store() else {
        return;
    };
    let notify = clients.clone();
    tokio::spawn(async move {
        let mut lease = None;
        let mut announced = std::collections::HashMap::new();
        let mut cleaned = std::collections::HashSet::new();
        loop {
            if lease.is_none() {
                lease = store.lease();
            }
            // A link can already have pending events when it is created.
            // Forward those as well as events written by another engine.
            for (watch, events) in crate::pull_watch::unannounced(&store.load(), &mut announced) {
                notify.publish(
                    clients::Topic::PullWatches,
                    PullWatchNotification { watch, events },
                );
            }
            if lease.is_some() {
                crate::pull_watch::check_due(&store, |watch, events| {
                    // A worktree thread whose pull request merged is done.
                    crate::thread_status::on_pull_watch(watch);
                    // A change its thread has not been told about wants the
                    // user's look: the thread's status says so in every
                    // client, and its next `status` call hands the events
                    // over (#3369).
                    crate::thread_status::on_pull_events(&watch.session_id, events);
                    notify.publish(
                        clients::Topic::PullWatches,
                        PullWatchNotification {
                            watch: watch.clone(),
                            events: events.to_vec(),
                        },
                    );
                })
                .await;
                // A worktree thread whose own pull request merged no longer
                // needs its checkout: remove it, as `cleanup_worktree` would.
                crate::worktrees::clean_merged(&store.load(), &mut cleaned).await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });
}

/// The user started writing a message: wait for background MCP startup,
/// so the turn the message starts does not wait for them. Answers at once;
/// the servers start in the background, once.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/warm", response = serde_json::Value)]
pub(crate) struct WarmRequest {}

/// The project's pull requests (#2645): `state` is `open` (the default),
/// `closed`, `merged` or `all`. Answers `{list}` or `{error: {code, message}}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/pulls", response = serde_json::Value)]
pub(crate) struct PullsRequest {
    pub cwd: std::path::PathBuf,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One pull request in full. Answers `{pull}` or `{error: {code, message}}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/pulls/view", response = serde_json::Value)]
pub(crate) struct PullViewRequest {
    pub cwd: std::path::PathBuf,
    pub number: u64,
}

/// Fetch a pull request's head into a local branch a worktree can check out
/// (#3727). Answers `{branch}` or `{error}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/pulls/branch", response = serde_json::Value)]
pub(crate) struct PullBranchRequest {
    pub cwd: std::path::PathBuf,
    pub number: u64,
}

/// One delegate's pull request and its checks, by branch (the worktree's) or
/// the branch the folder is on. Answers `{branch, pull}` or `{error}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/pulls/state", response = serde_json::Value)]
pub(crate) struct PullStateRequest {
    pub cwd: std::path::PathBuf,
    #[serde(default)]
    pub branch: Option<String>,
}

/// Merge a pull request once every check passes, as `scripts/pr-merge.sh` does
/// (#3153). Answers `{merge}` or `{error}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/pulls/merge", response = serde_json::Value)]
pub(crate) struct PullMergeRequest {
    pub cwd: std::path::PathBuf,
    #[serde(default)]
    pub number: Option<u64>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// The orchestration call this merge answers: `_rustcode/pulls/merge/cancel`
    /// with the same id ends it before it merges.
    #[serde(default)]
    pub merge_id: Option<String>,
    /// The thread the result is handed to. A merge claims its link's
    /// "Merged" news, so the thread hears about it once; the answer's
    /// `announced` says the link told it first.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Stop the merge started under `merge_id` (the call that asked for it was
/// dropped). Answers `{ cancelled }`: whether one was still running.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/pulls/merge/cancel", response = serde_json::Value)]
pub(crate) struct PullMergeCancelRequest {
    pub merge_id: String,
}

/// Remove a merged delegate's worktree and branch (#3153). `merged`, `head_sha`
/// and `branch` are what the app read for its pull request; the engine still
/// reads the branch the worktree is on now and checks it is provably that pull
/// request's head before accepting a rename, and that the worktree is clean and
/// its commits are not lost. Answers `{cleanup}` or `{error}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/worktrees/cleanup", response = serde_json::Value)]
pub(crate) struct WorktreeCleanupRequest {
    pub path: std::path::PathBuf,
    /// The branch recorded for the worktree, checked against HEAD: when the
    /// worktree is on another, cleanup reads that branch's pull request itself.
    #[serde(default)]
    pub branch: Option<String>,
    /// The commit the pull request was merged at.
    #[serde(default)]
    pub head_sha: Option<String>,
    #[serde(default)]
    pub merged: bool,
}

/// A pull request's diff. Answers `{diff, truncated}` or `{error}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/pulls/diff", response = serde_json::Value)]
pub(crate) struct PullDiffRequest {
    pub cwd: std::path::PathBuf,
    pub number: u64,
}

/// Post one review the user wrote and submitted (#2646). Answers
/// `{review: {url}}` or `{error: {code, message}}`; `uncertain` means it may
/// have been posted.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/pulls/review", response = serde_json::Value)]
pub(crate) struct PullReviewRequest {
    pub cwd: std::path::PathBuf,
    pub repository: String,
    pub number: u64,
    pub commit_id: String,
    pub event: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub comments: Vec<crate::pull_requests::ReviewComment>,
}

/// The repository's newest issues and pull requests, for `#` completion in a
/// comment field (#3769). Answers `{references: [...]}` or `{error}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/pulls/references", response = serde_json::Value)]
pub(crate) struct PullReferencesRequest {
    pub cwd: std::path::PathBuf,
}

/// Reply to, resolve or unresolve one review conversation, or add a line
/// comment, from the pull request's diff view (#3768). Answers
/// `{done: {resolved?}}` or `{error: {code, message}}`; `uncertain` means
/// the change may have been made.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/pulls/thread", response = serde_json::Value)]
pub(crate) struct PullThreadRequest {
    pub cwd: std::path::PathBuf,
    #[serde(default)]
    pub repository: String,
    pub number: u64,
    pub action: crate::pull_requests::ThreadAction,
}
