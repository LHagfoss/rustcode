use std::sync::Arc;

#[path = "state/models.rs"]
mod models;
use models::MAX_LIVE_TOOL_OUTPUT_BYTES;
pub use models::*;

pub(crate) struct StallRecovery {
    pub reset_orchestrator: bool,
    /// The queue still holds prompts: recovery must preserve them (the spawn
    /// loop restarts them) instead of telling the user to reissue.
    pub queue_preserved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolConfirmationResponse {
    Approve,
    ApproveAndRemember(String),
    ForbidAndRemember(String),
    Deny,
}

/// A single-flight claim for the queue orchestrator.  The session and
/// generation are both part of the claim so a task that is unwinding after a
/// cancellation or session switch cannot release a newer orchestrator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrchestratorLease {
    pub(crate) session_id: String,
    pub(crate) generation: u64,
}

/// Stall watchdog timeout (issue #1226). The provider stream idle timeout
/// (120s) already bounds supplier silence, so a longer active-state silence
/// means our own machinery died, not the provider.
pub(crate) const STALL_WATCHDOG_TIMEOUT_SECS: u64 = 5 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingSteer {
    pub(crate) session_id: String,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DraftSubmitMode {
    #[default]
    Steer,
    Queue,
}

pub struct AppState {
    pub input_buffer: String,
    /// Deadline through which a second Ctrl+C confirms application exit.
    pub ctrl_c_exit_deadline: Option<std::time::Instant>,
    pub history: History,
    /// First history index shown in the TUI. The full history remains available
    /// to the model; `/clear` advances this boundary without deleting messages.
    pub history_display_start: usize,
    pub current_response: Arc<String>,
    /// Monotonic content revision used by snapshot delta consumers.
    pub(crate) current_response_revision: u64,
    /// Revision of the most recent clear or replacement, if any.
    pub(crate) current_response_last_rewrite_revision: u64,
    pub current_token_usage: Option<TokenUsage>,
    pub current_thought_time_ms: u64,
    pub current_thought_tokens: u32,
    pub current_thought_started_at: Option<std::time::Instant>,
    pub model_quota_remaining: Option<f32>,
    pub pending_queue: Vec<String>,
    /// User instructions submitted during an explicitly steerable active turn.
    /// These remain separate from ordinary follow-up prompts until applied.
    pub(crate) pending_steers: Vec<PendingSteer>,
    /// Number of promoted steering prompts at the head of `pending_queue`.
    /// This prefix survives FIFO edits so its segment wakeup can retain context.
    pub(crate) promoted_steer_prefix_count: usize,
    /// Session ID of the regular interactive turn currently allowed to accept
    /// steering input. Other kinds of active work leave this unset.
    pub(crate) active_turn_steerable_session: Option<String>,
    pub(crate) draft_submit_mode: DraftSubmitMode,
    /// Background task IDs whose terminal completion has already been queued.
    /// This makes completion notifications idempotent across callback races.
    pub background_wakeup_ids: std::collections::BTreeSet<String>,
    /// Completed background outputs withheld while a turn is in flight. They
    /// join history at the next turn boundary instead of derailing the
    /// current turn's context mid-stream.
    pub pending_background_outputs: Vec<crate::PendingBackgroundOutput>,
    /// The logical turn state waiting for a background task completion. This
    /// is intentionally kept outside serialized history so an orchestrator
    /// restart can resume the same in-memory task without creating a second
    /// planning or verification ledger.
    pub(crate) background_turn_context: Option<Box<crate::network::TurnContext>>,
    pub status: AppStatus,
    /// Single-flight guard for the agent loop. `status` transiently reads Idle
    /// in windows where an orchestrator is still alive, so gating spawns on it
    /// let a background wakeup (or the event-loop drainer) start a *second*
    /// concurrent orchestrator — two turns streaming the same history produced
    /// duplicate assistant messages. Spawns gate on this instead.
    pub orchestrator_running: bool,
    pub(crate) orchestrator_generation: u64,
    pub(crate) orchestrator_owner: Option<OrchestratorLease>,
    /// Time at which the app most recently entered an eligible idle state.
    /// Unlike user activity, this cannot be stale from before a turn ran.
    pub(crate) idle_since: std::time::Instant,
    /// Whether the latest logical turn completed with a model-facing final
    /// response. Tool-only, interrupted, and failed turns must not trigger an
    /// automatic presentation recap.
    pub(crate) last_turn_had_model_final_response: bool,
    /// Prevents manual and automatic summaries from running concurrently.
    pub(crate) summary_in_flight: bool,
    /// Count of conversational messages at which the last summary completed.
    /// A changed conversation is required before the idle timer can summarize again.
    pub(crate) last_summary_history_len: Option<usize>,
    pub cursor_position: usize,

    pub suggestion_cycle: crate::app::suggestion::SuggestionCycle,
    pub response_time: Option<std::time::Duration>,
    pub history_index: Option<usize>,
    pub temp_input: String,
    /// Every input the user submitted this run, oldest first — both plain text
    /// and slash commands. Arrow-up/down recalls from this, not from chat
    /// history (commands never become chat messages, and generated user blobs
    /// like /goal's "Goal: ..." text aren't typed input).
    pub input_history: Vec<String>,

    pub api_base_url: String,
    pub model_name: String,
    /// Probed function-calling support, keyed by endpoint URL. Populated once
    /// per endpoint per run; a negative result is also written back to the
    /// profile so later runs skip the probe.
    pub function_calling_support: std::collections::HashMap<String, bool>,
    /// Successful image analyses keyed by the image bytes' stable hash.
    pub image_analysis_cache: std::collections::HashMap<String, String>,
    pub config: crate::config::AppConfig,
    #[allow(dead_code)]
    pub cwd_and_branch: String,
    /// Cached workspace path and Git branch used by the composer footer.
    pub(crate) workspace_location: crate::app::workspace::WorkspaceLocationCache,
    /// Workspace root supplied by an external frontend such as ACP.
    pub workspace_root: Option<std::path::PathBuf>,
    /// Task/project directory supplied by an external frontend. This is the
    /// default navigation scope; `workspace_root` remains the hard boundary.
    pub task_working_directory: Option<std::path::PathBuf>,

    pub update_check: crate::update::UpdateState,
    pub show_update_prompt: bool,
    pub update_prompt_index: usize,
    pub dismissed_update_version: Option<crate::update::Version>,
    pub update_requested: bool,

    pub active_suggestion_index: Option<usize>,
    /// Completion token explicitly dismissed with Esc. It remains suppressed
    /// until the token under the cursor changes, matching Codex popup behavior.
    pub dismissed_completion: Option<String>,

    pub show_model_picker: bool,
    pub model_picker_index: usize,
    pub modal_picker_index: usize,
    pub model_picker_search: String,

    pub show_theme_picker: bool,
    pub theme_picker_index: usize,
    pub theme_picker_initial: String,

    pub show_command_picker: bool,
    pub command_picker_index: usize,
    pub command_picker_search: String,

    pub show_history_picker: bool,
    pub history_picker_index: usize,
    pub history_picker_sessions: Vec<crate::config::SessionMeta>,
    pub history_picker_truncated: bool,
    pub pending_delete_session_idx: Option<usize>,
    pub show_subagent_picker: bool,
    pub subagent_picker_index: usize,
    pub show_context_modal: bool,
    pub active_session_id: String,
    /// Whether the current logical turn may set the title of a new session.
    pub session_title_tool_available: bool,

    pub show_mcp_config: bool,
    pub mcp_picker_index: usize,
    pub mcp_edit_state: Option<McpEditState>,

    pub last_copy_text: Option<(String, std::time::Instant)>,
    pub generation_start_time: Option<std::time::Instant>,
    pub pending_tool_confirmation: Option<Vec<ToolConfirmation>>,
    /// Complete serialized arguments for the pending confirmation actions.
    /// Kept separately from the bounded terminal/UI preview on each action.
    pub pending_approval_details: Option<Vec<String>>,
    /// Controller-issued identity for the currently pending approval batch.
    pub pending_approval_batch_id: Option<String>,
    pub modal_scroll_row: u16,
    /// Selected approval row: 0 = approve, 1 = deny. UI-only state.
    pub tool_confirmation_selected: usize,

    pub tool_confirmation_response: Option<tokio::sync::oneshot::Sender<ToolConfirmationResponse>>,

    /// Active interactive `ask_question` prompt and the channel that delivers the
    /// user's selection back to the awaiting tool call.
    pub pending_question: Option<PendingQuestion>,
    pub question_response: Option<tokio::sync::oneshot::Sender<String>>,
    /// Remaining questions in a chained `ask_question` call (everything after
    /// the active `pending_question`), plus already-answered ones in order.
    /// Empty for the legacy single-question shape.
    pub pending_question_queue: Vec<PendingQuestion>,
    pub pending_question_done: Vec<PendingQuestion>,

    /// The names of user-approved tools currently running in the background.
    /// While this is not empty, the modal overlay stays closed and the user can
    /// keep working normally.
    pub running_tools: Vec<String>,
    /// Tool calls currently being executed, including read/search calls that
    /// finish too quickly to be useful as a name-only `running_tools` status.
    /// This is deliberately not serialized: it is a terminal presentation
    /// projection, not conversation context.
    pub live_tool_calls: Arc<Vec<LiveToolCall>>,
    /// Monotonic identity source for presentation-only live tool projections.
    pub live_tool_call_sequence: u64,

    pub stream_tracker: Option<StreamTracker>,

    pub auto_confirm: bool,

    pub(crate) subagent_supervisor: crate::app::SubagentSupervisor,
    pub subagents: Vec<SubAgent>,
    /// Selected conversation context for the subagent picker. `None` keeps
    /// the root conversation active without changing its stored history.
    pub selected_subagent_id: Option<u32>,
    pub delegation_armed: bool,
    pub delegation_active: bool,
    pub continuous_mode: bool,
    pub next_subagent_id: u32,

    /// Persistent task plan, written via the `todo_write` agent tool.
    pub todos: Vec<TodoItem>,

    /// File paths the agent has read this session, mapped to the file's mtime at
    /// read time. Surfaced back to the model so it doesn't re-read unchanged files,
    /// and used by the repeat guard to ALLOW re-reads when a file changed on disk.
    pub read_file_mtimes: std::collections::HashMap<String, std::time::SystemTime>,

    /// Signatures of recent read-only tool calls, used by the repeat-loop guard
    /// to short-circuit identical re-reads (e.g. viewing the same file twice).
    pub recent_read_calls: std::collections::VecDeque<String>,

    /// Structured facts from recent reads, keyed by the same signature. Content
    /// is retained separately only for small reads, while failure, truncation,
    /// and bounded-output recovery metadata remain available for every entry.
    pub recent_read_outputs: std::collections::HashMap<String, CachedReadOutput>,

    pub scroll_row: u16,
    pub is_scroll_locked_to_bottom: bool,
    pub last_max_scroll: u16,
    /// Wrapped transcript height from the latest conversation render. The UI
    /// uses this to keep the composer close to short conversations.
    pub conversation_content_height: u16,
    pub viewport_height: u16,
    #[allow(dead_code)]
    pub mouse_capture_enabled: bool,
    pub agent_mode: crate::config::AgentMode,
    pub chat_area: Option<ratatui::layout::Rect>,
    /// Screen rect of the bottom input box. Kept so shutdown can erase the
    /// transient composer without disturbing the transcript above it.
    pub input_text_area: Option<ratatui::layout::Rect>,
    pub scroll_to_bottom_btn: Option<ratatui::layout::Rect>,
    /// Clickable element the pointer is over, refreshed on every mouse move.
    #[allow(dead_code)]
    pub hover: HoverTarget,
    #[allow(dead_code)]
    pub selected_text: Option<String>,
    pub sel_start: Option<(u16, u16)>,
    pub sel_end: Option<(u16, u16)>,
    pub selecting: bool,
    /// True when the active selection lives in the input box rather than the
    /// chat. Input has no scroll offset, so highlight/extract use scroll_row 0.
    pub sel_in_input: bool,
    /// Screen rows carrying a code-block `[Copy]` badge, mapped to the block's
    /// text, for click-to-copy hit-testing.
    #[cfg(test)]
    pub code_copy_rows: Vec<(u16, String)>,

    /// Timestamp of the last escape key press (for double-esc detection)
    #[allow(dead_code)]
    pub last_escape_time: Option<std::time::Instant>,

    pub raw_cli_mode: bool,
    pub tip_index: usize,

    pub current_terminal_title: Option<String>,
    pub current_terminal_progress: Option<crate::app::activity::TerminalProgress>,
    /// Cached custom session title: the session id it was read for, and the
    /// title found on disk (`None` when that session has no `title.txt`).
    /// Keeps the draw loop from hitting the filesystem on every frame.
    pub session_title_cache: Option<(String, Option<String>)>,
    /// Changes whenever an in-flight title load must be discarded.
    pub session_title_cache_generation: u64,
    /// Set by background tasks that mutate state outside the input path, so the
    /// draw loop knows to render once even though no key was pressed.
    pub redraw_requested: bool,
    /// Monotonic version of render-visible state. A render may publish its
    /// layout metrics only while this remains unchanged.
    pub(crate) render_revision: u64,
    /// Requests the terminal loop to clear the entire screen and reset the inline viewport.
    pub clear_screen_requested: bool,

    /// Snapshot of environment context from the first turn, used for delta diffing.
    pub context_snapshot: Option<crate::context::ContextSnapshot>,
    /// Cached static system prompt + tool schema, rebuilt only when the tool
    /// protocol, agent mode, or MCP tool set changes.
    pub prompt_cache: PromptCache,
    pub verbosity: Verbosity,
    pub expanded_thoughts: std::collections::HashSet<usize>,
    /// Warning or informational notices collected from background operations (e.g. MCP startup timeouts)
    /// to be displayed cleanly upon application exit instead of interrupting active terminal rendering.
    pub exit_warnings: Vec<String>,
}

impl AppState {
    #[allow(dead_code)]
    pub(crate) fn active_history_display_start(&self) -> usize {
        if self.selected_subagent_id.is_some() {
            0
        } else {
            self.history_display_start
        }
    }

    /// Return the cached custom title for the active session without touching
    /// the filesystem. `Some(None)` means the cache contains a miss.
    pub(crate) fn cached_session_title(&self) -> Option<Option<String>> {
        self.session_title_cache
            .as_ref()
            .filter(|(cached_id, _)| *cached_id == self.active_session_id)
            .map(|(_, title)| title.clone())
    }

    /// Install a title loaded for `session_id` only if that session is still
    /// active. Returns whether the cache was installed.
    pub(crate) fn install_session_title_cache(
        &mut self,
        session_id: &str,
        generation: u64,
        title: Option<String>,
    ) -> bool {
        if self.active_session_id != session_id || self.session_title_cache_generation != generation
        {
            return false;
        }
        self.session_title_cache = Some((session_id.to_owned(), title));
        true
    }

    /// Forget the cached session title so it is re-read from disk on the next
    /// draw. Use after `save_session_title`.
    pub fn invalidate_session_title_cache(&mut self) {
        self.session_title_cache = None;
        self.session_title_cache_generation = self.session_title_cache_generation.wrapping_add(1);
    }

    /// Ask the draw loop for one more frame. Background tasks that mutate state
    /// while the app is otherwise idle should call this, since the loop no
    /// longer redraws on a fixed timer.
    pub fn request_redraw(&mut self) {
        self.redraw_requested = true;
        self.render_revision = self.render_revision.wrapping_add(1);
    }

    pub(crate) fn mark_user_activity(&mut self) {
        let now = std::time::Instant::now();
        if self.status == AppStatus::Idle {
            self.idle_since = now;
        }
    }

    /// Claim the queue orchestrator for the current session.  The returned
    /// lease must be supplied when the task finishes; this makes release
    /// conditional on the task that originally claimed the slot.
    pub(crate) fn claim_orchestrator(&mut self) -> Option<OrchestratorLease> {
        if self.orchestrator_running {
            return None;
        }
        self.orchestrator_generation = self.orchestrator_generation.wrapping_add(1);
        let lease = OrchestratorLease {
            session_id: self.active_session_id.clone(),
            generation: self.orchestrator_generation,
        };
        self.orchestrator_owner = Some(lease.clone());
        self.orchestrator_running = true;
        Some(lease)
    }

    /// Release only the exact ownership claim that was acquired by a task.
    /// Returns false when a newer session or generation owns the slot.
    pub(crate) fn release_orchestrator(&mut self, lease: &OrchestratorLease) -> bool {
        if self.orchestrator_owner.as_ref() != Some(lease)
            || self.active_session_id != lease.session_id
        {
            return false;
        }
        self.orchestrator_owner = None;
        self.orchestrator_running = false;
        true
    }

    /// Invalidate the current owner before cancelling or switching sessions.
    /// A stale task may still unwind, but its release can no longer affect a
    /// later claim.
    pub(crate) fn invalidate_orchestrator(&mut self) {
        self.orchestrator_generation = self.orchestrator_generation.wrapping_add(1);
        self.orchestrator_owner = None;
        self.orchestrator_running = false;
    }

    pub(crate) fn enter_idle(&mut self) {
        self.status = AppStatus::Idle;
        self.idle_since = std::time::Instant::now();
    }

    /// Start a chained question flow: the first question becomes active, the
    /// rest wait in the queue. Single-question callers pass an empty queue,
    /// which behaves exactly like the legacy flow.
    pub(crate) fn begin_question_chain(&mut self, mut questions: Vec<PendingQuestion>) {
        self.pending_question_done.clear();
        self.pending_question_queue.clear();
        self.pending_question = if questions.is_empty() {
            None
        } else {
            Some(questions.remove(0))
        };
        self.pending_question_queue = questions;
    }

    /// Drop the whole chain (cancel / session reset paths).
    pub(crate) fn clear_question_chain(&mut self) {
        self.pending_question = None;
        self.pending_question_queue.clear();
        self.pending_question_done.clear();
    }

    /// Total questions in the active chain (answered + active + queued).
    pub(crate) fn question_chain_len(&self) -> usize {
        self.pending_question_done.len()
            + usize::from(self.pending_question.is_some())
            + self.pending_question_queue.len()
    }

    /// 1-based position of the active question within its chain.
    pub(crate) fn question_chain_position(&self) -> usize {
        self.pending_question_done.len() + usize::from(self.pending_question.is_some())
    }

    /// Chain questions with a recorded answer (accepted via Enter).
    pub(crate) fn question_chain_answered(&self) -> usize {
        self.pending_question_done
            .iter()
            .filter(|question| question.is_answered())
            .count()
            + usize::from(
                self.pending_question
                    .as_ref()
                    .is_some_and(PendingQuestion::is_answered),
            )
    }

    /// Record `answer` for the active question and advance to the next queued
    /// one. Returns true when another question became active.
    pub(crate) fn advance_question_chain(&mut self, answer: String) -> bool {
        if let Some(mut current) = self.pending_question.take() {
            current.answer = Some(answer);
            current.custom_input = None;
            current.custom_cursor = 0;
            self.pending_question_done.push(current);
        }
        if self.pending_question_queue.is_empty() {
            false
        } else {
            self.pending_question = Some(self.pending_question_queue.remove(0));
            true
        }
    }

    /// Move focus between chain questions without answering (Tab /
    /// Shift+Tab). Highlight, ticks, and recorded answers travel with each
    /// question, so coming back restores exactly what the user left.
    pub(crate) fn focus_question(&mut self, delta: isize) {
        let Some(active) = self.pending_question.take() else {
            return;
        };
        let mut ordered = std::mem::take(&mut self.pending_question_done);
        let active_pos = ordered.len();
        ordered.push(active);
        ordered.extend(std::mem::take(&mut self.pending_question_queue));
        if ordered.len() < 2 {
            // Single question: restore untouched.
            let mut iter = ordered.into_iter();
            self.pending_question = iter.next();
            self.pending_question_queue = iter.collect();
            return;
        }
        let next = (active_pos as isize + delta).rem_euclid(ordered.len() as isize) as usize;
        let mut iter = ordered.into_iter();
        self.pending_question_done = iter.by_ref().take(next).collect();
        self.pending_question = iter.next();
        self.pending_question_queue = iter.collect();
    }

    /// Drain the whole chain for submission, recording `current_answer` for
    /// the active question first. Returns `(header, question, answer)` triples
    /// in original order; unanswered questions yield an empty answer.
    pub(crate) fn take_question_chain_answers(
        &mut self,
        current_answer: Option<String>,
    ) -> Vec<(String, String, String)> {
        if let (Some(answer), Some(active)) = (current_answer, self.pending_question.as_mut()) {
            active.answer = Some(answer);
        }
        let mut ordered = std::mem::take(&mut self.pending_question_done);
        if let Some(active) = self.pending_question.take() {
            ordered.push(active);
        }
        ordered.extend(std::mem::take(&mut self.pending_question_queue));
        ordered
            .into_iter()
            .map(|question| {
                (
                    question.header.clone(),
                    question.question.clone(),
                    question.answer.clone().unwrap_or_default(),
                )
            })
            .collect()
    }

    /// Detect a dead turn: status says work is in flight but nothing can make
    /// progress — no background tasks, no running tools, and the stream (if
    /// any) silent past the watchdog timeout. Also catches a stuck
    /// orchestrator flag wedging a non-empty queue (the loop only spawns
    /// while the flag is clear), and a queue orphaned with no owner at all
    /// (invalidated while prompts remained, or a spawn that never happened):
    /// the prompts are preserved and the spawn loop restarts them. Returns
    /// recovery instructions; the caller logs the event and resets state.
    pub(crate) fn check_stall_watchdog(
        &self,
        background_active: bool,
        now: std::time::Instant,
    ) -> Option<StallRecovery> {
        if !matches!(self.status, AppStatus::Streaming | AppStatus::Queued) {
            return None;
        }
        if background_active || !self.running_tools.is_empty() {
            return None;
        }
        let timeout = std::time::Duration::from_secs(STALL_WATCHDOG_TIMEOUT_SECS);
        let generation_stale = self
            .generation_start_time
            .is_some_and(|started| now.saturating_duration_since(started) >= timeout);
        let stream_stale = match &self.stream_tracker {
            None => true,
            Some(tracker) => now.saturating_duration_since(tracker.last_update) >= timeout,
        };
        if !self.pending_queue.is_empty() {
            if self.orchestrator_running && generation_stale {
                return Some(StallRecovery {
                    reset_orchestrator: true,
                    queue_preserved: true,
                });
            }
            if !self.orchestrator_running && generation_stale {
                // Orphaned queue: no loop owns the spawn slot, so nothing will
                // ever drain these prompts. Reset the (already clear) flag and
                // let the spawn loop restart them; never silently drop them.
                // generation_stale (not merely stream silence) gates this so a
                // freshly submitted prompt can never trip it.
                return Some(StallRecovery {
                    reset_orchestrator: false,
                    queue_preserved: true,
                });
            }
            return None;
        }
        if generation_stale && stream_stale {
            return Some(StallRecovery {
                reset_orchestrator: self.orchestrator_running,
                queue_preserved: false,
            });
        }
        None
    }

    pub(crate) fn should_start_idle_summary(
        &self,
        now: std::time::Instant,
        background_tasks_active: bool,
        idle_after: std::time::Duration,
    ) -> bool {
        self.status == AppStatus::Idle
            && !self.orchestrator_running
            && self.pending_queue.is_empty()
            && !self.summary_in_flight
            && !background_tasks_active
            && !self.modal_open()
            && self.input_buffer.trim().is_empty()
            && self.last_turn_had_model_final_response
            && self.summary_history_len() >= 2
            && self.last_summary_history_len != Some(self.summary_history_len())
            && now.duration_since(self.idle_since) >= idle_after
    }

    fn summary_history_len(&self) -> usize {
        self.history
            .iter()
            .filter(|message| message.role != "system" && !message.conversation_recap)
            .count()
    }

    pub(crate) fn claim_summary(&mut self) -> bool {
        if self.summary_in_flight {
            return false;
        }
        self.summary_in_flight = true;
        true
    }

    pub(crate) fn finish_summary(&mut self) {
        self.summary_in_flight = false;
        self.last_summary_history_len = Some(self.summary_history_len());
        self.enter_idle();
    }

    /// Clear the pending Ctrl+C exit confirmation, invalidating the footer
    /// snapshot only when the confirmation was actually armed.
    pub fn clear_ctrl_c_exit_arming(&mut self) {
        if self.ctrl_c_exit_deadline.take().is_some() {
            self.request_redraw();
        }
    }

    /// Expire a pending Ctrl+C exit confirmation and request the redraw that
    /// removes its footer warning. Returns whether an armed deadline expired.
    pub fn expire_ctrl_c_exit_arming(&mut self, now: std::time::Instant) -> bool {
        if self
            .ctrl_c_exit_deadline
            .is_some_and(|deadline| deadline <= now)
        {
            self.ctrl_c_exit_deadline = None;
            self.request_redraw();
            true
        } else {
            false
        }
    }

    pub(crate) fn ctrl_c_exit_armed(&self) -> bool {
        self.ctrl_c_exit_deadline
            .is_some_and(|deadline| deadline > std::time::Instant::now())
    }

    /// Clear the render-visible response buffer and invalidate in-flight layout metrics.
    pub(crate) fn clear_current_response(&mut self) {
        let changed = !self.current_response.is_empty();
        Arc::make_mut(&mut self.current_response).clear();
        if changed {
            self.current_response_revision = self.current_response_revision.wrapping_add(1);
            self.current_response_last_rewrite_revision = self.current_response_revision;
        }
        self.request_redraw();
    }

    /// Append streamed render-visible response text and invalidate in-flight layout metrics.
    pub(crate) fn append_current_response(&mut self, chunk: &str) {
        Arc::make_mut(&mut self.current_response).push_str(chunk);
        if !chunk.is_empty() {
            self.current_response_revision = self.current_response_revision.wrapping_add(1);
        }
        self.request_redraw();
    }

    /// Replace the render-visible response buffer and invalidate in-flight layout metrics.
    pub(crate) fn replace_current_response(&mut self, response: impl Into<String>) {
        let response = response.into();
        if self.current_response.as_str() != response {
            self.current_response = Arc::new(response);
            self.current_response_revision = self.current_response_revision.wrapping_add(1);
            self.current_response_last_rewrite_revision = self.current_response_revision;
        }
        self.request_redraw();
    }

    pub(crate) fn render_snapshot(&self) -> crate::ui::render_snapshot::RenderSnapshot {
        crate::ui::render_snapshot::RenderSnapshot::new(self)
    }

    /// Refresh the cached footer location when its debounce window expires.
    /// Git discovery happens here, before rendering, so a frame only reads
    /// the already-resolved display string.
    pub(crate) fn refresh_workspace_location(&mut self, now: std::time::Instant) -> bool {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        if self.workspace_location.refresh_if_due(&cwd, now) {
            self.cwd_and_branch = self.workspace_location.display();
            self.request_redraw();
            true
        } else {
            false
        }
    }

    /// Publish layout information only if the state rendered to obtain it is
    /// still current. Returns false when a newer redraw invalidated it.
    pub(crate) fn publish_render_metrics(
        &mut self,
        revision: u64,
        height: u16,
        input_area: ratatui::layout::Rect,
    ) -> bool {
        if revision != self.render_revision {
            return false;
        }
        self.conversation_content_height = height;
        self.input_text_area = Some(input_area);
        true
    }

    /// Request that the draw loop clear the physical terminal screen and reset the inline viewport.
    pub fn request_clear_screen(&mut self) {
        self.clear_screen_requested = true;
        self.request_redraw();
    }

    /// Add a live tool projection for the TUI without changing canonical
    /// conversation history or provider-facing state.
    pub fn begin_live_tool_call(
        &mut self,
        provider_call_id: Option<&str>,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> String {
        let (action, target) = crate::app::activity::summarize_tool_call(tool_name, arguments);
        if let Some(call) = Arc::make_mut(&mut self.live_tool_calls)
            .iter_mut()
            .find(|call| {
                !call.execution_started
                    && match provider_call_id {
                        Some(call_id) => call.provider_call_id.as_deref() == Some(call_id),
                        // Text protocols do not provide a stable call id. Keep
                        // one speculative projection for the whole response;
                        // the final tool name/arguments replace the preview as
                        // the stream becomes authoritative.
                        None => call.provider_call_id.is_none(),
                    }
            })
        {
            call.tool_name = tool_name.to_owned();
            call.action = action;
            call.target = target;
            call.execution_started = true;
            call.started_at = std::time::Instant::now();
            let key = call.key.clone();
            self.request_redraw();
            return key;
        }

        let sequence = self.live_tool_call_sequence;
        self.live_tool_call_sequence = self.live_tool_call_sequence.saturating_add(1);
        let key = match provider_call_id {
            Some(call_id) => format!("provider:{call_id}:{sequence}"),
            None => format!("local:{sequence}"),
        };
        Arc::make_mut(&mut self.live_tool_calls).push(LiveToolCall::new(
            key.clone(),
            provider_call_id.map(str::to_owned),
            tool_name,
            action,
            target,
        ));
        self.request_redraw();
        key
    }

    /// Update or project a speculative live tool call during stream reception
    /// so partial arguments (e.g. TargetFile, CommandLine, Query) render live.
    pub fn update_speculative_live_tool_call(
        &mut self,
        provider_call_id: Option<&str>,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) {
        if tool_name.is_empty() {
            return;
        }
        let (action, target) = crate::app::activity::summarize_tool_call(tool_name, arguments);
        self.update_speculative_live_tool_call_with_summary(
            provider_call_id,
            tool_name,
            action,
            target,
        );
    }

    /// Project a native text-protocol call before its arguments identify a
    /// target. Fenced calls retain their existing target-based visibility.
    pub fn update_speculative_native_tool_call(
        &mut self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) {
        if tool_name.is_empty() {
            return;
        }
        let (mut action, mut target) =
            crate::app::activity::summarize_tool_call(tool_name, arguments);
        if target.is_empty() || target == "?" {
            action = "Calling".to_owned();
            target = crate::tools::mcp_tool_display_name(tool_name)
                .unwrap_or_else(|| tool_name.to_owned());
        }
        self.update_speculative_live_tool_call_with_summary(None, tool_name, action, target);
    }

    fn update_speculative_live_tool_call_with_summary(
        &mut self,
        provider_call_id: Option<&str>,
        tool_name: &str,
        action: String,
        target: String,
    ) {
        if let Some(call) = Arc::make_mut(&mut self.live_tool_calls)
            .iter_mut()
            .find(|call| {
                !call.execution_started
                    && match provider_call_id {
                        Some(call_id) => call.provider_call_id.as_deref() == Some(call_id),
                        // A provider-less projection represents the one
                        // textual call allowed in the current model turn.
                        // Reuse it even if the streamed tool name changes;
                        // structured calls retain independent provider IDs.
                        None => call.provider_call_id.is_none(),
                    }
            })
        {
            call.tool_name = tool_name.to_owned();
            call.action = action;
            call.target = target;
            self.request_redraw();
            return;
        }

        let sequence = self.live_tool_call_sequence;
        self.live_tool_call_sequence = self.live_tool_call_sequence.saturating_add(1);
        let key = match provider_call_id {
            Some(call_id) => format!("provider:{call_id}:{sequence}"),
            None => format!("local:{sequence}"),
        };
        let mut call = LiveToolCall::new(
            key,
            provider_call_id.map(str::to_owned),
            tool_name,
            action,
            target,
        );
        call.execution_started = false;
        Arc::make_mut(&mut self.live_tool_calls).push(call);
        self.request_redraw();
    }

    /// Append bounded command output to one presentation-only live tool cell.
    /// `stderr` is retained so failures can be styled independently while the
    /// canonical tool result continues to own the complete bounded payload.
    pub fn append_live_tool_output(&mut self, key: &str, bytes: &[u8], stderr: bool) {
        if bytes.is_empty() {
            return;
        }
        let Some(call) = Arc::make_mut(&mut self.live_tool_calls)
            .iter_mut()
            .find(|call| call.key == key)
        else {
            return;
        };
        let text = String::from_utf8_lossy(bytes);
        if let Some(last) = call.output.back_mut()
            && last.stderr == stderr
        {
            last.text.push_str(&text);
        } else {
            call.output.push_back(LiveToolOutputChunk {
                stderr,
                text: text.into_owned(),
            });
        }

        let mut retained = call
            .output
            .iter()
            .map(|chunk| chunk.text.len())
            .sum::<usize>();
        while retained > MAX_LIVE_TOOL_OUTPUT_BYTES && call.output.len() > 1 {
            if let Some(removed) = call.output.pop_front() {
                retained = retained.saturating_sub(removed.text.len());
                call.omitted_output_bytes =
                    call.omitted_output_bytes.saturating_add(removed.text.len());
            }
        }
        if retained > MAX_LIVE_TOOL_OUTPUT_BYTES
            && let Some(chunk) = call.output.front_mut()
        {
            let remove = retained - MAX_LIVE_TOOL_OUTPUT_BYTES;
            let mut boundary = remove.min(chunk.text.len());
            while boundary < chunk.text.len() && !chunk.text.is_char_boundary(boundary) {
                boundary += 1;
            }
            chunk.text.drain(..boundary);
            call.omitted_output_bytes = call.omitted_output_bytes.saturating_add(boundary);
        }
        self.request_redraw();
    }

    /// Finish one live tool projection. The completed semantic result is
    /// persisted separately as the normal `ChatMessage` tool result.
    pub fn finish_live_tool_call(&mut self, key: &str) {
        if let Some(position) = self
            .live_tool_calls
            .iter()
            .position(|live_call| live_call.key == key)
        {
            Arc::make_mut(&mut self.live_tool_calls).remove(position);
            self.request_redraw();
        }
    }

    /// Remove projections left by cancellation or an interrupted turn.
    pub fn clear_live_tool_calls(&mut self) {
        if !self.live_tool_calls.is_empty() {
            Arc::make_mut(&mut self.live_tool_calls).clear();
            self.request_redraw();
        }
    }

    /// Clear all render-only state left by a cancelled or interrupted turn.
    pub(crate) fn clear_active_turn_projection(&mut self) {
        self.clear_current_response();
        self.clear_live_tool_calls();
        self.running_tools.clear();
        self.current_token_usage = None;
        self.stream_tracker = None;
        self.generation_start_time = None;
        self.request_redraw();
    }

    #[cfg(test)]
    pub fn move_tool_confirmation_selection(&mut self, direction: i8) {
        let max = self
            .pending_tool_confirmation
            .as_ref()
            .filter(|items| {
                items.len() == 1
                    && (items[0].rememberable_prefix.is_some()
                        || items[0].forbidden_prefix.is_some())
            })
            .map_or(1, |items| {
                1 + items[0].rememberable_prefix.is_some() as usize
                    + items[0].forbidden_prefix.is_some() as usize
            });
        self.tool_confirmation_selected = if direction < 0 {
            self.tool_confirmation_selected.saturating_sub(1)
        } else {
            (self.tool_confirmation_selected + 1).min(max)
        };
        self.request_redraw();
    }

    /// Consume a pending redraw request.
    pub fn take_redraw_request(&mut self) -> bool {
        std::mem::take(&mut self.redraw_requested)
    }

    #[allow(dead_code)]
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_row = self.last_max_scroll;
    }

    pub fn new() -> Self {
        let workspace = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        Self::new_with_workspace_session(&workspace, None)
    }

    /// Explicit construction for daemon turns without changing the active TUI session.
    pub(crate) fn new_with_workspace_session(
        workspace: &std::path::Path,
        session: Option<&str>,
    ) -> Self {
        let (api_base_url, model_name, mut config) =
            crate::config::load_config_for_workspace(&workspace);
        config.start_time = Some(std::time::SystemTime::now());
        let active_session_id = session
            .map(str::to_owned)
            .unwrap_or_else(|| crate::config::start_session(&mut config));
        if session.is_none() {
            crate::config::record_session_settings(&active_session_id, &config);
        }
        let agent_mode = config.agent_mode;
        let verbosity = config.verbosity.clone();
        let subagent_supervisor =
            crate::app::SubagentSupervisor::new(config.subagent_concurrency_limit);
        let history = History::default();
        let workspace_location = crate::app::workspace::WorkspaceLocationCache::new(
            &workspace,
            std::time::Instant::now(),
        );
        let cwd_and_branch = workspace_location.display();
        if session.is_none() {
            crate::ui::theme::ensure_themes_dir();
            crate::ui::theme::set_active_theme(&config.theme);
        }

        let app = Self {
            input_buffer: String::new(),
            ctrl_c_exit_deadline: None,
            scroll_to_bottom_btn: None,
            hover: HoverTarget::None,
            history,
            history_display_start: 0,
            current_response: Arc::new(String::new()),
            current_response_revision: 0,
            current_response_last_rewrite_revision: 0,
            current_token_usage: None,
            current_thought_time_ms: 0,
            current_thought_tokens: 0,
            current_thought_started_at: None,
            model_quota_remaining: None,
            pending_queue: Vec::new(),
            pending_steers: Vec::new(),
            promoted_steer_prefix_count: 0,
            active_turn_steerable_session: None,
            draft_submit_mode: DraftSubmitMode::Steer,
            background_wakeup_ids: std::collections::BTreeSet::new(),
            pending_background_outputs: Vec::new(),
            background_turn_context: None,
            status: AppStatus::Idle,
            orchestrator_running: false,
            orchestrator_generation: 0,
            orchestrator_owner: None,
            idle_since: std::time::Instant::now(),
            last_turn_had_model_final_response: false,
            summary_in_flight: false,
            last_summary_history_len: None,
            cursor_position: 0,
            suggestion_cycle: crate::app::suggestion::SuggestionCycle::new(),
            response_time: None,
            history_index: None,
            temp_input: String::new(),
            input_history: Vec::new(),
            api_base_url,
            function_calling_support: std::collections::HashMap::new(),
            image_analysis_cache: std::collections::HashMap::new(),
            model_name,
            config,
            cwd_and_branch,
            workspace_location,
            workspace_root: None,
            task_working_directory: None,
            update_check: crate::update::UpdateState::Unknown,
            show_update_prompt: false,
            update_prompt_index: 0,
            dismissed_update_version: None,
            update_requested: false,
            active_suggestion_index: None,
            dismissed_completion: None,
            show_model_picker: false,
            model_picker_index: 0,
            modal_picker_index: 0,
            model_picker_search: String::new(),
            show_theme_picker: false,
            theme_picker_index: 0,
            theme_picker_initial: String::new(),
            verbosity,
            expanded_thoughts: std::collections::HashSet::new(),
            show_command_picker: false,
            command_picker_index: 0,
            command_picker_search: String::new(),
            show_history_picker: false,
            history_picker_index: 0,
            history_picker_sessions: Vec::new(),
            history_picker_truncated: false,
            pending_delete_session_idx: None,
            show_subagent_picker: false,
            subagent_picker_index: 0,
            show_context_modal: false,
            session_title_tool_available: false,
            show_mcp_config: false,
            mcp_picker_index: 0,
            mcp_edit_state: None,
            last_copy_text: None,
            generation_start_time: None,
            pending_tool_confirmation: None,
            pending_approval_details: None,
            pending_approval_batch_id: None,
            modal_scroll_row: 0,
            tool_confirmation_selected: 0,
            tool_confirmation_response: None,
            pending_question: None,
            question_response: None,
            pending_question_queue: Vec::new(),
            pending_question_done: Vec::new(),
            running_tools: Vec::new(),
            live_tool_calls: Arc::new(Vec::new()),
            live_tool_call_sequence: 0,
            stream_tracker: None,
            auto_confirm: false,
            subagent_supervisor,
            active_session_id,
            subagents: Vec::new(),
            selected_subagent_id: None,
            delegation_armed: false,
            delegation_active: false,
            next_subagent_id: 1,
            todos: Vec::new(),
            read_file_mtimes: std::collections::HashMap::new(),
            recent_read_calls: std::collections::VecDeque::new(),
            recent_read_outputs: std::collections::HashMap::new(),
            scroll_row: 0,
            is_scroll_locked_to_bottom: true,
            current_terminal_title: None,
            current_terminal_progress: None,
            session_title_cache: None,
            session_title_cache_generation: 0,
            redraw_requested: false,
            render_revision: 0,
            clear_screen_requested: false,
            last_max_scroll: 0,
            conversation_content_height: 0,
            viewport_height: 0,
            mouse_capture_enabled: true,
            agent_mode,
            chat_area: None,
            input_text_area: None,
            selected_text: None,
            sel_start: None,
            sel_end: None,
            selecting: false,
            sel_in_input: false,
            #[cfg(test)]
            code_copy_rows: Vec::new(),

            last_escape_time: None,

            raw_cli_mode: false,
            tip_index: random_tip_index(),
            continuous_mode: false,
            context_snapshot: None,
            prompt_cache: PromptCache::default(),
            exit_warnings: Vec::new(),
        };
        app
    }

    #[cfg(test)]
    pub fn record_warning(&mut self, warning: impl Into<String>) {
        self.exit_warnings.push(warning.into());
    }

    /// True when any modal overlay is open (pickers or tool confirmation);
    /// the background content renders dimmed.
    pub fn modal_open(&self) -> bool {
        self.show_model_picker
            || self.show_theme_picker
            || self.show_command_picker
            || self.show_history_picker
            || self.show_subagent_picker
            || self.show_context_modal
            || self.show_update_prompt
            || self.show_mcp_config
            || self.status == AppStatus::AwaitingToolConfirmation
            || self.status == AppStatus::AwaitingQuestion
            || self.status == AppStatus::VerbosityPicker
            || self.status == AppStatus::ThinkingPicker
            || self.status == AppStatus::EffortPicker
            || self.status == AppStatus::ProtocolPicker
            || self.status == AppStatus::YoloPicker
    }

    /// Restores status when closing a modal or picker, preserving running turns if active.
    pub fn close_modal_status(&mut self) {
        self.status = if self.orchestrator_running {
            AppStatus::Streaming
        } else if !self.pending_queue.is_empty() {
            AppStatus::Queued
        } else {
            AppStatus::Idle
        };
    }

    /// Context window of the active profile, in tokens.
    pub fn active_context_window(&self) -> u32 {
        self.active_context_budget().context_window
    }

    pub fn active_model_profile(&self) -> Option<crate::config::ModelProfile> {
        self.config
            .models
            .iter()
            .find(|p| p.matches_request(&self.api_base_url, &self.model_name))
            .or_else(|| {
                let mut candidates = self
                    .config
                    .models
                    .iter()
                    .filter(|p| p.model == self.model_name || p.name == self.model_name);
                let candidate = candidates.next()?;
                candidates.next().is_none().then_some(candidate)
            })
            .cloned()
    }

    /// Whether the active endpoint/model should receive the local-model
    /// compatibility contract when no profile override says otherwise.
    pub fn active_model_is_local(&self) -> bool {
        self.active_model_profile()
            .is_some_and(|profile| profile.is_local())
            || {
                let lower = self.api_base_url.to_ascii_lowercase();
                lower.contains("11434") || lower.contains("ollama")
            }
    }

    pub fn vision_model_profile(&self) -> Option<crate::config::ModelProfile> {
        let name = self.config.vision_model.as_deref()?;
        self.config
            .models
            .iter()
            .find(|p| p.name == name || p.model == name)
            .cloned()
    }

    pub fn get_history_token_budget(&self) -> u32 {
        self.active_context_budget().history_tokens
    }

    /// A single model-aware budget shared by automatic compaction and final
    /// request trimming. Profiles without a matching entry retain the existing
    /// default window and conservative reserves.
    pub fn active_context_budget(&self) -> crate::config::ContextBudget {
        self.active_model_profile()
            .map(|profile| profile.context_budget())
            .unwrap_or_else(|| {
                crate::config::ModelProfile {
                    name: self.model_name.clone(),
                    url: self.api_base_url.clone(),
                    model: self.model_name.clone(),
                    context_window: Some(crate::config::DEFAULT_CONTEXT_WINDOW),
                    engine: None,
                    api_key: None,
                    env_key: None,
                    tool_protocol: None,
                    enable_thinking: None,
                    reasoning_effort: None,
                    max_tokens: None,
                    supports_vision: None,
                    ..Default::default()
                }
                .context_budget()
            })
    }

    fn clamp_cursor(&mut self) {
        self.cursor_position = self.cursor_position.min(self.input_buffer.len());
        while !self.input_buffer.is_char_boundary(self.cursor_position) {
            self.cursor_position -= 1;
        }
    }

    fn char_len_before_cursor(&self) -> Option<usize> {
        self.input_buffer[..self.cursor_position]
            .chars()
            .next_back()
            .map(|c| c.len_utf8())
    }

    pub fn insert_char(&mut self, c: char) {
        self.history_index = None;
        self.clamp_cursor();
        self.input_buffer.insert(self.cursor_position, c);
        self.cursor_position += c.len_utf8();
        self.reset_suggestion_index();
        self.request_redraw();
    }

    pub fn delete_char_backspace(&mut self) {
        self.history_index = None;
        self.clamp_cursor();
        if let Some(len) = self.char_len_before_cursor() {
            self.cursor_position -= len;
            self.input_buffer.remove(self.cursor_position);
        }
        self.reset_suggestion_index();
        self.request_redraw();
    }

    pub fn delete_char_delete(&mut self) {
        self.history_index = None;
        self.clamp_cursor();
        if self.cursor_position < self.input_buffer.len() {
            self.input_buffer.remove(self.cursor_position);
        }
        self.reset_suggestion_index();
        self.request_redraw();
    }

    pub fn delete_word_backspace(&mut self) {
        self.history_index = None;
        self.clamp_cursor();
        let end = self.cursor_position;
        self.move_cursor_word_left();
        let start = self.cursor_position;
        if start < end {
            self.input_buffer.replace_range(start..end, "");
        }
        self.reset_suggestion_index();
        self.request_redraw();
    }

    pub fn delete_word_forward(&mut self) {
        self.history_index = None;
        self.clamp_cursor();
        let start = self.cursor_position;
        self.move_cursor_word_right();
        let end = self.cursor_position;
        self.cursor_position = start;
        if start < end {
            self.input_buffer.replace_range(start..end, "");
        }
        self.reset_suggestion_index();
        self.request_redraw();
    }

    pub fn kill_line_to_start(&mut self) {
        self.history_index = None;
        self.clamp_cursor();
        let end = self.cursor_position;
        let start = self.input_buffer[..end].rfind('\n').map_or(0, |i| i + 1);
        if start < end {
            self.input_buffer.replace_range(start..end, "");
            self.cursor_position = start;
        }
        self.reset_suggestion_index();
        self.request_redraw();
    }

    pub fn reset_suggestion_index(&mut self) {
        let completion = self.completion_identity();
        if self.dismissed_completion.as_ref() != completion.as_ref() {
            self.dismissed_completion = None;
        }
        if completion.is_some() && self.dismissed_completion.is_none() {
            if self.active_suggestion_index.is_none() {
                self.active_suggestion_index = Some(0);
            }
        } else {
            self.active_suggestion_index = None;
        }
    }

    pub fn completion_identity(&self) -> Option<String> {
        if let Some(command) = crate::app::suggestion::command_token(&self.input_buffer) {
            return Some(format!("command:{command}"));
        }
        crate::app::get_at_word_query(&self.input_buffer, self.cursor_position)
            .map(|(start, query)| format!("file:{start}:{query}"))
    }

    pub fn dismiss_completion(&mut self) -> bool {
        let Some(completion) = self.completion_identity() else {
            return false;
        };
        self.dismissed_completion = Some(completion);
        self.active_suggestion_index = None;
        self.request_redraw();
        true
    }

    pub fn move_cursor_left(&mut self) {
        self.clamp_cursor();
        if let Some(len) = self.char_len_before_cursor() {
            self.cursor_position -= len;
        }
        self.reset_suggestion_index();
        self.request_redraw();
    }

    pub fn move_cursor_right(&mut self) {
        self.clamp_cursor();
        if let Some(c) = self.input_buffer[self.cursor_position..].chars().next() {
            self.cursor_position += c.len_utf8();
        }
        self.reset_suggestion_index();
        self.request_redraw();
    }

    pub fn move_cursor_word_left(&mut self) {
        self.clamp_cursor();
        let mut pos = self.cursor_position;

        while let Some(c) = self.input_buffer[..pos].chars().next_back() {
            if !c.is_whitespace() {
                break;
            }
            pos -= c.len_utf8();
        }

        while let Some(c) = self.input_buffer[..pos].chars().next_back() {
            if c.is_whitespace() {
                break;
            }
            pos -= c.len_utf8();
        }
        self.cursor_position = pos;
        self.reset_suggestion_index();
        self.request_redraw();
    }

    pub fn move_cursor_word_right(&mut self) {
        self.clamp_cursor();
        let mut pos = self.cursor_position;

        while let Some(c) = self.input_buffer[pos..].chars().next() {
            if c.is_whitespace() {
                break;
            }
            pos += c.len_utf8();
        }

        while let Some(c) = self.input_buffer[pos..].chars().next() {
            if !c.is_whitespace() {
                break;
            }
            pos += c.len_utf8();
        }
        self.cursor_position = pos;
        self.reset_suggestion_index();
        self.request_redraw();
    }

    pub fn move_cursor_to_start(&mut self) {
        self.cursor_position = 0;
        self.reset_suggestion_index();
        self.request_redraw();
    }

    pub fn move_cursor_to_end(&mut self) {
        self.cursor_position = self.input_buffer.len();
        self.reset_suggestion_index();
        self.request_redraw();
    }

    pub fn move_cursor_line_up(&mut self) {
        self.clamp_cursor();
        let pos = self.cursor_position;
        let before = &self.input_buffer[..pos];
        let current_line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let col = before[current_line_start..].chars().count();

        if current_line_start > 0 {
            let prev_line_end = current_line_start - 1;
            let prev_line_start = self.input_buffer[..prev_line_end]
                .rfind('\n')
                .map_or(0, |i| i + 1);
            let prev_line = &self.input_buffer[prev_line_start..prev_line_end];
            let prev_char_count = prev_line.chars().count();
            let target_col = col.min(prev_char_count);
            let target_byte_offset: usize = prev_line
                .chars()
                .take(target_col)
                .map(|c| c.len_utf8())
                .sum();
            self.cursor_position = prev_line_start + target_byte_offset;
        } else {
            self.cursor_position = 0;
        }
        self.request_redraw();
    }

    pub fn move_cursor_line_down(&mut self) {
        self.clamp_cursor();
        let pos = self.cursor_position;
        let before = &self.input_buffer[..pos];
        let current_line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let col = before[current_line_start..].chars().count();

        if let Some(next_line_start_rel) = self.input_buffer[pos..].find('\n') {
            let next_line_start = pos + next_line_start_rel + 1;
            let next_line_end = self.input_buffer[next_line_start..]
                .find('\n')
                .map_or(self.input_buffer.len(), |i| next_line_start + i);
            let next_line = &self.input_buffer[next_line_start..next_line_end];
            let next_char_count = next_line.chars().count();
            let target_col = col.min(next_char_count);
            let target_byte_offset: usize = next_line
                .chars()
                .take(target_col)
                .map(|c| c.len_utf8())
                .sum();
            self.cursor_position = next_line_start + target_byte_offset;
        } else {
            self.cursor_position = self.input_buffer.len();
        }
        self.request_redraw();
    }

    pub fn get_command_suggestion(&self) -> Option<String> {
        self.suggestion_cycle
            .get_completion_suffix(&self.input_buffer)
    }

    pub fn cycle_suggestion(&mut self) {
        if self.suggestion_cycle.cycle(&self.input_buffer) {
            let Some(command) =
                crate::app::suggestion::command_token(&self.input_buffer).map(str::to_owned)
            else {
                return;
            };
            let prefix = self
                .suggestion_cycle
                .original_prefix
                .as_deref()
                .unwrap_or(&self.input_buffer);
            let matches: Vec<&str> = crate::app::suggestion::filtered_commands(prefix)
                .iter()
                .map(|command| command.name)
                .collect();
            if let Some(idx) = self.suggestion_cycle.suggestion_index
                && idx < matches.len()
            {
                let replacement = matches[idx];
                let suffix_cursor = self.cursor_position.saturating_sub(command.len());
                self.input_buffer
                    .replace_range(0..command.len(), replacement);
                self.cursor_position = replacement
                    .len()
                    .saturating_add(suffix_cursor)
                    .min(self.input_buffer.len());
            }
        }
        self.request_redraw();
    }

    pub fn reset_suggestion_cycle(&mut self) {
        self.composer().reset_suggestion_cycle();
        self.request_redraw();
    }

    /// Pull the most recently queued prompt back into the input box so the
    /// user can edit or drop it. Internal wakeup entries are left untouched.
    /// Returns true when a prompt was pulled.
    pub fn pop_queued_prompt(&mut self) -> bool {
        let changed = self.composer().pop_queued_prompt();
        if changed {
            self.request_redraw();
        }
        changed
    }

    /// Whether the current streaming turn is the regular interactive turn
    /// explicitly marked as accepting steering, with no blocking interaction
    /// awaiting the user's decision.
    pub(crate) fn can_accept_steer(&self) -> bool {
        self.active_turn_steerable_session.as_deref() == Some(self.active_session_id.as_str())
            && self.status == AppStatus::Streaming
            && self.pending_tool_confirmation.is_none()
            && self.pending_question.is_none()
            && self.pending_question_queue.is_empty()
    }

    /// Queue one distinct steer for the active session. Whitespace-only text
    /// is ignored; valid text is preserved byte-for-byte.
    pub(crate) fn queue_steer(&mut self, text: String) -> bool {
        if text.trim().is_empty() || !self.can_accept_steer() {
            return false;
        }
        self.pending_steers.push(PendingSteer {
            session_id: self.active_session_id.clone(),
            text,
        });
        true
    }

    /// Atomically remove the complete steer batch for `session_id` so callers
    /// can append it to history under the same state lock.
    pub(crate) fn take_steers_for_history(&mut self, session_id: &str) -> Vec<String> {
        if self.active_turn_steerable_session.as_deref() != Some(session_id)
            || self
                .pending_steers
                .iter()
                .any(|steer| steer.session_id != session_id)
        {
            return Vec::new();
        }
        self.pending_steers
            .drain(..)
            .map(|steer| steer.text)
            .collect()
    }

    /// Move this session's still-pending steers ahead of existing follow-ups.
    /// Finalization may already have cleared the active marker; the active
    /// session and every captured item must still match before promotion.
    pub(crate) fn promote_pending_steers_to_queue(&mut self, session_id: &str) {
        if self.active_session_id != session_id
            || self.pending_steers.is_empty()
            || self
                .pending_steers
                .iter()
                .any(|steer| steer.session_id != session_id)
        {
            return;
        }
        let prompts = self
            .pending_steers
            .drain(..)
            .map(|steer| steer.text)
            .collect::<Vec<_>>();
        let promoted_count = prompts.len();
        let insertion_position = self
            .promoted_steer_prefix_count
            .min(self.pending_queue.len());
        self.pending_queue
            .splice(insertion_position..insertion_position, prompts);
        self.promoted_steer_prefix_count = self
            .promoted_steer_prefix_count
            .saturating_add(promoted_count);
        if self.active_turn_steerable_session.as_deref() == Some(session_id) {
            self.active_turn_steerable_session = None;
        }
    }

    /// Drop steering state when the active session is replaced.
    pub(crate) fn clear_session_steering(&mut self) {
        self.pending_steers.clear();
        self.promoted_steer_prefix_count = 0;
        self.active_turn_steerable_session = None;
        self.draft_submit_mode = DraftSubmitMode::Steer;
    }

    /// Mark a popped FIFO head as a promoted steer when it belongs to the
    /// tracked queue prefix.
    pub(crate) fn take_promoted_steer_prefix_prompt(&mut self) -> bool {
        if self.promoted_steer_prefix_count == 0 {
            return false;
        }
        self.promoted_steer_prefix_count -= 1;
        true
    }

    /// Keep the promoted-steer prefix count aligned when an item is removed
    /// from the pending queue for editing or cancellation.
    pub(crate) fn note_pending_prompt_removed(&mut self, position: usize) {
        if position < self.promoted_steer_prefix_count {
            self.promoted_steer_prefix_count -= 1;
        }
    }

    /// Remove background wakeups whose results are already part of the history
    /// snapshot being sent to the model. User prompts remain queued in order.
    pub(crate) fn consume_observed_background_wakeups(&mut self) -> usize {
        let before = self.pending_queue.len();
        self.pending_queue
            .retain(|item| !item.starts_with("__task_wakeup__:"));
        before.saturating_sub(self.pending_queue.len())
    }

    pub fn history_up(&mut self) {
        self.composer().history_up();
        self.request_redraw();
    }

    pub fn history_down(&mut self) {
        self.composer().history_down();
        self.request_redraw();
    }

    #[allow(dead_code)]
    pub fn scroll_up(&mut self, amount: u16) {
        self.clear_selection();
        self.is_scroll_locked_to_bottom = false;
        self.scroll_row = self.scroll_row.saturating_sub(amount);
    }

    #[allow(dead_code)]
    pub fn scroll_down(&mut self, amount: u16) {
        self.clear_selection();
        let max = self.last_max_scroll;
        let next = self.scroll_row.saturating_add(amount).min(max);
        self.scroll_row = next;
        if next >= max {
            self.is_scroll_locked_to_bottom = true;
        }
    }

    /// One page = the visible conversation height, minus a line of overlap for context.
    #[allow(dead_code)]
    pub fn page_rows(&self) -> u16 {
        self.viewport_height.saturating_sub(1).max(1)
    }

    /// Which clickable element sits under a screen cell. Hit-tested against the
    /// rects and rows the last render recorded, so it is only meaningful for
    /// coordinates from the current frame.
    #[cfg(test)]
    pub fn hover_target_at(&self, column: u16, row: u16) -> HoverTarget {
        if let Some(rect) = self.scroll_to_bottom_btn
            && rect.contains(ratatui::layout::Position::new(column, row))
        {
            return HoverTarget::ScrollPill;
        }
        if let Some((_, code_text)) = self.code_copy_rows.iter().find(|(r, _)| *r == row) {
            let badge_width = if self
                .last_copy_text
                .as_ref()
                .is_some_and(|(t_text, t)| t_text == code_text && t.elapsed().as_secs() < 2)
            {
                12
            } else {
                9
            };
            let is_on_copy_button = self.chat_area.map_or(true, |ca| {
                column >= (ca.x + ca.width).saturating_sub(badge_width)
            });
            if is_on_copy_button {
                return HoverTarget::CopyBadge(row);
            }
        }
        HoverTarget::None
    }

    /// Tool protocol to use when talking to `url`.
    ///
    /// A profile override wins; otherwise a provider known to implement
    /// function calling gets the structured protocol, because a call returned
    /// as data cannot be confused with prose about a call. Everything else
    /// falls back to the configured text protocol, which is what servers
    /// without function calling need. A rejected probe cannot fall back to an
    /// API-native global default, so that case uses the JSON text protocol.
    pub fn tool_protocol_for(&self, url: &str) -> crate::config::ToolProtocol {
        if let Some(profile) = self.config.models.iter().find(|profile| profile.url == url)
            && let Some(protocol) = profile.tool_protocol
        {
            return protocol;
        }
        if self.config.models.iter().any(|profile| {
            (profile.url == url || profile.endpoint_url() == url)
                && profile.resolved_api_protocol() == crate::config::ApiProtocol::Responses
        }) {
            return crate::config::ToolProtocol::ApiNative;
        }
        let detected_support = self.function_calling_support.get(url).copied();
        if crate::config::provider_supports_function_calling(url) || detected_support == Some(true)
        {
            return crate::config::ToolProtocol::ApiNative;
        }
        if detected_support == Some(false)
            && self.config.tool_protocol == crate::config::ToolProtocol::ApiNative
        {
            return crate::config::ToolProtocol::Json;
        }
        self.config.tool_protocol
    }

    /// True when this endpoint's function-calling support is still unknown, so
    /// the caller should probe before building a turn.
    pub fn function_calling_unknown(&self, url: &str) -> bool {
        let responses_api = self.config.models.iter().any(|profile| {
            (profile.url == url || profile.endpoint_url() == url)
                && profile.resolved_api_protocol() == crate::config::ApiProtocol::Responses
        });
        if responses_api {
            return false;
        }
        let overridden = self
            .config
            .models
            .iter()
            .any(|profile| profile.url == url && profile.tool_protocol.is_some());
        !overridden
            && !crate::config::provider_supports_function_calling(url)
            && !self.function_calling_support.contains_key(url)
    }

    /// Record what a probe found, for this run only.
    ///
    /// Deliberately not written to the config file: the answer can change when
    /// a gateway is reconfigured, and silently editing settings the user did not
    /// write would make that change impossible to notice. One tiny request per
    /// endpoint per run is cheaper than a wrong answer cached forever. Use
    /// `/protocol` to pin a choice.
    pub fn record_function_calling_support(&mut self, url: &str, supported: bool) {
        self.function_calling_support
            .insert(url.to_string(), supported);
    }

    /// Tool protocol for the model this session is currently talking to.
    pub fn active_tool_protocol(&self) -> crate::config::ToolProtocol {
        self.tool_protocol_for(&self.api_base_url)
    }

    pub fn clear_selection(&mut self) {
        self.sel_start = None;
        self.sel_end = None;
        self.selecting = false;
        self.sel_in_input = false;
    }

    /// Append a compact notification to the durable conversation transcript.
    pub fn set_notice(&mut self, text: impl Into<String>) {
        self.history.push(ChatMessage::new("system", text.into()));
        self.request_redraw();
    }

    pub fn set_warning_notice(&mut self, text: impl Into<String>) {
        self.set_notice(text);
    }
}

#[cfg(test)]
#[path = "state/chat_message_tests.rs"]
mod chat_message_tests;
#[cfg(test)]
#[path = "state/history_tests.rs"]
mod history_tests;
#[cfg(test)]
#[path = "state/hover_tests.rs"]
mod hover_tests;
#[cfg(test)]
#[path = "state/idle_summary_tests.rs"]
mod idle_summary_tests;
#[cfg(test)]
#[path = "state/input_history_tests.rs"]
mod input_history_tests;
#[cfg(test)]
#[path = "state/live_tool_tests.rs"]
mod live_tool_tests;
#[cfg(test)]
#[path = "state/prompt_cache_tests.rs"]
mod prompt_cache_tests;
#[cfg(test)]
#[path = "state/protocol_tests.rs"]
mod protocol_tests;
#[cfg(test)]
#[path = "state/question_chain_tests.rs"]
mod question_chain_tests;
#[cfg(test)]
#[path = "state/queue_pull_back_tests.rs"]
mod queue_pull_back_tests;
#[cfg(test)]
#[path = "state/stall_watchdog_tests.rs"]
mod stall_watchdog_tests;
#[cfg(test)]
#[path = "state/steering_tests.rs"]
mod steering_tests;
