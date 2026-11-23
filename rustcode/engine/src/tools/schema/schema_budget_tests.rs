use super::*;

/// Ceiling for the built-in half of the native `tools` block under the
/// root coding policy with agent tools, the widest menu a request carries.
///
/// The block was 31,659 bytes before its descriptions were trimmed to
/// 23,583 (#1826). The 25 KiB ceiling accommodates archive retrieval and
/// MCP server management (#1890) while staying below the old size. It is
/// sent on every request, so keep new descriptions concise and measure
/// their cost. Run with `-- --nocapture` for the per-tool table.
const BUILTIN_SCHEMA_BYTE_BUDGET: usize = 25 * 1024;

/// Room the plain root block must keep under that budget, so the next tool
/// does not use the last free byte.
const MIN_BUILTIN_SCHEMA_HEADROOM_BYTES: usize = 256;

/// Serialized bytes of every built-in tool's native schema entry, largest
/// first, measured the way the request builder serializes it. Covers the
/// whole registry and the agent-control tools, advertised or not.
fn builtin_schema_sizes() -> Vec<(&'static str, usize)> {
    let entry = |name: &str, description: &str, parameters: Value| {
        // One array element, without the array's own brackets.
        serialized_schema_bytes(&[serde_json::json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": provider_compatible_schema(parameters),
            }
        })]) - 2
    };
    let mut sizes = TOOLS
        .iter()
        .map(|tool| {
            (
                tool.name,
                entry(tool.name, tool.description, schema_for_tool(tool.name)),
            )
        })
        .chain(AGENT_TOOL_SPECS.iter().map(|(name, description, _)| {
            (*name, entry(name, description, schema_for_agent_tool(name)))
        }))
        .collect::<Vec<_>>();
    sizes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    sizes
}

/// Room the newest client's block must keep under the hard cap. Without
/// it the next tool added here fits the cap by 1 byte and the block still
/// shrinks on the request after that; the assertion turns the next
/// addition into a build failure with the number to free (#3376).
const MIN_ORCHESTRATION_HEADROOM_BYTES: usize = 2 * 1024;

#[test]
fn the_orchestration_tools_fit_the_hard_cap() {
    // Every supported client version gets every tool its version
    // advertises in its built-in schema: the hard cap must never withhold
    // one the client can answer, since the tool is then silently missing
    // from the menu. This is the regression test for the 41 KiB cap that
    // cut `thread_status`, `merge_when_green` and `cleanup_worktree` from a
    // v9 client (#3376): the names are compared, not only the count, so a
    // swap cannot pass as a match.
    for version in 1..=crate::orchestration::VERSION {
        let policy = ToolSchemaPolicy::root(true).with_orchestration_tools(version);
        let expected = crate::tools::TOOLS
            .iter()
            .filter(|tool| builtin_is_advertised(tool, policy))
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        let (tools, withheld) = build_builtin_native_tools_schema(policy);
        let kept = tools
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .collect::<Vec<_>>();
        let bytes = serialized_schema_bytes(&tools);
        println!(
            "orchestration v{version}: {bytes} bytes, {} of {} tools, {} bytes free, withheld {withheld:?}",
            kept.len(),
            expected.len(),
            MAX_BUILTIN_NATIVE_SCHEMA_BYTES - bytes
        );
        assert_eq!(kept, expected, "v{version} lost a tool to the cap");
        assert!(withheld.is_empty(), "v{version} withheld: {withheld:?}");
        assert!(
            bytes < MAX_BUILTIN_NATIVE_SCHEMA_BYTES,
            "v{version} is {bytes} bytes, over the hard cap"
        );
        if version == crate::orchestration::VERSION {
            let free = MAX_BUILTIN_NATIVE_SCHEMA_BYTES - bytes;
            assert!(
                free >= MIN_ORCHESTRATION_HEADROOM_BYTES,
                "v{version} has {free} bytes free, under the {MIN_ORCHESTRATION_HEADROOM_BYTES} \
                     a new tool needs: shorten descriptions in `tools/misc.rs` and `tools/schema.rs` first"
            );
        }
    }
    // A client below 5 is not shown an argument its app cannot answer.
    let start_schema = |version: u8| {
        let policy = ToolSchemaPolicy::root(true).with_orchestration_tools(version);
        let (tools, _) = build_builtin_native_tools_schema(policy);
        tools
            .into_iter()
            .find(|tool| tool["function"]["name"] == crate::orchestration::START_SESSION)
            .expect("start_session")
    };
    assert!(start_schema(4)["function"]["parameters"]["properties"]["account"].is_null());
    assert!(start_schema(5)["function"]["parameters"]["properties"]["account"].is_object());
    // A standalone thread arrived in v15 (issue 3443): v14 never sees the
    // field, so an app that cannot start one is not offered it.
    assert!(start_schema(14)["function"]["parameters"]["properties"]["standalone"].is_null());
    assert!(start_schema(15)["function"]["parameters"]["properties"]["standalone"].is_object());
    // Nor is v9 shown files to attach, which its app would drop.
    let message_schema = |version: u8| {
        let policy = ToolSchemaPolicy::root(true).with_orchestration_tools(version);
        let (tools, _) = build_builtin_native_tools_schema(policy);
        tools
            .into_iter()
            .find(|tool| tool["function"]["name"] == crate::orchestration::MESSAGE_SESSION)
            .expect("message_session")
    };
    assert!(message_schema(9)["function"]["parameters"]["properties"]["attachments"].is_null());
    assert!(message_schema(10)["function"]["parameters"]["properties"]["attachments"].is_object());
}

