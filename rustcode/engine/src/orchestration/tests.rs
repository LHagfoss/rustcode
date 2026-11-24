use super::*;

#[test]
fn message_steering_is_boolean_and_versioned() {
    let args = |steer| json!({"session_id": "b", "message": "do it", "steer": steer});
    assert!(validate(MESSAGE_SESSION, &args(json!(true))).is_ok());
    assert!(validate(MESSAGE_SESSION, &args(json!(false))).is_ok());
    assert!(validate(MESSAGE_SESSION, &args(json!("true"))).is_err());
    let schema = (tool(MESSAGE_SESSION).schema)();
    assert!(schema_for(MESSAGE_SESSION, 13, schema.clone())["properties"]["steer"].is_null());
    assert_eq!(
        schema_for(MESSAGE_SESSION, 14, schema)["properties"]["steer"]["type"],
        "boolean"
    );
    assert!(prompt_hint(14).contains("steer: true"));
}

#[test]
fn a_one_way_message_is_the_default() {
    let args = |value| json!({"session_id": "b", "message": "fyi", "return_to": value});
    assert!(validate(MESSAGE_SESSION, &args(json!("none"))).is_ok());
    assert!(validate(MESSAGE_SESSION, &args(json!("me"))).is_ok());
    assert!(validate(MESSAGE_SESSION, &args(json!("bad"))).is_err());
    assert!(!waits_for_reply(&json!({})));
    assert!(waits_for_reply(&args(json!("me"))));
    assert!(!waits_for_reply(&args(json!("none"))));
    assert!(names_wait(MESSAGE_SESSION, &args(json!("none"))));
    // An older app never sees `return_to`: its calls leave it out, and are not refused.
    assert!(!names_wait(
        MESSAGE_SESSION,
        &json!({"session_id": "b", "message": "x"})
    ));
    assert!(!names_wait(MESSAGE_SESSION, &args(json!("me"))));
    let schema = (tool(MESSAGE_SESSION).schema)();
    assert!(schema_for(MESSAGE_SESSION, 16, schema.clone())["properties"]["return_to"].is_null());
    assert_eq!(
        schema_for(MESSAGE_SESSION, ONE_WAY_VERSION, schema)["properties"]["return_to"]["type"],
        "string"
    );
}

#[tokio::test]
async fn a_version_sixteen_app_refuses_a_one_way_message_before_sending_it() {
    let args = json!({"session_id": "b", "message": "fyi", "return_to": "none"});
    let cancel = tokio_util::sync::CancellationToken::new();
    let output = execute_versioned(
        client(json!({"link_id": "should-not-be-made"})),
        16,
        &cancel,
        MESSAGE_SESSION,
        &args,
    )
    .await;
    assert!(!output.success);
    assert!(
        output
            .content
            .contains("one-way message_session needs a newer"),
        "{}",
        output.content
    );
    let sent = execute_versioned(
        client(json!({"session_id": "b", "delivered": "started"})),
        ONE_WAY_VERSION,
        &cancel,
        MESSAGE_SESSION,
        &args,
    )
    .await;
    assert!(sent.success, "{}", sent.content);
}

#[test]
fn a_project_on_a_windows_machine_is_absolute_from_any_machine() {
    for path in [
        "/Users/ada/code/app",
        r"C:\Users\ada\code\app",
        "C:/Users/ada/code/app",
        r"\\?\C:\Users\ada\code\app",
        r"\\server\share\app",
    ] {
        let args = json!({"prompt": "hi", "project": path, "machine": "pc"});
        assert_eq!(validate(START_SESSION, &args), Ok(()), "{path}");
    }
    for path in ["code/app", "C:", "C:code", r"Users\ada"] {
        let args = json!({"prompt": "hi", "project": path});
        assert!(validate(START_SESSION, &args).is_err(), "{path}");
    }
}

struct Answering(Value);

impl PreviewClient for Answering {
    fn request(&self, params: Value) -> crate::preview::PreviewFuture {
        let answer = self.0.clone();
        Box::pin(async move {
            assert!(params.get("tool").is_some());
            Ok(answer)
        })
    }
}

fn client(answer: Value) -> Option<Arc<dyn PreviewClient>> {
    Some(Arc::new(Answering(answer)) as Arc<dyn PreviewClient>)
}

#[test]
fn the_client_must_advertise_the_host() {
    let meta = |value: Value| value.as_object().cloned();
    let version = |value: Value| client_meta_version(meta(value).as_ref());
    assert_eq!(version(json!({"rustcode": {"orchestration": 1}})), 1);
    assert_eq!(version(json!({"rustcode": {"orchestration": 2}})), 2);
    // A newer client gets what this engine speaks.
    assert_eq!(version(json!({"rustcode": {"orchestration": 9}})), 9);
    assert_eq!(version(json!({"rustcode": {"orchestration": 10}})), 10);
    assert_eq!(version(json!({"rustcode": {"orchestration": 11}})), 11);
    // A newer client gets what this engine speaks.
    assert_eq!(version(json!({"rustcode": {"orchestration": 15}})), 15);
    assert_eq!(version(json!({"rustcode": {"orchestration": 16}})), 16);
    assert_eq!(version(json!({"rustcode": {"orchestration": 17}})), 17);
    assert_eq!(version(json!({"rustcode": {"orchestration": 18}})), 18);
    assert_eq!(version(json!({"rustcode": {"orchestration": 19}})), 19);
    assert_eq!(version(json!({"rustcode": {"orchestration": 20}})), 20);
    assert_eq!(version(json!({"rustcode": {"orchestration": 99}})), VERSION);
    assert_eq!(version(json!({"rustcode": {"preview": 1}})), 0);
    assert_eq!(client_meta_version(None), 0);
}

#[test]
fn compaction_requires_a_compatible_app_and_a_boolean_resume_flag() {
    assert!(!tool_names(LOOPS_VERSION).contains(&COMPACT_SESSION));
    assert!(tool_names(COMPACT_VERSION).contains(&COMPACT_SESSION));
    assert!(validate(COMPACT_SESSION, &json!({})).is_ok());
    assert!(
        validate(
            COMPACT_SESSION,
            &json!({"session_id": "thread", "resume": true})
        )
        .is_ok()
    );
    assert!(validate(COMPACT_SESSION, &json!({"resume": "yes"})).is_err());
}

#[test]
fn version_one_keeps_exactly_the_m1_tools() {
    assert!(tool_names(0).is_empty());
    assert_eq!(tool_names(1), TOOL_NAMES.to_vec());
    let v2 = tool_names(2);
    assert_eq!(v2.len(), 6);
    for name in TOOL_NAMES.iter().chain(V2_TOOL_NAMES.iter()) {
        assert!(v2.contains(name), "{name}");
        assert!(is_tool(name));
    }
    assert!(V2_TOOL_NAMES.iter().all(|name| is_v2_tool(name)));
    assert!(!TOOL_NAMES.iter().any(|name| is_v2_tool(name)));
}

#[test]
fn version_three_adds_the_management_tools_only_for_its_clients() {
    assert!(
        V3_TOOL_NAMES
            .iter()
            .all(|name| !tool_names(2).contains(name))
    );
    let v3 = tool_names(3);
    assert_eq!(v3.len(), 16);
    for name in V3_TOOL_NAMES {
        assert!(v3.contains(&name), "{name}");
        assert!(is_tool(name) && is_v3_tool(name));
        assert!(PROMPT_HINT_V3.contains(name), "{name}");
    }
}

