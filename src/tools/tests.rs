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
        "Issue exactly one tool call",
        "wait for its result",
        "already applied",
        "advisory loop signals, not a hard stop",
        "native function-calling interface",
        "plain-text summary",
        "do NOT print tool calls as text or JSON",
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
    assert_eq!(tool_safety("use_skill"), ToolSafety::ReadOnly);
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
    let sound_schema = &tools
        .iter()
        .find(|tool| tool["function"]["name"] == "generate_sound_effect")
        .expect("sound generation tool is advertised")["function"]["parameters"];
    assert_eq!(sound_schema["properties"]["duration_seconds"]["minimum"], 0);
    assert!(
        sound_schema["properties"]["duration_seconds"]
            .get("exclusiveMinimum")
            .is_none()
    );
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
fn mcp_schema_selection_has_bounded_deterministic_discovery_fallback() {
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
    assert_eq!(selected.len(), MCP_DISCOVERY_FALLBACK_COUNT);
    assert_eq!(stats.fallback, MCP_DISCOVERY_FALLBACK_COUNT);
    assert_eq!(stats.omitted, mcp.len() - MCP_DISCOVERY_FALLBACK_COUNT);
    assert_eq!(
        stats.selected_names,
        vec!["tool_00", "tool_01", "tool_02", "tool_03"]
    );
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

    assert_eq!(selected, vec![0, 1]);
    assert_eq!(
        stats.selected_names,
        vec!["search_issues", "weather_forecast"]
    );

    let messages = vec![serde_json::json!({
        "role":"user",
        "content":"Search issues"
    })];
    let (selected, stats) = select_mcp_tools_for_context_with_sticky(
        &mcp,
        &messages,
        &["weather_forecast".to_string()],
    );
    assert_eq!(selected, vec![0, 1]);
    assert_eq!(
        stats.selected_names,
        vec!["search_issues", "weather_forecast"]
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
    assert!(!child_names.iter().any(|name| is_agent_tool(name)));
    assert!(!ToolSchemaPolicy::subagent().include_mcp_tools);
    assert!(ToolSchemaPolicy::root(true).include_mcp_tools);
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
            "view_file"
        ]
    );
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

#[test]
fn api_native_builtin_selection_keeps_coding_core_and_routes_specialized_tools() {
    let coding_messages = vec![serde_json::json!({
        "role": "user",
        "content": "Implement the parser fix, edit the Rust files, and run tests"
    })];
    let coding = native_tools_schema_for_context(ToolSchemaPolicy::root(false), &coding_messages).0;
    let coding_names = coding
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect::<Vec<_>>();
    for required in [
        "grep",
        "view_file",
        "replace_file_content",
        "run_command",
        "complete_task",
    ] {
        assert!(coding_names.contains(&required), "missing {required}");
    }
    for irrelevant in ["render_video", "generate_music", "get_time", "remember"] {
        assert!(
            !coding_names.contains(&irrelevant),
            "unrelated schema leaked into coding request: {irrelevant}"
        );
    }

    let media_messages = vec![serde_json::json!({
        "role": "user",
        "content": "Render a video and generate music for it"
    })];
    let media = native_tools_schema_for_context(ToolSchemaPolicy::root(false), &media_messages).0;
    let media_names = media
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect::<Vec<_>>();
    assert!(media_names.contains(&"render_video"));
    assert!(media_names.contains(&"generate_music"));
}

