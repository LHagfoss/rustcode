use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "rc",
    version = env!("CARGO_PKG_VERSION"),
    about = "RustCode CLI: threads, config, harnesses, machines, remote access, and server"
)]
pub struct Cli {
    /// Check for and install the latest GitHub Release, using Homebrew for Homebrew installs
    #[arg(long = "update", alias = "upgrade")]
    pub update: bool,

    /// Run as a headless Agent Client Protocol server over stdio
    #[arg(long = "acp")]
    pub acp: bool,

    /// Automatically approve tool confirmations for this run
    #[arg(long = "yolo")]
    pub yolo: bool,

    /// Include threads from all workspaces (`threads list` and `threads search`)
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

    /// Sync config, skills, and themes with remote Git repository; the same
    /// as `config sync`
    #[command(hide = true)]
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

    /// What RustCode is set up with: settings, harnesses, accounts, MCP
    /// servers, skills, themes, defaults and Git sync
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
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

    /// List skills and turn one on or off for every project; the same as
    /// `config skill`
    #[command(alias = "skills", hide = true)]
    Skill {
        #[command(subcommand)]
        command: ToggleCommands,
    },

    /// List the harnesses and choose the one new threads start on; the same
    /// as `config harness`
    #[command(alias = "harnesses", hide = true)]
    Harness {
        #[command(subcommand)]
        command: HarnessCommands,
    },

    /// List, check, add and remove the named accounts of a harness (sign-ins
    /// are never printed); the same as `config account`
    #[command(alias = "accounts", hide = true)]
    Account {
        #[command(subcommand)]
        command: AccountCommands,
    },

    /// List the GitHub and model sign-ins, or sign one out; the same as
    /// `config connections`
    #[command(alias = "connection", hide = true)]
    Connections {
        #[command(subcommand)]
        command: ConnectionCommands,
    },

    /// Connect, change and revoke outside agents (Claude Desktop, Cursor, …)
    Outside {
        #[command(subcommand)]
        command: OutsideCommands,
    },

    /// List RustCode's worktrees of a repository, read one, remove one, or
    /// prune the records of worktrees whose folders are gone
    #[command(alias = "worktrees")]
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommands,
    },

    /// Read and change RustCode's server settings (`settings.json`), the
    /// values Settings in the app shows; the same as `config settings`
    #[command(alias = "setting", hide = true)]
    Settings {
        #[command(subcommand)]
        command: SettingsCommands,
    },

    /// List, show, create, validate and set themes; the same as `config theme`
    #[command(hide = true)]
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
        /// Report linked pull requests per harness and model instead: merged
        /// share, CI failures, and cost and tokens per merged pull request
        #[arg(long, conflicts_with = "since")]
        pull_requests: bool,
        /// With --pull-requests: add a cost estimate from this TOML price
        /// file (`source`, `as_of`, USD per 1M tokens per model), printed
        /// apart from the cost the harness reported
        #[arg(long, value_name = "FILE", requires = "pull_requests")]
        prices: Option<std::path::PathBuf>,
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
    /// with; the same as `config defaults`
    #[command(hide = true)]
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

    /// Make this computer one another can pair with, in one step: run the
    /// server in the background from login, switch remote access on and
    /// print a connect code
    #[command(after_help = "Examples:
  rc up                    # on a headless machine; type the code on the other one
  rc up --yes --json       # for a script (`rc machines add-ssh` runs this)")]
    Up {
        /// Stop a server with turns running, to replace it with the service,
        /// without asking
        #[arg(short, long)]
        yes: bool,
        /// Print the result as JSON (progress goes to standard error)
        #[arg(long)]
        json: bool,
        /// Run the server in the background until the computer restarts,
        /// instead of installing it as a login service
        #[arg(long)]
        no_service: bool,
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

    /// Manage jobs through the running daemon
    #[command(alias = "job", alias = "loops", alias = "loop", alias = "cron")]
    Jobs {
        #[command(subcommand)]
        command: CronCommands,
    },

    /// Add, list, remove, turn on or off, and restart MCP servers; the same
    /// as `config mcp`
    #[command(hide = true)]
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

    /// Stdio MCP server that lets another app (Claude Desktop, Cursor, Codex)
    /// use RustCode's threads with the scoped token made in Settings ›
    /// Outside agents; the app starts it from the snippet RustCode shows
    #[command(name = "mcp-connect")]
    McpConnect {
        /// The token RustCode showed when the app was connected. Normally set
        /// in `RUSTCODE_MCP_TOKEN` instead, so it is not visible in `ps`.
        #[arg(long)]
        token: Option<String>,
    },

    /// Claude Code `PreToolUse` hook that applies the project's command
    /// policy (`.rustcode/command-policy.toml`) to a Bash call read from
    /// stdin; started by Claude Code, not by hand
    #[command(name = "guard-command", hide = true)]
    GuardCommand,

    #[command(flatten)]
    Agent(crate::agent_command::AgentCommands),
}

