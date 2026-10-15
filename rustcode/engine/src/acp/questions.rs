//! ACP v1 has no method for asking the user a question, so a pending
//! `ask_question` is offered to the client as a `session/request_permission`
//! whose options are the question's answers plus a cancel option. Without this
//! bridge a turn that asks a question waits until the prompt is cancelled.
//!
//! The request is ordinary ACP: any client can render it as a permission
//! choice. The `_meta.rustcode` object and the `raw_input` question payload let
//! a RustCode-aware client (the macOS app) render it as a question instead.
//! Such a client may answer with several options or free text by selecting
//! any answer option and adding `_meta.rustcode.answer`, either
//! `{"options": [labels]}` or `{"text": "..."}`.

use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use agent_client_protocol::{Client, ConnectionTo};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Version of the `_meta.rustcode` question payload. Bump it on any change a
/// client could observe.
pub(crate) const QUESTION_META_VERSION: u64 = 2;
const OPTION_PREFIX: &str = "question_option_";
const CANCEL_OPTION_ID: &str = "question_cancel";

pub(crate) fn question_permission_request(
    session_id: &str,
    question_id: &str,
    prompt: &crate::controller::QuestionPrompt,
) -> RequestPermissionRequest {
    let title = if prompt.header.trim().is_empty() {
        prompt.text.clone()
    } else {
        format!("{}: {}", prompt.header, prompt.text)
    };
    let tool_call = ToolCallUpdate::new(
        format!("question:{question_id}"),
        ToolCallUpdateFields::new()
            .title(title)
            .kind(ToolKind::Think)
            .raw_input(serde_json::json!({
                "header": prompt.header,
                "question": prompt.text,
                "options": prompt.options,
                "descriptions": prompt.descriptions,
                "multiple": prompt.multiple,
            })),
    );
    let mut options = prompt
        .options
        .iter()
        .enumerate()
        .map(|(index, label)| {
            PermissionOption::new(
                format!("{OPTION_PREFIX}{index}"),
                label.clone(),
                PermissionOptionKind::AllowOnce,
            )
        })
        .collect::<Vec<_>>();
    options.push(PermissionOption::new(
        CANCEL_OPTION_ID,
        "Cancel",
        PermissionOptionKind::RejectOnce,
    ));
    let mut meta = serde_json::Map::new();
    meta.insert(
        "rustcode".to_owned(),
        serde_json::json!({
            "kind": "question",
            "version": QUESTION_META_VERSION,
            "questionId": question_id,
            "answerKinds": ["options", "text"],
        }),
    );
    RequestPermissionRequest::new(session_id.to_owned(), tool_call, options).meta(meta)
}

/// The client's answer, or `None` when it cancelled, rejected or named an
/// option this question does not have. A `_meta.rustcode.answer` on a
/// selected outcome carries several options or free text and takes
/// precedence over the selected option; the engine validates it like any
/// other answer.
pub(crate) fn question_reply(
    response: &RequestPermissionResponse,
    prompt: &crate::controller::QuestionPrompt,
) -> Option<crate::controller::QuestionReply> {
    let RequestPermissionOutcome::Selected(selected) = &response.outcome else {
        return None;
    };
    if selected.option_id.0.as_ref() == CANCEL_OPTION_ID {
        return None;
    }
    if let Some(answer) = response
        .meta
        .as_ref()
        .and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("answer"))
    {
        if let Some(text) = answer.get("text").and_then(|text| text.as_str()) {
            return Some(crate::controller::QuestionReply::Custom(text.to_owned()));
        }
        if let Some(options) = answer.get("options").and_then(|options| options.as_array()) {
            return options
                .iter()
                .map(|option| option.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .map(crate::controller::QuestionReply::Options);
        }
        return None;
    }
    let index = selected
        .option_id
        .0
        .strip_prefix(OPTION_PREFIX)?
        .parse::<usize>()
        .ok()?;
    prompt
        .options
        .get(index)
        .cloned()
        .map(|label| crate::controller::QuestionReply::Options(vec![label]))
}

/// Ids of the questions already offered to the client during one prompt turn,
/// shared by the turn's event loop and the tasks that ask them, so a question
/// is never sent twice.
pub(crate) type AskedQuestions = Arc<std::sync::Mutex<std::collections::HashSet<String>>>;

/// The question the turn is waiting on, with its id and prompt read together
/// under one lock so a request can never pair one question's text with
/// another's id.
pub(crate) fn pending_question_prompt(
    state: &crate::app::AppState,
) -> Option<(String, crate::controller::QuestionPrompt)> {
    state.question_response.as_ref()?;
    let question = state.pending_question.as_ref()?;
    Some((
        question.id.clone(),
        crate::controller::QuestionPrompt {
            header: question.header.clone(),
            text: question.question.clone(),
            options: question.options.clone(),
            descriptions: question.descriptions.clone(),
            multiple: question.is_multi_select,
        },
    ))
}

