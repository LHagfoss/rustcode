//! Slash commands with UI-neutral behavior in the native app.

#[derive(Debug, PartialEq, Eq)]
pub(super) enum NativeSlashCommand {
    Help,
    New,
    Clear,
    Cancel,
    Model(Option<String>),
    ProviderAuth(String),
    Status,
    Perf,
    Usage,
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
        "/login" | "/logout" | "/auth" | "/account" | "/accounts" | "/refresh" => {
            NativeSlashCommand::ProviderAuth(format!(
                "{}{}",
                name.to_ascii_lowercase(),
                &input[name.len()..]
            ))
        }
        "/perf" => NativeSlashCommand::Perf,
        "/status" => NativeSlashCommand::Status,
        "/usage" => NativeSlashCommand::Usage,
        "/model" => NativeSlashCommand::Model((!arguments.is_empty()).then(|| arguments.into())),
        "/change_title" => {
            NativeSlashCommand::ChangeTitle((!arguments.is_empty()).then(|| arguments.into()))
        }
        "/info" => NativeSlashCommand::Info,
        _ => NativeSlashCommand::Unknown(name.to_owned()),
    })
}

pub(super) const HELP: &str = "Native commands:\n\n- `/help` — Show commands\n- `/new` — Start a new chat\n- `/clear` — Start a new chat\n- `/cancel` — Stop the current turn\n- `/model [profile]` — Show or select a model profile\n- `/login` — Show provider login methods\n- `/login <provider>` — Sign in to a provider\n- `/login github-copilot [new|account-id]` — Connect GitHub Copilot (GitHub CLI or configured OAuth app)\n- `/login claude` — Connect a Claude subscription through the local Claude Code CLI\n- `/login <provider> api-key <ENV_VAR>` — Save an API key environment variable\n- `/auth status` — Show saved provider accounts\n- `/accounts` — List configured provider accounts\n- `/account refresh [provider] [account-id]` — Refresh a provider model catalog\n- `/logout <provider> [account-id]` — Sign out of a provider account\n- `/status` — Show provider and session status\n- `/usage` — Show provider usage and local session token totals\n- `/change_title <title>` — Rename this chat\n- `/info` — Show session and turn status";

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
        assert_eq!(parse("/status"), Some(NativeSlashCommand::Status));
        assert_eq!(parse("/usage"), Some(NativeSlashCommand::Usage));
        assert_eq!(
            parse("/accounts"),
            Some(NativeSlashCommand::ProviderAuth("/accounts".into()))
        );
        assert_eq!(
            parse("/account refresh openai account-1"),
            Some(NativeSlashCommand::ProviderAuth(
                "/account refresh openai account-1".into()
            ))
        );
        assert_eq!(parse("ordinary prompt"), None);
        assert_eq!(
            parse("/not-supported argument"),
            Some(NativeSlashCommand::Unknown("/not-supported".into()))
        );
    }

    #[test]
    fn parses_provider_auth_commands_as_native_commands() {
        assert_eq!(
            parse("/login openai"),
            Some(NativeSlashCommand::ProviderAuth("/login openai".into()))
        );
        assert_eq!(
            parse("/auth status"),
            Some(NativeSlashCommand::ProviderAuth("/auth status".into()))
        );
        assert_eq!(
            parse("/logout openai"),
            Some(NativeSlashCommand::ProviderAuth("/logout openai".into()))
        );
        assert_eq!(
            parse("/LOGIN OpenAI"),
            Some(NativeSlashCommand::ProviderAuth("/login OpenAI".into()))
        );
    }

    #[test]
    fn leaves_unrelated_slashes_out_of_provider_auth_dispatch() {
        assert_eq!(
            parse("/loginish openai"),
            Some(NativeSlashCommand::Unknown("/loginish".into()))
        );
        assert_eq!(
            parse("/authenticate"),
            Some(NativeSlashCommand::Unknown("/authenticate".into()))
        );
    }
}
