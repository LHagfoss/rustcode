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
//!
//! An answer may also carry files (`"attachments"` in `answerKinds`):
//! `{"text"?, "options"?, "attachments": [...]}`, each file `{name, path}`,
//! `{name, text}` (its content) or `{name, mimeType, data}` (a base64 image).
//! The engine saves each image where the thread runs, the same store a
//! prompt's images go to, and the agent reads the answer as text that ends
//! with the files' paths and the content of any sent inline, so every harness
//! gets them whether or not it reads images.

use super::clients::SessionClients;
use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
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
            "answerKinds": ["options", "text", "attachments"],
        }),
    );
    RequestPermissionRequest::new(session_id.to_owned(), tool_call, options).meta(meta)
}

/// The same request, numbered as question `index + 1` of `count` asked one
/// after another, so the client can say how far along the set the user is.
/// A set of one stays unnumbered.
pub(crate) fn numbered(
    mut request: RequestPermissionRequest,
    index: usize,
    count: usize,
) -> RequestPermissionRequest {
    if count > 1
        && let Some(rustcode) = request
            .meta
            .as_mut()
            .and_then(|meta| meta.get_mut("rustcode"))
            .and_then(serde_json::Value::as_object_mut)
    {
        rustcode.insert("step".to_owned(), serde_json::json!(index + 1));
        rustcode.insert("steps".to_owned(), serde_json::json!(count));
    }
    request
}

/// Most files one answer may carry, as many as a set of answers takes elsewhere.
const MAX_ANSWER_FILES: usize = 100;
/// Largest base64 image accepted before decoding: 20 MiB of image bytes.
const MAX_ENCODED_IMAGE_LEN: usize = (20usize * 1024 * 1024).div_ceil(3) * 4;

/// An answer with files: the options picked and the words written, then the
/// path of every file (an image saved here first) and the content of each
/// file sent inline. `None` when it names an option the question does not
/// have, picks several on a single-choice question, or carries too many files.
fn answer_with_files(
    answer: &serde_json::Value,
    files: &[serde_json::Value],
    prompt: &crate::controller::QuestionPrompt,
    save_image: &mut dyn FnMut(&[u8], &str) -> std::io::Result<std::path::PathBuf>,
) -> Option<crate::controller::QuestionReply> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    if files.len() > MAX_ANSWER_FILES {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(options) = answer.get("options").and_then(|options| options.as_array()) {
        let options = options
            .iter()
            .map(|option| option.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()?;
        let known = options.iter().all(|option| prompt.options.contains(option));
        if !known || (options.len() > 1 && !prompt.multiple) {
            return None;
        }
        if !options.is_empty() {
            parts.push(options.join(", "));
        }
    }
    if let Some(text) = answer.get("text").and_then(|text| text.as_str())
        && !text.trim().is_empty()
    {
        parts.push(text.trim().to_owned());
    }
    let mut paths = Vec::new();
    let mut inline = Vec::new();
    for file in files {
        let name = file
            .get("name")
            .and_then(|name| name.as_str())
            .unwrap_or("file");
        if let Some(path) = file.get("path").and_then(|path| path.as_str()) {
            paths.push(format!("- `{path}`"));
        } else if let Some(text) = file.get("text").and_then(|text| text.as_str()) {
            inline.push(format!("Attached file `{name}`:\n```\n{text}\n```"));
        } else if let Some(data) = file.get("data").and_then(|data| data.as_str()) {
            let mime = file
                .get("mimeType")
                .and_then(|mime| mime.as_str())
                .unwrap_or("");
            let saved = image_extension(mime)
                .ok_or_else(|| format!("{mime} is not PNG, JPEG, GIF or WebP"))
                .and_then(|extension| {
                    if data.len() > MAX_ENCODED_IMAGE_LEN {
                        return Err("it is over 20 MiB".to_owned());
                    }
                    let bytes = STANDARD
                        .decode(data.trim())
                        .map_err(|error| format!("it is not valid base64: {error}"))?;
                    save_image(&bytes, extension).map_err(|error| error.to_string())
                });
            paths.push(match saved {
                Ok(path) => format!("- `{}` ({name})", path.display()),
                Err(why) => format!("- {name} (could not be saved: {why})"),
            });
        }
    }
    if !paths.is_empty() {
        parts.push(format!("Attached files:\n{}", paths.join("\n")));
    }
    parts.extend(inline);
    let text = parts.join("\n\n");
    (!text.is_empty()).then_some(crate::controller::QuestionReply::Custom(text))
}

fn image_extension(mime: &str) -> Option<&'static str> {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
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
    question_reply_with(response, prompt, &mut |bytes, extension| {
        crate::controller::save_image_attachment(bytes, extension)
    })
}

