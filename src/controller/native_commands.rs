//! Slash commands with UI-neutral behavior in the native app.

#[derive(Debug, PartialEq, Eq)]
pub(super) enum NativeSlashCommand {
    Help,
    New,
    Clear,
    Cancel,
    Model(Option<String>),
    ChangeTitle(Option<String>),
    Info,
    Unknown(String),
}

pub(super) fn parse(input: &str) -> Option<NativeSlashCommand> {
    let input = input.trim();
    let (name, arguments) = input
        .split_once(char::is_whitespace)
        .map_or((input, ""), |(name, arguments)| (name, arguments.trim()));
    if !name.starts_with('/') {
        return None;
    }
    Some(match name.to_ascii_lowercase().as_str() {
        "/help" => NativeSlashCommand::Help,
        "/new" => NativeSlashCommand::New,
        "/clear" => NativeSlashCommand::Clear,
        "/cancel" => NativeSlashCommand::Cancel,
        "/model" => NativeSlashCommand::Model((!arguments.is_empty()).then(|| arguments.into())),
        "/change_title" => {
            NativeSlashCommand::ChangeTitle((!arguments.is_empty()).then(|| arguments.into()))
        }
        "/info" => NativeSlashCommand::Info,
        _ => NativeSlashCommand::Unknown(name.to_owned()),
    })
}

pub(super) const HELP: &str = "Native commands:\n\n- `/help` — Show commands\n- `/new` — Start a new chat\n- `/clear` — Start a new chat\n- `/cancel` — Stop the current turn\n- `/model [profile]` — Show or select a model profile\n- `/change_title <title>` — Rename this chat\n- `/info` — Show session and turn status";

#[cfg(test)]
mod tests {
    use super::{NativeSlashCommand, parse};

    #[test]
    fn parses_commands_without_swallowing_arguments() {
        assert_eq!(
            parse("  /MODEL deepseek flash  "),
            Some(NativeSlashCommand::Model(Some("deepseek flash".into())))
        );
        assert_eq!(
            parse("/change_title A longer chat title"),
            Some(NativeSlashCommand::ChangeTitle(Some(
                "A longer chat title".into()
            )))
        );
        assert_eq!(parse("/model"), Some(NativeSlashCommand::Model(None)));
        assert_eq!(parse("/info"), Some(NativeSlashCommand::Info));
        assert_eq!(parse("ordinary prompt"), None);
        assert_eq!(
            parse("/not-supported argument"),
            Some(NativeSlashCommand::Unknown("/not-supported".into()))
        );
    }
}
