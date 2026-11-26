//! Browser tools ask before every call and stay out of Plan mode's page actions.

use super::*;
use crate::config::AgentMode;
use serde_json::json;

#[test]
fn browser_tools_ask_even_with_auto_approve() {
    for name in [
        "browser_open",
        "browser_read",
        "browser_screenshot",
        "browser_click",
        "browser_type",
    ] {
        assert_eq!(
            authorize_tool_with_args(name, &json!({}), AgentMode::Build, true, false),
            AuthorizationDecision::RequireConfirmation,
            "{name}"
        );
        assert_eq!(
            authorize_tool_with_args(name, &json!({}), AgentMode::Build, false, true),
            AuthorizationDecision::Allow,
            "{name}: an approval already given for this call runs it"
        );
        assert!(always_asks(name));
    }
    assert!(!always_asks("read_file"));
    assert_eq!(
        authorize_tool_with_args("read_file", &json!({}), AgentMode::Build, true, false),
        AuthorizationDecision::Allow
    );
}

#[test]
fn plan_mode_refuses_browser_actions_but_still_asks_for_reads() {
    for name in ["browser_open", "browser_click", "browser_type"] {
        assert!(!allowed_in_plan_mode(name), "{name}");
        let decision = authorize_tool_with_args(name, &json!({}), AgentMode::Plan, true, true);
        assert!(
            matches!(&decision, AuthorizationDecision::Deny(reason) if reason.contains("Plan mode")),
            "{name}: {decision:?}"
        );
    }
    for name in ["browser_read", "browser_screenshot"] {
        assert!(allowed_in_plan_mode(name), "{name}");
        assert_eq!(
            authorize_tool_with_args(name, &json!({}), AgentMode::Plan, true, false),
            AuthorizationDecision::RequireConfirmation,
            "{name}"
        );
    }
}
