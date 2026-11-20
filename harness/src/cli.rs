use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "rustcode",
    version = env!("CARGO_PKG_VERSION"),
    about = "RustCode: standalone terminal coding agent harness (TUI and headless runner)"
)]
pub struct Cli {
    /// Resume a chat session: bare `--resume` restores the most recent
    /// session in the current workspace, `--resume <id>` restores that
    /// exact session from any workspace.
    #[arg(short = 'r', long = "resume", num_args = 0..=1, default_missing_value = "")]
    pub resume: Option<String>,

    /// Resume a session and continue pending work: bare `--continue`
    /// uses the most recent session in the current workspace,
    /// `--continue <id>` continues that exact session from any workspace.
    #[arg(short = 'c', long = "continue", num_args = 0..=1, default_missing_value = "")]
    pub continue_session: Option<String>,

    /// Run a quick prompt non-interactively and exit
    #[arg(short = 'p', long = "prompt")]
    pub prompt: Option<String>,

    /// Repeat a headless prompt up to N turns until `<loop:done/>` (circuit breaker, max 10)
    #[arg(long = "loop")]
    pub loop_count: Option<usize>,

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

    /// Use the full terminal screen for the interactive UI
    #[arg(long = "fullscreen")]
    pub fullscreen: bool,

    /// Include threads from all workspaces (default scopes the picker and
    /// bare --resume/--continue to the current workspace; explicit
    /// --resume/--continue <id> always bypasses scoping)
    #[arg(long = "all")]
    pub all: bool,

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

    /// List, show, create, validate and set terminal themes
    Theme {
        #[command(subcommand)]
        command: ThemeCommands,
    },

    /// Check environment prerequisites (config, binaries, skills)
    Doctor {
        /// Create missing config/skill directories where possible
        #[arg(long)]
        fix: bool,
    },

    /// Inspect or compare structured turn telemetry (legacy scoring supported)
    Bench {
        /// Read a session performance.json report
        #[arg(long)]
        report: Option<std::path::PathBuf>,
        /// Compare the report with a baseline performance.json
        #[arg(long, requires = "report")]
        baseline: Option<std::path::PathBuf>,
        /// Tool rounds used
        #[arg(long, default_value_t = 0)]
        rounds: usize,
        /// Tool calls made
        #[arg(long, default_value_t = 0)]
        calls: usize,
        /// Recovery events
        #[arg(long, default_value_t = 0)]
        recoveries: usize,
        /// Turn completed successfully
        #[arg(long, default_value_t = false)]
        completed: bool,
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

    /// Detached session owner for mobile-created TUI sessions (started by the gateway)
    #[command(hide = true)]
    Remote {
        #[command(subcommand)]
        command: RemoteCommands,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum RemoteCommands {
    /// Own a mobile-created session without a terminal
    #[command(hide = true)]
    SessionOwner {
        #[arg(long)]
        session_id: String,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum ThemeCommands {
    /// List every theme; `*` marks the active one
    List,
    /// Show a theme's resolved desktop tokens
    Show {
        name: String,
        /// Print the theme as `_rustcode/themes` reports it
        #[arg(long)]
        json: bool,
    },
    /// Write a commented theme file to the user themes directory
    New {
        name: String,
        /// Theme to start from (default: rustcode)
        #[arg(long)]
        from: Option<String>,
    },
    /// Make a theme the active one in the user config
    Set { name: String },
    /// Check theme files: every one, a theme by name, or a file by path
    Validate { target: Option<String> },
    /// Print the user themes directory, or a theme's file
    Path { name: Option<String> },
}

pub fn parse() -> Cli {
    Cli::parse()
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
    fn parses_acp_flag_and_subcommand() {
        let cli = Cli::try_parse_from(["rustcode", "--acp"]).unwrap();
        assert!(cli.acp);
        assert!(Cli::try_parse_from(["rustcode", "acp"]).is_ok());
    }

    #[test]
    fn resume_accepts_bare_flag_and_explicit_id() {
        assert_eq!(Cli::try_parse_from(["rustcode"]).unwrap().resume, None);
        assert_eq!(
            Cli::try_parse_from(["rustcode", "--resume"])
                .unwrap()
                .resume,
            Some(String::new())
        );
        assert_eq!(
            Cli::try_parse_from(["rustcode", "--resume", "abc-123"])
                .unwrap()
                .resume,
            Some("abc-123".to_owned())
        );
        assert_eq!(
            Cli::try_parse_from(["rustcode", "-r", "abc-123"])
                .unwrap()
                .resume,
            Some("abc-123".to_owned())
        );
        assert_eq!(
            Cli::try_parse_from(["rustcode", "-r"]).unwrap().resume,
            Some(String::new())
        );
    }

    #[test]
    fn continue_accepts_bare_flag_and_explicit_id() {
        assert_eq!(
            Cli::try_parse_from(["rustcode"]).unwrap().continue_session,
            None
        );
        assert_eq!(
            Cli::try_parse_from(["rustcode", "--continue"])
                .unwrap()
                .continue_session,
            Some(String::new())
        );
        assert_eq!(
            Cli::try_parse_from(["rustcode", "--continue", "sess-1"])
                .unwrap()
                .continue_session,
            Some("sess-1".to_owned())
        );
        assert_eq!(
            Cli::try_parse_from(["rustcode", "-c", "sess-1"])
                .unwrap()
                .continue_session,
            Some("sess-1".to_owned())
        );
    }

    #[test]
    fn resume_composes_with_model_fullscreen_yolo_and_all() {
        let cli = Cli::try_parse_from([
            "rustcode",
            "--resume",
            "sess-9",
            "--model",
            "profile",
            "--fullscreen",
            "--yolo",
            "--all",
        ])
        .unwrap();
        assert_eq!(cli.resume, Some("sess-9".to_owned()));
        assert_eq!(cli.model, Some("profile".to_owned()));
        assert!(cli.fullscreen);
        assert!(cli.yolo);
        assert!(cli.all);
    }

    #[test]
    fn fullscreen_is_opt_in() {
        assert!(!Cli::try_parse_from(["rustcode"]).unwrap().fullscreen);
        assert!(
            Cli::try_parse_from(["rustcode", "--fullscreen"])
                .unwrap()
                .fullscreen
        );
    }

    #[test]
    fn parses_loop_flag() {
        let cli = Cli::try_parse_from(["rustcode", "-p", "hi", "--loop", "5"]).unwrap();
        assert_eq!(cli.loop_count, Some(5));
    }

    #[test]
    fn parses_remote_session_owner() {
        let cli = Cli::try_parse_from([
            "rustcode",
            "remote",
            "session-owner",
            "--session-id",
            "sess-1",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Remote {
                command: RemoteCommands::SessionOwner { session_id }
            }) if session_id == "sess-1"
        ));
    }

    #[test]
    fn parses_theme_doctor_bench_and_discord() {
        assert!(matches!(
            Cli::try_parse_from(["rustcode", "theme", "list"])
                .unwrap()
                .command,
            Some(Commands::Theme {
                command: ThemeCommands::List
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["rustcode", "doctor", "--fix"])
                .unwrap()
                .command,
            Some(Commands::Doctor { fix: true })
        ));
        assert!(matches!(
            Cli::try_parse_from(["rustcode", "bench", "--rounds", "4", "--completed"])
                .unwrap()
                .command,
            Some(Commands::Bench {
                rounds: 4,
                completed: true,
                ..
            })
        ));
        assert!(Cli::try_parse_from(["rustcode", "discord", "--enable"]).is_ok());
        assert!(Cli::try_parse_from(["rustcode", "discord", "--enable", "--disable"]).is_err());
    }
}
