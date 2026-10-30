# Configuration

RustCode stores its canonical configuration in `config.toml`. On macOS and
Linux this is `${XDG_CONFIG_HOME:-~/.config}/rustcode/config.toml`; on Windows
it is `%APPDATA%\rustcode\config.toml`. `RUSTCODE_CONFIG_DIR` overrides the
directory on every platform, which is useful for portable installs and tests.

Older installations using `models.json` and `config.json` are still read. On
the next normal save, Rustcode writes the merged configuration to
`config.toml` and leaves the legacy files intact as a rollback copy. Missing
fields use compiled defaults. Malformed or newer unsupported TOML is preserved
and reported instead of being overwritten.

## User-data migrations

When a newer RustCode first starts (terminal, `--acp` for the macOS app, or a
headless prompt), it applies any user-data migrations that release introduced,
whether it was installed with Homebrew, the installer, or by replacing the
binary. Each migration has a stable id and runs once: its id is recorded in
`storage/migrations.json` in the config directory only after it succeeded.
`storage/migrations.lock` keeps RustCode processes that start together from
running the same step twice; a process that cannot take the lock within ten
seconds starts normally and leaves the work to the one that holds it.

A step that fails is not recorded. RustCode prints which step failed, carries
on starting, and retries the step (and the steps after it) on the next start.
Migrations are written to be safe to repeat after an interruption, and they
keep the original data until its replacement is in place.

Inspect the state without changing anything:

```bash
rustcode storage status          # pending and completed migrations
rustcode storage status --json   # the same, for scripts and support tools
rustcode storage usage           # how much space each store takes
rustcode storage usage --json    # the same, with each store's paths
```

`storage usage` reads only. It lists each store of the config directory with
its size, file count, what it holds and how it is bounded today:

| Store | Holds | Bounded by |
| --- | --- | --- |
| threads | each thread's history, logs and artifacts | kept; scratch threads are pruned |
| history archive | history that compaction removed from a thread | kept |
| tool output | full output of commands too long for the conversation | kept; referenced from thread histories |
| attachments | images attached to messages | kept; referenced from thread histories |
| workspaces | isolated subagent workspaces | kept |
| backups | copies of config files taken before a change | kept |
| logs | `debug.log` and `debug.log.1` | rotation |
| sync history | the Git history of `rustcode sync` | kept |
| indexes | the symbol index and saved memory | kept |
| remote | paired devices and the gateway's state | kept |

Everything else (configuration, credentials, models, locks) is listed as
"other". The `sessions` link left by the threads rename is not counted twice.
No store is cleaned automatically beyond what the table says.

`tool output` and `attachments` collect files whose thread was later deleted
or pruned. `storage clean` finds them:

```bash
rustcode storage clean           # list them; changes nothing
rustcode storage clean --apply   # move them to storage/removed
rustcode storage clean --json    # the plan (and result), for scripts
```

A file is listed only when no file under `threads/` or `history_archive/`
names it and it has not changed for 30 days; folders and links in the stores
are never touched, and no other store is. `--apply` moves each file to
`storage/removed/<date-time>/<store>/`: move it back to restore it. A later
`--apply` deletes a folder there once it is 14 days old. Nothing runs this
automatically.

If `migrations.json` was written by a newer RustCode (a higher `format`), this
version runs no migrations and leaves the file alone; upgrade RustCode. If the
file is unreadable, RustCode reports it and runs nothing; move it aside to let
every step run again. The file is machine-local and is never synced.

Registered migrations:

| Id | What it does |
| --- | --- |
| `0001-sessions-folder-to-threads` | Renames `sessions/` to `threads/` and leaves `sessions` as a link to it, so an older RustCode still finds every thread. If an older version created a new `sessions/` after the rename, its threads are moved in; a thread present in both folders is left in `sessions/` and reported, and the step runs again on the next start once one copy is moved aside. Until the step has run, RustCode reads the old folder. |

The legacy configuration conversion above and `rustcode threads migrate
[--dry-run]` keep their own entry points for now.