#[test]
fn version_four_adds_the_data_store_tools_only_for_its_clients() {
    let v3 = tool_names(3);
    assert!(v3.iter().all(|name| !is_v4_tool(name)), "{v3:?}");
    let v4 = tool_names(4);
    assert_eq!(v4.len(), v3.len() + 2, "{v4:?}");
    assert!(v4.contains(&THREAD_USAGE) && v4.contains(&QUERY_DATA));
    // Version 5 adds account targeting to existing tools, not another tool.
    assert_eq!(tool_names(5), v4);
    // Version 6 adds the activity read, version 7 the queue control.
    assert_eq!(tool_names(6), [v4.clone(), V6_TOOL_NAMES.to_vec()].concat());
    assert_eq!(
        tool_names(7),
        [tool_names(6), V7_TOOL_NAMES.to_vec()].concat()
    );
    // Version 8 adds native-subagent guidance to the hint, not another tool.
    assert_eq!(tool_names(8), tool_names(7));
    assert_eq!(
        tool_names(9),
        [tool_names(8), V9_TOOL_NAMES.to_vec()].concat()
    );
    // Version 10 adds answering requests and organizing several threads.
    assert_eq!(
        tool_names(10),
        [tool_names(9), V10_TOOL_NAMES.to_vec()].concat()
    );
    // Version 11 adds the projects and the scheduler's work.
    assert_eq!(
        tool_names(11),
        [tool_names(10), V11_TOOL_NAMES.to_vec()].concat()
    );
    // Version 12 adds the other paired machines.
    assert_eq!(
        tool_names(12),
        [tool_names(11), V12_TOOL_NAMES.to_vec()].concat()
    );
    // Version 13 adds worktree status and handoff.
    assert_eq!(
        tool_names(13),
        [tool_names(12), V13_TOOL_NAMES.to_vec()].concat()
    );
    assert_eq!(
        tool_names(14),
        tool_names(13),
        "a newer client gets what this engine speaks"
    );
    // The hint says how to reach a machine and that omitting it stays here.
    for said in ["list_machines", "`machine`", "this computer"] {
        assert!(PROMPT_HINT_V12.contains(said), "{said}");
    }
    for name in V10_TOOL_NAMES {
        assert!(is_tool(name) && is_v10_tool(name), "{name}");
        assert!(
            !is_v9_tool(name) && !tool_names(9).contains(&name),
            "{name} is v10 only"
        );
        assert!(PROMPT_HINT_V10.contains(name), "{name}");
        assert!(!PROMPT_HINT_V9.contains(name), "{name} is v10 only");
    }
    for name in V11_TOOL_NAMES {
        assert!(is_tool(name) && is_v11_tool(name), "{name}");
        assert!(
            !is_v10_tool(name) && !tool_names(10).contains(&name),
            "{name} is v11 only"
        );
        assert!(PROMPT_HINT_V11.contains(name), "{name}");
        assert!(!PROMPT_HINT_V10.contains(name), "{name} is v11 only");
    }
    for name in V12_TOOL_NAMES {
        assert!(is_tool(name) && is_v12_tool(name), "{name}");
        assert!(
            !is_v11_tool(name) && !tool_names(11).contains(&name),
            "{name} is v12 only"
        );
        assert!(PROMPT_HINT_V12.contains(name), "{name}");
        assert!(!PROMPT_HINT_V11.contains(name), "{name} is v12 only");
    }
    for name in V13_TOOL_NAMES {
        assert!(is_tool(name) && is_v13_tool(name), "{name}");
        assert!(
            !is_v12_tool(name) && !tool_names(12).contains(&name),
            "{name} is v13 only"
        );
        assert!(PROMPT_HINT_V13.contains(name), "{name}");
        assert!(!PROMPT_HINT_V12.contains(name), "{name} is v13 only");
    }
    // The v13 hint keeps what v11 and v12 taught.
    for said in [
        "read_project",
        "schedule_task",
        "list_machines",
        "handoff_worktree",
    ] {
        assert!(PROMPT_HINT_V13.contains(said), "{said}");
        assert!(PROMPT_HINT_V15.contains(said), "{said} in the v15 hint");
    }
    // v15 adds no tool: it teaches the argument that starts a thread of the
    // user's own, and a v14 client is never told about one it cannot answer.
    assert!(PROMPT_HINT_V15.contains("standalone"));
    assert!(!PROMPT_HINT_V14.contains("standalone"));
    // The v15 hint keeps what v14 taught about messaging any project.
    assert!(PROMPT_HINT_V15.contains("`steer: true`"));
    assert_eq!(
        prompt_hint(14),
        PROMPT_HINT_V14,
        "a v14 app keeps its exact hint"
    );
    assert_eq!(
        prompt_hint(15),
        PROMPT_HINT_V15,
        "a v15 app keeps its exact hint"
    );
    assert!(prompt_hint(VERSION).contains(CREATE_LOOP));
    // v16 teaches `machine` on the project tools and matching by remote.
    for said in ["`clone_project` take", "this one's git remote", "offline"] {
        assert!(PROMPT_HINT_V16.contains(said), "{said} in the v16 hint");
        assert!(!PROMPT_HINT_V15.contains(said), "{said} is v16 only");
    }
    for said in ["standalone", "`steer: true`", "handoff_worktree"] {
        assert!(PROMPT_HINT_V16.contains(said), "{said} in the v16 hint");
    }
    for name in V16_MACHINE_TOOL_NAMES {
        assert!(PROMPT_HINT_V16.contains(name), "{name} in the v16 hint");
    }
    // Every agent is asked to name its own thread once (issue 3599).
    assert!(PROMPT_HINT_V16.contains(
        "If you have not named this thread yet, make `update_session` your first tool call"
    ));
    // "Settle" means settling this thread, or the one the user names, and nothing else.
    assert!(PROMPT_HINT_V16.contains("When the user says to settle"));
    assert!(PROMPT_HINT_V16.contains("it does not merge, push or clean anything up"));
    // A wave is waited on in one call, not one call per delegate.
    assert!(PROMPT_HINT_V16.contains("`link_ids` and `all: true`"));
    // Every tool `machine` applies to is named in the v12 hint.
    for name in MACHINE_TOOL_NAMES {
        assert!(PROMPT_HINT_V12.contains(name), "{name} in the v12 hint");
    }
    for name in [THREAD_USAGE, QUERY_DATA] {
        assert!(is_tool(name) && is_v4_tool(name) && engine_answers(name));
        assert!(PROMPT_HINT_V4.contains(name), "{name}");
        assert!(!PROMPT_HINT_V3.contains(name), "{name} is v4 only");
    }
    for name in V6_TOOL_NAMES {
        assert!(is_tool(name) && is_v6_tool(name));
        assert!(
            !is_v4_tool(name) && !v4.contains(&name),
            "{name} is v6 only"
        );
        assert!(PROMPT_HINT_V6.contains(name), "{name}");
        assert!(!PROMPT_HINT_V5.contains(name), "{name} is v6 only");
    }
    for name in V7_TOOL_NAMES {
        assert!(is_tool(name) && is_v7_tool(name));
        assert!(
            !is_v6_tool(name) && !tool_names(6).contains(&name),
            "{name} is v7 only"
        );
        assert!(PROMPT_HINT_V7.contains(name), "{name}");
        assert!(!PROMPT_HINT_V6.contains(name), "{name} is v7 only");
    }
    for name in V9_TOOL_NAMES {
        assert!(is_tool(name) && is_v9_tool(name));
        assert!(
            !is_v7_tool(name) && !tool_names(8).contains(&name),
            "{name} is v9 only"
        );
        assert!(PROMPT_HINT_V9.contains(name), "{name}");
        assert!(!PROMPT_HINT_V8.contains(name), "{name} is v9 only");
        assert!(!PROMPT_HINT_V7.contains(name), "{name} is v9 only");
    }
    // An orchestrator is told how to keep its own context small .
    for said in ["Stay lean", "link_ids", "end the turn", "thread_status"] {
        assert!(PROMPT_HINT_V9.contains(said), "{said}");
    }
    // The hint a client is told follows the version it advertised.
    assert_eq!(prompt_hint(1), PROMPT_HINT);
    assert_eq!(prompt_hint(2), PROMPT_HINT_V2);
    assert_eq!(prompt_hint(3), PROMPT_HINT_V3);
    assert_eq!(prompt_hint(4), PROMPT_HINT_V4);
    assert_eq!(prompt_hint(5), PROMPT_HINT_V5);
    assert_eq!(prompt_hint(6), PROMPT_HINT_V6);
    assert_eq!(prompt_hint(7), PROMPT_HINT_V7);
    assert_eq!(prompt_hint(8), PROMPT_HINT_V8);
    assert_eq!(prompt_hint(9), PROMPT_HINT_V9);
    assert_eq!(prompt_hint(10), PROMPT_HINT_V10);
    assert_eq!(prompt_hint(11), PROMPT_HINT_V11);
    assert_eq!(prompt_hint(12), PROMPT_HINT_V12);
    assert_eq!(prompt_hint(13), PROMPT_HINT_V13);
    assert_eq!(prompt_hint(14), PROMPT_HINT_V14);
    assert_eq!(prompt_hint(15), PROMPT_HINT_V15);
    assert_eq!(prompt_hint(16), PROMPT_HINT_V16);
    // Versions 17 and 18 add arguments explained by each tool's schema.
    assert_eq!(prompt_hint(17), PROMPT_HINT_V16);
    assert_eq!(prompt_hint(18), PROMPT_HINT_V16);
    assert_eq!(prompt_hint(0), "");
    // Accounts arrived in 5: a v4 hint promises nothing it cannot do.
    assert!(!PROMPT_HINT_V4.contains("account"));
    assert!(PROMPT_HINT_V5.contains("account"));
    assert_eq!(
        client_meta_version(Some(
            &json!({"rustcode": {"orchestration": 11}})
                .as_object()
                .cloned()
                .unwrap()
        )),
        11
    );
    assert_eq!(
        client_meta_version(Some(
            &json!({"rustcode": {"orchestration": 12}})
                .as_object()
                .cloned()
                .unwrap()
        )),
        12
    );
    assert_eq!(
        client_meta_version(Some(
            &json!({"rustcode": {"orchestration": 3}})
                .as_object()
                .cloned()
                .unwrap()
        )),
        3
    );
    // It rides in the root prompt and in the MCP server's instructions, so
    // it is bounded: v4 adds the two data tools, v5 the account line and
    // the line that says a thread an agent starts is its delegate, v6 the
    // activity read, v7 the queue control, v8 that native subagents go
    // through RustCode's tools, v9 a delegate's pull request.

    let tokens = rustcode_core::tokens::estimate_tokens(PROMPT_HINT_V11);
    println!(
        "prompt hints: v3 {} tokens, v4 {} tokens, v5 {} tokens, v6 {} tokens, v9 {tokens} tokens",
        rustcode_core::tokens::estimate_tokens(PROMPT_HINT_V3),
        rustcode_core::tokens::estimate_tokens(PROMPT_HINT_V4),
        rustcode_core::tokens::estimate_tokens(PROMPT_HINT_V5),
        rustcode_core::tokens::estimate_tokens(PROMPT_HINT_V6)
    );
    // v9 adds the three delegate pull-request tools, all named with their use,
    // and how an orchestrator keeps its own context small: those few lines
    // save far more than they cost on each request.
    assert!(tokens < 1000, "{tokens}");
    // v12 is the v11 hint plus the machine sentences, still bounded.
    let v12 = rustcode_core::tokens::estimate_tokens(PROMPT_HINT_V12);
    assert!(v12 < 1150, "{v12}");
    // v13 adds the worktree pair, and the seconds each call saves.
    let v13 = rustcode_core::tokens::estimate_tokens(PROMPT_HINT_V13);
    assert!(v13 < 1200, "{v13}");
}

