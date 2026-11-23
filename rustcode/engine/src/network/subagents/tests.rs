use super::*;

fn credential(method: crate::provider_auth::AuthMethod) -> crate::provider_auth::CredentialRef {
    crate::provider_auth::CredentialRef {
        provider: "provider".into(),
        account: "account".into(),
        method,
    }
}

/// The #2334 config: one model id at two providers (Copilot listed
/// first), and two ChatGPT-plan profiles for one model at one URL.
fn issue_profiles() -> Vec<crate::config::ModelProfile> {
    use crate::provider_auth::AuthMethod;
    let profile = |name: &str, url: &str, model: &str, method| crate::config::ModelProfile {
        name: name.into(),
        url: url.into(),
        model: model.into(),
        credential: Some(credential(method)),
        ..Default::default()
    };
    let openai = "https://api.openai.com/v1/responses";
    vec![
        profile(
            "copilot/gpt-6-luna",
            "https://api.githubcopilot.com/responses",
            "gpt-6-luna",
            AuthMethod::GitHubCopilot,
        ),
        profile(
            "chatgpt/gpt-5-6-sol",
            openai,
            "gpt-5.6-sol",
            AuthMethod::ChatGpt,
        ),
        profile(
            "chatgpt/gpt-6-luna",
            openai,
            "gpt-6-luna",
            AuthMethod::ChatGpt,
        ),
        profile(
            "chatgpt/gpt-5-6-sol-N2UEH7zP",
            openai,
            "gpt-5.6-sol",
            AuthMethod::ChatGpt,
        ),
        profile(
            "chatgpt/broken",
            "https://chatgpt.com/backend-api",
            "gpt-6-luna",
            AuthMethod::ChatGpt,
        ),
    ]
}

#[test]
fn omitted_model_and_inherit_aliases_run_on_the_parents_profile() {
    let profiles = issue_profiles();
    for requested in [
        None,
        Some(""),
        Some("  "),
        Some("default"),
        Some("inherit"),
        Some("Parent"),
    ] {
        assert_eq!(
            resolve_subagent_profile(&profiles, Some("chatgpt/gpt-6-luna"), requested),
            Ok(Some("chatgpt/gpt-6-luna".to_owned())),
            "{requested:?}"
        );
    }
    // The parent's model id names the parent's profile, not the first
    // profile serving that id (Copilot, which rejects the model).
    assert_eq!(
        resolve_subagent_profile(&profiles, Some("chatgpt/gpt-6-luna"), Some("gpt-6-luna")),
        Ok(Some("chatgpt/gpt-6-luna".to_owned()))
    );
    // An endpoint no profile describes: the child follows the session.
    assert_eq!(resolve_subagent_profile(&profiles, None, None), Ok(None));
}

#[test]
fn explicit_model_resolves_by_profile_name_without_model_id_ambiguity() {
    let profiles = issue_profiles();
    assert_eq!(
        resolve_subagent_profile(
            &profiles,
            Some("chatgpt/gpt-6-luna"),
            Some("chatgpt/gpt-5-6-sol")
        ),
        Ok(Some("chatgpt/gpt-5-6-sol".to_owned()))
    );
    let several =
        resolve_subagent_profile(&profiles, Some("chatgpt/gpt-6-luna"), Some("gpt-5.6-sol"))
            .unwrap_err();
    assert!(
        several.contains("chatgpt/gpt-5-6-sol, chatgpt/gpt-5-6-sol-N2UEH7zP"),
        "{several}"
    );
    assert!(several.contains("Omit `model`"), "{several}");
}

#[test]
fn unusable_model_fails_at_spawn_with_a_few_choices_and_the_omit_hint() {
    let mut profiles = issue_profiles();
    profiles.extend((0..30).map(|index| crate::config::ModelProfile {
        name: format!("local-{index}"),
        url: "http://127.0.0.1:1234/v1/chat/completions".into(),
        model: format!("local-{index}"),
        ..Default::default()
    }));
    let unknown = resolve_subagent_profile(
        &profiles,
        Some("chatgpt/gpt-6-luna"),
        Some("chatgpt/gpt-5-mini"),
    )
    .unwrap_err();
    assert!(
        unknown.starts_with("error: no model profile named 'chatgpt/gpt-5-mini'."),
        "{unknown}"
    );
    assert!(
        unknown.contains("Omit `model` to run on your own model (chatgpt/gpt-6-luna)"),
        "{unknown}"
    );
    let suggested = unknown
        .rsplit_once("such as: ")
        .map(|(_, list)| list.trim_end_matches('.').split(", ").collect::<Vec<_>>())
        .unwrap();
    assert_eq!(suggested.len(), SUGGESTED_PROFILES, "{unknown}");
    assert_eq!(suggested[0], "chatgpt/gpt-6-luna");
    assert!(suggested[1].starts_with("chatgpt/"), "{unknown}");
    assert!(!unknown.contains("local-29"), "{unknown}");

    let broken = resolve_subagent_profile(
        &profiles,
        Some("chatgpt/gpt-6-luna"),
        Some("chatgpt/broken"),
    )
    .unwrap_err();
    assert!(broken.contains("cannot send requests"), "{broken}");
}

#[tokio::test]
async fn spawn_rejects_an_unknown_model_before_queueing_a_child() {
    let mut s = AppState::new();
    s.delegation_active = true;
    s.config.models = issue_profiles();
    s.config.default.set_big("chatgpt/gpt-6-luna".into());
    s.api_base_url = "https://api.openai.com/v1/responses".into();
    s.model_name = "gpt-6-luna".into();
    let state = Arc::new(Mutex::new(s));
    let result = handle_agent_tool(
        &reqwest::Client::new(),
        &state,
        &tokio_util::sync::CancellationToken::new(),
        "spawn_agent",
        &serde_json::json!({"task": "research", "model": "chatgpt/gpt-5-mini"}),
    )
    .await;
    assert!(!result.success);
    assert!(
        result.content.contains("no model profile named"),
        "{}",
        result.content
    );
    assert!(state.lock().await.subagents.is_empty());
}

