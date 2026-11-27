//! What a thread's harness is offered by RustCode's MCP server, without a
//! thread: the `instructions` and the tool list, as `rc tools manifest`
//! prints them. The evals in `evals/` read them from here, so an eval always
//! tests this checkout's text.

use serde_json::{Value, json};

use super::mcp_server::{Offered, all_tools, instructions};

/// Which variant of the list a thread is offered.
#[derive(Debug, Clone, Copy, Default)]
pub struct Offer {
    /// `smart_ui_check`, as when the user has Smart UI on.
    pub smart_ui: bool,
    /// `ask_user_question`, as for a harness without a question tool.
    pub ask_user: bool,
    /// Every tool, as with `orchestration-all-tools` on; otherwise the core list.
    pub all_tools: bool,
    /// The short list with `tool_search` and `call_tool`, as for a harness
    /// that sends every schema with each request.
    pub deferred: bool,
}

/// `{"instructions", "tools"}` for a client of this engine's orchestration version.
pub fn manifest(offer: Offer) -> Value {
    let offered = Offered {
        smart_ui: offer.smart_ui,
        ask_user: offer.ask_user,
        core_only: !offer.all_tools,
        deferred: offer.deferred,
    };
    json!({
        "instructions": instructions(super::VERSION, offered),
        "tools": all_tools(super::VERSION, offered),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_uses_public_names_and_the_settle_rule() {
        let manifest = manifest(Offer::default());
        let names: Vec<&str> = manifest["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(names.contains(&"update_thread"), "{names:?}");
        assert!(!names.contains(&"update_session"), "{names:?}");
        let text = manifest["instructions"].as_str().unwrap();
        assert!(text.contains("`update_thread` with `status: \"settled\"`"));
    }

    #[test]
    fn a_deferred_manifest_lists_tool_search() {
        let manifest = manifest(Offer {
            deferred: true,
            ..Offer::default()
        });
        assert!(
            manifest["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == "tool_search")
        );
    }
}
