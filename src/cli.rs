use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "rustcode",
    version = env!("CARGO_PKG_VERSION"),
    about = "AI-powered agentic coding assistant terminal"
)]
pub struct Cli {
    /// Resume the most recent chat session
    #[arg(short = 'r', long = "resume")]
    pub resume: bool,

    /// Alias for --resume
    #[arg(short = 'c', long = "continue")]
    pub continue_session: bool,

    /// Run a quick prompt non-interactively and exit
    #[arg(short = 'p', long = "prompt")]
    pub prompt: Option<String>,

    /// Override the active AI model name
    #[arg(short = 'm', long = "model")]
    pub model: Option<String>,

    /// Check for and install the latest GitHub Release, using Homebrew for Homebrew installs
    #[arg(long = "update", alias = "upgrade")]
    pub update: bool,

    /// Run as a headless Agent Client Protocol server over stdio
    #[arg(long = "acp")]
    pub acp: bool,

    /// Automatically approve tool confirmations for this run
    #[arg(long = "yolo")]
    pub yolo: bool,

    /// Create a project-local .rustcode/config.toml from global defaults
    #[arg(long = "init")]
    pub init: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(clap::Subcommand, Debug)]
pub enum Commands {
    /// Create a project-local .rustcode/config.toml from global defaults
    Init,

    /// Run as a headless Agent Client Protocol server over stdio
    Acp,

    /// Sync config, skills, and themes with remote Git repository
    Sync {
        /// Pull latest config, skills, and themes from remote
        #[arg(long, conflicts_with = "push")]
        pull: bool,
        /// Push local config, skills, and themes to remote
        #[arg(long, conflicts_with = "pull")]
        push: bool,
        #[command(subcommand)]
        command: Option<SyncCommands>,
    },

    /// Inspect or migrate the on-disk session store
    Sessions {
        #[command(subcommand)]
        command: Option<SessionCommands>,
    },

