//! Slash commands over ACP. The catalog and the allowlisted host commands are
//! the ones the remote (iOS) client uses; ACP advertises them with
//! `available_commands_update` and runs them when a prompt starts with one.

use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate, SessionUpdate,
    UnstructuredCommandInput,
};

use crate::remote::protocol::{RemoteCommandInfo, RemoteResult};

/// Commands ACP runs itself on top of the remote host commands, with whether
/// each takes arguments. They need the session's model client, its turn or
/// this Mac's browser, so the phone does not get them.
const ACP_COMMANDS: &[(&str, bool)] = &[
    ("/compact", false),
    ("/recap", false),
    ("/summarize", false),
    ("/goal", true),
    ("/fork", false),
    ("/login", true),
    ("/auth", true),
    ("/logout", true),
    ("/accounts", false),
    ("/account", true),
    ("/refresh", true),
];

/// Bare host commands that only read the session. They answer beside a
/// running turn instead of waiting behind it at the session's turn gate, and
/// are advertised with `_meta.rustcode.report` so a client sends them at once.
const REPORT_COMMANDS: &[&str] = &[
    "/about",
    "/agents",
    "/changelog",
    "/context",
    "/help",
    "/info",
    "/mcp",
    "/perf",
    "/prompts",
    "/ps",
    "/pwd",
    "/session",
    "/skills",
    "/stats",
    "/status",
    "/tasks",
    "/tools",
    "/usage",
    "/workspace",
];

/// Whether `text` is a bare report command, safe to run during a turn.
pub(crate) fn is_report(text: &str) -> bool {
    let text = text.trim();
    REPORT_COMMANDS
        .iter()
        .any(|name| name.eq_ignore_ascii_case(text))
}

/// The remote catalog with the ACP-only commands made host commands.
fn catalog() -> Vec<RemoteCommandInfo> {
    crate::remote::ops::command_catalog()
        .into_iter()
        .map(|mut command| {
            if command.name == "/remote" {
                command.action = "remote".into();
                command.description = "Share this chat with a phone or another device".into();
                command.insertion = "/remote".into();
            }
            if let Some((_, arguments)) =
                ACP_COMMANDS.iter().find(|(name, _)| *name == command.name)
            {
                command.action = "execute".into();
                if let Some(description) = command.description.strip_suffix(" · On Mac") {
                    command.description = description.to_owned();
                }
                command.insertion =
                    format!("{}{}", command.name, if *arguments { " " } else { "" });
            }
            command
        })
        .collect()
}

/// Commands a client can offer: host commands (`execute`) and those it handles
/// natively (`new_session`, `model_picker`, `session_picker`, `copy_response`).
/// Terminal-only commands are left out. ACP names carry no leading slash; the
/// RustCode action rides in `_meta.rustcode.action`.
pub(crate) fn available_commands_update() -> SessionUpdate {
    let commands = catalog()
        .into_iter()
        .filter(|command| command.action != "terminal")
        .map(available_command)
        .collect();
    SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(commands))
}

fn available_command(command: RemoteCommandInfo) -> AvailableCommand {
    let takes_arguments = command.insertion.ends_with(' ');
    let mut rustcode = serde_json::Map::new();
    rustcode.insert("action".into(), command.action.clone().into());
    if command.action == "execute" && is_report(&command.name) {
        rustcode.insert("report".into(), true.into());
    }
    let mut meta = serde_json::Map::new();
    meta.insert("rustcode".into(), serde_json::Value::Object(rustcode));
    AvailableCommand::new(
        command.name.trim_start_matches('/').to_owned(),
        command.description,
    )
    .input(
        takes_arguments.then(|| {
            AvailableCommandInput::Unstructured(UnstructuredCommandInput::new("arguments"))
        }),
    )
    .meta(meta)
}

