//! What a session owner publishes on its [`OwnerLink`](super::owner::OwnerLink).
//!
//! [`SessionPublisher`] turns the owner's applied state into the ordered
//! [`OwnerMessage`]s of one registration. It does no I/O: the owner calls
//! [`SessionPublisher::publish`] while it holds the state lock and sends the
//! returned messages after releasing it.
//!
//! The engine's turn events ([`AgentUiEvent`]) trail the state they were
//! derived from and carry no turn, question or batch identity, so they cannot
//! be cut exactly against a snapshot. Everything a client must not see twice
//! or must name later is therefore read from the state itself, under the same
//! lock that builds a snapshot: response text, the pending question and the
//! pending approval. The remaining events (tools, subagents) are keyed by an
//! ID and only ever restate what a snapshot already holds.

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hasher};

use crate::app::AppState;
use crate::controller::AgentUiEvent;

use super::ops::SessionRegistration;
use super::owner::OwnerMessage;
use super::projection::{
    ProjectionContext, ProjectionLimits, bound_text, project_active_timing, project_ended_timing,
    project_event, project_live_thought_time, project_pending_approval, project_pending_question,
    project_session_info, project_snapshot, project_turn_timing,
};
use super::protocol::{
    OwnerHealth, REMOTE_PROTOCOL_VERSION, RemoteEvent, RemoteEventFrame, RemoteSessionInfo,
    ResyncReason,
};

/// Position in the live response up to which text was published.
#[derive(Default)]
struct ResponseCursor {
    revision: u64,
    len: usize,
    /// Length and hash of the text published since the response last
    /// restarted, to tell at the end of a turn what is still missing.
    emitted_len: usize,
    emitted_hash: DefaultHasher,
}

impl ResponseCursor {
    /// A cursor that has published everything `state` currently holds.
    fn at(state: &AppState) -> Self {
        let mut emitted_hash = DefaultHasher::new();
        emitted_hash.write(state.current_response.as_bytes());
        Self {
            revision: state.current_response_revision,
            len: state.current_response.len(),
            emitted_len: state.current_response.len(),
            emitted_hash,
        }
    }

    /// Text added since the cursor, or the whole response when it was
    /// rewritten rather than extended.
    fn advance(&mut self, state: &AppState) -> Option<String> {
        let response = state.current_response.as_str();
        let revision = state.current_response_revision;
        if revision == self.revision && response.len() == self.len {
            return None;
        }
        let extends = state.current_response_last_rewrite_revision <= self.revision
            && response.len() >= self.len
            && response.is_char_boundary(self.len);
        let text = if extends {
            self.emitted_len += response.len() - self.len;
            &response[self.len..]
        } else {
            self.emitted_len = response.len();
            self.emitted_hash = DefaultHasher::new();
            response
        };
        self.emitted_hash.write(text.as_bytes());
        self.revision = revision;
        self.len = response.len();
        (!text.is_empty()).then(|| text.to_owned())
    }

    /// The end of the finished response that was never observed in the live
    /// buffer. A response that rewrote published text yields nothing: a delta
    /// cannot replace, and the transcript carries the final version.
    fn finish<'a>(&self, final_content: &'a str) -> &'a str {
        let prefix_matches = final_content.get(..self.emitted_len).is_some_and(|prefix| {
            let mut hasher = DefaultHasher::new();
            hasher.write(prefix.as_bytes());
            hasher.finish() == self.emitted_hash.finish()
        });
        if prefix_matches {
            &final_content[self.emitted_len..]
        } else {
            ""
        }
    }
}

/// State of the session no event describes; a change is published as a
/// snapshot.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Unannounced {
    history_rewrites: u64,
    history_len: usize,
    queued_prompts: usize,
}

impl Unannounced {
    fn of(state: &AppState) -> Self {
        Self {
            history_rewrites: state.history.rewrite_revision(),
            history_len: state.history.len(),
            queued_prompts: state.pending_queue.len() + state.pending_steers.len(),
        }
    }
}

/// Publishes one registration: assigns sequence numbers, tracks what the
/// remote side has been told, and falls back to a snapshot when events were
/// lost.
pub struct SessionPublisher {
    registration: SessionRegistration,
    limits: ProjectionLimits,
    /// Sequence of the last event produced, sent or not.
    sequence: u64,
    /// Agent events applied to the state since the last publication.
    observed: Vec<AgentUiEvent>,
    /// Turn whose `PromptStarted` was observed without its end, by the
    /// identity the state gave it then.
    open_turn: Option<String>,
    /// Turn the remote side was told is running.
    announced_turn: Option<String>,
    response: ResponseCursor,
    question_id: Option<String>,
    batch_id: Option<String>,
    unannounced: Unannounced,
    info: RemoteSessionInfo,
    /// The outbound queue overflowed; nothing is sent until a resync.
    lagging: bool,
    snapshot_due: Option<Option<ResyncReason>>,
    live_clock: Option<(String, Option<u64>, Option<u64>)>,
    snapshot_terminal_turns: HashSet<String>,
}

