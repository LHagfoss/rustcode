// Recorded on the parent commit in this test environment: ApiNative build
// prompt = 15,641 bytes / 3,354 tokens.
#[test]
fn compressed_core_prompt_preserves_contracts_and_reduces_size() {
    const PRIOR_BYTES: usize = 15_641;
    const PRIOR_TOKENS: usize = 3_354;
    let prompt = super::tool_system_prompt(
        false,
        crate::config::ToolProtocol::ApiNative,
        crate::config::AgentMode::Build,
    );

    assert!(
        prompt.len() < PRIOR_BYTES * 80 / 100,
        "{} bytes",
        prompt.len()
    );
    assert!(
        crate::network::compaction::estimate_tokens(&prompt) < PRIOR_TOKENS * 80 / 100,
        "{} tokens",
        crate::network::compaction::estimate_tokens(&prompt)
    );
    assert!(prompt.len() <= super::schema::BASE_PROMPT_MAX_BYTES);
    assert!(
        crate::network::compaction::estimate_tokens(&prompt)
            <= super::schema::BASE_PROMPT_MAX_TOKENS
    );
    for required in [
        "sandbox/",
        "background",
        "run_command",
        "destructive operations",
        "Batch independent reads",
        "Wait for results",
        "already applied",
        "advisory loop signals, not a hard stop",
        "native function-calling interface",
        "plain-text summary",
        "do NOT print tool calls as text or JSON",
        // Tables, bullets and paragraphs moved to the root-only Output style
        // hint (`output_style_reaches_root_prompts_within_budget`).
        "For research/review answers, label verified findings, judgments, and gaps",
        "never claim comprehensive coverage without evidence",
        "Avoid fixed long templates",
        "Cite `path:line` with full repo-relative paths",
    ] {
        assert!(prompt.contains(required), "missing {required:?}");
    }
}

#[test]
fn api_native_prompt_and_tool_schema_are_measured_separately() {
    let prompt = super::tool_system_prompt(
        false,
        crate::config::ToolProtocol::ApiNative,
        crate::config::AgentMode::Build,
    );
    let schema = serde_json::to_string(&super::native_tools_schema(false)).unwrap();
    assert!(prompt.len() <= super::schema::BASE_PROMPT_MAX_BYTES);
    assert!(schema.len() > 1_000);
    assert!(crate::network::compaction::estimate_tokens(&schema) > 0);
}

#[test]
fn compact_text_contract_exposes_its_active_protocol_and_tool_surface() {
    let policy = ToolSchemaPolicy::root_for_mode_with_compact_prompt(
        false,
        crate::config::AgentMode::Build,
        true,
    );
    let surface = textual_tool_surface(policy, crate::config::AgentMode::Build);
    let prompt = tool_system_prompt_for_policy(
        policy,
        crate::config::ToolProtocol::Native,
        crate::config::AgentMode::Build,
    );

    assert!(surface.builtin > 0);
    assert_eq!(surface.agent, 0);
    assert!(prompt.contains("[TOOL_CALLS]tool_name[ARGS]"));
    assert!(prompt.contains("Available tools:"));
    assert!(prompt.contains("Use the exact tool names and JSON argument shapes"));
    assert!(!prompt.contains("native function-calling interface"));
}
use super::*;

#[test]
fn tools_have_unique_names() {
    let mut names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names.len(),
        TOOLS.len(),
        "duplicate tool names in TOOLS registry"
    );
}

#[test]
fn conservative_builtin_aliases_recover_common_call_spellings() {
    assert_eq!(resolve_builtin_tool_alias("read"), Some("view_file"));
    assert_eq!(resolve_builtin_tool_alias("write"), Some("write_to_file"));
    assert_eq!(
        resolve_builtin_tool_alias("write_file"),
        Some("write_to_file")
    );
    assert_eq!(resolve_builtin_tool_alias("bash"), Some("run_command"));
    assert_eq!(
        resolve_builtin_tool_alias("replace_string_in_file"),
        Some("replace_file_content")
    );
    assert_eq!(
        resolve_builtin_tool_alias("str_replace"),
        Some("replace_file_content")
    );
}

#[test]
fn aliases_do_not_claim_canonical_or_unknown_names() {
    // Canonical names are never rewritten, and arbitrary/MCP-like names are
    // left for strict validation rather than guessed at.
    assert_eq!(resolve_builtin_tool_alias("view_file"), None);
    assert_eq!(resolve_builtin_tool_alias("write_to_file"), None);
    assert_eq!(resolve_builtin_tool_alias("mcp__server__write"), None);
    assert_eq!(resolve_builtin_tool_alias("write_filee"), None);
}

#[test]
fn every_tool_schema_is_a_json_object() {
    for tool in TOOLS {
        let schema = (tool.schema)();
        assert_eq!(
            schema.get("type").and_then(|t| t.as_str()),
            Some("object"),
            "tool '{}' schema must be a JSON object with type \"object\"",
            tool.name
        );
        assert!(
            schema.get("properties").is_some(),
            "tool '{}' schema must declare properties",
            tool.name
        );
    }
}

#[test]
fn policy_tables_match_registry() {
    use ToolCapability::*;
    // Representative spot-checks against the pre-refactor lookup tables.
    assert_eq!(tool_capabilities("grep"), &[ReadWorkspace]);
    assert_eq!(tool_safety("grep"), ToolSafety::ReadOnly);
    assert!(!needs_confirmation("grep"));

    assert_eq!(tool_capabilities("run_command"), &[ExecuteCommands]);
    assert_eq!(tool_safety("run_command"), ToolSafety::ProcessControl);
    assert!(needs_confirmation("run_command"));

    assert_eq!(tool_capabilities("use_skill"), &[SessionState]);
    assert_eq!(tool_safety("use_skill"), ToolSafety::ControlPlane);
    assert!(!needs_confirmation("use_skill"));

    assert_eq!(tool_capabilities("manage_task"), &[ExecuteCommands]);
    assert_eq!(tool_safety("manage_task"), ToolSafety::ProcessControl);
    assert!(!needs_confirmation("manage_task"));

    // Agent tools live outside TOOLS and keep their fallback arms.
    assert_eq!(
        tool_capabilities("spawn_agent"),
        &[AgentDelegation, SessionState]
    );
    assert_eq!(tool_safety("spawn_agent"), ToolSafety::Delegation);
    assert!(!needs_confirmation("spawn_agent"));
    assert_eq!(tool_capabilities("todo_write"), &[SessionState]);
    assert_eq!(tool_safety("background_output"), ToolSafety::ProcessControl);
    assert_eq!(tool_safety("unknown_tool_xyz"), ToolSafety::Unknown);
}

#[test]
fn schema_from_arguments_extracts_string_props() {
    let schema = schema_from_arguments(
        r#"{"pattern": "regex pattern", "path": "optional dir", "ignore_case": optional bool}"#,
    );
    let props = schema["properties"].as_object().unwrap();
    assert_eq!(schema["type"], "object");
    assert_eq!(props["pattern"]["type"], "string");
    assert_eq!(props["pattern"]["description"], "regex pattern");
    // Non-string values (bool/number) still register as optional string props.
    assert_eq!(props["ignore_case"]["type"], "string");
    assert!(props.contains_key("path"));
}

#[test]
fn schema_from_arguments_marks_array_params() {
    // Description says "array" → real array schema, not string.
    let schema = schema_from_arguments(
        r#"{"path": "file path", "edits": "optional array of {old_string, new_string}"}"#,
    );
    let props = schema["properties"].as_object().unwrap();
    assert_eq!(props["edits"]["type"], "array");
    assert_eq!(props["edits"]["items"]["type"], "object");
    assert_eq!(props["path"]["type"], "string");

    // Array-literal value with no description → still an array.
    let schema2 =
        schema_from_arguments(r#"{"question": "q", "options": ["Option 1", "Option 2"]}"#);
    let props2 = schema2["properties"].as_object().unwrap();
    assert_eq!(props2["options"]["type"], "array");
}

#[test]
fn coerce_array_accepts_real_and_stringified() {
    assert_eq!(coerce_array(&serde_json::json!([1, 2])).unwrap().len(), 2);
    assert_eq!(
        coerce_array(&serde_json::json!("[1, 2, 3]")).unwrap().len(),
        3
    );
    assert!(coerce_array(&serde_json::json!("not json")).is_none());
    assert!(coerce_array(&serde_json::json!(5)).is_none());
}

#[test]
fn schema_from_arguments_handles_no_args() {
    let schema = schema_from_arguments("{} (no arguments)");
    assert_eq!(schema["type"], "object");
    assert!(schema["properties"].as_object().unwrap().is_empty());
}

#[test]
fn native_tools_schema_covers_builtins_and_agent_tools() {
    let tools = native_tools_schema(true);
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    // Every entry is a well-formed function tool.
    assert!(tools.iter().all(|t| t["type"] == "function"));
    assert!(names.contains(&"grep"));
    assert!(names.contains(&"view_file"));
    assert!(names.contains(&"complete_task"));
    // Agent tools are included when requested.
    assert!(names.contains(&"spawn_agent"));
    assert!(names.contains(&"todo_write"));
    // Excluded when not requested.
    let no_agents = native_tools_schema(false);
    assert!(
        !no_agents
            .iter()
            .any(|t| t["function"]["name"] == "spawn_agent")
    );
}

#[test]
fn run_command_schema_distinguishes_waited_background_from_detached_servers() {
    let run_command = native_tools_schema(false)
        .into_iter()
        .find(|tool| tool["function"]["name"] == "run_command")
        .expect("run_command is advertised");
    let function = &run_command["function"];
    let properties = &function["parameters"]["properties"];
    assert_eq!(properties["background"]["default"], false);
    assert_eq!(properties["detached"]["default"], false);
    assert!(
        function["description"]
            .as_str()
            .is_some_and(|description| description.contains("detached=true"))
    );
}

#[test]
fn native_tool_schema_variants_are_stable_across_reuse() {
    assert_eq!(
        native_tools_schema(false),
        native_tools_schema(false),
        "built-in schema variant changed between requests"
    );
    assert_eq!(
        native_tools_schema(true),
        native_tools_schema(true),
        "agent-enabled schema variant changed between requests"
    );
}

#[test]
fn provider_schema_removes_unsupported_json_schema_metadata_and_bounds() {
    let canonical = serde_json::json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "example",
        "type": "object",
        "properties": {
            "duration": {"type": "number", "exclusiveMinimum": 0},
            "nested": {
                "type": "array",
                "items": {"$schema": "nested", "type": "string"}
            }
        }
    });

    let compatible = provider_compatible_schema(canonical.clone());

    assert_eq!(canonical["properties"]["duration"]["exclusiveMinimum"], 0);
    assert_eq!(compatible["properties"]["duration"]["minimum"], 0);
    assert!(compatible.get("$schema").is_none());
    assert!(compatible.get("$id").is_none());
    assert!(
        compatible["properties"]["duration"]
            .get("exclusiveMinimum")
            .is_none()
    );
    assert!(
        compatible["properties"]["nested"]["items"]
            .get("$schema")
            .is_none()
    );
}

#[test]
fn provider_schema_recursively_normalizes_union_type_arrays() {
    fn contains_type_array(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(object) => {
                object.get("type").is_some_and(serde_json::Value::is_array)
                    || object.values().any(contains_type_array)
            }
            serde_json::Value::Array(values) => values.iter().any(contains_type_array),
            _ => false,
        }
    }

    let canonical = serde_json::json!({
        "type": "object",
        "properties": {
            "folder": {
                "type": ["string", "null"],
                "format": "uri",
                "minLength": 1
            },
            "attachments": {
                "type": ["array", "null"],
                "items": {
                    "type": ["string", "null"],
                    "enum": ["inline", null]
                },
                "minItems": 1
            },
            "metadata": {
                "type": "object",
                "additionalProperties": {
                    "type": ["integer", "null"],
                    "minimum": 0
                }
            }
        }
    });
    let original = canonical.clone();

    let compatible = provider_compatible_schema(canonical.clone());

    assert_eq!(canonical, original, "canonical schema must not be mutated");
    assert!(contains_type_array(&canonical));
    assert!(!contains_type_array(&compatible));
    assert_eq!(
        compatible["properties"]["folder"]["anyOf"],
        serde_json::json!([
            {"type": "string", "format": "uri", "minLength": 1},
            {"type": "null"}
        ])
    );
    assert_eq!(
        compatible["properties"]["attachments"]["anyOf"][0]["items"]["anyOf"],
        serde_json::json!([
            {"type": "string", "enum": ["inline", null]},
            {"type": "null"}
        ])
    );
    assert_eq!(
        compatible["properties"]["metadata"]["additionalProperties"]["anyOf"],
        serde_json::json!([
            {"type": "integer", "minimum": 0},
            {"type": "null"}
        ])
    );
}