/// A client cuts instructions past its limit, and what it cut was the
/// rules that protect the user. Every version stays under it, and the
/// current one still carries those rules.
#[test]
fn the_mcp_instructions_fit_what_a_client_keeps() {
    for version in 0..=VERSION {
        let text = mcp_instructions(version);
        assert!(
            text.chars().count() <= MCP_INSTRUCTIONS_LIMIT,
            "version {version} is {} characters",
            text.chars().count()
        );
        assert_eq!(text, text.trim(), "version {version}");
    }
    let text = mcp_instructions(VERSION);
    for said in [
        "never read or print the destination file",
        "never edit `settings.json` or `config.toml` by hand",
        "only when the user asks",
        "never fall back to this computer when it is offline",
        "`link_ids` and `all: true`",
        "it does not merge, push or clean anything up",
        "Name this thread first",
        CREATE_LOOP,
    ] {
        assert!(text.contains(said), "{said}");
    }
}

#[test]
fn worktree_tool_arguments_are_checked() {
    let ok = |name: &str, args: Value| validate(name, &args).is_ok();
    // Where every worktree stands: the caller's own, one thread's, or all.
    assert!(ok(WORKTREE_STATUS, json!({})));
    assert!(ok(WORKTREE_STATUS, json!({"session_id": "d1"})));
    assert!(ok(
        WORKTREE_STATUS,
        json!({"session_id": "d1", "all_projects": true})
    ));
    assert!(!ok(WORKTREE_STATUS, json!({"session_id": " "})));
    assert!(!ok(WORKTREE_STATUS, json!({"all_projects": "yes"})));
    // Handoff: `to` is `checkout` unless it says `worktree`.
    assert!(ok(HANDOFF_WORKTREE, json!({"session_id": "d1"})));
    assert!(ok(
        HANDOFF_WORKTREE,
        json!({"session_id": "d1", "to": "checkout"})
    ));
    assert!(ok(
        HANDOFF_WORKTREE,
        json!({"session_id": "d1", "to": "worktree"})
    ));
    assert!(!ok(HANDOFF_WORKTREE, json!({})));
    assert!(!ok(HANDOFF_WORKTREE, json!({"session_id": " "})));
    assert!(!ok(
        HANDOFF_WORKTREE,
        json!({"session_id": "d1", "to": "nowhere"})
    ));
    assert!(!ok(HANDOFF_WORKTREE, json!({"session_id": "d1", "to": 1})));
}

#[test]
fn delegate_pull_request_arguments_are_checked() {
    let ok = |name: &str, args: Value| validate(name, &args).is_ok();
    assert!(ok(THREAD_STATUS, json!({})));
    assert!(ok(THREAD_STATUS, json!({"session_id": "d1"})));
    assert!(!ok(THREAD_STATUS, json!({"session_id": " "})));
    assert!(ok(MERGE_WHEN_GREEN, json!({"session_id": "d1"})));
    assert!(ok(
        MERGE_WHEN_GREEN,
        json!({"session_id": "d1", "timeout_ms": 60000})
    ));
    assert!(!ok(MERGE_WHEN_GREEN, json!({})));
    assert!(ok(MERGE_WHEN_GREEN, json!({"number": 42})));
    assert!(!ok(MERGE_WHEN_GREEN, json!({"number": 0})));
    assert!(!ok(
        MERGE_WHEN_GREEN,
        json!({"number": 42, "session_id": "d1"})
    ));
    assert!(!ok(
        MERGE_WHEN_GREEN,
        json!({"session_id": "d1", "timeout_ms": 0})
    ));
    // Over the cap is cut to it by `timeout`, not refused.
    assert!(ok(
        MERGE_WHEN_GREEN,
        json!({"session_id": "d1", "timeout_ms": MAX_MERGE_WAIT_MS + 1})
    ));
    assert!(!ok(
        MERGE_WHEN_GREEN,
        json!({"session_id": "d1", "timeout_ms": "soon"})
    ));
    assert!(ok(CLEANUP_WORKTREE, json!({"session_id": "d1"})));
    assert!(ok(
        CLEANUP_WORKTREE,
        json!({"session_id": "d1", "settle": false})
    ));
    assert!(!ok(CLEANUP_WORKTREE, json!({})));
    assert!(!ok(
        CLEANUP_WORKTREE,
        json!({"session_id": "d1", "settle": "yes"})
    ));
    // The merge waits 25 minutes and the engine outlasts the loop's tail,
    // still inside the external harness's MCP timeout.
    let deadline = timeout(MERGE_WHEN_GREEN, &json!({"session_id": "d1"}));
    assert!(deadline >= Duration::from_millis(MAX_MERGE_WAIT_MS) + Duration::from_secs(170));
    assert!(deadline < Duration::from_secs(tool_socket::TOOL_TIMEOUT_SECS));
    // A longer ask is cut to the cap.
    assert_eq!(
        timeout(
            MERGE_WHEN_GREEN,
            &json!({"session_id": "d1", "timeout_ms": 3_300_000})
        ),
        deadline
    );
}

#[test]
fn project_and_scheduled_task_arguments_are_checked() {
    let ok = |name: &str, args: Value| validate(name, &args).is_ok();
    assert!(ok(READ_PROJECT, json!({"path": "/abs/project"})));
    assert!(!ok(READ_PROJECT, json!({"path": "project"})));
    assert!(!ok(READ_PROJECT, json!({})));
    assert!(ok(
        CREATE_PROJECT,
        json!({"path": "/abs/new", "init": false})
    ));
    assert!(!ok(
        CREATE_PROJECT,
        json!({"path": "/abs/new", "init": "yes"})
    ));
    assert!(ok(CLONE_PROJECT, json!({"url": "git@github.com:o/r.git"})));
    assert!(ok(
        CLONE_PROJECT,
        json!({"url": "git@github.com:o/r.git", "parent": "/abs/code", "name": "r"})
    ));
    assert!(!ok(CLONE_PROJECT, json!({"url": " "})));
    assert!(!ok(
        CLONE_PROJECT,
        json!({"url": "git@github.com:o/r.git", "parent": "code"})
    ));
    assert!(ok(
        UPDATE_PROJECT,
        json!({"path": "/abs/p", "name": "RustCode"})
    ));
    assert!(ok(
        UPDATE_PROJECT,
        json!({"path": "/abs/p", "model": "opus"})
    ));
    assert!(!ok(UPDATE_PROJECT, json!({"path": "/abs/p"})));
    assert!(ok(REMOVE_PROJECT, json!({"path": "/abs/p"})));

    let task = json!({
        "prompt": "Check the build",
        "schedule": {"kind": "cron", "expression": "0 9 * * 1-5", "timezone": "Europe/Oslo"},
        "project": "/abs/project",
    });
    assert!(ok(SCHEDULE_TASK, task.clone()));
    let mut loop_args = task.clone();
    loop_args["name"] = json!("CI watch");
    loop_args["harness"] = json!("codex");
    loop_args["account"] = json!("work");
    assert!(ok(CREATE_LOOP, loop_args.clone()));
    loop_args["thread_id"] = json!("existing");
    assert!(!ok(CREATE_LOOP, loop_args));
    assert!(!tool_names(18).contains(&CREATE_LOOP));
    assert!(tool_names(LOOPS_VERSION).contains(&CREATE_LOOP));
    assert!(!tool_names(COMPACT_VERSION).contains(&UPDATE_LOOP));
    assert!(tool_names(UPDATE_LOOP_VERSION).contains(&UPDATE_LOOP));
    assert!(!tool_names(UPDATE_LOOP_VERSION).contains(&RUN_MACHINE_COMMAND));
    assert!(tool_names(MACHINE_EXEC_VERSION).contains(&RUN_MACHINE_COMMAND));
    assert!(ok(
        RUN_MACHINE_COMMAND,
        json!({"command": "uname -a", "machine": "vps", "cwd": "/home/user", "timeout_secs": 30})
    ));
    assert!(!ok(RUN_MACHINE_COMMAND, json!({"command": "   "})));
    assert!(!ok(
        RUN_MACHINE_COMMAND,
        json!({"command": "ls", "cwd": "relative/dir"})
    ));
    assert!(!ok(
        RUN_MACHINE_COMMAND,
        json!({"command": "ls", "timeout_secs": 0})
    ));
    assert!(ok(UPDATE_LOOP, json!({"id": "loop-1", "paused": true})));
    assert!(ok(
        UPDATE_LOOP,
        json!({"id": "loop-1", "name": "Renamed", "schedule": {"kind": "manual"}})
    ));
    assert!(!ok(UPDATE_LOOP, json!({"id": "loop-1"})));
    assert!(!ok(UPDATE_LOOP, json!({"paused": true})));
    assert!(!ok(
        UPDATE_LOOP,
        json!({"id": "loop-1", "thread_id": "t-1", "harness": "codex"})
    ));
    assert!(ok(
        SCHEDULE_TASK,
        json!({"prompt": "Once", "schedule": {"kind": "at", "at": "2026-03-04T09:30:00+01:00"}})
    ));
    assert!(!ok(
        SCHEDULE_TASK,
        json!({"schedule": task["schedule"].clone()})
    ));
    assert!(!ok(
        SCHEDULE_TASK,
        json!({"prompt": "Check", "schedule": {"kind": "daily", "hour": 9}})
    ));
    assert!(!ok(
        SCHEDULE_TASK,
        json!({"prompt": "Check", "schedule": {"kind": "cron", "expression": "0 9 * *"}})
    ));
    // The kind's own field is required here, not later in `create`: a cron with no
    // expression and an `at` with no time are invalid arguments, not a dead daemon.
    assert!(!ok(
        SCHEDULE_TASK,
        json!({"prompt": "Check", "schedule": {"kind": "cron", "timezone": "UTC"}})
    ));
    assert!(!ok(
        SCHEDULE_TASK,
        json!({"prompt": "Check", "schedule": {"kind": "at"}})
    ));
    assert!(!ok(
        SCHEDULE_TASK,
        json!({"prompt": "Check", "schedule": {"kind": "at", "at": "tomorrow"}})
    ));
    assert!(!ok(
        SCHEDULE_TASK,
        json!({"prompt": "Check", "schedule": {"kind": "at", "at": "2026-03-04T09:30:00+01:00"}, "project": "relative"})
    ));
    assert!(ok(LIST_SCHEDULED_TASKS, json!({})));
    assert!(ok(DELETE_SCHEDULED_TASK, json!({"id": "task-1"})));
    assert!(!ok(DELETE_SCHEDULED_TASK, json!({})));
    assert!(ok(RUN_SCHEDULED_TASK_NOW, json!({"id": "task-1"})));
    assert!(!ok(RUN_SCHEDULED_TASK_NOW, json!({"id": " "})));
}