#[test]
fn child_requests_use_the_profile_it_was_spawned_on() {
    let mut s = AppState::new();
    s.config.models = issue_profiles();
    s.config.default.set_big("chatgpt/gpt-6-luna".into());
    s.api_base_url = "https://api.openai.com/v1/responses".into();
    s.model_name = "gpt-6-luna".into();
    let name = |model| subagent_request_profile(&s, model).map(|profile| profile.name);
    assert_eq!(name(None).as_deref(), Some("chatgpt/gpt-6-luna"));
    assert_eq!(
        name(Some("chatgpt/gpt-5-6-sol-N2UEH7zP")).as_deref(),
        Some("chatgpt/gpt-5-6-sol-N2UEH7zP")
    );
    // The request then selects that profile by name, where URL and model
    // alone would match two credential-bearing profiles.
    let profile = subagent_request_profile(&s, Some("chatgpt/gpt-5-6-sol")).unwrap();
    assert!(
        super::super::stream_request::select_request_profile(
            &s.config.models,
            s.config.default.big(),
            &profile.url,
            &profile.model,
        )
        .is_err()
    );
    assert_eq!(
        super::super::stream_request::select_request_profile(
            &s.config.models,
            &profile.name,
            &profile.url,
            &profile.model,
        )
        .unwrap()
        .map(|profile| profile.name)
        .as_deref(),
        Some("chatgpt/gpt-5-6-sol")
    );
}

#[test]
fn read_only_child_gate_runs_web_and_skills_but_not_writes() {
    let call = |name: &str| crate::tools::ToolCall {
        name: name.to_owned(),
        arguments: serde_json::json!({}),
        call_id: None,
    };
    for name in [
        "search_web",
        "read_url",
        "use_skill",
        "list_skills",
        "view_file",
    ] {
        assert!(is_read_only_subagent_call(&call(name)), "{name}");
    }
    for name in [
        "write_to_file",
        "delete_file",
        "git_commit",
        "remember",
        "manage_task",
    ] {
        assert!(!is_read_only_subagent_call(&call(name)), "{name}");
    }
}

#[test]
fn wait_targets_come_from_ids_or_id_without_duplicates() {
    let ids = |args: serde_json::Value| agent_ids_arg(&args);
    assert_eq!(ids(serde_json::json!({"id": 3})), [3]);
    assert_eq!(ids(serde_json::json!({"id": "4"})), [4]);
    assert_eq!(ids(serde_json::json!({"ids": [2, "5", 2]})), [2, 5]);
    assert_eq!(ids(serde_json::json!({"ids": [2], "id": 7})), [2, 7]);
    assert!(ids(serde_json::json!({"ids": ["x"], "timeout_ms": 5})).is_empty());
}

#[tokio::test]
async fn nested_depth_limit_rejects_before_creating_or_launching_another_child() {
    let mut s = AppState::new();
    s.delegation_active = true;
    let controller = crate::app::SubagentController;
    let first = controller.spawn(&mut s, "first", None, None, false, Vec::new(), None, None);
    let second = controller.spawn(
        &mut s,
        "second",
        None,
        Some(first),
        false,
        Vec::new(),
        None,
        None,
    );
    let third = controller.spawn(
        &mut s,
        "third",
        None,
        Some(second),
        false,
        Vec::new(),
        None,
        None,
    );
    assert_eq!(s.subagents[2].depth, 3);
    assert_eq!(s.subagents[2].root_id, Some(first.raw()));
    let state = Arc::new(Mutex::new(s));
    let result = handle_agent_tool_for_parent(
        &reqwest::Client::new(),
        &state,
        &tokio_util::sync::CancellationToken::new(),
        "spawn_agent",
        &serde_json::json!({"task":"fourth"}),
        Some(third.raw()),
    )
    .await;
    assert!(!result.success);
    assert!(result.content.contains("depth"));
    assert_eq!(state.lock().await.subagents.len(), 3);
}

#[tokio::test(start_paused = true)]
async fn wait_timeout_preserves_active_agent_and_idle_mail_delivers_before_followup() {
    let mut s = AppState::new();
    s.delegation_active = true;
    let id = crate::app::SubagentController.spawn(
        &mut s,
        "original",
        None,
        None,
        false,
        Vec::new(),
        None,
        None,
    );
    let supervisor = s.subagent_supervisor.clone();
    supervisor
        .spawn(
            id,
            tokio_util::sync::CancellationToken::new(),
            std::future::pending(),
        )
        .unwrap();
    let state = Arc::new(Mutex::new(s));
    let started = tokio::time::Instant::now();
    let result = handle_agent_tool(
        &reqwest::Client::new(),
        &state,
        &tokio_util::sync::CancellationToken::new(),
        "wait_agent",
        &serde_json::json!({"id":id.raw(),"timeout_ms":1}),
    )
    .await;
    assert!(result.success && result.content.contains("timed out"));
    // A one-millisecond poll is raised to the minimum wait.
    assert!(
        tokio::time::Instant::now() - started
            >= std::time::Duration::from_millis(MIN_WAIT_AGENT_TIMEOUT_MS)
    );
    assert!(supervisor.is_active(id));
    supervisor.shutdown_and_wait().await;
    let mut s = state.lock().await;
    crate::app::SubagentController
        .set_status(&mut s, id, crate::app::SubAgentStatus::Completed)
        .unwrap();
    s.subagents[0].mailbox.push_back("older evidence".into());
    crate::app::SubagentController
        .send_input(&mut s, id, "followup")
        .unwrap();
    assert_eq!(
        s.subagents[0]
            .history
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        ["original", "older evidence", "followup"]
    );
}