#[test]
fn bootstrap_schema_phase_prunes_index_tools_until_source_exists() {
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
    let (schemas, stats) = native_tools_schema_for_context_with_sticky_at(
        ToolSchemaPolicy::root(false),
        &messages,
        &[],
        Some(dir.path()),
    );
    let names = schemas
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"run_command"));
    assert!(names.contains(&"write_to_file"));
    assert!(!names.contains(&"find_symbol"));
    assert!(!names.contains(&"get_project_map"));
    assert_eq!(stats.phase, ToolSchemaPhase::Bootstrap);
    assert!(stats.builtin_selected < stats.builtin_available);
    let bootstrap_schema_tokens = crate::network::compaction::estimate_tool_schema_tokens(&schemas);
    let baseline_schema_tokens =
        crate::network::compaction::estimate_tool_schema_tokens(&native_tools_schema(false));
    assert!(
        bootstrap_schema_tokens < baseline_schema_tokens * 80 / 100,
        "bootstrap schema should materially reduce the baseline: {bootstrap_schema_tokens} vs {baseline_schema_tokens}"
    );

    for index in 0..4 {
        std::fs::write(dir.path().join(format!("src{index}.ts")), "export {};\n").unwrap();
    }
    assert_eq!(
        tool_schema_phase(&messages, Some(dir.path())),
        ToolSchemaPhase::Established
    );
    let (schemas, stats) = native_tools_schema_for_context_with_sticky_at(
        ToolSchemaPolicy::root(false),
        &messages,
        &[],
        Some(dir.path()),
    );
    let names = schemas
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"find_symbol"));
    assert!(names.contains(&"get_project_map"));
    assert_eq!(stats.phase, ToolSchemaPhase::Established);
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

    let stripped = crate::network::text::strip_tool_call_syntax(text);
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
    assert!(is_tool_call_start("```tool\n{\"name\": \"run_command\"}"));
    assert!(is_tool_call_start("```json\n{\"name\": \"run_command\"}"));
    assert!(is_tool_call_start("[TOOL_CALLS]"));
    assert!(is_tool_call_start("<tool_call>"));
    assert!(is_tool_call_start(
        "Let me execute this:\n{\"action\": \"manage_task\", \"task_id\": \"task-123\"}"
    ));
    assert!(!is_tool_call_start(
        "Here is a regular markdown code block:\n```rust\nfn main() {}\n```"
    ));
    assert!(!is_tool_call_start(
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
    assert_eq!(tool_safety("use_skill"), ToolSafety::ReadOnly);
    assert_eq!(tool_safety("grep"), ToolSafety::ReadOnly);
    assert!(is_read_only_call(&call("use_skill")));
    assert!(is_read_only_call(&call("view_file")));
    assert_eq!(tool_safety("write_to_file"), ToolSafety::WorkspaceMutation);
    assert!(!is_read_only_call(&call("write_to_file")));
    assert_eq!(tool_safety("run_command"), ToolSafety::ProcessControl);
    assert_eq!(tool_safety("unknown_mcp_tool"), ToolSafety::Unknown);
    assert!(!is_read_only_call(&call("unknown_mcp_tool")));
}

#[test]
fn skills_are_read_only_and_not_isolated_as_control_plane() {
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

    assert_eq!(isolated.len(), 2);
    assert_eq!(deferred, 0);
}

// Regression: session 1785595713111. Asked to plan a --json flag, the model
// produced a plan containing "use grep to find the argument parsing" as a
// future step and hedged over whether the project uses clap or structopt —
// both answerable from a Cargo.toml it was allowed to read. It made zero
// tool calls.
// Regression: session 1785595170460 msg 4 read
// "replace_file_content: error: error: target_content (old_string) is empty".
// Every test session opened its edit with old_string: "" — the model's
// instinct for "add a line at the top" — costing a turn before the error
// taught it otherwise. The spec the model reads before calling now says it.
// The orchestration boundary accepts a model batch for transcript safety but
// executes only one call from it.
#[test]
fn the_prompt_matches_what_the_executor_actually_does() {
    let prompt = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );

    assert!(
        prompt.contains("Issue exactly one tool call"),
        "got: {prompt}"
    );
    assert!(prompt.contains("wait for its result"), "got: {prompt}");
    assert!(!prompt.contains("run in parallel"), "got: {prompt}");
    assert!(
        !prompt.contains("at most 1 or 2 tool calls"),
        "the old cap is gone"
    );

    // And the stated limit on changes must be the one the executor enforces.
    assert!(
        prompt.contains("Issue exactly one tool call"),
        "got: {prompt}"
    );
    assert!(
        prompt.contains("use `view_file` with `start_line`/`end_line`"),
        "got: {prompt}"
    );
    assert!(
        !prompt.contains("next_start_line"),
        "prompt must only advertise schema-valid view_file arguments: {prompt}"
    );
}

#[test]
fn complete_view_file_results_force_progress() {
    let prompt = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );

    for required in [
        "complete results are authoritative",
        "do not reread them",
        "edit or verify next",
    ] {
        assert!(prompt.contains(required), "missing {required:?}: {prompt}");
    }
}

#[test]
fn manual_preview_port_guidance_honors_user_control() {
    let prompt = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );

    for required in [
        "For manual previews",
        "use the user's exact port",
        "do not start/probe/fallback",
        "let them run it after verification",
        "do not start a server merely to inspect a static app",
    ] {
        assert!(prompt.contains(required), "missing {required:?}: {prompt}");
    }
}

// Regression: the JSON format must tell the model to make one call per
// response, while the parser remains able to recover every call in a batch so
// the orchestration layer can close the unexecuted calls safely.
#[test]
fn json_protocol_tool_format_requires_one_call_per_response() {
    let prompt = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );

    assert!(
        !prompt.contains("Emit one tool call at a time"),
        "got: {prompt}"
    );
    assert!(
        !prompt.contains("executes calls sequentially"),
        "got: {prompt}"
    );
    assert!(prompt.contains("Emit exactly one fence"), "got: {prompt}");
    assert!(
        !prompt.contains("Several fences are allowed"),
        "got: {prompt}"
    );
}