#[test]
fn answering_and_batching_arguments_are_checked() {
    let ok = |name: &str, args: Value| validate(name, &args).is_ok();
    let ask = json!({"session_id": "d1", "request_id": "7"});
    assert!(ok(
        ANSWER_REQUEST,
        json!({"session_id": "d1", "request_id": "7", "choice": "allow_once"})
    ));
    assert!(ok(
        ANSWER_REQUEST,
        json!({"session_id": "d1", "request_id": "7", "answer": "yes, run it"})
    ));
    assert!(!ok(ANSWER_REQUEST, ask));
    assert!(!ok(
        ANSWER_REQUEST,
        json!({"session_id": "d1", "request_id": "7", "choice": "allow_hour"})
    ));
    // One of the two, never both.
    assert!(!ok(
        ANSWER_REQUEST,
        json!({"session_id": "d1", "request_id": "7", "choice": "deny", "answer": "no"})
    ));
    assert!(!ok(
        ANSWER_REQUEST,
        json!({"request_id": "7", "choice": "deny"})
    ));
    assert!(!ok(
        ANSWER_REQUEST,
        json!({"session_id": "d1", "choice": "deny"})
    ));
    assert!(!ok(
        ANSWER_REQUEST,
        json!({"session_id": "d1", "request_id": "7", "answer": "  "})
    ));
    // The id is the string the list printed, not the number the engine sent.
    assert!(!ok(
        ANSWER_REQUEST,
        json!({"session_id": "d1", "request_id": 7, "choice": "deny"})
    ));

    assert!(ok(
        ORGANIZE_THREADS,
        json!({"session_ids": ["a"], "pinned": true})
    ));
    assert!(ok(
        ORGANIZE_THREADS,
        json!({"session_ids": ["a", "b"], "title": "Done", "archived": true})
    ));
    assert!(!ok(ORGANIZE_THREADS, json!({"session_ids": []})));
    assert!(!ok(ORGANIZE_THREADS, json!({"session_ids": ["a"]})));
    assert!(!ok(ORGANIZE_THREADS, json!({"pinned": true})));
    assert!(!ok(
        ORGANIZE_THREADS,
        json!({"session_ids": ["a"], "pinned": "yes"})
    ));
    assert!(!ok(
        ORGANIZE_THREADS,
        json!({"session_ids": [1], "pinned": true})
    ));

    // Attachments are absolute paths, at most ten of them.
    let message = |extra: Value| {
        let mut args = json!({"session_id": "d1", "message": "look"});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().cloned().unwrap());
        args
    };
    assert!(ok(MESSAGE_SESSION, message(json!({}))));
    assert!(ok(
        MESSAGE_SESSION,
        message(json!({"attachments": ["/repo/a.png", "/repo/b.log"]}))
    ));
    assert!(!ok(
        MESSAGE_SESSION,
        message(json!({"attachments": ["docs/notes.md"]}))
    ));
    assert!(!ok(
        MESSAGE_SESSION,
        message(json!({"attachments": "a.png"}))
    ));
    let many: Vec<String> = (0..=MAX_ATTACHMENTS)
        .map(|i| format!("/repo/{i}.txt"))
        .collect();
    assert!(!ok(MESSAGE_SESSION, message(json!({"attachments": many}))));
}

#[test]
fn version_four_arguments_are_checked() {
    let ok = |name: &str, args: Value| validate(name, &args).is_ok();
    assert!(ok(THREAD_USAGE, json!({})));
    assert!(ok(
        THREAD_USAGE,
        json!({"since": "2026-11-01", "group_by": "day"})
    ));
    assert!(ok(
        THREAD_USAGE,
        json!({"since": "2026-11", "group_by": "model"})
    ));
    assert!(ok(THREAD_USAGE, json!({"project": "/repo/a"})));
    assert!(!ok(THREAD_USAGE, json!({"since": "yesterday"})));
    assert!(!ok(THREAD_USAGE, json!({"group_by": "tool"})));
    assert!(!ok(THREAD_USAGE, json!({"project": ""})));
    assert!(ok(QUERY_DATA, json!({"sql": "SELECT 1"})));
    assert!(ok(QUERY_DATA, json!({"sql": "SELECT 1", "limit": 5})));
    assert!(!ok(QUERY_DATA, json!({})));
    assert!(!ok(QUERY_DATA, json!({"sql": "  "})));
    assert!(!ok(QUERY_DATA, json!({"sql": "SELECT 1", "limit": "5"})));
    assert!(!ok(
        QUERY_DATA,
        json!({"sql": "x".repeat(MAX_SQL_BYTES + 1)})
    ));
    // The descriptions are how an agent learns the store without another call.
    let tool = |name: &str| {
        crate::tools::TOOLS
            .iter()
            .find(|tool| tool.name == name)
            .unwrap()
    };
    assert!(tool(THREAD_USAGE).description.contains("group_by"));
    assert!(tool(THREAD_USAGE).description.contains("never a price"));
    for table in [
        "threads(",
        "turns(",
        "tool_calls(",
        "compactions(",
        "daily_usage",
    ] {
        assert!(tool(QUERY_DATA).description.contains(table), "{table}");
    }
    // Columns an agent will write SQL against, by their real names: the
    // description is the only place it learns them.
    for column in [
        "input_tokens",
        "output_tokens",
        "context_tokens",
        "stop_reason",
        "tokens_before",
        "tokens_after",
    ] {
        assert!(tool(QUERY_DATA).description.contains(column), "{column}");
    }
    // No invented column: `turns.tokens` does not exist.
    assert!(
        !tool(QUERY_DATA)
            .description
            .contains("model, tokens, cost_usd")
    );
    assert!(tool(QUERY_DATA).description.contains("read-only"));
    assert!((tool(QUERY_DATA).schema)()["properties"]["limit"]["maximum"] == 1000);
}

/// One tool of the table, by name.
fn tool(name: &str) -> &'static crate::tools::Tool {
    crate::tools::TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .unwrap_or_else(|| panic!("{name}"))
}

#[test]
fn version_seven_offers_the_queue_control_to_its_clients_only() {
    let names = tool_names(7);
    assert!(names.contains(&MANAGE_QUEUE));
    assert!(!tool_names(6).contains(&MANAGE_QUEUE));
    assert!(is_tool(MANAGE_QUEUE) && is_v7_tool(MANAGE_QUEUE));
    // It says what it may touch, and that a change asks the user.
    let description = tool(MANAGE_QUEUE).description;
    for said in [
        "queue_id",
        "200 characters",
        "Only this thread's own queued messages",
        "started",
        "steer",
        "asked once",
    ] {
        assert!(description.contains(said), "{said}: {description}");
    }
    let schema = (tool(MANAGE_QUEUE).schema)();
    assert_eq!(schema["properties"]["action"]["enum"], json!(QUEUE_ACTIONS));
    assert_eq!(
        schema["properties"]["text"]["maxLength"],
        json!(MAX_MESSAGE_BYTES),
        "an edit is as long as the message it replaces"
    );
    assert_eq!(schema["required"], json!(["session_id", "action"]));
}

