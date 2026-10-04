use crate::app::{AppState, state::DraftSubmitMode};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SubmitOutcome {
    Empty,
    Steered,
    Queued,
}

pub(crate) fn submit_plain_prompt_with_mode(
    state: &mut AppState,
    text: String,
    mode: DraftSubmitMode,
) -> SubmitOutcome {
    state.draft_submit_mode = mode;
    submit_plain_prompt(state, text)
}

pub(crate) fn submit_plain_prompt(state: &mut AppState, text: String) -> SubmitOutcome {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return SubmitOutcome::Empty;
    }

    if state.can_accept_steer()
        && state.draft_submit_mode == DraftSubmitMode::Steer
        && state.queue_steer(text.clone())
    {
        state.input_buffer.clear();
        state.cursor_position = 0;
        state.draft_submit_mode = DraftSubmitMode::Steer;
        state.request_redraw();
        return SubmitOutcome::Steered;
    }

    begin_task_delegation(state, &text);
    state.pending_queue.push(text);
    state.input_buffer.clear();
    state.cursor_position = 0;
    state.draft_submit_mode = DraftSubmitMode::Steer;
    state.request_redraw();
    SubmitOutcome::Queued
}

/// Decide whether the task being submitted may use subagents, consuming the
/// one-shot `/delegate` arming. An explicit request in the prompt is its own
/// authorization: requiring `/delegate` as well left the model with no agent
/// tools and no way to learn why (#1710).
pub(crate) fn begin_task_delegation(state: &mut AppState, prompt: &str) {
    state.delegation_active =
        state.delegation_armed || state.delegation_sticky || prompt_requests_delegation(prompt);
    state.delegation_armed = false;
}

/// Words that may sit between a delegation verb and the agent noun, as in
/// "use 2 read-only sub agents" or "delegate this task to a subagent".
const DELEGATION_FILLER: &[&str] = &[
    "a",
    "an",
    "the",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "some",
    "several",
    "multiple",
    "few",
    "many",
    "more",
    "another",
    "new",
    "separate",
    "parallel",
    "background",
    "read",
    "only",
    "this",
    "that",
    "it",
    "task",
    "work",
    "to",
    "up",
    "off",
];

const DELEGATION_NEGATIONS: &[&str] = &["don", "dont", "not", "no", "never", "without", "avoid"];

/// Bounded match for an explicit request to delegate, e.g. "use 1 sub agent to
/// check the latest PRs". A bare mention ("fix the subagent picker") does not
/// count: a delegation verb must lead to the agent noun through filler only.
pub(crate) fn prompt_requests_delegation(prompt: &str) -> bool {
    let lowered = prompt.to_lowercase();
    let mut tokens: Vec<&str> = Vec::new();
    for token in lowered
        // `_` stays inside a token so the tool name `spawn_agent` is not a request.
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|token| !token.is_empty())
    {
        // "sub agent" and "sub-agent" are the same noun as "subagent".
        match (tokens.last().copied(), token) {
            (Some("sub"), "agent") => *tokens.last_mut().unwrap() = "subagent",
            (Some("sub"), "agents") => *tokens.last_mut().unwrap() = "subagents",
            _ => tokens.push(token),
        }
    }

    tokens.iter().enumerate().any(|(index, verb)| {
        // Plain "agent" is too common ("use agent mode") to follow a weak verb.
        let accepts_plain_agent = match *verb {
            "spawn" | "launch" | "delegate" => true,
            "use" | "using" | "with" | "via" | "start" | "run" | "have" | "let" | "ask" | "get"
            | "give" => false,
            _ => return false,
        };
        let negated = tokens[index.saturating_sub(3)..index]
            .iter()
            .any(|token| DELEGATION_NEGATIONS.contains(token));
        if negated {
            return false;
        }
        tokens[index + 1..]
            .iter()
            .take(6)
            .find(|token| {
                !DELEGATION_FILLER.contains(token) && !token.chars().all(|c| c.is_ascii_digit())
            })
            .is_some_and(|noun| match *noun {
                "subagent" | "subagents" => true,
                "agent" | "agents" => accepts_plain_agent,
                _ => false,
            })
    })
}
