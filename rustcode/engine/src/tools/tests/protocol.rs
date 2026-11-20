//! Tool-call protocol prompts, batching, validation and execution policy.

use super::*;

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
// The orchestration boundary accepts a model batch for transcript safety,
// executes every valid read and at most one mutation from it.
#[test]
fn the_prompt_matches_what_the_executor_actually_does() {
    let prompt = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );

    assert!(prompt.contains("Batch independent reads"), "got: {prompt}");
    // The workflow rule itself, not a tool description that happens to say
    // "wait for" (which is all the lower-case match used to find).
    assert!(
        prompt.contains("Wait for results before the next calls"),
        "got: {prompt}"
    );
    assert!(
        prompt.contains("mutations run in order"),
        "mutations stay ordered: {prompt}"
    );
    assert!(
        !prompt.contains("at most 1 or 2 tool calls"),
        "the old cap is gone"
    );

    // And the stated limit on changes must be the one the executor enforces.
    assert!(prompt.contains("mutations run in order"), "got: {prompt}");
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

// Regression: the JSON format must tell the model to batch reads while
// keeping mutations ordered, while the parser remains able to recover every
// call in a batch so the orchestration layer can close unexecuted calls safely.
#[test]
fn json_protocol_tool_format_batches_reads_per_response() {
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
    assert!(prompt.contains("multiple fences"), "got: {prompt}");
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
        spec.description.contains(
            "To insert, anchor on an adjacent line and repeat that line in the replacement"
        ),
        "got: {}",
        spec.description
    );
    assert!(
        spec.description.contains("an empty target is rejected"),
        "got: {}",
        spec.description
    );
    assert!(
        spec.arguments.contains("target_content"),
        "got: {}",
        spec.description
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

    // Which parameters belong to which mode is stated once, in the tool
    // description, instead of being repeated on every parameter (#1826).
    for mode_rule in [
        "outline=true instead returns a bounded Markdown heading outline",
        "paged by outline_offset/outline_limit",
        "start_line, end_line and content_offset are then ignored",
        "never counts as a complete read",
    ] {
        assert!(
            spec.description.contains(mode_rule),
            "missing {mode_rule:?}: {}",
            spec.description
        );
    }
    let schema = schema_for_tool("view_file");
    assert_eq!(schema["properties"]["end_line"]["description"], "Inclusive");
    for (field, minimum) in [
        ("start_line", 1),
        ("end_line", 1),
        ("content_offset", 0),
        ("outline_offset", 0),
        ("outline_limit", 1),
    ] {
        assert_eq!(schema["properties"][field]["type"], "integer", "{field}");
        assert_eq!(schema["properties"][field]["minimum"], minimum, "{field}");
    }
    assert_eq!(schema["properties"]["outline"]["type"], "boolean");
    assert_eq!(schema["properties"]["outline_limit"]["maximum"], 100);
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
    assert!(prompt.contains("Shift+Tab"), "got: {prompt}");

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
fn validation_resolves_defs_references_for_schemars_enums() {
    // Verbatim shape emitted by schemars/rmcp for a Rust enum behind an
    // optional field. The `kind` property carries only `$ref` plus a
    // description, so a validator that ignores `$defs` sees no `type` at all.
    let schema = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "additionalProperties": false,
        "$defs": {
            "FeedKind": {
                "type": "string",
                "enum": ["foryou", "popular", "discussed", "tag", "source"]
            }
        },
        "properties": {
            "kind": {"$ref": "#/$defs/FeedKind", "description": "Feed type."},
            "limit": {"type": ["integer", "null"]}
        }
    });

    // The exact call rejected in session 01a0fc8b: a valid enum member behind a
    // $ref must pass, not fail with a misleading "must be object".
    assert!(
        validate_value_against_schema(
            &serde_json::json!({"kind": "foryou", "limit": 15}),
            &schema,
            "$",
            false
        )
        .is_ok(),
        "a $ref-backed string enum must accept its members"
    );

    // String-encoded integers stay rejected for MCP tools.
    let lenient = validate_value_against_schema(
        &serde_json::json!({"kind": "tag", "limit": "15"}),
        &schema,
        "$",
        false,
    )
    .expect_err("MCP arguments arrive verbatim");
    assert_eq!(lenient, "$.limit must be integer or null");

    let wrong_type = validate_value_against_schema(
        &serde_json::json!({"kind": {"nested": true}}),
        &schema,
        "$",
        false,
    )
    .expect_err("a $ref-backed string must not accept an object");
    assert_eq!(wrong_type, "$.kind must be string");

    let unknown_member = validate_value_against_schema(
        &serde_json::json!({"kind": "trending"}),
        &schema,
        "$",
        false,
    )
    .expect_err("only advertised enum members may be sent");
    assert_eq!(
        unknown_member,
        "$.kind must be one of [\"foryou\", \"popular\", \"discussed\", \"tag\", \"source\"]"
    );
}

#[test]
fn validation_rejects_unresolvable_and_self_referential_defs() {
    let dangling = serde_json::json!({
        "type": "object",
        "properties": {"kind": {"$ref": "#/$defs/Missing"}}
    });
    let error = validate_value_against_schema(
        &serde_json::json!({"kind": "foryou"}),
        &dangling,
        "$",
        false,
    )
    .expect_err("a dangling reference must not silently pass as an object");
    assert!(
        error.contains("$.kind references schema '#/$defs/Missing'"),
        "{error}"
    );

    // A self-referential definition must terminate on the depth bound.
    let recursive = serde_json::json!({
        "type": "object",
        "$defs": {"Node": {"$ref": "#/$defs/Node"}},
        "properties": {"next": {"$ref": "#/$defs/Node"}}
    });
    let error =
        validate_value_against_schema(&serde_json::json!({"next": {}}), &recursive, "$", false)
            .expect_err("a self-referential reference must not spin");
    assert!(error.contains("schema reference limit"), "{error}");
}

#[test]
fn validation_applies_sibling_keywords_next_to_a_reference() {
    let schema = serde_json::json!({
        "type": "object",
        "$defs": {"Name": {"type": "string"}},
        "properties": {"tag": {"$ref": "#/$defs/Name", "maxLength": 4}}
    });

    assert!(
        validate_value_against_schema(&serde_json::json!({"tag": "rust"}), &schema, "$", false)
            .is_ok()
    );
    let error =
        validate_value_against_schema(&serde_json::json!({"tag": "systems"}), &schema, "$", false)
            .expect_err("a maxLength beside $ref must still apply");
    assert_eq!(error, "$.tag must contain at most 4 characters");
}

#[test]
fn validation_caps_the_enum_members_quoted_in_a_rejection() {
    let members = (0..40)
        .map(|index| format!("option{index}"))
        .collect::<Vec<_>>();
    let schema = serde_json::json!({
        "type": "object",
        "$defs": {"Wide": {"type": "string", "enum": members}},
        "properties": {"pick": {"$ref": "#/$defs/Wide"}}
    });

    let error =
        validate_value_against_schema(&serde_json::json!({"pick": "nope"}), &schema, "$", false)
            .expect_err("an unknown enum member must be rejected");
    assert!(error.contains("option0"), "{error}");
    assert!(
        !error.contains("option39"),
        "large enums must stay bounded: {error}"
    );
    assert!(error.contains("32 more"), "{error}");
}