#[test]
fn queue_control_arguments_are_checked() {
    let ok = |args: Value| validate(MANAGE_QUEUE, &args).is_ok();
    assert!(ok(json!({"session_id": "b", "action": "list"})));
    // A list names no message.
    assert!(ok(
        json!({"session_id": "b", "action": "list", "queue_id": "ignored"})
    ));
    // A cancel or a steer names one message and nothing else; an edit says what instead.
    for action in ["cancel", "steer"] {
        assert!(
            ok(json!({"session_id": "b", "action": action, "queue_id": "q1"})),
            "{action}"
        );
        assert!(
            !ok(json!({"session_id": "b", "action": action})),
            "{action}"
        );
        assert!(
            !ok(json!({"session_id": "b", "action": action, "queue_id": " "})),
            "{action}"
        );
    }
    assert!(ok(
        json!({"session_id": "b", "action": "edit", "queue_id": "q1", "text": "hi"})
    ));
    assert!(!ok(
        json!({"session_id": "b", "action": "edit", "queue_id": "q1"})
    ));
    assert!(!ok(
        json!({"session_id": "b", "action": "edit", "queue_id": "q1", "text": " "})
    ));
    let long = "x".repeat(MAX_MESSAGE_BYTES + 1);
    assert!(!ok(
        json!({"session_id": "b", "action": "edit", "queue_id": "q1", "text": long})
    ));
    assert!(!ok(
        json!({"session_id": "b", "action": "reorder", "queue_id": "q1"})
    ));
    assert!(!ok(json!({"session_id": "b"})));
    // A list ignores a queue_id; one that acts on a message takes only a real id.
    let enormous = "x".repeat(MAX_QUEUE_ID_CHARS + 1);
    assert!(ok(
        json!({"session_id": "b", "action": "list", "queue_id": enormous.clone()})
    ));
    assert!(!ok(
        json!({"session_id": "b", "action": "cancel", "queue_id": enormous})
    ));
    assert!(!ok(json!({"action": "list"})));
    assert_eq!(timeout(MANAGE_QUEUE, &json!({})), REQUEST_TIMEOUT);
}

#[tokio::test]
async fn a_client_below_seven_never_gets_the_queue_control() {
    let cancel = tokio_util::sync::CancellationToken::new();
    let args = json!({"session_id": "b", "action": "list"});
    let refused = execute_versioned(
        client(json!({"queued": []})),
        6,
        &cancel,
        MANAGE_QUEUE,
        &args,
    )
    .await;
    assert!(!refused.success);
    assert!(refused.content.contains("newer RustCode app"));
    let answered = execute_versioned(
        client(json!({"queued": []})),
        7,
        &cancel,
        MANAGE_QUEUE,
        &args,
    )
    .await;
    assert!(answered.success, "{}", answered.content);
}

#[test]
fn the_version_three_hint_and_read_tools_say_what_they_answer() {
    // Reads answer questions about the user's work, across projects when asked.
    assert!(PROMPT_HINT_V3.contains("all_projects"));
    assert!(PROMPT_HINT_V3.contains("before answering about it"));
    assert!(PROMPT_HINT_V3.contains("do them only when the user asks"));
    let tool = |name: &str| {
        crate::tools::TOOLS
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name}"))
    };
    for name in [
        LIST_SESSIONS,
        SEARCH_SESSIONS,
        READ_SESSION,
        LIST_WORKTREES,
        LIST_PROJECTS,
        LIST_PENDING_REQUESTS,
        LIST_HARNESSES,
        LIST_PULL_REQUESTS,
    ] {
        assert!(
            !tool(name).description.contains("involve another thread"),
            "{name} reads: no delegation-only rule"
        );
    }
    assert!(
        tool(LIST_SESSIONS)
            .description
            .contains("what the user has been working on")
    );
    assert!(
        (tool(LIST_SESSIONS).schema)()["properties"]["all_projects"].is_object(),
        "list_sessions takes all_projects"
    );
    assert!((tool(SEARCH_SESSIONS).schema)()["properties"]["all_projects"].is_object());
    assert!((tool(LIST_WORKTREES).schema)()["properties"]["all_projects"].is_object());
    assert!(tool(READ_SESSION).description.contains("next_offset"));
    assert!((tool(READ_SESSION).schema)()["properties"]["offset"].is_object());
    assert!(tool(WAIT_SESSION).description.contains("answer_truncated"));
    // The longest prompt and message are 32 KiB in the schema and in the check.
    assert_eq!(
        (tool(MESSAGE_SESSION).schema)()["properties"]["message"]["maxLength"],
        32768
    );
    assert_eq!(
        (tool(START_SESSION).schema)()["properties"]["prompt"]["maxLength"],
        32768
    );
    // Tools that give another thread work keep the approval rule.
    assert!(
        tool(MESSAGE_SESSION)
            .description
            .contains("The user is asked once")
    );
    assert!(
        tool(START_SESSION)
            .description
            .contains("The user approves")
    );
}

#[test]
fn version_three_arguments_are_checked() {
    let ok = |name: &str, args: Value| validate(name, &args).is_ok();
    assert!(ok(LIST_HARNESSES, json!({})));
    // Narrowing a long model list is optional, as an id and a substring.
    assert!(ok(LIST_HARNESSES, json!({"harness": "codex"})));
    assert!(ok(
        LIST_HARNESSES,
        json!({"harness": "codex", "model": "opus"})
    ));
    assert!(!ok(LIST_HARNESSES, json!({"harness": ""})));
    assert!(!ok(LIST_HARNESSES, json!({"model": 7})));
    assert!(!ok(LIST_HARNESSES, json!({"harness": ["codex"]})));
    assert!(ok(SEARCH_SESSIONS, json!({"query": "login"})));
    assert!(!ok(SEARCH_SESSIONS, json!({"query": " "})));
    // Reaching every project is a flag, never a string, on the reads.
    assert!(ok(
        SEARCH_SESSIONS,
        json!({"query": "login", "all_projects": true})
    ));
    assert!(!ok(
        SEARCH_SESSIONS,
        json!({"query": "login", "all_projects": "yes"})
    ));
    assert!(ok(LIST_SESSIONS, json!({"all_projects": true})));
    assert!(!ok(LIST_SESSIONS, json!({"all_projects": 1})));
    assert!(ok(LIST_WORKTREES, json!({"all_projects": false})));
    assert!(!ok(LIST_WORKTREES, json!({"all_projects": "false"})));
    assert!(ok(INTERRUPT_SESSION, json!({"session_id": "b"})));
    assert!(!ok(INTERRUPT_SESSION, json!({})));
    assert!(ok(CONFIGURE_SESSION, json!({})));
    assert!(ok(
        CONFIGURE_SESSION,
        json!({"model": "m", "effort": "high"})
    ));
    assert!(ok(LIST_PENDING_REQUESTS, json!({})));
    assert!(ok(
        TRANSFER_SESSION,
        json!({"session_id": "a", "harness": "codex"})
    ));
    assert!(!ok(TRANSFER_SESSION, json!({"session_id": "a"})));
    assert!(!ok(TRANSFER_SESSION, json!({"harness": "codex"})));
    assert!(ok(LIST_PROJECTS, json!({})));
    assert!(ok(LIST_PULL_REQUESTS, json!({})));
    assert!(ok(LIST_PULL_REQUESTS, json!({"state": "merged"})));
    assert!(!ok(LIST_PULL_REQUESTS, json!({"state": "draft"})));
    assert!(!ok(LIST_PULL_REQUESTS, json!({"state": 1})));
    assert!(ok(UPDATE_SESSION, json!({"pinned": true})));
    assert!(ok(
        UPDATE_SESSION,
        json!({"session_id": "b", "status": "settled"})
    ));
    assert!(!ok(UPDATE_SESSION, json!({"session_id": "b"})));
    assert!(!ok(UPDATE_SESSION, json!({"status": "gone"})));
    assert!(!ok(UPDATE_SESSION, json!({"pinned": "yes"})));
    assert!(ok(
        UPDATE_SESSION,
        json!({"snoozed_until": "2026-11-05T09:00:00Z"})
    ));
    assert!(ok(UPDATE_SESSION, json!({"snoozed_until": null})));
    assert!(!ok(UPDATE_SESSION, json!({"snoozed_until": 5})));
}