fn spawn_test_child(s: &mut AppState, task: &str) -> crate::app::SubagentId {
    crate::app::SubagentController.spawn(s, task, None, None, false, Vec::new(), None, None)
}

/// Another child's activity wakes the wait but does not end it: the
/// caller gets the result it asked for, not a bare "activity" (#2559).
#[tokio::test]
async fn wait_keeps_waiting_through_another_childs_activity() {
    let mut s = AppState::new();
    s.delegation_active = true;
    let slow = spawn_test_child(&mut s, "slow");
    let fast = spawn_test_child(&mut s, "fast");
    let supervisor = s.subagent_supervisor.clone();
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    supervisor
        .spawn(
            slow,
            tokio_util::sync::CancellationToken::new(),
            async move {
                let _ = released.await;
                Ok("slow result".to_string())
            },
        )
        .unwrap();
    let state = Arc::new(Mutex::new(s));
    let wait = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            handle_agent_tool(
                &reqwest::Client::new(),
                &state,
                &tokio_util::sync::CancellationToken::new(),
                "wait_agent",
                &serde_json::json!({"id": slow.raw(), "timeout_ms": 10_000}),
            )
            .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    // Unrelated activity: another child finishes and announces itself.
    supervisor
        .spawn(fast, tokio_util::sync::CancellationToken::new(), async {
            Ok("fast result".to_string())
        })
        .unwrap();
    supervisor.send_completion_notice(fast.raw(), "completed", "fast result");
    supervisor.notify_activity();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(!wait.is_finished(), "the wait returned without the result");
    release.send(()).unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), wait)
        .await
        .expect("the wait ends with the result")
        .unwrap();
    assert!(result.success, "{}", result.content);
    assert!(result.content.contains("slow result"), "{}", result.content);
}

/// A message for the waiter does end the wait, so it can read it.
#[tokio::test]
async fn wait_returns_early_for_a_new_message_to_the_waiter() {
    let mut s = AppState::new();
    s.delegation_active = true;
    let slow = spawn_test_child(&mut s, "slow");
    let supervisor = s.subagent_supervisor.clone();
    supervisor
        .spawn(
            slow,
            tokio_util::sync::CancellationToken::new(),
            std::future::pending(),
        )
        .unwrap();
    let state = Arc::new(Mutex::new(s));
    let wait = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            handle_agent_tool(
                &reqwest::Client::new(),
                &state,
                &tokio_util::sync::CancellationToken::new(),
                "wait_agent",
                &serde_json::json!({"id": slow.raw(), "timeout_ms": 10_000}),
            )
            .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    supervisor
        .send_root_message(slow.raw(), "halfway: found parse()".into())
        .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), wait)
        .await
        .expect("a message ends the wait")
        .unwrap();
    assert!(
        result.success && result.content.contains("still working"),
        "{}",
        result.content
    );
    supervisor.shutdown_and_wait().await;
}

/// `cancel_agent` reports the terminal state once the child stops,
/// instead of only saying cancellation was requested (#2558).
#[tokio::test]
async fn cancel_reports_the_terminal_state_once_the_child_stops() {
    let mut s = AppState::new();
    s.delegation_active = true;
    let child = spawn_test_child(&mut s, "stuck");
    let supervisor = s.subagent_supervisor.clone();
    supervisor
        .spawn(
            child,
            tokio_util::sync::CancellationToken::new(),
            std::future::pending(),
        )
        .unwrap();
    let state = Arc::new(Mutex::new(s));
    let outcome = cancel_subagent_tree(&state, child.raw()).await.unwrap();
    assert_eq!(
        outcome.message,
        format!("subagent {} cancelled", child.raw())
    );
    assert!(outcome.settled);
    assert!(!supervisor.is_active(child));
    assert!(!supervisor.cancel_requested(child));
}

#[test]
fn structured_completion_is_bounded_after_json_escaping() {
    let output = "\n\t".repeat(20_000);
    let (result, truncated) =
        bounded_completion_output(1, "completed", &output, false, &"handoff".repeat(1000));
    assert!(truncated);
    assert!(result.len() <= 8192);
    let value: serde_json::Value =
        serde_json::from_str(result.split_once('\n').unwrap().1).unwrap();
    assert_eq!(value["status"], "completed");
    assert_eq!(value["truncated"], true);
}

#[test]
fn subagent_context_defaults_to_session_source_and_prefers_agent_workspace() {
    let source = tempfile::tempdir().unwrap();
    let agent_workspace = tempfile::tempdir().unwrap();
    let state = crate::app::AppState::new_with_workspace_session(
        source.path(),
        Some("subagent-source-context"),
    );

    assert_eq!(
        subagent_context_workspace_root(&state, None).as_deref(),
        Some(source.path())
    );
    assert_eq!(
        subagent_context_workspace_root(&state, Some(agent_workspace.path().to_path_buf()))
            .as_deref(),
        Some(agent_workspace.path())
    );
}

async fn gated_subagent_server() -> (
    String,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = socket.read(&mut buffer).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }
        let _ = accepted_tx.send(());
        let _ = release_rx.await;
        let chunk = serde_json::json!({
            "choices": [{"delta": {"content": "child finished"}, "finish_reason": "stop"}]
        });
        let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = socket.write_all(response.as_bytes()).await;
    });
    (format!("http://{address}"), accepted_rx, release_tx)
}

/// A provider that answers each request with the next of `answers`
/// (SSE bodies), then holds any further request open.
async fn scripted_subagent_server(answers: Vec<String>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for body in answers {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
        let _held = listener.accept().await;
        std::future::pending::<()>().await;
    });
    format!("http://{address}")
}