## Configuration files

The TOML file contains the `default` model selection, `models` array, runtime
preferences, MCP servers, tool protocol, agent mode, verbosity, theme, active
session, and the per-turn `max_tool_rounds` safety backstop. It defaults to 40
rounds and is only a final limit after semantic loop and failure guards.
Writes use a temporary file and replacement so an interrupted save does not
leave a truncated configuration. On Unix, the file is written with owner-only
permissions because model profiles may contain API keys.

Several workspace-changing tool calls in one response (shell commands that are
not provably read-only, file writes and edits, MCP tools without
`readOnlyHint`) run one after another in the order given. Each model profile
may set `max_mutating_calls_per_response` to change how many run per response:
the default is four, zero is normalized to that default and values above the
hard cap are clamped. Calls beyond the limit are held and run automatically in
a later round. Set `allow_tool_batching = false` to restrict a profile to one
workspace-changing call per response:

```toml
[[models]]
name = "trusted-model"
url = "https://example.invalid/v1"
model = "trusted-model"
max_mutating_calls_per_response = 2
```

OpenCode Zen's paid Muse model uses the OpenAI Responses API. RustCode includes
this profile by default; add `OPENCODE_API_KEY` to the environment and select
`opencode-muse-spark-1.3` as the model:

```toml
[[models]]
name = "opencode-muse-spark-1.3"
url = "https://opencode.ai/zen/v1/responses"
model = "muse-spark-1.3"
engine = "openai"
env_key = "OPENCODE_API_KEY"
api_protocol = "responses"
tool_protocol = "apinative"
```

OpenCode's temporary free models are restricted by OpenCode's service to the
OpenCode client session. RustCode does not bypass that restriction; use a paid
Zen model and valid billing/API credentials for this provider profile.

## MCP servers

Configured MCP servers are started by Rustcode before ACP prompts are handled;
ACP's optional MCP-over-ACP transport is not required.

Add, list, and remove servers from the command line. `rustcode mcp add` takes
a name followed by a URL for a remote (Streamable HTTP) server, or by a command
to spawn over stdio:

```sh
rustcode mcp add --transport http api https://mcp.example.com/mcp \
  --header "Authorization: Bearer <token>"
rustcode mcp add --env API_KEY=<key> files -- npx -y @example/files-mcp
rustcode mcp list
rustcode mcp remove api
```

`--transport` is inferred from an `http(s)://` target when omitted. Repeat
`--header` (remote) or `--env` (stdio) as needed, and put a stdio command after
`--` when its arguments start with a dash. `--force` replaces a server with the
same name. Servers are written to `[[mcp_servers]]` in the user `config.toml`
and start with the next session. Header and environment values are stored
there in plain text (the file is created with mode `0600`), and `mcp list`
prints only their names. `config.toml` is one of the files `rustcode sync`
pushes, so keep the sync remote private when it holds tokens.

Inside a session the model does the same through the `manage_mcp_servers`
tool, which applies the validation of `rustcode mcp add`:

