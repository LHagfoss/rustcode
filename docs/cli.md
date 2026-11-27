# Command line

RustCode ships two separate binaries:

- **`rc`** (`rustcode/cli`) — the orchestrator CLI and desktop app sidecar (threads, config, harnesses, machines, remote access, background server, usage/data queries, MCP bridges).
- **`rustcode`** (`harness/`) — the standalone terminal coding agent harness (interactive TUI, `--resume`, `--continue`, `-p`, `--loop`, `--fullscreen`).

Every `rc` command, what it does, and what it changes is listed below. Each command takes
`--help`. Commands that manage RustCode itself (#3821) go through the running
server when there is one (`rc server`), so the app hears of the change at
once, and work on the files directly when none runs; the **Server** column
says where a command needs the server. Commands marked **asks** confirm on a
terminal and need `--yes` (`-y`) in a script. `--json` prints stable JSON on
the commands that list or read.

No command prints a credential: accounts show their sign-in state, MCP servers
the names of their headers and environment variables, never the values. The
one exception is `outside create`, whose token is the point of the command and
is shown once.

`cli_doc_lists_every_command` in `rustcode/cli/src/cli.rs` fails when a
command is added without a row here.

`rc --help` lists the commands in the groups below: Orchestrator,
Threads, Config, Machines & remote, and Server (`HELP_GROUPS` in the same
file). What RustCode is set up with is under `rc config`: settings,
harness, account, connections, mcp, skill, theme, defaults and sync. Each
still works under its old top-level name (`rc mcp list` is
`rc config mcp list`), so scripts written against those keep working.

## Running RustCode

| Command | What it does | Changes |
| --- | --- | --- |
| `rustcode` | The terminal app (`--resume`, `--continue`, `-p` for one prompt) | Threads |
| `rustcode init` | Create a project `.rustcode/config.toml` from the global defaults | Project config |
| `rustcode acp` | Run as an ACP server over stdio (what an editor starts) | — |
| `rc up` | Make this computer pairable in one step: background server from login, remote access on, a connect code | Service, remote access |
| `rustcode doctor` | Check config, binaries, harnesses (install, version, sign-in, named accounts) and skills | Creates missing folders with `--fix` |
| `rustcode bench` | Inspect or compare structured turn telemetry | — |
| `rc export [<id>]` | Export a thread as a zip (same as `threads export`) | A zip file |
| `rustcode discord` | Discord Rich Presence setup and status | Config |

## Server, daemon and machines

| Command | What it does | Changes |
| --- | --- | --- |
| `rc server run\|connect` | Run the server, or relay stdio to it (starting it) | — |
| `rc server install\|uninstall\|restart` | Manage the background service | Service (**asks** on restart) |
| `rc server status\|logs\|link-path\|update-cli` | Read the server's state, log, CLI link and update | `update-cli` replaces the CLI |
| `rustcode serve` | Serve sessions over TCP for remote frontends (experimental) | — |
| `rc daemon start\|run\|stop\|status\|logs` | The scheduler daemon | Daemon process |
| `rc jobs add\|list\|pause\|resume\|run\|stop\|history\|delete` | Jobs in the daemon (aliases: `loops`, `cron`) | Jobs |
| `rc remote …` | The remote gateway: pairing, devices, status | Pairings |
| `rc machines list\|add\|remove\|code\|connect\|add-ssh\|discover` | Machines this one is paired with | Pairings |

## Threads and worktrees

| Command | What it does | Changes | Server |
| --- | --- | --- | --- |
| `rc threads list\|search\|path` | Find threads and their folders | — | |
| `rc threads settle\|unsettle\|review <id>…` | Change a thread's status | Thread status | |
| `rc threads pin\|unpin <id>…` | Pin a thread to the top of the list | Thread status | |
| `rc threads rename <id> <title>` | Give a thread a new title | Thread title | |
| `rc threads move <id> --harness <h> [--account] [--model]` | Continue a thread on another harness, account or model | The thread's harness | Needed |
| `rc threads configure <id> [--model] [--effort] [--json]` | Show a thread's model and effort, or change them on its own harness; a value its harness does not list is refused | The thread's model and effort | Needed to change |
| `rc threads export\|migrate` | Export a thread as a zip, or move legacy threads | Files | |
| `rc worktree list\|status` | RustCode's worktrees of a repository | — | |
| `rc worktree remove <path> [--force]` | Remove one; uncommitted or unpushed work is kept unless `--force` (**asks**) | The worktree | |
| `rc worktree prune` | `git worktree prune`: records of worktrees whose folders are gone | Git's records | |

## Config: settings and what threads can use

| Command | What it does | Changes | Server |
| --- | --- | --- | --- |
| `rc config settings list\|get [--project]` | Server settings with value, source, default and accepted values | — | |
| `rc config settings set <key> <value> [--project]` | Change one; prints `before → after` | `settings.json` | |
| `rc config settings reset <key> [--project]` | Back to the default (or the machine's value) | `settings.json` | |
| `rc config skill list\|enable\|disable` | Skills offered to threads | `config.toml` | |
| `rc config mcp add\|list\|remove` | MCP servers in the user config | `config.toml` | |
| `rc config mcp enable\|disable` | Turn one on or off; the server starts or stops it | `config.toml` | |
| `rc config mcp restart <name>` | Reconnect one | — | Needed |
| `rc config harness list` | Harnesses, sign-in, accounts and the default | — | |
| `rc config harness default <id> [--model] [--effort]` | The harness new threads start on | `config.toml` | |
| `rc config theme list\|show\|new\|set\|validate\|path` | Themes for the terminal and the app | Theme files, `config.toml` | |
| `rc config defaults` | Show or set the model profile and effort new sessions start with | `config.toml` | |
| `rc config sync pull\|push\|init` | Sync config, skills and themes with a Git remote; paired machines can follow this one's settings instead (Settings › Machines) | Config files | |

Old names: `rc settings`, `rc skill`, `rc mcp`,
`rc harness`, `rc theme`, `rc defaults` and `rc sync`
are the same commands.

## Accounts, sign-ins and outside agents

Not available to agents: an agent never adds, signs out or mints a credential.
`rc account` and `rc connections` are the same as
`rc config account` and `rc config connections`.

| Command | What it does | Changes | Server |
| --- | --- | --- | --- |
| `rc config account list\|check` | Named harness accounts and their sign-in state | — | |
| `rc config account add <harness> <id> [--name] [--config-dir]` | Register a named login in its own folder | `config.toml` | |
| `rc config account remove <harness> <id>` | Forget it; its folder and login stay (**asks**). Refused while a thread runs on it | `config.toml` | |
| `rc config connections list` | GitHub CLI accounts and saved model sign-ins | — | |
| `rc config connections logout github <login>` / `<provider> <account>` | Sign one out (**asks**) | The sign-in | |
| `rc outside list` | Connected outside agents and their access | — | |
| `rc outside create <name> [--platform] [--access] [--ask]` | Connect an app; prints its token and snippet once (**asks** above read-only) | `outside-agents.json` | |
| `rc outside update <id> [--access] [--ask]` | Change its access or whether it asks | `outside-agents.json` | |
| `rc outside code [--access] [--ask]` | A one-time code (10 minutes) that approves one sign-in on the public sign-in page, at most at `--access` (`docs/mcp-outside-agents.md` › Hosted agents) | `outside-codes.json` | |
| `rc outside revoke <id>` | Its token stops working at once (**asks**) | `outside-agents.json` | |

## Data and storage

| Command | What it does | Changes |
| --- | --- | --- |
| `rc usage [--since] [--json]` | Tokens per harness and model, as each reported | — |
| `rc data query\|schema\|stats` | The local data store, read-only SQL | — |
| `rc storage status\|usage\|clean` | User-data migrations, disk use, and unreferenced command output | `clean --apply` moves files aside, restorable for 14 days |

## Started by other programs

| Command | Started by |
| --- | --- |
| `rc mcp-orchestrator` | The engine, for an external harness thread |
| `rc mcp-connect` | An outside agent's app, from its snippet |
| `rc guard-command` | Claude Code's `PreToolUse` hook |

## For agents

An agent manages the same things through the `rustcode` MCP server:
`manage_settings` (settings) and `manage_capabilities` (skills, MCP servers,
the default harness, the theme) answer reads at once and show the thread's
approval card, with the value before and after, for every change;
`transfer_session` moves a thread to another harness, account and model. See
[configuration.md](configuration.md) › Server settings across machines and
[mcp-outside-agents.md](mcp-outside-agents.md) for the Admin level an outside
agent needs to change anything.

### From a thread's shell: `rc tools`, `rc secret`, `rc terminal`

Every listed MCP tool is sent with each model request, so a thread also
reaches its tools from its shell, where one costs nothing until it is run.
The engine starts each external harness with `RUSTCODE_THREAD_ID`,
`RUSTCODE_ORCHESTRATOR_SOCKET` and `RUSTCODE_THREAD_CREDENTIAL` (the token the
thread's `rc mcp-orchestrator` already holds, under a name Codex does not
strip from its shells). A command the agent runs inherits them; outside a
thread these commands say so and exit 1.

| Command | What it does |
| --- | --- |
| `rc tools list` | Every tool the thread can call, the ones its MCP list leaves out (`orchestration-all-tools`) included; `--json` for the MCP entries |
| `rc tools show <name>` | A tool's description and its JSON arguments |
| `rc tools call <name> '<json>'` | Calls it through the same dispatch as an MCP `tools/call`: the same checks, approval cards and private forms. `-` reads the arguments from stdin; `--json` prints `{"success", "content"}`. Exits 1 on a failed call |
| `rc secret request NAME… --reason <why> --file <.env>` | One private form per name (`request_secret`); the value goes to the dotenv file and never reaches the agent. An existing assignment is replaced where it stands, a new one is appended, other lines stay; the answer says added or updated. Stops at the first decline, exit 2 |
| `rc terminal start <name> [--cwd DIR] -- <command>` | Runs a long-lived command (a dev server, a watcher) in its own process group, so it outlives the shell call; one argument is a shell line, several are quoted as words |
| `rc terminal output <name> [--since N] [--tail BYTES]` | Its output, the last 16 KiB by default; prints the offset to pass as `--since` next |
| `rc terminal wait <name> --contains <text> [--timeout S]` | Waits for new output with the text: exit 0 found, 1 the command ended first, 2 timed out |
| `rc terminal send <name> --text <t> [--enter]` · `list [--all]` · `stop <name>` | Types into its input, lists this thread's terminals, or stops one with everything it started |

A terminal's files are under `<config>/run/terminals/<thread>/<name>/`
(`output.log`, a named pipe for input, its `exit` status). Its input is a pipe,
not a TTY, so a program that needs a terminal to prompt will not prompt.