fn pending_batch_id(state: &AppState) -> Option<&str> {
    state
        .pending_tool_confirmation
        .as_ref()
        .filter(|confirmations| !confirmations.is_empty())
        .and(state.pending_approval_batch_id.as_deref())
}

impl SessionPublisher {
    /// Start publishing `state` under `registration`. Returns the publisher
    /// and the [`OwnerMessage::Register`] that opens the link.
    ///
    /// `turn_stream_open` says whether the owner has applied a
    /// `PromptStarted` whose turn end it has not seen yet, so the end of a
    /// turn that was already running is published too.
    pub fn register(
        state: &AppState,
        registration: SessionRegistration,
        turn_stream_open: bool,
    ) -> (Self, OwnerMessage) {
        let limits = ProjectionLimits::default();
        let mut publisher = Self {
            info: project_session_info(state, &Self::context(&registration, 0, state)),
            registration,
            limits,
            sequence: 0,
            observed: Vec::new(),
            open_turn: state.active_turn_id.clone().filter(|_| turn_stream_open),
            announced_turn: None,
            response: ResponseCursor::default(),
            question_id: None,
            batch_id: None,
            unannounced: Unannounced::of(state),
            lagging: false,
            snapshot_due: None,
            live_clock: None,
            snapshot_terminal_turns: HashSet::new(),
        };
        let OwnerMessage::Snapshot { snapshot, .. } = publisher.snapshot(state, None) else {
            unreachable!("snapshot() builds a snapshot message");
        };
        let register = OwnerMessage::Register {
            registration: publisher.registration.clone(),
            snapshot,
        };
        (publisher, register)
    }

    pub fn registration(&self) -> &SessionRegistration {
        &self.registration
    }

    /// Sequence of the last event produced under this registration.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Whether events are being dropped until the next resync snapshot.
    pub fn is_lagging(&self) -> bool {
        self.lagging
    }

    /// Whether `state` still shows the session this registration shares.
    pub fn owns(&self, state: &AppState) -> bool {
        state.active_session_id == self.registration.session_id
    }

    /// Note an agent event the owner has applied. Text deltas are not kept:
    /// response text is read from the state when publishing.
    pub fn observe(&mut self, event: &AgentUiEvent) {
        if !matches!(event, AgentUiEvent::TextDelta { .. }) {
            self.observed.push(event.clone());
        }
    }

    /// Publish a snapshot with the next publication.
    pub fn request_snapshot(&mut self) {
        self.snapshot_due.get_or_insert(None);
    }

    /// The outbound queue refused a message: stop sending events and resync
    /// once there is room.
    pub fn mark_lagging(&mut self) {
        self.lagging = true;
    }

    fn context(
        registration: &SessionRegistration,
        sequence: u64,
        state: &AppState,
    ) -> ProjectionContext {
        ProjectionContext {
            registration_epoch: registration.registration_epoch,
            sequence,
            // A terminal owner has no controller generation.
            generation: 0,
            health: OwnerHealth::Live,
            title: state
                .session_title_cache
                .as_ref()
                .filter(|(session_id, _)| *session_id == state.active_session_id)
                .and_then(|(_, title)| title.clone()),
        }
    }

    fn event(&mut self, event: RemoteEvent) -> OwnerMessage {
        self.sequence += 1;
        OwnerMessage::Event(RemoteEventFrame {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            session_id: self.registration.session_id.clone(),
            registration_epoch: self.registration.registration_epoch,
            sequence: self.sequence,
            generation: 0,
            event,
        })
    }