    /// Configure RustCode's optional local Discord Rich Presence publisher
    Discord {
        /// Enable Rich Presence and print desktop Discord setup guidance
        #[arg(long, conflicts_with_all = ["status", "enable", "disable"])]
        setup: bool,
        /// Show configuration and whether a local Discord IPC socket is visible
        #[arg(long, conflicts_with_all = ["setup", "enable", "disable"])]
        status: bool,
        /// Enable Rich Presence in the RustCode config
        #[arg(long, conflicts_with_all = ["setup", "status", "disable"])]
        enable: bool,
        /// Disable Rich Presence in the RustCode config
        #[arg(long, conflicts_with_all = ["setup", "status", "enable"])]
        disable: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum SessionCommands {
    /// Migrate legacy sessions into sessions/YYYY/MM/DD/<id>
    Migrate {
        /// Report the migration without changing files
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum SyncCommands {
    /// Pull latest config, skills, and themes from remote
    Pull,
    /// Push local config, skills, and themes to remote
    Push,
    /// Initialize remote Git repository for config sync
    Init { remote_url: String },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SyncAction {
    Pull,
    Push,
    PullThenPush,
    Init(String),
}

pub(crate) fn resolve_sync_action(
    command: Option<SyncCommands>,
    pull: bool,
    push: bool,
) -> Result<SyncAction, &'static str> {
    if pull && push {
        return Err("--pull and --push cannot be used together");
    }

    if pull || push {
        if command.is_some() {
            return Err("sync flags cannot be combined with a sync subcommand");
        }
        return Ok(if pull {
            SyncAction::Pull
        } else {
            SyncAction::Push
        });
    }

    Ok(match command {
        Some(SyncCommands::Pull) => SyncAction::Pull,
        Some(SyncCommands::Push) => SyncAction::Push,
        Some(SyncCommands::Init { remote_url }) => SyncAction::Init(remote_url),
        None => SyncAction::PullThenPush,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn parses_update_and_upgrade_flags() {
        let cli = Cli::try_parse_from(["rustcode", "--update"]).unwrap();
        assert!(cli.update);
        let cli_alias = Cli::try_parse_from(["rustcode", "--upgrade"]).unwrap();
        assert!(cli_alias.update);
    }

    #[test]
    fn update_help_describes_supported_install_sources() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("GitHub Release"));
        assert!(help.contains("Homebrew"));
    }

    #[test]
    fn parses_acp_flag() {
        let cli = Cli::try_parse_from(["rustcode", "--acp"]).unwrap();
        assert!(cli.acp);
    }

    #[test]
    fn parses_acp_subcommand() {
        assert!(Cli::try_parse_from(["rustcode", "acp"]).is_ok());
    }

    #[test]
    fn parses_yolo_flag() {
        let cli = Cli::try_parse_from(["rustcode", "--yolo"]).unwrap();
        assert!(cli.yolo);
    }

    #[test]
    fn parses_project_init_flag_and_subcommand() {
        let flag = Cli::try_parse_from(["rustcode", "--init"]).unwrap();
        assert!(flag.init);
        assert!(matches!(
            Cli::try_parse_from(["rustcode", "init"]).unwrap().command,
            Some(Commands::Init)
        ));
    }

    #[test]
    fn parses_session_migration_dry_run() {
        let cli = Cli::try_parse_from(["rustcode", "sessions", "migrate", "--dry-run"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Sessions {
                command: Some(SessionCommands::Migrate { dry_run: true })
            })
        ));
    }

    #[test]
    fn parses_discord_configuration_flags() {
        assert!(matches!(
            Cli::try_parse_from(["rustcode", "discord", "--setup"])
                .unwrap()
                .command,
            Some(Commands::Discord { setup: true, .. })
        ));
        assert!(matches!(
            Cli::try_parse_from(["rustcode", "discord", "--status"])
                .unwrap()
                .command,
            Some(Commands::Discord { status: true, .. })
        ));
        assert!(Cli::try_parse_from(["rustcode", "discord", "--enable"]).is_ok());
        assert!(Cli::try_parse_from(["rustcode", "discord", "--disable"]).is_ok());
    }

    #[test]
    fn discord_configuration_flags_are_mutually_exclusive() {
        assert!(Cli::try_parse_from(["rustcode", "discord", "--enable", "--disable"]).is_err());
    }

    #[test]
    fn parses_sync_direction_flags() {
        let pull = Cli::try_parse_from(["rustcode", "sync", "--pull"]).unwrap();
        assert!(matches!(
            pull.command,
            Some(Commands::Sync {
                pull: true,
                push: false,
                command: None
            })
        ));

        let push = Cli::try_parse_from(["rustcode", "sync", "--push"]).unwrap();
        assert!(matches!(
            push.command,
            Some(Commands::Sync {
                pull: false,
                push: true,
                command: None
            })
        ));
    }

    #[test]
    fn preserves_sync_subcommands_and_init() {
        assert!(matches!(
            Cli::try_parse_from(["rustcode", "sync", "pull"])
                .unwrap()
                .command,
            Some(Commands::Sync {
                pull: false,
                push: false,
                command: Some(SyncCommands::Pull)
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["rustcode", "sync", "push"])
                .unwrap()
                .command,
            Some(Commands::Sync {
                pull: false,
                push: false,
                command: Some(SyncCommands::Push)
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["rustcode", "sync", "init", "origin"])
                .unwrap()
                .command,
            Some(Commands::Sync {
                pull: false,
                push: false,
                command: Some(SyncCommands::Init { remote_url })
            }) if remote_url == "origin"
        ));
    }

    #[test]
    fn rejects_both_sync_direction_flags() {
        let error = Cli::try_parse_from(["rustcode", "sync", "--pull", "--push"])
            .unwrap_err()
            .to_string();
        assert!(error.contains("cannot be used with"));
    }

    #[test]
    fn resolves_sync_dispatch_actions() {
        assert_eq!(
            resolve_sync_action(None, false, false),
            Ok(SyncAction::PullThenPush)
        );
        assert_eq!(resolve_sync_action(None, true, false), Ok(SyncAction::Pull));
        assert_eq!(resolve_sync_action(None, false, true), Ok(SyncAction::Push));
        assert_eq!(
            resolve_sync_action(Some(SyncCommands::Pull), false, false),
            Ok(SyncAction::Pull)
        );
        assert_eq!(
            resolve_sync_action(Some(SyncCommands::Push), false, false),
            Ok(SyncAction::Push)
        );
        assert_eq!(
            resolve_sync_action(
                Some(SyncCommands::Init {
                    remote_url: "origin".to_string()
                }),
                false,
                false
            ),
            Ok(SyncAction::Init("origin".to_string()))
        );
        assert_eq!(
            resolve_sync_action(None, true, true),
            Err("--pull and --push cannot be used together")
        );
        assert_eq!(
            resolve_sync_action(Some(SyncCommands::Pull), true, false),
            Err("sync flags cannot be combined with a sync subcommand")
        );
    }
}
