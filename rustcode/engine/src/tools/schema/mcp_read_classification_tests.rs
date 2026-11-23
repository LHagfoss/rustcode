use super::{mcp_annotations_mark_read_only, mcp_name_suggests_read_only};
use serde_json::json;

// Session 01a130ea (#1970): five unannotated worklog lookups in one
// response were held over the workspace-change limit.
#[test]
fn unannotated_lookups_are_reads() {
    for name in [
        "solidtime_list_clients",
        "solidtime_get_active_timer",
        "solidtime_list_entries",
        "list_clients",
        "get_active_timer",
        "search_issues",
        "find_user",
        "read_page",
        "show-config",
    ] {
        assert!(mcp_name_suggests_read_only(name), "{name}");
        assert!(mcp_annotations_mark_read_only(name, None), "{name}");
    }
}

#[test]
fn unannotated_mutations_and_unknown_verbs_stay_conservative() {
    for name in [
        "solidtime_start_timer",
        "solidtime_create_entry",
        "delete_list_items",
        "get_or_create_issue",
        "search_and_replace",
        "worklog_check_gaps",
        "whoami",
        "solidtime_list",
        "listClients",
    ] {
        assert!(!mcp_name_suggests_read_only(name), "{name}");
    }
}

#[test]
fn explicit_annotations_override_the_name_heuristic() {
    let not_read_only = json!({"readOnlyHint": false});
    assert!(!mcp_annotations_mark_read_only(
        "list_clients",
        Some(&not_read_only)
    ));
    let destructive = json!({"destructiveHint": true});
    assert!(!mcp_annotations_mark_read_only(
        "get_and_purge",
        Some(&destructive)
    ));
    let read_only = json!({"readOnlyHint": true});
    assert!(mcp_annotations_mark_read_only("whoami", Some(&read_only)));
}

// #2587: twelve servers inlined 101 MCP schemas into a small model's
// textual contract. Past the budget, tools are named and deferred to
// `list_mcp_tools`, and no server crowds out the others.
#[test]
fn textual_mcp_listing_defers_tools_past_the_schema_budget() {
    let schema = json!({"type": "object", "properties": {"q": {"type": "string"}}});
    let entries = (0..40)
        .map(|i| {
            let server = if i < 30 { "big" } else { "small" };
            (
                format!("{server}_tool_{i:02}"),
                server.to_string(),
                "x".repeat(300),
                schema.clone(),
            )
        })
        .collect::<Vec<_>>();

    let budget = super::MAX_MCP_NATIVE_SCHEMA_BYTES;
    let listing = super::textual_mcp_listing(&entries, false, budget);

    assert!(listing.len() < budget + 1024, "{} bytes", listing.len());
    assert!(listing.contains("- big_tool_00 | Args:"));
    assert!(listing.contains("- small_tool_30 | Args:"), "{listing}");
    assert!(listing.contains("`list_mcp_tools` with `select`"));
    assert!(listing.contains("big_tool_29"));
    assert!(!listing.contains("- big_tool_29 | Args:"));

    let small = super::textual_mcp_listing(&entries[..2], false, 8 * 1024);
    assert!(!small.contains("list_mcp_tools"));
}