// Regression: a repeated `replace_file_content` call that reports
// "already applied" (PR #306's idempotency guard) is a true no-op — not a
// silently-different second edit. The prompt should discourage endlessly
// repeating it while leaving room for a different inspection, edit, or test
// when the task still needs work.
#[test]
fn prompt_treats_loop_signals_as_advisory() {
    let prompt = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );

    assert!(prompt.contains("already applied"), "got: {prompt}");
    assert!(
        prompt.contains("advisory loop signals, not a hard stop"),
        "got: {prompt}"
    );
    for required in [
        "different `view_file` range",
        "`grep`, edit, or test",
        "cached replay content",
        "without new evidence",
    ] {
        assert!(prompt.contains(required), "missing {required:?}: {prompt}");
    }
    assert!(!prompt.contains("harness ends the turn after a handful"));
}

// The native tag protocol's parser (`parse_tool_calls_tags`) also walks
// every `[TOOL_CALLS]` occurrence in one response, so it must not carry
// the same "one at a time" claim the JSON section used to have, and its
// format text must actually differ from the JSON fence syntax it
// describes.
#[test]
fn native_protocol_prompt_describes_native_syntax_and_not_json_fences() {
    let native = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Native,
        crate::config::AgentMode::Build,
    );
    let json = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );

    assert!(
        !native.contains("Emit one tool call at a time"),
        "got: {native}"
    );
    assert!(
        native.contains("[TOOL_CALLS]tool_name[ARGS]"),
        "got: {native}"
    );
    assert!(
        !native.contains("```tool"),
        "native prompt must not describe the JSON fence syntax"
    );
    assert!(
        !json.contains("[TOOL_CALLS]tool_name[ARGS]"),
        "json prompt must not describe the native tag syntax"
    );
}

// ApiNative carries tool schemas through the request's native `tools`
// field, not the prompt text, so its section must say to use that
// interface directly and must not print either text-based protocol's
// call syntax.
#[test]
fn api_native_protocol_prompt_defers_to_the_request_schema() {
    let prompt = tool_system_prompt(
        false,
        crate::config::ToolProtocol::ApiNative,
        crate::config::AgentMode::Build,
    );

    assert!(
        prompt.contains("native function-calling interface"),
        "got: {prompt}"
    );
    assert!(!prompt.contains("```tool"), "got: {prompt}");
    assert!(!prompt.contains("[TOOL_CALLS]"), "got: {prompt}");
}

#[test]
fn the_edit_tool_spec_explains_how_to_insert() {
    let spec = TOOLS
        .iter()
        .find(|tool| tool.name == "replace_file_content")
        .expect("tool exists");

    assert!(
        spec.description.contains("to INSERT text"),
        "got: {}",
        spec.description
    );
    assert!(
        spec.description.contains("prepend"),
        "got: {}",
        spec.description
    );
    assert!(
        spec.description.contains("An empty target is rejected"),
        "got: {}",
        spec.description
    );
    assert!(
        spec.arguments.contains("never empty"),
        "got: {}",
        spec.arguments
    );
}

#[test]
fn the_view_file_spec_describes_the_hard_read_window() {
    let spec = TOOLS
        .iter()
        .find(|tool| tool.name == "view_file")
        .expect("tool exists");

    assert!(
        spec.description.contains("800-line hard cap"),
        "got: {}",
        spec.description
    );
    assert!(
        spec.description.contains("targeted follow-up ranges"),
        "got: {}",
        spec.description
    );
    assert!(
        spec.arguments.contains("800 lines"),
        "got: {}",
        spec.arguments
    );
    assert!(
        spec.arguments.contains("targeted follow-up"),
        "got: {}",
        spec.arguments
    );
    assert!(
        !spec.arguments.contains("start_line + 2000"),
        "got: {}",
        spec.arguments
    );

    let schema = schema_for_tool("view_file");
    assert_eq!(
        schema["properties"]["end_line"]["description"],
        "Inclusive end line; each call is capped at 800 lines. Request targeted follow-up ranges for more content."
    );
}

#[test]
fn error_messages_are_prefixed_once() {
    assert_eq!(
        as_error_message("target_content is empty"),
        "error: target_content is empty"
    );
    // A handler that already framed its message keeps it as written.
    assert_eq!(
        as_error_message("error: target_content is empty"),
        "error: target_content is empty"
    );
    assert_eq!(
        as_error_message("  Error: file not found"),
        "Error: file not found"
    );
}

#[test]
fn plan_mode_prompt_demands_investigation_not_a_plan_to_investigate() {
    let prompt = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Plan,
    );

    assert!(prompt.contains("PLAN MODE"));
    assert!(
        prompt.contains("Investigate before planning"),
        "got: {prompt}"
    );
    assert!(
        prompt.contains("specific to THIS repository"),
        "got: {prompt}"
    );
    assert!(prompt.contains("resolve unknowns now"), "got: {prompt}");

    // Build mode must not carry the plan-mode restrictions.
    let build = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );
    assert!(!build.contains("PLAN MODE"));
}

