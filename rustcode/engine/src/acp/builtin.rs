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
}
