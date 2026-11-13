//! `ask_user_question` (#3393): a multiple-choice question for an external
//! harness that has no question tool of its own (Pi, OpenCode, Gemini CLI, a
//! custom ACP agent; `harness::needs_question_tool`). The `rustcode` MCP
//! server lists it and forwards each call to the hub, and the hub asks it
//! through the thread's own permission channel as the same RustCode question
//! Codex's `request_user_input` becomes (`acp/questions.rs`). The app shows
//! its question card and the thread reads as waiting on the user until the
//! answer comes back, which the agent gets as JSON.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use agent_client_protocol::schema::v1::{RequestPermissionRequest, RequestPermissionResponse};
use serde_json::{Value, json};

use crate::controller::{QuestionPrompt, QuestionReply};

pub const TOOL_NAME: &str = "ask_user_question";

pub const MAX_QUESTIONS: usize = 4;
pub const MIN_OPTIONS: usize = 2;
pub const MAX_OPTIONS: usize = 4;
const MAX_HEADER_CHARS: usize = 40;
const MAX_QUESTION_CHARS: usize = 1_000;
const MAX_LABEL_CHARS: usize = 120;
const MAX_DESCRIPTION_CHARS: usize = 500;

/// What the user said when they dismissed a question instead of answering.
pub const DISMISSED: &str = "error: the user dismissed the question without answering. Continue on your own judgement, and say which choice you made.";

/// Asks the thread's client one question and gives its whole answer, or
/// `None` when nobody could be asked.
pub type Asker = Arc<
    dyn Fn(
            RequestPermissionRequest,
        ) -> Pin<Box<dyn Future<Output = Option<RequestPermissionResponse>> + Send>>
        + Send
        + Sync,
>;

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "questions": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_QUESTIONS,
                "items": {
                    "type": "object",
                    "properties": {
                        "header": {"type": "string", "maxLength": MAX_HEADER_CHARS, "description": "A short label for the question, a few words"},
                        "question": {"type": "string", "description": "The full question, ending with a question mark"},
                        "options": {
                            "type": "array",
                            "minItems": MIN_OPTIONS,
                            "maxItems": MAX_OPTIONS,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": {"type": "string", "description": "The choice, a few words"},
                                    "description": {"type": "string", "description": "What choosing it means"}
                                },
                                "required": ["label", "description"],
                                "additionalProperties": false
                            }
                        },
                        "multi_select": {"type": "boolean", "default": false, "description": "Let the user pick more than one option"}
                    },
                    "required": ["question", "options"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["questions"],
        "additionalProperties": false
    })
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn bounded(field: &str, value: &str, max: usize) -> Result<String, String> {
    if value.chars().count() > max {
        return Err(format!("{field} is longer than {max} characters"));
    }
    Ok(value.to_owned())
}

/// The questions a call asks, checked: 1 to 4, each with 2 to 4 distinct
/// options that have a label and a description.
pub fn validate(args: &Value) -> Result<Vec<QuestionPrompt>, String> {
    let questions = args
        .get("questions")
        .and_then(Value::as_array)
        .ok_or("questions must be an array")?;
    if questions.is_empty() || questions.len() > MAX_QUESTIONS {
        return Err(format!("ask 1 to {MAX_QUESTIONS} questions"));
    }
    questions
        .iter()
        .enumerate()
        .map(|(index, question)| {
            let number = index + 1;
            let prompt = text(question, "question")
                .ok_or_else(|| format!("question {number} has no question text"))?;
            let options = question
                .get("options")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("question {number} has no options array"))?;
            if options.len() < MIN_OPTIONS || options.len() > MAX_OPTIONS {
                return Err(format!(
                    "question {number} needs {MIN_OPTIONS} to {MAX_OPTIONS} options; the app adds a free-text answer itself"
                ));
            }
            let mut labels = Vec::with_capacity(options.len());
            let mut descriptions = Vec::with_capacity(options.len());
            for option in options {
                let label = text(option, "label")
                    .ok_or_else(|| format!("question {number} has an option without a label"))?;
                if labels.iter().any(|seen: &String| seen.eq_ignore_ascii_case(label)) {
                    return Err(format!("question {number} lists \"{label}\" twice"));
                }
                labels.push(bounded("an option label", label, MAX_LABEL_CHARS)?);
                descriptions.push(bounded(
                    "an option description",
                    text(option, "description").unwrap_or_default(),
                    MAX_DESCRIPTION_CHARS,
                )?);
            }
            let multiple = match question.get("multi_select") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(multiple)) => *multiple,
                Some(_) => return Err(format!("question {number}: multi_select must be a boolean")),
            };
            Ok(QuestionPrompt {
                header: bounded(
                    "a header",
                    text(question, "header").unwrap_or_default(),
                    MAX_HEADER_CHARS,
                )?,
                text: bounded("a question", prompt, MAX_QUESTION_CHARS)?,
                options: labels,
                descriptions,
                multiple,
            })
        })
        .collect()
}