#[test]
fn version_two_arguments_are_checked() {
    let ok = |name: &str, args: Value| validate(name, &args).is_ok();
    assert!(ok(READ_SESSION, json!({"session_id": "b"})));
    assert!(ok(READ_SESSION, json!({"session_id": "b", "offset": 0})));
    assert!(ok(
        READ_SESSION,
        json!({"session_id": "b", "last_turns": 10, "offset": 16384})
    ));
    assert!(!ok(READ_SESSION, json!({"session_id": "b", "offset": -1})));
    assert!(!ok(READ_SESSION, json!({"session_id": "b", "offset": "0"})));
    assert!(ok(
        READ_SESSION,
        json!({"session_id": "b", "last_turns": 10})
    ));
    assert!(!ok(
        READ_SESSION,
        json!({"session_id": "b", "last_turns": 0})
    ));
    assert!(!ok(
        READ_SESSION,
        json!({"session_id": "b", "last_turns": 11})
    ));
    assert!(!ok(READ_SESSION, json!({})));

    assert!(ok(START_SESSION, json!({"prompt": "fix it"})));
    assert!(ok(
        START_SESSION,
        json!({"prompt": "fix it", "title": "Fix", "harness": "codex", "model": "m",
                   "effort": "high", "worktree": false, "project": "/abs/repo"})
    ));
    assert!(!ok(START_SESSION, json!({"prompt": " "})));
    let long = "x".repeat(MAX_MESSAGE_BYTES + 1);
    assert!(!ok(START_SESSION, json!({"prompt": long})));
    assert!(!ok(
        START_SESSION,
        json!({"prompt": "a", "project": "relative/path"})
    ));
    assert!(!ok(
        START_SESSION,
        json!({"prompt": "a", "worktree": "yes"})
    ));
    assert!(!ok(START_SESSION, json!({"prompt": "a", "harness": ""})));
    // A named login is optional, and when given a non-empty string.
    assert!(ok(
        START_SESSION,
        json!({"prompt": "a", "harness": "claude-code", "account": "work"})
    ));
    assert!(!ok(START_SESSION, json!({"prompt": "a", "account": ""})));
    assert!(!ok(START_SESSION, json!({"prompt": "a", "account": 5})));
    let title = "t".repeat(MAX_TITLE_CHARS + 1);
    assert!(!ok(START_SESSION, json!({"prompt": "a", "title": title})));

    let two = json!([{"harness": "codex"}, {"harness": "claude-code", "model": "m"}]);
    assert!(ok(FAN_OUT, json!({"prompt": "a", "harnesses": two})));
    assert!(!ok(
        FAN_OUT,
        json!({"prompt": "a", "harnesses": [{"harness": "codex"}]})
    ));
    let five = json!([{"harness": "a"}, {"harness": "b"}, {"harness": "c"},
                          {"harness": "d"}, {"harness": "e"}]);
    assert!(!ok(FAN_OUT, json!({"prompt": "a", "harnesses": five})));
    let twice = json!([{"harness": "codex"}, {"harness": "codex"}]);
    assert!(!ok(FAN_OUT, json!({"prompt": "a", "harnesses": twice})));
    // The same harness on two models is a fair comparison.
    let models = json!([{"harness": "codex", "model": "a"}, {"harness": "codex", "model": "b"}]);
    assert!(ok(FAN_OUT, json!({"prompt": "a", "harnesses": models})));
    // So are two named accounts of one harness and model.
    let accounts = json!([
        {"harness": "claude-code", "model": "opus", "account": "work"},
        {"harness": "claude-code", "model": "opus", "account": "home"}
    ]);
    assert!(ok(FAN_OUT, json!({"prompt": "a", "harnesses": accounts})));
    assert!(!ok(
        FAN_OUT,
        json!({"prompt": "a", "harnesses": [{"harness": "a", "account": ""},
                                                  {"harness": "b"}]})
    ));
    assert!(!ok(
        FAN_OUT,
        json!({"prompt": "a", "harnesses": [{"model": "x"}, {"harness": "b"}]})
    ));
    assert!(!ok(FAN_OUT, json!({"harnesses": two})));
    // A fan-out names the project on that machine as a start does.
    assert!(ok(
        FAN_OUT,
        json!({"prompt": "a", "harnesses": two, "project": "/home/lars/code/rustcode"})
    ));
    assert!(!ok(
        FAN_OUT,
        json!({"prompt": "a", "harnesses": two, "project": "rustcode"})
    ));

    assert_eq!(timeout(START_SESSION, &json!({})), START_TIMEOUT);
    assert_eq!(timeout(FAN_OUT, &json!({})), START_TIMEOUT);
    assert_eq!(timeout(READ_SESSION, &json!({})), REQUEST_TIMEOUT);
}

#[test]
fn a_named_login_is_optional_and_advertised_where_it_is_used() {
    let tool = |name: &str| {
        crate::tools::TOOLS
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name}"))
    };
    // The schema is what tells an agent it may name an account, from v5 on.
    let start = (tool(START_SESSION).schema)();
    assert!(start["properties"]["account"].is_object());
    let fan = (tool(FAN_OUT).schema)();
    assert!(fan["properties"]["harnesses"]["items"]["properties"]["account"].is_object());
    // A v4 client is not told about a field its app cannot answer.
    let start_v4 = schema_for(START_SESSION, 4, start);
    assert!(start_v4["properties"]["account"].is_null());
    let fan_v4 = schema_for(FAN_OUT, 4, fan);
    assert!(fan_v4["properties"]["harnesses"]["items"]["properties"]["account"].is_null());
    // Other fields stay as they were.
    assert!(start_v4["properties"]["prompt"].is_object());
    assert!(fan_v4["properties"]["harnesses"]["items"]["properties"]["model"].is_object());
    // The MCP server builds its tools from the same versioned schema.
    let mcp_v4 = crate::orchestration::mcp_server::tools(4, false);
    let mcp_start = mcp_v4
        .iter()
        .find(|tool| tool["name"] == START_SESSION)
        .expect("start_session for a v4 client");
    assert!(mcp_start["inputSchema"]["properties"]["account"].is_null());
    let mcp_v5 = crate::orchestration::mcp_server::tools(5, false);
    let mcp_start = mcp_v5
        .iter()
        .find(|tool| tool["name"] == START_SESSION)
        .expect("start_session for a v5 client");
    assert!(mcp_start["inputSchema"]["properties"]["account"].is_object());
    assert!(tool(START_SESSION).description.contains("named account"));
    assert!(tool(FAN_OUT).description.contains("named account"));
    // Discovery: list_harnesses names the accounts start_session takes.
    assert!(tool(LIST_HARNESSES).description.contains("named accounts"));
}

/// A machine is a version 12 argument (16 for the rest of the tools): an
/// older app never sees the field, and a call that names one is refused
/// rather than acted on here.
#[test]
fn a_machine_is_optional_and_advertised_where_it_is_used() {
    let tool = |name: &str| {
        crate::tools::TOOLS
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name}"))
    };
    for name in MACHINE_TOOL_NAMES {
        let schema = (tool(name).schema)();
        assert_eq!(
            schema["properties"]["machine"]["type"], "string",
            "{name} takes a machine"
        );
        // A v11 client is not told about a field its app cannot answer.
        let old = schema_for(name, 11, schema.clone());
        assert!(old["properties"]["machine"].is_null(), "{name} at v11");
        // At v12 it is there, and the fields it had stay.
        let new = schema_for(name, 12, schema);
        assert!(new["properties"]["machine"].is_object(), "{name} at v12");
        assert!(PROMPT_HINT_V12.contains(name), "{name} in the hint");
    }
    // Version 16 gives the rest of the thread tools and the project reads
    // the same argument, hidden from a v15 client.
    for name in V16_MACHINE_TOOL_NAMES {
        let schema = (tool(name).schema)();
        assert_eq!(
            schema["properties"]["machine"]["type"], "string",
            "{name} takes a machine"
        );
        let old = schema_for(name, 15, schema.clone());
        assert!(old["properties"]["machine"].is_null(), "{name} at v15");
        let new = schema_for(name, 16, schema);
        assert!(new["properties"]["machine"].is_object(), "{name} at v16");
        assert_eq!(machine_version(name), Some(REMOTE_TOOLS_VERSION));
        assert!(validate(name, &json!({"machine": "  "})).is_err(), "{name}");
    }
    // The tools that do not act on a machine are unchanged: they act on a
    // checkout and a GitHub session that live on the other machine.
    for name in [
        MERGE_WHEN_GREEN,
        CLEANUP_WORKTREE,
        HANDOFF_WORKTREE,
        TRANSFER_SESSION,
        ORGANIZE_THREADS,
        CREATE_PROJECT,
    ] {
        let schema = (tool(name).schema)();
        assert!(
            schema["properties"]["machine"].is_null(),
            "{name} takes no machine"
        );
    }
    // list_machines is what names them, and carries no machine of its own.
    let listed = (tool(LIST_MACHINES).schema)();
    assert!(listed["properties"]["machine"].is_null());
    // The MCP server builds its tools from the same versioned schema.
    let mcp_v11 = crate::orchestration::mcp_server::tools(11, false);
    let start = mcp_v11
        .iter()
        .find(|tool| tool["name"] == START_SESSION)
        .expect("start_session for a v11 client");
    assert!(start["inputSchema"]["properties"]["machine"].is_null());
    assert!(
        !mcp_v11.iter().any(|tool| tool["name"] == LIST_MACHINES),
        "a v11 client never sees list_machines"
    );
    let mcp_v12 = crate::orchestration::mcp_server::tools(12, false);
    let start = mcp_v12
        .iter()
        .find(|tool| tool["name"] == START_SESSION)
        .expect("start_session for a v12 client");
    assert!(start["inputSchema"]["properties"]["machine"].is_object());
    let machines = mcp_v12
        .iter()
        .find(|tool| tool["name"] == LIST_MACHINES)
        .expect("list_machines for a v12 client");
    assert!(
        machines["description"]
            .as_str()
            .unwrap_or("")
            .contains("machine")
    );
    assert!(validate(START_SESSION, &json!({"prompt": "go", "machine": "Studio"})).is_ok());
    assert!(validate(START_SESSION, &json!({"prompt": "go", "machine": "  "})).is_err());
}

