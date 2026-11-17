# Command line

Every `rustcode` command, what it does, and what it changes. Each command takes
`--help`. Commands that manage RustCode itself (#3821) go through the running
server when there is one (`rustcode server`), so the app hears of the change at
once, and work on the files directly when none runs; the **Server** column
says where a command needs the server. Commands marked **asks** confirm on a
terminal and need `--yes` (`-y`) in a script. `--json` prints stable JSON on
the commands that list or read.

No command prints a credential: accounts show their sign-in state, MCP servers
the names of their headers and environment variables, never the values. The
one exception is `outside create`, whose token is the point of the command and
is shown once.

`cli_doc_lists_every_command` in `rustcode/tui/src/cli.rs` fails when a
command is added without a row here.

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
| `rustcode sync pull\|push\|init` | Sync config, skills and themes with a Git remote | Config files |
| `rustcode discord` | Discord Rich Presence setup and status | Config |
| `rustcode defaults` | Show or set the model profile and effort new sessions start with | Config |

## Server, daemon and machines

| Command | What it does | Changes |
| --- | --- | --- |
| `rustcode server run\|connect` | Run the server, or relay stdio to it (starting it) | — |
| `rustcode server install\|uninstall\|restart` | Manage the background service | Service (**asks** on restart) |
| `rustcode server status\|logs\|link-path\|update-cli` | Read the server's state, log, CLI link and update | `update-cli` replaces the CLI |
| `rustcode serve` | Serve sessions over TCP for remote frontends (experimental) | — |
| `rustcode daemon start\|run\|stop\|status\|logs` | The scheduler daemon | Daemon process |
| `rustcode cron add\|list\|pause\|resume\|run\|history\|delete` | Scheduled jobs in the daemon | Jobs |
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

## Settings and what threads can use

| Command | What it does | Changes | Server |
| --- | --- | --- | --- |
| `rustcode settings list\|get [--project]` | Server settings with value, source, default and accepted values | — | |
| `rustcode settings set <key> <value> [--project]` | Change one; prints `before → after` | `settings.json` | |
| `rustcode settings reset <key> [--project]` | Back to the default (or the machine's value) | `settings.json` | |
| `rustcode skill list\|enable\|disable` | Skills offered to threads | `config.toml` | |
| `rustcode mcp add\|list\|remove` | MCP servers in the user config | `config.toml` | |
| `rustcode mcp enable\|disable` | Turn one on or off; the server starts or stops it | `config.toml` | |
| `rustcode mcp restart <name>` | Reconnect one | — | Needed |
| `rustcode harness list` | Harnesses, sign-in, accounts and the default | — | |
| `rustcode harness default <id> [--model] [--effort]` | The harness new threads start on | `config.toml` | |
| `rustcode theme list\|show\|new\|set\|validate\|path` | Themes for the terminal and the app | Theme files, `config.toml` | |

## Accounts, sign-ins and outside agents

Not available to agents: an agent never adds, signs out or mints a credential.

| Command | What it does | Changes | Server |
| --- | --- | --- | --- |
| `rustcode account list\|check` | Named harness accounts and their sign-in state | — | |
| `rustcode account add <harness> <id> [--name] [--config-dir]` | Register a named login in its own folder | `config.toml` | |
| `rustcode account remove <harness> <id>` | Forget it; its folder and login stay (**asks**). Refused while a thread runs on it | `config.toml` | |
| `rustcode connections list` | GitHub CLI accounts and saved model sign-ins | — | |
| `rustcode connections logout github <login>` / `<provider> <account>` | Sign one out (**asks**) | The sign-in | |
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