    /// Snapshot `state` at the current sequence and align everything the
    /// publisher tracks with it, so only later changes are published after.
    fn snapshot(&mut self, state: &AppState, resync: Option<ResyncReason>) -> OwnerMessage {
        let snapshot = project_snapshot(
            state,
            &Self::context(&self.registration, self.sequence, state),
            &self.limits,
        );
        self.announced_turn = state.active_turn_id.clone();
        self.snapshot_terminal_turns = state
            .history
            .iter()
            .filter_map(|message| message.turn.as_ref())
            .filter(|turn| turn.outcome.is_some())
            .map(|turn| turn.turn_id.clone())
            .collect();
        self.live_clock = snapshot.turn.as_ref().and_then(|turn| {
            turn.timing.as_ref().map(|timing| {
                (
                    timing.turn_id.clone(),
                    timing.elapsed_work_ms.map(|ms| ms / 1000),
                    turn.thought_time_ms.map(|ms| ms / 1000),
                )
            })
        });
        self.response = ResponseCursor::at(state);
        self.question_id = snapshot
            .pending_question
            .as_ref()
            .map(|question| question.question_id.clone());
        self.batch_id = snapshot
            .pending_approval
            .as_ref()
            .map(|approval| approval.batch_id.clone());
        self.unannounced = Unannounced::of(state);
        self.info = snapshot.session.clone();
        self.snapshot_due = None;
        OwnerMessage::Snapshot {
            snapshot: Box::new(snapshot),
            resync,
        }
    }