/// A machine a v11 app would ignore is refused the same way an account a
/// v4 app would ignore is, whichever side the call is made from.
#[tokio::test]
async fn an_older_client_cannot_act_on_another_machine() {
    let cancel = tokio_util::sync::CancellationToken::new();
    let start = json!({"prompt": "go", "machine": "Studio"});
    let refused = execute_versioned(
        client(json!({"session_id": "c"})),
        11,
        &cancel,
        START_SESSION,
        &start,
    )
    .await;
    assert!(!refused.success);
    assert!(refused.content.contains("newer RustCode app"));
    assert!(refused.content.contains("Nothing was sent"));
    let sent = execute_versioned(
        client(json!({"session_id": "c"})),
        12,
        &cancel,
        START_SESSION,
        &start,
    )
    .await;
    assert!(sent.success, "{}", sent.content);
    // The same refusal on a list, not only on a start.
    let refused = execute_versioned(
        client(json!({"sessions": []})),
        11,
        &cancel,
        LIST_SESSIONS,
        &json!({"machine": "Studio"}),
    )
    .await;
    assert!(!refused.success);
    assert!(refused.content.contains("newer RustCode app"));
    // A v16 machine tool is refused by a v15 app and sent by a v16 one.
    let answer =
        json!({"session_id": "s", "request_id": "1", "choice": "deny", "machine": "Studio"});
    let refused = execute_versioned(
        client(json!({"session_id": "s"})),
        15,
        &cancel,
        ANSWER_REQUEST,
        &answer,
    )
    .await;
    assert!(!refused.success);
    assert!(refused.content.contains("answer_request with machine"));
    let sent = execute_versioned(
        client(json!({"session_id": "s"})),
        16,
        &cancel,
        ANSWER_REQUEST,
        &answer,
    )
    .await;
    assert!(sent.success, "{}", sent.content);
    // A null is no machine, so it is not a version 12 argument.
    let null = json!({"prompt": "go", "machine": null});
    let sent = execute_versioned(
        client(json!({"session_id": "c"})),
        11,
        &cancel,
        START_SESSION,
        &null,
    )
    .await;
    assert!(sent.success, "{}", sent.content);
}

/// An orchestrator that wants room left on an account reads
/// `list_harnesses` (answered by the app, from its plan cache): the tool
/// has to say the field is there and what its numbers mean, or a model
/// never looks at it and delegates onto a window that is nearly used up.
#[test]
fn list_harnesses_advertises_each_logins_usage_windows() {
    let tool = crate::tools::TOOLS
        .iter()
        .find(|tool| tool.name == LIST_HARNESSES)
        .expect("list_harnesses");
    for field in [
        "usage",
        "used_percent",
        "window_minutes",
        "resets_at",
        "as_of",
    ] {
        assert!(tool.description.contains(field), "{field}");
    }
    // An external harness gets the same wording through the MCP server.
    let listed = crate::orchestration::mcp_server::tools(VERSION, false)
        .into_iter()
        .find(|tool| tool["name"] == LIST_HARNESSES)
        .expect("list_harnesses for an MCP harness");
    let description = listed["description"].as_str().unwrap_or_default();
    assert!(description.contains("used_percent"), "{description}");
}

/// A harness may list hundreds of models (OpenCode, Pi), so the tool says
/// how to narrow the list; the schema carries both optional filters.
#[test]
fn list_harnesses_advertises_its_harness_and_model_filters() {
    let tool = crate::tools::TOOLS
        .iter()
        .find(|tool| tool.name == LIST_HARNESSES)
        .expect("list_harnesses");
    assert!(
        tool.description.contains("`harness`"),
        "{}",
        tool.description
    );
    assert!(tool.description.contains("`model`"), "{}", tool.description);
    assert!(
        tool.description.contains("omitted_models"),
        "{}",
        tool.description
    );
    let schema = (tool.schema)();
    assert_eq!(schema["properties"]["harness"]["type"], "string");
    assert_eq!(schema["properties"]["model"]["type"], "string");
    // Nothing is required: a call with no filter is the plain list.
    assert!(schema["required"].as_array().is_some_and(Vec::is_empty));
}

#[tokio::test]
async fn an_older_client_cannot_smuggle_an_account_past_its_version() {
    let cancel = tokio_util::sync::CancellationToken::new();
    let start = json!({"prompt": "go", "account": "work"});
    // Version 4 would otherwise ignore the argument and pick another login.
    let refused = execute_versioned(
        client(json!({"session_id": "c"})),
        4,
        &cancel,
        START_SESSION,
        &start,
    )
    .await;
    assert!(!refused.success);
    assert!(refused.content.contains("newer RustCode app"));
    assert!(refused.content.contains("Nothing was sent"));
    let fan = json!({"prompt": "go", "harnesses": [
        {"harness": "a"},
        {"harness": "b", "account": "work"}
    ]});
    let refused = execute_versioned(client(json!({"links": []})), 4, &cancel, FAN_OUT, &fan).await;
    assert!(!refused.success);
    assert!(refused.content.contains("newer RustCode app"));
    // The same calls go through for a v5 client.
    for (name, args) in [(START_SESSION, start), (FAN_OUT, fan)] {
        let sent =
            execute_versioned(client(json!({"session_id": "c"})), 5, &cancel, name, &args).await;
        assert!(sent.success, "{name}: {}", sent.content);
    }
    // A null is no login, so it is not a version 5 argument.
    let null = json!({"prompt": "go", "account": null});
    let sent = execute_versioned(
        client(json!({"session_id": "c"})),
        4,
        &cancel,
        START_SESSION,
        &null,
    )
    .await;
    assert!(sent.success, "{}", sent.content);
}

#[tokio::test]
async fn a_version_one_client_never_gets_a_version_two_call() {
    let cancel = tokio_util::sync::CancellationToken::new();
    let args = json!({"prompt": "go"});
    let refused = execute_versioned(
        client(json!({"session_id": "c"})),
        1,
        &cancel,
        START_SESSION,
        &args,
    )
    .await;
    assert!(!refused.success);
    assert!(refused.content.contains("Nothing was sent"));
    let started = execute_versioned(
        client(json!({"session_id": "c"})),
        2,
        &cancel,
        START_SESSION,
        &args,
    )
    .await;
    assert!(started.success, "{}", started.content);
}

#[test]
fn the_version_two_hint_stays_short() {
    let tokens = rustcode_core::tokens::estimate_tokens(PROMPT_HINT_V2);
    assert!(tokens < 115, "{tokens}");
    for name in tool_names(2) {
        assert!(PROMPT_HINT_V2.contains(name), "{name}");
    }
}

#[test]
fn arguments_are_checked_before_anything_is_sent() {
    assert!(
        validate(
            MESSAGE_SESSION,
            &json!({"session_id": "b", "message": "hi"})
        )
        .is_ok()
    );
    assert!(validate(MESSAGE_SESSION, &json!({"session_id": "b"})).is_err());
    let long = "x".repeat(MAX_MESSAGE_BYTES + 1);
    assert!(
        validate(
            MESSAGE_SESSION,
            &json!({"session_id": "b", "message": long})
        )
        .is_err()
    );
    // 32 KiB fits; one byte more does not.
    assert!(
        validate(
            MESSAGE_SESSION,
            &json!({"session_id": "b", "message": "x".repeat(MAX_MESSAGE_BYTES)})
        )
        .is_ok()
    );
    assert!(validate(WAIT_SESSION, &json!({})).is_err());
    assert!(validate(WAIT_SESSION, &json!({"link_ids": ["a/1"]})).is_ok());
    assert_eq!(
        timeout(WAIT_SESSION, &json!({"timeout_ms": 9_999_999})),
        Duration::from_millis(MAX_WAIT_MS) + Duration::from_secs(30)
    );
    assert_eq!(MAX_WAIT_MS, 25 * 60 * 1000);
    assert_eq!(
        requested_wait(WAIT_SESSION, &json!({"timeout_ms": 1_950_000})),
        Some(Duration::from_millis(MAX_WAIT_MS))
    );
    assert_eq!(
        requested_wait(WAIT_SESSION, &json!({"timeout_ms": 5_000})),
        Some(Duration::from_secs(5))
    );
    assert_eq!(requested_wait(LIST_SESSIONS, &json!({})), None);
}