#[test]
fn truncate_keeps_leading_calls_and_reports_the_drop() {
    let call = |name: &str| ToolCall {
        name: name.to_string(),
        arguments: serde_json::json!({}),
        call_id: None,
    };

    // Under the limit: nothing is touched.
    let (kept, dropped) = truncate_tool_batch(
        vec![call("grep"), call("view_file")],
        MAX_MUTATING_CALLS_PER_RESPONSE,
    );
    assert_eq!(kept.len(), 2);
    assert_eq!(dropped, 0);

    // Reads fan out freely: ten searches are one thought, not ten.
    let reads: Vec<ToolCall> = (0..10).map(|_| call("grep")).collect();
    let (kept, dropped) = truncate_tool_batch(reads, MAX_MUTATING_CALLS_PER_RESPONSE);
    assert_eq!(kept.len(), 10);
    assert_eq!(dropped, 0);

    // The raised default keeps a small focused mutation sequence together.
    assert!(
        MAX_MUTATING_CALLS_PER_RESPONSE >= 3,
        "the mutating default must allow a focused batch, got {}",
        MAX_MUTATING_CALLS_PER_RESPONSE
    );

    // Only the first call that can change the workspace is kept under a
    // limit of one, while reads before and after it remain available in
    // this provider batch.
    let over = vec![
        call("grep"),
        call("run_command"),
        call("write_to_file"),
        call("run_command"),
        call("write_to_file"),
        call("run_command"),
        call("grep"),
    ];
    let (kept, dropped) = truncate_tool_batch(over, 1);
    assert_eq!(kept.len(), 1 + 2);
    assert_eq!(dropped, 4);
    assert_eq!(kept[0].name, "grep");
    assert_eq!(kept[1].name, "run_command");

    // A profile override permits a bounded larger set while retaining
    // response order and the same read behavior.
    let over = vec![
        call("grep"),
        call("run_command"),
        call("write_to_file"),
        call("run_command"),
        call("write_to_file"),
    ];
    let (kept, dropped) = truncate_tool_batch(over, 3);
    assert_eq!(kept.len(), 4);
    assert_eq!(dropped, 1);
    assert_eq!(kept[1].name, "run_command");
    assert_eq!(kept[3].name, "run_command");

    // Read-only calls are not capped by an arbitrary response-wide ceiling.
    let runaway: Vec<ToolCall> = (0..50).map(|_| call("grep")).collect();
    let (kept, dropped) = truncate_tool_batch(runaway, MAX_MUTATING_CALLS_PER_RESPONSE);
    assert_eq!(kept.len(), 50);
    assert_eq!(dropped, 0);
}

#[test]
fn partition_retains_multiple_read_calls_for_non_orchestration_consumers() {
    let call = |name: &str| ToolCall {
        name: name.to_string(),
        arguments: serde_json::json!({}),
        call_id: None,
    };

    // Batch partitioning remains available to non-root consumers; the root
    // model-round boundary no longer sends such a batch to the executor.
    let (kept, dropped) = truncate_tool_batch(
        vec![
            call("use_skill"),
            call("use_skill"),
            call("grep"),
            call("run_command"),
        ],
        MAX_MUTATING_CALLS_PER_RESPONSE,
    );
    assert_eq!(kept.len(), 4);
    assert_eq!(dropped, 0);
}

#[test]
fn partition_preserves_reads_after_the_mutation_limit() {
    let call = |name: &str| ToolCall {
        name: name.to_owned(),
        arguments: serde_json::json!({}),
        call_id: None,
    };
    let (kept, dropped) = partition_tool_batch(
        vec![
            call("grep"),
            call("run_command"),
            call("write_to_file"),
            call("view_file"),
            call("run_command"),
            call("get_time"),
        ],
        1,
    );

    assert_eq!(
        kept.iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>(),
        ["grep", "run_command", "view_file", "get_time"]
    );
    assert_eq!(
        dropped
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>(),
        ["write_to_file", "run_command"]
    );
}

#[test]
fn validation_accepts_string_encoded_integers_like_the_handlers() {
    // Handlers read line numbers via parse_json_number, which tolerates
    // providers that send integers as strings; validation must accept the
    // same shape end to end.
    assert!(
        validate_tool_calls(
            &[ToolCall {
                name: "replace_file_content".to_string(),
                arguments: serde_json::json!({
                    "path": "src/store.ts",
                    "old_string": "old",
                    "new_string": "new",
                    "start_line": "500"
                }),
                call_id: None,
            }],
            MAX_MUTATING_CALLS_PER_RESPONSE
        )
        .is_ok()
    );
    assert!(
        validate_tool_calls(
            &[ToolCall {
                name: "replace_file_content".to_string(),
                arguments: serde_json::json!({
                    "path": "src/store.ts",
                    "old_string": "old",
                    "new_string": "new",
                    "start_line": "not-a-number"
                }),
                call_id: None,
            }],
            MAX_MUTATING_CALLS_PER_RESPONSE
        )
        .is_err()
    );
}