/// Ask the client the pending question on its own task so the turn's event
/// loop keeps running. Each answer is bound to its question id: if the
/// question was already answered, replaced or cancelled, the late reply is
/// dropped. A chain advances without a new UI event, so the task keeps asking
/// until no unasked question is pending.
pub(crate) fn spawn_question_requests(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    state: Arc<Mutex<crate::app::AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    asked: AskedQuestions,
) -> Result<(), agent_client_protocol::Error> {
    let request_connection = connection.clone();
    let session_id = session_id.to_owned();
    connection.spawn(async move {
        loop {
            let Some((question_id, prompt)) = pending_question_prompt(&*state.lock().await) else {
                break;
            };
            if !asked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(question_id.clone())
            {
                break;
            }
            let request = question_permission_request(&session_id, &question_id, &prompt);
            let answer = request_connection
                .send_request(request)
                .block_task()
                .await
                .ok()
                .and_then(|response| question_reply(&response, &prompt));
            let answered = match answer {
                Some(reply) => {
                    crate::controller::apply_question_answer_for_question(
                        &state,
                        &question_id,
                        reply,
                    )
                    .await
                }
                None => Err(crate::controller::QuestionRejection::Invalid(
                    "cancelled".to_owned(),
                )),
            };
            match answered {
                Ok(()) => {}
                Err(crate::controller::QuestionRejection::Stale) => break,
                Err(crate::controller::QuestionRejection::Invalid(_)) => {
                    // Same semantics as Escape in the terminal: cancelling the
                    // question cancels the turn, and so does an answer the
                    // question cannot take, which would otherwise leave the
                    // turn waiting. The clone shares the turn's cancellation
                    // state; only the local handle is replaced.
                    let mut token = cancel_token;
                    let _ = crate::controller::cancel_question_for_question(
                        &state,
                        &mut token,
                        &question_id,
                    )
                    .await;
                    break;
                }
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::SelectedPermissionOutcome;

    fn prompt() -> crate::controller::QuestionPrompt {
        crate::controller::QuestionPrompt {
            header: "Plan".to_owned(),
            text: "Which approach?".to_owned(),
            options: vec!["Small".to_owned(), "Large".to_owned()],
            descriptions: vec!["Minimal diff".to_owned()],
            multiple: false,
        }
    }

    #[test]
    fn question_request_lists_answers_then_cancel_and_tags_rustcode_meta() {
        let request = question_permission_request("session-1", "q-7", &prompt());
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["sessionId"], "session-1");
        assert_eq!(value["toolCall"]["toolCallId"], "question:q-7");
        assert_eq!(value["toolCall"]["title"], "Plan: Which approach?");
        assert_eq!(value["toolCall"]["rawInput"]["question"], "Which approach?");
        assert_eq!(value["toolCall"]["rawInput"]["multiple"], false);
        let options = value["options"].as_array().unwrap();
        let ids = options
            .iter()
            .map(|option| option["optionId"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            ["question_option_0", "question_option_1", "question_cancel"]
        );
        assert_eq!(options[2]["kind"], "reject_once");
        assert_eq!(value["_meta"]["rustcode"]["kind"], "question");
        assert_eq!(value["_meta"]["rustcode"]["version"], QUESTION_META_VERSION);
        assert_eq!(value["_meta"]["rustcode"]["questionId"], "q-7");
        assert_eq!(
            value["_meta"]["rustcode"]["answerKinds"],
            serde_json::json!(["options", "text"])
        );
    }

    #[test]
    fn pending_question_prompt_reads_id_and_text_together_only_while_waiting() {
        let mut state = crate::app::AppState::new();
        let mut question = crate::app::PendingQuestion::new(
            "Which approach?".to_owned(),
            vec!["Small".to_owned(), "Large".to_owned()],
            true,
        );
        question.id = "q-1".to_owned();
        state.pending_question = Some(question);
        assert!(pending_question_prompt(&state).is_none());

        let (sender, _receiver) = tokio::sync::oneshot::channel();
        state.question_response = Some(sender);
        let (id, prompt) = pending_question_prompt(&state).unwrap();
        assert_eq!(id, "q-1");
        assert_eq!(prompt.text, "Which approach?");
        assert_eq!(prompt.options, ["Small", "Large"]);
        assert!(prompt.multiple);
    }

    #[test]
    fn question_reply_maps_option_ids_and_rejects_cancel_or_unknown() {
        use crate::controller::QuestionReply;
        let prompt = prompt();
        let select = |id: &str| {
            RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                SelectedPermissionOutcome::new(id.to_owned()),
            ))
        };
        assert_eq!(
            question_reply(&select("question_option_1"), &prompt),
            Some(QuestionReply::Options(vec!["Large".to_owned()]))
        );
        assert_eq!(question_reply(&select("question_cancel"), &prompt), None);
        assert_eq!(question_reply(&select("question_option_9"), &prompt), None);
        assert_eq!(question_reply(&select("allow_once"), &prompt), None);
        assert_eq!(
            question_reply(
                &RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled),
                &prompt
            ),
            None
        );
    }

    #[test]
    fn question_reply_reads_several_options_or_text_from_rustcode_meta() {
        use crate::controller::QuestionReply;
        let prompt = prompt();
        let with_answer = |id: &str, answer: serde_json::Value| {
            let mut meta = serde_json::Map::new();
            meta.insert(
                "rustcode".to_owned(),
                serde_json::json!({ "answer": answer }),
            );
            RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                SelectedPermissionOutcome::new(id.to_owned()),
            ))
            .meta(meta)
        };
        assert_eq!(
            question_reply(
                &with_answer(
                    "question_option_0",
                    serde_json::json!({ "options": ["Small", "Large"] })
                ),
                &prompt
            ),
            Some(QuestionReply::Options(vec![
                "Small".to_owned(),
                "Large".to_owned()
            ]))
        );
        assert_eq!(
            question_reply(
                &with_answer(
                    "question_option_0",
                    serde_json::json!({ "text": "Neither" })
                ),
                &prompt
            ),
            Some(QuestionReply::Custom("Neither".to_owned()))
        );
        assert_eq!(
            question_reply(
                &with_answer("question_cancel", serde_json::json!({ "text": "Neither" })),
                &prompt
            ),
            None
        );
        assert_eq!(
            question_reply(
                &with_answer("question_option_0", serde_json::json!({ "options": [1] })),
                &prompt
            ),
            None
        );
    }
}