#[test]
fn the_block_check_is_not_a_built_in_tool() {
    // It is listed by the `rustcode` MCP server an external harness runs,
    // and answered there; a RustCode turn would only get the error handler.
    assert!(
        crate::tools::TOOLS
            .iter()
            .any(|tool| tool.name == crate::smart_ui::CHECK_TOOL_NAME)
    );
    for policy in [
        ToolSchemaPolicy::root(true),
        ToolSchemaPolicy::read_only_inspection(),
    ] {
        let (tools, _) = build_builtin_native_tools_schema(policy);
        assert!(
            !tools
                .iter()
                .any(|tool| tool["function"]["name"] == crate::smart_ui::CHECK_TOOL_NAME),
            "the block check is never a built-in tool"
        );
    }
}

#[test]
fn the_link_tool_is_built_in_for_every_client() {
    // The `rustcode` MCP server an external harness runs lists and answers
    // it (`orchestration/mcp_server.rs`, #3369), and a RustCode turn
    // answers it in `pull_watch::answer` through its own handler: since
    // #3376 freed the room, it is advertised to every client instead of
    // only to an external harness.
    assert!(
        crate::tools::TOOLS
            .iter()
            .any(|tool| tool.name == crate::pull_watch::LINK_TOOL_NAME)
    );
    for policy in [
        ToolSchemaPolicy::root(false),
        ToolSchemaPolicy::root(true),
        ToolSchemaPolicy::root(true).with_orchestration_tools(crate::orchestration::VERSION),
    ] {
        let (tools, withheld) = build_builtin_native_tools_schema(policy);
        assert!(
            tools
                .iter()
                .any(|tool| tool["function"]["name"] == crate::pull_watch::LINK_TOOL_NAME),
            "the link tool is missing from the built-in block: {policy:?}, withheld {withheld:?}"
        );
    }
    // It links a thread, so it is not a read-only inspection tool.
    let (tools, _) = build_builtin_native_tools_schema(ToolSchemaPolicy::read_only_inspection());
    assert!(
        !tools
            .iter()
            .any(|tool| tool["function"]["name"] == crate::pull_watch::LINK_TOOL_NAME)
    );
}

#[test]
fn builtin_schema_bytes_stay_within_budget() {
    let sizes = builtin_schema_sizes();
    for (name, bytes) in &sizes {
        println!("{bytes:>6}  {name}");
    }
    println!(
        "{:>6}  all {} built-in and agent tools",
        sizes.iter().map(|(_, bytes)| bytes).sum::<usize>(),
        sizes.len()
    );

    let (tools, withheld) = build_builtin_native_tools_schema(ToolSchemaPolicy::root(true));
    let bytes = serialized_schema_bytes(&tools);
    println!(
        "{bytes:>6}  advertised built-in block, {} tools (budget {BUILTIN_SCHEMA_BYTE_BUDGET})",
        tools.len()
    );
    assert!(
        withheld.is_empty(),
        "the byte cap withheld built-in tools: {withheld:?}"
    );
    assert!(
        bytes <= BUILTIN_SCHEMA_BYTE_BUDGET,
        "built-in tool schemas grew to {bytes} bytes, over the {BUILTIN_SCHEMA_BYTE_BUDGET} budget; \
             shorten a description (largest first: {:?})",
        &sizes[..5]
    );
    // A budget reached to the last byte is a budget that fails the next
    // tool that arrives, which is how the versioned block lost three tools
    // under its old cap (#3376). Keep room for one modest addition.
    let free = BUILTIN_SCHEMA_BYTE_BUDGET - bytes;
    assert!(
        free >= MIN_BUILTIN_SCHEMA_HEADROOM_BYTES,
        "the built-in block has {free} bytes free, under the {MIN_BUILTIN_SCHEMA_HEADROOM_BYTES} \
             a new tool needs: shorten a description (largest first: {:?})",
        &sizes[..5]
    );
    // The budget only means something while it sits under the hard cap
    // that starts withholding tools.
    assert!(BUILTIN_SCHEMA_BYTE_BUDGET < MAX_BUILTIN_NATIVE_SCHEMA_BYTES);
}

#[test]
fn cached_builtin_schema_matches_a_fresh_build_for_each_policy() {
    for policy in [
        ToolSchemaPolicy::root(false),
        ToolSchemaPolicy::root(true),
        ToolSchemaPolicy::root(true).with_session_title_tool(),
        ToolSchemaPolicy::read_only_inspection(),
    ] {
        let (tools, withheld) = build_builtin_native_tools_schema(policy);
        let cached = cached_builtin_native_tools_schema(policy);
        assert_eq!(cached.tools, tools, "{policy:?}");
        assert_eq!(cached.withheld, withheld, "{policy:?}");
        assert_eq!(cached.bytes, serialized_schema_bytes(&tools), "{policy:?}");
        assert!(Arc::ptr_eq(
            &cached,
            &cached_builtin_native_tools_schema(policy)
        ));
    }
}

/// Trimming must never leave a tool undescribed, and a description pays
/// for every byte, including stray runs of spaces.
#[test]
fn every_builtin_tool_keeps_a_description() {
    for tool in TOOLS {
        assert!(
            tool.description.trim().len() >= 20,
            "{} lost its description",
            tool.name
        );
        assert!(
            !tool.description.contains("  "),
            "{} has a run of spaces in its description",
            tool.name
        );
    }
}
