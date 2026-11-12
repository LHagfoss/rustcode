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

    /// Include threads from all workspaces (default scopes the picker,
    /// bare --resume/--continue, and `threads list` to the current
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

    /// Export a thread as a zip archive (default: into ~/Downloads) to send
    /// to another machine; same as `threads export`
    Export {
        /// Thread id, as shown by `threads list` or `/status`; defaults to
        /// the most recent thread
        id: Option<String>,
        /// Directory to write the archive into
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },

    /// List, search, settle, export or locate saved threads (`sessions` still works)
    #[command(name = "threads", alias = "sessions")]
    Sessions {
        #[command(subcommand)]
        command: Option<SessionCommands>,
    },

    /// List, show, create, validate and set themes (terminal and desktop app)
    Theme {
        #[command(subcommand)]
        command: ThemeCommands,
    },

    /// Inspect user-data migrations
    Storage {
        #[command(subcommand)]
        command: StorageCommands,
    },

    /// Show tokens per harness and model from the local data store; cost is
    /// not computed (for disk space, see `storage usage`)
    Usage {
        /// Only turns started on or after this day, YYYY-MM-DD
        #[arg(long)]
        since: Option<String>,
        /// Print the totals as JSON
        #[arg(long)]
        json: bool,
    },

    /// Inspect the local data store: threads, turns and tool calls, metadata only
    Data {
        #[command(subcommand)]
        command: DataCommands,
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

    /// Show or set the model profile and reasoning effort new sessions start
    /// with, as JSON
    Defaults {
        /// Model profile name to make the default
        #[arg(long)]
        model: Option<String>,
        /// Reasoning effort for the default profile ("default" clears it)
        #[arg(long)]
        effort: Option<String>,
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

    /// Run or connect to the RustCode server: one engine that the app and
    /// other clients share, which keeps threads running when they close
    Server {
        #[command(subcommand)]
        command: ServerCommands,
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

    /// List, add and remove the machines this one is paired with (a phone or
    /// another computer running RustCode)
    #[command(alias = "machine")]
    Machines {
        #[command(subcommand)]
        command: MachineCommands,
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

    /// Stdio MCP server that gives an external harness thread the
    /// orchestration tools; started by the engine, not by hand
    #[command(name = "mcp-orchestrator", hide = true)]
    McpOrchestrator {
        /// The thread the harness runs
        #[arg(long)]
        session: String,
        /// The parent engine's orchestrator socket
        #[arg(long)]
        socket: std::path::PathBuf,
        /// The per-thread token the parent engine issued. Normally passed in
        /// `RUSTCODE_ORCHESTRATOR_TOKEN` instead, so it is not visible in `ps`.
        #[arg(long)]
        token: Option<String>,
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
pub enum ServerCommands {
    /// Run the server in the foreground until stopped; exits at once if one
    /// already runs for this user
    Run {
        /// Set by the service manager that starts the server at login
        #[arg(long, hide = true)]
        service: bool,
    },
    /// Relay ACP on stdin and stdout to the server, starting it in the
    /// background if it is not running
    Connect,
    /// Run the server in the background from login: a LaunchAgent on macOS,
    /// a systemd user unit on Linux, a logon task on Windows
    Install {
        /// Install a development build (inside a cargo target directory) anyway
        #[arg(long)]
        force_dev: bool,
        /// Stop a server that is not the service's even if turns are running
        #[arg(short, long)]
        yes: bool,
    },
    /// Show whether the server is installed and running, its clients and
    /// running turns, and what is wrong
    Status {
        /// Print JSON for scripts
        #[arg(long)]
        json: bool,
    },
    /// Stop the server and start it again from the installed service
    Restart {
        /// Wait until no turn is running first
        #[arg(long)]
        when_idle: bool,
        /// Restart even if turns are running, without asking
        #[arg(short, long)]
        yes: bool,
        /// Set by the restart itself on the helper it hands a restart to when
        /// it runs inside the server it stops
        #[arg(long, hide = true)]
        detached: bool,
    },
    /// Point the `rustcode` a shell runs at this one, when it is another
    /// version
    LinkPath,
    /// Update the `rustcode` a shell runs when it is an older copy an install
    /// left behind
    UpdateCli {
        /// Print what was found and done as JSON, for the app
        #[arg(long)]
        json: bool,
    },
    /// Remove the background service (threads, settings and pairings stay)
    Uninstall {
        /// Stop a server with turns running without asking
        #[arg(short, long)]
        yes: bool,
    },
    /// Show the server's log
    Logs {
        /// Keep printing what is appended
        #[arg(short, long)]
        follow: bool,
        /// How many of the last lines to show
        #[arg(short = 'n', long, default_value_t = 100)]
        lines: usize,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum RemoteCommands {
    /// Enable the persistent background gateway
    #[command(alias = "on")]
    Start,
    /// Own a mobile-created session without a terminal
    #[command(hide = true)]
    SessionOwner {
        #[arg(long)]
        session_id: String,
    },
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
        /// Prefer interfaces matching this name with an auto bind ("netbird"
        /// selects the mesh address first so the QR survives Wi-Fi changes)
        #[arg(long)]
        prefer_interface: Option<String>,
    },
    /// Create a pairing challenge on the running gateway and show it
    Pair,
    /// List paired devices
    Devices {
        #[command(subcommand)]
        command: Option<RemoteDeviceCommands>,
        #[arg(long)]
        json: bool,
    },
    /// Accept a device's request to connect and show the code to type on it
    /// (the request ids are in `rustcode remote status`)
    Accept {
        /// Request id from `rustcode remote status`
        id: String,
    },
    /// Deny a device's request to connect
    Deny {
        /// Request id from `rustcode remote status`
        id: String,
    },
    /// Revoke a paired device and close its connections
    Revoke {
        /// Device identifier, identifier prefix, or unique name
        device: String,
    },
    /// Show gateway status
    Status,
    /// Stop the running gateway
    #[command(alias = "off")]
    Stop,
}

#[derive(clap::Subcommand, Debug)]
pub enum MachineCommands {
    /// List saved machines, the route in use and whether each answers now
    List {
        /// Also read each connected machine's thread titles over ACP
        #[arg(long)]
        threads: bool,
        /// Print the machines as JSON
        #[arg(long)]
        json: bool,
    },
    /// Ask a machine to let this one in, pair with it and save it
    Add {
        /// Address to dial: host, host:port, [v6]:port or ws://host:port
        address: Option<String>,
        /// Name to save the machine under (default: the name it reports)
        #[arg(long)]
        name: Option<String>,
        /// The eight-digit code the machine shows, when it cannot be typed
        #[arg(long)]
        code: Option<String>,
        /// Choose the machine from Bonjour instead of typing an address
        #[arg(long, conflicts_with = "address")]
        discover: bool,
        /// How long to look for machines on the network with --discover
        #[arg(long, default_value_t = 4, value_name = "SECONDS")]
        discover_secs: u64,
    },
    /// Forget a machine and its token
    Remove {
        /// Name, host id or a prefix of one
        machine: String,
    },
    /// List the machines on this network that take a desktop client
    Discover {
        /// How long to listen for announcements
        #[arg(long, default_value_t = 4, value_name = "SECONDS")]
        secs: u64,
        /// Print what was found as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum RemoteDeviceCommands {
    /// Allow full ACP access: can see and run all threads on this machine
    Allow { device: String },
    /// Deny ACP access and disconnect the device immediately
    Deny { device: String },
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
    /// Migrate legacy threads into threads/YYYY/MM/DD/<id>
    Migrate {
        /// Report the migration without changing files
        #[arg(long)]
        dry_run: bool,
    },
    /// List saved threads (scoped to the current workspace by default)
    List {
        /// List threads from all workspaces with their workspace per entry
        #[arg(long)]
        all: bool,
    },
    /// Find saved threads whose messages contain a text (case-insensitive),
    /// scoped to the current workspace by default
    Search {
        /// Text to look for in user and assistant messages
        query: String,
        /// Search threads from all workspaces
        #[arg(long)]
        all: bool,
        /// Most threads to return
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Print the matches as a JSON array
        #[arg(long)]
        json: bool,
    },
    /// Print the on-disk directory of a thread (history.json, logs, …)
    Path {
        /// Thread id, as shown by `threads list` or `/session`
        id: String,
    },
    /// Mark threads settled (done), so they leave the app's thread list
    Settle {
        /// Thread ids, or unique prefixes of them
        #[arg(required = true, value_name = "ID")]
        ids: Vec<String>,
    },
    /// Put settled threads back to active; a thread in review is kept there
    Unsettle {
        /// Thread ids, or unique prefixes of them
        #[arg(required = true, value_name = "ID")]
        ids: Vec<String>,
    },
    /// Mark threads as needing review
    Review {
        /// Thread ids, or unique prefixes of them
        #[arg(required = true, value_name = "ID")]
        ids: Vec<String>,
    },
    /// Export a thread as a zip archive (default: into ~/Downloads)
    Export {
        /// Thread id; defaults to the most recent thread
        id: Option<String>,
        /// Directory to write the archive into
        #[arg(long)]
        out: Option<std::path::PathBuf>,
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

#[derive(clap::Subcommand, Debug)]
pub enum StorageCommands {
    /// List pending and completed user-data migrations without changing files
    Status {
        /// Print the report as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show how much disk space each store of RustCode's data takes, without
    /// changing files (`rustcode usage` reports tokens, not bytes)
    Usage {
        /// Print the report as JSON, with each store's paths
        #[arg(long)]
        json: bool,
    },
    /// List command output and attachments no thread refers to; --apply moves them aside
    Clean {
        /// Move the listed files to storage/removed, where they can be restored for 14 days
        #[arg(long)]
        apply: bool,
        /// Print the plan (and result) as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum DataCommands {
    /// Run one read-only SQL query over the data store (one statement, at most
    /// 1000 rows or 2 s)
    #[command(after_help = "Examples:
  rustcode data query \"SELECT * FROM threads LIMIT 5\"
  rustcode data query \"SELECT * FROM daily_usage ORDER BY day DESC LIMIT 10\" --json")]
    Query {
        /// The statement, quoted; only reads, and only the data store
        sql: String,
        /// Print the rows as JSON
        #[arg(long)]
        json: bool,
    },
    /// Print the tables and views of the data store with their columns
    Schema,
    /// Show each table's row count and size, so growth can be tracked
    Stats {
        /// Print the report as JSON
        #[arg(long)]
        json: bool,
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
        let threads = Cli::try_parse_from(["rustcode", "threads", "list"]).unwrap();
        assert!(matches!(
            threads.command,
            Some(Commands::Sessions {
                command: Some(SessionCommands::List { all: false })
            })
        ));
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
    fn parses_sessions_search() {
        let cli = Cli::try_parse_from(["rustcode", "sessions", "search", "parser"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Sessions {
                command: Some(SessionCommands::Search { query, all: false, limit: 50, json: false })
            }) if query == "parser"
        ));
        let cli = Cli::try_parse_from([
            "rustcode",
            "sessions",
            "search",
            "two words",
            "--all",
            "--limit",
            "5",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Sessions {
                command: Some(SessionCommands::Search { query, all: true, limit: 5, json: true })
            }) if query == "two words"
        ));
    }

    #[test]
    fn parses_threads_settle_unsettle_and_review_with_several_ids() {
        let cli = Cli::try_parse_from(["rustcode", "threads", "settle", "a", "b"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Sessions {
                command: Some(SessionCommands::Settle { ids })
            }) if ids == ["a".to_owned(), "b".to_owned()]
        ));
        let cli = Cli::try_parse_from(["rustcode", "threads", "unsettle", "a"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Sessions {
                command: Some(SessionCommands::Unsettle { ids })
            }) if ids == ["a".to_owned()]
        ));
        let cli = Cli::try_parse_from(["rustcode", "threads", "review", "a"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Sessions {
                command: Some(SessionCommands::Review { ids })
            }) if ids == ["a".to_owned()]
        ));
        // Several ids are the point; none is a usage error.
        assert!(Cli::try_parse_from(["rustcode", "threads", "settle"]).is_err());
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
    fn parses_theme_commands() {
        let parse = |args: &[&str]| {
            let mut argv = vec!["rustcode", "theme"];
            argv.extend_from_slice(args);
            match Cli::try_parse_from(argv).map(|cli| cli.command) {
                Ok(Some(Commands::Theme { command })) => Some(command),
                _ => None,
            }
        };
        assert!(matches!(parse(&["list"]), Some(ThemeCommands::List)));
        assert!(matches!(
            parse(&["show", "nord", "--json"]),
            Some(ThemeCommands::Show { name, json: true }) if name == "nord"
        ));
        assert!(matches!(
            parse(&["new", "mine", "--from", "light"]),
            Some(ThemeCommands::New { name, from: Some(from) }) if name == "mine" && from == "light"
        ));
        assert!(matches!(
            parse(&["new", "mine"]),
            Some(ThemeCommands::New { from: None, .. })
        ));
        assert!(matches!(
            parse(&["set", "dracula"]),
            Some(ThemeCommands::Set { name }) if name == "dracula"
        ));
        assert!(matches!(
            parse(&["validate"]),
            Some(ThemeCommands::Validate { target: None })
        ));
        assert!(matches!(
            parse(&["validate", "./x.toml"]),
            Some(ThemeCommands::Validate { target: Some(_) })
        ));
        assert!(matches!(
            parse(&["path"]),
            Some(ThemeCommands::Path { name: None })
        ));
        assert!(matches!(
            parse(&["path", "nord"]),
            Some(ThemeCommands::Path { name: Some(_) })
        ));
        assert!(parse(&["show"]).is_none(), "show needs a name");
        assert!(parse(&["paint"]).is_none());
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
        let export = Cli::try_parse_from(["rustcode", "export", "abc", "--out", "/tmp"]).unwrap();
        assert!(matches!(
            export.command,
            Some(Commands::Export { id: Some(id), out: Some(_) }) if id == "abc"
        ));
        let export = Cli::try_parse_from(["rustcode", "sessions", "export"]).unwrap();
        assert!(matches!(
            export.command,
            Some(Commands::Sessions {
                command: Some(SessionCommands::Export {
                    id: None,
                    out: None
                })
            })
        ));
        let path = Cli::try_parse_from(["rustcode", "sessions", "path", "abc"]).unwrap();
        assert!(matches!(
            path.command,
            Some(Commands::Sessions {
                command: Some(SessionCommands::Path { id })
            }) if id == "abc"
        ));
    }

    #[test]
    fn parses_storage_status() {
        let cli = Cli::try_parse_from(["rustcode", "storage", "status", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Storage {
                command: StorageCommands::Status { json: true }
            })
        ));
    }

    #[test]
    fn parses_storage_clean() {
        let cli = Cli::try_parse_from(["rustcode", "storage", "clean", "--apply"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Storage {
                command: StorageCommands::Clean {
                    apply: true,
                    json: false
                }
            })
        ));
    }

    #[test]
    fn parses_storage_usage() {
        let cli = Cli::try_parse_from(["rustcode", "storage", "usage"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Storage {
                command: StorageCommands::Usage { json: false }
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
                    prefer_interface: None,
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
        for (alias, want) in [
            ("on", "Start"),
            ("off", "Stop"),
            ("start", "Start"),
            ("stop", "Stop"),
        ] {
            let cli = Cli::try_parse_from(["rustcode", "remote", alias]).unwrap();
            assert!(
                matches!(&cli.command, Some(Commands::Remote { command }) if format!("{command:?}").starts_with(want)),
                "unexpected parse for remote {alias}: {cli:?}"
            );
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
    fn parses_machine_commands() {
        let machines = |args: &[&str]| {
            let mut all = vec!["rustcode", "machines"];
            all.extend_from_slice(args);
            match Cli::try_parse_from(all).map(|cli| cli.command) {
                Ok(Some(Commands::Machines { command })) => Ok(command),
                Ok(other) => panic!("expected a machines command, got {other:?}"),
                Err(error) => Err(error.to_string()),
            }
        };
        assert!(matches!(
            machines(&["list"]).unwrap(),
            MachineCommands::List {
                threads: false,
                json: false
            }
        ));
        assert!(matches!(
            machines(&["list", "--threads", "--json"]).unwrap(),
            MachineCommands::List {
                threads: true,
                json: true
            }
        ));
        assert!(matches!(
            machines(&["add", "192.168.1.20:17879", "--name", "Studio", "--code", "4821-9034"])
                .unwrap(),
            MachineCommands::Add {
                ref address,
                ref name,
                ref code,
                discover: false,
                ..
            } if address.as_deref() == Some("192.168.1.20:17879")
                && name.as_deref() == Some("Studio")
                && code.as_deref() == Some("4821-9034")
        ));
        assert!(matches!(
            machines(&["add", "--discover"]).unwrap(),
            MachineCommands::Add {
                discover: true,
                address: None,
                ..
            }
        ));
        // An address and --discover are two ways to name the same machine.
        assert!(machines(&["add", "10.0.0.2", "--discover"]).is_err());
        assert!(matches!(
            machines(&["remove", "Studio"]).unwrap(),
            MachineCommands::Remove { ref machine } if machine == "Studio"
        ));
        assert!(machines(&["discover", "--json", "--secs", "2"]).is_ok());
        assert!(machines(&["remove"]).is_err());
        assert!(machines(&[]).is_err());
        // `machine` is the singular alias, and the subcommand keeps its name.
        assert!(matches!(
            Cli::try_parse_from(["rustcode", "machine", "list"])
                .unwrap()
                .command,
            Some(Commands::Machines { .. })
        ));
    }

    #[test]
    fn parses_server_commands() {
        let server = |args: &[&str]| {
            let mut all = vec!["rustcode", "server"];
            all.extend_from_slice(args);
            match Cli::try_parse_from(all).unwrap().command {
                Some(Commands::Server { command }) => command,
                other => panic!("expected a server command, got {other:?}"),
            }
        };
        assert!(matches!(
            server(&["run"]),
            ServerCommands::Run { service: false }
        ));
        assert!(matches!(
            server(&["run", "--service"]),
            ServerCommands::Run { service: true }
        ));
        assert!(matches!(server(&["connect"]), ServerCommands::Connect));
        assert!(matches!(
            server(&["install"]),
            ServerCommands::Install {
                force_dev: false,
                yes: false
            }
        ));
        assert!(matches!(
            server(&["install", "--force-dev"]),
            ServerCommands::Install {
                force_dev: true,
                yes: false
            }
        ));
        assert!(matches!(
            server(&["status", "--json"]),
            ServerCommands::Status { json: true }
        ));
        assert!(matches!(
            server(&["restart", "--when-idle"]),
            ServerCommands::Restart {
                when_idle: true,
                yes: false,
                detached: false
            }
        ));
        assert!(matches!(
            server(&["restart", "-y"]),
            ServerCommands::Restart {
                when_idle: false,
                yes: true,
                detached: false
            }
        ));
        assert!(matches!(
            server(&["restart", "--yes", "--when-idle", "--detached"]),
            ServerCommands::Restart {
                when_idle: true,
                yes: true,
                detached: true
            }
        ));
        assert!(matches!(server(&["link-path"]), ServerCommands::LinkPath));
        assert!(matches!(
            server(&["update-cli"]),
            ServerCommands::UpdateCli { json: false }
        ));
        assert!(matches!(
            server(&["update-cli", "--json"]),
            ServerCommands::UpdateCli { json: true }
        ));
        assert!(matches!(
            server(&["uninstall"]),
            ServerCommands::Uninstall { yes: false }
        ));
        assert!(matches!(
            server(&["logs"]),
            ServerCommands::Logs {
                follow: false,
                lines: 100
            }
        ));
        assert!(matches!(
            server(&["logs", "-f", "--lines", "20"]),
            ServerCommands::Logs {
                follow: true,
                lines: 20
            }
        ));
        assert!(Cli::try_parse_from(["rustcode", "server"]).is_err());
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

    #[test]
    fn mcp_orchestrator_takes_its_session_socket_and_token() {
        let cli = Cli::try_parse_from([
            "rustcode",
            "mcp-orchestrator",
            "--session",
            "abc",
            "--socket",
            "/tmp/o.sock",
            "--token",
            "t",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::McpOrchestrator { session, socket, token })
                if session == "abc" && socket == std::path::Path::new("/tmp/o.sock") && token.as_deref() == Some("t")
        ));
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
    fn parses_usage_and_data_commands() {
        assert!(matches!(
            Cli::try_parse_from(["rustcode", "usage"]).unwrap().command,
            Some(Commands::Usage {
                since: None,
                json: false
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["rustcode", "usage", "--since", "2026-11-01", "--json"])
                .unwrap()
                .command,
            Some(Commands::Usage { since: Some(day), json: true }) if day == "2026-11-01"
        ));
        assert!(matches!(
            Cli::try_parse_from(["rustcode", "data", "schema"])
                .unwrap()
                .command,
            Some(Commands::Data {
                command: DataCommands::Schema
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["rustcode", "data", "query", "SELECT 1", "--json"])
                .unwrap()
                .command,
            Some(Commands::Data {
                command: DataCommands::Query { sql, json: true }
            }) if sql == "SELECT 1"
        ));
        assert!(
            Cli::try_parse_from(["rustcode", "data", "query"]).is_err(),
            "the statement is required"
        );
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

#[cfg(test)]
mod remote_device_grant_tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn parses_device_grant_commands() {
        for action in ["allow", "deny"] {
            assert!(Cli::try_parse_from(["rustcode", "remote", "devices", action, "abcd"]).is_ok());
        }
    }
}
