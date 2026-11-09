# Harnesses

A **harness** is the agent runtime that runs a thread. RustCode's built-in
harness is the engine's own turn loop, and it stays the default with the most
features. Other coding agents — Codex, Claude Code, Gemini CLI, OpenCode, Pi, or
any ACP agent — can run a thread as external harnesses. Each owns its own
sign-in, model catalog, tools and permission rules. RustCode never stores their
credentials.

Terms (see [terminology.md](terminology.md)):

- **Provider**: a model API the built-in harness calls (OpenAI, Anthropic,
  Copilot, an OpenAI-compatible server). Unchanged by harnesses.
- **Harness**: what runs a thread's turns. Chosen when the thread starts.
- **Agent**: a subagent a thread starts (see terminology.md).

## Where it lives

The harness layer is in the engine (`rustcode/engine/src/harness/`), not in a
frontend. Every client of `rustcode --acp`, the desktop app first of all, gets
external harnesses through the same ACP connection it already uses.

```
client ──ACP──▶ rustcode --acp ──▶ built-in turn loop          (harness "rustcode")
                               └─▶ child process, ACP client   (kind "acp")
                               └─▶ codex app-server, JSON-RPC  (kind "codex")
```

For an external thread the engine is a proxy: it spawns the harness process,
translates its output into ordinary ACP `session/update` notifications and
`session/request_permission` requests, and keeps the thread in RustCode's
thread list. The client renders an external thread with the same code as a
built-in one.

## Kinds

| Kind | Driven through | Examples |
| --- | --- | --- |
| `builtin` | The built-in harness's turn loop | `rustcode` |
| `acp` | ACP v1 over the child's stdio; RustCode is the ACP client | `opencode acp`, `gemini --experimental-acp`, Claude Code through `claude-agent-acp`, Codex through `codex-acp` |
| `codex` | `codex app-server` JSON-RPC over stdio | `codex` |

Native protocols are used only where they give something ACP does not
(Codex's app-server needs no extra bridge install). Everything else is ACP.

## Capabilities

Each harness declares what it supports. Clients read these flags and never
branch on the harness id. A missing feature shows as unavailable, with a
reason, and is never faked (for example, a permission mode is offered only
when the harness itself enforces it; RustCode does not auto-answer approvals
to imitate one).

```jsonc
{
  "steer": "live" | "next_turn" | "none",   // _rustcode/steer, for clients that predate `steering`
  "steering": {                              // the same fact, and what a steer can carry
    "live": true,                            // a message goes into the running turn
    "attachments": false,                    // a live steer takes its pictures and files too
    "while_awaiting_user": false,            // a live steer goes in while an approval waits
    "interrupt_restart": true,               // the turn can be stopped, the message sent in its place
    "queue": true                            // a message that does not steer waits for the next turn
  },
  "cancel": true,                            // session/cancel
  "resume": true,                            // session/load after restart
  "rewind": false,                           // _rustcode/rewind
  "images": true,                            // image prompt blocks
  "models": "dynamic" | "static" | "none",  // via the "model" config option
  "effort": true,                            // "effort" config option
  "permission_modes": ["default", "auto_edit", "full_access"],
  "subagents": false,                        // emits subagent activity
  "slash_commands": true,                    // available_commands_update
  "smart_ui": false,                         // honours rustcode-ui blocks
  "rustcode_extensions": false,              // PRs, usage, preview… (built-in only)
  "orchestration": true                      // thread tools via rustcode mcp-orchestrator (docs/orchestrator.md)
}
```

## ACP contract

All methods are on the client's existing `rustcode --acp` connection.

### `_rustcode/harnesses` → `HarnessSnapshot`

```jsonc
{
  "harnesses": [{
    "id": "codex",
    "name": "Codex",
    "kind": "codex",
    "builtin": false,           // true only for "rustcode"
    "known": true,              // one RustCode ships with: turn off, never remove
    "enabled": true,            // user turned it on (onboarding / Settings)
    "command": ["codex", "app-server"],
    "installed": true,          // command resolves on PATH
    "version": "0.162.1",       // when cheaply known
    "auth": "ok" | "needs_login" | "unknown",
    "login_command": ["codex", "login"],   // run in a terminal by the client
    "install_hint": "brew install codex",
    "capabilities": { /* see above */ },
    "models": [{ "id": "gpt-6-luna", "name": "gpt-6-luna",
                 "efforts": ["low", "medium", "high"], "default_effort": "medium" }],
                 // "efforts_unknown": true when those are the union of what the
                 // harness's models offer, not this model's own (see below)
    "last": { "model": "gpt-6-luna", "effort": "high" },  // last new-thread choice, or null
    "accounts": [{              // named logins of this harness; [] for most
      "id": "paral-2", "name": "Paral 2",
      "config_dir": "~/.claude-paral-2", "auth": "ok" | "needs_login" | "unknown",
      "own_dir": false          // true for the Mac's own login, ~/.claude or ~/.codex
    }],
    // How a client adds and signs in a named login, from the engine; null for
    // a harness without named logins (Claude Code and Codex have them).
    "account_env": "CLAUDE_CONFIG_DIR",           // the variable naming its directory
    "account_login_command": ["claude", "auth", "login"],  // run in a terminal
    "discovered_accounts": [{   // logins on this Mac no account names yet
      "config_dir": "~/.claude-personal", "suggested_name": "Personal"
    }],
    "error": null               // last detection/start error, if any
  }],
  "default": { "harness": "rustcode", "model": null, "effort": null },
  "onboarded": false            // first-run onboarding finished
}
```

`models` may be empty until `_rustcode/harnesses/models` ran for that harness
(or a thread started it) once; after that it comes from the model cache, also
after an engine restart (see below). For `rustcode` it lists the configured
model profiles, each with the efforts its `effort` option offers (`[]` when it
offers no choice) and `default_effort` (the profile's `reasoning_effort`, else
`"default"`); `version` is the engine's and `auth` is `"ok"` (its providers
have their own sign-in). `last` is what `_rustcode/harnesses/remember` kept
(always present, `null` when nothing is kept, so a client can tell an engine
that remembers from an older one). `login_command` and `install_hint` are `null` for custom
harnesses. `known` is true for `rustcode` and every harness in the table below,
and false for one the user added by hand: a client shows "Remove" only for the
latter, and groups those apart. Detection is cached for a minute; `set` and
`remove` refresh it.

