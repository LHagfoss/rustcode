# Terminology

What RustCode calls the pieces of a conversation, for the people using it and
in code (#2632). The short version: a conversation is a **thread**, in the
apps, the CLI and on disk. Only the Agent Client Protocol keeps its own name,
**session**, because other ACP clients depend on it.

## The parts, by name

| Name | Means |
| --- | --- |
| **RustCode** | The orchestrator: the app and the CLI that own your threads, projects, settings and usage data, and run every thread on a harness. |
| **App** | The desktop app in `apps/desktop` (Tauri + React). The main way to use RustCode. |
| **TUI** | The terminal front end in `rustcode/tui`, opened by `rustcode` in a terminal. Today it starts every thread on the built-in harness. |
| **Harness** | What runs a thread's turns: an external coding agent (Codex, Claude Code, Gemini CLI, OpenCode, Pi, any ACP agent) or RustCode's built-in harness. Chosen per thread. |
| **Built-in harness** | RustCode's own turn loop in `rustcode/engine/src/network/turn`, used by the TUI, local models and testing. It is what the harness id `rustcode` means. |
| **RustCode server** (Engine in code) | The host process and library in `rustcode/engine`, run as `rustcode --acp` behind the app: threads, tools, configuration, ACP, background jobs. The app calls it the **RustCode server** ("server" in short UI text: *Restart server*, *Server log*, Settings › Diagnostics). Code keeps `engine` (`engine.rs`, `restartEngine`, the `rustcode/engine` crate, `engine_*` commands): no crate or binary is called server, and `rustcode serve` and the remote gateway are separate processes. |

RustCode is the only product here. The older separate repositories
`rustcode-app` and `rustcode_app` are not this product, and nothing in this
repository depends on them.

## Glossary

| Term | Means | Where it shows |
| --- | --- | --- |
| **Thread** | A conversation the user sees and returns to: its messages, its project folder, its model, its history, logs and folder under `~/.config/rustcode/threads/`. | macOS and iOS sidebars and menus, the TUI, the CLI (`rustcode threads …`), storage, logs. |
| **Session** | The provider's own handle on a conversation: an ACP `sessionId` or a Codex thread id. In RustCode it is a *binding* to a thread, not the app's object ([the execution graph](architecture/execution-graph.md)); today one thread still has exactly one. | ACP (`session/new`, `session/load`, `sessionId`), and internal code identifiers that have not been renamed. |
| **Turn** | One user message and everything the agent does to answer it, until the answer ends or the user stops it. | "Worked for 1m 29s", the turn summary, Stop. |
| **Round** | One model request inside a turn; tool calls run between rounds. | Logs and `turn.summary` events (`rounds`), not the UI. |
| **Tool call** | One action the agent takes in a round (read, edit, run a command). | The thread's steps. |
| **Run** | A row of consecutive tool calls the thread folds into one line. | "Ran 3 commands", "Show steps". |
| **Agent** (subagent) | A helper the thread's agent starts for a delegated task. It has its own conversation but is not a thread: it is not listed in the sidebar by itself and ends with its parent. | A row in the thread where it started, `spawn_agent`. |
| **Delegate** | A thread working for another thread (`start_session`, `fan_out`, `message_session`), which the orchestrating thread is told about when it finishes or asks. It is a thread of its own, listed in the sidebar; a *link* in code. | The wake notice ("Delegate finished: …"), `link_id`, the orchestration tools and docs. |
| **Harness** | What runs a thread's turns: RustCode's built-in harness or an external coding agent such as Codex or Claude Code. Chosen when the thread starts ([harnesses.md](harnesses.md)). | The new-thread picker, onboarding, Settings; `_meta.rustcode.harness` in ACP. |
| **Provider** | A model API the built-in harness calls (OpenAI, Anthropic, Copilot, an OpenAI-compatible server). An external harness has its own models and sign-in, not RustCode's providers. | Settings → providers, `[[providers]]` in the config. |
| **Environment** | A computer running RustCode that this app can run threads on: this computer, or one paired over a connect code, SSH or an address. The app says *environment* (Settings › Environments, *Add environment*, the Run on selector), after T3 Code. Code, the shell and the protocol keep `machine` (`Machine`, `machines_*`, `machines.json`, `machines.rs`, `?machines=`). | Settings › Environments, the new-thread Run on selector, the Add environment dialog. |
| **Plan** | The agent's task list (`todo_write`): what it proposes to do, step by step. | The task plan; never proof that a step ran. |

## Harness, provider, agent

These three are easy to mix up:

- A **harness** runs the thread. Pick Codex as the harness and Codex's own
  loop, tools, sign-in and models answer every turn; RustCode only shows the
  thread and keeps its transcript.
- A **provider** is where the built-in harness sends its model requests. It
  matters only when the harness is `rustcode`.
- An **agent** is a subagent a thread starts. Say "Codex" or "the harness",
  not "the Codex agent", for an external harness, so "agent" keeps meaning
  the rows under a thread.

## Session = the provider's handle

A thread is the conversation; a session is the provider's handle on it. Today
they coincide — one thread, one id, one folder, one ACP session — and the two
words are used for the same thing. That is an implementation detail, not the
model: [#2882](https://github.com/LHagfoss/rustcode/issues/2882) gives a thread
1..n provider threads (a harness or provider switch adds one), and a session
then names only the binding. The domain model is
[the execution graph](architecture/execution-graph.md).

Where both words appear today they mean the same:

| You see | Means the same as | Where |
| --- | --- | --- |
| Thread, thread id | Session, session id (`sessionId`) | ACP (`session/new`, `session/load`) |
| `~/.config/rustcode/threads/` | `~/.config/rustcode/sessions/` (a link to it) | Storage |
| `rustcode threads …` | `rustcode sessions …` (alias) | CLI |
| `thread_id`, `list_threads`, `kind: "threads"` | `session_id`, `list_sessions`, `kind: "sessions"` | Remote protocol, when the device asks for thread names ([details](remote-protocol/README.md#thread-names)) |
| "Thread status", "Resume thread" | "Session status", "Resume session" | Terminal UI; old transcripts still say session and render the same |
| `/session` | (unchanged command name) | Slash commands keep their names |
| `SessionStore`, `selectedSession`, `session_id` | (unchanged) | Internal code identifiers |

## Environment = machine

The apps call a computer a thread can run on an **environment**, as T3 Code
does. The word is user-facing only: `Machine`, `useMachines`, the
`machines_*` shell commands, `machines.json`, `rustcode machines …` and the
`machine` field new threads accept keep their names, so no stored file, CLI
script or protocol client changes. Write *environment* in UI text and docs
about the app, and *machine* in code and the CLI.

## What stays "session"

- **ACP.** `session/*` and `sessionId` are the
  [Agent Client Protocol](acp.md)'s own names, shared with other ACP clients
  (editors that talk to `rustcode acp`). Renaming them would break those
  clients, so they stay.
- **Internal identifiers.** Types and functions such as `SessionStore` or
  `selectedSession` keep their names; renaming thousands of identifiers buys
  nothing a user sees. New code may say thread.
- **Old names keep working.** `rustcode sessions …` stays as an alias of
  `rustcode threads …`, `~/.config/rustcode/sessions` is a link to
  `threads/` after the migration, and the remote protocol keeps its v1 names
  for devices that do not ask for thread names.

## Ambiguous references

- **Agent conversations** are "the agent's conversation" (or "agent-2's
  conversation"), never "thread", so a helper inside a thread is not mistaken
  for a thread of its own.
- **Thread ids** are what `--resume` takes and what "Copy Thread ID" copies;
  they are the same strings as ACP session ids.

## Rollout

All done:

1. Storage: `~/.config/rustcode/sessions/` is renamed to `threads/` by the
   first start of a new version (storage migration
   `0001-sessions-folder-to-threads`, see `rustcode storage status`), with a
   `sessions` link left for older versions.
2. CLI: `rustcode threads …`, with `sessions` as an alias.
3. Apps: the macOS, iOS and terminal UIs say "thread".
4. Remote protocol (engine ↔ iOS): a device that asks for thread names in its
   handshake gets them; an older app, or an older gateway, keeps the v1
   names. The current iOS app asks.

Still "session" on purpose: ACP, slash command names (`/session`), internal
identifiers, and log and event names (`session_id` in `operational_event`).
