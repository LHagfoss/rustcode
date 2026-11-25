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
| **Thread** | A conversation the user sees and returns to: its messages, its project folder, its model, its history, logs and folder under `~/.config/rustcode/threads/`. | macOS and iOS sidebars and menus, the TUI, the CLI (`rc threads …`), storage, logs. |
| **Session** | The provider's own handle on a conversation: an ACP `sessionId` or a Codex thread id. In RustCode it is a *binding* to a thread, not the app's object ([the execution graph](architecture/execution-graph.md)); today one thread still has exactly one. | ACP (`session/new`, `session/load`, `sessionId`), and internal code identifiers that have not been renamed. |
| **Turn** | One user message and everything the agent does to answer it, until the answer ends or the user stops it. | "Worked for 1m 29s", the turn summary, Stop. |
| **Round** | One model request inside a turn; tool calls run between rounds. | Logs and `turn.summary` events (`rounds`), not the UI. |
| **Tool call** | One action the agent takes in a round (read, edit, run a command). | The thread's steps. |
| **Run** | A row of consecutive tool calls the thread folds into one line. | "Ran 3 commands", "Show steps". |
| **Agent** (subagent) | A helper the thread's agent starts for a delegated task. It has its own conversation but is not a thread: it is not listed in the sidebar by itself and ends with its parent. | A row in the thread where it started, `spawn_agent`. |
| **Delegate** | A thread working for another thread (`start_session`, `fan_out`, `message_session`), which the orchestrating thread is told about when it finishes or asks. It is a thread of its own, listed in the sidebar; a *link* in code. | The wake notice ("Delegate finished: …"), `link_id`, the orchestration tools and docs. |
| **Harness** | What runs a thread's turns: RustCode's built-in harness or an external coding agent such as Codex or Claude Code. Chosen when the thread starts ([harnesses.md](harnesses.md)). | The new-thread picker, onboarding, Settings; `_meta.rustcode.harness` in ACP. |
| **Provider** | A model API the built-in harness calls (OpenAI, Anthropic, Copilot, an OpenAI-compatible server). An external harness has its own models and sign-in, not RustCode's providers. | Settings → providers, `[[providers]]` in the config. |
| **Machine** | A computer running RustCode that this app can run threads on: this computer, or one paired over a connect code, SSH or an address. Never *environment* ([Machine](#machine)). Code, the shell and the protocol say `machine` too (`Machine`, `machines_*`, `machines.json`, `machines.rs`, `?machines=`). | Settings › Machines, the new-thread Run on selector, the Add machine dialog, `list_machines`. |
| **Job** | An automated agent prompt run once, on a schedule, manually, or from a webhook by the scheduler daemon ([daemon.md](daemon.md)). Each run starts a fresh thread (or reuses one chosen thread) on any harness, read-only by default. | The sidebar's *Jobs* row, `rc jobs …` (aliases `loops`, `cron`), MCP tools `list_jobs`, `create_job`, `update_job`, `run_job`, `delete_job` (aliases `*_loop`). |
| **Needs you** | A thread that waits on the user: an approval or a question. The sidebar section, the Board column, a thread's status (Thread info, the chat's recap) all say *Needs you*; a compact sidebar row names which, *Approval* or *Question*. The transcript's working line keeps *Waiting for approval*, since it says what the agent is doing. Not *Needs input* or *Waiting on you*. | Sidebar, Board, Thread info › Status, the recap card. |
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
| `rc threads …` | `rc sessions …` (alias) | CLI |
| `thread_id`, `list_threads`, `read_thread` | `session_id`, `list_sessions`, `read_session` (aliases) | RustCode MCP |
| `thread_id`, `list_threads`, `kind: "threads"` | `session_id`, `list_sessions`, `kind: "sessions"` | Remote protocol, when the device asks for thread names ([details](remote-protocol/README.md#thread-names)) |
| "Thread status", "Resume thread" | "Session status", "Resume session" | Terminal UI; old transcripts still say session and render the same |
| `/session` | (unchanged command name) | Slash commands keep their names |
| `SessionStore`, `selectedSession`, `session_id` | (unchanged) | Internal code identifiers |

## Machine

A computer a thread can run on is a **machine**, in the apps, the docs, the
CLI and the code alike: Settings › Machines, *Add machine*, the Run on
selector, `Machine`, `useMachines`, the `machines_*` shell commands,
`machines.json`, `rc machines …`, the `machine` field new threads accept
and the `list_machines` orchestration tool. Do not call it an *environment*:
that word means a process's environment variables, or a harness's sandbox.

The apps said *environment* for a while (after T3 Code). What that left behind
keeps working: the fleet sync's wire method is still
`_rustcode/environment/sync`, so older paired computers answer it, and a
computer's own name written to `environment.json` by those versions is still
read until the next rename writes `machine.json`.

A machine's **name** is shared across the fleet: a rename on one computer
is stamped and reaches every paired computer, and the newest rename wins
([Machine names and the fleet](remote.md#machine-names-and-the-fleet)).
A machine this computer only *knows of* through a paired one is **known**,
not paired: it is listed read-only until it is paired on its own. The code says
`fleet` (`fleet.rs`, `fleet.json`, `state/fleet.ts`).

## What stays "session"

- **ACP.** `session/*` and `sessionId` are the
  [Agent Client Protocol](acp.md)'s own names, shared with other ACP clients
  (editors that talk to `rustcode acp`). Renaming them would break those
  clients, so they stay.
- **Internal identifiers.** Types and functions such as `SessionStore` or
  `selectedSession` keep their names; renaming thousands of identifiers buys
  nothing a user sees. New code may say thread.
- **Old names keep working.** `rc sessions …` stays as an alias of
  `rc threads …`, `~/.config/rustcode/sessions` is a link to
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
   `0001-sessions-folder-to-threads`, see `rc storage status`), with a
   `sessions` link left for older versions.
2. CLI: `rc threads …`, with `sessions` as an alias.
3. Apps: the macOS, iOS and terminal UIs say "thread".
4. Remote protocol (engine ↔ iOS): a device that asks for thread names in its
   handshake gets them; an older app, or an older gateway, keeps the v1
   names. The current iOS app asks.

Still "session" on purpose: ACP, slash command names (`/session`), internal
identifiers, and log and event names (`session_id` in `operational_event`).