/// The command `text` invokes, lowercased with its slash, or `None` when it
/// is prompt text. A first word naming a catalog command is a command. So is
/// one shaped like a command name (`/` and lowercase letters, digits, `_` or
/// `-`) that is not an existing path, which catches typos and terminal-only
/// commands; `/Users/me/x.rs fix this` and `/tmp is full` stay prompts.
fn command_name(text: &str, catalog: &[RemoteCommandInfo]) -> Option<String> {
    let first = text.split_whitespace().next()?;
    let rest = first.strip_prefix('/')?;
    let name = first.to_ascii_lowercase();
    if catalog.iter().any(|command| command.name == name) {
        return Some(name);
    }
    let looks_like_command = rest.starts_with(|c: char| c.is_ascii_lowercase())
        && rest
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    (looks_like_command && !std::path::Path::new(first).exists()).then_some(name)
}

/// What a slash command typed over ACP does.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AcpCommand {
    /// The Markdown reply, ready now.
    Reply(String),
    /// A host command that waits on I/O (`/diff`, `/quota`).
    Io(String),
    Compact,
    Recap,
    Summarize,
    Fork,
    /// A provider login command, with its full input.
    ProviderAuth(String),
    /// `/goal`: start a model turn on this prompt in continuous mode.
    Goal(String),
}

fn reply(result: Result<RemoteResult, crate::remote::protocol::RemoteError>, name: &str) -> String {
    match result {
        Ok(RemoteResult::CommandExecuted { title, output, .. }) => {
            format!("**{title}**\n\n{output}")
        }
        Ok(_) => format!("`{name}` finished."),
        Err(error) => format!("`{name}`: {}", error.message),
    }
}

/// Classify `text`, running it when the answer is immediate. `None` means
/// the text is a prompt for the model, and like a terminal submit it starts
/// a task: the session's delegation setting decides whether the turn gets the
/// agent tools, consuming a one-shot `/delegate`.
pub(crate) fn classify(state: &mut crate::app::AppState, text: &str) -> Option<AcpCommand> {
    let command = classify_command(state, text);
    if command.is_none() {
        crate::app::actions::begin_task_delegation(state);
    }
    command
}

fn classify_command(state: &mut crate::app::AppState, text: &str) -> Option<AcpCommand> {
    let text = text.trim();
    let catalog = catalog();
    let name = command_name(text, &catalog)?;
    let arguments = text
        .split_once(char::is_whitespace)
        .map_or("", |(_, arguments)| arguments.trim());
    let Some(command) = catalog.iter().find(|command| command.name == name) else {
        return Some(AcpCommand::Reply(format!(
            "Unknown command `{name}`. Use `/help` to list the commands."
        )));
    };
    Some(match (command.action.as_str(), name.as_str()) {
        ("terminal", _) => AcpCommand::Reply(format!(
            "`{name}` is not available in this app. Use it in the RustCode terminal."
        )),
        ("execute", _) if ACP_COMMANDS.iter().any(|(acp, _)| *acp == name) => match name.as_str() {
            "/compact" | "/recap" | "/summarize" | "/fork" if !arguments.is_empty() => {
                AcpCommand::Reply(format!("`{name}` takes no arguments."))
            }
            "/compact" => AcpCommand::Compact,
            "/recap" => AcpCommand::Recap,
            "/summarize" => AcpCommand::Summarize,
            "/fork" => AcpCommand::Fork,
            "/goal" if arguments.is_empty() => {
                AcpCommand::Reply("**Goal**\n\nUsage: `/goal <task description>`".into())
            }
            "/goal" => {
                crate::app::actions::begin_task_delegation(state);
                state.continuous_mode = true;
                AcpCommand::Goal(format!(
                    "Goal: {arguments}\n\nContinuous autoloop mode is active. You must execute tools in a loop to complete the goal, and call the 'complete_task' tool when you are fully finished."
                ))
            }
            _ => AcpCommand::ProviderAuth(format!("{name}{}", &text[name.len()..])),
        },
        ("execute", "/help") if arguments.is_empty() => AcpCommand::Reply(format!(
            "**Commands**\n\n{}",
            catalog
                .iter()
                .filter(|command| command.action != "terminal")
                .map(|command| format!("- `{}` — {}", command.name, command.description))
                .collect::<Vec<_>>()
                .join("\n")
        )),
        ("execute", "/diff" | "/quota") => AcpCommand::Io(text.to_owned()),
        ("execute", _) => AcpCommand::Reply(reply(
            crate::remote::ops::execute_command(state, text),
            &name,
        )),
        _ => AcpCommand::Reply(format!(
            "`{name}` is an app action; use the app's control for it."
        )),
    })
}