| id | kind | command | version from | auth from |
| --- | --- | --- | --- | --- |
| `codex` | `codex` | `codex app-server` | `codex --version` | `codex login status` |
| `claude-code` | `acp` | `npx -y @agentclientprotocol/claude-agent-acp` | `claude --version` | `claude auth status --json` |
| `gemini` | `acp` | `gemini --experimental-acp` | `gemini --version` | `unknown` |
| `opencode` | `acp` | `opencode acp` | `opencode --version` | `unknown` |
| `pi` | `acp` | `npx -y pi-acp` (pi's RPC mode adapted to ACP) | `pi --version` | `unknown` |

Commands are looked up on the engine's `PATH` and then the login shell's, so
an app started from the Dock finds what the terminal finds. On Windows a bare
name tries the `PATHEXT` extensions in order, as the shell does: `codex` is
`codex.exe`, npm's `npx` is `npx.cmd`.

Claude Code's status is the `loggedIn` boolean of `claude auth status --json`;
a third-party backend (Bedrock, Vertex, Foundry) or an API key source signs in
without that login, so both count as `ok`. Output that is not that JSON leaves
`auth` at `unknown`, never `needs_login`: an older CLI without the command is
no answer, not a sign-out. Probes run in the user's home, so a project's own
settings cannot decide a harness-wide answer.

Errors (an unknown id, an invalid command, removing a known harness) are
`invalid_params` with a message for the user.

### `_rustcode/harnesses/models` `{ "harness": "codex", "refresh"?: true }` → `{ "models": [...] }`

Starts the harness briefly if needed to list its models. `refresh` starts it
even when its models are known, which is what Settings' Check sends: a Check
proves the harness runs now, not that it once did. Codex answers from
`model/list` (hidden models left out, efforts per model). An ACP harness is
started in the system temp directory and one session is opened there to read
its `model` and thought-level options, then killed; no thread's process is
touched.

An ACP agent reports the thought levels of the model its session runs alone,
so a model is selected to read its own. That costs a round trip each, so the
listing asks only the models a person can see: the model the session runs,
the model a setting names (`[harness_defaults].model`, a `[[harnesses]]`
entry's `model`, or one already learned), then the catalog in order, at most
30 models and 5 seconds (`PROBE_LIMIT` and `PROBE_BUDGET` in
`rustcode/engine/src/harness/acp_child.rs`). Each probed model lists the
efforts it really takes, ordered from no thinking to the most (`default`
first), with `default_effort` the level the harness puts it on.

Every other model keeps `"efforts_unknown": true` and lists the union of the
levels known for the harness, with `default_effort` the level the harness runs
with: never another model's list as if it were its own. A level picked from
that union is clamped to the closest one the model really takes when it is
sent, and a thread that starts or switches to the model learns its own levels,
so a model the user picks is exact from then on.

**Model cache.** Every listing (this request, or a thread starting the
harness) is kept in memory and in `~/.config/rustcode/harness-models.json`:
per harness its models, `fetched_at` (Unix seconds), the harness `version`
it came from, and the learned `effort`, `resume` and `images` capabilities. A
restarted engine reads it, so the snapshot, the capabilities and the
`session/load` answer of a restored thread (its `model` and `effort`
options, before the harness starts again) are complete. A cached list older
than 24 hours, from another harness version, or learned at a thread start
(which sees the model it runs alone) is answered at once and listed again in
the background. A failed listing is not cached: the next request tries again
(one listing per harness runs at a time). Every model a thread starts or
switches to also updates that model's entry, so the cache converges on the
models people use.

### `_rustcode/harnesses/set` → `HarnessSnapshot`

```jsonc
{ "id": "codex", "enabled": true, "command": ["codex", "app-server"] }
```

Adds a custom ACP harness when `id` is new (`kind` defaults to `acp`; pass
`name`, `kind`, `command`, and `env` for variables the process starts with).
`command` is the program then one argument per entry, started directly with no
shell. `command`, `name` and `env` are optional on existing ones; `env`
replaces the old map and an empty one clears it. The snapshot lists a
harness's variable names as `env_names`, never their values.

### `_rustcode/harnesses/remove` `{ "id": "my-agent" }` → `HarnessSnapshot`

Only custom harnesses can be removed; built-in entries can only be disabled.

### `_rustcode/harnesses/default` → `HarnessSnapshot`

```jsonc
{ "harness": "codex", "model": "gpt-6-luna", "effort": "low" }
```

Sets `[harness_defaults]`, which harness a new thread starts on. A model or
effort given here is also kept as that harness's last choice.

### `_rustcode/harnesses/remember` → `{ "harness", "last" }`

```jsonc
{ "harness": "claude-code", "model": "opus", "effort": "high" }
// → { "harness": "claude-code", "last": { "model": "opus", "effort": "high" } }
```

Keeps the model and effort last chosen for a new thread on that harness,
as `model` and `effort` on its `[[harnesses]]` entry (empty values clear
them). A client sends it when the user picks a model or effort in a
new-thread picker. A model or effort changed inside a thread
(`session/set_config_option` `model` or `effort`) changes that thread and is
also kept as the harness's last choice, as most apps do: the next new thread
starts where the user last left that harness. It never changes which harness
new threads start on; that stays `[harness_defaults].harness`. A known
harness without an entry gets one with `enabled = false` (as it was);
`rustcode` gets an entry of its own. An unknown harness is `invalid_params`.

### Named accounts (Claude Code and Codex)

A harness may have named logins, each only a config directory of its own:
`CLAUDE_CONFIG_DIR` for Claude Code, `CODEX_HOME` for Codex. RustCode names the
directory and starts the harness with it. It never reads, copies, stores or
proxies the tokens in it, except the read-only plan-usage lookup below.
Accounts are chosen by hand per thread: there is no automatic switching,
rotation or pooling, and each login keeps its own plan and limits. Only
`claude-code` and `codex` have accounts; Pi, OpenCode, Gemini and a custom
harness run on their own single sign-in.

Each harness's facts are one `AccountLayout` in
`rustcode/engine/src/harness/config.rs`: its variable, default directory
(`.claude`, `.codex`), the prefix a named directory is created under
(`.claude-`, `.codex-`), its sign-in command (`claude auth login`,
`codex login`), the file a signed-in directory holds (`.credentials.json`,
`auth.json`), the variables that would override the login
(`ANTHROPIC_API_KEY`/`ANTHROPIC_AUTH_TOKEN`/`CLAUDE_CODE_OAUTH_TOKEN`,
`OPENAI_API_KEY`/`CODEX_API_KEY`), and the names that make a directory look
like one of its logins. Everything else in the account code reads that
description, so a new harness needs one constant and no branch.

```toml
[[harness_accounts]]
id = "paral-2"                 # letters, digits, '-', '_' or '.'
harness = "claude-code"
name = "Paral 2"
config_dir = "~/.claude-paral-2"  # optional; defaults to ~/.claude-<id>
                                  # (or ~/.codex-<id> for Codex)
```

The engine creates the directory when it is missing. `~/.claude` and
`~/.codex` may be accounts too: they are registered like any other, and a
thread on one runs as the Mac's own login, with no `CLAUDE_CONFIG_DIR` or
`CODEX_HOME` set (macOS keys Claude Code's Keychain item on the variable, so
setting it would look signed out; Codex reads `~/.codex` with none).

A Codex account's directory *replaces* the Codex home: `CODEX_HOME` makes the
CLI read that directory's `config.toml`, `AGENTS.md` and `[mcp_servers]`
instead of `~/.codex`'s. So a new account starts without the user's profiles,
model providers and MCP servers, and RustCode copies and links nothing: what an
account should have goes in its directory by hand. `~/.codex` itself, registered
or not, is never written to. Settings › Harnesses' Codex Accounts section says
"Each account has its own Codex settings." for the same reason.
The section is read from the user config only: a checked-out project cannot
choose where a thread signs in.

| request | answer |
| --- | --- |
| `_rustcode/harnesses/accounts/add` `{ "harness": "claude-code", "id": "paral-2", "name": "Paral 2", "config_dir": "~/.claude-paral-2" }` | `HarnessSnapshot` |
| `_rustcode/harnesses/accounts/remove` `{ "harness": "claude-code", "id": "paral-2" }` | `HarnessSnapshot` |
| `_rustcode/harnesses/accounts/check` `{ "redetect"?: ["codex"] }` | `HarnessSnapshot`, each account's `auth` read again; `redetect` names harnesses whose install is detected again instead of read from the last minute |

*Remove* drops the config entry only: the directory, with the login and the
conversations in it, stays on disk, and the request is refused while a live
thread runs on that account. Threads that ran on it stay readable: they open
and replay their history, and only their next prompt reports the missing
account (they can be moved to another named account, or to the harness's own
sign-in when it has none left). `auth` for an account is `ok` when its own
sign-in is there, `needs_login` when a look found none, and `unknown` when it
was not looked for. `check` (Settings › Harnesses' look when the page opens, when
the window regains focus, and on Check) runs the harness's own cheap status command
with the account's directory and with the variables a thread on that account
would drop (`codex login status`, `claude auth status --json`), then falls back
to the marker file by name (`<dir>/.credentials.json`, Codex's
`<dir>/auth.json`) and, on macOS, to whether Claude Code's Keychain item for
the directory exists. The answer carries no secret and the engine reads none,
and a status command or Keychain read that could not answer never counts as
signing out. It does not force a plan read. Nothing is started as a session to
answer. `own_dir` is true for the
Mac's own login, `~/.claude` or `~/.codex`,
whatever path the account was written with.

`discovered_accounts` lists the login directories of that harness on this Mac
that no account names yet: `~/.claude` and `~/.claude-*` that hold
`.claude.json`, `.credentials.json`, `settings.json` or `projects/`, or
`~/.codex` and `~/.codex-*` that hold `config.toml`, `auth.json` or
`sessions/`. Only names are read, never file contents, and `suggested_name` is
the title-cased suffix (`Personal` for `~/.claude-personal`, `Paral` for
`~/.codex-paral`) or the harness's own name for its default directory. Settings'
*Found on this Mac* registers one with `_rustcode/harnesses/accounts/add` and
its `config_dir`. `account_env` and `account_login_command` say how the client
runs one: the variable to set and the harness's own command to run in a
terminal.

A thread picks an account in `session/new`'s `_meta.rustcode.account`, next
to `harness`, or on `_rustcode/session/harness` when it moves to another
login. An unknown account id is an error, never a quiet fall back to the
harness's own login. Moving an idle thread to another account is exactly a harness
switch: its native session id is cleared, the handoff marker is recorded, the
new account's process starts, and the next prompt carries the conversation so
far as a summary (the conversation lives in the other directory). A running
turn is refused. The thread's `harness.json` keeps the account, so a resume
starts in the same directory.

The started process gets `CLAUDE_CONFIG_DIR` only; `HOME` is untouched (it
would hide the macOS Keychain login) and `ANTHROPIC_API_KEY`,
`ANTHROPIC_AUTH_TOKEN` and `CLAUDE_CODE_OAUTH_TOKEN` are removed from that
child, so a login-shell variable cannot override the chosen login.

With named accounts, a new thread always starts on one of them: the picker
offers no harness login, and the account is the one chosen for that harness
while it is still there, else the first named login that is signed in. A
thread started by another thread (`start_session`, `fan_out`) runs on its
parent's account when it uses the same harness, else on that same first login
(`apps/desktop/src/state/logins.ts`). A thread that started on the harness's
own sign-in before accounts existed keeps running on it, shows as "Original
login", and moves to a named account through the same switch below. Either tool also
takes an explicit `account` naming one of that harness's accounts (an id, or a
display name; `list_harnesses` lists them), which wins over the inherited
choice. An exact id wins over a display name, so an id stays reachable when
another login is named the same; a case-insensitive id that matches more than
one account, an ambiguous display name, an id the harness does not list, and
an explicitly signed-out login are all refused with the ids that would work,
never a quiet fall back to another login. `list_harnesses` carries each
account's `signed_out` flag so an agent can avoid one before asking, and the
approval card shows the account's display name and id. That argument is
orchestration version 5: an
older client is not shown it, and the engine refuses it from one that sends it
(`docs/orchestrator.md` › "Orchestration tools v5"). Claude Code's own
`auth status --json` reports its harness and account `auth`; the plan read
described below is what reports a login signed out for the usage page.

Plan usage is read per account, one login at a time:
`_rustcode/usage/plans` with `claudeAccount: "<id>"` returns only that account's
Claude row (`account` and its name) — the one the thread on screen runs on, so
its numbers are there when the thread opens — and `claude: false` beside it
leaves the default login unread. `claudeAccounts: true` reads every account's
row instead, which Settings › Usage sends when the page is shown or refreshed.
An id that names no `claude-code` account of the config reads nothing, so an
unseen account's Keychain item is never read in the background. On macOS a
named account's Keychain item is `Claude Code-credentials-` plus the first 8
hex digits of the SHA-256 of its directory path; that name is undocumented, and
a missing item is reported as not signed in rather than guessed at. Both copies
Claude Code can keep are read — that item and `<config dir>/.credentials.json`
— and the one whose access token expires last is used, since Claude Code writes
back to whichever store it could open and one can lag the other.
A Claude
row that could not be read is `unavailable` with a `reason` beside its `error`:
`expired`, `refused`, `forbidden`, `denied` or `transient`. An access token
lives hours and Claude Code renews it only when it runs, so a login nobody ran
a thread on since lapses while staying signed in: instead of only saying so, the
read answers with the windows RustCode recorded last for that login (in
`claude-usage.json` in the config directory) and marks them `asOf` with when
they were read, which the app draws "as of" then. Only a login with nothing
recorded is `expired`, whose message asks for a thread on that account, which
is what renews the token.
A read that a denied
Keychain prompt stops pauses that one login (a signed-out login is not read
again in the background either); Settings › Usage, Refresh and
Check in Settings › Harnesses read it again. A Keychain item macOS refused whose
credentials-file fallback holds an expired sign-in, or whose request to Anthropic
does not succeed, is reported as `denied` rather than as `expired`, `refused` or
`transient`, so the retry policy cannot turn a refusal into a prompt on every
poll; a fallback whose request does answer reports its windows normally. The
refusal itself is remembered for the engine's lifetime: background reads then
use the credentials file only and never ask macOS again, while a fallback that
answers keeps drawing its numbers. An explicit look (Refresh, Check) asks macOS
once more, and a handover clears the memory. An expired or refused sign-in is not
paused: Claude Code renews it, so the background read keeps asking and picks the
new sign-in up without a visit to Settings. A `forbidden` one (HTTP 403) is not
retried on its own either, since renewing it does not grant the usage read.
`Refresh` sends `force: true`, which reads the logins again rather than answering
from the engine's one-minute cache; with Claude's switch off no login is read,
forced or not. One login is read once at a time: a forced read that arrives
while the background poll is reading it waits, and shares an answer whose read
began after the ask instead of reading the Keychain a second time.

### `_rustcode/onboarding/complete` `{}` → `HarnessSnapshot`

Marks first-run onboarding done (`onboarded: true`).

### Starting a thread on a harness

`session/new` with:

```jsonc
"_meta": { "rustcode": { "harness": "codex", "model": "gpt-6-luna", "effort": "low" } }
```

For a harness with named accounts, `"account": "paral-2"` picks one; without
it the harness's own login runs, exactly as before accounts existed (the app
always names one when the harness has any).

