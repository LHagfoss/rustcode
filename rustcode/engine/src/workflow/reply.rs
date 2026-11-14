//! A worker's reply as `agent()`'s result. Without a schema it is the reply's
//! text. With one, the worker is asked for JSON and the reply is read as JSON
//! and checked against the schema, so a script gets a value it can use or an
//! error it can catch, never prose it has to guess at.

use serde_json::Value;

/// What a worker is told when `agent()` was given a schema.
pub(super) fn ask_for_json(prompt: &str, schema: &Value) -> String {
    let schema = serde_json::to_string_pretty(schema).unwrap_or_default();
    format!(
        "{prompt}\n\nWhen you are done, reply with one JSON value that matches this JSON Schema, and nothing else: no prose before or after it, and no code fence.\n\n{schema}"
    )
}

/// The reply as the value `agent()` returns.
pub(super) fn result(reply: &str, schema: Option<&Value>) -> Result<Value, String> {
    let Some(schema) = schema else {
        return Ok(Value::String(reply.to_owned()));
    };
    let mut first_problem = None;
    for candidate in candidates(reply) {
        let Ok(value) = serde_json::from_str::<Value>(&candidate) else {
            continue;
        };
        match check(&value, schema, "$") {
            Ok(()) => return Ok(value),
            // The first candidate is the reply itself; later ones are cut-outs of it.
            Err(problem) => {
                first_problem.get_or_insert(problem);
            }
        }
    }
    Err(match first_problem {
        Some(problem) => format!("the worker's JSON does not match the schema: {problem}"),
        None => "the worker's reply holds no JSON value".to_owned(),
    })
}

/// The pieces of a reply that may be its JSON: all of it, each fenced block,
/// then the span from the first bracket to the last matching one.
fn candidates(reply: &str) -> Vec<String> {
    let trimmed = reply.trim();
    let mut found = vec![trimmed.to_owned()];
    let mut rest = trimmed;
    while let Some(open) = rest.find("```") {
        let after = &rest[open + 3..];
        // The fence's language tag runs to the end of its line.
        let body_start = after.find('\n').map_or(0, |end| end + 1);
        let body = &after[body_start..];
        let Some(close) = body.find("```") else { break };
        found.push(body[..close].trim().to_owned());
        rest = &body[close + 3..];
    }
    for (open, close) in [('{', '}'), ('[', ']')] {
        if let (Some(start), Some(end)) = (trimmed.find(open), trimmed.rfind(close))
            && start < end
        {
            found.push(trimmed[start..=end].to_owned());
        }
    }
    found
}

/// The part of JSON Schema a worker's reply is held to: `type`, `enum`,
/// `required`, `properties`, `items` and `additionalProperties: false`.
/// Anything else in a schema still goes to the worker as a description of what
/// to write, and is not checked here.
fn check(value: &Value, schema: &Value, path: &str) -> Result<(), String> {
    let Some(schema) = schema.as_object() else {
        return Ok(());
    };
    if let Some(kinds) = schema.get("type") {
        let kinds: Vec<&str> = match kinds {
            Value::String(kind) => vec![kind.as_str()],
            Value::Array(kinds) => kinds.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !kinds.is_empty() && !kinds.iter().any(|kind| is_type(value, kind)) {
            return Err(format!(
                "{path} is {}, not {}",
                type_name(value),
                kinds.join(" or ")
            ));
        }
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        return Err(format!("{path} is not one of the allowed values"));
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    return Err(format!("{path} is missing \"{name}\""));
                }
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        if let Some(properties) = properties {
            for (name, sub_schema) in properties {
                if let Some(member) = object.get(name) {
                    check(member, sub_schema, &format!("{path}.{name}"))?;
                }
            }
        }
        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            for name in object.keys() {
                if !properties.is_some_and(|known| known.contains_key(name)) {
                    return Err(format!("{path} has an unexpected \"{name}\""));
                }
            }
        }
    }
    if let (Some(items), Some(sub_schema)) = (value.as_array(), schema.get("items")) {
        for (index, item) in items.iter().enumerate() {
            check(item, sub_schema, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}

fn is_type(value: &Value, kind: &str) -> bool {
    match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => {
            value.is_i64()
                || value.is_u64()
                || value
                    .as_f64()
                    .is_some_and(|number| number.is_finite() && number.fract() == 0.0)
        }
        _ => true,
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "verdict": {"type": "string", "enum": ["pass", "fail"]},
                "issues": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["verdict"],
            "additionalProperties": false
        })
    }

    #[test]
    fn without_a_schema_the_reply_is_its_text() {
        assert_eq!(result("all good", None), Ok(json!("all good")));
    }

    #[test]
    fn a_reply_is_read_as_json_however_it_is_wrapped() {
        let want = json!({"verdict": "pass", "issues": []});
        let schema = schema();
        for reply in [
            r#"{"verdict": "pass", "issues": []}"#,
            "```json\n{\"verdict\": \"pass\", \"issues\": []}\n```",
            "Here you go:\n```\n{\"verdict\": \"pass\", \"issues\": []}\n```\nDone.",
            "Result: {\"verdict\": \"pass\", \"issues\": []} - that is all",
        ] {
            assert_eq!(result(reply, Some(&schema)), Ok(want.clone()), "{reply}");
        }
    }

    #[test]
    fn a_reply_that_breaks_the_schema_says_where() {
        let schema = schema();
        let missing = result(r#"{"issues": []}"#, Some(&schema)).unwrap_err();
        assert!(missing.contains("missing \"verdict\""), "{missing}");
        let wrong = result(r#"{"verdict": "maybe"}"#, Some(&schema)).unwrap_err();
        assert!(wrong.contains("$.verdict"), "{wrong}");
        let nested = result(r#"{"verdict": "pass", "issues": [1]}"#, Some(&schema)).unwrap_err();
        assert!(nested.contains("$.issues[0]"), "{nested}");
        let extra = result(r#"{"verdict": "pass", "other": 1}"#, Some(&schema)).unwrap_err();
        assert!(extra.contains("unexpected \"other\""), "{extra}");
        let none = result("no json here", Some(&schema)).unwrap_err();
        assert!(none.contains("no JSON"), "{none}");
    }

    #[test]
    fn integers_and_number_lists_are_typed() {
        let schema = json!({"type": "array", "items": {"type": "integer"}});
        assert_eq!(result("[1, 2, 3.0]", Some(&schema)), Ok(json!([1, 2, 3.0])));
        assert!(result("[1, 2.5]", Some(&schema)).is_err());
    }

    #[test]
    fn the_prompt_carries_the_schema() {
        let asked = ask_for_json("Review this.", &schema());
        assert!(asked.starts_with("Review this."));
        assert!(asked.contains("\"verdict\""));
    }
}