/// An SSE body with `content` and one native call of `name`.
fn native_tool_call_answer(content: &str, name: &str, arguments: serde_json::Value) -> String {
    let call = serde_json::json!({
        "choices": [{"delta": {
            "content": content,
            "tool_calls": [{
                "index": 0,
                "id": "call-1",
                "type": "function",
                "function": {"name": name, "arguments": arguments.to_string()},
            }],
        }}]
    });
    let finish = serde_json::json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]});
    format!("data: {call}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

async fn native_test_state(url: String) -> Arc<Mutex<AppState>> {
    let state = delegated_test_state(url.clone()).await;
    state
        .lock()
        .await
        .record_function_calling_support(&url, true);
    state
}

/// Cancelling a child while its tool call runs ends it: the child used to
/// lock the state twice on that path and hang holding it, so the cancel
/// never settled and the whole session froze.
#[tokio::test]
async fn cancel_during_a_tool_call_settles_and_releases_the_state() {
    let url = scripted_subagent_server(vec![native_tool_call_answer(
        "",
        "wait_agent",
        serde_json::json!({"id": 1, "timeout_ms": 30_000}),
    )])
    .await;
    let state = native_test_state(url).await;
    let (sibling, supervisor) = {
        let mut s = state.lock().await;
        (
            spawn_test_child(&mut s, "stuck"),
            s.subagent_supervisor.clone(),
        )
    };
    supervisor
        .spawn(
            sibling,
            tokio_util::sync::CancellationToken::new(),
            std::future::pending(),
        )
        .unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let spawned = handle_agent_tool(
        &reqwest::Client::new(),
        &state,
        &cancel,
        "spawn_agent",
        &serde_json::json!({"task": "wait for the sibling"}),
    )
    .await;
    assert!(spawned.success, "{}", spawned.content);
    let child = 2;
    // The call is recorded before it runs; the child is then in the wait.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if state.lock().await.subagents.iter().any(|agent| {
                agent.id == child
                    && agent
                        .history
                        .iter()
                        .any(|message| !message.tool_calls.is_empty())
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the child reaches its tool call");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        cancel_subagent_tree(&state, child),
    )
    .await
    .expect("cancel returns")
    .unwrap();
    assert!(outcome.settled, "{}", outcome.message);
    assert_eq!(outcome.message, format!("subagent {child} cancelled"));
    let s = tokio::time::timeout(std::time::Duration::from_secs(5), state.lock())
        .await
        .expect("the child released the state lock");
    let agent = s.subagents.iter().find(|agent| agent.id == child).unwrap();
    assert_eq!(agent.status, crate::app::SubAgentStatus::Cancelled);
    assert!(
        agent
            .history
            .iter()
            .any(|message| message.tool_call_id.as_deref() == Some("call-1")),
        "the interrupted call keeps its result"
    );
    drop(s);
    supervisor.shutdown_and_wait().await;
}

/// A child that reaches its round limit ends as failed and still hands
/// back what it had found.
#[tokio::test]
async fn round_limit_fails_the_child_with_its_last_partial_answer() {
    let url = scripted_subagent_server(vec![native_tool_call_answer(
        "The parser drops the last token.",
        "list_agents",
        serde_json::json!({}),
    )])
    .await;
    let state = native_test_state(url).await;
    state.lock().await.config.loop_guard.subagent_max_rounds = 1;
    let cancel = tokio_util::sync::CancellationToken::new();
    let spawned = handle_agent_tool(
        &reqwest::Client::new(),
        &state,
        &cancel,
        "spawn_agent",
        &serde_json::json!({"task": "find the bug"}),
    )
    .await;
    assert!(spawned.success, "{}", spawned.content);
    let waited = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        handle_agent_tool(
            &reqwest::Client::new(),
            &state,
            &cancel,
            "wait_agent",
            &serde_json::json!({"id": 1}),
        ),
    )
    .await
    .expect("the child ends at its limit");
    assert!(
        waited.content.contains("limit of 1 model rounds"),
        "{}",
        waited.content
    );
    assert!(
        waited.content.contains("The parser drops the last token."),
        "{}",
        waited.content
    );
    assert_eq!(
        state.lock().await.subagents[0].status,
        crate::app::SubAgentStatus::Failed
    );
}

#[test]
fn contract_paths_cover_every_path_argument_and_resolve_dot_dot() {
    let root = std::path::Path::new("/work/agent-1");
    let allowed = vec!["src/".to_string()];
    let outside =
        |args: serde_json::Value| call_paths_outside_contract(&args, Some(root), &allowed);
    assert!(!outside(serde_json::json!({"path": "src/lib.rs"})));
    assert!(!outside(serde_json::json!({"path": "./src/a/../b.rs"})));
    assert!(!outside(
        serde_json::json!({"path": "/work/agent-1/src/lib.rs"})
    ));
    assert!(outside(serde_json::json!({"path": "src/../Cargo.toml"})));
    assert!(outside(serde_json::json!({"path": "../../etc/passwd"})));
    assert!(outside(serde_json::json!({"path": "srcx/lib.rs"})));
    assert!(outside(
        serde_json::json!({"path": "/work/agent-1/Cargo.toml"})
    ));
    assert!(outside(
        serde_json::json!({"src": "src/a.rs", "dest": "build.rs"})
    ));
    assert!(!outside(
        serde_json::json!({"src": "src/a.rs", "dest": "src/b.rs"})
    ));
    assert!(outside(
        serde_json::json!({"paths": ["src/a.rs", "README.md"]})
    ));
    assert!(!outside(serde_json::json!({"command": "cargo test"})));
}

#[test]
fn bounded_completion_keeps_the_conclusion_at_the_end() {
    let output = format!("{}FINAL: the fix is in parse()", "detail ".repeat(4000));
    let (result, truncated) = bounded_completion_output(1, "completed", &output, false, "");
    assert!(truncated);
    assert!(result.len() <= 8192);
    assert!(result.contains("FINAL: the fix is in parse()"), "{result}");
    assert!(result.contains("bytes omitted"));
}