#[test]
fn mcp_schema_selection_omits_irrelevant_tools_but_keeps_relevant_and_used() {
    let mcp = vec![
        (
            "search_issues".to_string(),
            "Search GitHub issues and pull requests".to_string(),
            serde_json::json!({"type":"object","properties":{"query":{"type":"string"}}}),
        ),
        (
            "weather_forecast".to_string(),
            "Get a weather forecast for a city".to_string(),
            serde_json::json!({"type":"object","properties":{"city":{"type":"string"}}}),
        ),
        (
            "deploy_release".to_string(),
            "Publish a release to production".to_string(),
            serde_json::json!({"type":"object","properties":{"version":{"type":"string"}}}),
        ),
    ];
    let messages = vec![
        serde_json::json!({"role":"user","content":"Find the open issue about parser retries"}),
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{"function":{"name":"weather_forecast"}}]
        }),
    ];

    let (selected, stats) = select_mcp_tools_for_context(&mcp, &messages);
    let names: Vec<&str> = selected
        .iter()
        .map(|index| mcp[*index].0.as_str())
        .collect();
    assert_eq!(names, vec!["search_issues", "weather_forecast"]);
    assert!(!names.contains(&"deploy_release"));
    assert_eq!(stats.available, 3);
    assert_eq!(stats.selected, 2);
    assert_eq!(stats.relevant, 1);
    assert_eq!(stats.previously_used, 1);
    assert_eq!(stats.omitted, 1);
}

#[test]
fn discovery_query_keeps_its_match_in_the_next_native_schema() {
    let mcp = vec![
        (
            "mcp__calendar__search_events".to_string(),
            "Find calendar events by date or attendee".to_string(),
            serde_json::json!({"type":"object","properties":{"query":{"type":"string"}}}),
        ),
        (
            "mcp__mail__search_messages".to_string(),
            "Find email messages by sender or subject".to_string(),
            serde_json::json!({"type":"object","properties":{"query":{"type":"string"}}}),
        ),
    ];
    let owners = vec!["calendar".to_string(), "mail".to_string()];
    let messages = vec![
        serde_json::json!({"role":"user","content":"Look up the next appointment"}),
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{
                "function":{
                    "name":"list_mcp_tools",
                    "arguments":"{\"query\":\"attendee\",\"limit\":4}"
                }
            }]
        }),
    ];

    let (selected, _) =
        super::schema::select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
            &mcp,
            &owners,
            &[],
            &messages,
            &[],
            super::schema::ToolSchemaPhase::Established,
        );
    let names: Vec<&str> = selected
        .iter()
        .map(|index| mcp[*index].0.as_str())
        .collect();
    assert_eq!(names, ["mcp__calendar__search_events"]);
}

#[test]
fn newest_discovery_after_latest_user_turn_replaces_historical_matches() {
    let mut mcp = (0..20)
        .map(|index| {
            (
                format!("mcp__old__tool_{index:02}"),
                "Archive a historical item".to_string(),
                serde_json::json!({"type":"object","properties":{}}),
            )
        })
        .collect::<Vec<_>>();
    let mut owners = vec!["old".to_string(); 20];
    mcp.push((
        "mcp__new__latest_tool".to_string(),
        "Current operation".to_string(),
        serde_json::json!({"type":"object","properties":{}}),
    ));
    owners.push("new".to_string());
    let messages = vec![
        serde_json::json!({"role":"user","content":"Check the forecast"}),
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{
                "function":{
                    "name":"list_mcp_tools",
                    "arguments":"{\"server\":\"old\",\"limit\":20}"
                }
            }]
        }),
        serde_json::json!({"role":"user","content":"Now do the current operation"}),
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{
                "function":{
                    "name":"list_mcp_tools",
                    "arguments":"{\"query\":\"latest_tool\",\"limit\":4}"
                }
            }]
        }),
    ];

    let (selected, _) =
        super::schema::select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
            &mcp,
            &owners,
            &[],
            &messages,
            &[],
            super::schema::ToolSchemaPhase::Established,
        );
    let names: Vec<&str> = selected
        .iter()
        .map(|index| mcp[*index].0.as_str())
        .collect();
    assert_eq!(names, ["mcp__new__latest_tool"]);
}

// Regression: every request ends with a synthetic `user` context message.
// Discovery looked for calls after the latest `user` message, found none, and
// never bound what `list_mcp_tools` had just returned.
#[test]
fn discovery_binds_its_result_when_the_request_ends_with_runtime_context() {
    let schema = serde_json::json!({"type":"object","properties":{}});
    let mut mcp = (0..20)
        .map(|index| {
            (
                format!("mcp__notes__entry_{index:02}"),
                "Archive a historical item".to_string(),
                schema.clone(),
            )
        })
        .collect::<Vec<_>>();
    let mut owners = vec!["notes".to_string(); 20];
    let mut raw_names = (0..20)
        .map(|index| format!("entry_{index:02}"))
        .collect::<Vec<_>>();
    mcp.push((
        "mcp__chat__send_chat_message".to_string(),
        "Send one message to a visible chat".to_string(),
        schema,
    ));
    owners.push("chat".to_string());
    raw_names.push("send_chat_message".to_string());
    let messages = vec![
        serde_json::json!({"role":"user","content":"Tell them the build is green"}),
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{
                "function":{
                    "name":"list_mcp_tools",
                    "arguments":"{\"query\":\"send message chat composer\",\"limit\":20}"
                }
            }]
        }),
        serde_json::json!({"role":"tool","content":"{\"results\":[]}"}),
        serde_json::json!({
            "role":"user",
            "content":"<rustcode_context>\n# Environment\n</rustcode_context>"
        }),
    ];

    let (selected, _) = super::schema::select_mcp_tools_for_context_with_raw_names(
        &mcp,
        &owners,
        &raw_names,
        &[],
        &messages,
        &[],
        super::schema::ToolSchemaPhase::Established,
    );
    let names: Vec<&str> = selected
        .iter()
        .map(|index| mcp[*index].0.as_str())
        .collect();
    assert_eq!(names, ["mcp__chat__send_chat_message"]);
}

// Regression: only the newest discovery call bound its tools, so finding
// tool B evicted tool A and the model listed both again and again.
#[test]
fn every_discovery_call_since_the_user_turn_binds_its_tools_ahead_of_the_pin() {
    let schema = serde_json::json!({"type":"object","properties":{}});
    let mut mcp = (0..MAX_MCP_NATIVE_SCHEMAS)
        .map(|index| {
            (
                format!("mcp__notes__entry_{index:02}"),
                "Archive a historical item".to_string(),
                schema.clone(),
            )
        })
        .collect::<Vec<_>>();
    let mut owners = vec!["notes".to_string(); MAX_MCP_NATIVE_SCHEMAS];
    let mut raw_names = (0..MAX_MCP_NATIVE_SCHEMAS)
        .map(|index| format!("entry_{index:02}"))
        .collect::<Vec<_>>();
    for (server, name, description) in [
        ("weather", "get_forecast", "Weather forecast for a city"),
        (
            "chat",
            "send_chat_message",
            "Send one message to a visible chat",
        ),
    ] {
        mcp.push((
            format!("mcp__{server}__{name}"),
            description.to_string(),
            schema.clone(),
        ));
        owners.push(server.to_string());
        raw_names.push(name.to_string());
    }
    // A full pinned menu from an earlier turn.
    let sticky = mcp[..MAX_MCP_NATIVE_SCHEMAS]
        .iter()
        .map(|(name, _, _)| name.clone())
        .collect::<Vec<_>>();
    let call = |arguments: &str| {
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{"function":{"name":"list_mcp_tools","arguments":arguments}}]
        })
    };
    let messages = vec![
        serde_json::json!({"role":"user","content":"Tell the team what tomorrow looks like"}),
        call("{\"query\":\"weather forecast\"}"),
        serde_json::json!({"role":"tool","content":"{\"results\":[]}"}),
        call("{\"query\":\"send chat message\"}"),
        serde_json::json!({"role":"tool","content":"{\"results\":[]}"}),
    ];

    let (selected, stats) = super::schema::select_mcp_tools_for_context_with_raw_names(
        &mcp,
        &owners,
        &raw_names,
        &[],
        &messages,
        &sticky,
        super::schema::ToolSchemaPhase::Established,
    );
    let names: Vec<&str> = selected
        .iter()
        .map(|index| mcp[*index].0.as_str())
        .collect();
    assert!(names.contains(&"mcp__weather__get_forecast"), "{names:?}");
    assert!(names.contains(&"mcp__chat__send_chat_message"), "{names:?}");
    assert_eq!(stats.requested, 2);
    assert!(names.len() <= MAX_MCP_NATIVE_SCHEMAS);
}

#[test]
fn a_select_call_binds_tools_named_in_every_accepted_form() {
    let schema = serde_json::json!({"type":"object","properties":{}});
    let entries = [
        ("snapchat", "get_friends", "get_friends"),
        ("discord", "list_channels", "list_channels"),
        ("mail", "search_messages", "mcp__mail__search_messages"),
        ("discord", "send_message", "mcp__discord__send_message"),
    ];
    let mcp = entries
        .iter()
        .map(|(_, _, advertised)| (advertised.to_string(), String::new(), schema.clone()))
        .collect::<Vec<_>>();
    let owners = entries
        .iter()
        .map(|(server, _, _)| server.to_string())
        .collect::<Vec<_>>();
    let raw_names = entries
        .iter()
        .map(|(_, raw, _)| raw.to_string())
        .collect::<Vec<_>>();
    let messages = vec![
        serde_json::json!({"role":"user","content":"Do it"}),
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{"function":{
                "name":"list_mcp_tools",
                "arguments":"{\"select\":[\"get_friends\",\"discord.list_channels\",\"mcp__mail__search_messages\"]}"
            }}]
        }),
    ];

    let (selected, _) = super::schema::select_mcp_tools_for_context_with_raw_names(
        &mcp,
        &owners,
        &raw_names,
        &[],
        &messages,
        &[],
        super::schema::ToolSchemaPhase::Established,
    );
    assert_eq!(selected, [0, 1, 2]);
}

// Regression: naming a server requested all of its tools, and the first
// sixteen by name were kept. `send_chat_message` sorts last.
#[test]
fn a_named_server_larger_than_the_cap_keeps_the_tools_the_request_points_at() {
    let schema = serde_json::json!({"type":"object","properties":{}});
    let mut mcp = (0..20)
        .map(|index| {
            (
                format!("mcp__teams__inspect_page_{index:02}"),
                "Inspect one part of the page".to_string(),
                schema.clone(),
            )
        })
        .collect::<Vec<_>>();
    mcp.push((
        "mcp__teams__list_chats".to_string(),
        "List the visible chats".to_string(),
        schema.clone(),
    ));
    mcp.push((
        "mcp__teams__send_chat_message".to_string(),
        "Send one message to a visible chat".to_string(),
        schema,
    ));
    let messages = vec![
        serde_json::json!({"role":"user","content":"what is new in teams"}),
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{"function":{"name":"mcp__teams__list_chats","arguments":"{}"}}]
        }),
        serde_json::json!({"role":"user","content":"then send a message in teams: hello"}),
    ];

    let (selected, stats) =
        super::schema::select_mcp_tools_for_context_with_sticky(&mcp, &messages, &[]);
    // Every slot went to the named server, and the stats say so.
    assert_eq!(stats.requested, super::schema::MAX_MCP_NATIVE_SCHEMAS);
    assert_eq!(stats.relevant, 0);
    let names: Vec<&str> = selected
        .iter()
        .map(|index| mcp[*index].0.as_str())
        .collect();
    assert_eq!(names.len(), super::schema::MAX_MCP_NATIVE_SCHEMAS);
    // The tool already in use and the one the words point at are both kept;
    // four of the twenty look-alikes are what the cap drops.
    assert!(names.contains(&"mcp__teams__list_chats"), "{names:?}");
    assert!(
        names.contains(&"mcp__teams__send_chat_message"),
        "{names:?}"
    );
}

