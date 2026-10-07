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

## Configuration files

The TOML file contains the `default` model selection, `models` array, runtime
preferences, MCP servers, tool protocol, agent mode, verbosity, theme, active
session, and the per-turn `max_tool_rounds` safety backstop. It defaults to 40
rounds and is only a final limit after semantic loop and failure guards.
Writes use a temporary file and replacement so an interrupted save does not
leave a truncated configuration. On Unix, the file is written with owner-only
permissions because model profiles may contain API keys.

Each model profile may optionally set `max_mutating_calls_per_response` to a
bounded value when its provider is trusted to emit independent edits. Omit the
field to retain the safe default of one; zero is normalized to that default and
values above the hard cap are clamped. Calls still execute sequentially and the
absolute per-response tool-call ceiling remains in force:

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

Run `/skills` (or the `list_skills` tool) to see every root that was actually
searched, and `rustcode doctor` to see which ones exist. `rustcode doctor
--fix` creates only the RustCode-owned root, never the universal or workspace
roots.

## Reduced motion

The activity line above the composer animates a highlight sweep while RustCode
is working. Set `reduced_motion = true` in `config.toml` to render it as plain
static text instead:

```toml
reduced_motion = true
```

The default is off, so the sweep is unchanged unless you opt in. This is purely
presentational, so a project `.rustcode/config.toml` may set it.

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