No `harness` (or `"rustcode"`) starts a built-in thread, exactly as before.
A missing `model` or `effort` falls back, in order, to the harness's last
choice (`_rustcode/harnesses/remember`), to `[harness_defaults]` when that
names the same harness (the built-in one when it names none), then to the
harness's own default. A remembered or default model the harness no longer
lists (when its list is known) is skipped, and so is an effort its model does
not offer, without an error; a remembered effort is used only with the model
it was kept with. Any configured harness can
start a thread; `enabled` is what the app offers, not a gate. The engine
starts the harness before it answers, so a harness that cannot start fails
`session/new` with its reason (for example `codex not found on PATH`). The
response carries:

```jsonc
"_meta": { "rustcode": { "harness": { "id": "codex", "capabilities": { … } },
                         "account": { "id": "paral-2", "name": "Paral 2" } } }
```

`account` is there only for a thread on a named login.

(built-in threads carry it too, on `session/new` and `session/load`, with
`id: "rustcode"`) and the usual `configOptions` (`model`, `effort` when supported), so model
and effort switch with `session/set_config_option` for every harness. For an
ACP harness the agent's own options are passed through: its `model` option
(or category `model`) is `model`, its first `thought_level` option is
`effort`, any other keeps its own id. Codex's `model` and `effort` apply from
the next `turn/start`.