#[test]
fn a_discovery_query_is_matched_by_its_words_and_ranked() {
    use super::mcp_discovery::{McpToolDoc, rank};
    let doc = |server: &'static str, name: &'static str, description: &'static str| McpToolDoc {
        server,
        raw_name: name,
        advertised: name,
        description,
    };
    let docs = [
        doc(
            "teams",
            "send_chat_message",
            "Send one message to a visible Teams chat",
        ),
        doc(
            "teams",
            "reply_to_chat_message",
            "Preview or send a quoted reply",
        ),
        doc("audit", "lighthouse_audit", "Run an accessibility audit"),
        doc("pool", "latest_tool", "Current operation"),
        doc("pool", "tool_00", "Archive a historical item"),
    ];
    let ranked = rank("send message chat Teams composer", &docs, None);
    assert_eq!(ranked, [0, 1], "the best match first, unrelated tools out");
    // An exact name outranks a tool that only shares a word, and the floor
    // keeps that tool out of the result.
    assert_eq!(rank("latest_tool", &docs, None), [3]);
    assert_eq!(rank("pool.latest_tool", &docs, None), [3]);
    // Scoping to a server never reaches across it.
    assert!(rank("send message", &docs, Some("audit")).is_empty());
}

#[test]
fn discovery_handoff_trims_server_and_query_like_runtime() {
    let mcp = vec![(
        "mcp__mail__search_messages".to_string(),
        "Find messages by attendee".to_string(),
        serde_json::json!({"type":"object","properties":{}}),
    )];
    let owners = vec!["mail".to_string()];
    let messages = vec![
        serde_json::json!({"role":"user","content":"Compare the annual totals"}),
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{
                "function":{
                    "name":"list_mcp_tools",
                    "arguments":"{\"server\":\" mail \",\"query\":\" attendee \",\"limit\":4}"
                }
            }]
        }),
    ];

    let (selected, _) =
        super::schema::select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
            &mcp,
            &owners,
            &[],
            &messages,
            &[],
            super::schema::ToolSchemaPhase::Established,
        );
    let names: Vec<&str> = selected
        .iter()
        .map(|index| mcp[*index].0.as_str())
        .collect();
    assert_eq!(names, ["mcp__mail__search_messages"]);
}

#[test]
fn discovery_handoff_uses_the_owner_boundary_for_double_underscore_servers() {
    let mcp = vec![(
        "mcp__project__service__lookup".to_string(),
        "Zeta operation".to_string(),
        serde_json::json!({"type":"object","properties":{}}),
    )];
    let owners = vec!["project__service".to_string()];
    let messages = vec![
        serde_json::json!({"role":"user","content":"Do an unrelated action"}),
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{
                "function":{
                    "name":"list_mcp_tools",
                    "arguments":"{\"select\":[\"project__service.lookup\"]}"
                }
            }]
        }),
    ];

    // `server.tool` resolves only when the raw name is split at the owner's
    // boundary (`lookup`), not at the first `__` (`service__lookup`).
    let (selected, _) =
        super::schema::select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
            &mcp,
            &owners,
            &[],
            &messages,
            &[],
            super::schema::ToolSchemaPhase::Established,
        );
    assert_eq!(selected, [0]);
}

#[test]
fn discovery_limit_handoff_orders_colliding_aliases_by_raw_tool_name() {
    let mcp = vec![
        (
            "mcp__alpha___lookup__0000000000000001".to_string(),
            "Lookup records".to_string(),
            serde_json::json!({"type":"object","properties":{}}),
        ),
        (
            "lookup__0".to_string(),
            "Lookup zero records".to_string(),
            serde_json::json!({"type":"object","properties":{}}),
        ),
        (
            "mcp__alpha___lookup__0000000000000002".to_string(),
            "Lookup records".to_string(),
            serde_json::json!({"type":"object","properties":{}}),
        ),
    ];
    let owners = vec![
        "alpha!".to_string(),
        "alpha!".to_string(),
        "alpha_".to_string(),
    ];
    let raw_names = vec![
        "lookup".to_string(),
        "lookup__0".to_string(),
        "lookup".to_string(),
    ];
    let messages = vec![
        serde_json::json!({"role":"user","content":"Find one matching record"}),
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{
                "function":{
                    "name":"list_mcp_tools",
                    "arguments":"{\"query\":\"lookup\",\"limit\":1}"
                }
            }]
        }),
    ];

    let (selected, _) = super::schema::select_mcp_tools_for_context_with_raw_names(
        &mcp,
        &owners,
        &raw_names,
        &[],
        &messages,
        &[],
        super::schema::ToolSchemaPhase::Established,
    );
    let names: Vec<&str> = selected
        .iter()
        .map(|index| mcp[*index].0.as_str())
        .collect();
    // Runtime inventory sorts by server then raw name; lookup is the first
    // result even though its hashed callable alias sorts after lookup__0.
    assert_eq!(names, ["mcp__alpha___lookup__0000000000000001"]);
}

#[test]
fn a_pinned_menu_survives_a_transcript_that_rescores_every_tool() {
    let schema = serde_json::json!({"type":"object","properties":{}});
    // One more name-matching tool than the budget allows, so any change in the
    // score ordering evicts a member and rewrites the whole `tools` block.
    let mut mcp = (0..(MAX_MCP_NATIVE_SCHEMAS + 1))
        .map(|index| {
            (
                format!("mcp__pool__alpha_{index:02}"),
                "Alpha".to_string(),
                schema.clone(),
            )
        })
        .collect::<Vec<_>>();
    // Relevant only once the assistant mentions "bravo". It sorts ahead of every
    // alpha tool, so an unpinned round hands it a slot the pinned menu keeps shut.
    mcp.push((
        "mcp__pool__aaa_bravo".to_string(),
        "Bravo".to_string(),
        schema,
    ));
    let mut messages = vec![serde_json::json!({"role":"user","content":"alpha"})];

    let (round0, stats0) = select_mcp_tools_for_context(&mcp, &messages);
    assert_eq!(stats0.selected, MAX_MCP_NATIVE_SCHEMAS);
    assert!(
        !stats0
            .selected_names
            .contains(&"mcp__pool__aaa_bravo".to_string())
    );

    let used = stats0.selected_names[0].clone();
    messages.push(serde_json::json!({
        "role":"assistant",
        "content":"Retrying down the bravo route.",
        "tool_calls":[{"function":{"name": used}}]
    }));
    messages.push(serde_json::json!({"role":"tool","content":"done"}));

    // Without the pin, the new "bravo" term re-scores the whole pool and two
    // previously selected tools drop out.
    let (_, unpinned) = select_mcp_tools_for_context(&mcp, &messages);
    assert!(
        unpinned
            .selected_names
            .contains(&"mcp__pool__aaa_bravo".to_string())
    );
    assert_ne!(unpinned.selected_names, stats0.selected_names);

    let (round1, stats1) =
        select_mcp_tools_for_context_with_sticky(&mcp, &messages, &stats0.selected_names);
    assert_eq!(stats1.selected_names, stats0.selected_names);
    assert_eq!(round1, round0);
    // A tool the model used in round N is still offered in round N+1.
    assert!(stats1.selected_names.contains(&used));
    // The model called one of the retained tools: the counter must report it.
    assert_eq!(stats1.previously_used, 1);
    // Nothing was admitted by relevance scoring, so it must not claim credit.
    assert_eq!(stats1.relevant, 0);
}

#[test]
fn a_pinned_menu_holds_when_the_schema_budget_is_exhausted() {
    // Real sessions select 13-16 of 485 tools and report
    // `schema_budget_exhausted: true` from round 3 onward, so the pin has to
    // survive a genuinely full budget, not merely a tight one. (#1591)
    let padded = |topic: &str| {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": format!("{topic} {}", "detail ".repeat(300))}
            }
        })
    };
    let mut mcp = (0..40)
        .map(|index| {
            (
                format!("mcp__alpha__review_{index:03}"),
                format!("Review helper {index}"),
                padded("review"),
            )
        })
        .collect::<Vec<_>>();
    // Scores 3 in round 0 (a description-only "review" match) and 11 once the
    // assistant names it, which outranks every review tool on the next round.
    mcp.push((
        "mcp__alpha__widget_000".to_string(),
        "Widget review helper".to_string(),
        padded("widget"),
    ));
    let mut messages = vec![serde_json::json!({"role":"user","content":"review"})];

    let (round0, stats0) = select_mcp_tools_for_context(&mcp, &messages);
    assert!(stats0.schema_budget_exhausted);
    assert!(stats0.selected < MAX_MCP_NATIVE_SCHEMAS);
    let widget = "mcp__alpha__widget_000".to_string();
    assert!(!stats0.selected_names.contains(&widget));

    let used = stats0.selected_names[0].clone();
    messages.push(serde_json::json!({
        "role":"assistant",
        "content":"The widget path looks better.",
        "tool_calls":[{"function":{"name": used}}]
    }));
    messages.push(serde_json::json!({"role":"tool","content":"done"}));

    // Unpinned, the new term wins a slot the pin is holding and the byte budget
    // then decides a different eviction, so the whole `tools` block changes.
    let (_, unpinned) = select_mcp_tools_for_context(&mcp, &messages);
    assert!(unpinned.selected_names.contains(&widget));
    assert_ne!(unpinned.selected_names, stats0.selected_names);

    // Pinned, the menu is identical even though the budget is still exhausted:
    // retention is a retention guarantee, not a preference competing for slots.
    let (round1, stats1) =
        select_mcp_tools_for_context_with_sticky(&mcp, &messages, &stats0.selected_names);
    assert_eq!(round1, round0);
    assert_eq!(stats1.selected_names, stats0.selected_names);
    assert_eq!(stats1.mcp_schema_bytes, stats0.mcp_schema_bytes);
    assert_eq!(stats1.previously_used, 1);
    assert_eq!(stats1.relevant, 0);
}

#[test]
fn previously_used_counts_a_tool_admitted_by_a_server_reservation() {
    let tools = vec![
        (
            "mail_send_email".to_string(),
            "Send an email by UID".to_string(),
            serde_json::json!({"type":"object","properties":{}}),
        ),
        (
            "mail_list_emails".to_string(),
            "List recent emails".to_string(),
            serde_json::json!({"type":"object","properties":{}}),
        ),
    ];
    let owners = vec!["mail".to_string(), "mail".to_string()];
    let messages = vec![
        serde_json::json!({"role":"user","content":"Send an email"}),
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{"function":{"name":"mail_send_email"}}]
        }),
        serde_json::json!({"role":"tool","content":"sent"}),
    ];

    let (selected, stats) =
        super::schema::select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
            &tools,
            &owners,
            &["mail".to_string()],
            &messages,
            &[],
            ToolSchemaPhase::Established,
        );
    let names = selected
        .iter()
        .map(|index| tools[*index].0.as_str())
        .collect::<Vec<_>>();
    // The reservation bound the whole mail toolset before the candidate loop ran,
    // so the used tool was already selected and the old increment sat behind the
    // dedup `continue` — it reported 0 on every request. (#1591)
    assert_eq!(names, ["mail_send_email", "mail_list_emails"]);
    assert_eq!(stats.reserved_servers, ["mail"]);
    assert_eq!(stats.previously_used, 1);
}

#[test]
fn withheld_tools_are_named_in_the_round_notice() {
    let notice = super::withheld_tools_notice(&super::McpSchemaSelectionStats {
        withheld_builtin_names: vec!["search_web".to_string(), "remember".to_string()],
        available: 485,
        selected: 13,
        ..Default::default()
    })
    .expect("withheld tools must produce a notice");
    assert!(notice.contains("search_web, remember"));
    assert!(notice.contains("472 registered MCP tools"));
    assert!(notice.contains("list_mcp_tools"));

    assert!(
        super::withheld_tools_notice(&super::McpSchemaSelectionStats {
            available: 13,
            selected: 13,
            ..Default::default()
        })
        .is_none(),
        "an unfiltered request must not spend tokens on the notice"
    );
}