    /// Everything that changed since the last call, in order. `state` must be
    /// the session this registration [`owns`](Self::owns). `room_for_resync`
    /// says whether a lagging link has drained enough to take a snapshot.
    pub fn publish(&mut self, state: &AppState, room_for_resync: bool) -> Vec<OwnerMessage> {
        let mut messages = Vec::new();
        for event in std::mem::take(&mut self.observed) {
            match event {
                AgentUiEvent::PromptStarted { prompt, timing } => {
                    if timing
                        .as_ref()
                        .is_some_and(|turn| self.snapshot_terminal_turns.contains(&turn.turn_id))
                    {
                        continue;
                    }
                    self.open_turn = timing
                        .as_ref()
                        .map(|turn| turn.turn_id.clone())
                        .or_else(|| state.active_turn_id.clone());
                    if timing.is_some()
                        && self.announced_turn.is_some()
                        && self.announced_turn == state.active_turn_id
                        && self.open_turn != state.active_turn_id
                    {
                        self.request_snapshot();
                        continue;
                    }
                    match &self.open_turn {
                        // A snapshot already announced this turn.
                        Some(turn_id) if self.announced_turn.as_ref() == Some(turn_id) => {}
                        Some(turn_id) => {
                            self.announced_turn = Some(turn_id.clone());
                            self.response = ResponseCursor::default();
                            let started = RemoteEvent::TurnStarted {
                                turn_id: Some(turn_id.clone()),
                                timing: timing.as_ref().map(project_turn_timing),
                                prompt: bound_text(&prompt, self.limits.text_bytes, None),
                            };
                            messages.push(self.event(started));
                        }
                        // The turn ended before its start was observed.
                        None => self.request_snapshot(),
                    }
                }
                AgentUiEvent::TurnFinished {
                    content, timing, ..
                } => {
                    let authoritative = timing.is_some();
                    let timing = timing.as_ref().map(project_turn_timing).or_else(|| {
                        self.open_turn
                            .as_ref()
                            .and_then(|id| project_ended_timing(state, id))
                    });
                    if timing.as_ref().is_some_and(|turn| {
                        turn.outcome.is_none() && (authoritative || turn.elapsed_work_ms.is_some())
                    }) {
                        self.request_snapshot();
                        continue;
                    }
                    if let Some(turn_id) = self.end_of_announced_turn_with_timing(timing.as_ref()) {
                        let rest = self.response.finish(&content);
                        if rest.len() > self.limits.live_response_bytes {
                            self.request_snapshot();
                        } else if !rest.is_empty() {
                            let text = rest.to_owned();
                            messages.push(self.event(RemoteEvent::TextDelta {
                                text,
                                timing: None,
                                thought_time_ms: None,
                            }));
                        }
                        messages.push(self.event(RemoteEvent::TurnFinished {
                            timing: timing.filter(|turn| turn.outcome.is_some()),
                            turn_id: Some(turn_id),
                        }));
                        self.unannounced.history_len = state.history.len();
                    }
                }
                AgentUiEvent::Cancelled { timing, .. } => {
                    let timing = timing.as_ref().map(project_turn_timing).or_else(|| {
                        self.open_turn
                            .as_ref()
                            .and_then(|id| project_ended_timing(state, id))
                    });
                    if let Some(turn_id) = self.end_of_announced_turn_with_timing(timing.as_ref()) {
                        messages.push(self.event(RemoteEvent::TurnCancelled {
                            timing: timing.filter(|turn| turn.outcome.is_some()),
                            turn_id: Some(turn_id),
                        }));
                        self.unannounced.history_len = state.history.len();
                    }
                }
                // Published from the state below, where their identity is.
                AgentUiEvent::TextDelta { .. }
                | AgentUiEvent::QuestionRequested { .. }
                | AgentUiEvent::ApprovalRequested { .. } => {}
                other => {
                    for event in project_event(state, &other, &self.limits) {
                        messages.push(self.event(event));
                    }
                }
            }
        }

        if self.announced_turn.is_some() && state.active_turn_id == self.announced_turn {
            match self.response.advance(state) {
                Some(text) if text.len() > self.limits.live_response_bytes => {
                    self.request_snapshot();
                }
                Some(text) => messages.push(
                    self.event(RemoteEvent::TextDelta {
                        text,
                        timing: project_active_timing(state)
                            .filter(|timing| timing.elapsed_work_ms.is_some()),
                        thought_time_ms: project_live_thought_time(state),
                    }),
                ),
                None => {}
            }
        }

        if self.announced_turn.is_some() && state.active_turn_id == self.announced_turn {
            let timing = project_active_timing(state);
            let thought = project_live_thought_time(state);
            if let Some(timing) = timing.filter(|timing| timing.elapsed_work_ms.is_some()) {
                // Clock-only updates once per displayed second, still within
                // the normal sequenced event stream and replay ring.
                let clock = (
                    timing.turn_id.clone(),
                    timing.elapsed_work_ms.map(|ms| ms / 1000),
                    thought.map(|ms| ms / 1000),
                );
                if self.live_clock.as_ref() != Some(&clock) {
                    self.live_clock = Some(clock);
                    messages.push(self.event(RemoteEvent::TextDelta {
                        text: String::new(),
                        timing: Some(timing),
                        thought_time_ms: thought,
                    }));
                }
            }
        } else {
            self.live_clock = None;
        }

        let question_id = state.pending_question.as_ref().map(|q| q.id.as_str());
        if question_id != self.question_id.as_deref() {
            if let Some(question_id) = self.question_id.take() {
                messages.push(self.event(RemoteEvent::QuestionResolved { question_id }));
            }
            if let Some(question) = project_pending_question(state) {
                self.question_id = Some(question.question_id.clone());
                messages.push(self.event(RemoteEvent::QuestionRequested { question }));
            }
        }
        if pending_batch_id(state) != self.batch_id.as_deref() {
            if let Some(batch_id) = self.batch_id.take() {
                messages.push(self.event(RemoteEvent::ApprovalResolved { batch_id }));
            }
            if let Some(approval) = project_pending_approval(state, &self.limits) {
                self.batch_id = Some(approval.batch_id.clone());
                messages.push(self.event(RemoteEvent::ApprovalRequested { approval }));
            }
        }

        let unannounced = Unannounced::of(state);
        if unannounced.history_rewrites != self.unannounced.history_rewrites {
            self.snapshot_due = Some(Some(ResyncReason::HistoryChanged));
        } else if unannounced.queued_prompts != self.unannounced.queued_prompts
            || (self.announced_turn.is_none()
                && unannounced.history_len != self.unannounced.history_len)
        {
            self.request_snapshot();
        }
        self.unannounced = unannounced;

        if self.lagging {
            // The events above kept their sequence numbers but are not sent:
            // the gap, and the resync snapshot after it, tell the gateway.
            if !room_for_resync {
                return Vec::new();
            }
            self.lagging = false;
            return vec![self.snapshot(state, Some(ResyncReason::Lagged))];
        }
        if let Some(resync) = self.snapshot_due {
            messages.push(self.snapshot(state, resync));
        } else {
            let info = project_session_info(state, &Self::context(&self.registration, 0, state));
            if info != self.info {
                self.info = info.clone();
                messages.push(OwnerMessage::SessionInfo(info));
            }
        }
        messages
    }

    /// The announced turn, if the turn that just ended in the event stream is
    /// that one. An end that belongs to a turn a snapshot has since replaced
    /// is not published.
    fn end_of_announced_turn_with_timing(
        &mut self,
        timing: Option<&super::protocol::RemoteTurnTiming>,
    ) -> Option<String> {
        if let Some(timing) = timing.filter(|timing| timing.outcome.is_some()) {
            if self.announced_turn.as_deref() != Some(timing.turn_id.as_str()) {
                return None;
            }
            self.open_turn = None;
            return self.announced_turn.take();
        }
        self.end_of_announced_turn()
    }