#[test]
fn a_retry_key_is_checked_and_hashes_the_same_arguments_the_same() {
    let keyed = |args: Value| validate(MESSAGE_SESSION, &args).is_ok();
    assert!(keyed(
        json!({"session_id": "b", "message": "hi", "client_request_id": "call-1"})
    ));
    assert!(keyed(json!({"session_id": "b", "message": "hi"})));
    assert!(keyed(
        json!({"session_id": "b", "message": "hi", "client_request_id": null})
    ));
    // 1 to 128 characters of A-Z, a-z, 0-9, '.', '_', ':' and '-'.
    assert!(keyed(
        json!({"session_id": "b", "message": "hi", "client_request_id": "a.b_c:d-9"})
    ));
    assert!(keyed(
        json!({"session_id": "b", "message": "hi", "client_request_id": "x".repeat(128)})
    ));
    assert!(!keyed(
        json!({"session_id": "b", "message": "hi", "client_request_id": "x".repeat(129)})
    ));
    assert!(!keyed(
        json!({"session_id": "b", "message": "hi", "client_request_id": ""})
    ));
    assert!(!keyed(
        json!({"session_id": "b", "message": "hi", "client_request_id": "has space"})
    ));
    assert!(!keyed(
        json!({"session_id": "b", "message": "hi", "client_request_id": 7})
    ));
    // The same tools take it, and start_session and fan_out too.
    assert!(
        validate(
            START_SESSION,
            &json!({"prompt": "go", "client_request_id": "k"})
        )
        .is_ok()
    );
    assert!(
        validate(
            FAN_OUT,
            &json!({"prompt": "go", "harnesses": [{"harness": "a"}, {"harness": "b"}],
                        "client_request_id": "k"})
        )
        .is_ok()
    );
    assert!(
        validate(
            FAN_OUT,
            &json!({"prompt": "go", "harnesses": [{"harness": "a"}, {"harness": "b"}],
                        "client_request_id": "not allowed"})
        )
        .is_err()
    );

    // Only the keyed tools forward a key.
    let key = json!({"session_id": "b", "message": "hi", "client_request_id": "k"});
    assert_eq!(request_key(MESSAGE_SESSION, &key), Some("k"));
    assert_eq!(request_key(LIST_SESSIONS, &key), None);
    assert_eq!(
        request_key(MESSAGE_SESSION, &json!({"message": "hi"})),
        None
    );

    // The hash ignores how the arguments were ordered and says nothing about
    // the text: the same prompt and session hash alike, and a changed
    // prompt, target or action does not.
    let one = json!({"session_id": "b", "message": "hi", "client_request_id": "k"});
    let other = json!({"client_request_id": "k", "message": "hi", "session_id": "b"});
    assert_eq!(
        args_hash(MESSAGE_SESSION, &one),
        args_hash(MESSAGE_SESSION, &other)
    );
    assert_ne!(
        args_hash(MESSAGE_SESSION, &one),
        args_hash(
            MESSAGE_SESSION,
            &json!({"session_id": "b", "message": "hi there", "client_request_id": "k"})
        )
    );
    assert_ne!(
        args_hash(MESSAGE_SESSION, &one),
        args_hash(
            START_SESSION,
            &json!({"session_id": "b", "message": "hi", "client_request_id": "k"})
        )
    );
    let hash = args_hash(MESSAGE_SESSION, &one);
    assert_eq!(hash.len(), 64);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(!hash.contains("hi"));
}

#[tokio::test]
async fn a_retry_key_and_its_args_hash_are_forwarded_to_the_client() {
    struct Recording(Arc<std::sync::Mutex<Vec<Value>>>);
    impl PreviewClient for Recording {
        fn request(&self, params: Value) -> crate::preview::PreviewFuture {
            self.0.lock().unwrap().push(params);
            Box::pin(async { Ok(json!({"link_id": "a/1"})) })
        }
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let args = json!({"session_id": "b", "message": "hi", "client_request_id": "call-1"});
    let output = execute_with(
        Some(Arc::new(Recording(Arc::clone(&seen))) as Arc<dyn PreviewClient>),
        &cancel,
        MESSAGE_SESSION,
        &args,
    )
    .await;
    assert!(output.success, "{}", output.content);
    let sent = seen.lock().unwrap()[0].clone();
    assert_eq!(sent["clientRequestId"], "call-1");
    assert_eq!(sent["argsHash"], json!(args_hash(MESSAGE_SESSION, &args)));

    // Without a key nothing is added, so the client sees the call it always saw.
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    execute_with(
        Some(Arc::new(Recording(Arc::clone(&seen))) as Arc<dyn PreviewClient>),
        &cancel,
        MESSAGE_SESSION,
        &json!({"session_id": "b", "message": "hi"}),
    )
    .await;
    let sent = seen.lock().unwrap()[0].clone();
    assert!(sent.get("clientRequestId").is_none());
    assert!(sent.get("argsHash").is_none());
}

#[tokio::test]
async fn calls_go_to_the_host_and_its_errors_come_back_as_failures() {
    let cancel = tokio_util::sync::CancellationToken::new();
    let none = execute_with(None, &cancel, LIST_SESSIONS, &json!({})).await;
    assert!(!none.success);
    assert!(none.content.contains("macOS app"));

    let listed = execute_with(
        client(json!({"sessions": [{"session_id": "b"}]})),
        &cancel,
        LIST_SESSIONS,
        &json!({}),
    )
    .await;
    assert!(listed.success);
    assert!(listed.content.contains("\"b\""));

    let denied = execute_with(
        client(json!({"error": {"code": "denied", "message": "the user said no"}})),
        &cancel,
        MESSAGE_SESSION,
        &json!({"session_id": "b", "message": "hi"}),
    )
    .await;
    assert!(!denied.success);
    assert_eq!(denied.error_kind, Some(ToolErrorKind::PermissionDenied));

    let invalid = execute_with(client(json!({})), &cancel, MESSAGE_SESSION, &json!({})).await;
    assert_eq!(invalid.error_kind, Some(ToolErrorKind::InvalidArguments));

    // A busy refusal keeps the wording and the slots it leaves.
    let busy = execute_with(
            client(json!({"error": {"code": "busy", "message": "at most 8 threads at once; wait_session on one first", "links_left": 0}})),
            &cancel,
            START_SESSION,
            &json!({"prompt": "go"}),
        )
        .await;
    assert!(!busy.success);
    assert!(busy.content.contains("wait_session on one first"));
    assert!(busy.content.contains("links_left: 0"));
}

#[tokio::test]
async fn a_failure_without_a_message_still_says_why() {
    let cancel = tokio_util::sync::CancellationToken::new();
    struct Answering(Value);
    impl PreviewClient for Answering {
        fn request(&self, _params: Value) -> crate::preview::PreviewFuture {
            let answer = self.0.clone();
            Box::pin(async move { Ok(answer) })
        }
    }
    let client =
        |answer: Value| -> Option<Arc<dyn PreviewClient>> { Some(Arc::new(Answering(answer))) };
    for answer in [
        json!({"error": {"code": "denied"}}),
        json!({"error": {"code": "denied", "message": "  "}}),
    ] {
        let refused = execute_with(
            client(answer),
            &cancel,
            START_SESSION,
            &json!({"prompt": "go"}),
        )
        .await;
        assert!(!refused.success);
        assert!(
            refused.content.contains("dismissed the approval"),
            "{}",
            refused.content
        );
        assert!(
            !refused.content.trim_end().ends_with(':'),
            "{}",
            refused.content
        );
    }
    let unknown = execute_with(
        client(json!({"error": {"code": "weird", "message": ""}})),
        &cancel,
        START_SESSION,
        &json!({"prompt": "go"}),
    )
    .await;
    assert!(
        unknown.content.contains("reported no reason"),
        "{}",
        unknown.content
    );
}

#[tokio::test]
async fn a_cancelled_call_says_a_message_may_have_gone() {
    struct Never;
    impl PreviewClient for Never {
        fn request(&self, _params: Value) -> crate::preview::PreviewFuture {
            Box::pin(std::future::pending())
        }
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    let output = execute_with(
        Some(Arc::new(Never) as Arc<dyn PreviewClient>),
        &cancel,
        MESSAGE_SESSION,
        &json!({"session_id": "b", "message": "hi"}),
    )
    .await;
    assert_eq!(output.error_kind, Some(ToolErrorKind::Cancelled));
    assert!(output.content.contains("may have been delivered"));
}

/// A client whose call stays pending until it is cancelled: the harness
/// dropped it, and the app is still preparing an answer nobody will read.
struct Held(Arc<std::sync::Mutex<Vec<String>>>);

impl PreviewClient for Held {
    fn request(&self, params: Value) -> crate::preview::PreviewFuture {
        let call = params["callId"].as_str().unwrap_or_default().to_owned();
        self.0.lock().unwrap().push(format!("asked:{call}"));
        Box::pin(std::future::pending())
    }

    fn call_cancelled(&self, call_id: &str) {
        self.0.lock().unwrap().push(format!("cancelled:{call_id}"));
    }
}

#[tokio::test]
async fn a_dropped_call_is_reported_to_the_client_by_its_own_id() {
    let said = Arc::new(std::sync::Mutex::new(Vec::new()));
    let cancel = tokio_util::sync::CancellationToken::new();
    let token = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        token.cancel();
    });
    let output = execute_with(
        Some(Arc::new(Held(Arc::clone(&said))) as Arc<dyn PreviewClient>),
        &cancel,
        WAIT_SESSION,
        &json!({"link_ids": ["a/1"], "timeout_ms": 600_000}),
    )
    .await;
    assert_eq!(output.error_kind, Some(ToolErrorKind::Cancelled));
    let said = said.lock().unwrap().clone();
    assert_eq!(said.len(), 2, "{said:?}");
    let asked = said[0]
        .strip_prefix("asked:")
        .expect("the call id")
        .to_owned();
    assert!(!asked.is_empty(), "{said:?}");
    // The very call the request carried is the one the client is told to end.
    assert_eq!(said[1], format!("cancelled:{asked}"));
}

#[test]
fn a_dropped_call_is_named_by_its_session_and_id() {
    assert!(CANCELLED_METHOD.starts_with("_rustcode/orchestration"));
    assert_eq!(
        cancelled_params("s-1", "call-7"),
        json!({"sessionId": "s-1", "callId": "call-7"})
    );
    // Every call of this engine gets an id of its own.
    assert_ne!(next_call_id(), next_call_id());
}

#[test]
fn answers_are_compact_json() {
    let out = output(
        LIST_SESSIONS,
        json!({"sessions": [{"id": "a", "title": "x"}]}),
    );
    assert!(out.success);
    assert_eq!(out.content, r#"{"sessions":[{"id":"a","title":"x"}]}"#);
}

#[test]
fn a_wait_defaults_to_a_long_one() {
    assert_eq!(
        requested_wait(WAIT_SESSION, &json!({"link_id": "a/1"})),
        Some(Duration::from_millis(20 * 60 * 1000))
    );
}
