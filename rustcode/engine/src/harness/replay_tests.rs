//! Contract tests for external harness adapters: a fixture is the traffic of
//! a scripted session (JSONL, one frame per line, `docs/harnesses.md`, *Replay
//! fixtures*), replayed into a [`Transcript`]; the history it records must
//! match the fixture's `expect` line. When an adapter changes the shape of
//! what it sends, the fixture recorded from the new version fails here first.

use super::Transcript;
use agent_client_protocol::schema::v1::{RequestPermissionRequest, SessionUpdate};
use rustcode_core::ChatMessage;
use serde::Deserialize;
use serde_json::Value;

const FIXTURES: [(&str, &str); 2] = [
    (
        "claude-agent-acp",
        include_str!("../fixtures/harness/claude-agent-acp.jsonl"),
    ),
    ("pi", include_str!("../fixtures/harness/pi.jsonl")),
];

/// What one recorded message must hold; a field left out is not checked.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expected {
    role: String,
    #[serde(default)]
    contains: Option<String>,
    #[serde(default)]
    calls: Vec<String>,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    pending: Option<bool>,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    error_kind: Option<String>,
    #[serde(default)]
    command: Option<String>,
}

struct Replayed {
    history: Vec<ChatMessage>,
    expected: Vec<Expected>,
    permissions: usize,
    expected_permissions: usize,
}

fn replay(name: &str, fixture: &str) -> Replayed {
    let mut transcript = Transcript::default();
    let mut expected = None;
    let mut expected_permissions = 0;
    let mut permissions = 0;
    for (number, line) in fixture.lines().enumerate() {
        let at = format!("{name}:{}", number + 1);
        if line.trim().is_empty() {
            continue;
        }
        let frame: Value =
            serde_json::from_str(line).unwrap_or_else(|error| panic!("{at}: {error}"));
        if frame.get("adapter").is_some() {
            continue;
        }
        if let Some(expect) = frame.get("expect") {
            expected = Some(
                serde_json::from_value::<Vec<Expected>>(expect.clone())
                    .unwrap_or_else(|error| panic!("{at}: {error}")),
            );
            expected_permissions = frame["permissions"].as_u64().unwrap_or(0) as usize;
            continue;
        }
        let params = frame.get("params").cloned().unwrap_or(Value::Null);
        match (
            frame["dir"].as_str(),
            frame.get("method").and_then(Value::as_str),
            frame.get("response").and_then(Value::as_str),
        ) {
            (Some("client"), Some("session/prompt"), None) => {
                let text = params["prompt"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|block| block["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                transcript.push_user(&text);
            }
            (Some("client"), Some("session/cancel"), None) => {}
            (Some("agent"), Some("session/update"), None) => {
                let update: SessionUpdate = serde_json::from_value(params["update"].clone())
                    .unwrap_or_else(|error| {
                        panic!("{at}: not a session/update this engine reads: {error}")
                    });
                transcript.observe(&update);
            }
            (Some("agent"), Some("session/request_permission"), None) => {
                serde_json::from_value::<RequestPermissionRequest>(params).unwrap_or_else(
                    |error| panic!("{at}: not a permission request this engine reads: {error}"),
                );
                permissions += 1;
            }
            (Some("agent"), None, Some("session/prompt")) => transcript.end_reply(),
            _ => panic!("{at}: a frame the replay does not know: {line}"),
        }
    }
    Replayed {
        history: transcript.history,
        expected: expected.unwrap_or_else(|| panic!("{name}: no expect line")),
        permissions,
        expected_permissions,
    }
}

fn check(name: &str, index: usize, message: &ChatMessage, expected: &Expected) {
    let at = format!("{name} message {}", index + 1);
    assert_eq!(message.role, expected.role, "{at}: role");
    if let Some(text) = &expected.contains {
        assert!(
            message.content.contains(text.as_str()),
            "{at}: {:?} does not contain {text:?}",
            message.content
        );
    }
    let calls: Vec<_> = message
        .tool_calls
        .iter()
        .map(|call| call.name.clone())
        .collect();
    assert_eq!(calls, expected.calls, "{at}: tool calls");
    let record = message.tool_result.as_ref();
    if expected.role == "tool" {
        assert!(record.is_some(), "{at}: no tool result");
    }
    let Some(record) = record else {
        return;
    };
    if let Some(tool) = &expected.tool {
        assert_eq!(&record.tool_name, tool, "{at}: tool kind");
    }
    if let Some(success) = expected.success {
        assert_eq!(record.success, success, "{at}: success");
    }
    if let Some(pending) = expected.pending {
        assert_eq!(record.pending, pending, "{at}: pending");
    }
    if let Some(code) = expected.exit_code {
        assert_eq!(record.exit_code, Some(code), "{at}: exit code");
    }
    if let Some(kind) = &expected.error_kind {
        assert_eq!(
            record.error_kind.as_deref(),
            Some(kind.as_str()),
            "{at}: error kind"
        );
    }
    if let Some(command) = &expected.command {
        assert_eq!(
            record.command.as_deref(),
            Some(command.as_str()),
            "{at}: command"
        );
    }
}

#[test]
fn every_fixture_replays_into_the_history_it_expects() {
    for (name, fixture) in FIXTURES {
        let replayed = replay(name, fixture);
        assert_eq!(
            replayed.history.len(),
            replayed.expected.len(),
            "{name}: recorded {:#?}",
            replayed
                .history
                .iter()
                .map(|message| (&message.role, &message.content))
                .collect::<Vec<_>>()
        );
        for (index, (message, expected)) in
            replayed.history.iter().zip(&replayed.expected).enumerate()
        {
            check(name, index, message, expected);
        }
        assert_eq!(
            replayed.permissions, replayed.expected_permissions,
            "{name}: permission requests"
        );
    }
}

#[test]
fn ignoring_structured_output_fails_the_claude_fixture() {
    let (name, fixture) = FIXTURES[0];
    let without_output = fixture.replace("\"exitCode\":0", "\"exit\":\"none\"");
    let replayed = replay(name, &without_output);
    let shell = &replayed.history[2];
    assert_ne!(
        shell
            .tool_result
            .as_ref()
            .and_then(|record| record.exit_code),
        replayed.expected[2].exit_code,
        "the exit code comes from rawOutput, so dropping it must show"
    );
}

#[test]
fn fixtures_hold_no_personal_data() {
    for (name, fixture) in FIXTURES {
        let lower = fixture.to_lowercase();
        for marker in [
            "/users/",
            "/home/",
            "c:\\\\users",
            "sk-",
            "ghp_",
            "github_pat_",
            "bearer ",
            "@",
        ] {
            assert!(!lower.contains(marker), "{name} contains {marker:?}");
        }
    }
}