#[test]
fn enum_guidance_resolves_references_and_prefers_a_real_member() {
    let schema = serde_json::json!({
        "type": "object",
        "$defs": {
            "FeedKind": {"type": "string", "enum": ["foryou", "popular"]},
            "Broken": {"$ref": "#/$defs/Absent"}
        },
        "properties": {
            "kind": {"$ref": "#/$defs/FeedKind", "description": "Feed type."},
            "odd": {"$ref": "#/$defs/Broken", "description": "Unresolvable."}
        }
    });

    // No `required` list, so the guidance must still be concrete rather than `{}`.
    assert_eq!(
        example_value_for_schema(&schema),
        serde_json::json!({"kind": "foryou"})
    );
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
fn subagent_id_schemas_accept_numeric_strings_and_main_message_id() {
    for tool_name in [
        "wait_agent",
        "cancel_agent",
        "inspect_agent",
        "send_message",
        "followup_task",
    ] {
        let schema = super::registered_tool_schema(tool_name)
            .unwrap_or_else(|| panic!("missing schema for {tool_name}"));
        let message =
            matches!(tool_name, "send_message" | "followup_task").then_some("new evidence");
        for id in [serde_json::json!(1), serde_json::json!("1")] {
            let mut arguments = serde_json::json!({"id": id});
            if let Some(message) = message {
                arguments["message"] = serde_json::json!(message);
            }
            validate_value_against_schema(&arguments, &schema, "$", true)
                .unwrap_or_else(|error| panic!("{tool_name} rejected a supported id: {error}"));
        }
    }

    let send_message = super::registered_tool_schema("send_message").unwrap();
    assert!(
        validate_value_against_schema(
            &serde_json::json!({"id": 0, "message": "reply"}),
            &send_message,
            "$",
            true,
        )
        .is_ok()
    );

    for tool_name in ["wait_agent", "cancel_agent", "inspect_agent"] {
        let schema = super::registered_tool_schema(tool_name).unwrap();
        assert!(
            validate_value_against_schema(&serde_json::json!({"id": 0}), &schema, "$", true,)
                .is_err(),
            "{tool_name} must reject reserved root id 0"
        );
    }
}

#[test]
fn agent_schemas_advertise_roles_interrupt_and_hour_long_waits() {
    let spawn = super::registered_tool_schema("spawn_agent").unwrap();
    assert_eq!(
        spawn["properties"]["agent_type"]["enum"],
        serde_json::json!(["default", "explorer", "worker"])
    );
    let native = native_tools_schema(true);
    let advertised = |tool: &str, property: &str| {
        native
            .iter()
            .find(|schema| schema["function"]["name"] == tool)
            .is_some_and(|schema| {
                !schema["function"]["parameters"]["properties"][property].is_null()
            })
    };
    assert!(advertised("spawn_agent", "agent_type"));

    for tool_name in ["followup_task", "send_agent"] {
        let schema = super::registered_tool_schema(tool_name).unwrap();
        validate_value_against_schema(
            &serde_json::json!({"id": "1", "message": "redirect", "interrupt": true}),
            &schema,
            "$",
            true,
        )
        .unwrap_or_else(|error| panic!("{tool_name} rejected interrupt: {error}"));
        assert!(advertised(tool_name, "interrupt"), "{tool_name}");
    }
    // Queue-only delivery has nothing to interrupt.
    assert!(!advertised("send_message", "interrupt"));

    let wait = super::registered_tool_schema("wait_agent").unwrap();
    for (timeout_ms, accepted) in [(3_600_000_u64, true), (3_600_001, false)] {
        assert_eq!(
            validate_value_against_schema(
                &serde_json::json!({"ids": [1, 2], "timeout_ms": timeout_ms}),
                &wait,
                "$",
                true,
            )
            .is_ok(),
            accepted,
            "{timeout_ms}"
        );
    }
}

#[test]
fn agent_tool_count_follows_the_delegation_capability() {
    use ToolCapability::AgentDelegation;
    let build = crate::config::AgentMode::Build;
    let policy = ToolSchemaPolicy::root(true);

    // Every delegation tool the request advertises, wherever it is defined.
    let delegation = native_tools_schema(true)
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .filter(|name| tool_capabilities(name).contains(&AgentDelegation))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for name in ["spawn_agent", "send_agent", "wait_agent", "followup_task"] {
        assert!(delegation.iter().any(|tool| tool == name), "{name}");
    }
    assert_eq!(agent_tool_count(policy, build), delegation.len());

    // These two used to be the count's other half; they delegate nothing.
    for name in ["set_goal", "todo_write"] {
        assert!(
            !tool_capabilities(name).contains(&AgentDelegation),
            "{name}"
        );
        assert!(!delegation.iter().any(|tool| tool == name), "{name}");
    }

    assert_eq!(agent_tool_count(ToolSchemaPolicy::root(false), build), 0);
    assert_eq!(agent_tool_count(policy, crate::config::AgentMode::Plan), 0);

    // Delegation tools in `TOOLS` are already part of `builtin`; the total
    // counts each advertised tool once.
    let surface = textual_tool_surface(policy, build);
    assert_eq!(surface.agent, delegation.len());
    assert_eq!(
        surface.total(),
        surface.builtin + surface.mcp + super::schema::AGENT_TOOL_SPECS.len()
    );
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
fn shell_assessment_cache_rejects_a_stale_call_signature() {
    use crate::tools::{
        ShellAssessment, shell_assessment_cache_key, shell_assessment_matches_call,
        shell_call_signature,
    };

    let original = ToolCall {
        name: "run_command".to_string(),
        arguments: serde_json::json!({"command": "python --version"}),
        call_id: Some("call-1".to_string()),
    };
    let assessment = ShellAssessment {
        cache_key: shell_assessment_cache_key(&original),
        call_signature: shell_call_signature(&original),
        local_authorization: AuthorizationDecision::RequireConfirmation,
    };
    assert!(shell_assessment_matches_call(&assessment, &original));

    let changed = ToolCall {
        arguments: serde_json::json!({"command": "rm -f scratch.txt"}),
        ..original
    };
    assert!(!shell_assessment_matches_call(&assessment, &changed));
}

#[test]
fn an_approved_matching_confirmation_assessment_is_not_prompted_twice() {
    use crate::tools::{
        ShellAssessment, execution_authorization, shell_assessment_cache_key, shell_call_signature,
    };

    let call = ToolCall {
        name: "run_command".to_string(),
        arguments: serde_json::json!({"command": "python --version"}),
        call_id: Some("approved-confirmed-call".to_string()),
    };
    let assessment = ShellAssessment {
        cache_key: shell_assessment_cache_key(&call),
        call_signature: shell_call_signature(&call),
        local_authorization: AuthorizationDecision::RequireConfirmation,
    };

    assert_eq!(
        execution_authorization(
            &call.name,
            &call.arguments,
            call.call_id.as_deref(),
            crate::config::AgentMode::Build,
            false,
            true,
            Some(&assessment),
        ),
        AuthorizationDecision::Allow,
        "the matching assessment records the call that was already approved"
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
fn one_shot_network_sandbox_escalation_is_auto_approved_in_yolo() {
    assert_eq!(
        authorize_tool_with_args(
            "run_command",
            &serde_json::json!({"command": "curl https://example.com", "network_access": true}),
            crate::config::AgentMode::Build,
            true,
            false,
        ),
        AuthorizationDecision::Allow
    );
}

#[test]
fn one_shot_filesystem_sandbox_escalation_is_auto_approved_in_yolo() {
    let args = serde_json::json!({
        "command": "touch /tmp/release.txt",
        "filesystem_write_path": "/tmp"
    });
    assert_eq!(
        authorize_tool_with_args(
            "run_command",
            &args,
            crate::config::AgentMode::Build,
            true,
            false,
        ),
        AuthorizationDecision::Allow
    );
    assert!(crate::tools::rememberable_command_prefix_for_call(&args).is_none());
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
fn delegation_prompt_matches_tool_availability() {
    let prompt = tool_system_prompt(
        false,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );
    // When unavailable, explain how to enable it and honor the hard config gate.
    assert!(!prompt.contains("Subagent tools are available by default"));
    assert!(!prompt.contains("Review every subagent result"));
    assert!(!prompt.contains("- spawn_agent | Args:"));
    assert!(prompt.contains("No agent tools; don't search"));
    assert!(prompt.contains("`/delegate`"));
    assert!(prompt.contains("config disables"));
    for policy in [
        ToolSchemaPolicy::subagent(),
        ToolSchemaPolicy::read_only_inspection(),
    ] {
        let prompt = tool_system_prompt_for_policy(
            policy,
            crate::config::ToolProtocol::Json,
            crate::config::AgentMode::Build,
        );
        assert!(!prompt.contains("/delegate"));
    }

    let delegated = tool_system_prompt(
        true,
        crate::config::ToolProtocol::Json,
        crate::config::AgentMode::Build,
    );
    assert!(delegated.contains("Subagent tools are available by default"));
    assert!(delegated.contains("use them when parallel work or independent review will help"));
    assert!(delegated.contains("Review every subagent result"));
    assert!(delegated.contains("- spawn_agent | Args:"));
    assert!(!delegated.contains("No subagent tools this task"));

    assert!(prompt.contains("Never run `cargo check` on a standalone `.rs` file"));
    assert!(prompt.contains("Prefer the smallest focused sequence"));
    assert!(prompt.contains("git-feature-workflow"));
    // A fix is not a release: the publish workflow runs only on request (#2762).
    assert!(prompt.contains(
        "Do not branch, commit, push or open a PR unless the user or the project's instructions ask"
    ));
    assert!(prompt.contains("Chained shell commands are fine"));
    assert!(prompt.contains("Tool results are authoritative: claim checks only"));
    assert!(prompt.contains("never use `git add .`"));
}

#[test]
fn api_native_prompt_states_that_the_tool_list_is_filtered() {
    let prompt = tool_system_prompt(
        false,
        crate::config::ToolProtocol::ApiNative,
        crate::config::AgentMode::Build,
    );
    assert!(prompt.contains("# Tool Surface"));
    assert!(prompt.contains("filtered, not exhaustive"));
    assert!(prompt.contains("list_mcp_tools"));
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
    assert!(prompt.contains("For local machine state check the shell first"));
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
    let _catalog = crate::skills::lock_skill_catalog_tests();
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
fn zoom_context_is_available_in_coding_and_inspection_surfaces() {
    let tool = TOOLS
        .iter()
        .find(|tool| tool.name == "zoom_context")
        .expect("compacted context must be recoverable through a built-in tool");
    assert!(!tool.requires_confirmation);
    assert_eq!(tool.safety, ToolSafety::ReadOnly);
    for policy in [
        ToolSchemaPolicy::root(false),
        ToolSchemaPolicy::read_only_inspection(),
        ToolSchemaPolicy::root_for_mode_with_compact_prompt(
            false,
            crate::config::AgentMode::Build,
            true,
        ),
    ] {
        let schemas = native_tools_schema_for_context(policy, &[]).0;
        assert!(
            schemas
                .iter()
                .any(|schema| schema["function"]["name"] == "zoom_context")
        );
        let prompt = super::schema::tool_system_prompt_for_policy(
            policy,
            crate::config::ToolProtocol::Json,
            crate::config::AgentMode::Build,
        );
        assert!(prompt.contains("zoom_context"));
    }
    assert!(allowed_in_plan_mode("zoom_context"));
}

#[test]
fn list_directory_uses_active_workspace_root_instead_of_process_cwd() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(workspace.path().join("workspace-only.txt"), "content")
        .expect("workspace marker");
    set_active_workspace_root(Some(workspace.path().to_path_buf()));

    let result = super::search::list_directory(&serde_json::json!({"path": "."}));
    set_active_workspace_root(None);
    let result = result.expect("workspace listing");
    assert!(result.starts_with("[Directory: "));
    assert!(result.ends_with("workspace-only.txt"));
}

#[test]
fn run_command_uses_active_workspace_root_when_cwd_is_omitted() {
    if !crate::tools::exec::sandbox::runtime_tests_available() {
        return;
    }
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
        "lscpu | head -20",
        "free -h",
        "df -h | grep -v tmpfs",
        "ps -e | grep -i hyprland",
        "uptime",
        "uname -a",
        "lspci | grep -i vga",
        "lsblk",
        "ss -tlnp",
        "pacman -Qe",
        "paru -Qs rustcode",
        "systemctl status docker",
        "systemctl list-units --type=service",
        "hostnamectl",
        "timedatectl",
        "sysctl -a",
        "lscpu 2>/dev/null | head -20",
        "ps -e 2>/dev/null | grep hyprland",
        "df -h >/dev/null 2>&1 | head",
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
        "echo x > /tmp/out",
        "cat <<EOF",
        "hostname myhost",
        "hostnamectl set-hostname myhost",
        "timedatectl set-time 2026-01-01",
        "sysctl -w net.ipv4.ip_forward=1",
        "pacman -Syu",
        "systemctl restart docker",
        "dmesg -C",
        "yq -i .a=1 file.yaml",
        "echo foo 2> /tmp/err",
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

#[test]
fn the_write_tool_spec_steers_large_files_to_chunks_first() {
    let spec = TOOLS
        .iter()
        .find(|tool| tool.name == "write_to_file")
        .expect("tool exists");

    // Issue #1233: large single writes hit provider output cutoffs and lose
    // the whole call, so the spec must steer chunked writes from the start.
    assert!(
        spec.description.contains("write_file_chunk past ~4 KiB"),
        "got: {}",
        spec.description
    );
    assert!(
        spec.arguments
            .contains("prefer write_file_chunk from the start"),
        "got: {}",
        spec.arguments
    );
}

#[test]
fn the_run_command_spec_forbids_moving_the_user_checkout() {
    let spec = TOOLS
        .iter()
        .find(|tool| tool.name == "run_command")
        .expect("tool exists");

    // Hard safety rails stay: no rebase/reset/force-push in the active
    // checkout, and no discarding the user's uncommitted work. The worktree is
    // now conditional rather than mandatory, and repository instructions still
    // outrank generic skills.
    for required_guidance in [
        "git rebase",
        "reset --hard",
        "force-push",
        "uncommitted work",
        "AGENTS.md",
        "outranks generic workflow skills",
        "git worktree add",
        "only when genuinely required",
        "git worktree remove",
        "git worktree prune",
        "git pull --ff-only",
    ] {
        assert!(
            spec.description.contains(required_guidance),
            "run_command spec must mention {required_guidance}: {}",
            spec.description
        );
    }
}

#[test]
fn repository_branch_policy_resolves_generic_switch_recipe_without_moving_checkout() {
    let repository_policy = include_str!("../../../../../AGENTS.md");
    let run_command_spec = TOOLS
        .iter()
        .find(|tool| tool.name == "run_command")
        .expect("tool exists")
        .description;

    // The repository policy defines when a worktree is required; the tool spec
    // must defer to it rather than prescribing one recipe for every repo, and
    // must still refuse to move or discard the user's checkout.
    assert!(
        repository_policy.contains("Work in a new worktree on a task branch")
            && repository_policy.contains("unless the user says"),
        "repository policy must default to a worktree and allow an explicit override"
    );
    assert!(
        repository_policy.contains("no `git rebase`, `git reset --hard`, or")
            && repository_policy.contains("force-push in the active checkout"),
        "repository policy must keep the hard safety rails"
    );
    assert!(
        repository_policy.contains("`git pull --ff-only`"),
        "repository policy must sync the active checkout"
    );
    assert!(
        repository_policy.contains("git worktree remove")
            && repository_policy.contains("git worktree prune"),
        "worktrees must be cleaned up after push and merge"
    );
    assert!(
        run_command_spec
            .contains("follow the repository `AGENTS.md`, which outranks generic workflow skills"),
        "run_command must defer to the repository policy"
    );
}

#[test]
fn fuzzy_match_resolves_case_typos_and_aliases() {
    assert_eq!(super::fuzzy_match_tool_name("view_file"), Some("view_file"));
    assert_eq!(super::fuzzy_match_tool_name("VIEW_FILE"), Some("view_file"));
    assert_eq!(super::fuzzy_match_tool_name("read"), Some("view_file"));
    assert_eq!(super::fuzzy_match_tool_name("bash"), Some("run_command"));
    // Single-edit typo resolves.
    assert_eq!(
        super::fuzzy_match_tool_name("gti_status"),
        Some("git_status")
    );
    assert!(super::fuzzy_match_tool_name("definitely-not-a-tool-xyz").is_none());
    assert!(super::fuzzy_match_tool_name("").is_none());
}

#[test]
fn shared_truncate_bytes_bounds_output_at_utf8_boundary() {
    assert_eq!(super::truncate_bytes("abc", 10), "abc");
    let text = format!("{}é{}", "a".repeat(19), "z");
    let truncated = super::truncate_bytes(&text, 20);
    assert!(truncated.len() <= 20 + 40);
    assert!(truncated.contains("truncated to 20 bytes"));
}

#[test]
fn unknown_tool_error_suggests_close_match() {
    let output = super::dispatch::execute("gti_status", &serde_json::json!({}));
    assert!(
        output.contains("Did you mean 'git_status'"),
        "got: {output}"
    );
}

#[test]
fn filter_tools_ranks_exact_before_fuzzy_and_respects_limit() {
    let ranked = super::filter_tools_by_query("git_status", 10);
    assert_eq!(ranked.first(), Some(&"git_status"));
    let limited = super::filter_tools_by_query("git", 2);
    assert_eq!(limited.len(), 2);
    assert_eq!(limited, vec!["git_add", "git_commit"]);
    let all_git = super::filter_tools_by_query("git", 10);
    assert!(all_git.contains(&"git_status"));
    assert!(all_git.contains(&"git_diff"));
    assert!(super::filter_tools_by_query("", 5).is_empty());
}

#[test]
fn trusted_native_tools_can_read_write_and_search_outside_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("probe.txt");
    set_active_workspace_context(
        Some(workspace.path().into()),
        Some(workspace.path().into()),
        false,
        Some(crate::config::SandboxMode::Trusted),
    );
    let write = execute_with_metadata(
        "write_to_file",
        &serde_json::json!({"path": path, "content": "outside-marker"}),
    );
    let read = execute_with_metadata("view_file", &serde_json::json!({"path": path}));
    let search = execute_with_metadata(
        "grep",
        &serde_json::json!({"path": outside.path(), "pattern": "outside-marker"}),
    );
    set_active_workspace_context(None, None, false, None);
    assert!(write.success, "{}", write.content);
    assert!(
        read.success && read.content.contains("outside-marker"),
        "{}",
        read.content
    );
    assert!(
        search.success && search.content.contains("outside-marker"),
        "{}",
        search.content
    );
}

#[test]
fn trusted_relative_paths_stay_anchored_to_workspace_without_a_task_directory() {
    let workspace = tempfile::tempdir().unwrap();
    set_active_workspace_context(
        Some(workspace.path().into()),
        None,
        false,
        Some(crate::config::SandboxMode::Trusted),
    );
    let write = execute_with_metadata(
        "write_to_file",
        &serde_json::json!({"path": "relative-marker.txt", "content": "relative-marker"}),
    );
    let read = execute_with_metadata(
        "view_file",
        &serde_json::json!({"path": "relative-marker.txt"}),
    );
    set_active_workspace_context(None, None, false, None);
    assert!(write.success, "{}", write.content);
    assert!(
        read.success && read.content.contains("relative-marker"),
        "{}",
        read.content
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("relative-marker.txt")).unwrap(),
        "relative-marker"
    );
}

#[test]
fn explicitly_restricted_native_tools_retain_the_workspace_boundary() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("restricted-marker.txt");
    for mode in [
        crate::config::SandboxMode::ReadOnly,
        crate::config::SandboxMode::WorkspaceWrite,
        crate::config::SandboxMode::WorkspaceWriteNetwork,
    ] {
        set_active_workspace_context(Some(workspace.path().into()), None, false, Some(mode));
        let write = execute_with_metadata(
            "write_to_file",
            &serde_json::json!({"path": path, "content": "no"}),
        );
        let search = execute_with_metadata(
            "grep",
            &serde_json::json!({"path": outside.path(), "pattern": "marker"}),
        );
        set_active_workspace_context(None, None, false, None);
        assert!(
            !write.success && write.content.contains("escapes the workspace"),
            "{}",
            write.content
        );
        assert!(
            !search.success && search.content.contains("escapes the workspace"),
            "{}",
            search.content
        );
        assert!(!path.exists());
    }
}

#[test]
fn mcp_schema_lookup_accepts_the_canonical_name_of_a_bare_advertised_tool() {
    let schema =
        serde_json::json!({"type": "object", "properties": {"account": {"type": "string"}}});
    // `list_emails` is unique, so requests advertise it without a prefix;
    // `send_message` collides with a built-in and is advertised qualified.
    let tools = || {
        vec![
            (
                "list_emails".to_owned(),
                "mail".to_owned(),
                "list_emails".to_owned(),
                String::new(),
                schema.clone(),
            ),
            (
                "mcp__chat__send_message".to_owned(),
                "chat".to_owned(),
                "send_message".to_owned(),
                String::new(),
                serde_json::json!({"type": "object"}),
            ),
        ]
    };
    let find = |name| {
        super::schema::find_mcp_tool_schema(tools(), name, super::schema::mcp_canonical_name)
    };

    assert_eq!(find("list_emails"), Some(schema.clone()));
    assert_eq!(find("mcp__mail__list_emails"), Some(schema.clone()));
    assert!(find("mcp__chat__send_message").is_some());
    // A qualified tool stays unreachable by its ambiguous bare name.
    assert_eq!(find("send_message"), None);
    assert_eq!(find("mcp__other__list_emails"), None);
}

/// A menu of `count` tools that no test prompt is relevant to, plus a small
/// mail server whose tools the word "mails" points at.
fn pinned_menu_with_mail_tools(count: usize) -> (Vec<(String, String, Value)>, Vec<String>) {
    let schema = serde_json::json!({"type": "object", "properties": {}});
    let mut tools = (0..count)
        .map(|index| {
            (
                format!("zeta_{index:02}"),
                "Zeta".to_string(),
                schema.clone(),
            )
        })
        .collect::<Vec<_>>();
    let pinned = tools.iter().map(|(name, _, _)| name.clone()).collect();
    for name in ["list_emails", "read_email"] {
        tools.push((
            name.to_string(),
            "Mailbox access".to_string(),
            schema.clone(),
        ));
    }
    (tools, pinned)
}

#[test]
fn a_new_user_message_seats_its_tools_ahead_of_a_full_pinned_menu() {
    let (tools, pinned) = pinned_menu_with_mail_tools(MAX_MCP_NATIVE_SCHEMAS);
    let mut messages = vec![
        serde_json::json!({"role": "user", "content": "tidy the parser"}),
        serde_json::json!({"role": "assistant", "content": "Done."}),
    ];

    // A turn that asks for nothing new leaves the menu, and the cached
    // `tools` block, exactly as it was.
    messages.push(serde_json::json!({"role": "user", "content": "now tidy the lexer"}));
    let (_, same) = select_mcp_tools_for_context_with_sticky(&tools, &messages, &pinned);
    assert_eq!(same.selected_names, pinned);

    // A full menu still admits what the newest message asks for.
    messages.push(serde_json::json!({"role": "assistant", "content": "Done."}));
    messages.push(serde_json::json!({"role": "user", "content": "check my latest mails"}));
    let (_, turn) = select_mcp_tools_for_context_with_sticky(&tools, &messages, &pinned);
    assert_eq!(turn.selected, MAX_MCP_NATIVE_SCHEMAS);
    assert!(turn.selected_names.contains(&"list_emails".to_string()));
    assert!(turn.selected_names.contains(&"read_email".to_string()));
    assert_eq!(turn.relevant, 2);

    // Later rounds of that turn, pinned to its menu, select the same names.
    messages.push(serde_json::json!({
        "role": "assistant",
        "content": "Reading the inbox with zeta and the parser in mind.",
        "tool_calls": [{"function": {"name": "list_emails"}}]
    }));
    messages.push(serde_json::json!({"role": "tool", "content": "3 messages"}));
    let (_, round) =
        select_mcp_tools_for_context_with_sticky(&tools, &messages, &turn.selected_names);
    assert_eq!(round.selected_names, turn.selected_names);
    assert_eq!(round.relevant, 0);
}

#[test]
fn a_harness_notice_does_not_unbind_a_discovery_result() {
    let (tools, pinned) = pinned_menu_with_mail_tools(MAX_MCP_NATIVE_SCHEMAS);
    let owners = tools
        .iter()
        .map(|(name, _, _)| {
            if name.starts_with("zeta") {
                "zeta"
            } else {
                "mail"
            }
            .to_string()
        })
        .collect::<Vec<_>>();
    let mut messages = vec![
        serde_json::json!({"role": "user", "content": "what is in my inbox"}),
        serde_json::json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{"function": {
                "name": "list_mcp_tools",
                "arguments": "{\"server\":\"mail\"}"
            }}]
        }),
        serde_json::json!({"role": "tool", "content": "list_emails, read_email"}),
    ];
    let select = |messages: &[Value]| {
        super::schema::select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
            &tools,
            &owners,
            &[],
            messages,
            &pinned,
            ToolSchemaPhase::Established,
        )
        .1
    };
    let bound = select(&messages);
    assert!(bound.selected_names.contains(&"list_emails".to_string()));

    // A rejected call is answered with a notice projected as a `user`
    // message. It used to pass for the user's turn, which left the discovery
    // call behind it and dropped the tools the model had just been shown.
    messages.push(serde_json::json!({
        "role": "user",
        "content": "<rustcode_runtime_notice provenance=\"lifecycle\">\n[Tool call rejected before execution]\n</rustcode_runtime_notice>"
    }));
    messages.push(serde_json::json!({"role": "user", "content": "<rustcode_context>\n# Runtime"}));
    let after = select(&messages);
    assert_eq!(after.selected_names, bound.selected_names);
}

