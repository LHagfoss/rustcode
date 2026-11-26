//! `checkpoint(question, options)`: a workflow run stops and asks the user on
//! the thread that started it, through the same question card
//! `ask_user_question` shows, and goes on with the option picked. Each thread
//! that can ask registers how (`register`): a built-in thread when it opens,
//! a harness thread with its question channel. An answered checkpoint is a
//! recorded call, so a resumed run replays the answer without asking again.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::orchestration::ask_user::{self, Asker};

/// The `harness` a checkpoint's call record names, so a status and the app
/// tell it from a worker: a running one is waiting on the user.
pub(crate) const KIND: &str = "checkpoint";

/// The most threads whose asker is kept; the oldest registration goes first.
const MAX_ASKERS: usize = 512;

static ASKERS: LazyLock<Mutex<Vec<(String, Asker)>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Let `session_id`'s user be asked through `asker` from now on.
pub(crate) fn register(session_id: &str, asker: Asker) {
    let mut askers = ASKERS.lock().unwrap_or_else(|error| error.into_inner());
    askers.retain(|(id, _)| id != session_id);
    if askers.len() >= MAX_ASKERS {
        askers.remove(0);
    }
    askers.push((session_id.to_owned(), asker));
}

fn asker(session_id: &str) -> Option<Asker> {
    ASKERS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .find(|(id, _)| id == session_id)
        .map(|(_, asker)| asker.clone())
}

/// The options a `checkpoint()` call offers, from its recorded options
/// (`{"checkpoint": [..]}`); `None` for an `agent()` call.
pub(super) fn options_of(options: &Value) -> Option<Result<Vec<String>, String>> {
    let offered = options.get("checkpoint")?;
    let labels = offered
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(
        if (2..=4).contains(&labels.len()) && offered.as_array().map(Vec::len) == Some(labels.len())
        {
            Ok(labels)
        } else {
            Err(
                "checkpoint(question, options): options must be 2 to 4 non-empty strings"
                    .to_owned(),
            )
        },
    )
}

/// Ask `thread`'s user `question` with `options`; the option picked.
pub(super) async fn ask(
    thread: &str,
    question: &str,
    options: &[String],
    cancel: &CancellationToken,
) -> Result<String, String> {
    let Some(asker) = asker(thread) else {
        return Err(format!(
            "checkpoint: nobody can be asked on thread {thread} now; open it in the app and retry the run"
        ));
    };
    let prompts = ask_user::validate(&json!({
        "questions": [{
            "header": "Workflow",
            "question": question,
            "options": options
                .iter()
                .map(|label| json!({"label": label, "description": label}))
                .collect::<Vec<_>>(),
        }]
    }))?;
    let answer = ask_user::ask(&asker, &prompts, cancel).await?;
    chosen(&answer).ok_or_else(|| "checkpoint: the answer named no option".to_owned())
}

/// The option an `ask_user::ask` answer picked.
fn chosen(answer: &str) -> Option<String> {
    let value: Value = serde_json::from_str(answer).ok()?;
    let first = value.get("answers")?.get(0)?;
    first
        .get("selected")
        .and_then(|selected| selected.get(0))
        .and_then(Value::as_str)
        .or_else(|| first.get("text").and_then(Value::as_str))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_need_two_to_four_labels() {
        assert_eq!(
            options_of(&json!({"checkpoint": ["Stop", "Continue"]})),
            Some(Ok(vec!["Stop".to_owned(), "Continue".to_owned()]))
        );
        assert!(
            options_of(&json!({"checkpoint": ["Only"]}))
                .unwrap()
                .is_err()
        );
        assert!(
            options_of(&json!({"checkpoint": ["A", ""]}))
                .unwrap()
                .is_err()
        );
        assert!(options_of(&json!({"checkpoint": "A"})).unwrap().is_err());
        assert!(options_of(&json!({"harness": "codex"})).is_none());
    }

    #[test]
    fn the_answer_names_the_option_picked() {
        let answer = json!({"answers": [{"question": "Go?", "selected": ["Continue"]}]});
        assert_eq!(chosen(&answer.to_string()).as_deref(), Some("Continue"));
        assert_eq!(chosen("{}"), None);
    }

    #[tokio::test]
    async fn a_thread_nobody_can_ask_is_refused() {
        let refused = ask(
            "no-such-thread",
            "Go?",
            &["Yes".to_owned(), "No".to_owned()],
            &CancellationToken::new(),
        )
        .await;
        assert!(refused.unwrap_err().contains("nobody can be asked"));
    }
}