#[test]
fn mcp_schema_selection_avoids_arbitrary_zero_relevance_fallback() {
    let mcp: Vec<_> = (0..(MAX_MCP_NATIVE_SCHEMAS + 4))
        .map(|index| {
            (
                format!("tool_{index:02}"),
                "No matching task description".to_string(),
                serde_json::json!({"type":"object","properties":{}}),
            )
        })
        .collect();
    let messages = vec![serde_json::json!({"role":"user","content":"unmatched request"})];

    let (selected, stats) = select_mcp_tools_for_context(&mcp, &messages);
    assert!(
        selected.is_empty(),
        "zero relevance must not expose arbitrary MCP tools"
    );
    assert_eq!(stats.fallback, 0);
    assert_eq!(stats.omitted, mcp.len());
    assert!(stats.selected_names.is_empty());
}

#[test]
fn mcp_catalog_result_does_not_make_every_listed_tool_previously_used() {
    let mut tools = (0..MAX_MCP_NATIVE_SCHEMAS)
        .map(|index| {
            (
                format!("discord_tool_{index:02}"),
                "Unrelated Discord operation".to_string(),
                serde_json::json!({"type":"object"}),
            )
        })
        .collect::<Vec<_>>();
    tools.push((
        "read_email".to_string(),
        "Read an email by uid".to_string(),
        serde_json::json!({"type":"object"}),
    ));
    let catalog = tools
        .iter()
        .map(|(name, _, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let messages = vec![
        serde_json::json!({"role":"user","content":"Read the newest email"}),
        serde_json::json!({"role":"tool","content":catalog}),
    ];
    let (selected, stats) = select_mcp_tools_for_context(&tools, &messages);
    assert!(selected.contains(&MAX_MCP_NATIVE_SCHEMAS));
    assert_eq!(stats.previously_used, 0);
}

#[test]
fn mcp_server_suffix_alias_pins_the_requested_server() {
    let tools = vec![
        (
            "mcp__mail__read_email".to_string(),
            "Read an email".to_string(),
            serde_json::json!({"type":"object"}),
        ),
        (
            "mcp__mail__search_emails".to_string(),
            "Search email".to_string(),
            serde_json::json!({"type":"object"}),
        ),
    ];
    let messages = vec![serde_json::json!({
        "role":"user",
        "content":"Check whether mail_mcp works"
    })];
    let (selected, _) = select_mcp_tools_for_context(&tools, &messages);
    assert_eq!(selected, vec![0, 1]);
}

#[test]
fn natural_latest_mails_request_binds_inbox_listing_among_unrelated_servers() {
    let mut tools = (0..MAX_MCP_NATIVE_SCHEMAS)
        .map(|index| {
            (
                format!("codebase_latest_{index:02}"),
                "Unrelated operation".to_string(),
                serde_json::json!({"type":"object"}),
            )
        })
        .collect::<Vec<_>>();
    tools.extend([
        (
            "mcp__mail__list_attachments".to_string(),
            "List attachments for a known email".to_string(),
            serde_json::json!({"type":"object"}),
        ),
        (
            "search_emails".to_string(),
            "Search emails by free text".to_string(),
            serde_json::json!({"type":"object"}),
        ),
        (
            "reply_email".to_string(),
            "Reply to an email by UID".to_string(),
            serde_json::json!({"type":"object"}),
        ),
        (
            "send_email".to_string(),
            "Send a new email".to_string(),
            serde_json::json!({"type":"object"}),
        ),
        (
            "list_emails".to_string(),
            "List recent emails from a folder".to_string(),
            serde_json::json!({"type":"object"}),
        ),
    ]);
    let mut messages = vec![serde_json::json!({
        "role":"user","content":"what are my latest mails"
    })];
    let (_, stats) = select_mcp_tools_for_context(&tools, &messages);
    assert!(
        stats.selected_names.contains(&"list_emails".to_string()),
        "latest mails should bind list_emails before catalog discovery"
    );

    messages.push(serde_json::json!({
        "role":"tool","content":"registry catalog lists list_emails"
    }));
    let (_, stats) = select_mcp_tools_for_context(&tools, &messages);
    assert!(
        stats.selected_names.contains(&"list_emails".to_string()),
        "list_emails should remain callable after discovery"
    );
}

#[test]
fn runtime_context_server_names_do_not_displace_requested_mail_tools() {
    let mut tools = (0..MAX_MCP_NATIVE_SCHEMAS)
        .map(|index| {
            (
                format!("mcp__discord__tool_{index:02}"),
                "Unrelated operation".to_string(),
                serde_json::json!({"type":"object"}),
            )
        })
        .collect::<Vec<_>>();
    tools.push((
        "mcp__mail__read_email".to_string(),
        "Read an email".to_string(),
        serde_json::json!({"type":"object"}),
    ));
    let messages = vec![serde_json::json!({
        "role":"user",
        "content":"Check mail_mcp reads\n<rustcode_context>\nConnected servers: discord, mail\n</rustcode_context>"
    })];
    let (selected, stats) = select_mcp_tools_for_context(&tools, &messages);
    assert_eq!(selected, vec![MAX_MCP_NATIVE_SCHEMAS]);
    assert_eq!(stats.selected_names, vec!["mcp__mail__read_email"]);
}

#[test]
fn mcp_schema_selection_enforces_a_measured_schema_byte_budget() {
    let large_description = "x".repeat(MAX_MCP_NATIVE_SCHEMA_BYTES);
    let tools = vec![
        (
            "first_tool".to_string(),
            large_description,
            serde_json::json!({"type":"object","properties":{}}),
        ),
        (
            "second_tool".to_string(),
            "small relevant tool".to_string(),
            serde_json::json!({"type":"object","properties":{}}),
        ),
    ];
    let messages = vec![serde_json::json!({
        "role": "user",
        "content": "Use the first_tool and second_tool"
    })];

    let (_, stats) = select_mcp_tools_for_context(&tools, &messages);
    assert!(stats.mcp_schema_bytes <= stats.mcp_schema_budget_bytes);
    assert!(stats.schema_budget_exhausted);
    assert!(stats.selected_names.contains(&"second_tool".to_string()));
}

#[test]
fn mcp_per_server_cap_prevents_one_heavy_server_starving_the_rest() {
    use super::schema::MAX_MCP_PER_SERVER_SCHEMA_BYTES;
    // One heavy server with large descriptions vs one light server; both
    // relevant. Without per-server fairness the heavy server consumes the
    // whole budget and the light server is dropped (#1633).
    let heavy_desc = "x".repeat(MAX_MCP_PER_SERVER_SCHEMA_BYTES);
    let mut tools = Vec::new();
    let mut owners = Vec::new();
    for i in 0..6 {
        tools.push((
            format!("heavy_tool_{i}"),
            format!("heavy relevant {heavy_desc}"),
            serde_json::json!({"type":"object","properties":{}}),
        ));
        owners.push("heavy".to_string());
    }
    for i in 0..4 {
        tools.push((
            format!("light_tool_{i}"),
            "light relevant tool".to_string(),
            serde_json::json!({"type":"object","properties":{}}),
        ));
        owners.push("light".to_string());
    }
    let messages = vec![serde_json::json!({
        "role": "user",
        "content": "heavy relevant light relevant"
    })];
    let (selected, stats) =
        super::schema::select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
            &tools,
            &owners,
            &[],
            &messages,
            &[],
            ToolSchemaPhase::Established,
        );
    let names: std::collections::HashSet<&str> =
        selected.iter().map(|i| tools[*i].0.as_str()).collect();
    assert!(
        names.iter().any(|n| n.starts_with("light_tool_")),
        "light server must survive heavy server pressure: {names:?}"
    );
    assert!(stats.mcp_schema_bytes <= stats.mcp_schema_budget_bytes);
}

#[test]
fn mcp_always_include_reserves_a_complete_server_toolset_within_the_native_cap() {
    let mut tools = Vec::new();
    let mut owners = Vec::new();
    for name in [
        "list_emails",
        "send_email",
        "reply_email",
        "download_attachment",
        "search_emails",
        "create_draft",
        "delete_email",
    ] {
        tools.push((
            name.to_string(),
            "Email operation".to_string(),
            serde_json::json!({"type":"object","properties":{}}),
        ));
        owners.push("mail".to_string());
    }
    for index in 0..110 {
        let server = format!("server_{}", index % 6);
        tools.push((
            format!("{server}_discord_tool_{index:03}"),
            "Browse Discord channels and threads".to_string(),
            serde_json::json!({"type":"object","properties":{}}),
        ));
        owners.push(server);
    }
    let messages = vec![serde_json::json!({
        "role":"user",
        "content":"Browse Discord channels and threads"
    })];

    let (selected, stats) =
        super::schema::select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
            &tools,
            &owners,
            &["mail".to_string()],
            &messages,
            &[],
            ToolSchemaPhase::Established,
        );
    let selected_names = selected
        .iter()
        .map(|index| tools[*index].0.as_str())
        .collect::<std::collections::HashSet<_>>();

    assert_eq!(tools.len(), 117);
    assert_eq!(selected.len(), MAX_MCP_NATIVE_SCHEMAS);
    for name in [
        "list_emails",
        "send_email",
        "reply_email",
        "download_attachment",
        "search_emails",
        "create_draft",
        "delete_email",
    ] {
        assert!(
            selected_names.contains(name),
            "reserved mail tool {name} missing"
        );
    }
    assert_eq!(stats.reserved_servers, ["mail"]);
    assert!(stats.rejected_reservations.is_empty());
    assert_eq!(stats.omitted_names.len(), 117 - MAX_MCP_NATIVE_SCHEMAS);
}

#[test]
fn mcp_always_include_reports_a_reservation_that_exceeds_the_tool_cap() {
    let mut tools = Vec::new();
    let mut owners = Vec::new();
    for index in 0..(MAX_MCP_NATIVE_SCHEMAS + 1) {
        tools.push((
            format!("oversize_tool_{index:02}"),
            "Send email using this mail service".to_string(),
            serde_json::json!({"type":"object","properties":{}}),
        ));
        owners.push("oversize".to_string());
    }
    let messages = vec![serde_json::json!({
        "role":"user",
        "content":"Send email using the mail service"
    })];

    let (selected, stats) =
        super::schema::select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
            &tools,
            &owners,
            &["oversize".to_string()],
            &messages,
            &[],
            ToolSchemaPhase::Established,
        );

    assert!(selected.is_empty());
    assert!(stats.reserved_servers.is_empty());
    assert_eq!(stats.rejected_reservations, ["oversize"]);
    assert_eq!(stats.omitted_names.len(), tools.len());
}

#[test]
fn mcp_always_include_rejects_a_server_toolset_that_exceeds_the_schema_byte_budget() {
    let tools = vec![
        (
            "first_tool".to_string(),
            format!(
                "Send email {}",
                "x".repeat(super::schema::MAX_MCP_NATIVE_SCHEMA_BYTES / 2)
            ),
            serde_json::json!({"type":"object","properties":{}}),
        ),
        (
            "second_tool".to_string(),
            format!(
                "Send email {}",
                "x".repeat(super::schema::MAX_MCP_NATIVE_SCHEMA_BYTES / 2)
            ),
            serde_json::json!({"type":"object","properties":{}}),
        ),
    ];
    let owners = vec!["large".to_string(), "large".to_string()];
    let messages = vec![serde_json::json!({"role":"user","content":"Send email"})];

    let (selected, stats) =
        super::schema::select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
            &tools,
            &owners,
            &["large".to_string()],
            &messages,
            &[],
            ToolSchemaPhase::Established,
        );

    assert!(selected.is_empty());
    assert_eq!(stats.rejected_reservations, ["large"]);
    assert!(stats.schema_budget_exhausted);
    assert!(stats.mcp_schema_bytes <= stats.mcp_schema_budget_bytes);
}