#[test]
fn head_tail_truncation_stays_within_the_limit_on_char_boundaries() {
    let text = "é".repeat(5000);
    for max in [0, 7, 95, 96, 97, 200, 1001] {
        let truncated = crate::tools::truncate_bytes_head_tail(&text, max);
        assert!(truncated.len() <= max, "{max}: {}", truncated.len());
    }
    assert_eq!(crate::tools::truncate_bytes_head_tail("short", 96), "short");
}

async fn delegated_test_state(url: String) -> Arc<Mutex<AppState>> {
    let mut state = AppState::new();
    state.api_base_url = url;
    state.model_name = "subagent-test-model".to_owned();
    state.delegation_active = true;
    Arc::new(Mutex::new(state))
}

#[test]
fn subagent_id_parser_accepts_decimal_ids_and_rejects_agent_names() {
    for (value, expected) in [
        (serde_json::json!(1), Some(1)),
        (serde_json::json!("1"), Some(1)),
        (serde_json::json!(0), Some(0)),
        (serde_json::json!("agent-1"), None),
    ] {
        assert_eq!(agent_id_arg(&serde_json::json!({"id": value})), expected);
    }
}

#[test]
fn read_only_subagent_gate_allows_inspection_shells_and_rejects_mutation_controls() {
    let call = |arguments| crate::tools::ToolCall {
        name: "run_command".to_owned(),
        arguments,
        call_id: None,
    };

    for command in [
        "gh pr list --limit 15 --state open",
        // Brace expansion on a reading command only adds paths to read.
        "rg -n 'x' rustcode/engine/src/tools/{mod.rs,schema.rs}",
        "cat src/{a,b}.rs | head -5",
        // Help and version text from a tool the policy does not know.
        "agent-desktop --help && agent-desktop snapshot --help",
        "cargo --version",
    ] {
        assert!(
            is_read_only_subagent_call(&call(serde_json::json!({ "command": command }))),
            "read-only agent must allow {command}"
        );
    }
    for arguments in [
        serde_json::json!({"command":"cargo test"}),
        serde_json::json!({"command":"echo x > /tmp/out"}),
        serde_json::json!({"command":"gh pr list", "env":{"BASH_ENV":"/tmp/hook"}}),
        serde_json::json!({"command":"gh pr list", "background":true}),
        serde_json::json!({"command":"gh pr list", "background":"true"}),
        serde_json::json!({"command":"gh pr list", "detached":true}),
        serde_json::json!({"command":"gh pr list", "detached":"true"}),
        serde_json::json!({"command":"gh pr list", "network_access":true}),
        serde_json::json!({"command":"gh pr list", "network_access":"false"}),
        serde_json::json!({"command":"gh pr list", "filesystem_write_path":"/tmp"}),
        serde_json::json!({"command":"tail -f file &"}),
        serde_json::json!({"command":"rm -rf src/{a,b}"}),
        serde_json::json!({"command":"git checkout src/{a,b}"}),
        serde_json::json!({"command":"cat {-n,x}"}),
        serde_json::json!({"command":"touch name --help"}),
        serde_json::json!({"command":"shutdown -h"}),
        serde_json::json!({"command":"tool --help > out.txt"}),
        serde_json::json!({"command":"tool --help && rm -rf x"}),
    ] {
        assert!(
            !is_read_only_subagent_call(&call(arguments.clone())),
            "read-only agent must reject {arguments}"
        );
    }
}

#[test]
fn subagent_batches_are_truncated_and_validated_before_execution() {
    let call = |name: &str, arguments: serde_json::Value| crate::tools::ToolCall {
        name: name.to_owned(),
        arguments,
        call_id: None,
    };
    let mut calls = vec![
        (call("grep", serde_json::json!({"pattern": "TODO"})), None),
        (
            call("run_command", serde_json::json!({"command": "cargo test"})),
            None,
        ),
        (
            call(
                "run_command",
                serde_json::json!({"command": "cargo test -- --nocapture"}),
            ),
            None,
        ),
    ];

    let (requested, dropped, validation) = prepare_subagent_tool_batch(&mut calls, 1);
    assert_eq!((requested, calls.len(), dropped), (3, 2, 1));
    assert!(validation.is_ok());

    calls.push((
        call(
            "run_command",
            serde_json::json!({"command": "cargo test -- --nocapture"}),
        ),
        None,
    ));
    let (requested, dropped, validation) = prepare_subagent_tool_batch(&mut calls, 2);
    assert_eq!((requested, calls.len(), dropped), (3, 3, 0));
    assert!(validation.is_ok());
}

#[test]
fn subagent_batch_retains_reads_after_a_dropped_mutation() {
    let mut calls = vec![
        (
            crate::tools::ToolCall {
                name: "run_command".to_owned(),
                arguments: serde_json::json!({"command": "cargo test"}),
                call_id: None,
            },
            None,
        ),
        (
            crate::tools::ToolCall {
                name: "write_to_file".to_owned(),
                arguments: serde_json::json!({"path": "x", "content": "x"}),
                call_id: None,
            },
            None,
        ),
        (
            crate::tools::ToolCall {
                name: "grep".to_owned(),
                arguments: serde_json::json!({"pattern": "x"}),
                call_id: None,
            },
            None,
        ),
    ];

    let (requested, dropped, validation) = prepare_subagent_tool_batch(&mut calls, 1);

    assert_eq!((requested, calls.len(), dropped), (3, 2, 1));
    assert_eq!(calls[1].0.name, "grep");
    assert!(validation.is_ok());
}

