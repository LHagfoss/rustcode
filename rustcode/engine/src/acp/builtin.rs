//! The contract between the engine and the built-in agent (ADR 0008).
//!
//! The ACP server holds each built-in thread as a [`BuiltinAgent`] trait
//! object and runs its prompts through it, so the server does not reach into
//! the agent's state. The agent implements it in `acp/prompt.rs` for now; once
//! its files move to their own crate, the binary registers it at startup.

use agent_client_protocol::schema::v1::{
    SessionConfigId, SessionConfigOption, SessionConfigOptionValue, StopReason,
};
use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;

use super::clients::SessionClients;
use super::session::{KnownTaskIds, ScheduledSessionTurn, SessionTurnState};

/// A boxed future the agent hands back, so the trait stays object safe.
pub(crate) type AgentFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The result of a prompt or report: why the turn stopped, or the ACP error.
pub(crate) type TurnResult = Result<StopReason, agent_client_protocol::Error>;

/// One prompt turn on a built-in thread: what the engine knows about the turn
/// and the session it runs in.
pub(crate) struct BuiltinTurn {
    pub(crate) session_id: String,
    pub(crate) cwd: PathBuf,
    pub(crate) text: String,
    pub(crate) origin: Option<serde_json::Value>,
    /// The turn's place in the session's queue; the agent waits on it first.
    pub(crate) scheduled: ScheduledSessionTurn,
    /// The session's turn queue, which wakes the thread for background work
    /// that finishes after the turn.
    pub(crate) turns: std::sync::Weak<SessionTurnState>,
    pub(crate) task_events: Arc<std::sync::Mutex<Receiver<rustcode_tasks::TaskEvent>>>,
    pub(crate) known_task_ids: Arc<std::sync::Mutex<KnownTaskIds>>,
    pub(crate) terminal_backlog: Arc<std::sync::Mutex<VecDeque<rustcode_tasks::TaskEvent>>>,
    pub(crate) terminal_overflow: Arc<AtomicBool>,
    pub(crate) clients: SessionClients,
    pub(crate) client: Arc<reqwest::Client>,
    pub(crate) auto_approve: bool,
    pub(crate) remote: Option<super::remote::RemoteManager>,
}

/// One built-in thread, as the ACP server sees it.
pub(crate) trait BuiltinAgent: Send + Sync {
    /// Why the thread cannot take images with its current model, when it
    /// cannot.
    fn image_support_error(&self) -> AgentFuture<'_, Option<String>>;

    /// Answer a report command (`commands::is_report`) from the thread's
    /// state, beside a running turn.
    fn report<'a>(
        &'a self,
        session_id: &'a str,
        text: &'a str,
        clients: &'a SessionClients,
    ) -> AgentFuture<'a, TurnResult>;

    /// Run one prompt turn to its end.
    fn prompt(&self, turn: BuiltinTurn) -> AgentFuture<'static, TurnResult>;

    /// End the thread's turn for steering: the steers it did not read, which
    /// go back to the client to send next.
    fn end_steers<'a>(&'a self, session_id: &'a str) -> AgentFuture<'a, Vec<String>>;

    /// Offer a steer to the running turn. The answer says whether the turn
    /// took it (`{"accepted": bool}`); `origin` is the device that sent it.
    fn steer<'a>(
        &'a self,
        text: &'a str,
        origin: Option<serde_json::Value>,
    ) -> AgentFuture<'a, serde_json::Value>;

    /// Record a background task's result in the transcript of `session_id`,
    /// so the next request carries it.
    fn record_background_result<'a>(
        &'a self,
        session_id: &'a str,
        task_id: &'a str,
        call_id: &'a str,
        output: crate::tools::ToolExecutionOutput,
    ) -> AgentFuture<'a, ()>;

    /// When the thread last had a message, read without waiting: `None`
    /// while a turn holds it or before its first message.
    fn last_active(&self) -> Option<String>;

    /// Cut the conversation back to before user message `user_index`, whose
    /// text must be `text`. `first_only` refuses unless that message is still
    /// the whole transcript.
    fn rewind<'a>(
        &'a self,
        user_index: usize,
        text: &'a str,
        first_only: bool,
    ) -> AgentFuture<'a, Result<serde_json::Value, agent_client_protocol::Error>>;

    /// Set one of the thread's config options (model, effort, mode, ...) and
    /// return them all. A model or effort becomes the built-in harness's
    /// choice for the next new thread.
    fn set_config<'a>(
        &'a self,
        config_id: &'a SessionConfigId,
        value: &'a SessionConfigOptionValue,
    ) -> AgentFuture<'a, Result<Vec<SessionConfigOption>, agent_client_protocol::Error>>;

    /// The thread's plan usage limits, for the usage meter.
    fn usage_limits(&self) -> AgentFuture<'_, serde_json::Value>;

    /// Answer `_rustcode/agents`: the thread's subagents and their history.
    fn agents<'a>(
        &'a self,
        cwd: &'a std::path::Path,
        client: &'a reqwest::Client,
        action: super::agents::AgentsAction,
    ) -> AgentFuture<'a, Result<serde_json::Value, agent_client_protocol::Error>>;

    /// Answer `_rustcode/mcp/app`: an MCP app's call to its server, which may
    /// wait for the user's approval. `builtin` is false when the thread runs
    /// on an external harness that keeps its own MCP servers.
    fn mcp_app(
        &self,
        request: super::mcp_app::McpAppRequest,
        builtin: bool,
        clients: SessionClients,
        auto_approve: bool,
    ) -> AgentFuture<'static, Result<serde_json::Value, agent_client_protocol::Error>>;

    /// Replace the timed command approvals the thread's copy of the user
    /// config holds, after one was revoked.
    fn set_timed_command_approvals(
        &self,
        approvals: Vec<crate::config::TimedCommandApproval>,
    ) -> AgentFuture<'_, ()>;

    /// Take the next queued prompt of `session_id` to run, when it is the
    /// thread's open session and no turn is running.
    fn take_queued<'a>(&'a self, session_id: &'a str) -> AgentFuture<'a, Option<String>>;

    /// Move the steers the last turn of `session_id` did not read onto the
    /// prompt queue.
    fn promote_steers<'a>(&'a self, session_id: &'a str) -> AgentFuture<'a, ()>;

    /// Whether a queued prompt waits and no turn is running.
    fn has_queued_work(&self) -> AgentFuture<'_, bool>;

    /// Start sharing `session_id` with the remote gateway under
    /// `registration`: the publisher and the message that opens the link.
    /// `None` when the thread no longer shows that session.
    fn share<'a>(
        &'a self,
        session_id: &'a str,
        registration: crate::remote::SessionRegistration,
    ) -> AgentFuture<
        'a,
        Option<(
            Box<dyn BuiltinPublisher>,
            crate::remote::owner::OwnerMessage,
        )>,
    >;

    /// Apply a device's mutation (prompt, cancel, answer, settings) to the
    /// shared thread.
    fn remote_mutation<'a>(
        &'a self,
        cancel_token: &'a mut tokio_util::sync::CancellationToken,
        registration: &'a crate::remote::SessionRegistration,
        request: &'a crate::remote::RemoteRequest,
    ) -> AgentFuture<'a, Result<crate::remote::SessionMutation, crate::remote::RemoteError>>;

    /// Answer a device's read of the shared thread.
    fn remote_read<'a>(
        &'a self,
        registration: &'a crate::remote::SessionRegistration,
        request: &'a crate::remote::RemoteRequest,
    ) -> AgentFuture<'a, Result<crate::remote::RemoteResult, crate::remote::RemoteError>>;

    /// The workspace a device's `get_changes` reads, checked like any read.
    fn remote_changes_workspace<'a>(
        &'a self,
        registration: &'a crate::remote::SessionRegistration,
        request: &'a crate::remote::RemoteRequest,
    ) -> AgentFuture<'a, Result<PathBuf, crate::remote::RemoteError>>;

    /// The thread's model, read without waiting: `None` while a turn holds
    /// the thread.
    fn selected_model(&self) -> Option<String>;

    /// The thread's state, for the agent tests that still live in the
    /// engine until the agent gets its own crate.
    #[cfg(test)]
    fn test_state(&self) -> Arc<tokio::sync::Mutex<crate::app::AppState>>;

    /// The thread's transcript.
    fn history(&self) -> AgentFuture<'_, rustcode_core::History>;

    /// What a client joining the open thread gets besides its transcript.
    fn attach_snapshot(&self) -> AgentFuture<'_, AttachSnapshot>;
}