#[test]
fn smart_ui_hint_needs_the_capability_and_a_root_prompt() {
    use super::schema::{SMART_UI_OFF_PROMPT_HINT, SMART_UI_PROMPT_HINT, append_smart_ui_hint};
    use crate::app::SmartUi;
    use crate::config::AgentMode;
    // The format and design guidance moved to the built-in `smart-ui`
    // skill (#2625); the root prompt keeps only when to use it.
    assert!(crate::network::compaction::estimate_tokens(SMART_UI_PROMPT_HINT) < 150);
    assert!(crate::network::compaction::estimate_tokens(SMART_UI_OFF_PROMPT_HINT) < 80);
    assert!(SMART_UI_PROMPT_HINT.contains("```rustcode-ui v1"));
    assert!(SMART_UI_PROMPT_HINT.contains("`use_skill` with `smart-ui` directly"));
    assert!(SMART_UI_OFF_PROMPT_HINT.contains("Settings › Chat › Smart UI"));
    assert!(!SMART_UI_OFF_PROMPT_HINT.contains("General"));
    let with = |policy: ToolSchemaPolicy, on: SmartUi| {
        let mut prompt = String::new();
        append_smart_ui_hint(&mut prompt, policy, on);
        prompt
    };
    let root = ToolSchemaPolicy::root_for_mode(false, AgentMode::Build);
    assert_eq!(with(root, SmartUi::On), SMART_UI_PROMPT_HINT);
    assert_eq!(with(root, SmartUi::Off), SMART_UI_OFF_PROMPT_HINT);
    assert_eq!(with(root, SmartUi::Unsupported), "");
    let compact = ToolSchemaPolicy {
        compact_text_prompt: true,
        ..root
    };
    assert_eq!(with(compact, SmartUi::On), "");
    assert_eq!(with(compact, SmartUi::Off), "");
}