/// One answer as the agent reads it: the options picked, or the user's own
/// words when they wrote their answer instead.
fn answer_json(prompt: &QuestionPrompt, reply: QuestionReply) -> Value {
    match reply {
        QuestionReply::Options(selected) => json!({
            "question": prompt.text,
            "selected": selected,
        }),
        QuestionReply::Custom(text) => json!({
            "question": prompt.text,
            "selected": [],
            "text": text,
        }),
    }
}

/// An answer that names an option the question did not offer, or several
/// options on a single-choice question, is no answer.
fn checked(prompt: &QuestionPrompt, reply: QuestionReply) -> Option<QuestionReply> {
    match &reply {
        QuestionReply::Options(selected) => (!selected.is_empty()
            && (prompt.multiple || selected.len() == 1)
            && selected.iter().all(|label| prompt.options.contains(label)))
        .then_some(reply),
        QuestionReply::Custom(text) => (!text.trim().is_empty()).then_some(reply),
    }
}

/// Ask `prompts` one after another through `asker` and answer with every
/// reply as JSON text, or with [`DISMISSED`] as soon as one is dismissed.
/// Cancelling stops at once; the hub then has nobody to answer.
pub async fn ask(
    asker: &Asker,
    prompts: &[QuestionPrompt],
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<String, String> {
    let call = format!("{:016x}", rand::random::<u64>());
    let mut answers = Vec::with_capacity(prompts.len());
    for (index, prompt) in prompts.iter().enumerate() {
        let request = crate::acp::questions::question_permission_request(
            "",
            &format!("{TOOL_NAME}:{call}:{index}"),
            prompt,
        );
        let response = tokio::select! {
            response = asker(request) => response,
            () = cancel.cancelled() => return Err("error: the question was cancelled".to_owned()),
        };
        let reply = response
            .and_then(|response| crate::acp::questions::question_reply(&response, prompt))
            .and_then(|reply| checked(prompt, reply))
            .ok_or_else(|| DISMISSED.to_owned())?;
        answers.push(answer_json(prompt, reply));
    }
    Ok(json!({ "answers": answers }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{RequestPermissionOutcome, SelectedPermissionOutcome};
    use std::sync::Mutex;

    fn option(label: &str) -> Value {
        json!({"label": label, "description": format!("{label} it is")})
    }

    fn one(options: Vec<Value>) -> Value {
        json!({"questions": [{"header": "Pick", "question": "Which?", "options": options}]})
    }

    #[test]
    fn a_valid_call_becomes_question_prompts() {
        let prompts = validate(&json!({"questions": [
            {"header": "Source", "question": "Where from?", "options": [option("A"), option("B")]},
            {"question": "Which checks?", "options": [option("Lint"), option("Test"), option("Build")], "multi_select": true},
        ]}))
        .unwrap();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0].header, "Source");
        assert_eq!(prompts[0].options, ["A", "B"]);
        assert_eq!(prompts[0].descriptions, ["A it is", "B it is"]);
        assert!(!prompts[0].multiple);
        assert!(prompts[1].multiple);
        assert_eq!(prompts[1].header, "");
    }

    #[test]
    fn validation_bounds_questions_and_options() {
        assert!(validate(&json!({})).is_err());
        assert!(validate(&json!({"questions": []})).is_err());
        let five = (0..5)
            .map(|_| json!({"question": "Q?", "options": [option("A"), option("B")]}))
            .collect::<Vec<_>>();
        assert!(validate(&json!({ "questions": five })).is_err());
        assert!(
            validate(&one(vec![option("A")]))
                .unwrap_err()
                .contains("2 to 4")
        );
        assert!(
            validate(&one(vec![
                option("A"),
                option("B"),
                option("C"),
                option("D"),
                option("E")
            ]))
            .is_err()
        );
        assert!(
            validate(&one(vec![option("A"), option("a")]))
                .unwrap_err()
                .contains("twice")
        );
        assert!(validate(&one(vec![option("A"), json!({"description": "no label"})])).is_err());
        assert!(
            validate(
                &json!({"questions": [{"question": " ", "options": [option("A"), option("B")]}]})
            )
            .is_err()
        );
        assert!(validate(&json!({"questions": [{"question": "Q?", "options": [option("A"), option("B")], "multi_select": "yes"}]})).is_err());
        assert!(
            validate(&one(vec![
                option("A"),
                option("B"),
                option("C"),
                option("D")
            ]))
            .is_ok()
        );
    }

    /// Answers each request with what `reply` makes of it, and keeps the requests.
    fn asker(
        reply: impl Fn(&RequestPermissionRequest) -> Option<RequestPermissionResponse>
        + Send
        + Sync
        + 'static,
    ) -> (Asker, Arc<Mutex<Vec<RequestPermissionRequest>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let asker: Asker = Arc::new(move |request| {
            let answer = reply(&request);
            kept.lock().unwrap().push(request);
            Box::pin(async move { answer })
        });
        (asker, seen)
    }

    fn selected(option_id: &str, meta: Option<Value>) -> RequestPermissionResponse {
        let response = RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(option_id.to_owned()),
        ));
        match meta {
            Some(Value::Object(meta)) => response.meta(meta),
            _ => response,
        }
    }

    #[tokio::test]
    async fn answers_come_back_as_json_for_every_question() {
        let prompts = validate(&json!({"questions": [
            {"header": "Source", "question": "Where from?", "options": [option("A"), option("B")]},
            {"question": "Which checks?", "options": [option("Lint"), option("Test")], "multi_select": true},
            {"question": "Anything else?", "options": [option("No"), option("Yes")]},
        ]}))
        .unwrap();
        let (asker, seen) = asker(|request| {
            let id = request.tool_call.tool_call_id.0.to_string();
            Some(if id.ends_with(":0") {
                selected("question_option_1", None)
            } else if id.ends_with(":1") {
                selected(
                    "question_option_0",
                    Some(json!({"rustcode": {"answer": {"options": ["Lint", "Test"]}}})),
                )
            } else {
                selected(
                    "question_option_0",
                    Some(json!({"rustcode": {"answer": {"text": "ship it"}}})),
                )
            })
        });
        let cancel = tokio_util::sync::CancellationToken::new();
        let answer = ask(&asker, &prompts, &cancel).await.unwrap();
        let answer: Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(
            answer,
            json!({"answers": [
                {"question": "Where from?", "selected": ["B"]},
                {"question": "Which checks?", "selected": ["Lint", "Test"]},
                {"question": "Anything else?", "selected": [], "text": "ship it"},
            ]})
        );
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        // The same request the question bridge sends, so the app shows its card.
        let meta = seen[0].meta.as_ref().unwrap();
        assert_eq!(meta["rustcode"]["kind"], "question");
        assert_eq!(
            seen[0].tool_call.fields.raw_input.as_ref().unwrap()["header"],
            "Source"
        );
    }

    #[tokio::test]
    async fn dismissing_or_a_bad_answer_ends_the_call_with_a_clear_error() {
        let prompts = validate(&one(vec![option("A"), option("B")])).unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        for reply in [
            None,
            Some(selected("question_cancel", None)),
            Some(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            )),
            // Two options on a single-choice question, and one it never offered.
            Some(selected(
                "question_option_0",
                Some(json!({"rustcode": {"answer": {"options": ["A", "B"]}}})),
            )),
            Some(selected(
                "question_option_0",
                Some(json!({"rustcode": {"answer": {"options": ["Z"]}}})),
            )),
        ] {
            let (asker, _) = asker(move |_| reply.clone());
            assert_eq!(ask(&asker, &prompts, &cancel).await.unwrap_err(), DISMISSED);
        }
    }

    #[tokio::test]
    async fn a_cancelled_call_stops_waiting() {
        let prompts = validate(&one(vec![option("A"), option("B")])).unwrap();
        let asker: Asker = Arc::new(|_| Box::pin(std::future::pending()));
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        assert!(
            ask(&asker, &prompts, &cancel)
                .await
                .unwrap_err()
                .contains("cancelled")
        );
    }
}