/// `rc config …`: the commands that change what RustCode is set up
/// with, in one place. Each is also accepted under its old top-level name
/// (`rc sync`, `rc mcp`, …), so nothing written against those
/// breaks; [`Cli::flatten_config`] turns one into the other.
#[derive(clap::Subcommand, Debug)]
pub enum ConfigCommands {
    /// Read and change RustCode's server settings (`settings.json`), the
    /// values Settings in the app shows; writes go through the running server
    #[command(alias = "setting")]
    Settings {
        #[command(subcommand)]
        command: SettingsCommands,
    },
    /// List the harnesses and choose the one new threads start on
    #[command(alias = "harnesses")]
    Harness {
        #[command(subcommand)]
        command: HarnessCommands,
    },
    /// List, check, add and remove the named accounts of a harness (sign-ins
    /// are never printed)
    #[command(alias = "accounts")]
    Account {
        #[command(subcommand)]
        command: AccountCommands,
    },
    /// List the GitHub and model sign-ins, or sign one out
    #[command(alias = "connection")]
    Connections {
        #[command(subcommand)]
        command: ConnectionCommands,
    },
    /// Add, list, remove, turn on or off, and restart MCP servers
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },
    /// List skills and turn one on or off for every project
    #[command(alias = "skills")]
    Skill {
        #[command(subcommand)]
        command: ToggleCommands,
    },
    /// List, show, create, validate and set themes (terminal and desktop app)
    Theme {
        #[command(subcommand)]
        command: ThemeCommands,
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
    /// Sync config, skills, and themes with a remote Git repository (paired
    /// machines can follow this one's settings instead: Settings › Machines)
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
}

impl From<ConfigCommands> for Commands {
    fn from(command: ConfigCommands) -> Self {
        match command {
            ConfigCommands::Settings { command } => Commands::Settings { command },
            ConfigCommands::Harness { command } => Commands::Harness { command },
            ConfigCommands::Account { command } => Commands::Account { command },
            ConfigCommands::Connections { command } => Commands::Connections { command },
            ConfigCommands::Mcp { command } => Commands::Mcp { command },
            ConfigCommands::Skill { command } => Commands::Skill { command },
            ConfigCommands::Theme { command } => Commands::Theme { command },
            ConfigCommands::Defaults { model, effort } => Commands::Defaults { model, effort },
            ConfigCommands::Sync {
                pull,
                push,
                command,
            } => Commands::Sync {
                pull,
                push,
                command,
            },
        }
    }
}

impl Cli {
    /// `config <command>` as the top-level command it stands for, so each is
    /// handled in one place.
    pub(crate) fn flatten_config(mut self) -> Self {
        self.command = self.command.map(|command| match command {
            Commands::Config { command } => command.into(),
            other => other,
        });
        self
    }
}

/// The sections `rustcode --help` lists commands under, in order. Every
/// visible command is in exactly one (`help_groups_cover_every_command`);
/// `config` lists its own commands.
const HELP_GROUPS: &[(&str, &[&str])] = &[
    ("General", &["init", "acp", "doctor", "bench", "discord"]),
    (
        "Threads",
        &["threads", "export", "worktree", "usage", "data", "storage"],
    ),
    ("Config", &["config"]),
    ("In a thread", &["tools", "secret", "terminal"]),
    (
        "Machines & remote",
        &[
            "up",
            "machines",
            "remote",
            "outside",
            "serve",
            "mcp-connect",
        ],
    ),
    ("Server", &["server", "daemon", "jobs"]),
];