#[test]
fn visualization_hint_needs_a_rendering_frontend_and_a_root_prompt() {
    use super::schema::{
        VISUALIZATION_PROMPT_HINT as HINT, VISUALIZATION_PROMPT_HINT_LEVEL_2 as HINT_2,
        append_visualization_hint,
    };
    use crate::config::{AgentMode, ToolProtocol};
    assert!(crate::network::compaction::estimate_tokens(HINT) < 130);
    assert!(crate::network::compaction::estimate_tokens(HINT_2) < 90);
    let with = |policy: ToolSchemaPolicy, level: u8| {
        let mut prompt = String::new();
        append_visualization_hint(&mut prompt, policy, level, crate::app::SmartUi::Unsupported);
        prompt
    };
    // Smart UI on is taught views instead, at any level.
    let mut with_views = String::new();
    append_visualization_hint(
        &mut with_views,
        ToolSchemaPolicy::root(false),
        2,
        crate::app::SmartUi::On,
    );
    assert_eq!(with_views, "");
    for mode in [AgentMode::Build, AgentMode::Plan] {
        let root = ToolSchemaPolicy::root_for_mode(false, mode);
        // Level 1 frontends (the TUI) keep exactly the level 1 prompt.
        assert_eq!(with(root, 1), HINT);
        assert_eq!(with(root, 2), format!("{HINT}{HINT_2}"));
        assert_eq!(with(root, 9), format!("{HINT}{HINT_2}"));
        assert_eq!(with(root, 0), "");
    }
    for type_name in ["line_chart", "stat_list", "checklist"] {
        assert!(!HINT.contains(type_name) && HINT_2.contains(type_name));
    }
    assert_eq!(with(ToolSchemaPolicy::subagent(), 2), "");
    let compact =
        ToolSchemaPolicy::root_for_mode_with_compact_prompt(false, AgentMode::Build, true);
    assert_eq!(with(compact, 2), "");
    // The cached base prompt never carries it, so frontends that cannot
    // render share the same prefix as before.
    for protocol in [
        ToolProtocol::ApiNative,
        ToolProtocol::Json,
        ToolProtocol::Native,
    ] {
        assert!(
            !super::tool_system_prompt(false, protocol, AgentMode::Build).contains("rustcode-viz")
        );
    }
}

