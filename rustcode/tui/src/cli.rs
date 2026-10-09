use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "rustcode",
    version = env!("CARGO_PKG_VERSION"),
    about = "AI-powered agentic coding assistant terminal"
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

    /// Include sessions from all workspaces (default scopes the picker,
    /// bare --resume/--continue, and `sessions list` to the current
    /// workspace; explicit --resume/--continue <id> always bypasses scoping)
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

    /// Manage the background scheduler daemon
    Daemon {
        #[command(subcommand)]
        command: DaemonCommands,
    },

    /// Serve interactive sessions over TCP for remote frontends (experimental)
    Serve {
        /// TCP port to listen on
        #[arg(long, default_value_t = 17878)]
        port: u16,
        /// Address to bind (loopback by default; non-loopback needs --allow-remote)
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// Per-launch auth token (generated and printed when omitted)
        #[arg(long)]
        token: Option<String>,
        /// Allow binding non-loopback addresses for LAN clients
        #[arg(long)]
        allow_remote: bool,
    },

    /// Run the remote gateway that shares `/remote` sessions and manage paired devices
    Remote {
        #[command(subcommand)]
        command: RemoteCommands,
    },

    /// Manage scheduled jobs through the running daemon
    Cron {
        #[command(subcommand)]
        command: CronCommands,
    },

    /// Add, list, or remove MCP servers in the user config
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum McpCommands {
    /// Add an MCP server: a URL for a remote server, or a command to spawn
    #[command(after_help = "Examples:
  rustcode mcp add --transport http api https://mcp.example.com/mcp --header \"Authorization: Bearer <token>\"
  rustcode mcp add --env API_KEY=<key> files -- npx -y @example/files-mcp")]
    Add(McpAddArgs),
    /// List configured MCP servers (header and environment values are hidden)
    List,
    /// Remove an MCP server from the user config
    #[command(alias = "rm")]
    Remove { name: String },
}