fn question_reply_with(
    response: &RequestPermissionResponse,
    prompt: &crate::controller::QuestionPrompt,
    save_image: &mut dyn FnMut(&[u8], &str) -> std::io::Result<std::path::PathBuf>,
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
        if let Some(files) = answer
            .get("attachments")
            .and_then(|files| files.as_array())
            .filter(|files| !files.is_empty())
        {
            return answer_with_files(answer, files, prompt, save_image);
        }
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
/// until no unasked question is pending. The task belongs to the engine, so
/// it goes on to whichever client has the session.
pub(crate) fn spawn_question_requests(
    clients: &SessionClients,
    session_id: &str,
    state: Arc<Mutex<crate::app::AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    asked: AskedQuestions,
) -> Result<(), agent_client_protocol::Error> {
    let clients = clients.clone();
    let session_id = session_id.to_owned();
    tokio::spawn(async move {
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
            let _waiting = crate::thread_status::waiting(&session_id);
            let answer = clients
                .request(None, request)
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
    });
    Ok(())
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
            serde_json::json!(["options", "text", "attachments"])
        );
        assert!(value["_meta"]["rustcode"].get("step").is_none());
    }

    #[test]
    fn a_question_in_a_set_is_numbered_and_a_lone_one_is_not() {
        let request = numbered(question_permission_request("s", "q", &prompt()), 1, 3);
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["_meta"]["rustcode"]["step"], 2);
        assert_eq!(value["_meta"]["rustcode"]["steps"], 3);
        let lone = numbered(question_permission_request("s", "q", &prompt()), 0, 1);
        let value = serde_json::to_value(&lone).unwrap();
        assert!(value["_meta"]["rustcode"].get("step").is_none());
    }

    fn answered(answer: serde_json::Value) -> RequestPermissionResponse {
        let mut meta = serde_json::Map::new();
        meta.insert(
            "rustcode".to_owned(),
            serde_json::json!({ "answer": answer }),
        );
        RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new("question_option_0".to_owned()),
        ))
        .meta(meta)
    }

    #[test]
    fn files_in_an_answer_reach_the_agent_as_paths_and_inline_content() {
        use crate::controller::QuestionReply;
        let mut saved = Vec::new();
        let mut save = |bytes: &[u8], extension: &str| {
            saved.push((bytes.to_vec(), extension.to_owned()));
            Ok(std::path::PathBuf::from(format!("/store/shot.{extension}")))
        };
        let reply = question_reply_with(
            &answered(serde_json::json!({
                "options": ["Large"],
                "attachments": [
                    {"name": "shot.png", "mimeType": "image/png", "data": "aGk="},
                    {"name": "log.txt", "path": "/repo/log.txt"},
                    {"name": "notes.md", "text": "# Notes"},
                ],
            })),
            &prompt(),
            &mut save,
        );
        assert_eq!(
            reply,
            Some(QuestionReply::Custom(
                "Large\n\nAttached files:\n- `/store/shot.png` (shot.png)\n- `/repo/log.txt`\n\nAttached file `notes.md`:\n```\n# Notes\n```".to_owned()
            ))
        );
        assert_eq!(saved, [(b"hi".to_vec(), "png".to_owned())]);
    }

    #[test]
    fn a_file_alone_or_with_text_is_an_answer_and_a_bad_image_is_named() {
        use crate::controller::QuestionReply;
        let mut save = |_: &[u8], _: &str| -> std::io::Result<std::path::PathBuf> {
            Err(std::io::Error::other("disk full"))
        };
        assert_eq!(
            question_reply_with(
                &answered(serde_json::json!({
                    "attachments": [{"name": "a.png", "mimeType": "image/png", "data": "aGk="}],
                })),
                &prompt(),
                &mut save,
            ),
            Some(QuestionReply::Custom(
                "Attached files:\n- a.png (could not be saved: disk full)".to_owned()
            ))
        );
        assert_eq!(
            question_reply_with(
                &answered(serde_json::json!({
                    "text": " Like this ",
                    "attachments": [{"name": "a.txt", "path": "/a.txt"}],
                })),
                &prompt(),
                &mut save,
            ),
            Some(QuestionReply::Custom(
                "Like this\n\nAttached files:\n- `/a.txt`".to_owned()
            ))
        );
    }

    #[test]
    fn files_do_not_make_an_invalid_choice_an_answer() {
        let mut save = |_: &[u8], _: &str| Ok(std::path::PathBuf::from("/x.png"));
        let file = serde_json::json!([{"name": "a.txt", "path": "/a.txt"}]);
        for answer in [
            serde_json::json!({"options": ["Huge"], "attachments": file}),
            serde_json::json!({"options": ["Small", "Large"], "attachments": file}),
        ] {
            assert_eq!(
                question_reply_with(&answered(answer), &prompt(), &mut save),
                None
            );
        }
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