#[test]
fn output_style_reaches_root_prompts_within_budget() {
    use super::schema::{
        OUTPUT_STYLE_PROMPT_HINT as STYLE, OUTPUT_STYLE_VIZ_HINT as VIZ, SMART_UI_OFF_PROMPT_HINT,
        append_output_style_hint, harness_output_style, visualization_prompt_hint,
    };
    use crate::config::{AgentMode, ToolProtocol};
    let tokens = crate::network::compaction::estimate_tokens;
    let with = |policy: ToolSchemaPolicy, level: u8| {
        let mut prompt = String::new();
        append_output_style_hint(&mut prompt, policy, level, crate::app::SmartUi::Unsupported);
        prompt
    };
    for mode in [AgentMode::Build, AgentMode::Plan] {
        let root = ToolSchemaPolicy::root_for_mode(false, mode);
        // Every frontend renders Markdown; only viz frontends get the viz clause.
        assert_eq!(with(root, 0), format!("{STYLE}\n"));
        assert_eq!(with(root, 1), format!("{STYLE}{VIZ}\n"));
        assert_eq!(with(root, 2), format!("{STYLE}{VIZ}\n"));
    }
    assert_eq!(with(ToolSchemaPolicy::subagent(), 2), "");
    let compact =
        ToolSchemaPolicy::root_for_mode_with_compact_prompt(false, AgentMode::Build, true);
    assert_eq!(with(compact, 2), "");
    for rule in [
        "```diff",
        "`path:line`",
        "headings only",
        "No emoji",
        "full URL",
    ] {
        assert!(STYLE.contains(rule), "{rule}");
    }
    // Frontends that cannot draw the blocks never hear of them.
    assert!(!STYLE.contains("rustcode-viz") && !STYLE.contains("rustcode-ui"));
    assert!(VIZ.contains("rustcode-viz"));
    // Budget (docs/output-style.md): under 250 extra tokens for the desktop
    // app with Smart UI off or a viz client, less for plain ACP clients.
    assert!(tokens(&with(ToolSchemaPolicy::root(false), 0)) < 150);
    assert!(tokens(&with(ToolSchemaPolicy::root(false), 2)) < 170);
    let viz_client = format!(
        "{}{}{SMART_UI_OFF_PROMPT_HINT}",
        with(ToolSchemaPolicy::root(false), 2),
        visualization_prompt_hint(2)
    );
    let before = format!("{}{SMART_UI_OFF_PROMPT_HINT}", visualization_prompt_hint(2));
    assert!(tokens(&viz_client) - tokens(&before) <= 250);
    // With Smart UI on the output style carries the one On sentence instead.
    let mut desktop = String::new();
    append_output_style_hint(
        &mut desktop,
        ToolSchemaPolicy::root(false),
        2,
        crate::app::SmartUi::On,
    );
    assert!(tokens(&desktop) <= 250);
    // The cached base prompt never carries it, so the prefix stays shared.
    for protocol in [
        ToolProtocol::ApiNative,
        ToolProtocol::Json,
        ToolProtocol::Native,
    ] {
        assert!(
            !super::tool_system_prompt(false, protocol, AgentMode::Build)
                .contains("# Output style")
        );
    }
    let harness = harness_output_style(crate::app::SmartUi::Unsupported);
    assert!(harness.starts_with(STYLE) && harness.contains("`path:line`"));
    assert!(!harness.contains("rustcode-viz"));
    assert!(tokens(&harness) < 170);
}

#[test]
fn harness_instructions_teach_the_viz_types_the_client_draws() {
    use super::schema::{harness_instructions, harness_output_style};
    let none = harness_instructions("Claude Code", 0, crate::app::SmartUi::Unsupported, None, "");
    assert!(none.contains("through the Claude Code harness"));
    // It knows the app is what owns its threads, and where to ask or look.
    assert!(none.contains("owns this machine's threads"));
    assert!(none.contains("rustcode threads list --all"));
    assert!(none.contains("~/.config/rustcode/threads"));
    // It knows where usage and the recorded turns are, and that a shell can ask.
    assert!(none.contains("`thread_usage`"));
    assert!(none.contains("`rustcode usage`"));
    assert!(none.contains("`query_data`"));
    assert!(none.contains("`rustcode data query"));
    assert!(!none.contains("rustcode-viz"));
    let style = harness_output_style(crate::app::SmartUi::Unsupported);
    assert!(none.ends_with(&style));

    let level_1 = harness_instructions("Codex", 1, crate::app::SmartUi::Unsupported, None, "");
    assert!(level_1.contains("```rustcode-viz") && !level_1.contains("line_chart"));
    assert!(level_1.ends_with(&style));

    let level_2 = harness_instructions("Pi", 2, crate::app::SmartUi::Unsupported, None, "");
    assert!(level_2.contains("line_chart") && level_2.contains("checklist"));
}

#[test]
fn harness_instructions_carry_the_skills_note_before_the_reply_style() {
    use super::schema::{harness_instructions, harness_output_style};
    use crate::app::SmartUi;
    let note = "# Skills\nRustCode keeps this machine's skills here: `/skills`.\n\n";
    let with = harness_instructions("Codex", 1, SmartUi::On, Some(note), "");
    let without = harness_instructions("Codex", 1, SmartUi::On, None, "");
    assert!(with.contains(note));
    assert!(!without.contains("RustCode keeps this machine's skills here"));
    assert!(with.ends_with(&harness_output_style(SmartUi::On)));
}