#[test]
fn native_subagent_history_keeps_call_and_result_ids_structured() {
    let assistant = ChatMessage::new("assistant", "").with_tool_calls(vec![
        crate::app::ToolCallRef {
            id: "call-1".to_string(),
            name: "view_file".to_string(),
            arguments: r#"{"path":"src/lib.rs"}"#.to_string(),
        },
        crate::app::ToolCallRef {
            id: "call-2".to_string(),
            name: "grep".to_string(),
            arguments: r#"{"pattern":"TODO"}"#.to_string(),
        },
    ]);
    let result = ChatMessage::new("tool", "view_file: content")
        .answering(Some("call-1".to_string()))
        .with_tool_result(crate::app::ToolResultRecord {
            workspace_generation: None,
            workspace_epoch: None,
            evidence_hash: None,
            tool_name: "view_file".to_string(),
            truncated: true,
            completeness: rustcode_core::ToolResultCompleteness::ByteTruncated,
            ..Default::default()
        });

    let assistant_message =
        subagent_history_message(&assistant, crate::config::ToolProtocol::ApiNative);
    let result_message = subagent_history_message(&result, crate::config::ToolProtocol::ApiNative);

    assert_eq!(assistant_message["role"], "assistant");
    assert_eq!(assistant_message["tool_calls"].as_array().unwrap().len(), 2);
    assert_eq!(assistant_message["tool_calls"][1]["id"], "call-2");
    assert_eq!(result_message["role"], "tool");
    assert_eq!(result_message["tool_call_id"], "call-1");
    assert!(
        result_message["content"]
            .as_str()
            .unwrap()
            .contains("content")
    );
    assert!(
        result_message["content"]
            .as_str()
            .unwrap()
            .contains("result_metadata")
    );
    assert!(
        result_message["content"]
            .as_str()
            .unwrap()
            .contains("byte_truncated")
    );
}

#[test]
fn api_native_subagent_ignores_text_tool_syntax_without_native_calls() {
    let calls = parse_subagent_tool_calls(
        "```tool\n{\"name\":\"grep\",\"arguments\":{\"pattern\":\"decoy\"}}\n```",
        crate::config::ToolProtocol::ApiNative,
        Vec::new(),
    );

    assert!(calls.is_empty());
}

#[test]
fn text_protocol_subagent_still_parses_tool_syntax() {
    let calls = parse_subagent_tool_calls(
        "```tool\n{\"name\":\"grep\",\"arguments\":{\"pattern\":\"needle\"}}\n```",
        crate::config::ToolProtocol::Json,
        Vec::new(),
    );

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0.name, "grep");
    assert_eq!(calls[0].0.arguments["pattern"], "needle");
    assert!(calls[0].1.is_none());
}

#[tokio::test]
async fn spawn_tool_returns_while_child_runs_and_wait_delivers_once() {
    let (url, accepted, release) = gated_subagent_server().await;
    let state = delegated_test_state(url).await;
    let cancel = tokio_util::sync::CancellationToken::new();

    let spawned = handle_agent_tool(
        &reqwest::Client::new(),
        &state,
        &cancel,
        "spawn_agent",
        &serde_json::json!({"task": "inspect the code"}),
    )
    .await;
    assert!(spawned.success);
    // The receipt is JSON: the id every other agent tool takes, the name
    // the child goes by, and what it was started as.
    let receipt: serde_json::Value = serde_json::from_str(&spawned.content).unwrap();
    assert_eq!(
        receipt,
        serde_json::json!({
            "agent_id": 1,
            "nickname": "agent-1",
            "agent_type": "default",
            "model": "subagent-test-model",
            "status": "queued",
        })
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), accepted)
        .await
        .unwrap()
        .unwrap();
    assert!(state.lock().await.subagents[0].active_turn);

    release.send(()).unwrap();
    let waited = handle_agent_tool(
        &reqwest::Client::new(),
        &state,
        &cancel,
        "wait_agent",
        &serde_json::json!({"id": 1}),
    )
    .await;
    let replayed = handle_agent_tool(
        &reqwest::Client::new(),
        &state,
        &cancel,
        "wait_agent",
        &serde_json::json!({"id": 1}),
    )
    .await;
    assert!(waited.success, "{}", waited.content);
    assert_eq!(waited.content, replayed.content);
    assert!(waited.content.contains("child finished"));
}

#[tokio::test]
async fn wait_agent_with_ids_returns_the_child_that_finishes_first() {
    let mut s = AppState::new();
    s.delegation_active = true;
    let controller = crate::app::SubagentController;
    let slow = controller.spawn(&mut s, "slow", None, None, false, Vec::new(), None, None);
    let fast = controller.spawn(&mut s, "fast", None, None, false, Vec::new(), None, None);
    let supervisor = s.subagent_supervisor.clone();
    let parent = tokio_util::sync::CancellationToken::new();
    supervisor
        .spawn(slow, parent.clone(), std::future::pending())
        .unwrap();
    supervisor
        .spawn(fast, parent.clone(), async { Ok("fast result".to_owned()) })
        .unwrap();
    let state = Arc::new(Mutex::new(s));

    let waited = handle_agent_tool(
        &reqwest::Client::new(),
        &state,
        &parent,
        "wait_agent",
        // Longer than the old five-minute ceiling; it returns on the first result.
        &serde_json::json!({"ids": [slow.raw(), fast.raw()], "timeout_ms": 900_000}),
    )
    .await;
    assert!(waited.success, "{}", waited.content);
    assert!(
        waited
            .content
            .starts_with(&format!("subagent {} completed", fast.raw()))
    );
    assert!(waited.content.contains("fast result"));
    assert!(supervisor.is_active(slow));

    // `id` alone still names a single target.
    let single = handle_agent_tool(
        &reqwest::Client::new(),
        &state,
        &parent,
        "wait_agent",
        &serde_json::json!({"id": fast.raw()}),
    )
    .await;
    assert_eq!(single.content, waited.content);
    supervisor.shutdown_and_wait().await;
}