#[test]
fn validation_scopes_string_integer_leniency_to_builtin_tools() {
    // MCP servers receive arguments verbatim with no parse_json_number
    // coercion, so string-encoded integers must be rejected for them.
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "limit": { "type": "integer" } }
    });
    let stringy = serde_json::json!({ "limit": "10" });
    let numeric = serde_json::json!({ "limit": 10 });

    assert!(
        validate_value_against_schema(&stringy, &schema, "$", false).is_err(),
        "non-builtin (MCP-style) validation must reject string integers"
    );
    assert!(validate_value_against_schema(&numeric, &schema, "$", false).is_ok());
    assert!(validate_value_against_schema(&stringy, &schema, "$", true).is_ok());
}

#[test]
fn validation_accepts_json_schema_union_types() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "limit": { "type": ["integer", "null"] } }
    });

    assert!(
        validate_value_against_schema(&serde_json::json!({"limit": 10}), &schema, "$", false)
            .is_ok()
    );
    assert!(
        validate_value_against_schema(&serde_json::json!({"limit": null}), &schema, "$", false)
            .is_ok()
    );
    let error =
        validate_value_against_schema(&serde_json::json!({"limit": "10"}), &schema, "$", false)
            .expect_err("MCP validation must reject string-encoded integers");
    assert_eq!(error, "$.limit must be integer or null");
}

#[test]
fn validation_accepts_nullable_strings_expressed_with_any_of() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "folder": {
                "anyOf": [{"type": "string"}, {"type": "null"}]
            }
        }
    });

    for value in [serde_json::json!("inbox"), serde_json::Value::Null] {
        assert!(
            validate_value_against_schema(
                &serde_json::json!({"folder": value}),
                &schema,
                "$",
                false
            )
            .is_ok()
        );
    }

    let error =
        validate_value_against_schema(&serde_json::json!({"folder": 42}), &schema, "$", false)
            .expect_err("numbers must not satisfy a nullable string schema");
    assert!(error.contains("$.folder does not match anyOf"));
    assert!(error.contains("branch 1: $.folder must be string"));
    assert!(error.contains("branch 2: $.folder must be null"));
}

#[test]
fn validation_enforces_one_of_branch_count_and_reports_invalid_values() {
    let nullable_string = serde_json::json!({
        "oneOf": [{"type": "string"}, {"type": "null"}]
    });
    assert!(
        validate_value_against_schema(&serde_json::json!("inbox"), &nullable_string, "$", false)
            .is_ok()
    );
    assert!(
        validate_value_against_schema(&serde_json::Value::Null, &nullable_string, "$", false)
            .is_ok()
    );

    let error =
        validate_value_against_schema(&serde_json::json!(false), &nullable_string, "$", false)
            .expect_err("booleans must not satisfy a nullable string schema");
    assert!(error.contains("$ does not match oneOf"));
    assert!(error.contains("branch 1: $ must be string"));
    assert!(error.contains("branch 2: $ must be null"));

    let overlapping = serde_json::json!({
        "oneOf": [{"type": "string"}, {"type": ["string", "null"]}]
    });
    assert_eq!(
        validate_value_against_schema(&serde_json::json!("inbox"), &overlapping, "$", false)
            .unwrap_err(),
        "$ must match exactly one oneOf branch, but matched 2"
    );

    let constrained_number = serde_json::json!({
        "oneOf": [
            {"type": "integer", "maximum": 0},
            {"type": "integer", "minimum": 1}
        ]
    });
    assert!(
        validate_value_against_schema(&serde_json::json!(2), &constrained_number, "$", false)
            .is_ok()
    );
    let error =
        validate_value_against_schema(&serde_json::json!(0.5), &constrained_number, "$", false)
            .expect_err("a non-integer must fail both constrained integer branches");
    assert!(error.contains("branch 1: $ must be integer"));
    assert!(error.contains("branch 2: $ must be integer"));
}