/// Codex's Windows sandbox denies WMI (#3526), so the thread is told what to read instead. It is the
/// harness's note, only on Windows, and it never displaces the reply style at the end.
#[test]
fn harness_instructions_carry_the_sandbox_note_before_the_reply_style() {
    use super::schema::{
        CODEX_WINDOWS_SANDBOX_NOTE, codex_windows_sandbox_note, harness_instructions,
        harness_output_style,
    };
    use crate::app::SmartUi;
    assert_eq!(codex_windows_sandbox_note().is_empty(), !cfg!(windows));
    let with = harness_instructions("Codex", 1, SmartUi::On, None, CODEX_WINDOWS_SANDBOX_NOTE);
    assert!(with.contains("Get-CimInstance"), "{with}");
    assert!(
        with.contains(r"HKLM:\HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
        "{with}"
    );
    assert!(with.contains("Full access"), "{with}");
    assert!(with.ends_with(&harness_output_style(SmartUi::On)));
    let plain = harness_instructions("Codex", 1, SmartUi::On, None, "");
    assert!(!plain.contains("Get-CimInstance"), "{plain}");
}

#[test]
fn harness_instructions_teach_smart_ui_from_the_setting() {
    use super::schema::{
        SMART_UI_HARNESS_HINT, SMART_UI_OFF_PROMPT_HINT, harness_instructions, harness_output_style,
    };
    use crate::app::SmartUi;
    let instructions = |smart_ui| harness_instructions("Codex", 2, smart_ui, None, "");

    // On: the fence, the rule, the kit's names and what the card refuses.
    let on = instructions(SmartUi::On);
    for part in [
        "```rustcode-ui v1",
        "inline <style> or <script> only if needed",
        "rustcode-lib:chart.js",
        "--rc-series-1…8",
        ".rc-num",
        "--rc-separator",
        "Answer visually, asked or not",
        "never a Markdown table",
        "A report to another thread stays plain Markdown",
        "set no colours",
        "as little CSS as you can",
        ".rc-badge",
        ":: bars",
        ":: stats",
        ":: steps",
        ":: table",
        ":: cards",
        ":: sources",
        "{ok|warn|bad|info}",
        ".rc-findings",
        ".rc-details",
        ".rc-term",
        ".rc-chip",
        ".rc-tip",
        ".rc-tabs",
        "data-rc-send",
        "on a <form>, with its fields",
        "data-rc-copy",
        "data-rc-tab",
        "data-rc-panel",
        "data-rc-sort",
        ".rc-spark",
        ".rc-chart",
        "legend toggles",
        ".rc-dash > .rc-head",
        ".rc-hero-value",
        ".rc-hero-delta",
        ".rc-stats.rc-flat",
        "plugins.rcMarks",
        "No outer border",
        "no vh",
        "smart_ui_check",
    ] {
        assert!(on.contains(part), "{part}");
    }
    assert!(!on.contains("rustcode-viz"));
    assert!(on.ends_with(&harness_output_style(SmartUi::On)));

    // Off: the off note, and no format to write.
    let off = instructions(SmartUi::Off);
    assert!(off.contains(SMART_UI_OFF_PROMPT_HINT.trim()));
    assert!(!off.contains("```rustcode-ui v1"));

    // A client that cannot run the blocks hears nothing about them.
    let absent = instructions(SmartUi::Unsupported);
    assert!(!absent.contains("Smart UI"));
    assert!(!absent.contains("rustcode-ui"));

    // It rides in every prompt: Codex sends it as `developerInstructions`, so
    // keep the whole thing (runtime info, viz, Smart UI, style) under 4 KB.
    let bytes = on.len();
    let tokens = crate::network::compaction::estimate_tokens(&on);
    let before = crate::network::compaction::estimate_tokens(&absent);
    println!(
        "harness instructions: {} bytes without Smart UI, {bytes} with it on ({before} -> {tokens} tokens); \
         the harness hint is {} bytes, the root Smart UI hint {} tokens, its off note {}",
        absent.len(),
        SMART_UI_HARNESS_HINT.len(),
        crate::network::compaction::estimate_tokens(super::schema::SMART_UI_PROMPT_HINT),
        crate::network::compaction::estimate_tokens(super::schema::SMART_UI_OFF_PROMPT_HINT),
    );
    assert!(bytes < 4_096, "{bytes} bytes");
    // 2,166 bytes: the "pick by question" rule, the kit's chart, sparkline,
    // sortable-table and hover pieces, and the folded-sources rule cost about
    // 400 over the 1,592 it was; the dashboard layout (headline, context,
    // stats, chart) and the chart marks add about 170 more, paid on every turn
    // of every external thread, so it stays under 2,198 (1,998 plus 10%). The
    // review findings class adds about 80 more (its markup is in the smart-ui
    // skill, not here), so the cap was 2,300. Answering visually by default
    // (data, results and 4+ items in a view, no Markdown tables, brief text)
    // adds about 30, so it is 2,350. The `::` shorthand fits in the same cap:
    // the kit markup it writes no longer has to be spelled out here.
    assert!(
        SMART_UI_HARNESS_HINT.len() < 2_350,
        "{}",
        SMART_UI_HARNESS_HINT.len()
    );
}

/// The tail of a root prompt that Smart UI decides.
fn root_prompt_hints(level: u8, smart_ui: crate::app::SmartUi) -> String {
    use super::schema::{
        append_output_style_hint, append_smart_ui_hint, append_visualization_hint,
    };
    let root = ToolSchemaPolicy::root(false);
    let mut prompt = String::new();
    append_output_style_hint(&mut prompt, root, level, smart_ui);
    append_visualization_hint(&mut prompt, root, level, smart_ui);
    append_smart_ui_hint(&mut prompt, root, smart_ui);
    prompt
}

#[test]
fn smart_ui_off_and_unsupported_prompts_keep_their_exact_text() {
    use super::schema::harness_instructions;
    use crate::app::SmartUi;
    // Captured before Smart UI became the default for structured replies, so
    // the TUI and other clients do not change. Every byte is on purpose: to
    // change it, edit the file with the prompt and say why.
    let expected = include_str!("../testdata/smart_ui_off_prompts.txt").replace("\r\n", "\n");
    let mut actual = String::new();
    for (level, smart_ui, name) in [
        (0, SmartUi::Unsupported, "0/unsupported"),
        (1, SmartUi::Unsupported, "1/unsupported"),
        (2, SmartUi::Unsupported, "2/unsupported"),
        (0, SmartUi::Off, "0/off"),
        (1, SmartUi::Off, "1/off"),
        (2, SmartUi::Off, "2/off"),
    ] {
        actual.push_str(&format!(
            "=== root {name}\n{}=== harness {name}\n{}",
            root_prompt_hints(level, smart_ui),
            harness_instructions("Codex", level, smart_ui, None, "")
        ));
    }
    assert_eq!(actual, expected);
}

#[test]
fn smart_ui_on_teaches_views_and_not_rustcode_viz() {
    use super::schema::{
        OUTPUT_STYLE_PROMPT_HINT as STYLE, OUTPUT_STYLE_SMART_UI_HINT as VIEWS,
        SMART_UI_HARNESS_HINT, SMART_UI_PROMPT_HINT, harness_instructions, output_style_hint,
    };
    use crate::app::SmartUi;
    let tokens = crate::network::compaction::estimate_tokens;
    for level in [0, 1, 2] {
        let root = root_prompt_hints(level, SmartUi::On);
        assert_eq!(root, format!("{STYLE}{VIEWS}\n{SMART_UI_PROMPT_HINT}"));
        assert_eq!(
            output_style_hint(level, SmartUi::On),
            format!("{STYLE}{VIEWS}\n")
        );
        let harness = harness_instructions("Codex", level, SmartUi::On, None, "");
        for text in [&root, &harness] {
            assert!(!text.contains("rustcode-viz"), "{text}");
            assert!(!text.contains("# Visualizations"), "{text}");
            assert!(text.contains(VIEWS.trim()), "{text}");
        }
    }
    // The same rule in both, in the same words.
    for rule in [
        "Answer visually, asked or not: data, numbers, results, status, comparisons, plans or 4+ items go in a view (chart, bars, stats, table), never a Markdown table.",
        "Only short explanations, one fact, code and diffs stay Markdown.",
        "Be brief: one line with the finding before the view, nothing after it.",
        "Sources go last inside the view, folded: details.rc-sources > ol > li > a[href].",
        "A report to another thread stays plain Markdown.",
    ] {
        assert!(SMART_UI_PROMPT_HINT.contains(rule), "{rule}");
        assert!(SMART_UI_HARNESS_HINT.contains(rule), "{rule}");
    }
    // Dropping `# Visualizations` pays for the longer rule: the desktop app's
    // tail is shorter than the 436 tokens it was (docs/smart-ui.md).
    let root = root_prompt_hints(2, SmartUi::On);
    println!(
        "root prompt hints with Smart UI on: {} bytes, {} tokens",
        root.len(),
        tokens(&root)
    );
    assert!(tokens(&root) < 436, "{}", tokens(&root));
}

#[test]
fn desktop_control_is_only_offered_to_capable_root_sessions() {
    let has_desktop = |policy| {
        native_tools_schema_for_context(policy, &[])
            .0
            .iter()
            .any(|tool| tool["function"]["name"] == crate::desktop_control::TOOL_NAME)
    };
    let root = ToolSchemaPolicy::root(false);
    assert!(!has_desktop(root));
    assert!(has_desktop(root.with_desktop_tool()));
    for policy in [
        ToolSchemaPolicy::read_only_inspection(),
        ToolSchemaPolicy::subagent(),
        ToolSchemaPolicy::subagent_with_delegation(),
    ] {
        assert!(!has_desktop(policy.with_desktop_tool()));
    }
    assert!(!allowed_in_plan_mode(crate::desktop_control::TOOL_NAME));
}

#[test]
fn localhost_preview_is_offered_only_with_a_client_preview() {
    let has_preview = |schemas: &[serde_json::Value]| {
        schemas
            .iter()
            .any(|tool| tool["function"]["name"] == crate::preview::PREVIEW_TOOL_NAME)
    };
    let regular = ToolSchemaPolicy::root(false);
    assert!(!has_preview(
        &native_tools_schema_for_context(regular, &[]).0
    ));
    assert!(!has_preview(&native_tools_schema(true)));
    assert!(has_preview(
        &native_tools_schema_for_context(regular.with_preview_tool(), &[]).0
    ));
    // Plan mode and subagents never drive the user's preview.
    for policy in [
        ToolSchemaPolicy::read_only_inspection(),
        ToolSchemaPolicy::subagent(),
        ToolSchemaPolicy::subagent_with_delegation(),
    ] {
        assert!(!policy.with_preview_tool().include_preview_tool);
        assert!(!has_preview(
            &native_tools_schema_for_context(policy.with_preview_tool(), &[]).0
        ));
    }
    // The shared system prompt never names the tool; sessions that have it
    // get the separate hint.
    for protocol in [
        crate::config::ToolProtocol::Json,
        crate::config::ToolProtocol::Native,
    ] {
        let prompt = tool_system_prompt_for_policy(
            regular.with_preview_tool(),
            protocol,
            crate::config::AgentMode::Build,
        );
        assert!(!prompt.contains(crate::preview::PREVIEW_TOOL_NAME));
    }
    let mut prompt = String::new();
    super::schema::append_preview_hint(&mut prompt, regular);
    assert!(prompt.is_empty());
    super::schema::append_preview_hint(&mut prompt, regular.with_preview_tool());
    assert!(prompt.contains("localhost_preview"));
}

/// A pin full of tools nobody called, as the previous request's menu, and two
/// later turns. The tool found by discovery on turn one is still bound on
/// turn three, and the tool the model called by its canonical name, while
/// it is advertised bare, counts as previously used (#2502).
#[test]
fn bound_and_used_mcp_tools_stay_selected_on_later_turns() {
    let schema = serde_json::json!({"type":"object","properties":{}});
    let mut mcp = (0..MAX_MCP_NATIVE_SCHEMAS)
        .map(|index| {
            (
                format!("draft_{index:02}"),
                "Archive a historical item".to_string(),
                schema.clone(),
            )
        })
        .collect::<Vec<_>>();
    let mut owners = vec!["notes".to_string(); MAX_MCP_NATIVE_SCHEMAS];
    let mut raw_names = mcp.iter().map(|tool| tool.0.clone()).collect::<Vec<_>>();
    let pin = raw_names.clone();
    for (server, name, description) in [
        (
            "teams",
            "send_chat_message",
            "Send one message to a visible chat",
        ),
        (
            "inbox",
            "get_chat_messages",
            "Read the latest messages of a chat",
        ),
    ] {
        mcp.push((name.to_string(), description.to_string(), schema.clone()));
        owners.push(server.to_string());
        raw_names.push(name.to_string());
    }
    let messages = vec![
        serde_json::json!({"role":"user","content":"Read what JMRanked said"}),
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{"function":{
                "name":"list_mcp_tools",
                "arguments":"{\"server\":\"teams\",\"select\":[\"send_chat_message\"]}"
            }}]
        }),
        serde_json::json!({"role":"tool","content":"{\"results\":[]}"}),
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{"function":{"name":"mcp__inbox__get_chat_messages","arguments":"{}"}}]
        }),
        serde_json::json!({"role":"tool","content":"two messages"}),
        serde_json::json!({"role":"user","content":"check again"}),
        serde_json::json!({"role":"assistant","content":"Nothing new."}),
        serde_json::json!({"role":"user","content":"and once more"}),
    ];

    let (selected, stats) = super::schema::select_mcp_tools_for_context_with_raw_names(
        &mcp,
        &owners,
        &raw_names,
        &[],
        &messages,
        &pin,
        super::schema::ToolSchemaPhase::Established,
    );
    let names: Vec<&str> = selected
        .iter()
        .map(|index| mcp[*index].0.as_str())
        .collect();
    assert!(names.contains(&"send_chat_message"), "{names:?}");
    assert!(names.contains(&"get_chat_messages"), "{names:?}");
    assert_eq!(stats.previously_used, 1);
    assert_eq!(selected.len(), MAX_MCP_NATIVE_SCHEMAS);
}