#[test]
fn function_words_in_a_prompt_do_not_select_unrelated_mcp_tools() {
    let tools = [
        (
            "mcp__snapchat__send_chat_message",
            "Send a chat message to one of the friends of the account. You can attach media.",
        ),
        (
            "take_heapsnapshot",
            "Take a heap snapshot of the page and save it. Use it to find leaks.",
        ),
        (
            "get_css_styles",
            "Get the computed styles of an element and what rules they come from.",
        ),
        (
            "disk_usage_report",
            "Report how much space each directory takes on a volume.",
        ),
    ]
    .into_iter()
    .map(|(name, description)| {
        (
            name.to_string(),
            description.to_string(),
            serde_json::json!({"type":"object","properties":{}}),
        )
    })
    .collect::<Vec<_>>();
    let messages = vec![serde_json::json!({
        "role": "user",
        "content": "check how much total disk space i have, what takes alot of disk usage and whan I can delete",
    })];
    let (selected, stats) =
        select_mcp_tools_for_context_in_phase(&tools, &messages, ToolSchemaPhase::Bootstrap);
    assert_eq!(stats.selected_names, ["disk_usage_report"]);
    assert_eq!(selected.len(), 1);
}

#[test]
fn bootstrap_mcp_selection_does_not_flood_empty_projects_with_discovery_tools() {
    let tools = (0..8)
        .map(|index| {
            (
                format!("codebase_tool_{index}"),
                "Generic code project result".to_string(),
                serde_json::json!({"type":"object","properties":{}}),
            )
        })
        .collect::<Vec<_>>();
    let messages = vec![serde_json::json!({"role":"user","content":"Create a project"})];
    let (selected, stats) =
        select_mcp_tools_for_context_in_phase(&tools, &messages, ToolSchemaPhase::Bootstrap);
    assert!(selected.is_empty());
    assert_eq!(stats.fallback, 0);
    assert_eq!(stats.omitted, tools.len());
    assert_eq!(stats.phase, ToolSchemaPhase::Bootstrap);
}

#[test]
fn mcp_discovery_prefers_the_socraticode_core_without_schema_flooding() {
    let names = [
        "codebase_about",
        "codebase_context_index",
        "codebase_flow",
        "codebase_graph_visualize",
        "codebase_impact",
        "codebase_search",
        "codebase_status",
        "codebase_symbols",
    ];
    let tools = names
        .iter()
        .map(|name| {
            (
                (*name).to_string(),
                "Generic code project result".to_string(),
                serde_json::json!({"type":"object","properties":{}}),
            )
        })
        .collect::<Vec<_>>();
    let messages = vec![serde_json::json!({
        "role":"user",
        "content":"Create result.txt in this repository"
    })];

    let (selected, stats) = select_mcp_tools_for_context(&tools, &messages);
    assert_eq!(selected.len(), MCP_DISCOVERY_FALLBACK_COUNT);
    assert_eq!(stats.relevant, 0);
    assert_eq!(
        stats.selected_names,
        vec![
            "codebase_flow",
            "codebase_impact",
            "codebase_search",
            "codebase_symbols"
        ]
    );
}

#[test]
fn mcp_schema_selection_retains_sticky_tools_and_adds_newly_relevant_tools() {
    let mcp = vec![
        (
            "search_issues".to_string(),
            "Search GitHub issues".to_string(),
            serde_json::json!({"type":"object"}),
        ),
        (
            "weather_forecast".to_string(),
            "Get a weather forecast".to_string(),
            serde_json::json!({"type":"object"}),
        ),
    ];
    let messages = vec![serde_json::json!({
        "role":"user",
        "content":"Deploy the release"
    })];

    let (selected, stats) = select_mcp_tools_for_context_with_sticky(
        &mcp,
        &messages,
        &["weather_forecast".to_string()],
    );

    assert_eq!(selected, vec![1]);
    assert_eq!(stats.selected_names, vec!["weather_forecast"]);

    let messages = vec![serde_json::json!({
        "role":"user",
        "content":"Search issues"
    })];
    let (selected, stats) = select_mcp_tools_for_context_with_sticky(
        &mcp,
        &messages,
        &["weather_forecast".to_string()],
    );
    // The pinned tool keeps its seat; the new one is appended after it.
    assert_eq!(selected, vec![1, 0]);
    assert_eq!(
        stats.selected_names,
        vec!["weather_forecast", "search_issues"]
    );
}

#[test]
fn explicitly_named_mcp_server_tools_are_pinned_across_a_wrong_tool_round() {
    let distractor_terms = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda";
    let mut mcp = (0..16)
        .map(|index| {
            (
                format!("mcp__other_server__tool_{index:02}"),
                distractor_terms.to_string(),
                serde_json::json!({"type":"object","properties":{}}),
            )
        })
        .collect::<Vec<_>>();
    mcp.extend([
        (
            "mcp__requested_server__alpha".to_string(),
            "Unrelated tool".to_string(),
            serde_json::json!({"type":"object","properties":{}}),
        ),
        (
            "mcp__requested_server__beta".to_string(),
            "Unrelated tool".to_string(),
            serde_json::json!({"type":"object","properties":{}}),
        ),
    ]);
    let messages = vec![serde_json::json!({
        "role": "user",
        "content": "Use MCP server requested_server for alpha beta gamma delta epsilon zeta eta theta iota kappa lambda"
    })];

    let (first, first_stats) = select_mcp_tools_for_context(&mcp, &messages);
    assert!(first_stats.selected <= MAX_MCP_NATIVE_SCHEMAS);
    assert!(first.contains(&16));
    assert!(first.contains(&17));

    let follow_up = [
        messages[0].clone(),
        serde_json::json!({
            "role": "assistant",
            "tool_calls": [{"function":{"name":"grep"}}]
        }),
        serde_json::json!({"role":"tool","content":"grep completed"}),
    ];
    let (second, second_stats) =
        select_mcp_tools_for_context_with_sticky(&mcp, &follow_up, &first_stats.selected_names);
    assert!(second_stats.selected <= MAX_MCP_NATIVE_SCHEMAS);
    assert!(second.contains(&16));
    assert!(second.contains(&17));

    let old_turn_menu = mcp[..MAX_MCP_NATIVE_SCHEMAS]
        .iter()
        .map(|(name, _, _)| name.clone())
        .collect::<Vec<_>>();
    let (new_turn, new_turn_stats) =
        select_mcp_tools_for_context_with_sticky(&mcp, &messages, &old_turn_menu);
    assert!(new_turn_stats.selected <= MAX_MCP_NATIVE_SCHEMAS);
    assert!(new_turn.contains(&16));
    assert!(new_turn.contains(&17));
}

#[test]
fn explicitly_named_mcp_tool_is_pinned_before_relevance_selection() {
    let distractor_terms = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda";
    let mut mcp = (0..16)
        .map(|index| {
            (
                format!("mcp__other_server__tool_{index:02}"),
                distractor_terms.to_string(),
                serde_json::json!({"type":"object","properties":{}}),
            )
        })
        .collect::<Vec<_>>();
    mcp.push((
        "mcp__requested_server__send_email".to_string(),
        "Unrelated tool".to_string(),
        serde_json::json!({"type":"object","properties":{}}),
    ));
    let messages = vec![serde_json::json!({
        "role": "user",
        "content": "Call mcp__requested_server__send_email"
    })];

    let (selected, stats) = select_mcp_tools_for_context(&mcp, &messages);
    assert_eq!(selected.len(), MAX_MCP_NATIVE_SCHEMAS);
    assert!(selected.contains(&16));
    assert!(
        stats
            .selected_names
            .contains(&"mcp__requested_server__send_email".to_string())
    );
}

#[test]
fn native_tools_schema_requires_explicit_delegation() {
    let disabled = native_tools_schema(false);
    let enabled = native_tools_schema(true);
    assert!(disabled.iter().all(|t| {
        !matches!(
            t["function"]["name"].as_str(),
            Some("spawn_agent") | Some("send_agent") | Some("wait_agent") | Some("cancel_agent")
        )
    }));
    assert!(
        enabled
            .iter()
            .any(|t| t["function"]["name"] == "spawn_agent")
    );
    assert!(
        enabled
            .iter()
            .any(|t| t["function"]["name"] == "send_agent")
    );
}

#[test]
fn request_schema_policy_isolates_subagents_from_parent_delegation() {
    let root = native_tools_schema_for_context(ToolSchemaPolicy::root(true), &[]).0;
    let child = native_tools_schema_for_context(ToolSchemaPolicy::subagent(), &[]).0;
    let root_names = root
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect::<Vec<_>>();
    let child_names = child
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect::<Vec<_>>();

    assert!(root_names.contains(&"spawn_agent"));
    assert!(root_names.contains(&"wait_agent"));
    assert!(child_names.contains(&"run_command"));
    assert!(!child_names.iter().any(|name| is_agent_tool(name)));
    assert!(ToolSchemaPolicy::subagent().include_mcp_tools);
    assert!(ToolSchemaPolicy::root(true).include_mcp_tools);
}

#[test]
fn subagents_get_the_parents_tools_cut_only_by_write_access() {
    let names = |policy| {
        native_tools_schema_for_context(policy, &[])
            .0
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
            .collect::<Vec<_>>()
    };
    let root = names(ToolSchemaPolicy::root(true));
    let reader = names(ToolSchemaPolicy::for_subagent(false));
    let writer = names(ToolSchemaPolicy::for_subagent(true));
    for policy in [
        ToolSchemaPolicy::for_subagent(false),
        ToolSchemaPolicy::for_subagent(true),
    ] {
        assert!(policy.include_mcp_tools);
    }
    // #2334: an explorer had no web tools and fell back to a refused `curl`.
    for name in [
        "search_web",
        "read_url",
        "list_skills",
        "use_skill",
        "list_mcp_tools",
        "recall_memory",
        "git_diff",
        "view_file",
        "run_command",
    ] {
        assert!(root.iter().any(|n| n == name), "{name}");
        assert!(reader.iter().any(|n| n == name), "reader lacks {name}");
        assert!(writer.iter().any(|n| n == name), "writer lacks {name}");
    }
    for name in [
        "write_to_file",
        "replace_file_content",
        "delete_file",
        "git_commit",
        "manage_task",
    ] {
        assert!(!reader.iter().any(|n| n == name), "reader has {name}");
        assert!(writer.iter().any(|n| n == name), "writer lacks {name}");
    }
    // Session-level tools stay with the root agent.
    for name in [
        "complete_task",
        "ask_question",
        "set_session_title",
        "set_goal",
        "todo_write",
    ] {
        assert!(!reader.iter().any(|n| n == name), "reader has {name}");
        assert!(!writer.iter().any(|n| n == name), "writer has {name}");
    }
    assert!(reader.iter().any(|n| n == "spawn_agent"));
    assert!(!writer.iter().any(|n| is_agent_tool(n)));
}

#[test]
fn read_only_subagent_shell_surface_matches_native_and_text_protocols() {
    let policy = ToolSchemaPolicy::subagent_with_delegation();
    let native = native_tools_schema_for_context(policy, &[]).0;
    let names = native
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"run_command"));
    assert!(names.contains(&"spawn_agent"));
    assert!(!names.contains(&"write_to_file"));
    assert!(!names.contains(&"set_goal"));
    assert!(!names.contains(&"todo_write"));

    let text = tool_system_prompt_for_policy(
        policy,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );
    assert!(text.contains("- run_command |"));
    assert!(text.contains("- spawn_agent |"));
    assert!(!text.contains("- write_to_file |"));
    assert!(!text.contains("- set_goal |"));
    assert!(!text.contains("- todo_write |"));

    let plan_text = tool_system_prompt_for_policy(
        ToolSchemaPolicy::read_only_inspection(),
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Plan,
    );
    assert!(!plan_text.contains("- run_command |"));
}

#[test]
fn read_only_inspection_profile_is_small_and_deterministic() {
    let policy = ToolSchemaPolicy::read_only_inspection();
    let first = native_tools_schema_for_context(policy, &[]).0;
    let second = native_tools_schema_for_context(policy, &[]).0;
    assert_eq!(first, second);
    let names = first
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "grep",
            "glob",
            "list_directory",
            "find_symbol",
            "get_project_map",
            "view_file",
            "zoom_context"
        ]
    );

    let subagent = native_tools_schema_for_context(ToolSchemaPolicy::subagent(), &[]).0;
    let subagent_names = subagent
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect::<Vec<_>>();
    assert!(subagent_names.contains(&"run_command"));
    assert!(!subagent_names.iter().any(|name| is_agent_tool(name)));
}