After that the thread uses standard ACP: `session/prompt`, `session/cancel`,
`session/update` (agent text, thoughts, tool calls and their output, plans,
usage), `session/request_permission`. `session/list` returns external threads
too, each with `_meta.rustcode.harness` = `{ "id": "codex" }` (and
`_meta.model`). `session/load` resumes one when the harness supports
`resume`.

What each part does for an external thread:

- **Prompt.** An ACP harness gets the prompt blocks unchanged. Codex gets
  text, images (as `data:` URLs) and file links as paths. A turn that fails
  (Codex `turn/completed` with `failed`, an ACP error, the process exiting)
  fails `session/prompt` with the harness's message. A harness that exited is
  started again by the next prompt.
- **Reply style and runtime info.** An external harness never sees
  RustCode's system prompt. It gets a
  `<runtime_info>` line (it runs in RustCode through that harness; embed
  images and link files by absolute path so the app opens them), the
  output-style rules ([output-style.md](output-style.md)) and, while the user
  has Smart UI on, the `rustcode-ui v1` fence it would otherwise never hear of
  ([smart-ui.md](smart-ui.md#external-harnesses)). Codex gets them as
  `developerInstructions` on `thread/start` and `thread/resume`, which Codex
  adds as a developer message after its own instructions. That field replaces
  a `developer_instructions` set in the user's Codex `config.toml` for that
  thread. An ACP agent that advertises `sessionCapabilities.systemPrompt`
  (Claude Code's adapter) gets them as `systemPrompt` in `append` mode on
  `session/new` and `session/load`, after its own system prompt. An agent with
  no such capability (pi-acp, OpenCode) gets the same text as a
  `<rustcode_runtime>` block at the head of the first prompt of a session
  RustCode created — once, never after `session/load`. It never reaches the
  thread's history, title or search index, and the agent's own echo of it as
  `user_message_chunk` is stripped.
- **Native subagents.** A `claude-code` session RustCode starts with the
  orchestrator attached gets
  `_meta.claudeCode.options.disallowedTools: ["Task", "Agent"]` on
  `session/new` and `session/load` — the shape `claude-agent-acp` forwards to
  the Claude Agent SDK and merges with its own list. The thread delegates
  through `start_session` and `fan_out` instead, so every subagent is a
  RustCode thread in the delegate UI. The rustcode MCP server is untouched.
  Without the orchestrator (orchestration off in Settings, or a platform
  where it has no socket) nothing is disallowed: the thread keeps its own
  subagent tool rather than losing delegation entirely. Another harness's
  session gets no `_meta` from RustCode.
- **Codex events.** Message deltas are `agent_message_chunk`, reasoning
  summaries `agent_thought_chunk`, command, file-change, MCP and web-search
  items `tool_call`/`tool_call_update` (command output and diffs as text
  content), `turn/plan/updated` a `plan`, `thread/tokenUsage/updated` a
  `usage_update` (`used` = the latest request's input tokens, `size` = the
  model's context window or 0 when Codex does not say, thread totals in
  `_meta.rustcode` as the built-in harness sends them). Codex's own
  subagents (`collabAgentToolCall`) become one agent row per receiver thread,
  the same shape as other harnesses' subagents (`_meta.claudeCode.toolName =
  "Agent"`), updated by wait, message, resume and close calls and ended by the
  subagent's own `turn/completed`, which no longer ends the parent turn; a
  subagent's text, plan and usage stay out of the thread. Command and file
  approvals are `session/request_permission` with Allow / Allow for this
  thread / Deny (`accept`, `acceptForSession`, `decline`); a cancelled
  request answers `cancel`. `item/tool/requestUserInput` asks each question
  as a RustCode question (a `session/request_permission` with
  `_meta.rustcode.kind = "question"`, see `acp/questions.rs`), one at a time;
  the selected option or free text is the answer, and dismissing one fails
  the request. Secret questions are asked the same way (no masking). Codex
  asks before an MCP tool runs with an elicitation
  (`mcpServer/elicitation/request`) whose form has no fields: for the
  injected `rustcode` server it is accepted (RustCode asks the user itself),
  for any other server it becomes an approval card (Allow, Allow for this
  thread as `_meta.persist = "session"`, Deny). Elicitations with form
  fields or a URL are declined, since RustCode draws no form; dynamic tool
  calls are refused.
- **ACP events.** The agent's `session/update`s (its `usage_update`s too)
  are relayed under the thread's id. RustCode advertises no `fs` or `terminal` client capability,
  and refuses those requests, so the agent uses its own tools.
- **Steer.** `_rustcode/steer` on a Codex thread sends `turn/steer` while a
  turn runs, with the message's images too. On an ACP thread it uses the
  agent's `_session/steering` extension when the agent advertised it at
  `initialize` (`_meta.steering.supported`). Claude Code steers live that way:
  its adapter pushes the message onto the SDK's `--input-format stream-json`
  input itself, so RustCode writes nothing into that pipe — the child's stdin
  is the ACP channel, and a raw stream-json line there would break it. An
  agent that advertises no steering waits for the next turn instead (Gemini
  CLI, OpenCode, and `pi-acp` 0.0.34, which queues a prompt sent mid-turn
  behind the running one even though Pi itself has steering modes). The same
  wait applies to the built-in harness for a message with pictures, and to it
  alone while an approval or a question waits on the user: it reads a steer
  between steps only, where Codex and an ACP agent take one with the card up.
  Each harness reports all of that as `steering`, and the client decides once
  from it — steer, queue, or stop the turn and send the message in its place —
  instead of branching on the harness id (`state/queue.ts` in the desktop app).
- **Load.** `session/load` replays the recorded transcript (messages,
  thoughts, each tool call as one finished card with its title, kind,
  status, locations and output summary, each turn's last plan) and then the
  thread's latest `usage_update`; the harness
  starts with the next prompt and resumes its own session: Codex with
  `thread/resume`, an ACP agent with `session/load` when it advertises
  `loadSession` (its own replay is not sent again). An agent that cannot
  load starts a fresh session, without the earlier context, and its
  `resume` capability reads `false` once the engine has run it.
- **Capabilities.** `images`, `effort` and `resume` of an ACP harness are
  updated from its `initialize` answer and options the first time it runs,
  and kept in the model cache. A harness whose listed models offer efforts
  has `effort: true`.
  `permission_modes` is empty for external harnesses: each applies its own
  configured approval rules.

### Recorded tool output

A thread on an external harness records each tool call in its
`history.json` the way a built-in turn does: one `tool` message with the
output in `content`, the exit code in `tool_result.exit_code`, and a long
output cut to the shared limits with the full text saved under
`tool_output/` (`full_output_artifact`). The output is taken wherever the
harness delivers it, because adapters differ:

- **`content`**: the card text and diffs a harness sends with
  `tool_call`/`tool_call_update` (Codex's commands and file changes,
  Claude Code's rendered calls, Opencode).
- **`_meta.terminal_output.data`**: Pi's shell command output, which arrives
  only as deltas over a pseudo-terminal, with its exit code in
  `_meta.terminal_exit.exit_code`. Deltas are appended, not replaced.
- **`rawOutput`**: the structured result Codex sends for `mcpToolCall` and
  other non-terminal items, and the content blocks Claude Code's adapter
  sends for calls its renderer does not describe. Only text is kept: images,
  audio and other binary blocks are left out.

`read_session` still gives a thread's turns without tool output, and the
search index (`thread_index.rs`) still indexes only user and assistant
messages, so neither grows with this.

## Switching a thread's harness

`_rustcode/session/harness` `{ "sessionId", "harness", "model"?, "effort"? }`
→ `{ "configOptions", "_meta" }` moves a thread to another external harness
(`harness/handoff.rs`). The old harness
stops; `harness.json` names the new one, without a native session, with
`handoff_from` set. Its first prompt carries the conversation so far, newest
back to about 16k tokens (tool results cut short), as
`<context>…</context>\n\nUser message:\n` before the user's text; then
`handoff_from` clears. The history gets a `handoff` message, which replays as
an `agent_message_chunk` with `_meta.rustcode.kind == "handoff"` (`from`,
`to`, `model`) and is never sent to a model; the app draws it as a divider.
A built-in RustCode thread can move to an external harness; moving one to
RustCode is not supported yet. A running turn must stop first. In the app,
the composer's model picker › Harness switches an open thread.

## Compaction

Each harness compacts its own context with its own command; the desktop app
decides when to ask, but it never writes the summary itself. `/compact` on its
own asks for a compaction now: the Codex adapter calls Codex's `thread/compact/start`
instead of sending the text to the model, and an ACP harness (Claude Code,
OpenCode) gets `/compact` as its prompt and runs its own command. Codex's
`contextCompaction` item, whether asked for or automatic mid-turn, becomes
the engine's compaction card (`_meta.rustcode.kind = "compaction"`), which
the desktop app shows as one quiet "Context compacted" row. The context
panel has *Compact now*, which sends `/compact`.

A compaction shows while it runs, not only after (#3251). The same chunk
carries `_meta.rustcode.phase`: `started` ("Compacting context…") when the
summarizer begins, then `completed` (the card; the default when a harness sends
no phase) or `failed` with the reason as its text. The desktop app draws one
row for it and the card or the failure takes that row's place. The built-in
harness sends `started` for `/compact` and for its automatic compaction, and
`failed` when a summary ends without compacting; Codex's `item/started`
`contextCompaction` is `started`. No start notice from Claude Code or another ACP agent is mapped (their
adapters send none that RustCode reads), so for them the row comes from the
app's own `/compact`, shown the moment the app sends it.

The desktop app can also decide to compact on its own (#3170): after a turn
above its size threshold, and in the minute after a prompt cache is due to
expire (about 4.5 minutes after a turn) when the context is above 60k. It
sends the same `/compact`, so each harness compacts with its own command.
Claude Code uses both rules, Codex only the cache rule (it already compacts
inside a turn), and the built-in harness keeps its own pre-request compaction.
An ACP agent that does not advertise `/compact` is skipped.

## Configuration

`~/.config/rustcode/config.toml`:

```toml
[harness_defaults]
harness = "codex"
model = "gpt-6-luna"
effort = "low"
silence_timeout_minutes = 10   # optional; 0 turns the silence deadline off
idle_timeout_minutes = 30      # optional; 0 keeps every harness process running
max_live_harnesses = 12        # optional; 0 turns the live-process cap off
onboarded = true

[[harnesses]]
id = "codex"
enabled = true
model = "gpt-6-luna"   # last new-thread choice (_rustcode/harnesses/remember)
effort = "high"

[[harnesses]]
id = "rustcode"        # the built-in harness's last choice
model = "chatgpt/gpt-6-luna"

[[harnesses]]
id = "my-agent"
name = "My agent"
kind = "acp"
command = ["my-agent", "--acp"]
env = { MY_AGENT_PROFILE = "work" }   # optional; added to the process's environment
enabled = true

# A named login of a harness, each its own config directory
# (Claude Code and Codex).
[[harness_accounts]]
id = "paral-2"
harness = "claude-code"
name = "Paral 2"
config_dir = "~/.claude-paral-2"
```

Known harnesses (`rustcode`, `codex`, `claude-code`, `gemini`, `opencode`, `pi`)
need only `id` and `enabled`; their commands are built in and can be
overridden with `command`. An entry without `enabled` is on. `model` and
`effort` are optional on any entry (older configs without them load
unchanged). An entry whose id is a known harness, such as a `pi` entry written
by hand before Settings listed it, *is* that harness: it is listed once, with
the user's `command` and `enabled`, never as a second, custom row. Any other id
needs a `command` and is a custom harness, which Settings shows under "Added by
hand". Both sections are optional and only read from the user config: a
project's `.rustcode/config.toml` cannot add a harness or change a command.

`silence_timeout_minutes` is the turn silence deadline below: 10 minutes by
default, `0` turns it off. It is read when a turn starts, so it applies to
the next message of a thread already open.

`idle_timeout_minutes` is the idle timeout of a harness process, 30 minutes by
default: a thread that has had no turn, no request waiting on the user and no
client activity for that long has its process stopped, and its next turn starts
a new one that resumes the harness's own session (see *Process and safety
rules*). `0` keeps every process running. `max_live_harnesses` caps how many
harness processes run at once, 12 by default; above it the least recently used
idle ones are stopped in the same way, `0` turns the cap off. Both are read on
every reaper pass, a minute apart, so a change applies without a restart.

Named accounts are chosen by hand. There is no pooling or rotation: a
thread uses the account it was given until the user picks another, and
limits are that account's own. Token totals need no account handling: Codex
and Claude Code report them over the harness protocol, and the log scans in
`harness_usage.rs` already take the account's own directory for the harnesses
that keep a log (Claude Code, Pi). Codex plan windows are the account's own:
`provider_auth/codex_usage.rs` starts the Codex CLI with that account's
`CODEX_HOME` and keeps one answer per home, so a thread on `~/.codex-paral`
shows its own windows. Settings › Usage's ChatGPT row is the Mac's own
`~/.codex` login.

The terminal UI starts every thread on the built-in harness with its own
`[default]` model and does not read the per-harness choice; it has no
new-thread harness picker. The desktop app reads `last` from the snapshot
and, against an engine without `_rustcode/harnesses/remember`, keeps the
choices in its own storage instead.

## Process and safety rules

- One child process per external thread, in its own process group, started
  in the thread's working directory. Closing the thread, `session/close`, or
  the engine exiting kills the group.
- `session/cancel` asks the harness to stop; a harness that does not stop in
  5 s is killed and the turn ends as cancelled.
- A turn that sends nothing for 10 minutes with no tool call in flight ends
  as failed ("No output from <harness> for 10 minutes. Send a message to try
  again.") and its process is killed; the next message starts a new one that
  resumes the harness's session, so the conversation stays intact. Silence is
  expected while work runs: an unfinished tool call (a build, a test run, a
  `wait_session`), a permission request or question waiting on the user, and
  the harness's own background tasks hold the deadline, and every message the
  harness sends — a thought or a usage update included — restarts it. It is
  reported as a `harness.turn_silent` `[op]` event in the thread's
  `logs/debug.log`. `[harness_defaults].silence_timeout_minutes` tunes it.
- An idle process is stopped and resumed later, so many open threads do not
  keep many processes: a thread with no turn running, no approval or question
  waiting on the user and no background work, and nothing from the client or
  the harness for 30 minutes, has its harness stopped the way closing the
  thread stops it. A prompt, a load, every update the harness sends and an
  answer to a card all restart the timer, and nothing is reaped during a turn
  or while the user is being asked. The thread is asleep, not crashed: the
  transcript, the harness's own session id, its account, model and effort
  stay in `harness.json`, the client is told nothing, and the next message
  starts a new process that resumes that session exactly as one restarts
  after a dead harness. `max_live_harnesses` (12 by default) also stops the
  least recently used idle processes when more than that run. It is reported
  as `harness.reaped` and, on the next start, `harness.resumed` `[op]` events
  in the thread's `logs/debug.log`; `[harness_defaults].idle_timeout_minutes`
  and `max_live_harnesses` tune it, each `0` turning its half off.
- The child's stderr goes to the thread's `logs/debug.log` (lines prefixed
  `[harness <id>]`), never to the transcript.
- The engine passes no RustCode credentials, provider keys or tokens to a
  child. The child inherits the user's environment (without the engine's own
  `RUSTCODE_*` variables) and uses its own sign-in.
- RustCode's sandbox applies to the built-in harness only. An external
  harness enforces its own rules, and the client says so.
- Permission requests from a harness go to the client unchanged; nothing is
  auto-approved unless the user picked a mode the harness enforces.
- Thread metadata (harness id, account, native session id, cwd, title, model,
  effort, timestamps) is stored with the thread in
  `~/.config/rustcode/threads/…/harness.json`. The transcript is recorded in
  the thread's `history.json` too (the user's messages, the harness's
  replies with its thoughts, its tool calls and plans), so the thread list,
  search and history work even when the harness cannot resume. A tool call
  is an `assistant` message naming it (`tool_calls[].name` = its title,
  `arguments` = its raw input) and a `tool` message whose
  `tool_result.tool_name` is its ACP kind and whose content is the text its
  card showed, capped at 4 KB. A call still running when the turn ends is
  recorded as failed. A plan is a `tool` message with `tool_name` `plan`
  and a `- [x] / [~] / [ ]` checklist. The latest usage is kept in
  `harness.json` (`usage`).

## MCP servers and skills

A thread on an external harness runs with the workspace's MCP servers: the
engine passes the enabled `[[mcp_servers]]` entries of the user and project
config to the harness's own session (ACP `mcpServers`, or Codex's
`mcp_servers.<name>` config), so Settings → MCP Servers is the one list for
every harness and nothing is written into the other tools' config files.

Skills are shared in the read direction, and never written into another tool's
directory (#2898). RustCode scans a harness's own roots next to its own:

| Harness | Skills root it reads itself | What RustCode does |
| --- | --- | --- |
| Built-in | RustCode's roots ([configuration.md](configuration.md#skills)) | Discovers and offers them through `list_skills` / `use_skill` |
| Claude Code | `.claude/skills` in the project, `~/.claude/skills`, and `<CLAUDE_CONFIG_DIR>/skills` of each named login | Reads all of them, so Settings › Skills lists what each login installed; Claude Code keeps reading them itself, so these threads get no note |
| Pi | `~/.agents/skills`, `.agents/skills` | Reads the same roots (`universal` and project), so one list covers Pi and RustCode |
| Codex | No skills directory: `AGENTS.md` and prompts | Gets the note below, naming the roots to read |
| Gemini CLI | No skills directory: extensions and commands | Gets the note below, naming the roots to read |
| OpenCode | No skills directory: its own agent/plugin model | Gets the note below, naming the roots to read |

Every external-harness thread's instructions carry one short `# Skills`
section (`harness_skills_note`): the roots that exist on this machine, `read
<root>/<name>/SKILL.md` with the harness's own tools, and the names turned off
in RustCode (do not use them, even where the harness's own settings list them).
It is bounded to the roots and disabled names, never the whole catalog,
because that text is paid on every request, and it is capped at 1,500 bytes:
long lists end in "and N more" so a machine with many roots or very long paths
still pays a fixed, small amount per turn. A Claude Code thread is told only
about the roots RustCode owns: it loads its own `.claude/skills` roots with its
own Skill tool, so those are never named to it, and with nothing but harness
roots installed it gets no note at all.

The note is computed when RustCode starts that thread's harness process, and
again whenever it starts one again — after the idle reaper stopped it, for
example, `ensure_started` resumes the harness's own session with a fresh note.
So a toggle reaches every new thread at once, and an already-running process
only when it is restarted.

Settings › Skills groups a harness's skills under its own heading and lists
their paths, and says there that the folder is read only; those rows have no
Edit SKILL.md button, only Show in Finder. Nothing is copied, linked or removed
in `~/.claude` or any other harness directory, and no setting has to be turned
on for the list to be complete. A harness skill that a higher-precedence root
shadows by name is not listed a second time: the group shows the definition
that wins, and `rustcode doctor` lists the roots that were searched.