/// The MCP catalog of #2576 in miniature: the FTP and RCON servers a Minecraft
/// task uses, two messaging servers offering the same `get_chat_messages`, a
/// timer, and unrelated filler.
fn minecraft_session_catalog() -> (Vec<(String, String, Value)>, Vec<String>, Vec<String>) {
    let entries: &[(&str, &str, &str, Value)] = &[
        (
            "ftp",
            "upload-file",
            "Upload a file to the FTP server",
            serde_json::json!({}),
        ),
        (
            "ftp",
            "upload-local-file",
            "Upload a local file to the FTP server",
            serde_json::json!({}),
        ),
        (
            "ftp",
            "download-file",
            "Download a file from the FTP server",
            serde_json::json!({}),
        ),
        (
            "ftp",
            "list-directory",
            "List a directory on the FTP server",
            serde_json::json!({}),
        ),
        (
            "ftp",
            "edit-file",
            "Edit a file on the FTP server",
            serde_json::json!({}),
        ),
        (
            "rcon",
            "rcon_command",
            "Run a Minecraft console command over RCON",
            serde_json::json!({}),
        ),
        (
            "rcon",
            "rcon_status",
            "Minecraft server status over RCON",
            serde_json::json!({}),
        ),
        (
            "rcon",
            "rcon_list_players",
            "Players online on the Minecraft server",
            serde_json::json!({}),
        ),
        (
            "snapchat",
            "get_chat_messages",
            "Get the message transcript of a Snapchat chat, newest first",
            serde_json::json!({"type":"object","properties":{"chat_id":{"type":"string"},"before":{"type":"string"}}}),
        ),
        (
            "teams",
            "get_chat_messages",
            "Get the message transcript of a Teams chat, newest first",
            serde_json::json!({"type":"object","properties":{"chat_id":{"type":"string"},"before":{"type":"string"}}}),
        ),
        (
            "solidtime",
            "solidtime_start_timer",
            "Start a Solidtime timer for a project",
            serde_json::json!({}),
        ),
        (
            "notes",
            "list_articles",
            "List knowledge base articles",
            serde_json::json!({}),
        ),
        (
            "notes",
            "read_article",
            "Read a knowledge base article",
            serde_json::json!({}),
        ),
    ];
    let duplicated = |raw: &str| entries.iter().filter(|entry| entry.1 == raw).count() > 1;
    let tools = entries
        .iter()
        .map(|(server, raw, description, schema)| {
            let advertised = if duplicated(raw) {
                format!("mcp__{server}__{raw}")
            } else {
                raw.to_string()
            };
            (advertised, description.to_string(), schema.clone())
        })
        .collect();
    let owners = entries.iter().map(|entry| entry.0.to_string()).collect();
    let raw_names = entries.iter().map(|entry| entry.1.to_string()).collect();
    (tools, owners, raw_names)
}

fn assistant_call(name: &str, content: &str) -> Value {
    serde_json::json!({
        "role":"assistant",
        "content": content,
        "tool_calls":[{"function":{"name": name, "arguments":"{}"}}]
    })
}

// Regression (#2576): a compaction archived the user messages that named the
// FTP and RCON tools. Their slots went to keyword matches on "chat", "msg",
// "reply" and "before", and the model called a Snapchat reader mid-task.
#[test]
fn a_compaction_that_archives_the_naming_messages_keeps_the_mcp_menu_stable() {
    let (tools, owners, raw_names) = minecraft_session_catalog();
    let before = vec![
        serde_json::json!({"role":"user","content":
            "give my agent FTP access: use upload-file, upload-local-file, download-file, \
             list-directory, edit-file and rcon_command, rcon_list_players"}),
        assistant_call("download-file", "Reading the LuckPerms config."),
        serde_json::json!({"role":"tool","content":"permissions.yml"}),
        assistant_call("rcon_status", "Checking the server is up."),
        serde_json::json!({"role":"tool","content":"ok"}),
    ];
    let mut anchors = super::schema::McpSessionAnchors::default();
    let (_, first) = super::schema::select_mcp_tools_for_context_with_anchors(
        &tools,
        &owners,
        &raw_names,
        &[],
        &before,
        &[],
        &anchors,
        ToolSchemaPhase::Established,
    );
    anchors.absorb(&first);
    let ftp_and_rcon = tools[..8]
        .iter()
        .map(|(name, _, _)| name.clone())
        .collect::<Vec<_>>();
    for name in &ftp_and_rcon {
        assert!(first.selected_names.contains(name), "{name}: {first:?}");
    }

    // The compaction keeps a record and the latest turn; the assistant prose
    // is full of chat, msg, reply and before.
    let after = vec![
        serde_json::json!({"role":"system","content":
            "[Deterministic context record]\nGoal: FTP access to my minecraft server"}),
        serde_json::json!({"role":"user","content":
            "its up and running again. give 07Lugas almost all admin permissions, \
             fix the default permissions, talk with me first about stuff"}),
        assistant_call(
            "rcon_command",
            "EssentialsChat is loaded. Before the chat format change I will grant \
             /msg and /reply to default; check the chat messages before that.",
        ),
        serde_json::json!({"role":"tool","content":"ok"}),
    ];
    for sticky in [first.selected_names.clone(), Vec::new()] {
        let (_, next) = super::schema::select_mcp_tools_for_context_with_anchors(
            &tools,
            &owners,
            &raw_names,
            &[],
            &after,
            &sticky,
            &anchors,
            ToolSchemaPhase::Established,
        );
        for name in &ftp_and_rcon {
            assert!(next.selected_names.contains(name), "{name}: {next:?}");
        }
        for intruder in [
            "mcp__snapchat__get_chat_messages",
            "mcp__teams__get_chat_messages",
            "solidtime_start_timer",
        ] {
            assert!(
                !next.selected_names.contains(&intruder.to_string()),
                "{intruder}: {next:?}"
            );
        }
        // Every selected tool is reported under the bucket that admitted it.
        let reported = next.admitted_by.values().map(Vec::len).sum::<usize>();
        assert_eq!(reported, next.selected);
        assert!(
            next.admitted_by["previously_used"].contains(&"rcon_status".to_string()),
            "{next:?}"
        );
        assert!(
            next.admitted_by["requested"].contains(&"upload-file".to_string()),
            "{next:?}"
        );
    }
}

#[test]
fn generic_words_and_parameter_names_do_not_admit_an_mcp_tool() {
    let (tools, owners, raw_names) = minecraft_session_catalog();
    let messages = vec![serde_json::json!({"role":"user","content":
        "list the latest chat messages before the reply, send a msg"})];
    let (_, stats) = super::schema::select_mcp_tools_for_context_with_raw_names(
        &tools,
        &owners,
        &raw_names,
        &[],
        &messages,
        &[],
        ToolSchemaPhase::Established,
    );
    assert!(
        !stats
            .selected_names
            .iter()
            .any(|name| name.ends_with("get_chat_messages")),
        "{stats:?}"
    );
}