| `operation` | Effect |
| --- | --- |
| `add` | Saves the server to the user `config.toml` and starts it in the running session, so its tools are callable in the same turn. Takes `name`, `target` (one URL, or the command followed by its arguments), and optionally `transport`, `headers` (`Name: value`), `env` (`KEY=value`), `client_id`, `always_include`, `replace` (the tool's `--force`) and `start` (`false` to save without starting). |
| `list` | Prints the `mcp list` lines for this workspace, each marked `[running]` or `[not running]`. |
| `start` | Starts, or restarts, a configured server by `name` in the running session. |
| `remove` | Removes the server from the user config and stops it. |

Every operation asks for approval before it runs, because `add` and `start`
launch a command or open a connection the model chose and `add` and `remove`
edit your config; Plan mode blocks the tool outright. Approve-all (YOLO)
answers this prompt like any other. The prompt shows the command or URL and the
header and environment names; their values are left out of the prompt, the tool
result and startup errors, as in `mcp list`. The values the model passes are
still part of its own tool call in the session history, so prefer adding a
server that needs a real secret with `rustcode mcp add` yourself. A server that
fails to start stays saved, and `start` retries it. When a project
`.rustcode/config.toml` sets its own `mcp_servers`, a server added to the user
config is saved but is not active, and not started, in that workspace.

Native API requests expose at most 16 MCP tool schemas at a time. Set
`always_include = true` on an MCP server's `[[mcp_servers]]` entry to reserve
slots for its complete toolset, independent of the current prompt:

```toml
[[mcp_servers]]
name = "mail"
command = "mail-mcp"
args = []
always_include = true
```

Reservations are applied in configuration order and still obey the schema
byte budget. If a complete server toolset cannot fit either limit, that
reservation is rejected for the request; the rejection and omitted tool names
are recorded in `mcp.native_schema_selection`.

### Web search with Parallel

[Parallel Search MCP](https://docs.parallel.ai/integrations/mcp/search-mcp)
provides web search and page fetching over Streamable HTTP. The anonymous
endpoint needs no account or API key and uses Fast mode for search, with lower
rate limits intended for exploration and light use.

```sh
rustcode mcp add --transport http parallel-search https://search.parallel.ai/mcp \
  --header "User-Agent: RustCode"
rustcode mcp list
```

Start a new session in a workspace without a project `mcp_servers` override,
then ask, for example:

> Use Parallel to find the current Tokio `JoinSet` documentation, fetch the
> relevant page, and explain how to cancel all tasks. Include the source URL.

The server exposes `web_search` and `web_fetch`. Search sends the query to
Parallel; fetching sends the requested URLs. To remove the saved server, run
`rustcode mcp remove parallel-search`.

### Local code lookup with Sem

[Sem](https://github.com/Ataraxy-Labs/sem) provides symbol lookup, source search,
structural diffs and impact analysis over stdio MCP. Install Sem using its
upstream instructions and ensure `sem --version` works in Rustcode's environment.

```sh
rustcode mcp add --env SEM_REPO=/absolute/path/to/checkout sem -- sem mcp
rustcode mcp list
```

Replace the path with an existing Git checkout on the machine running Rustcode.
Start a new session without a project `mcp_servers` override, then ask:

> Use Sem to find an existing function by name and show its implementation and
> callers. Do not edit the code or run validation commands.

`sem_find` retrieves symbols and context; `sem_grep` searches source text.
The configured path is fixed, not automatically changed when you switch
workspaces. Remove it with `rustcode mcp remove sem` before configuring another
checkout. The first lookup may build a local index. The full server also exposes
validation tools that can execute commands: `SEM_REPO` selects a repository, not
a sandbox. Use trusted checkouts and appropriate process permissions. Source
returned by tools becomes part of the context sent to your configured model.

## Remote gateway address

`/remote` starts the [remote gateway](remote.md) when none is running. Where
that gateway listens comes from the `[remote]` section:

```toml
[remote]
bind = "100.92.13.44"                # address to listen on; default 127.0.0.1
port = 17879                         # optional; default 17879
advertise = "mac.netbird.cloud"      # optional; what devices dial, if not `bind`
prefer_interface = "netbird"         # optional; with an auto bind, try this interface first
```

Everything unset means loopback, which a phone cannot reach; `/remote` then
explains what to set. The section is read from the user configuration only: a
project file cannot choose the interface your sessions are exposed on. A
gateway you start yourself with `rustcode remote serve --bind …` takes its
address from the command line and ignores this section.

A paired device can browse folders and start sessions only inside your home
folder. To allow other places, such as a second disk, list them as `roots`:

```toml
[remote]
roots = ["/Volumes/Work", "/opt/projects"]   # absolute or ~/…; missing ones are ignored
```

The gateway reads `roots` on every request, so a change needs no restart. Like
the rest of the section it is read from the user configuration only.

## Project configuration

Create a project-local override with:

```bash
rustcode init
# or: rustcode --init
```

This creates `.rustcode/config.toml` from the global model defaults and adds
`.rustcode/config.toml` to the project `.gitignore`. It intentionally does not
copy API keys, MCP servers, or session state. Project configuration is loaded
from parent to child directories, so the nearest file wins:

```text
CLI overrides > nearest project config > global config > built-in defaults
```

Project files are partial overrides; omitted fields continue to come from the
lower-precedence layer.

Legacy `[laya]` config tables are ignored and can be removed from user or
project config files.

## Skills
Skills are plain Markdown: `<root>/<name>/SKILL.md`, scanned exactly one level
deep. RustCode reads them from these roots, highest precedence first, and the
first definition of a skill name wins:

1. `extra_skill_dirs` from the global `config.toml` (explicit override)
2. `RUSTCODE_EXTRA_SKILL_DIRS`, using the platform path-list separator
3. `<workspace>/.rustcode/skills`
4. `<workspace>/.agents/skills`
5. `~/.agents/skills` — the universal root, shared with other agents
6. `<config dir>/skills` — RustCode's own root (see *Configuration files*)

So a project can override a user-level skill, and an explicitly configured
root can override both. `.claude/skills` is deliberately **not** scanned; add
it to `extra_skill_dirs` if you really want Claude Code's skills.

Extra roots are a global-only setting — a checked-out `.rustcode/config.toml`
cannot widen skill discovery:

```toml
extra_skill_dirs = ["~/work/team-skills", "/opt/shared/skills"]
```

Run `/skills` to list and toggle the discovered skills, the `list_skills` tool
to see every root that was actually searched, and `rustcode doctor` to see
which ones exist. `rustcode doctor --fix` creates only the RustCode-owned root,
never the universal or workspace roots.

## Turning skills and MCP servers on and off

The terminal's `/skills` and `/mcp` panels and the macOS app's **Settings →
Skills** and **Settings → MCP Servers** list the same things and save to the
same place:

- **Skills.** Every discovered skill is listed (installed); a skill that is
  turned off stays installed but is never offered to the model — routing hints,
  `list_skills` and `use_skill` all skip it. Off skills are stored by name in
  `disabled_skills` in the user `config.toml`, so the choice applies to every
  project. A project `.rustcode/config.toml` cannot set it.

  ```toml
  disabled_skills = ["cloudflare", "wrangler"]
  ```

- **MCP servers.** Every configured server is listed with whether it is
  actually connected in the engine (and how many tools it has), not connected,
  turned off, or failed with its error. A toggle sets the server's `enabled`
  flag in the user config and starts or stops it right away. A server that only
  a project config file declares is shown but has to be changed in that file.
  Header and environment values are never shown; credentials in a URL and
  secret-looking arguments (`--token`, `API_KEY=…`) are masked.

A toggle applies from the next message of every open session in the process
that made it: the terminal you toggled in, or the macOS app's engine for all of
its chats. A turn already running is not changed. Another RustCode process
reads `disabled_skills` the next time it scans skills, and brings its MCP
servers in line with the config on its next message: added or turned-on
servers start, removed or turned-off ones stop.

The app's MCP Settings page also adds servers (**Add Server**) and removes them
(right-click a server). It reads and writes these through the ACP extension
requests `_rustcode/capabilities`, `_rustcode/capabilities/set` and
`_rustcode/capabilities/mcp/{add,remove}` (see
[acp.md](acp.md#skills-and-mcp-servers)).

## YOLO by default

YOLO auto-approves tool calls that would otherwise ask for confirmation. It is
normally per session, turned on with `--yolo`, `/yolo on` or the approval
prompt's toggle. Set `yolo = true` in the user `config.toml` to start every new
session that way, including sessions an app opens or creates through the
[remote gateway](remote.md):

```toml
yolo = true
```

The default is off. A session's own setting is recorded with the session
whenever it changes and wins over this default when the session is reopened by
a remote session owner, so `/yolo off` sticks for that session. Plan mode still
blocks mutating tools, and `sandbox_mode` is unchanged (YOLO runs commands as
`trusted` only while it is on). This is a user trust decision, so a project
`.rustcode/config.toml` cannot set it.

## Reduced motion

The activity line above the composer animates a highlight sweep while RustCode
is working. Set `reduced_motion = true` in `config.toml` to render it as plain
static text instead:

```toml
reduced_motion = true
```

The default is off, so the sweep is unchanged unless you opt in. This is purely
presentational, so a project `.rustcode/config.toml` may set it.

## Mouse wheel speed

Each mouse wheel event scrolls the transcript one row, and the terminal decides
how many events a wheel notch or trackpad gesture sends. When scrolling feels
slow in every TUI, raise it in the terminal: kitty on Wayland reports a mouse
wheel as high-resolution scrolling, so `touch_scroll_multiplier` in
`kitty.conf` (default 1) is the setting that applies, not
`wheel_scroll_multiplier`.

To scroll more rows per event in RustCode alone, run `/scroll-speed`: a panel
steps the value with ←/→ (or a digit), Enter saves it and Esc cancels.
`/scroll-speed 3` sets it directly. Both write `scroll_lines` (1–20) and apply
at once:

```toml
scroll_lines = 3
```

This is purely presentational, so a project `.rustcode/config.toml` may set it.

## Keeping the transcript in terminal scrollback

The readable transcript lives in the terminal surface RustCode paints, and on
exit that surface is erased, so the conversation does not stay in the
terminal's own scrollback. Set `preserve_transcript_scrollback = true` in
`config.toml` to also copy every committed transcript row into native
scrollback as the session runs:

```toml
preserve_transcript_scrollback = true
```

The default is off. Native scrollback is write-only: rows copied there cannot
be revised, so an expanded tool body stays expanded after you collapse it, and
the rows sit above the exit handoff where no erase can reach them (#1587,
#1593). Turn it on if you rely on the scrollback copy for terminal copy/paste
or shell piping, and accept that it is a copy rather than the live transcript.
Use the mouse wheel, `PageUp`/`PageDown`, or `Esc` to read the transcript while
a response streams; the transcript never leaves the view (#1595).

## Syncing config, skills, and themes

Initialize a config sync repository with a remote Git URL, then choose a
direction explicitly or run the default pull-then-pull sync:

```bash
rustcode sync init <remote-git-url>
rustcode sync --pull       # or: rustcode sync pull
rustcode sync --push       # or: rustcode sync push
rustcode sync              # pull, then push
```

`--pull` and `--push` cannot be used together.

`rustcode sync` stages only files inside the RustCode config directory:
`.gitignore`, `config.toml`, `skills/`, `themes/`, and `commands/`. Skills in
`~/.agents/skills`, a workspace `.rustcode/skills`, or any
`extra_skill_dirs` entry are **not** synced — the universal root is shared
with other agents, and pushing one agent's skills into it would conflict with
them. Version those skills separately, or point `extra_skill_dirs` at a
directory you sync yourself.

## Prompt commands

In the terminal frontend, reusable Markdown templates are loaded with
`/prompt <name> [arguments]`; `/prompts` lists their names and source paths. A
loaded template is placed in the composer for editing and review, and is sent
only after you press Enter again. Put user templates in
`<config dir>/commands/<name>.md` and workspace templates in
`<workspace>/.rustcode/commands/<name>.md`. Workspace templates override user
templates with the same name. Names use lowercase ASCII letters, numbers,
hyphens, or underscores. Templates can use the literal `$ARGUMENTS`
placeholder; supplied arguments replace every occurrence, or are appended after
a blank line if there is no placeholder. With no arguments, placeholders are
removed.

For example, create `<workspace>/.rustcode/commands/review.md`:

```markdown
Review $ARGUMENTS for correctness and missing tests.
```

Then run `/prompt review the current diff`, review or edit the staged text, and
press Enter to submit it. Templates are limited to 64 KiB and expanded prompts
to 256 KiB; empty, invalid UTF-8, symlink, and slash-command expansions are
rejected. Listing does not create command directories. User templates are
included in `rustcode sync`; workspace templates remain with the project.
