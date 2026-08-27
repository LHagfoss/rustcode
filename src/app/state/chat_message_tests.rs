use super::{ChatMessage, ToolCallRef};
use crate::config::ToolProtocol;

#[test]
fn resolved_tool_calls_prefers_structured_tool_calls() {
    let msg = ChatMessage::new("assistant", "<think>some reasoning</think>").with_tool_calls(vec![
        ToolCallRef {
            id: "call_1".to_string(),
            name: "read_file".to_string(),
            arguments: r#"{"path": "src/main.rs"}"#.to_string(),
        },
    ]);

    let calls = msg.resolved_tool_calls(ToolProtocol::ApiNative);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "read_file");
    assert_eq!(calls[0].arguments["path"], "src/main.rs");
}

#[test]
fn resolved_tool_calls_falls_back_to_parsing_content_text() {
    let msg = ChatMessage::new(
        "assistant",
        "```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"foo\"}}\n```",
    );

    let calls = msg.resolved_tool_calls(ToolProtocol::Json);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "grep");
}