#[test]
fn transcript_relevance_does_not_introduce_a_server_the_session_never_engaged() {
    let (tools, owners, raw_names) = minecraft_session_catalog();
    // "timer" and "Solidtime" appear only in the assistant's prose.
    let messages = vec![
        serde_json::json!({"role":"user","content":"restart the minecraft server"}),
        serde_json::json!({"role":"assistant","content":"I will start a Solidtime timer project."}),
    ];
    let (_, stats) = super::schema::select_mcp_tools_for_context_with_raw_names(
        &tools,
        &owners,
        &raw_names,
        &[],
        &messages,
        &[],
        ToolSchemaPhase::Established,
    );
    assert!(
        !stats
            .selected_names
            .contains(&"solidtime_start_timer".to_string()),
        "{stats:?}"
    );

    // The user asking for it in their own words is enough.
    let asked = vec![serde_json::json!({"role":"user","content":
        "start a solidtime timer for this project"})];
    let (_, stats) = super::schema::select_mcp_tools_for_context_with_raw_names(
        &tools,
        &owners,
        &raw_names,
        &[],
        &asked,
        &[],
        ToolSchemaPhase::Established,
    );
    assert!(
        stats
            .selected_names
            .contains(&"solidtime_start_timer".to_string()),
        "{stats:?}"
    );
}

#[test]
fn a_tool_name_two_servers_share_is_not_tie_broken_by_server_name() {
    let (tools, owners, raw_names) = minecraft_session_catalog();
    // "newest" and "transcript" point at both readers equally.
    let messages = vec![serde_json::json!({"role":"user","content":
        "show the newest messages from snapchat or teams chat, newest first"})];
    let (_, stats) = super::schema::select_mcp_tools_for_context_with_raw_names(
        &tools,
        &owners,
        &raw_names,
        &[],
        &[serde_json::json!({"role":"user","content":"newest transcript"})],
        &[],
        ToolSchemaPhase::Established,
    );
    assert!(
        !stats
            .selected_names
            .iter()
            .any(|name| name.ends_with("get_chat_messages")),
        "{stats:?}"
    );
    // Naming the servers makes the request explicit instead.
    let (_, named) = super::schema::select_mcp_tools_for_context_with_raw_names(
        &tools,
        &owners,
        &raw_names,
        &[],
        &messages,
        &[],
        ToolSchemaPhase::Established,
    );
    assert!(
        named
            .selected_names
            .contains(&"mcp__snapchat__get_chat_messages".to_string())
    );
    assert!(
        named
            .selected_names
            .contains(&"mcp__teams__get_chat_messages".to_string())
    );
}

fn select_from_minecraft_catalog(
    messages: &[Value],
    sticky: &[String],
) -> super::schema::McpSchemaSelectionStats {
    let (tools, owners, raw_names) = minecraft_session_catalog();
    super::schema::select_mcp_tools_for_context_with_raw_names(
        &tools,
        &owners,
        &raw_names,
        &[],
        messages,
        sticky,
        super::schema::ToolSchemaPhase::Established,
    )
    .1
}

/// A `manage_mcp_servers` listing the request carried as a `<tool_result>`
/// user row named every configured server, and each server's tools were
/// admitted as if the user had asked for them: twelve unrelated schemas and a
/// fully missed prompt cache.
#[test]
fn a_tool_result_row_naming_servers_does_not_request_their_tools() {
    let messages = vec![
        serde_json::json!({"role":"user","content":"check what went wrong in this session and log a github issue"}),
        serde_json::json!({"role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"manage_mcp_servers","arguments":"{\"operation\":\"list\"}"}}]}),
        serde_json::json!({"role":"user","content":"<tool_result>\nmanage_mcp_servers: ftp  stdio  [running]\nrcon  stdio  [running]\nsnapchat  stdio  [running]\nteams  stdio  [running]\nsolidtime  stdio  [running]\nnotes  stdio  [running]\n</tool_result>"}),
    ];
    for sticky in [Vec::new(), vec!["list_articles".to_string()]] {
        let stats = select_from_minecraft_catalog(&messages, &sticky);
        assert_eq!(stats.requested, 0, "{:?}", stats.admitted_by);
        assert_eq!(stats.relevant, 0, "{:?}", stats.admitted_by);
        assert_eq!(stats.selected_names, sticky);
    }
}

/// A tool the menu admits mid-session goes after the pinned ones, in the
/// order they were sent, so only the end of the `tools` block changes.
#[test]
fn a_newly_admitted_mcp_tool_is_appended_after_the_pinned_menu() {
    let pinned = vec!["rcon_status".to_string(), "read_article".to_string()];
    let first = vec![serde_json::json!({"role":"user","content":"how is the server"})];
    let stats = select_from_minecraft_catalog(&first, &pinned);
    assert_eq!(stats.selected_names, pinned);

    // `list_articles` sorts before both pinned tools in the catalog.
    let next = vec![
        first[0].clone(),
        serde_json::json!({"role":"assistant","content":"It is up."}),
        serde_json::json!({"role":"user","content":"now use list_articles"}),
    ];
    let stats = select_from_minecraft_catalog(&next, &pinned);
    assert_eq!(
        stats.selected_names,
        ["rcon_status", "read_article", "list_articles"]
    );

    // The next request pins that order and keeps it.
    let stats = select_from_minecraft_catalog(&next, &stats.selected_names.clone());
    assert_eq!(
        stats.selected_names,
        ["rcon_status", "read_article", "list_articles"]
    );
}

/// Every built-in schema offers a call `summary`; dispatch drops it, but the
/// binding of discovery results read it as an unknown argument and bound
/// nothing, so the tools a search found were not callable next request.
#[test]
fn a_discovery_call_with_a_summary_binds_what_it_found() {
    let messages = vec![
        serde_json::json!({"role":"user","content":"who is online"}),
        serde_json::json!({"role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"list_mcp_tools","arguments":"{\"query\":\"minecraft players\",\"summary\":\"Find the player list tool\"}"}}]}),
        serde_json::json!({"role":"tool","tool_call_id":"call_1","content":"list_mcp_tools: {\"results\":[]}"}),
    ];
    let stats = select_from_minecraft_catalog(&messages, &[]);
    assert!(
        stats.admitted_by["requested"].contains(&"rcon_list_players".to_string()),
        "{:?}",
        stats.admitted_by
    );
}

/// Description words alone do not admit a tool from a server the session
/// has not used: "session" and "token" in a vault description matched a
/// message about a RustCode session's token usage.
#[test]
fn description_words_alone_do_not_admit_a_tool_from_a_new_server() {
    let tools = vec![(
        "list_secrets".to_string(),
        "List each stored token and session secret".to_string(),
        serde_json::json!({}),
    )];
    let owners = vec!["vault".to_string()];
    let raw_names = vec!["list_secrets".to_string()];
    let select = |content: &str| {
        let messages = vec![serde_json::json!({"role":"user","content":content})];
        super::schema::select_mcp_tools_for_context_with_raw_names(
            &tools,
            &owners,
            &raw_names,
            &[],
            &messages,
            &[],
            super::schema::ToolSchemaPhase::Established,
        )
        .1
    };
    assert_eq!(
        select("why did this session use so many tokens").selected,
        0
    );
    assert_eq!(
        select("which secrets hold my session token").selected_names,
        ["list_secrets"]
    );
}

#[test]
fn thread_messaging_tools_are_offered_only_with_a_host() {
    let names = |policy: ToolSchemaPolicy| -> Vec<String> {
        native_tools_schema_for_context(policy, &[])
            .0
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
            .collect()
    };
    let offered = |policy: ToolSchemaPolicy| -> Vec<&'static str> {
        let names = names(policy);
        crate::orchestration::TOOL_NAMES
            .iter()
            .chain(crate::orchestration::V2_TOOL_NAMES.iter())
            .copied()
            .filter(|tool| names.iter().any(|name| name == tool))
            .collect()
    };
    let regular = ToolSchemaPolicy::root(true);
    assert!(offered(regular).is_empty());
    assert!(offered(regular.with_orchestration_tools(0)).is_empty());
    // A version 1 client sees exactly the M1 tools.
    assert_eq!(
        offered(regular.with_orchestration_tools(1)),
        crate::orchestration::TOOL_NAMES.to_vec()
    );
    let v2 = offered(regular.with_orchestration_tools(2));
    assert_eq!(v2.len(), 6, "{v2:?}");
    // Plan mode and subagents never message another thread.
    for policy in [
        ToolSchemaPolicy::read_only_inspection(),
        ToolSchemaPolicy::subagent(),
        ToolSchemaPolicy::subagent_with_delegation(),
    ] {
        for version in [1, 2] {
            let policy = policy.with_orchestration_tools(version);
            assert!(!policy.include_orchestration_tools);
            assert!(!policy.include_orchestration_v2_tools);
            assert!(offered(policy).is_empty());
        }
    }
    let mut prompt = String::new();
    super::schema::append_preview_hint(&mut prompt, regular);
    assert!(prompt.is_empty());
    super::schema::append_preview_hint(&mut prompt, regular.with_orchestration_tools(1));
    assert_eq!(prompt, crate::orchestration::PROMPT_HINT);
    let mut prompt = String::new();
    super::schema::append_preview_hint(&mut prompt, regular.with_orchestration_tools(2));
    assert_eq!(prompt, crate::orchestration::PROMPT_HINT_V2);
    let mut prompt = String::new();
    super::schema::append_preview_hint(&mut prompt, regular.with_orchestration_tools(3));
    assert_eq!(prompt, crate::orchestration::PROMPT_HINT_V3);
}