#[test]
fn validation_rejects_unknown_duplicate_and_mixed_calls() {
    let valid = ToolCall {
        name: "grep".to_string(),
        arguments: serde_json::json!({"pattern": "TODO"}),
        call_id: None,
    };
    assert!(
        validate_tool_calls(
            std::slice::from_ref(&valid),
            MAX_MUTATING_CALLS_PER_RESPONSE
        )
        .is_ok()
    );
    assert!(
        validate_tool_calls(
            &[ToolCall {
                name: "replace_file_content".to_string(),
                arguments: serde_json::json!({
                    "path": "src/store.ts",
                    "edits": [{"old_string": "old", "new_string": "new"}]
                }),
                call_id: None,
            }],
            MAX_MUTATING_CALLS_PER_RESPONSE
        )
        .is_ok()
    );
    assert!(validate_tool_calls(&[valid.clone(), valid], MAX_MUTATING_CALLS_PER_RESPONSE).is_err());
    let default_view = ToolCall {
        name: "view_file".to_string(),
        arguments: serde_json::json!({"path": "src/main.rs"}),
        call_id: None,
    };
    let explicit_default_view = ToolCall {
        name: "view_file".to_string(),
        arguments: serde_json::json!({"path": "src/main.rs", "start_line": "1"}),
        call_id: None,
    };
    assert!(
        validate_tool_calls(
            &[default_view, explicit_default_view],
            MAX_MUTATING_CALLS_PER_RESPONSE
        )
        .is_err(),
        "duplicate reads must still be rejected before execution"
    );
    assert!(
        validate_tool_calls(
            &[ToolCall {
                name: "not_registered".to_string(),
                arguments: serde_json::json!({}),
                call_id: None,
            }],
            MAX_MUTATING_CALLS_PER_RESPONSE
        )
        .is_err()
    );
    let calls = (0..33)
        .map(|index| ToolCall {
            name: "grep".to_string(),
            arguments: serde_json::json!({"pattern": format!("TODO-{index}")}),
            call_id: None,
        })
        .collect::<Vec<_>>();
    assert!(validate_tool_calls(&calls, MAX_MUTATING_CALLS_PER_RESPONSE).is_ok());
    assert!(
        validate_tool_calls(
            &[ToolCall {
                name: "run_command".to_string(),
                arguments: serde_json::json!({}),
                call_id: None,
            }],
            MAX_MUTATING_CALLS_PER_RESPONSE
        )
        .is_err()
    );
    assert!(
        validate_tool_calls(
            &[
                ToolCall {
                    name: "use_skill".to_string(),
                    arguments: serde_json::json!({"name": "x"}),
                    call_id: None,
                },
                ToolCall {
                    name: "grep".to_string(),
                    arguments: serde_json::json!({"pattern": "x"}),
                    call_id: None,
                },
            ],
            MAX_MUTATING_CALLS_PER_RESPONSE
        )
        .is_ok()
    );
}

#[test]
fn validation_enforces_the_resolved_mutation_limit() {
    let calls = [
        ToolCall {
            name: "run_command".to_string(),
            arguments: serde_json::json!({"command": "cargo test"}),
            call_id: None,
        },
        ToolCall {
            name: "run_command".to_string(),
            arguments: serde_json::json!({"command": "cargo test", "timeout_ms": 1}),
            call_id: None,
        },
    ];
    assert!(validate_tool_calls(&calls, 2).is_ok());
    assert!(validate_tool_calls(&calls, 1).is_err());
}

#[test]
fn authorization_is_centralized_and_conservative() {
    assert!(matches!(
        authorize_tool("write_to_file", crate::config::AgentMode::Plan, true, false),
        AuthorizationDecision::Deny(_)
    ));
    assert_eq!(
        authorize_tool(
            "write_to_file",
            crate::config::AgentMode::Build,
            false,
            false
        ),
        AuthorizationDecision::RequireConfirmation
    );
    assert_eq!(
        authorize_tool("mcp_unknown", crate::config::AgentMode::Build, false, false),
        AuthorizationDecision::RequireConfirmation
    );
    assert_eq!(
        authorize_tool("grep", crate::config::AgentMode::Build, false, false),
        AuthorizationDecision::Allow
    );
}

#[test]
fn manage_task_is_allowed_without_confirmation() {
    // MANAGE_TASK is ToolSafety::ProcessControl, not Unknown, so it must
    // not be swept into the conservative Unknown-confirmation fallback.
    assert_eq!(
        authorize_tool("manage_task", crate::config::AgentMode::Build, false, false),
        AuthorizationDecision::Allow
    );
}

#[test]
fn command_authorization_distinguishes_git_inspection_from_recovery() {
    assert_eq!(
        authorize_tool_with_args(
            "run_command",
            &serde_json::json!({"command": "git status --short"}),
            crate::config::AgentMode::Build,
            false,
            false,
        ),
        AuthorizationDecision::Allow
    );
    assert_eq!(
        authorize_tool_with_args(
            "run_command",
            &serde_json::json!({"command": "git restore -- src/GameScene.ts"}),
            crate::config::AgentMode::Build,
            false,
            false,
        ),
        AuthorizationDecision::RequireConfirmation
    );
}

#[test]
fn command_authorization_distinguishes_safe_and_unknown_shell_commands() {
    assert_eq!(
        authorize_tool_with_args(
            "run_command",
            &serde_json::json!({"command": "gh issue list --repo lhagfoss/rustcode"}),
            crate::config::AgentMode::Build,
            false,
            false,
        ),
        AuthorizationDecision::Allow
    );
    assert_eq!(
        authorize_tool_with_args(
            "run_command",
            &serde_json::json!({"command": "gh issue close 1 --repo lhagfoss/rustcode"}),
            crate::config::AgentMode::Build,
            false,
            false,
        ),
        AuthorizationDecision::RequireConfirmation
    );
}