#[test]
fn session_title_tool_is_first_turn_only_in_native_and_text_menus() {
    let regular = ToolSchemaPolicy::root(false);
    let first_turn = regular.with_session_title_tool();

    let regular_native = native_tools_schema_for_context(regular, &[]).0;
    let first_turn_native = native_tools_schema_for_context(first_turn, &[]).0;
    let native_has_title = |schemas: &[serde_json::Value]| {
        schemas
            .iter()
            .any(|tool| tool["function"]["name"] == "set_session_title")
    };
    assert!(!native_has_title(&regular_native));
    assert!(native_has_title(&first_turn_native));

    for protocol in [
        crate::config::ToolProtocol::Json,
        crate::config::ToolProtocol::Native,
    ] {
        let regular_prompt =
            tool_system_prompt_for_policy(regular, protocol, crate::config::AgentMode::Build);
        let first_turn_prompt =
            tool_system_prompt_for_policy(first_turn, protocol, crate::config::AgentMode::Build);
        // The system prompt stays byte-identical across turns so the cached
        // prefix survives; the first-turn guidance rides in the context tail.
        assert_eq!(regular_prompt, first_turn_prompt);
        assert!(!first_turn_prompt.contains("set_session_title"));
        let hint = super::schema::session_title_turn_hint(protocol);
        assert!(hint.contains("Do not copy the full prompt"));
        assert!(hint.contains("- set_session_title | Args: "));
    }

    for agent_mode in [
        crate::config::AgentMode::Build,
        crate::config::AgentMode::Plan,
    ] {
        assert_eq!(
            tool_system_prompt_for_policy(
                regular,
                crate::config::ToolProtocol::ApiNative,
                agent_mode
            ),
            tool_system_prompt_for_policy(
                first_turn,
                crate::config::ToolProtocol::ApiNative,
                agent_mode
            )
        );
    }
    let native_hint =
        super::schema::session_title_turn_hint(crate::config::ToolProtocol::ApiNative);
    assert!(native_hint.contains("set_session_title"));
    assert!(!native_hint.contains("| Args:"));

    let plan_prompt = tool_system_prompt_for_policy(
        first_turn,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Plan,
    );
    assert!(!plan_prompt.contains("set_session_title"));
}

#[test]
fn session_title_tool_schema_bounds_and_rejects_oversized_calls() {
    let schema = super::misc::SET_SESSION_TITLE.schema;
    let schema = schema();
    assert_eq!(schema["properties"]["title"]["minLength"], 1);
    assert_eq!(
        schema["properties"]["title"]["maxLength"],
        super::misc::MAX_SESSION_TITLE_CHARS
    );
    assert_eq!(schema["required"], serde_json::json!(["title"]));
    assert_eq!(schema["additionalProperties"], false);

    let call = ToolCall {
        name: "set_session_title".to_string(),
        arguments: serde_json::json!({
            "title": "x".repeat(super::misc::MAX_SESSION_TITLE_CHARS + 1)
        }),
        call_id: None,
    };
    let error = validate_tool_calls(&[call], 4).expect_err("oversized title must be rejected");
    assert!(error.contains("set_session_title"));
    assert!(error.contains("at most"));

    for invalid in ["   ", "line one\nline two"] {
        let error =
            (super::misc::SET_SESSION_TITLE.handler)(&serde_json::json!({"title": invalid}))
                .expect_err("unsafe title must be rejected");
        assert!(error.contains("title"));
    }
}

#[test]
fn edit_aliases_outside_the_advertised_properties_still_validate() {
    // The edit schema lists few names; its other aliases come in through
    // `patternProperties` and must pass validation, while a real typo fails.
    let call = |arguments: serde_json::Value| ToolCall {
        name: "replace_file_content".to_string(),
        arguments,
        call_id: None,
    };
    validate_tool_calls(
        &[call(serde_json::json!({
            "path": "src/a.rs", "oldText": "old", "newText": "new"
        }))],
        1,
    )
    .expect("a root alias pair must validate");
    validate_tool_calls(
        &[call(serde_json::json!({
            "path": "src/a.rs", "edits": [{"target": "old", "replacement": "new"}]
        }))],
        1,
    )
    .expect("an edits[] alias pair must validate");
    let error = validate_tool_calls(
        &[call(
            serde_json::json!({"path": "src/a.rs", "replacements": []}),
        )],
        1,
    )
    .expect_err("an unknown key must still be rejected");
    assert!(error.contains("replacements"), "got: {error}");
}

#[test]
fn a_complete_24_kib_write_is_validated_and_written() {
    // Issue #1886: 24,108 bytes of complete content were rejected after the
    // model had generated them, only because they were over 16 KiB.
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("src/main.rs");
    let content = "let x = 1;\n".repeat(24 * 1024 / 11);
    assert!(content.len() > rustcode_tools::filesystem::MAX_FILE_CHUNK_BYTES);
    let arguments = serde_json::json!({"path": path, "content": content});
    let call = ToolCall {
        name: "write_to_file".to_string(),
        arguments: arguments.clone(),
        call_id: None,
    };
    validate_tool_calls(&[call], 1).expect("a complete large write must not be rejected");

    set_active_workspace_context(
        Some(workspace.path().into()),
        Some(workspace.path().into()),
        false,
        Some(crate::config::SandboxMode::Trusted),
    );
    let write = execute_with_metadata("write_to_file", &arguments);
    set_active_workspace_context(None, None, false, None);
    assert!(write.success, "{}", write.content);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
}

#[test]
fn a_truncated_24_kib_write_is_rejected_and_never_written() {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("src/main.rs");
    let complete = serde_json::json!({
        "path": path,
        "content": "let x = 1;\n".repeat(24 * 1024 / 11),
    })
    .to_string();
    // The provider stops mid-string: the JSON arguments never close.
    let truncated = &complete[..complete.len() - 4096];

    let arguments = crate::network::parse_native_tool_arguments(truncated);
    assert!(arguments.get("content").is_none(), "no partial content");
    let call = ToolCall {
        name: "write_to_file".to_string(),
        arguments,
        call_id: None,
    };
    let error = validate_tool_calls(&[call], 1).expect_err("a cut-off write must be rejected");
    assert!(error.contains("malformed arguments"), "{error}");
    assert!(error.contains("No tool was executed"), "{error}");
    assert!(!path.exists());

    // The textual protocols see the same cut as an unclosed envelope, which
    // is continued or reissued and never handed to a handler.
    let textual = format!("[TOOL_CALLS]write_to_file[ARGS]{truncated}");
    assert!(has_incomplete_actionable_tool_call(&textual));
}

#[test]
fn the_write_tool_spec_states_the_remaining_hard_limit() {
    let spec = TOOLS
        .iter()
        .find(|tool| tool.name == "write_to_file")
        .expect("tool exists");
    let limit = format!(
        "{} KiB of JSON arguments",
        crate::network::stream_request::MAX_NATIVE_TOOL_ARGUMENT_BYTES / 1024
    );
    assert!(spec.description.contains(&limit), "{}", spec.description);
    assert!(spec.arguments.contains(&limit), "{}", spec.arguments);
    assert!(!spec.description.contains("16 KiB"));
    assert!(!spec.arguments.contains("16384"));
}

#[test]
fn inspection_schemas_use_strict_typed_arguments() {
    let schemas = native_tools_schema_for_context(ToolSchemaPolicy::read_only_inspection(), &[]).0;
    for tool in schemas {
        let parameters = &tool["function"]["parameters"];
        assert_eq!(parameters["type"], "object");
        assert_eq!(parameters["additionalProperties"], false, "{tool:?}");
    }
    let view = native_tools_schema_for_context(ToolSchemaPolicy::read_only_inspection(), &[])
        .0
        .into_iter()
        .find(|tool| tool["function"]["name"] == "view_file")
        .expect("view_file schema");
    assert_eq!(
        view["function"]["parameters"]["properties"]["start_line"]["type"],
        "integer"
    );
    assert_eq!(
        view["function"]["parameters"]["properties"]["end_line"]["type"],
        "integer"
    );
}

fn native_names(schemas: &[serde_json::Value]) -> Vec<String> {
    schemas
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn api_native_builtin_selection_is_policy_only_and_protocol_agnostic() {
    let coding_messages = vec![serde_json::json!({
        "role": "user",
        "content": "Implement the parser fix, edit the Rust files, and run tests"
    })];
    let research_messages = vec![serde_json::json!({
        "role": "user",
        "content": "Search the web and remember what you find"
    })];
    let coding = native_names(
        &native_tools_schema_for_context(ToolSchemaPolicy::root(false), &coding_messages).0,
    );
    let research = native_names(
        &native_tools_schema_for_context(ToolSchemaPolicy::root(false), &research_messages).0,
    );

    for required in [
        "grep",
        "view_file",
        "replace_file_content",
        "run_command",
        "complete_task",
        "search_web",
        "get_time",
        "remember",
    ] {
        assert!(coding.contains(&required.to_string()), "missing {required}");
    }
    // Keyword filtering silently hid 16 of 37 built-ins from every request and
    // let one assistant word flip five of them on for the rest of the session.
    // Membership is now a pure function of the policy. (#1589)
    assert_eq!(coding, research);

    let (_, stats) =
        native_tools_schema_for_context(ToolSchemaPolicy::root(false), &coding_messages);
    assert_eq!(stats.builtin_selected, stats.builtin_available);
    assert!(stats.withheld_builtin_names.is_empty());
}

#[test]
fn textual_and_native_protocols_advertise_the_same_builtin_tools() {
    let policy = ToolSchemaPolicy::root(false);
    let native = native_names(&native_tools_schema_for_context(policy, &[]).0);
    let textual = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );
    let advertised = super::TOOLS
        .iter()
        .map(|tool| tool.name)
        .filter(|name| textual.contains(&format!("- {name} | Args:")))
        .collect::<Vec<_>>();
    assert_eq!(advertised, native);
}

#[test]
fn builtin_tool_selection_is_stable_across_consecutive_turns() {
    let mut messages = vec![serde_json::json!({
        "role": "user",
        "content": "Fix the parser and rerun the tests"
    })];
    let first =
        native_names(&native_tools_schema_for_context(ToolSchemaPolicy::root(false), &messages).0);
    // Assistant prose used to feed `context_terms`, so one incidental "delete"
    // flipped delete_file/move_file/copy_file on for the rest of the session.
    messages.push(serde_json::json!({
        "role": "assistant",
        "content": "I did not delete or remove anything yet."
    }));
    messages.push(serde_json::json!({
        "role": "user",
        "content": "Now search the web and remember the decision"
    }));
    let second =
        native_names(&native_tools_schema_for_context(ToolSchemaPolicy::root(false), &messages).0);

    assert_eq!(first, second);
    assert!(first.contains(&"delete_file".to_string()));
    assert!(first.contains(&"search_web".to_string()));
    assert!(first.contains(&"list_mcp_tools".to_string()));
}

#[test]
fn api_native_exposes_file_creation_for_new_file_requests() {
    let messages = vec![serde_json::json!({
        "role": "user",
        "content": "Make a new file for the parser"
    })];
    let names = native_tools_schema_for_context(ToolSchemaPolicy::root(false), &messages)
        .0
        .into_iter()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    assert!(names.iter().any(|name| name == "write_to_file"));
    assert!(names.iter().any(|name| name == "write_file_chunk"));
}

#[test]
fn compact_text_prompt_explains_how_to_create_a_file() {
    let prompt = tool_system_prompt_for_policy(
        ToolSchemaPolicy::root_for_mode_with_compact_prompt(
            false,
            crate::config::AgentMode::Build,
            true,
        ),
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );
    assert!(prompt.contains("write_to_file | Args:"));
    assert!(prompt.contains("new or existing file path"));
}