#[tokio::test]
async fn spawn_roles_preset_write_access_without_bypassing_its_checks() {
    use crate::app::subagent_context::AgentRole;

    assert_eq!(AgentRole::Default.write_access(None), Ok(false));
    assert_eq!(AgentRole::Default.write_access(Some(true)), Ok(true));
    assert_eq!(AgentRole::Explorer.write_access(Some(false)), Ok(false));
    assert_eq!(AgentRole::Worker.write_access(None), Ok(true));
    assert!(AgentRole::Explorer.write_access(Some(true)).is_err());
    assert!(AgentRole::Worker.write_access(Some(false)).is_err());

    let (url, accepted, release) = gated_subagent_server().await;
    let state = delegated_test_state(url).await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let spawn = |args: serde_json::Value, parent: Option<u32>| {
        let state = Arc::clone(&state);
        let cancel = cancel.clone();
        async move {
            handle_agent_tool_for_parent(
                &reqwest::Client::new(),
                &state,
                &cancel,
                "spawn_agent",
                &args,
                parent,
            )
            .await
        }
    };

    let unknown = spawn(
        serde_json::json!({"task": "t", "agent_type": "reviewer"}),
        None,
    )
    .await;
    assert!(!unknown.success);
    assert!(unknown.content.contains("unknown agent_type 'reviewer'"));
    assert!(unknown.content.contains("default, explorer, worker"));

    let contradiction = spawn(
            serde_json::json!({"task": "t", "agent_type": "explorer", "write_access": true, "allowed_paths": ["src/"]}),
            None,
        )
        .await;
    assert!(!contradiction.success);
    assert!(contradiction.content.contains("explorer is read-only"));

    // A worker is `write_access` by another name: the same contract applies.
    let unscoped = spawn(
        serde_json::json!({"task": "t", "agent_type": "worker"}),
        None,
    )
    .await;
    assert!(!unscoped.success);
    assert!(unscoped.content.contains("allowed_paths"));
    assert!(state.lock().await.subagents.is_empty());

    let explorer = spawn(
        serde_json::json!({"task": "look", "agent_type": "explorer"}),
        None,
    )
    .await;
    assert!(explorer.success, "{}", explorer.content);
    let receipt: serde_json::Value = serde_json::from_str(&explorer.content).unwrap();
    assert_eq!(receipt["agent_type"], "explorer");
    tokio::time::timeout(std::time::Duration::from_secs(5), accepted)
        .await
        .unwrap()
        .unwrap();
    {
        let s = state.lock().await;
        assert_eq!(s.subagents[0].agent_type, AgentRole::Explorer);
        assert!(!s.subagents[0].write_access);
    }

    // Nested delegation stays read-only whichever way write access is asked for.
    let nested = spawn(
        serde_json::json!({"task": "t", "agent_type": "worker", "allowed_paths": ["src/"]}),
        Some(1),
    )
    .await;
    assert!(!nested.success);
    assert!(nested.content.contains("nested delegation is read-only"));
    assert_eq!(state.lock().await.subagents.len(), 1);

    let supervisor = state.lock().await.subagent_supervisor.clone();
    supervisor.shutdown_and_wait().await;
    let _ = release.send(());
}

#[tokio::test]
async fn spawn_cwd_becomes_the_shared_childs_working_directory() {
    let (url, accepted, release) = gated_subagent_server().await;
    let state = delegated_test_state(url).await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let directory = tempfile::tempdir().expect("cwd");
    let spawn = |args: serde_json::Value| {
        let state = Arc::clone(&state);
        let cancel = cancel.clone();
        async move {
            handle_agent_tool_for_parent(
                &reqwest::Client::new(),
                &state,
                &cancel,
                "spawn_agent",
                &args,
                None,
            )
            .await
        }
    };

    let missing = spawn(serde_json::json!({
        "task": "look", "cwd": directory.path().join("missing")
    }))
    .await;
    assert!(!missing.success);
    assert!(
        missing.content.contains("invalid cwd"),
        "{}",
        missing.content
    );
    let child = spawn(serde_json::json!({"task": "look", "cwd": directory.path()})).await;
    assert!(child.success, "{}", child.content);
    tokio::time::timeout(std::time::Duration::from_secs(5), accepted)
        .await
        .unwrap()
        .unwrap();
    {
        let s = state.lock().await;
        let expected = directory.path().canonicalize().unwrap();
        assert_eq!(s.subagents[0].working_directory.as_ref(), Some(&expected));
        assert_eq!(
            subagent_execution_root(&s.subagents[0]),
            Some(expected.clone())
        );
        assert_eq!(
            subagent_context_workspace_root(&s, subagent_execution_root(&s.subagents[0])),
            Some(expected)
        );
    }

    let supervisor = state.lock().await.subagent_supervisor.clone();
    supervisor.shutdown_and_wait().await;
    let _ = release.send(());
}