/// The command lines of `rustcode --help`, grouped by [`HELP_GROUPS`].
fn grouped_commands(root: &clap::Command) -> String {
    let styles = root.get_styles();
    let (header, literal) = (styles.get_header(), styles.get_literal());
    let mut rows: Vec<(Option<&str>, String, String)> = Vec::new();
    for (group, names) in HELP_GROUPS {
        rows.push((Some(group), String::new(), String::new()));
        for name in *names {
            let Some(command) = root.find_subcommand(name) else {
                continue;
            };
            if *name == "config" {
                for sub in command.get_subcommands() {
                    let about = sub.get_about().map(ToString::to_string).unwrap_or_default();
                    rows.push((None, format!("config {}", sub.get_name()), about));
                }
            } else {
                let about = command
                    .get_about()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                rows.push((None, (*name).to_owned(), about));
            }
        }
    }
    let width = rows
        .iter()
        .map(|(_, name, _)| name.len())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for (group, name, about) in rows {
        if let Some(group) = group {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("{header}{group}:{header:#}\n"));
        } else {
            let template_safe_about = about.replace('{', "(").replace('}', ")");
            out.push_str(&format!(
                "  {literal}{name:<width$}{literal:#}  {template_safe_about}\n"
            ));
        }
    }
    out
}

/// The `rustcode` command, with its commands listed in groups.
pub fn command() -> clap::Command {
    let root = <Cli as clap::CommandFactory>::command();
    let groups = grouped_commands(&root);
    let header = *root.get_styles().get_header();
    root.help_template(format!(
        "{{about-with-newline}}\n{{usage-heading}} {{usage}}\n\n{groups}\n\
         {header}Options:{header:#}\n{{options}}{{after-help}}"
    ))
}

/// Parse the command line, with `config <command>` already flattened.
pub fn parse() -> Cli {
    let matches = command().get_matches();
    <Cli as clap::FromArgMatches>::from_arg_matches(&matches)
        .unwrap_or_else(|error| error.format(&mut command()).exit())
        .flatten_config()
}