#[test]
fn bootstrap_schema_phase_does_not_move_the_builtin_tool_block() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("package.json"), "{}\n").unwrap();
    let messages = vec![serde_json::json!({
        "role": "user",
        "content": "Create a small TypeScript app and run its tests"
    })];
    assert_eq!(
        tool_schema_phase(&messages, Some(dir.path())),
        ToolSchemaPhase::Bootstrap
    );
    let (bootstrap_schemas, stats) = native_tools_schema_for_context_with_sticky_at(
        ToolSchemaPolicy::root(false),
        &messages,
        &[],
        Some(dir.path()),
    );
    let bootstrap = native_names(&bootstrap_schemas);
    for required in ["run_command", "write_to_file", "write_file_chunk"] {
        assert!(
            bootstrap.contains(&required.to_string()),
            "missing {required}"
        );
    }
    assert_eq!(stats.phase, ToolSchemaPhase::Bootstrap);

    for index in 0..4 {
        std::fs::write(dir.path().join(format!("src{index}.ts")), "export {};\n").unwrap();
    }
    assert_eq!(
        tool_schema_phase(&messages, Some(dir.path())),
        ToolSchemaPhase::Established
    );
    let (established_schemas, stats) = native_tools_schema_for_context_with_sticky_at(
        ToolSchemaPolicy::root(false),
        &messages,
        &[],
        Some(dir.path()),
    );
    assert_eq!(stats.phase, ToolSchemaPhase::Established);
    // The phase still gates the MCP discovery fallback, but it must not move the
    // built-in half: a mid-session flip used to swap the whole `tools` block and
    // invalidate the provider's cached prefix. (#1589)
    assert_eq!(bootstrap, native_names(&established_schemas));
    assert_eq!(stats.builtin_selected, stats.builtin_available);
    assert!(stats.withheld_builtin_names.is_empty());
}

#[test]
fn explicit_codebase_analysis_overrides_empty_workspace_bootstrap() {
    let dir = tempfile::tempdir().unwrap();
    let messages = vec![serde_json::json!({
        "role": "user",
        "content": "Analyze the repository's dependency graph and call graph"
    })];
    assert_eq!(
        tool_schema_phase(&messages, Some(dir.path())),
        ToolSchemaPhase::Established
    );
}

#[test]
fn subagent_text_prompt_does_not_advertise_delegation_or_mcp_tools() {
    let prompt = tool_system_prompt_for_policy(
        ToolSchemaPolicy::subagent(),
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );
    assert!(!prompt.contains("- spawn_agent |"));
    assert!(!prompt.contains("- send_agent |"));
    assert!(!prompt.contains("- wait_agent |"));
    assert!(!prompt.contains("- cancel_agent |"));
}

#[test]
fn subagent_lifecycle_tools_are_registered_and_model_facing() {
    let builtin_names = TOOLS.iter().map(|tool| tool.name).collect::<Vec<_>>();
    assert!(builtin_names.contains(&"wait_agent"));
    assert!(builtin_names.contains(&"cancel_agent"));

    let enabled = native_tools_schema(true);
    assert!(
        enabled
            .iter()
            .any(|tool| tool["function"]["name"] == "wait_agent")
    );
    assert!(
        enabled
            .iter()
            .any(|tool| tool["function"]["name"] == "cancel_agent")
    );
    assert_eq!(tool_safety("wait_agent"), ToolSafety::Delegation);
    assert_eq!(tool_safety("cancel_agent"), ToolSafety::Delegation);
}

#[test]
fn mcp_names_are_deterministically_qualified() {
    assert_eq!(
        mcp_canonical_name("build-server", "run"),
        "mcp__build_server__run"
    );
    assert_eq!(
        mcp_canonical_name("build-server", "run"),
        mcp_canonical_name("build-server", "run")
    );
    assert_ne!(
        mcp_canonical_name("build-server", "run"),
        mcp_canonical_name("test-server", "run")
    );
}

#[test]
fn canonical_mcp_names_have_readable_display_labels() {
    assert_eq!(
        super::mcp_tool_display_name("mcp__mail_mcp__SearchEmails").as_deref(),
        Some("mail_mcp.SearchEmails")
    );
    assert_eq!(super::mcp_tool_display_name("SearchEmails"), None);
}

#[test]
fn test_repair_json() {
    assert_eq!(repair_json("{\"name\": \"test\""), "{\"name\": \"test\"}");
    assert_eq!(
        repair_json("{\"name\": \"test\", \"arguments\": {\"path\": \"/foo\""),
        "{\"name\": \"test\", \"arguments\": {\"path\": \"/foo\"}}"
    );
    assert_eq!(
        repair_json(
            "{\"name\": \"test\", \"arguments\": {\"path\": \"/foo\", \"content\": \"hello"
        ),
        "{\"name\": \"test\", \"arguments\": {\"path\": \"/foo\", \"content\": \"hello\"}}"
    );
}

#[test]
fn test_parse_truncated_tool_call() {
    let text = "```tool\n{\"name\": \"write_to_file\", \"arguments\": {\"path\": \"/foo\", \"content\": \"hello";
    let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "write_to_file");
    assert_eq!(
        calls[0].arguments.get("path").unwrap().as_str().unwrap(),
        "/foo"
    );
    assert_eq!(
        calls[0].arguments.get("content").unwrap().as_str().unwrap(),
        "hello"
    );
}

#[test]
fn suspicious_textual_mutation_never_reaches_dispatch() {
    let temp_dir =
        std::env::temp_dir().join(format!("rustcode_reasoning_payload_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&temp_dir);
    let target = temp_dir.join("index.html");
    let text = format!(
        "[TOOL_CALLS]write_to_file[ARGS]{{\"path\":{:?},\"content\":\"partial body<think>\\nThe user wants me to continue the file.\\n</think>",
        target.to_string_lossy()
    );

    let calls = parse_tool_calls(&text, crate::config::ToolProtocol::Native);
    assert!(calls.is_empty(), "contaminated call must not be dispatched");
    let diagnostic = diagnose_failed_tool_call(&text).expect("recovery needs a diagnostic");
    assert!(diagnostic.contains(target.to_string_lossy().as_ref()));
    assert!(diagnostic.contains("smaller, complete, targeted"));
    assert!(!target.exists(), "rejected mutation must not create a file");

    let literal_args = serde_json::json!({
        "path": target,
        "content": "literal <think> marker in a requested document"
    });
    let result = execute_with_metadata("write_to_file", &literal_args);
    assert!(
        result.success,
        "literal content should remain writable: {result:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&literal_args["path"].as_str().unwrap()).unwrap(),
        "literal <think> marker in a requested document"
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_parse_tool_call_with_nested_code_fences_in_arguments() {
    let text = "```tool\n{\"name\": \"replace_file_content\", \"arguments\": {\"path\": \"SKILL.md\", \"target_content\": \"```sh\\nT=$(cut -d'\\\"' -f2 .env)\\n```\\n\", \"replacement_content\": \"## Auth\\n```sh\\nT=\\\"$TOKEN\\\"\\n```\"}}\n```\nFollow-up prose.";
    let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "replace_file_content");
    assert_eq!(
        calls[0].arguments.get("path").unwrap().as_str().unwrap(),
        "SKILL.md"
    );
    assert_eq!(
        calls[0]
            .arguments
            .get("target_content")
            .unwrap()
            .as_str()
            .unwrap(),
        "```sh\nT=$(cut -d'\"' -f2 .env)\n```\n"
    );
    assert_eq!(
        calls[0]
            .arguments
            .get("replacement_content")
            .unwrap()
            .as_str()
            .unwrap(),
        "## Auth\n```sh\nT=\"$TOKEN\"\n```"
    );
    assert!(diagnose_failed_tool_call(text).is_none());

    let stripped = rustcode_tool_protocol::text::strip_tool_call_syntax(text);
    assert_eq!(stripped.trim(), "Follow-up prose.");
}

#[test]
fn diagnose_reports_specific_json_error() {
    // Invalid JSON (bad escape + trailing junk) that repair can't rescue.
    let text =
        "```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"a\\zb\"} garbage}\n```";
    let diag = diagnose_failed_tool_call(text);
    assert!(diag.is_some(), "should diagnose an unparseable fence");
    let d = diag.unwrap();
    assert!(d.contains("JSON parse error"), "got: {d}");
    assert!(d.contains("Offending block"), "should echo the block: {d}");
}

#[test]
fn diagnose_ignores_valid_tool_call() {
    let text = "```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"x\"}}\n```";
    assert!(diagnose_failed_tool_call(text).is_none());
}

#[test]
fn diagnose_reports_validation_error() {
    let text = "```tool\n{\"name\": \"grep\", \"arguments\": {}}\n```";
    let diag = diagnose_failed_tool_call(text);
    assert!(diag.is_some(), "should diagnose invalid arguments");
    let d = diag.unwrap();
    assert!(d.contains("failed validation"), "got: {d}");
    assert!(d.contains("grep"), "got: {d}");
}

#[test]
fn diagnose_reports_schema_guidance_for_an_invalid_edit_shape() {
    let text = "```tool\n{\"name\": \"replace_file_content\", \"arguments\": {\"path\": \"src/store.ts\", \"replacements\": []}}\n```";
    let diag = diagnose_failed_tool_call(text).expect("should diagnose invalid arguments");
    assert!(
        diag.contains("replacements"),
        "must identify the invalid field: {diag}"
    );
    assert!(
        diag.contains("Expected arguments"),
        "must show the expected shape: {diag}"
    );
    assert!(
        diag.contains("\"edits\""),
        "must name the valid batch field: {diag}"
    );
    assert!(
        diag.contains("Example"),
        "must include a minimal valid example: {diag}"
    );
}

#[test]
fn malformed_native_arguments_preserve_parse_diagnostics_and_recovery_shape() {
    let call = ToolCall {
        name: "multi_replace_file_content".to_string(),
        arguments: serde_json::json!({
            "_invalid_arguments": {
                "kind": "incomplete_syntax",
                "execution": "rejected"
            },
            "_parse_error": "EOF while parsing an object",
            "_recovery": "No tool was executed"
        }),
        call_id: Some("call-1".to_string()),
    };
    let error = validate_tool_calls(&[call], 1).expect_err("malformed arguments must fail closed");
    assert!(error.contains("malformed arguments"), "{error}");
    assert!(error.contains("incomplete_syntax"), "{error}");
    assert!(error.contains("EOF while parsing an object"), "{error}");
    assert!(error.contains("No tool was executed"), "{error}");
    assert!(error.contains("start_line"), "{error}");
    assert!(error.contains("replacement_content"), "{error}");
    assert!(error.contains("replacements"), "{error}");
}

#[test]
fn view_file_guidance_uses_typed_line_ranges() {
    let call = ToolCall {
        name: "view_file".to_string(),
        arguments: serde_json::json!({"path": "src/lib.rs", "start_line": [340, 380]}),
        call_id: None,
    };
    let error = validate_tool_calls(&[call], 1).expect_err("line range arrays are invalid");
    assert!(error.contains("start_line"), "{error}");
    assert!(error.contains("\"start_line\":1"), "{error}");
    assert!(error.contains("\"end_line\":40"), "{error}");
}

#[test]
fn validation_error_names_schema_path_and_valid_example() {
    let error = validate_tool_calls(
        &[ToolCall {
            name: "replace_file_content".to_string(),
            arguments: serde_json::json!({
                "path": "src/store.ts",
                "edits": "[]"
            }),
            call_id: None,
        }],
        MAX_MUTATING_CALLS_PER_RESPONSE,
    )
    .expect_err("a stringified edits array must remain invalid");

    assert!(
        error.contains("Schema path: $.edits must be array"),
        "{error}"
    );
    assert!(error.contains("Expected arguments"), "{error}");
    assert!(error.contains("Example"), "{error}");
}

#[test]
fn ask_question_accepts_label_objects_in_the_single_question_shape() {
    let arguments = serde_json::json!({
        "question": "Which source?",
        "options": [
            {"label": "CHANGELOG.md", "description": "Curated notes"},
            "Releases API"
        ]
    });
    validate_tool_calls(
        &[ToolCall {
            name: "ask_question".to_string(),
            arguments: arguments.clone(),
            call_id: None,
        }],
        MAX_MUTATING_CALLS_PER_RESPONSE,
    )
    .expect("label objects are valid options");

    let summary = super::misc::ask_question(&arguments).unwrap();
    assert!(
        summary.contains("1. CHANGELOG.md — Curated notes"),
        "{summary}"
    );
    assert!(summary.contains("2. Releases API"), "{summary}");
}

#[test]
fn diagnose_reports_unknown_tool_as_unavailable() {
    let text = "```tool\n{\"name\": \"not_a_real_tool\", \"arguments\": {}}\n```";
    let diag = diagnose_failed_tool_call(text).expect("should diagnose unknown tool");
    assert!(diag.contains("unknown or unavailable tool"), "got: {diag}");
    assert!(
        diag.contains("not_a_real_tool"),
        "must name the unknown tool: {diag}"
    );
}

#[test]
fn test_parse_tool_calls_tag() {
    let text1 = "Let me check...[TOOL_CALLS]glob[ARGS]{\"pattern\": \"**/*.rs\"}";
    let calls1 = parse_tool_calls(text1, crate::config::ToolProtocol::Json);
    assert_eq!(calls1.len(), 1);
    assert_eq!(calls1[0].name, "glob");
    assert_eq!(
        calls1[0]
            .arguments
            .get("pattern")
            .unwrap()
            .as_str()
            .unwrap(),
        "**/*.rs"
    );

    let text2 = "Let me check...[TOOL_CALLS]glob\":{\"pattern\":\"**/*.rs\"}";
    let calls2 = parse_tool_calls(text2, crate::config::ToolProtocol::Json);
    assert_eq!(calls2.len(), 1);
    assert_eq!(calls2[0].name, "glob");
    assert_eq!(
        calls2[0]
            .arguments
            .get("pattern")
            .unwrap()
            .as_str()
            .unwrap(),
        "**/*.rs"
    );

    let text3 = "Plan:[TOOL_CALLS]todo_write[ARGS]{\"todos\": [{\"content\": \"Fix bug\"}]}";
    let calls3 = parse_tool_calls(text3, crate::config::ToolProtocol::Json);
    assert_eq!(calls3.len(), 1);
    assert_eq!(calls3[0].name, "todo_write");
}

#[test]
fn test_repair_json_escapes_multiline_string_literals() {
    let text = "```tool\n{\"name\": \"replace_file_content\", \"arguments\": {\"path\": \"src/config.rs\", \"edits\": [{\"old_string\": \"line1\", \"new_string\": \"line1\nline2\nline3\"}]}}\n```";
    let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "replace_file_content");
}

#[test]
fn test_is_tool_call_start_detects_json_and_embedded_tool_syntax() {
    assert!(rustcode_tool_protocol::is_tool_call_start(
        "```tool\n{\"name\": \"run_command\"}"
    ));
    assert!(rustcode_tool_protocol::is_tool_call_start(
        "```json\n{\"name\": \"run_command\"}"
    ));
    assert!(rustcode_tool_protocol::is_tool_call_start("[TOOL_CALLS]"));
    assert!(rustcode_tool_protocol::is_tool_call_start("<tool_call>"));
    assert!(rustcode_tool_protocol::is_tool_call_start(
        "Let me execute this:\n{\"action\": \"manage_task\", \"task_id\": \"task-123\"}"
    ));
    assert!(!rustcode_tool_protocol::is_tool_call_start(
        "Here is a regular markdown code block:\n```rust\nfn main() {}\n```"
    ));
    assert!(!rustcode_tool_protocol::is_tool_call_start(
        "Here is a plain json block:\n```json\n{\"seeds\": 580, \"potatoes\": 2423}\n```"
    ));
}

#[test]
fn test_diagnose_reports_missing_name_field() {
    let text = "```tool\n{\"arguments\": {\"path\": \"src/config.rs\"}}\n```";
    let diag = diagnose_failed_tool_call(text).unwrap();
    assert!(diag.contains("Missing required 'name' field"));
}

#[test]
fn test_extract_tool_call_recovers_name_nested_in_arguments() {
    let text = "```tool\n{\"arguments\": {\"name\": \"multi_replace_file_content\", \"path\": \"src/config.rs\", \"replacements\": []}}\n```";
    let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "multi_replace_file_content");
    assert_eq!(
        calls[0].arguments.get("path").unwrap().as_str().unwrap(),
        "src/config.rs"
    );
    assert!(calls[0].arguments.get("name").is_none());
}

#[test]
fn test_extract_tool_call_keeps_legitimate_name_argument() {
    // use_skill's only parameter is literally called `name`; with a proper
    // {"name", "arguments"} envelope it must survive extraction.
    let text = "```tool\n{\"name\": \"use_skill\", \"arguments\": {\"name\": \"spotify\"}}\n```";
    let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "use_skill");
    assert_eq!(
        calls[0].arguments.get("name").unwrap().as_str().unwrap(),
        "spotify"
    );
    // And the full validation path accepts it.
    validate_tool_calls(&calls, MAX_MUTATING_CALLS_PER_RESPONSE).unwrap();
}

