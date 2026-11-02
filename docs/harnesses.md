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
- **Agent**: a running thread or subagent.

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
  "rustcode_extensions": false               // PRs, usage, preview… (built-in only)
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
    "error": null               // last detection/start error, if any
  }],
  "default": { "harness": "rustcode", "model": null, "effort": null },
  "onboarded": false            // first-run onboarding finished
}
```

`models` may be empty until `_rustcode/harnesses/models` ran for that harness.

### `_rustcode/harnesses/models` `{ "harness": "codex" }` → `{ "models": [...] }`

Starts the harness briefly if needed to list its models. Cached in memory.

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

### `_rustcode/onboarding/complete` `{}` → `HarnessSnapshot`

Marks first-run onboarding done (`onboarded: true`).

### Starting a thread on a harness

`session/new` with:

```jsonc
"_meta": { "rustcode": { "harness": "codex", "model": "gpt-6-luna", "effort": "low" } }
```

No `harness` (or `"rustcode"`) starts a built-in thread. `model` and `effort`
fall back to the harness defaults. The response carries:

```jsonc
"_meta": { "rustcode": { "harness": { "id": "codex", "capabilities": { … } } } }
```

and the usual `configOptions` (`model`, `effort` when supported), so model
and effort switch with `session/set_config_option` for every harness.

After that the thread uses standard ACP: `session/prompt`, `session/cancel`,
`session/update` (agent text, thoughts, tool calls and their output, plans,
usage), `session/request_permission`. `session/list` returns external threads
too, each with `_meta.rustcode.harness`. `session/load` resumes one when the
harness supports `resume`.

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

[[harnesses]]
id = "my-agent"
name = "My agent"
kind = "acp"
command = ["my-agent", "--acp"]
enabled = true
```

Known harnesses (`rustcode`, `codex`, `claude-code`, `gemini`, `opencode`)
need only `id` and `enabled`; their commands are built in and can be
overridden with `command`.

## Process and safety rules

- One child process per external thread, in its own process group, started
  in the thread's working directory. Closing the thread, `session/close`, or
  the engine exiting kills the group.
- `session/cancel` asks the harness to stop; a harness that does not stop in
  5 s is killed and the turn ends as cancelled.
- The child's stderr goes to the thread's `logs/debug.log`, never to the
  transcript.
- The engine passes no RustCode credentials, provider keys or tokens to a
  child. The child inherits the user's environment and uses its own sign-in.
- RustCode's sandbox applies to the built-in harness only. An external
  harness enforces its own rules, and the client says so.
- Permission requests from a harness go to the client unchanged; nothing is
  auto-approved unless the user picked a mode the harness enforces.
- Thread metadata (harness id, native session id, cwd, title, timestamps)
  is stored with the thread in `~/.config/rustcode/threads/…`. The transcript
  itself is recorded too, so the thread list and history work even when the
  harness cannot resume.