#[test]
fn command_authorization_allows_harmless_inspection_but_confirms_side_effects() {
    for command in [
        "cat src/main.rs",
        "sed -n '1,40p' src/main.rs",
        "head -40 src/main.rs",
        "tail -40 src/main.rs",
        "grep -n TODO src/main.rs",
    ] {
        assert_eq!(
            authorize_tool_with_args(
                "run_command",
                &serde_json::json!({"command": command}),
                crate::config::AgentMode::Build,
                false,
                false,
            ),
            AuthorizationDecision::Allow,
            "harmless inspection should remain available: {command}"
        );
    }

    for command in [
        "cat src/main.rs > /tmp/main.rs",
        "sed -i 's/old/new/' file.txt",
        "cat src/main.rs | tee /tmp/main.rs",
        "git restore -- src/main.rs",
    ] {
        assert_eq!(
            authorize_tool_with_args(
                "run_command",
                &serde_json::json!({"command": command}),
                crate::config::AgentMode::Build,
                false,
                false,
            ),
            AuthorizationDecision::RequireConfirmation,
            "side-effectful command must remain gated: {command}"
        );
    }
}

#[test]
fn prompt_makes_delegation_explicitly_opt_in() {
    let prompt = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );
    assert!(prompt.contains("Do not spawn subagents unless the user explicitly requests"));
    assert!(prompt.contains("Review every subagent result"));
    assert!(prompt.contains("Never run `cargo check` on a standalone `.rs` file"));
    assert!(prompt.contains("Prefer the smallest focused sequence"));
    assert!(prompt.contains("git-feature-workflow"));
    assert!(prompt.contains("Chained shell commands are fine"));
    assert!(prompt.contains("Tool results are authoritative: claim checks only"));
    assert!(prompt.contains("never use `git add .`"));
}

#[test]
fn skills_are_discovered_and_loaded_in_two_on_demand_steps() {
    let prompt = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );
    let list = prompt.find("list_skills").expect("discovery guidance");
    let use_skill = prompt.find("use_skill").expect("invocation guidance");
    assert!(list < use_skill, "discover before loading: {prompt}");
    assert!(prompt.contains("metadata only"));
    assert!(prompt.contains("selected SKILL.md"));
    assert!(TOOLS.iter().any(|tool| tool.name == "list_skills"));
    assert!(TOOLS.iter().any(|tool| tool.name == "use_skill"));
}

#[test]
fn skill_catalog_is_not_embedded_in_the_base_prompt() {
    let prompt = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );
    let old_catalog = (0..100)
            .map(|index| {
                format!(
                    "  <skill>\n    <name>synthetic-skill-{index}</name>\n    <description>{}</description>\n  </skill>\n",
                    "A specialized workflow with enough detail to represent a real installed skill"
                )
            })
            .collect::<String>();

    let skills_section = prompt
        .split_once("# Skills\n")
        .and_then(|(_, rest)| rest.split_once("You are rustcode"))
        .map(|(section, _)| section)
        .expect("skills section");
    assert!(skills_section.len() < old_catalog.len());
    assert!(!prompt.contains("<available_skills>"));
    assert!(!prompt.contains("synthetic-skill-"));
}

#[test]
fn list_skills_returns_metadata_without_instruction_bodies() {
    let result = super::misc::list_skills(&serde_json::json!({})).unwrap();
    assert!(!result.contains("<skill_content"));
    if result.starts_with("<available_skills") {
        assert!(result.contains("<description>"));
        assert!(result.contains("Call use_skill"));
    }
}

#[test]
fn memory_tools_lifecycle_execution() {
    assert!(TOOLS.iter().any(|t| t.name == "remember"));
    assert!(TOOLS.iter().any(|t| t.name == "recall_memory"));
    assert!(TOOLS.iter().any(|t| t.name == "forget_memory"));

    let res = (super::misc::REMEMBER.handler)(&serde_json::json!({
        "key": "test_db_port",
        "value": "5433",
        "category": "database",
        "scope": "global"
    }))
    .unwrap();
    assert!(res.contains("Remembered globally"));

    let recall_res = (super::misc::RECALL_MEMORY.handler)(&serde_json::json!({
        "query": "test_db_port",
        "scope": "global"
    }))
    .unwrap();
    assert!(recall_res.contains("5433"));

    let forget_res = (super::misc::FORGET_MEMORY.handler)(&serde_json::json!({
        "key": "test_db_port",
        "scope": "global"
    }))
    .unwrap();
    assert!(forget_res.contains("Removed"));
}

#[test]
fn list_directory_uses_active_workspace_root_instead_of_process_cwd() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(workspace.path().join("workspace-only.txt"), "content")
        .expect("workspace marker");
    set_active_workspace_root(Some(workspace.path().to_path_buf()));

    let result = super::search::list_directory(&serde_json::json!({"path": "."}));
    set_active_workspace_root(None);
    assert_eq!(result.expect("workspace listing"), "workspace-only.txt");
}

#[test]
fn run_command_uses_active_workspace_root_when_cwd_is_omitted() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(workspace.path().join("workspace-only.txt"), "content")
        .expect("workspace marker");
    set_active_workspace_root(Some(workspace.path().to_path_buf()));

    let result = super::exec::run_command(&serde_json::json!({
        "command": "test -f workspace-only.txt"
    }));
    set_active_workspace_root(None);
    assert!(result.expect("workspace command").contains("exit code: 0"));
}