#[derive(clap::Args, Debug)]
pub struct McpAddArgs {
    /// Name the server's tools are grouped under
    pub name: String,
    /// Server URL, or the command and its arguments (put them after `--`
    /// when an argument starts with a dash)
    #[arg(required = true, num_args = 1.., value_name = "URL_OR_COMMAND")]
    pub target: Vec<String>,
    /// Transport; inferred from an http(s):// target when omitted
    #[arg(short = 't', long, value_enum)]
    pub transport: Option<McpTransport>,
    /// HTTP header sent with every request to a remote server, as `Name: value`
    #[arg(short = 'H', long = "header", value_name = "HEADER")]
    pub headers: Vec<String>,
    /// Environment variable for a spawned server, as `KEY=value`
    #[arg(short = 'e', long = "env", value_name = "KEY=VALUE")]
    pub env: Vec<String>,
    /// Pre-registered OAuth client ID for a remote server
    #[arg(long)]
    pub client_id: Option<String>,
    /// Reserve this server's whole toolset in every native tool request
    #[arg(long)]
    pub always_include: bool,
    /// Replace an existing server with the same name
    #[arg(long)]
    pub force: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpTransport {
    /// Spawn a local command and talk over stdin/stdout
    Stdio,
    /// Remote Streamable HTTP endpoint
    #[value(alias = "streamable-http")]
    Http,
}

/// Turn `mcp add` arguments into a config entry through the validator the
/// `manage_mcp_servers` tool shares.
pub(crate) fn mcp_server_from_args(
    args: &McpAddArgs,
) -> Result<rustcode::config::McpServerConfig, String> {
    rustcode::config::mcp_server_from_spec(&rustcode::config::McpServerSpec {
        name: args.name.clone(),
        target: args.target.clone(),
        transport: args.transport.map(|transport| match transport {
            McpTransport::Stdio => rustcode::config::McpTransport::Stdio,
            McpTransport::Http => rustcode::config::McpTransport::Http,
        }),
        headers: args.headers.clone(),
        env: args.env.clone(),
        client_id: args.client_id.clone(),
        always_include: args.always_include,
    })
}

pub(crate) use rustcode::config::mcp_server_summary;

#[derive(clap::Subcommand, Debug)]
pub enum DaemonCommands {
    /// Start the daemon in the background
    Start {
        #[arg(long)]
        json: bool,
    },
    /// Run the daemon in the foreground
    #[command(hide = true)]
    Run {
        #[arg(long)]
        json: bool,
    },
    /// Stop the running daemon
    Stop {
        #[arg(long)]
        json: bool,
    },
    /// Show daemon status
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Show the tail of the daemon log
    Logs {
        #[arg(long, default_value_t = 100, value_parser = parse_log_lines)]
        lines: usize,
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum RemoteCommands {
    /// Run the gateway in the foreground
    Serve {
        /// Address to bind (auto selects Wi-Fi/Ethernet; plain LAN traffic is not encrypted)
        #[arg(long, default_value = "auto")]
        bind: String,
        /// TCP port of the WebSocket listener
        #[arg(long, default_value_t = rustcode::remote_gateway::DEFAULT_PORT)]
        port: u16,
        /// Address devices should dial, when it differs from --bind (required for 0.0.0.0 with several interfaces)
        #[arg(long)]
        advertise: Option<String>,
    },
    /// Create a pairing challenge on the running gateway and show it
    Pair,
    /// List paired devices
    Devices {
        #[arg(long)]
        json: bool,
    },
    /// Revoke a paired device and close its connections
    Revoke {
        /// Device identifier, identifier prefix, or unique name
        device: String,
    },
    /// Show gateway status
    Status,
    /// Stop the running gateway
    Stop,
}

#[derive(clap::Subcommand, Debug)]
pub enum CronCommands {
    /// Add a scheduled job
    Add {
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        workspace: String,
        /// Schedule JSON, including an explicit timezone for recurring schedules
        #[arg(long)]
        schedule: String,
        /// Action JSON
        #[arg(long)]
        action: String,
        #[arg(long)]
        target_session: Option<String>,
        /// Retry policy JSON
        #[arg(long)]
        retry_policy: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List scheduled jobs
    List {
        #[arg(long)]
        json: bool,
    },
    /// Pause a scheduled job
    Pause {
        job_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Resume a scheduled job
    Resume {
        job_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Run a scheduled job now
    Run {
        job_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Show recent runs for a scheduled job
    History {
        job_id: String,
        #[arg(long, default_value_t = 20, value_parser = parse_history_limit)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Delete a scheduled job
    Delete {
        job_id: String,
        #[arg(long)]
        json: bool,
    },
}

fn parse_log_lines(value: &str) -> Result<usize, String> {
    parse_bounded_usize(value, 1, 1000, "lines")
}

fn parse_history_limit(value: &str) -> Result<usize, String> {
    parse_bounded_usize(value, 1, 50, "limit")
}

fn parse_bounded_usize(value: &str, min: usize, max: usize, name: &str) -> Result<usize, String> {
    let value = value
        .parse::<usize>()
        .map_err(|_| format!("{name} must be an integer"))?;
    if !(min..=max).contains(&value) {
        return Err(format!("{name} must be between {min} and {max}"));
    }
    Ok(value)
}

#[derive(clap::Subcommand, Debug)]
pub enum SessionCommands {
    /// Migrate legacy sessions into sessions/YYYY/MM/DD/<id>
    Migrate {
        /// Report the migration without changing files
        #[arg(long)]
        dry_run: bool,
    },
    /// List saved sessions (scoped to the current workspace by default)
    List {
        /// List sessions from all workspaces with their workspace per entry
        #[arg(long)]
        all: bool,
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
    fn parses_sessions_list_with_all_flag() {
        let cli = Cli::try_parse_from(["rustcode", "sessions", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Sessions {
                command: Some(SessionCommands::List { all: false })
            })
        ));
        let cli_all = Cli::try_parse_from(["rustcode", "sessions", "list", "--all"]).unwrap();
        assert!(matches!(
            cli_all.command,
            Some(Commands::Sessions {
                command: Some(SessionCommands::List { all: true })
            })
        ));
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
    fn parses_bench_stats() {
        let cli = Cli::try_parse_from([
            "rustcode",
            "bench",
            "--rounds",
            "4",
            "--calls",
            "6",
            "--recoveries",
            "1",
            "--completed",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Bench {
                rounds: 4,
                calls: 6,
                recoveries: 1,
                completed: true,
                ..
            })
        ));
    }

    #[test]
    fn parses_doctor_with_fix_flag() {
        let cli = Cli::try_parse_from(["rustcode", "doctor"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Doctor { fix: false })));
        let cli_fix = Cli::try_parse_from(["rustcode", "doctor", "--fix"]).unwrap();
        assert!(matches!(
            cli_fix.command,
            Some(Commands::Doctor { fix: true })
        ));
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

    #[test]
    fn parses_remote_gateway_commands() {
        let cli = Cli::try_parse_from(["rustcode", "remote", "serve"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Remote {
                command: RemoteCommands::Serve {
                    ref bind,
                    port: 17879,
                    advertise: None,
                }
            }) if bind == "auto"
        ));
        let cli = Cli::try_parse_from([
            "rustcode",
            "remote",
            "serve",
            "--bind",
            "0.0.0.0",
            "--port",
            "9000",
            "--advertise",
            "100.64.0.7",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Remote {
                command: RemoteCommands::Serve {
                    port: 9000,
                    advertise: Some(ref advertise),
                    ..
                }
            }) if advertise == "100.64.0.7"
        ));
        for action in ["pair", "devices", "status", "stop"] {
            let cli = Cli::try_parse_from(["rustcode", "remote", action]).unwrap();
            assert!(matches!(cli.command, Some(Commands::Remote { .. })));
        }
        let cli = Cli::try_parse_from(["rustcode", "remote", "revoke", "phone"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Remote {
                command: RemoteCommands::Revoke { ref device }
            }) if device == "phone"
        ));
        assert!(Cli::try_parse_from(["rustcode", "remote", "revoke"]).is_err());
        assert!(Cli::try_parse_from(["rustcode", "remote"]).is_err());
    }

    #[test]
    fn parses_daemon_control_commands() {
        for action in ["start", "run", "stop", "status"] {
            let cli = Cli::try_parse_from(["rustcode", "daemon", action, "--json"]).unwrap();
            assert!(matches!(cli.command, Some(Commands::Daemon { .. })));
        }
        let cli = Cli::try_parse_from(["rustcode", "daemon", "logs", "--lines", "25"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Daemon {
                command: DaemonCommands::Logs {
                    lines: 25,
                    json: false
                }
            })
        ));
    }

    #[test]
    fn parses_cron_add_structured_arguments() {
        let cli = Cli::try_parse_from([
            "rustcode",
            "cron",
            "add",
            "--id",
            "daily-report",
            "--name",
            "Daily report",
            "--workspace",
            "/work",
            "--schedule",
            r#"{"kind":"cron","expression":"0 9 * * *","timezone":"Europe/Oslo"}"#,
            "--action",
            r#"{"type":"prompt","prompt":"Report","workspace":"/work","model_profile":null,"session_id":null}"#,
            "--target-session",
            "session-1",
            "--retry-policy",
            r#"{"max_attempts":2,"initial_backoff_seconds":5,"max_backoff_seconds":30}"#,
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Cron {
                command: CronCommands::Add {
                    id,
                    target_session: Some(target),
                    json: true,
                    ..
                }
            }) if id == "daily-report" && target == "session-1"
        ));
    }

    #[test]
    fn parses_cron_job_controls_and_history_limit() {
        for action in ["pause", "resume", "run", "delete"] {
            let cli = Cli::try_parse_from(["rustcode", "cron", action, "job-1", "--json"]).unwrap();
            assert!(matches!(cli.command, Some(Commands::Cron { .. })));
        }
        let history = Cli::try_parse_from([
            "rustcode", "cron", "history", "job-1", "--limit", "7", "--json",
        ])
        .unwrap();
        assert!(matches!(
            history.command,
            Some(Commands::Cron {
                command: CronCommands::History {
                    job_id,
                    limit: 7,
                    json: true
                }
            }) if job_id == "job-1"
        ));
        assert!(Cli::try_parse_from(["rustcode", "cron", "list", "--json"]).is_ok());
    }

    #[test]
    fn rejects_incomplete_cron_add_and_unbounded_output_options() {
        assert!(
            Cli::try_parse_from([
                "rustcode",
                "cron",
                "add",
                "--id",
                "job-1",
                "--name",
                "Job",
                "--workspace",
                "/work",
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["rustcode", "daemon", "logs", "--lines", "0"]).is_err());
        assert!(
            Cli::try_parse_from(["rustcode", "cron", "history", "job-1", "--limit", "51"]).is_err()
        );
    }

    fn mcp_add(args: &[&str]) -> Result<rustcode::config::McpServerConfig, String> {
        let argv = ["rustcode", "mcp", "add"].iter().chain(args).copied();
        match Cli::try_parse_from(argv)
            .map_err(|error| error.to_string())?
            .command
        {
            Some(Commands::Mcp {
                command: McpCommands::Add(args),
            }) => mcp_server_from_args(&args),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn mcp_add_accepts_a_remote_server_with_trailing_header() {
        let server = mcp_add(&[
            "--transport",
            "http",
            "paral-x",
            "https://mcp.example.test/mcp",
            "--header",
            "Authorization: Bearer token",
        ])
        .unwrap();
        assert_eq!(server.name, "paral-x");
        assert_eq!(server.url.as_deref(), Some("https://mcp.example.test/mcp"));
        assert!(server.command.is_empty());
        assert_eq!(server.headers["Authorization"], "Bearer token");
        assert!(server.enabled);

        // The transport is inferred from the URL when the flag is omitted.
        assert_eq!(
            mcp_add(&["paral-x", "https://mcp.example.test/mcp"])
                .unwrap()
                .url,
            server.url
        );
    }

    #[test]
    fn mcp_add_accepts_a_stdio_command_with_dashed_arguments() {
        let server = mcp_add(&[
            "--env",
            "API_KEY=k",
            "files",
            "--",
            "npx",
            "-y",
            "files-mcp",
        ])
        .unwrap();
        assert_eq!(server.command, "npx");
        assert_eq!(server.args, ["-y", "files-mcp"]);
        assert_eq!(server.env["API_KEY"], "k");
        assert_eq!(server.url, None);
    }

    #[test]
    fn mcp_add_rejects_options_that_do_not_fit_the_transport() {
        for args in [
            &["--transport", "http", "x", "npx"][..],
            &["x", "https://mcp.example.test", "extra"],
            &["x", "https://mcp.example.test", "--env", "A=b"],
            &["x", "npx", "--header", "A: b"],
            &["x", "https://mcp.example.test", "--header", "no-separator"],
            &["--transport", "sse", "x", "https://mcp.example.test"],
            &["x"],
        ] {
            assert!(mcp_add(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn mcp_list_summary_hides_header_and_environment_values() {
        let mut server = mcp_add(&[
            "api",
            "https://mcp.example.test/mcp",
            "-H",
            "Authorization: Bearer hunter2",
            "-H",
            "X-Org: acme-secret",
        ])
        .unwrap();
        assert_eq!(
            mcp_server_summary(&server),
            "api  http  https://mcp.example.test/mcp  (headers: Authorization, X-Org)"
        );
        server.enabled = false;
        assert!(mcp_server_summary(&server).ends_with("[disabled]"));

        let stdio = mcp_add(&["-e", "API_KEY=hunter2", "files", "--", "npx", "-y", "m"]).unwrap();
        assert_eq!(
            mcp_server_summary(&stdio),
            "files  stdio  npx -y m  (env: API_KEY)"
        );
    }
}