/// A worker (the default way a model asks for writes) used to fail with
/// `git worktree add exited with exit status: 128`: every writer got a
/// worktree of the engine's own directory. A shared writer now works in
/// the thread's workspace, fenced by allowed_paths, and an isolated one
/// outside git falls back to that instead of failing.
#[tokio::test]
async fn write_enabled_children_start_in_the_thread_workspace() {
    let (url, accepted, release) = gated_subagent_server().await;
    let state = delegated_test_state(url).await;
    let workspace = tempfile::tempdir().expect("workspace");
    state.lock().await.workspace_root = Some(workspace.path().to_path_buf());
    let cancel = tokio_util::sync::CancellationToken::new();
    let spawn = |args: serde_json::Value| {
        let state = Arc::clone(&state);
        let cancel = cancel.clone();
        async move {
            handle_agent_tool_for_parent(
                &reqwest::Client::new(),
                &state,
                &cancel,
                "spawn_agent",
                &args,
                None,
            )
            .await
        }
    };

    let worker = spawn(serde_json::json!({
        "task": "edit", "agent_type": "worker", "allowed_paths": ["src/"]
    }))
    .await;
    assert!(worker.success, "{}", worker.content);
    tokio::time::timeout(std::time::Duration::from_secs(5), accepted)
        .await
        .unwrap()
        .unwrap();
    {
        let s = state.lock().await;
        let agent = &s.subagents[0];
        assert!(agent.write_access);
        assert_eq!(agent.workspace_root, None);
        assert_eq!(
            subagent_execution_root(agent).as_deref(),
            Some(workspace.path())
        );
        // The contract fences writes to allowed_paths inside it.
        let root = subagent_execution_root(agent);
        let allowed = &agent.allowed_paths;
        assert!(!call_paths_outside_contract(
            &serde_json::json!({"path": "src/lib.rs"}),
            root.as_deref(),
            allowed
        ));
        assert!(call_paths_outside_contract(
            &serde_json::json!({"path": "Cargo.toml"}),
            root.as_deref(),
            allowed
        ));
    }

    // Not a git repository: isolation falls back to the shared folder.
    let isolated = spawn(serde_json::json!({
        "task": "edit", "agent_type": "worker", "allowed_paths": ["docs/"],
        "workspace_mode": "isolated"
    }))
    .await;
    assert!(isolated.success, "{}", isolated.content);
    let receipt: serde_json::Value = serde_json::from_str(&isolated.content).unwrap();
    assert!(
        receipt["workspace"]
            .as_str()
            .is_some_and(|note| note.contains("not a git repository")),
        "{receipt}"
    );
    {
        let s = state.lock().await;
        let agent = &s.subagents[1];
        assert_eq!(agent.workspace_root, None);
        assert_eq!(
            subagent_execution_root(agent).as_deref(),
            Some(workspace.path())
        );
    }

    let supervisor = state.lock().await.subagent_supervisor.clone();
    supervisor.shutdown_and_wait().await;
    let _ = release.send(());
}

#[tokio::test]
async fn followup_with_interrupt_stops_the_running_turn_and_redirects_the_child() {
    let (url, accepted, release) = gated_subagent_server().await;
    let state = delegated_test_state(url).await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let call = |name: &'static str, args: serde_json::Value, parent: Option<u32>| {
        let state = Arc::clone(&state);
        let cancel = cancel.clone();
        async move {
            handle_agent_tool_for_parent(
                &reqwest::Client::new(),
                &state,
                &cancel,
                name,
                &args,
                parent,
            )
            .await
        }
    };
    call(
        "spawn_agent",
        serde_json::json!({"task": "old direction"}),
        None,
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(5), accepted)
        .await
        .unwrap()
        .unwrap();
    let supervisor = state.lock().await.subagent_supervisor.clone();

    // Queue-only messages cannot interrupt, and an agent cannot stop itself.
    let queue_only = call(
        "send_message",
        serde_json::json!({"id": 1, "message": "note", "interrupt": true}),
        None,
    )
    .await;
    assert!(!queue_only.success);
    let own_turn = call(
        "followup_task",
        serde_json::json!({"id": 1, "message": "note", "interrupt": true}),
        Some(1),
    )
    .await;
    assert!(!own_turn.success);
    assert!(own_turn.content.contains("itself or an ancestor"));
    assert_eq!(
        state.lock().await.subagents[0].status,
        crate::app::SubAgentStatus::Running
    );

    let redirected = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        call(
            "followup_task",
            serde_json::json!({"id": 1, "message": "new direction", "interrupt": true}),
            None,
        ),
    )
    .await
    .unwrap();
    assert!(redirected.success, "{}", redirected.content);
    assert!(redirected.content.contains("interrupted and redirected"));
    {
        let s = state.lock().await;
        let agent = &s.subagents[0];
        // A new turn is under way with the redirect as its instruction,
        // not queued behind the one that was stopped.
        assert!(agent.active_turn);
        assert!(agent.mailbox.is_empty());
        assert!(agent.completion.is_none());
        assert!(matches!(
            agent.status,
            crate::app::SubAgentStatus::Queued | crate::app::SubAgentStatus::Running
        ));
        assert_eq!(
            agent
                .history
                .iter()
                .map(|message| (message.role.as_str(), message.content.as_str()))
                .collect::<Vec<_>>(),
            [("user", "old direction"), ("user", "new direction")]
        );
    }
    assert!(supervisor.is_active(crate::app::SubagentId::from_raw(1)));
    // The stop was asked for; the root is not told its child was cancelled.
    assert!(supervisor.root_messages().is_empty());

    supervisor.shutdown_and_wait().await;
    let _ = release.send(());
}

#[tokio::test]
async fn running_child_accepts_message_and_cancel_tool_stops_it() {
    let (url, accepted, release) = gated_subagent_server().await;
    let state = delegated_test_state(url).await;
    let cancel = tokio_util::sync::CancellationToken::new();
    handle_agent_tool(
        &reqwest::Client::new(),
        &state,
        &cancel,
        "spawn_agent",
        &serde_json::json!({"task": "keep working"}),
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(5), accepted)
        .await
        .unwrap()
        .unwrap();

    let sent = handle_agent_tool(
        &reqwest::Client::new(),
        &state,
        &cancel,
        "send_agent",
        &serde_json::json!({"id": 1, "message": "duplicate turn"}),
    )
    .await;
    assert!(sent.success);
    assert!(sent.content.contains("queued"));
    assert_eq!(
        state.lock().await.subagents[0]
            .mailbox
            .front()
            .map(String::as_str),
        Some("duplicate turn")
    );

    let cancelled = handle_agent_tool(
        &reqwest::Client::new(),
        &state,
        &cancel,
        "cancel_agent",
        &serde_json::json!({"id": 1}),
    )
    .await;
    assert!(cancelled.success);
    let waited = handle_agent_tool(
        &reqwest::Client::new(),
        &state,
        &cancel,
        "wait_agent",
        &serde_json::json!({"id": 1}),
    )
    .await;
    assert!(!waited.success);
    assert!(waited.content.contains("cancelled"));
    let _ = release.send(());
    assert_eq!(
        state.lock().await.subagents[0].status,
        crate::app::SubAgentStatus::Cancelled
    );
}