// Issue #983: the mutating default must hold a small focused sequence, and
// read-only shell inspection must never consume it.
#[test]
fn mutating_default_allows_a_focused_batch() {
    assert!(
        crate::config::DEFAULT_MAX_MUTATING_CALLS_PER_RESPONSE >= 3,
        "got {}",
        crate::config::DEFAULT_MAX_MUTATING_CALLS_PER_RESPONSE
    );
    assert!(MAX_MUTATING_CALLS_PER_RESPONSE >= 3);

    let call = |name: &str, arguments: serde_json::Value| ToolCall {
        name: name.to_string(),
        arguments,
        call_id: None,
    };
    let batch = vec![
        call("grep", serde_json::json!({"pattern": "TODO"})),
        call("run_command", serde_json::json!({"command": "cargo test"})),
        call(
            "replace_file_content",
            serde_json::json!({
                "path": "src/a.ts",
                "edits": [{"old_string": "a", "new_string": "b"}]
            }),
        ),
        call(
            "write_to_file",
            serde_json::json!({"path": "x", "content": "y"}),
        ),
        call("view_file", serde_json::json!({"path": "src/a.ts"})),
    ];
    let (kept, dropped) = partition_tool_batch(
        batch,
        crate::config::DEFAULT_MAX_MUTATING_CALLS_PER_RESPONSE,
    );
    assert_eq!(
        kept.len(),
        5,
        "reads plus a focused mutation batch must survive"
    );
    assert!(dropped.is_empty());
    assert!(
        validate_tool_calls(
            &kept,
            crate::config::DEFAULT_MAX_MUTATING_CALLS_PER_RESPONSE
        )
        .is_ok()
    );
}

#[test]
fn is_read_only_call_matches_the_confirmation_policy() {
    let shell = |command: &str| ToolCall {
        name: "run_command".to_string(),
        arguments: serde_json::json!({"command": command}),
        call_id: None,
    };
    let named = |name: &str| ToolCall {
        name: name.to_string(),
        arguments: serde_json::json!({}),
        call_id: None,
    };

    // Native reads are inspection.
    assert!(is_read_only_call(&ToolCall {
        name: "grep".to_string(),
        arguments: serde_json::json!({"pattern": "TODO"}),
        call_id: None,
    }));
    assert!(is_read_only_call(&ToolCall {
        name: "view_file".to_string(),
        arguments: serde_json::json!({"path": "src/a.ts"}),
        call_id: None,
    }));
    // Read-only shell inspection needs no confirmation, so it is inspection.
    for command in [
        "pwd",
        "git status --short",
        "ls src",
        "cat src/app.ts",
        "rg foo src | head -20",
    ] {
        assert!(
            is_read_only_call(&shell(command)),
            "{command} must be read-only"
        );
    }
    // Mutating or unclassified work still consumes the budget.
    for command in [
        "cargo test",
        "rm -rf /tmp/x",
        "git restore -- src/GameScene.ts",
        "cargo test && rg foo src",
    ] {
        assert!(
            !is_read_only_call(&shell(command)),
            "{command} must be mutating"
        );
    }
    // Fail closed: a missing command and unknown tools count as mutating.
    assert!(!is_read_only_call(&named("run_command")));
    assert!(!is_read_only_call(&named("write_to_file")));
    assert!(!is_read_only_call(&named("unknown_mcp_tool")));
}

// Issue #983: several inspection shells run together with mutations without
// being dropped or reprimanded by the mutating cap.
#[test]
fn read_only_shell_commands_bypass_the_mutation_budget() {
    let shell = |command: &str| ToolCall {
        name: "run_command".to_string(),
        arguments: serde_json::json!({"command": command}),
        call_id: None,
    };
    let batch = vec![
        shell("git status --short"),
        shell("ls src"),
        shell("cat src/app.ts"),
        shell("rg foo src | head -20"),
        ToolCall {
            name: "replace_file_content".to_string(),
            arguments: serde_json::json!({
                "path": "src/a.ts",
                "edits": [{"old_string": "a", "new_string": "b"}]
            }),
            call_id: None,
        },
    ];
    // Four inspections plus one edit fit under a mutation limit of one
    // because the inspections never consume it.
    let (kept, dropped) = partition_tool_batch(batch.clone(), 1);
    assert!(
        dropped.is_empty(),
        "inspection must not be dropped: {dropped:?}"
    );
    assert_eq!(kept.len(), batch.len());
    assert!(validate_tool_calls(&kept, 1).is_ok());

    // A genuinely mutating shell still counts: two more `cargo test` calls
    // exceed a limit of one alongside the existing edit.
    let mut over = batch;
    over.push(shell("cargo test"));
    over.push(shell("cargo test -- --nocapture"));
    let (kept, dropped) = partition_tool_batch(over.clone(), 1);
    assert_eq!(
        dropped.len(),
        2,
        "excess mutating shells must yield: {kept:?}"
    );
    assert!(validate_tool_calls(&over, 1).is_err());
    assert!(validate_tool_calls(&kept, 1).is_ok());
}