#[derive(clap::Subcommand, Debug)]
pub enum McpCommands {
    /// Add an MCP server: a URL for a remote server, or a command to spawn
    #[command(after_help = "Examples:
  rc mcp add --transport http api https://mcp.example.com/mcp --header \"Authorization: Bearer <token>\"
  rc mcp add --env API_KEY=<key> files -- npx -y @example/files-mcp")]
    Add(McpAddArgs),
    /// List configured MCP servers (header and environment values are hidden)
    List,
    /// Remove an MCP server from the user config
    #[command(alias = "rm")]
    Remove { name: String },
    /// Turn a configured MCP server on (the running server starts it)
    Enable { name: String },
    /// Turn a configured MCP server off (the running server stops it)
    Disable { name: String },
    /// Reconnect an MCP server in the running RustCode server
    Restart { name: String },
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
    Connect {
        /// Run a standalone engine instead when the server is another
        /// version or cannot be reached (the desktop app's engine)
        #[arg(long, hide = true)]
        or_standalone: bool,
    },
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
    /// Point a service an older release installed (`rc server run`)
    /// at this rc, and restart the server onto it
    Repair {
        /// Restart even if turns are running, instead of once they finish
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
    /// Run a blind relay in the foreground (on a VPS) so machines and paired
    /// devices on different networks can reach each other; it prints the
    /// link a machine registers with
    Relay {
        /// Address to listen on
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        /// TCP port of the relay
        #[arg(long, default_value_t = rustcode::remote_relay::DEFAULT_PORT)]
        port: u16,
        /// Public name or IP machines and devices should dial, when it differs from --bind
        #[arg(long)]
        advertise: Option<String>,
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
    /// (the request ids are in `rc remote status`)
    Accept {
        /// Request id from `rc remote status`
        id: String,
    },
    /// Deny a device's request to connect
    Deny {
        /// Request id from `rc remote status`
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
    /// Forget a machine and its token, and end the access both ways
    Remove {
        /// Name, host id or a prefix of one
        machine: String,
        /// Do not ask the machine; it keeps listing this computer
        #[arg(long)]
        local: bool,
    },
    /// For a lost or stolen computer: end its pairings on this computer and on
    /// every saved machine that answers, then forget it here
    RevokeEverywhere {
        /// Name, host id or a prefix of one
        machine: String,
    },
    /// Create a connect code here, for another computer to pair with this
    /// one: valid ten minutes, once, no accept step. Anyone who has it can see
    /// and run all threads on this computer.
    #[command(after_help = "Examples:
  rc machines code                 # show the code and the link
  rc machines code --link          # print only the link, for a script
  rc machines connect \"$(ssh box rc machines code --link)\"")]
    Code {
        /// Print only the long form (`rustcode://connect/…`), for a script
        /// or `ssh <host> rc machines code --link`
        #[arg(long, conflicts_with_all = ["json", "cancel"])]
        link: bool,
        /// Print the code as JSON
        #[arg(long, conflicts_with = "cancel")]
        json: bool,
        /// Withdraw the open connect code instead
        #[arg(long)]
        cancel: bool,
    },
    /// Pair with a connect code created on another computer and save it
    Connect {
        /// The code (`K7MQ-29XH-TP4W-NRD6`) or link (`rustcode://connect/…`)
        code: String,
        /// Where to reach that computer, when the code carries no address or
        /// to use another one than it carries
        #[arg(long)]
        address: Option<String>,
        /// Name to save the machine under (default: the name it reports)
        #[arg(long)]
        name: Option<String>,
    },
    /// Pair with a computer this one reaches over SSH, in one step: install
    /// RustCode there at this version if it is missing or older, start its
    /// server with `rc up`, and pair with the connect code it prints.
    /// The code only travels inside SSH.
    #[command(
        name = "add-ssh",
        after_help = "Examples:
  rc machines add-ssh lars@box
  rc machines add-ssh studio --yes          # an alias from ~/.ssh/config; install without asking
  rc machines add-ssh admin@vps -p 2222 -i ~/.ssh/vps_ed25519
  rc machines add-ssh lars@box --mutual     # that computer can reach this one too

SSH runs non-interactively (BatchMode): a key that needs a passphrase must be in
the agent (`ssh-add`), and an unknown host key must be accepted once in a terminal."
    )]
    AddSsh {
        /// `user@host`, or a `Host` alias from `~/.ssh/config`
        target: String,
        /// SSH port, when not the one the SSH config gives
        #[arg(short, long)]
        port: Option<u16>,
        /// Private key to sign in with, when not the agent's or the config's
        #[arg(short, long)]
        identity: Option<std::path::PathBuf>,
        /// Name to save the machine under (default: the name it reports)
        #[arg(long)]
        name: Option<String>,
        /// Install or update RustCode there without asking
        #[arg(short, long, conflicts_with = "no_install")]
        yes: bool,
        /// Never install: fail when RustCode there is missing or older
        #[arg(long)]
        no_install: bool,
        /// Both ways: after pairing, that computer pairs back, so it can see
        /// and run the threads on this one too. Each side keeps its own
        /// pairing and revokes it alone
        #[arg(long, conflicts_with = "one_way")]
        mutual: bool,
        /// One way only: this computer reaches that one and not the other way
        /// round (the default when there is no terminal to ask at)
        #[arg(long)]
        one_way: bool,
        /// Print each step and the result as JSON lines, for the app and agents
        #[arg(long)]
        json: bool,
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
    /// Make this machine the fleet's hub, or join or leave one. A hub is an
    /// always-on machine (a VPS, a desktop that stays on) that outside agents
    /// reach the other machines through; they keep an outbound connection
    /// to it, so they need no open port
    Hub {
        #[command(subcommand)]
        command: HubCommands,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum HubCommands {
    /// Make this machine the fleet's hub: machines that join it keep a link
    /// to its remote gateway
    On,
    /// Stop being the hub; linked machines are let go
    Off,
    /// Join a saved machine that is the hub, granting it an access ceiling
    #[command(after_help = "Examples:
  rc machines hub join vps --access delegate
  rc machines hub join vps --access read-only --ask false")]
    Join {
        /// The hub: name, host id or a prefix of one, from `rc machines list`
        machine: String,
        /// The most the hub may do here, at the outside-agent levels:
        /// read-only, delegate, manage or admin
        #[arg(long, default_value = "admin")]
        access: String,
        /// Ask in the app here before each call from the hub that changes
        /// something
        #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
        ask: bool,
    },
    /// Leave the hub; the hub cannot reach this machine any more
    Leave,
    /// Show this machine's role and, on the hub, its members
    Status {
        /// Print the status as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum RemoteDeviceCommands {
    /// Allow ACP access: `read` lists and reads threads, `threads` also
    /// starts and drives them, `full` (the default) can also change this
    /// machine's settings, skills, accounts and jobs and run commands here
    Allow {
        device: String,
        #[arg(long, default_value = "full", value_parser = ["read", "threads", "full"])]
        access: String,
    },
    /// Deny ACP access and disconnect the device immediately
    Deny { device: String },
}

#[derive(clap::Subcommand, Debug)]
pub enum CronCommands {
    /// Add a job
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
    /// List jobs
    List {
        #[arg(long)]
        json: bool,
    },
    /// Pause a job
    Pause {
        job_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Resume a job
    Resume {
        job_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Run a job now
    Run {
        job_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop an active run of a job
    Stop {
        job_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Show recent runs for a job
    History {
        job_id: String,
        #[arg(long, default_value_t = 20, value_parser = parse_history_limit)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Delete a job
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
    /// Move a thread to another harness, account or model (needs the
    /// running server, which runs the thread)
    Move {
        /// Thread id, or a unique prefix of one
        id: String,
        /// The harness to continue on, as `rc harness list` names it
        #[arg(long)]
        harness: String,
        /// A named account of that harness
        #[arg(long)]
        account: Option<String>,
        /// The model to continue with
        #[arg(long)]
        model: Option<String>,
    },
    /// Show a thread's model and effort, or change them on its harness
    /// (a change needs the running server, which runs the thread)
    Configure {
        /// Thread id, or a unique prefix of one
        id: String,
        /// The model to continue with, as the thread's model picker lists it
        #[arg(long)]
        model: Option<String>,
        /// The reasoning effort to continue with
        #[arg(long)]
        effort: Option<String>,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Give a thread a new title
    Rename {
        /// Thread id, or a unique prefix of one
        id: String,
        title: String,
    },
    /// Pin threads to the top of the app's list
    Pin {
        /// Thread ids, or unique prefixes of them
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Unpin threads
    Unpin {
        #[arg(required = true)]
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
pub enum ToggleCommands {
    /// Every skill, on or off, and where it was found
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        json: bool,
    },
    /// Offer a skill to threads again
    Enable { name: String },
    /// Stop offering a skill to threads
    Disable { name: String },
}

#[derive(clap::Subcommand, Debug)]
pub enum HarnessCommands {
    /// Every harness: installed, signed in, its named accounts, and the default
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        json: bool,
    },
    /// Make a harness the default for new threads
    Default {
        /// The harness id, as `list` names it (claude-code, codex, rustcode, …)
        harness: String,
        /// The model new threads on it start with
        #[arg(long)]
        model: Option<String>,
        /// The reasoning effort they start with
        #[arg(long)]
        effort: Option<String>,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum AccountCommands {
    /// Every named account: its harness, folder and sign-in state
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        json: bool,
    },
    /// Look every account's sign-in up again; nothing is started
    Check {
        #[arg(long)]
        json: bool,
    },
    /// Register a named login of a harness, kept in its own config folder
    Add {
        /// claude-code or codex
        harness: String,
        /// A short id, e.g. work
        id: String,
        /// The name the app shows
        #[arg(long)]
        name: Option<String>,
        /// The config folder (default: the harness's, with the id appended)
        #[arg(long)]
        config_dir: Option<String>,
    },
    /// Forget an account; its folder and the login in it stay
    #[command(alias = "rm")]
    Remove {
        harness: String,
        id: String,
        /// Confirm without asking
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum ConnectionCommands {
    /// The GitHub CLI's accounts and the saved model sign-ins
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        json: bool,
    },
    /// Sign one out: `github <login>` or `<provider> <account>`
    Logout {
        /// `github`, or a model provider such as openai
        provider: String,
        /// The GitHub login, or the provider's account name
        account: String,
        /// The GitHub host
        #[arg(long, default_value = "github.com")]
        host: String,
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum OutsideCommands {
    /// The connected apps, their access and when each last called
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        json: bool,
    },
    /// Connect an app; prints its token and config snippet once
    Create {
        /// The name RustCode shows for it
        name: String,
        /// claude-desktop, cursor, codex or other
        #[arg(long, default_value = "other")]
        platform: String,
        /// read_only (default), delegate, manage or admin
        #[arg(long)]
        access: Option<String>,
        /// Ask in the app before each change (default: on above read_only)
        #[arg(long)]
        ask: Option<bool>,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Change an app's access, or whether it asks first
    Update {
        id: String,
        #[arg(long)]
        access: Option<String>,
        #[arg(long)]
        ask: Option<bool>,
    },
    /// A one-time code that approves one sign-in on this machine's public
    /// sign-in page (`[outside] public_url`, for ChatGPT, Claude and other
    /// hosted agents). It works once, for 10 minutes
    Code {
        /// The most the app may do: read_only (default), delegate, manage or admin
        #[arg(long)]
        access: Option<String>,
        /// Ask in the app before each change (default: on above read_only)
        #[arg(long)]
        ask: Option<bool>,
    },
    /// Revoke an app's token; it stops working at once
    Revoke {
        id: String,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// The machine owner's passkeys, which approve a sign-in on the public
    /// sign-in page without a code
    Owner {
        #[command(subcommand)]
        command: OwnerCommands,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum OwnerCommands {
    /// Print a link that adds a passkey on the public sign-in page. Open it
    /// on the device that keeps the passkey; it works once, for 10 minutes
    AddPasskey {
        /// What RustCode calls the passkey, such as the device it is on
        #[arg(long, default_value = "Passkey")]
        name: String,
    },
    /// List the owner's passkeys
    Passkeys {
        /// Print them as JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove a passkey by its id, a prefix of it, or its name; it stops
    /// approving sign-ins at once, and the browsers it signed in to the web
    /// app are signed out
    RemovePasskey { passkey: String },
    /// List the browsers signed in to the web app with a passkey
    Sessions {
        /// Print them as JSON
        #[arg(long)]
        json: bool,
    },
    /// Sign out a browser by its session id, a prefix of it, or `all`
    SignOut { session: String },
}

#[derive(clap::Subcommand, Debug)]
pub enum WorktreeCommands {
    /// The worktrees RustCode made for a repository (default: this folder's)
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        repo: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// A worktree's branch, how far it is from what it tracks, its changed
    /// files and last commit
    Status { path: std::path::PathBuf },
    /// Remove a worktree RustCode made; one with uncommitted or unpushed work
    /// is kept unless `--force`
    #[command(alias = "rm")]
    Remove {
        path: std::path::PathBuf,
        /// Remove it even with uncommitted or unpushed work, which is lost
        #[arg(long)]
        force: bool,
        /// Confirm `--force` without asking
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Drop git's records of worktrees whose folders are gone; no files change
    Prune {
        #[arg(long)]
        repo: Option<std::path::PathBuf>,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum SettingsCommands {
    /// Every setting with its value, where the value comes from, and its default
    #[command(alias = "ls")]
    List {
        /// A project's folder: the values a thread there gets, with its overrides
        #[arg(long)]
        project: Option<std::path::PathBuf>,
        /// Print the settings as JSON
        #[arg(long)]
        json: bool,
    },
    /// One setting's value, type, default and accepted values
    Get {
        key: String,
        #[arg(long)]
        project: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Change one setting, on this machine or as a project's override
    #[command(after_help = "Examples:
  rc settings set continue-after-limit false
  rc settings set auto-compact-threshold 200000
  rc settings set default-permission-mode edits --project ~/code/app")]
    Set {
        key: String,
        /// The new value: true/false, a number, one of the accepted values, or JSON for a list
        value: String,
        #[arg(long)]
        project: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Remove this machine's value (or the project's override), so the
    /// default (or the machine's value) applies again
    Reset {
        key: String,
        #[arg(long)]
        project: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
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
    /// changing files (`rc usage` reports tokens, not bytes)
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
  rc data query \"SELECT * FROM threads LIMIT 5\"
  rc data query \"SELECT * FROM daily_usage ORDER BY day DESC LIMIT 10\" --json")]
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
    /// Copy a paired machine's skills, themes and prompt commands here once
    From(crate::sync_from::SyncFromArgs),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SyncAction {
    Pull,
    Push,
    PullThenPush,
    Init(String),
    From(crate::sync_from::SyncFromArgs),
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
        Some(SyncCommands::From(args)) => SyncAction::From(args),
        None => SyncAction::PullThenPush,
    })
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;

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