#[test]
fn test_parse_tool_call_with_escaped_quotes_in_values() {
    // Reproduces the exact shape from session 1785743879558 MSG 7:
    // \\\" inside a JSON string value (e.g. serde rename_all = \"lowercase\")
    let text = "```tool\n{\"name\":\"replace_file_content\",\"arguments\":{\"edits\":[{\"new_string\":\"#[derive(Debug)]\\n#[serde(rename_all = \\\"lowercase\\\")]\\npub enum Foo {\",\"old_string\":\"#[derive(Debug)]\\npub enum Foo {\"}],\"path\":\"src/config.rs\"}}\n```";
    let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
    assert_eq!(
        calls.len(),
        1,
        "Expected 1 call, got {}: {:?}",
        calls.len(),
        calls
    );
    assert_eq!(calls[0].name, "replace_file_content");
    assert_eq!(
        calls[0].arguments.get("path").unwrap().as_str().unwrap(),
        "src/config.rs"
    );
}

#[test]
fn test_parse_multiple_fenced_tool_calls() {
    // Two distinct ```tool blocks in one turn → both parsed, in order.
    let text = "```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"foo\"}}\n```\n\
                    some prose\n\
                    ```tool\n{\"name\": \"view_file\", \"arguments\": {\"path\": \"src/x.rs\"}}\n```";
    let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "grep");
    assert_eq!(
        calls[0].arguments.get("pattern").unwrap().as_str().unwrap(),
        "foo"
    );
    assert_eq!(calls[1].name, "view_file");
    assert_eq!(
        calls[1].arguments.get("path").unwrap().as_str().unwrap(),
        "src/x.rs"
    );
}

#[test]
fn test_tool_code_decoy_is_skipped() {
    // Gemini habit: a real ```tool block plus redundant ```tool_code /
    // ```json decoys of the SAME call. Only one call, and the tool_code
    // fence must not be mis-parsed into garbage.
    let text = "```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"foo\"}}\n```\n\
                    ```tool_code\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"foo\"}}\n```\n\
                    ```json\n{\"tool_code\": {\"name\": \"grep\", \"arguments\": {\"pattern\": \"foo\"}}}\n```";
    let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "grep");
}

#[test]
fn test_two_tool_calls_with_tool_code_between() {
    // A tool_code decoy sitting between two real, distinct calls must be
    // skipped without swallowing the second call.
    let text = "```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"a\"}}\n```\n\
                    ```tool_code\n{\"name\": \"noise\", \"arguments\": {}}\n```\n\
                    ```tool\n{\"name\": \"glob\", \"arguments\": {\"pattern\": \"*.rs\"}}\n```";
    let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "grep");
    assert_eq!(calls[1].name, "glob");
}

#[test]
fn test_replace_file_content_tool_aliases_and_batch_edits() {
    let temp_dir = std::env::temp_dir().join(format!("rustcode_test_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&temp_dir);
    let test_file = temp_dir.join("test.rs");
    std::fs::write(&test_file, "fn foo() {}\nfn bar() {}\n").unwrap();

    // 1. Single edit using old_string / new_string aliases
    let args = serde_json::json!({
        "path": test_file.to_str().unwrap(),
        "old_string": "fn foo() {}",
        "new_string": "fn foo_updated() {}"
    });
    let res = filesystem::replace_file_content_tool(&args).unwrap();
    assert!(res.contains("successfully replaced"));
    // The diff reflects the real two-line file (changed line 1, unchanged
    // line 2 as context), not a fabrication derived purely from the
    // single-line old_string/new_string arguments.
    assert!(res.contains("@@ -1,2 +1,2 @@"), "got: {res}");
    assert!(res.contains("-fn foo() {}"), "got: {res}");
    assert!(res.contains("+fn foo_updated() {}"), "got: {res}");
    let updated = std::fs::read_to_string(&test_file).unwrap();
    assert!(updated.contains("fn foo_updated() {}"));

    // 2. Batch edits using edits array
    let args_batch = serde_json::json!({
        "path": test_file.to_str().unwrap(),
        "edits": [
            {"old_string": "fn foo_updated() {}", "new_string": "fn foo_v2() {}"},
            {"old_string": "fn bar() {}", "new_string": "fn bar_v2() {}"}
        ]
    });
    let res_batch = filesystem::replace_file_content_tool(&args_batch).unwrap();
    assert!(res_batch.contains("successfully applied 2 edits"));
    let final_content = std::fs::read_to_string(&test_file).unwrap();
    assert_eq!(final_content, "fn foo_v2() {}\nfn bar_v2() {}\n");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_view_file_tool_directory_fallback() {
    let temp_dir = std::env::temp_dir().join(format!("rustcode_dir_test_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&temp_dir);
    std::fs::write(temp_dir.join("sample.txt"), "hello").unwrap();

    let args = serde_json::json!({
        "path": temp_dir.to_str().unwrap()
    });
    let res = filesystem::view_file_tool(&args).unwrap();
    assert!(res.contains("sample.txt"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn plan_mode_allows_reads_but_denies_mutation_execution_and_unknown_tools() {
    assert!(allowed_in_plan_mode("view_file"));
    assert!(allowed_in_plan_mode("grep"));
    assert!(allowed_in_plan_mode("search_web"));
    assert!(allowed_in_plan_mode("ask_question"));
    assert!(!allowed_in_plan_mode("write_to_file"));
    assert!(!allowed_in_plan_mode("write_file_chunk"));
    assert!(!allowed_in_plan_mode("run_command"));
    assert!(!allowed_in_plan_mode("spawn_agent"));
    assert!(!allowed_in_plan_mode("unknown_mcp_tool"));
}

#[test]
fn tool_safety_is_conservative_and_classifies_reads() {
    let call = |name: &str| ToolCall {
        name: name.to_string(),
        arguments: serde_json::json!({}),
        call_id: None,
    };
    assert_eq!(tool_safety("use_skill"), ToolSafety::ControlPlane);
    assert_eq!(tool_safety("grep"), ToolSafety::ReadOnly);
    assert!(!is_read_only_call(&call("use_skill")));
    assert!(is_read_only_call(&call("view_file")));
    assert_eq!(tool_safety("write_to_file"), ToolSafety::WorkspaceMutation);
    assert!(!is_read_only_call(&call("write_to_file")));
    assert_eq!(
        tool_safety("write_file_chunk"),
        ToolSafety::WorkspaceMutation
    );
    assert!(!is_read_only_call(&call("write_file_chunk")));
    assert_eq!(tool_safety("run_command"), ToolSafety::ProcessControl);
    assert_eq!(tool_safety("unknown_mcp_tool"), ToolSafety::Unknown);
    assert!(!is_read_only_call(&call("unknown_mcp_tool")));
}

#[test]
fn skills_are_control_plane_and_isolated() {
    let calls = vec![
        ToolCall {
            name: "use_skill".to_string(),
            arguments: serde_json::json!({"name": "spotify"}),
            call_id: None,
        },
        ToolCall {
            name: "run_command".to_string(),
            arguments: serde_json::json!({"command": "spotify-cli p volume 3"}),
            call_id: None,
        },
    ];

    let (isolated, deferred) = isolate_control_plane_call(calls);

    assert_eq!(isolated.len(), 1);
    assert_eq!(isolated[0].name, "use_skill");
    assert_eq!(deferred, 1);
}

#[path = "tests/protocol.rs"]
mod protocol;