/// What a client joining an open built-in thread gets besides its transcript.
pub(crate) struct AttachSnapshot {
    /// The thread's last compaction, shown where it happened in the replay.
    pub(crate) compaction: Option<crate::app::CompactionStats>,
    pub(crate) config_options: Vec<SessionConfigOption>,
    pub(crate) usage: agent_client_protocol::schema::v1::SessionUpdate,
}

/// Publishes a shared built-in thread to the remote gateway.
pub(crate) trait BuiltinPublisher: Send {
    /// Note an agent event the thread has applied.
    fn observe(&mut self, event: &crate::controller::AgentUiEvent);

    /// Send a full snapshot on the next publish.
    fn request_snapshot(&mut self);

    /// Drop events until the next resync snapshot: the link fell behind.
    fn mark_lagging(&mut self);

    /// Sequence of the last event published.
    fn sequence(&self) -> u64;

    /// What changed since the last publish. `None` when the thread no longer
    /// shows the shared session.
    fn publish(&mut self, room_for_resync: bool) -> AgentFuture<'_, Option<Published>>;
}

/// One publish pass of a shared built-in thread.
pub(crate) struct Published {
    pub(crate) messages: Vec<crate::remote::owner::OwnerMessage>,
    /// A prompt is queued and no turn runs, so the turn loop should start.
    pub(crate) queued_while_idle: bool,
}

/// A scheduled job's prompt, which the agent runs headless in a daemon
/// session of its own.
pub(crate) struct ScheduledPrompt {
    pub(crate) prompt: String,
    pub(crate) workspace: PathBuf,
    pub(crate) model_profile: Option<String>,
    /// The thread the job was recorded from: an input, never written to.
    pub(crate) session: String,
    pub(crate) settings: Option<crate::sessions::SessionSettingsSnapshot>,
}

/// Why a scheduled prompt did not finish, by whether running it again can
/// help.
pub(crate) enum ScheduledPromptError {
    Permanent(String),
    Transient(String),
    /// The turn may have had effects before it failed.
    Ambiguous(String),
}

/// What the engine runs on the built-in agent outside an ACP thread.
pub(crate) trait BuiltinRuntime: Send + Sync {
    /// Run a scheduled job's prompt to its end and return the reply.
    fn run_scheduled(
        &self,
        prompt: ScheduledPrompt,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> AgentFuture<'static, Result<String, ScheduledPromptError>>;
}

/// The built-in agent's runtime. The agent lives in this crate until ADR
/// 0008's last step, so this is its headless runner; after that the binary
/// registers it at startup.
pub(crate) fn runtime() -> &'static dyn BuiltinRuntime {
    &crate::raw_cli::HeadlessRuntime
}
