//! MCP uses RustCode's thread terminology. The hub and ACP clients keep the
//! existing orchestration names so a newer MCP server can talk to older apps.

use serde_json::Value;

use super::hub::CallAnswer;

const TOOLS: &[(&str, &str)] = &[
    (super::LIST_SESSIONS, "list_threads"),
    (super::SEARCH_SESSIONS, "search_threads"),
    (super::READ_SESSION, "read_thread"),
    (super::START_SESSION, "start_thread"),
    (super::MESSAGE_SESSION, "message_thread"),
    (super::WAIT_SESSION, "wait_thread"),
    (super::UPDATE_SESSION, "update_thread"),
    (super::CONFIGURE_SESSION, "configure_thread"),
    (super::TRANSFER_SESSION, "transfer_thread"),
    (super::INTERRUPT_SESSION, "interrupt_thread"),
];
const ID_FIELDS: &[(&str, &str)] = &[("session_id", "thread_id"), ("session_ids", "thread_ids")];

pub(super) fn public_tool(name: &str) -> &str {
    TOOLS
        .iter()
        .find_map(|&(internal, public)| (name == internal).then_some(public))
        .unwrap_or(name)
}

pub(super) fn internal_tool(name: &str) -> &str {
    TOOLS
        .iter()
        .find_map(|&(internal, public)| (name == public).then_some(internal))
        .unwrap_or(name)
}

