//! Slash commands over ACP. The catalog and the allowlisted host commands are
//! the ones the remote (iOS) client uses; ACP advertises them with
//! `available_commands_update` and runs them when a prompt starts with one.

use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate, SessionUpdate,
    UnstructuredCommandInput,
};

use crate::remote::protocol::{RemoteCommandInfo, RemoteResult};

/// Commands a client can offer: host commands (`execute`) and those it handles
/// natively (`new_session`, `model_picker`, `session_picker`, `copy_response`).
/// Terminal-only commands are left out. ACP names carry no leading slash; the
/// RustCode action rides in `_meta.rustcode.action`.
pub(crate) fn available_commands_update() -> SessionUpdate {
    let commands = crate::remote::ops::command_catalog()
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

/// Runs `text` when it is one of the host commands, returning the Markdown
/// reply for the client. Anything else, including prompt text that merely
/// starts with a slash, is not a command and returns `None`.
pub(crate) fn run(state: &mut crate::app::AppState, text: &str) -> Option<String> {
    let text = text.trim();
    let name = text.split_whitespace().next()?.to_ascii_lowercase();
    if !name.starts_with('/') {
        return None;
    }
    let is_host_command = crate::remote::ops::command_catalog()
        .iter()
        .any(|command| command.action == "execute" && command.name == name);
    if !is_host_command {
        return None;
    }
    Some(match crate::remote::ops::execute_command(state, text) {
        Ok(RemoteResult::CommandExecuted { title, output, .. }) => {
            format!("**{title}**\n\n{output}")
        }
        Ok(_) => format!("`{name}` finished."),
        Err(error) => format!("`{name}`: {}", error.message),
    })
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
    fn runs_host_commands_and_leaves_other_text_to_the_model() {
        let mut state = crate::app::AppState::new();
        let reply = run(&mut state, "  /help").expect("help is a host command");
        assert!(reply.starts_with("**"));
        assert!(run(&mut state, "/new").is_none());
        assert!(run(&mut state, "/not-a-command please").is_none());
        assert!(run(&mut state, "explain /help").is_none());
    }
}
