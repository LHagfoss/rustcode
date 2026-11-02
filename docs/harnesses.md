# Harnesses

A **harness** is the agent runtime that runs a thread. RustCode's own engine is
the built-in harness, and it stays the default with the most features. Other
coding agents — Codex, Claude Code, Gemini CLI, OpenCode, or any ACP agent —
can run a thread as external harnesses. Each owns its own sign-in, model
catalog, tools and permission rules. RustCode never stores their credentials.

Terms (see [terminology.md](terminology.md)):

- **Provider**: a model API the built-in harness calls (OpenAI, Anthropic,
  Copilot, an OpenAI-compatible server). Unchanged by harnesses.
- **Harness**: what runs a thread's turns. Chosen when the thread starts.
- **Agent**: a subagent a thread starts (see terminology.md).

## Where it lives

The harness layer is in the engine (`rustcode/engine/src/harness/`), not in a
frontend. Every client of `rustcode --acp` (the desktop app, the SwiftUI app)
gets external harnesses through the same ACP connection it already uses.

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
| `builtin` | RustCode's turn loop | `rustcode` |
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
  "steer": "live" | "next_turn" | "none",   // _rustcode/steer
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
    "last": { "model": "gpt-6-luna", "effort": "high" },  // last new-thread choice, or null
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
harnesses. Detection is cached for a minute; `set` and `remove` refresh it.

| id | kind | command | version from | auth from |
| --- | --- | --- | --- | --- |
| `codex` | `codex` | `codex app-server` | `codex --version` | `codex login status` |
| `claude-code` | `acp` | `npx -y @agentclientprotocol/claude-agent-acp` | `claude --version` | `unknown` |
| `gemini` | `acp` | `gemini --experimental-acp` | `gemini --version` | `unknown` |
| `opencode` | `acp` | `opencode acp` | `opencode --version` | `unknown` |

Commands are looked up on the engine's `PATH` and then the login shell's, so
an app started from the Dock finds what the terminal finds.

Errors (an unknown id, an invalid command, removing a known harness) are
`invalid_params` with a message for the user.

### `_rustcode/harnesses/models` `{ "harness": "codex" }` → `{ "models": [...] }`

Starts the harness briefly if needed to list its models. Codex answers from
`model/list` (hidden models left out, efforts per model). An ACP harness is
started in the system temp directory and one session is opened there to read
its `model` and thought-level options; every model then shows the same
efforts, and `default_effort` is the harness's own (read before a thread's
choice is applied).

**Model cache.** Every listing (this request, or a thread starting the
harness) is kept in memory and in `~/.config/rustcode/harness-models.json`:
per harness its models, `fetched_at` (Unix seconds), the harness `version`
it came from, and the learned `effort`, `resume` and `images` capabilities. A
restarted engine reads it, so the snapshot, the capabilities and the
`session/load` answer of a restored thread (its `model` and `effort`
options, before the harness starts again) are complete. A cached list older
than 24 hours, or from another harness version, is answered at once and
listed again in the background. A failed listing is not cached: the next
request tries again (one listing per harness runs at a time).

### `_rustcode/harnesses/set` → `HarnessSnapshot`

```jsonc
{ "id": "codex", "enabled": true, "command": ["codex", "app-server"] }
```

Adds a custom ACP harness when `id` is new (`kind` defaults to `acp`; pass
`name`, `kind`, `command`). `command` and `name` are optional on existing ones.

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

### `_rustcode/onboarding/complete` `{}` → `HarnessSnapshot`

Marks first-run onboarding done (`onboarded: true`).

### Starting a thread on a harness

`session/new` with:

```jsonc
"_meta": { "rustcode": { "harness": "codex", "model": "gpt-6-luna", "effort": "low" } }
```

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
"_meta": { "rustcode": { "harness": { "id": "codex", "capabilities": { … } } } }
```

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
- **Reply style.** An external harness never sees RustCode's system prompt.
  Codex gets the output-style rules
  ([output-style.md](output-style.md)) as `developerInstructions` on
  `thread/start` and `thread/resume`, which Codex adds as a developer message
  after its own instructions. That field replaces a `developer_instructions`
  set in the user's Codex `config.toml` for that thread. ACP has no
  system-prompt field, so an ACP harness gets none: wrapping the first prompt
  (what T3 Code does) would show in the thread's own history and in the
  agent's replay, and `session/new` `_meta` is not read by any agent.
- **Codex events.** Message deltas are `agent_message_chunk`, reasoning
  summaries `agent_thought_chunk`, command, file-change, MCP and web-search
  items `tool_call`/`tool_call_update` (command output and diffs as text
  content), `turn/plan/updated` a `plan`, `thread/tokenUsage/updated` a
  `usage_update` (`used` = the latest request's input tokens, `size` = the
  model's context window or 0 when Codex does not say, thread totals in
  `_meta.rustcode` as the built-in harness sends them). Command and file
  approvals are `session/request_permission` with Allow / Allow for this
  thread / Deny (`accept`, `acceptForSession`, `decline`); a cancelled
  request answers `cancel`. `item/tool/requestUserInput` asks each question
  as a RustCode question (a `session/request_permission` with
  `_meta.rustcode.kind = "question"`, see `acp/questions.rs`), one at a time;
  the selected option or free text is the answer, and dismissing one fails
  the request. Secret questions are asked the same way (no masking). MCP
  elicitations (`mcpServer/elicitation/request`) are declined, since they
  need a form RustCode does not draw; dynamic tool calls are refused.
- **ACP events.** The agent's `session/update`s (its `usage_update`s too)
  are relayed under the thread's id. RustCode advertises no `fs` or `terminal` client capability,
  and refuses those requests, so the agent uses its own tools.
- **Steer.** `_rustcode/steer` on a Codex thread sends `turn/steer` while a
  turn runs (`steer: "live"`). ACP has no equivalent (`steer: "none"`): the
  client sends the message as the next prompt.
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

## Configuration

`~/.config/rustcode/config.toml`:

```toml
[harness_defaults]
harness = "codex"
model = "gpt-6-luna"
effort = "low"
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
enabled = true
```

Known harnesses (`rustcode`, `codex`, `claude-code`, `gemini`, `opencode`)
need only `id` and `enabled`; their commands are built in and can be
overridden with `command`. An entry without `enabled` is on. `model` and
`effort` are optional on any entry (older configs without them load
unchanged). Both sections are optional and only read from the user config: a
project's `.rustcode/config.toml` cannot add a harness or change a command.

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
- The child's stderr goes to the thread's `logs/debug.log` (lines prefixed
  `[harness <id>]`), never to the transcript.
- The engine passes no RustCode credentials, provider keys or tokens to a
  child. The child inherits the user's environment (without the engine's own
  `RUSTCODE_*` variables) and uses its own sign-in.
- RustCode's sandbox applies to the built-in harness only. An external
  harness enforces its own rules, and the client says so.
- Permission requests from a harness go to the client unchanged; nothing is
  auto-approved unless the user picked a mode the harness enforces.
- Thread metadata (harness id, native session id, cwd, title, model,
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