/// Use only for RustCode's own instructions, schema descriptions and errors.
pub(super) fn public_text(text: &str) -> String {
    text.split_inclusive(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .flat_map(|part| {
            let word = part.trim_end_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_');
            let public = match word {
                "session_id" => "thread_id",
                "session_ids" => "thread_ids",
                _ => public_tool(word),
            };
            [public, &part[word.len()..]]
        })
        .collect::<String>()
        .replace("<session>", "<thread_id>")
        .replace("session id", "thread id")
}

pub(super) fn public_entry(mut entry: Value) -> Value {
    if let Some(name) = entry["name"].as_str() {
        entry["name"] = Value::from(public_tool(name));
    }
    if let Some(description) = entry["description"].as_str() {
        entry["description"] = Value::from(public_text(description));
    }
    public_schema(&mut entry["inputSchema"]);
    entry
}

fn public_schema(schema: &mut Value) {
    match schema {
        Value::Object(fields) => {
            for &(internal, public) in ID_FIELDS {
                if let Some(field) = fields.remove(internal) {
                    fields.insert(public.into(), field);
                }
            }
            if let Some(required) = fields.get_mut("required").and_then(Value::as_array_mut) {
                for field in required {
                    if let Some(name) = field.as_str() {
                        *field = Value::from(public_text(name));
                    }
                }
            }
            if let Some(description) = fields.get_mut("description")
                && let Some(text) = description.as_str()
            {
                *description = Value::from(public_text(text));
            }
            for field in fields.values_mut() {
                public_schema(field);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(public_schema),
        _ => {}
    }
}

#[derive(Clone, Copy)]
pub(super) enum ReplyNames {
    Legacy,
    Thread,
    /// A tool whose name stayed the same and whose arguments cannot tell the
    /// callers apart. Keep the old result fields usable for these calls.
    Both,
}

pub(super) fn call_args(name: &str, args: &mut Value) -> Result<ReplyNames, CallAnswer> {
    let tool = internal_tool(name);
    // Loops already use thread_id from the MCP schema through to the app.
    if tool == super::CREATE_LOOP {
        return Ok(ReplyNames::Thread);
    }
    if !super::is_tool(tool) && tool != crate::workflow::STATUS_TOOL_NAME {
        return Ok(ReplyNames::Legacy);
    }
    let mut names = if tool != name {
        ReplyNames::Thread
    } else if public_tool(tool) != tool {
        ReplyNames::Legacy
    } else {
        ReplyNames::Both
    };
    if let Some(fields) = args.as_object_mut() {
        for &(internal, public) in ID_FIELDS {
            if let Some(value) = fields.remove(public) {
                if fields.get(internal).is_some_and(|old| *old != value) {
                    return Err(CallAnswer::failure(format!(
                        "error: conflicting values for {public} and {internal}"
                    )));
                }
                fields.insert(internal.into(), value);
                names = ReplyNames::Thread;
            } else if fields.contains_key(internal) && matches!(names, ReplyNames::Both) {
                names = ReplyNames::Legacy;
            }
        }
    }
    Ok(names)
}

impl ReplyNames {
    pub(super) fn answer(self, tool: &str, mut answer: CallAnswer) -> CallAnswer {
        // SQL columns and workflow results belong to the caller, and activity
        // metadata is a record of what a harness wrote.
        if matches!(self, Self::Legacy) || tool == super::QUERY_DATA {
            return answer;
        }
        if let Ok(mut body) = serde_json::from_str::<Value>(&answer.content) {
            if let Some(fields) = body.as_object_mut() {
                let diagnostic = if !answer.success || fields.contains_key("error") {
                    fields.get_mut("message")
                } else if tool == super::WAIT_SESSION {
                    fields.get_mut("note")
                } else {
                    None
                };
                if let Some(field) = diagnostic
                    && let Some(text) = field.as_str()
                {
                    *field = Value::from(public_text(text));
                }
            }
            self.result_fields(&mut body);
            answer.content = body.to_string();
        } else if !answer.success {
            answer.content = public_text(&answer.content);
        }
        answer
    }

    fn result_fields(self, value: &mut Value) {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields.iter_mut() {
                    if !matches!(key.as_str(), "result" | "data" | "events" | "metadata") {
                        self.result_fields(value);
                    }
                }
                for &(internal, public) in ID_FIELDS.iter().chain(&[("sessions", "threads")]) {
                    let field = if matches!(self, Self::Thread) {
                        fields.remove(internal)
                    } else {
                        fields.get(internal).cloned()
                    };
                    if let Some(field) = field {
                        fields.entry(public).or_insert(field);
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|item| self.result_fields(item)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shared_tool_replies_follow_the_callers_id_spelling() {
        for (args, expected) in [
            (json!({}), json!({"session_id": "t", "thread_id": "t"})),
            (json!({"session_id": "t"}), json!({"session_id": "t"})),
            (json!({"thread_id": "t"}), json!({"thread_id": "t"})),
        ] {
            let mut args = args;
            let tool = super::super::THREAD_STATUS;
            let names = call_args(tool, &mut args).unwrap();
            assert!(args.get("thread_id").is_none());
            let answer = names.answer(
                tool,
                CallAnswer {
                    success: true,
                    content: json!({"session_id": "t"}).to_string(),
                },
            );
            assert_eq!(
                serde_json::from_str::<Value>(&answer.content).unwrap(),
                expected
            );
        }
        assert!(
            call_args(
                "message_thread",
                &mut json!({"thread_id": "one", "session_id": "other"})
            )
            .is_err()
        );
    }

    #[test]
    fn create_loop_keeps_its_existing_thread_id() {
        let mut args = json!({"thread_id": "existing"});
        call_args(super::super::CREATE_LOOP, &mut args).unwrap();
        assert_eq!(args, json!({"thread_id": "existing"}));
    }

    #[test]
    fn thread_fields_do_not_rewrite_workflow_results_or_transcript_text() {
        let answer = ReplyNames::Thread.answer(
            crate::workflow::STATUS_TOOL_NAME,
            CallAnswer {
                success: true,
                content: json!({
                    "workers": [{"session_id": "worker"}],
                    "result": {"session_id": "script data"},
                    "text": "Paste `read_session`."
                })
                .to_string(),
            },
        );
        let body: Value = serde_json::from_str(&answer.content).unwrap();
        assert_eq!(
            body,
            json!({
                "workers": [{"thread_id": "worker"}],
                "result": {"session_id": "script data"},
                "text": "Paste `read_session`."
            })
        );
    }
}