/// Whether `text` is a bare `/context`, whose reply carries the breakdown
/// as data too.
pub(crate) fn is_context_report(text: &str) -> bool {
    text.trim().eq_ignore_ascii_case("/context")
}

/// Runs a command that waits on I/O or the model, under the session's turn,
/// and returns its Markdown reply. `progress` receives interim lines, such as
/// a device code to enter while a login waits.
pub(crate) async fn run(
    command: AcpCommand,
    state: &std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>,
    client: &reqwest::Client,
    cancel: &tokio_util::sync::CancellationToken,
    mut progress: impl FnMut(String),
) -> String {
    match command {
        AcpCommand::Reply(reply) => reply,
        AcpCommand::Goal(prompt) => prompt,
        AcpCommand::Io(text) => {
            let name = text
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            match crate::remote::ops::execute_io_command(state, client, &text).await {
                Some(result) => reply(result, &name),
                None => format!("`{name}` is not available in this app."),
            }
        }
        AcpCommand::Compact => format!(
            "**Compact**\n\n{}",
            crate::app::actions::compact_history(state, client, cancel).await
        ),
        AcpCommand::Recap => {
            let notice = new_messages_after(state, |state| async move {
                crate::app::actions::generate_conversation_recap(&state, client, true).await;
            })
            .await;
            format!("**Recap**\n\n{notice}")
        }
        AcpCommand::Summarize => {
            let notice = new_messages_after(state, |state| async move {
                crate::app::actions::summarize_session(&state, client).await;
            })
            .await;
            format!("**Summary**\n\n{notice}")
        }
        AcpCommand::Fork => format!("**Fork**\n\n{}", fork(&*state.lock().await)),
        AcpCommand::ProviderAuth(input) => {
            // The title goes first so interim lines land under it.
            progress("**Provider authentication**\n\n".into());
            let config = state.lock().await.config.clone();
            let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
            let future = crate::provider_auth::execute_command_with_progress(
                &input,
                &config,
                cancel,
                Some(sender),
            );
            tokio::pin!(future);
            let result = loop {
                tokio::select! {
                    result = &mut future => break result,
                    Some(message) = receiver.recv() => progress(format!("{message}\n\n")),
                }
            };
            let mut state = state.lock().await;
            let message = crate::controller::apply_provider_auth_result(&mut state, result);
            message
        }
    }
}

/// Run `action`, then return what it added to the transcript: the messages
/// it appended, or the notice it left when it added none.
async fn new_messages_after<F, Fut>(
    state: &std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>,
    action: F,
) -> String
where
    F: FnOnce(std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let before = {
        let mut state = state.lock().await;
        state.transient_notice = None;
        state.history.len()
    };
    action(std::sync::Arc::clone(state)).await;
    let mut state = state.lock().await;
    let added = state
        .history
        .get(before..)
        .unwrap_or_default()
        .iter()
        .map(|message| message.content.trim().to_owned())
        .filter(|content| !content.is_empty())
        .collect::<Vec<_>>();
    if !added.is_empty() {
        let session_id = state.active_session_id.clone();
        crate::config::save_session_history(&session_id, &state.history);
        return added.join("\n\n");
    }
    state
        .transient_notice
        .take()
        .map(|(notice, _)| notice)
        .unwrap_or_else(|| "Nothing was added.".into())
}