    fn end_of_announced_turn(&mut self) -> Option<String> {
        let ended = self.open_turn.take()?;
        if self.announced_turn.as_ref() == Some(&ended) {
            self.announced_turn.take()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppStatus, ChatMessage, PendingQuestion};
    use crate::remote::protocol::{RemoteSnapshot, SessionActivity};

    const SESSION: &str = "publisher-session";

    #[test]
    fn terminal_timing_is_retained_for_live_events_late_attach_and_history() {
        for outcome in [
            crate::app::TurnOutcome::Completed,
            crate::app::TurnOutcome::Cancelled,
            crate::app::TurnOutcome::Failed,
        ] {
            let mut state = shared_state();
            let (mut publisher, _) = register(&state);
            state.current_turn_started_at =
                Some(std::time::Instant::now() - std::time::Duration::from_secs(32));
            let id = state.begin_turn_identity();
            state.history.push(ChatMessage::new("user", "work"));
            publisher.observe(&AgentUiEvent::PromptStarted {
                prompt: "work".to_owned(),
                timing: state.active_turn_timing.clone(),
            });
            publisher.publish(&state, true);
            let timing = state.freeze_turn_timing(Some(outcome)).unwrap();
            state.enter_idle();
            state.end_turn_identity(&id);
            let event = if outcome == crate::app::TurnOutcome::Cancelled {
                AgentUiEvent::Cancelled {
                    completed_tool_ids: Vec::new(),
                    timing: Some(timing.clone()),
                }
            } else {
                AgentUiEvent::TurnFinished {
                    content: String::new(),
                    completed: outcome == crate::app::TurnOutcome::Completed,
                    timing: Some(timing.clone()),
                }
            };
            publisher.observe(&event);
            let sent = publisher.publish(&state, true);
            let terminal = events(&sent)
                .into_iter()
                .find_map(|(_, event)| match event {
                    RemoteEvent::TurnFinished { timing, .. }
                    | RemoteEvent::TurnCancelled { timing, .. } => timing,
                    _ => None,
                })
                .expect("live terminal timing");
            assert_eq!(terminal, project_turn_timing(&timing));
            let (_, late) = register(&state);
            assert_eq!(late.last_turn.as_ref(), Some(&terminal));
            assert!(late.turn.is_none());
            let history = super::super::projection::project_history_page(
                &state,
                &ProjectionLimits::default(),
                None,
                40,
            )
            .unwrap();
            assert_eq!(history.messages[0].turn.as_ref(), Some(&terminal));
            // A snapshot is an exact cut, even if delayed events contain IDs.
            let (mut reconnected, _) = register(&state);
            reconnected.observe(&AgentUiEvent::PromptStarted {
                prompt: "work".to_owned(),
                timing: Some(timing),
            });
            reconnected.observe(&event);
            assert!(events(&reconnected.publish(&state, true)).is_empty());
        }
    }

    #[test]
    fn live_thought_clock_updates_are_sequenced_and_unknown_timing_is_absent() {
        let mut state = shared_state();
        let (mut publisher, _) = register(&state);
        state.current_turn_started_at =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(12));
        state.begin_turn_identity();
        state.current_thought_time_ms = 2000;
        state.current_thought_started_at =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(2));
        publisher.observe(&AgentUiEvent::PromptStarted {
            prompt: "think".to_owned(),
            timing: state.active_turn_timing.clone(),
        });
        let sent = publisher.publish(&state, true);
        let projected = events(&sent);
        let (_, clock) = projected
            .iter()
            .find(|(_, event)| {
                matches!(
                    event,
                    RemoteEvent::TextDelta {
                        timing: Some(_),
                        ..
                    }
                )
            })
            .unwrap();
        let RemoteEvent::TextDelta {
            timing: Some(timing),
            thought_time_ms: Some(thought),
            ..
        } = clock
        else {
            panic!("clock update")
        };
        assert!((12000..13000).contains(&timing.elapsed_work_ms.unwrap()));
        assert!((4000..5000).contains(thought));
        assert!(projected.windows(2).all(|pair| pair[1].0 == pair[0].0 + 1));
        assert!(events(&publisher.publish(&state, true)).is_empty());
        state.current_thought_time_ms = 0;
        state.current_thought_started_at = None;
        state.clear_current_response();
        let snapshot = project_snapshot(
            &state,
            &SessionPublisher::context(publisher.registration(), publisher.sequence(), &state),
            &ProjectionLimits::default(),
        );
        assert_eq!(snapshot.turn.as_ref().unwrap().thought_time_ms, None);
    }

    #[test]
    fn a_legacy_continuation_with_unknown_work_is_not_finished_when_suspended() {
        let mut state = shared_state();
        let (mut publisher, _) = register(&state);
        state.current_turn_started_at = Some(std::time::Instant::now());
        state.begin_turn_identity();
        let mut legacy = state.active_turn_timing.clone().unwrap();
        legacy.started_at = None;
        state.resume_turn_identity(legacy.clone());
        publisher.observe(&AgentUiEvent::PromptStarted {
            prompt: "resume".to_owned(),
            timing: Some(legacy.clone()),
        });
        publisher.publish(&state, true);
        state.freeze_turn_timing(None);
        state.enter_idle();
        state.end_turn_identity(&legacy.turn_id);
        publisher.observe(&AgentUiEvent::TurnFinished {
            content: String::new(),
            completed: false,
            timing: Some(legacy),
        });
        assert!(
            !events(&publisher.publish(&state, true))
                .iter()
                .any(|(_, event)| matches!(event, RemoteEvent::TurnFinished { .. }))
        );
    }

    fn shared_state() -> AppState {
        let mut state = AppState::new();
        state.active_session_id = SESSION.to_owned();
        state
    }

    fn register(state: &AppState) -> (SessionPublisher, Box<RemoteSnapshot>) {
        let registration = SessionRegistration {
            session_id: SESSION.to_owned(),
            registration_epoch: 9,
        };
        let (publisher, message) = SessionPublisher::register(state, registration, false);
        let OwnerMessage::Register { snapshot, .. } = message else {
            panic!("registration opens with a register message");
        };
        (publisher, snapshot)
    }

    fn events(messages: &[OwnerMessage]) -> Vec<(u64, RemoteEvent)> {
        messages
            .iter()
            .filter_map(|message| match message {
                OwnerMessage::Event(frame) => Some((frame.sequence, frame.event.clone())),
                _ => None,
            })
            .collect()
    }

    fn snapshot_of(messages: &[OwnerMessage]) -> (&RemoteSnapshot, Option<ResyncReason>) {
        messages
            .iter()
            .find_map(|message| match message {
                OwnerMessage::Snapshot { snapshot, resync } => Some((snapshot.as_ref(), *resync)),
                _ => None,
            })
            .expect("a snapshot was published")
    }

    fn start_turn(state: &mut AppState, publisher: &mut SessionPublisher) -> String {
        let turn_id = state.begin_turn_identity();
        state.status = AppStatus::Streaming;
        publisher.observe(&AgentUiEvent::PromptStarted {
            prompt: "do it".to_owned(),
            timing: None,
        });
        turn_id
    }

    #[test]
    fn text_is_published_once_with_consecutive_sequences() {
        let mut state = shared_state();
        let (mut publisher, registered) = register(&state);
        assert_eq!(registered.sequence, 0);
        assert_eq!(registered.session.registration_epoch, 9);

        let turn_id = start_turn(&mut state, &mut publisher);
        state.append_current_response("Hel");
        // The engine's own delta for the same text is not published twice.
        publisher.observe(&AgentUiEvent::TextDelta {
            text: "Hel".to_owned(),
        });
        let first = events(&publisher.publish(&state, true));
        state.append_current_response("lo");
        let second = events(&publisher.publish(&state, true));

        assert_eq!(
            first,
            [
                (
                    1,
                    RemoteEvent::TurnStarted {
                        turn_id: Some(turn_id.clone()),
                        prompt: bound_text("do it", 64, None),
                        timing: None,
                    }
                ),
                (
                    2,
                    RemoteEvent::TextDelta {
                        text: "Hel".to_owned(),
                        timing: None,
                        thought_time_ms: None,
                    }
                ),
            ]
        );
        assert_eq!(
            second,
            [(
                3,
                RemoteEvent::TextDelta {
                    text: "lo".to_owned(),
                    timing: None,
                    thought_time_ms: None,
                }
            )]
        );
        assert!(events(&publisher.publish(&state, true)).is_empty());
    }

    #[test]
    fn a_snapshot_is_an_exact_cut_of_the_response_text() {
        let mut state = shared_state();
        let (mut publisher, _) = register(&state);
        start_turn(&mut state, &mut publisher);
        state.append_current_response("abc");
        publisher.publish(&state, true);

        // More text reaches the state, and a device attaches before the next
        // publication: the snapshot holds it and no delta repeats it.
        state.append_current_response("def");
        publisher.request_snapshot();
        let published = publisher.publish(&state, true);
        let (snapshot, resync) = snapshot_of(&published);
        let turn = snapshot.turn.as_ref().expect("the turn is running");
        let delivered: String = events(&published)
            .into_iter()
            .filter_map(|(_, event)| match event {
                RemoteEvent::TextDelta { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(resync, None);
        assert_eq!(delivered, "def");
        assert_eq!(turn.live_response.text, "abcdef");
        assert_eq!(snapshot.sequence, publisher.sequence());

        state.append_current_response("ghi");
        let after = events(&publisher.publish(&state, true));
        assert_eq!(
            after,
            [(
                snapshot.sequence + 1,
                RemoteEvent::TextDelta {
                    text: "ghi".to_owned(),
                    timing: None,
                    thought_time_ms: None,
                }
            )]
        );
    }

    #[test]
    fn turn_ends_name_the_turn_even_after_the_state_forgot_it() {
        let mut state = shared_state();
        let (mut publisher, _) = register(&state);
        let turn_id = start_turn(&mut state, &mut publisher);
        state.append_current_response("partial");
        publisher.publish(&state, true);

        // The turn ends: its identity and live text are gone from the state
        // before the terminal event is observed.
        state.end_turn_identity(&turn_id);
        state.clear_current_response();
        state.enter_idle();
        publisher.observe(&AgentUiEvent::TurnFinished {
            content: "partial answer".to_owned(),
            completed: true,
            timing: None,
        });
        let finished = events(&publisher.publish(&state, true));
        assert_eq!(
            finished
                .into_iter()
                .map(|(_, event)| event)
                .collect::<Vec<_>>(),
            [
                RemoteEvent::TextDelta {
                    text: " answer".to_owned(),
                    timing: None,
                    thought_time_ms: None,
                },
                RemoteEvent::TurnFinished {
                    turn_id: Some(turn_id),
                    timing: None,
                },
            ]
        );
    }

    #[test]
    fn a_turn_already_running_at_registration_is_announced_and_ended() {
        let mut state = shared_state();
        let turn_id = state.begin_turn_identity();
        state.status = AppStatus::Streaming;
        state.append_current_response("so far");
        let registration = SessionRegistration {
            session_id: SESSION.to_owned(),
            registration_epoch: 9,
        };
        let (mut publisher, message) = SessionPublisher::register(&state, registration, true);
        let OwnerMessage::Register { snapshot, .. } = message else {
            panic!("registration opens with a register message");
        };
        let turn = snapshot.turn.as_ref().expect("the running turn");
        assert_eq!(turn.turn_id, turn_id);
        assert_eq!(turn.live_response.text, "so far");

        state.append_current_response(", and more");
        state.end_turn_identity(&turn_id);
        publisher.observe(&AgentUiEvent::TurnFinished {
            content: "so far, and more.".to_owned(),
            completed: true,
            timing: None,
        });
        let ended: Vec<RemoteEvent> = events(&publisher.publish(&state, true))
            .into_iter()
            .map(|(_, event)| event)
            .collect();
        // The identity was retired before the text was read from the state:
        // the end of the response still arrives, once, before the turn ends.
        assert_eq!(
            ended,
            [
                RemoteEvent::TextDelta {
                    text: ", and more.".to_owned(),
                    timing: None,
                    thought_time_ms: None,
                },
                RemoteEvent::TurnFinished {
                    turn_id: Some(turn_id),
                    timing: None,
                },
            ]
        );
    }

    #[test]
    fn a_stale_turn_end_cannot_finish_the_turn_a_snapshot_announced() {
        let mut state = shared_state();
        let (mut publisher, _) = register(&state);
        let first = start_turn(&mut state, &mut publisher);
        publisher.publish(&state, true);

        // The first turn ends and a queued prompt starts before either
        // event is observed; a snapshot announces the second turn.
        state.end_turn_identity(&first);
        let second = state.begin_turn_identity();
        publisher.request_snapshot();
        let published = publisher.publish(&state, true);
        let (snapshot, _) = snapshot_of(&published);
        assert_eq!(snapshot.turn.as_ref().unwrap().turn_id, second);

        publisher.observe(&AgentUiEvent::TurnFinished {
            content: String::new(),
            completed: true,
            timing: None,
        });
        publisher.observe(&AgentUiEvent::PromptStarted {
            prompt: "next".to_owned(),
            timing: None,
        });
        assert!(events(&publisher.publish(&state, true)).is_empty());

        state.end_turn_identity(&second);
        state.enter_idle();
        publisher.observe(&AgentUiEvent::Cancelled {
            completed_tool_ids: Vec::new(),
            timing: None,
        });
        let ended = events(&publisher.publish(&state, true));
        assert_eq!(
            ended.last().map(|(_, event)| event),
            Some(&RemoteEvent::TurnCancelled {
                turn_id: Some(second),
                timing: None,
            })
        );
    }

    #[test]
    fn questions_are_announced_and_resolved_by_identity() {
        let mut state = shared_state();
        let (mut publisher, _) = register(&state);
        let first = PendingQuestion::new("Same?".to_owned(), vec!["Yes".to_owned()], false);
        let second = PendingQuestion::new("Same?".to_owned(), vec!["Yes".to_owned()], false);
        let (first_id, second_id) = (first.id.clone(), second.id.clone());
        state.begin_question_chain(vec![first, second]);

        let asked = events(&publisher.publish(&state, true));
        assert!(matches!(
            &asked[..],
            [(_, RemoteEvent::QuestionRequested { question })] if question.question_id == first_id
        ));
        // Nothing is repeated while the question stays pending.
        assert!(events(&publisher.publish(&state, true)).is_empty());

        // Identical text and options: only the identity shows the advance.
        state.advance_question_chain("Yes".to_owned());
        let advanced = events(&publisher.publish(&state, true));
        assert!(matches!(
            &advanced[..],
            [
                (_, RemoteEvent::QuestionResolved { question_id }),
                (_, RemoteEvent::QuestionRequested { question }),
            ] if *question_id == first_id && question.question_id == second_id
        ));

        state.clear_question_chain();
        let resolved = events(&publisher.publish(&state, true));
        assert!(matches!(
            &resolved[..],
            [(_, RemoteEvent::QuestionResolved { question_id })] if *question_id == second_id
        ));
    }

    #[test]
    fn a_full_queue_drops_events_then_resyncs_with_a_snapshot() {
        let mut state = shared_state();
        let (mut publisher, _) = register(&state);
        start_turn(&mut state, &mut publisher);
        state.append_current_response("one");
        let delivered = publisher.publish(&state, true);
        let last_delivered = events(&delivered).last().unwrap().0;

        publisher.mark_lagging();
        state.append_current_response(" two");
        assert!(publisher.publish(&state, false).is_empty());
        state.append_current_response(" three");
        assert!(publisher.publish(&state, false).is_empty());
        assert!(publisher.is_lagging());

        let recovered = publisher.publish(&state, true);
        let (snapshot, resync) = snapshot_of(&recovered);
        assert_eq!(recovered.len(), 1);
        assert_eq!(resync, Some(ResyncReason::Lagged));
        assert!(snapshot.sequence > last_delivered, "the gap is visible");
        assert_eq!(
            snapshot.turn.as_ref().unwrap().live_response.text,
            "one two three"
        );
        assert!(!publisher.is_lagging());

        state.append_current_response(" four");
        let after = events(&publisher.publish(&state, true));
        assert_eq!(
            after,
            [(
                snapshot.sequence + 1,
                RemoteEvent::TextDelta {
                    text: " four".to_owned(),
                    timing: None,
                    thought_time_ms: None,
                }
            )]
        );
    }

    #[test]
    fn changes_no_event_describes_are_published_as_snapshots() {
        let mut state = shared_state();
        let (mut publisher, _) = register(&state);

        // A background task result lands in the idle transcript.
        state.history.push(ChatMessage::new("system", "task done"));
        let published = publisher.publish(&state, true);
        let (snapshot, resync) = snapshot_of(&published);
        assert_eq!(resync, None);
        assert_eq!(snapshot.transcript.len(), 1);
        assert!(publisher.publish(&state, true).is_empty());

        // The terminal queues a prompt behind a running turn.
        state.status = AppStatus::Streaming;
        state.pending_queue.push("later".to_owned());
        let published = publisher.publish(&state, true);
        let (snapshot, _) = snapshot_of(&published);
        assert_eq!(snapshot.pending_prompts.len(), 1);
        assert_eq!(snapshot.session.activity, SessionActivity::Running);

        // Only the session-list row changes.
        state.pending_queue.clear();
        publisher.publish(&state, true);
        state.enter_idle();
        let published = publisher.publish(&state, true);
        assert!(matches!(
            &published[..],
            [OwnerMessage::SessionInfo(info)] if info.activity == SessionActivity::Idle
        ));
    }

    #[test]
    fn a_registration_only_owns_its_own_session() {
        let mut state = shared_state();
        let (publisher, _) = register(&state);
        assert!(publisher.owns(&state));
        state.active_session_id = "another-session".to_owned();
        assert!(!publisher.owns(&state));
    }
}
