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
| `rustcode up` | Make this computer pairable in one step: background server from login, remote access on, a connect code | Service, remote access |
| `rustcode doctor` | Check config, binaries and skills | Creates missing folders with `--fix` |
| `rustcode bench` | Inspect or compare structured turn telemetry | — |
| `rustcode export [<id>]` | Export a thread as a zip (same as `threads export`) | A zip file |
| `rustcode discord` | Discord Rich Presence setup and status | Config |

## Server, daemon and machines

| Command | What it does | Changes |
| --- | --- | --- |
| `rustcode server run\|connect` | Run the server, or relay stdio to it (starting it) | — |
| `rustcode server install\|uninstall\|restart` | Manage the background service | Service (**asks** on restart) |
| `rustcode server status\|logs\|link-path\|update-cli` | Read the server's state, log, CLI link and update | `update-cli` replaces the CLI |
| `rustcode serve` | Serve sessions over TCP for remote frontends (experimental) | — |
| `rustcode daemon start\|run\|stop\|status\|logs` | The scheduler daemon | Daemon process |
| `rustcode loops add\|list\|pause\|resume\|run\|history\|delete` | Loops in the daemon (alias: `cron`) | Loops |
| `rustcode remote …` | The remote gateway: pairing, devices, status | Pairings |
| `rustcode machines list\|add\|remove\|code\|connect\|add-ssh\|discover` | Machines this one is paired with | Pairings |

## Threads and worktrees

| Command | What it does | Changes | Server |
| --- | --- | --- | --- |
| `rustcode threads list\|search\|path` | Find threads and their folders | — | |
| `rustcode threads settle\|unsettle\|review <id>…` | Change a thread's status | Thread status | |
| `rustcode threads pin\|unpin <id>…` | Pin a thread to the top of the list | Thread status | |
| `rustcode threads rename <id> <title>` | Give a thread a new title | Thread title | |
| `rustcode threads move <id> --harness <h> [--account] [--model]` | Continue a thread on another harness, account or model | The thread's harness | Needed |
| `rustcode threads export\|migrate` | Export a thread as a zip, or move legacy threads | Files | |
| `rustcode worktree list\|status` | RustCode's worktrees of a repository | — | |
| `rustcode worktree remove <path> [--force]` | Remove one; uncommitted or unpushed work is kept unless `--force` (**asks**) | The worktree | |
| `rustcode worktree prune` | `git worktree prune`: records of worktrees whose folders are gone | Git's records | |

## Config: settings and what threads can use

| Command | What it does | Changes | Server |
| --- | --- | --- | --- |
| `rustcode config settings list\|get [--project]` | Server settings with value, source, default and accepted values | — | |
| `rustcode config settings set <key> <value> [--project]` | Change one; prints `before → after` | `settings.json` | |
| `rustcode config settings reset <key> [--project]` | Back to the default (or the machine's value) | `settings.json` | |
| `rustcode config skill list\|enable\|disable` | Skills offered to threads | `config.toml` | |
| `rustcode config mcp add\|list\|remove` | MCP servers in the user config | `config.toml` | |
| `rustcode config mcp enable\|disable` | Turn one on or off; the server starts or stops it | `config.toml` | |
| `rustcode config mcp restart <name>` | Reconnect one | — | Needed |
| `rustcode config harness list` | Harnesses, sign-in, accounts and the default | — | |
| `rustcode config harness default <id> [--model] [--effort]` | The harness new threads start on | `config.toml` | |
| `rustcode config theme list\|show\|new\|set\|validate\|path` | Themes for the terminal and the app | Theme files, `config.toml` | |
| `rustcode config defaults` | Show or set the model profile and effort new sessions start with | `config.toml` | |
| `rustcode config sync pull\|push\|init` | Sync config, skills and themes with a Git remote; paired machines can follow this one's settings instead (Settings › Machines) | Config files | |

Old names: `rustcode settings`, `rustcode skill`, `rustcode mcp`,
`rustcode harness`, `rustcode theme`, `rustcode defaults` and `rustcode sync`
are the same commands.

## Accounts, sign-ins and outside agents

Not available to agents: an agent never adds, signs out or mints a credential.
`rustcode account` and `rustcode connections` are the same as
`rustcode config account` and `rustcode config connections`.

| Command | What it does | Changes | Server |
| --- | --- | --- | --- |
| `rustcode config account list\|check` | Named harness accounts and their sign-in state | — | |
| `rustcode config account add <harness> <id> [--name] [--config-dir]` | Register a named login in its own folder | `config.toml` | |
| `rustcode config account remove <harness> <id>` | Forget it; its folder and login stay (**asks**). Refused while a thread runs on it | `config.toml` | |
| `rustcode config connections list` | GitHub CLI accounts and saved model sign-ins | — | |
| `rustcode config connections logout github <login>` / `<provider> <account>` | Sign one out (**asks**) | The sign-in | |
| `rustcode outside list` | Connected outside agents and their access | — | |
| `rustcode outside create <name> [--platform] [--access] [--ask]` | Connect an app; prints its token and snippet once (**asks** above read-only) | `outside-agents.json` | |
| `rustcode outside update <id> [--access] [--ask]` | Change its access or whether it asks | `outside-agents.json` | |
| `rustcode outside code [--access] [--ask]` | A one-time code (10 minutes) that approves one sign-in on the public sign-in page, at most at `--access` (`docs/mcp-outside-agents.md` › Hosted agents) | `outside-codes.json` | |
| `rustcode outside revoke <id>` | Its token stops working at once (**asks**) | `outside-agents.json` | |

## Data and storage

| Command | What it does | Changes |
| --- | --- | --- |
| `rustcode usage [--since] [--json]` | Tokens per harness and model, as each reported | — |
| `rustcode data query\|schema\|stats` | The local data store, read-only SQL | — |
| `rustcode storage status\|usage\|clean` | User-data migrations, disk use, and unreferenced command output | `clean --apply` moves files aside, restorable for 14 days |

## Started by other programs

| Command | Started by |
| --- | --- |
| `rustcode mcp-orchestrator` | The engine, for an external harness thread |
| `rustcode mcp-connect` | An outside agent's app, from its snippet |
| `rustcode guard-command` | Claude Code's `PreToolUse` hook |

## For agents

An agent manages the same things through the `rustcode` MCP server:
`manage_settings` (settings) and `manage_capabilities` (skills, MCP servers,
the default harness, the theme) answer reads at once and show the thread's
approval card, with the value before and after, for every change;
`transfer_session` moves a thread to another harness, account and model. See
[configuration.md](configuration.md) › Server settings across machines and
[mcp-outside-agents.md](mcp-outside-agents.md) for the Admin level an outside
agent needs to change anything.