/// Copy this chat into a new one beside it, leaving the open session as is.
fn fork(state: &crate::app::AppState) -> String {
    if !crate::config::session_has_content(&state.history) {
        return "There is nothing to fork yet.".into();
    }
    let session_id = crate::config::create_detached_session();
    if session_id.is_empty() {
        return "Session storage is unavailable, so the chat cannot be forked.".into();
    }
    // The fork shares the source checkout, never the task worktree.
    let workspace = crate::config::load_session_workspace(&state.active_session_id)
        .map(|workspace| rustcode_session::SessionWorkspace {
            task_workspace_id: None,
            ..workspace
        })
        .or_else(|| {
            state
                .workspace_root
                .clone()
                .map(|cwd| rustcode_session::SessionWorkspace {
                    cwd,
                    additional_directories: Vec::new(),
                    task_workspace_id: None,
                })
        });
    if let Some(workspace) = workspace {
        let _ = crate::config::save_session_workspace(&session_id, &workspace);
    }
    crate::config::record_session_settings(&session_id, &state.config);
    crate::config::save_session_history(&session_id, &state.history);
    let title = crate::config::load_session_title(&state.active_session_id)
        .map(|title| format!("{title} (fork)"))
        .unwrap_or_else(|| "Forked chat".into());
    crate::config::save_session_title(&session_id, &title);
    crate::config::flush_history();
    format!("Forked this chat into **{title}**. Open it from the chat list.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertises_host_and_native_commands_without_slash_or_terminal_ones() {
        let SessionUpdate::AvailableCommandsUpdate(update) = available_commands_update() else {
            panic!("expected an available_commands_update");
        };
        let names: Vec<_> = update
            .available_commands
            .iter()
            .map(|command| command.name.as_str())
            .collect();
        assert!(names.contains(&"status"));
        assert!(names.contains(&"model"));
        assert!(names.iter().all(|name| !name.starts_with('/')));
        let model = update
            .available_commands
            .iter()
            .find(|command| command.name == "model")
            .unwrap();
        assert_eq!(
            model.meta.as_ref().unwrap()["rustcode"]["action"],
            "model_picker"
        );
        assert!(model.input.is_some());
    }

    #[test]
    fn advertises_acp_commands_and_hides_the_deferred_ones() {
        let SessionUpdate::AvailableCommandsUpdate(update) = available_commands_update() else {
            panic!("expected an available_commands_update");
        };
        let find = |name: &str| {
            update
                .available_commands
                .iter()
                .find(|command| command.name == name)
        };
        for name in [
            "compact",
            "recap",
            "summarize",
            "goal",
            "fork",
            "login",
            "diff",
            "quota",
        ] {
            let command = find(name).unwrap_or_else(|| panic!("{name} is advertised"));
            assert_eq!(
                command.meta.as_ref().unwrap()["rustcode"]["action"],
                "execute"
            );
            assert!(!command.description.contains("On Mac"), "{name}");
        }
        assert!(find("goal").unwrap().input.is_some());
        assert!(find("compact").unwrap().input.is_none());
        assert_eq!(
            find("clear").unwrap().meta.as_ref().unwrap()["rustcode"]["action"],
            "new_session"
        );
        for name in ["delete_chat", "continue", "theme"] {
            assert!(find(name).is_none(), "{name} stays terminal-only");
        }
    }

    #[test]
    fn remote_is_advertised_as_an_app_action_not_a_prompt() {
        let SessionUpdate::AvailableCommandsUpdate(update) = available_commands_update() else {
            panic!("expected an available_commands_update");
        };
        let remote = update
            .available_commands
            .iter()
            .find(|command| command.name == "remote")
            .expect("remote is advertised");
        assert_eq!(
            remote.meta.as_ref().unwrap()["rustcode"]["action"],
            "remote"
        );
        assert!(remote.input.is_none());

        let mut state = crate::app::AppState::new();
        assert_eq!(
            classify(&mut state, "/remote"),
            Some(AcpCommand::Reply(
                "`/remote` is an app action; use the app's control for it.".into()
            ))
        );
        assert!(state.history.is_empty());
    }

    #[test]
    fn only_command_shaped_first_words_are_commands() {
        let catalog = catalog();
        let name = |text: &str| command_name(text, &catalog);
        assert_eq!(name("/help").as_deref(), Some("/help"));
        assert_eq!(name("/HELP me").as_deref(), Some("/help"));
        assert_eq!(name("/stauts").as_deref(), Some("/stauts"));
        assert_eq!(
            name("/not-a-command please").as_deref(),
            Some("/not-a-command")
        );
        assert_eq!(name("/delete_chat").as_deref(), Some("/delete_chat"));
        // Paths and other prose go to the model.
        assert_eq!(name("/Users/foo/bar.rs fix this"), None);
        assert_eq!(name("/src/main.rs"), None);
        assert_eq!(name("/tmp is full, why?"), None);
        assert_eq!(name("/Foo bar"), None);
        assert_eq!(name("/ alone"), None);
        assert_eq!(name("/"), None);
        assert_eq!(name("/9lives"), None);
        assert_eq!(name("explain /help"), None);
        assert_eq!(name("   "), None);
    }

    #[test]
    fn classifies_commands_without_sending_them_to_the_model() {
        let mut state = crate::app::AppState::new();
        let reply = |state: &mut crate::app::AppState, text: &str| match classify(state, text) {
            Some(AcpCommand::Reply(reply)) => reply,
            other => panic!("{text}: expected a reply, got {other:?}"),
        };
        let help = reply(&mut state, "  /help");
        assert!(help.starts_with("**Commands**"));
        assert!(help.contains("`/compact`"));
        assert!(!help.contains("On Mac"));
        assert!(reply(&mut state, "/status").starts_with("**Status**"));
        assert!(reply(&mut state, "/context").contains("| Category |"));
        assert!(reply(&mut state, "/new").contains("app action"));
        assert!(reply(&mut state, "/clear").contains("app action"));
        assert!(
            reply(&mut state, "/not-a-command please")
                .starts_with("Unknown command `/not-a-command`")
        );
        assert!(reply(&mut state, "/theme dark").contains("`/theme` is not available in this app"));
        assert!(reply(&mut state, "/delete_chat").contains("not available in this app"));
        assert!(reply(&mut state, "/continue").contains("not available in this app"));
        assert!(reply(&mut state, "/goal").contains("Usage: `/goal"));
        assert!(reply(&mut state, "/compact now").contains("takes no arguments"));
        assert_eq!(classify(&mut state, "/compact"), Some(AcpCommand::Compact));
        assert_eq!(classify(&mut state, "/recap"), Some(AcpCommand::Recap));
        assert_eq!(
            classify(&mut state, "/summarize"),
            Some(AcpCommand::Summarize)
        );
        assert_eq!(classify(&mut state, "/fork"), Some(AcpCommand::Fork));
        assert_eq!(
            classify(&mut state, "/diff"),
            Some(AcpCommand::Io("/diff".into()))
        );
        assert_eq!(
            classify(&mut state, "/quota"),
            Some(AcpCommand::Io("/quota".into()))
        );
        assert_eq!(
            classify(&mut state, "/LOGIN openai"),
            Some(AcpCommand::ProviderAuth("/login openai".into()))
        );
        assert!(classify(&mut state, "/Users/me/x.rs fix this").is_none());
        assert!(classify(&mut state, "explain /help").is_none());
        assert!(state.history.is_empty());
    }

    #[test]
    fn only_bare_read_only_commands_are_reports() {
        for text in ["/help", " /STATUS ", "/context", "/tasks", "/usage"] {
            assert!(is_report(text), "{text}");
        }
        for text in [
            "/context 256k",
            "/compact",
            "/fork",
            "/goal ship it",
            "/title new name",
            "/yolo on",
            "/stop",
            "/diff",
            "/help me",
            "status",
        ] {
            assert!(!is_report(text), "{text}");
        }
        // Every report is a host command whose bare form replies at once.
        let mut state = crate::app::AppState::new();
        for name in REPORT_COMMANDS {
            assert!(
                matches!(classify(&mut state, name), Some(AcpCommand::Reply(_))),
                "{name}"
            );
        }
        assert!(state.history.is_empty());
        let SessionUpdate::AvailableCommandsUpdate(update) = available_commands_update() else {
            panic!("expected an available_commands_update");
        };
        let report = |name: &str| {
            update
                .available_commands
                .iter()
                .find(|command| command.name == name)
                .and_then(|command| command.meta.as_ref())
                .and_then(|meta| meta["rustcode"].get("report").cloned())
        };
        assert_eq!(report("status"), Some(true.into()));
        assert_eq!(report("compact"), None);
    }

    #[test]
    fn a_model_prompt_gets_the_agent_tools_the_session_allows() {
        let mut state = crate::app::AppState::new();
        state.config.delegation_enabled = true;
        state.delegation_sticky = true;
        assert!(classify(&mut state, "investigate the failing build").is_none());
        assert!(state.delegation_active);

        state.delegation_sticky = false;
        state.delegation_armed = true;
        assert!(classify(&mut state, "once with agents").is_none());
        assert!(state.delegation_active);
        assert!(!state.delegation_armed);
        assert!(classify(&mut state, "then without").is_none());
        assert!(!state.delegation_active);

        state.delegation_sticky = true;
        state.config.delegation_enabled = false;
        assert!(classify(&mut state, "host disabled").is_none());
        assert!(!state.delegation_active);
    }

    #[test]
    fn goal_becomes_a_continuous_turn_prompt() {
        let mut state = crate::app::AppState::new();
        let Some(AcpCommand::Goal(prompt)) = classify(&mut state, "/goal ship the fix") else {
            panic!("expected a goal");
        };
        assert!(prompt.starts_with("Goal: ship the fix\n\n"));
        assert!(prompt.contains("complete_task"));
        assert!(state.continuous_mode);
    }

    #[test]
    fn fork_copies_the_chat_into_a_new_session_and_leaves_this_one_open() {
        let mut state = crate::app::AppState::new();
        assert!(fork(&state).contains("nothing to fork"));
        state
            .history
            .push(crate::app::ChatMessage::new("user", "original question"));
        state
            .history
            .push(crate::app::ChatMessage::new("assistant", "original answer"));
        let open = state.active_session_id.clone();
        let reply = fork(&state);
        assert!(reply.contains("Forked this chat"), "{reply}");
        assert_eq!(state.active_session_id, open);
        let forked = crate::config::list_sessions()
            .into_iter()
            .filter_map(|meta| crate::config::session_id_from_path(&meta.path))
            .filter(|id| *id != open)
            .find(|id| {
                crate::config::load_session_by_id(id).is_some_and(|(_, history)| {
                    history
                        .iter()
                        .any(|message| message.content == "original answer")
                })
            });
        assert!(forked.is_some(), "the fork is saved as its own session");
    }

    #[tokio::test]
    async fn io_and_model_commands_reply_under_the_turn() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = crate::app::AppState::new();
        app.workspace_root = Some(directory.path().to_path_buf());
        let state = std::sync::Arc::new(tokio::sync::Mutex::new(app));
        let client = reqwest::Client::new();
        let cancel = tokio_util::sync::CancellationToken::new();
        let diff = run(
            AcpCommand::Io("/diff".into()),
            &state,
            &client,
            &cancel,
            |_| {},
        )
        .await;
        assert!(diff.starts_with("**Git diff**"), "{diff}");
        let compact = run(AcpCommand::Compact, &state, &client, &cancel, |_| {}).await;
        assert!(
            compact.contains("Not enough messages to compact"),
            "{compact}"
        );
        let recap = run(AcpCommand::Recap, &state, &client, &cancel, |_| {}).await;
        assert!(
            recap.contains("no conversation history to recap"),
            "{recap}"
        );
    }
}
