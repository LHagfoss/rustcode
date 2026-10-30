# Terminology

What RustCode calls the pieces of a conversation, for the people using it and
in code (#2632). The short version: a conversation is a **thread**, in the
apps, the CLI and on disk. Only the Agent Client Protocol keeps its own name,
**session**, because other ACP clients depend on it.

## Glossary

| Term | Means | Where it shows |
| --- | --- | --- |
| **Thread** | A conversation the user sees and returns to: its messages, its project folder, its model, its history, logs and folder under `~/.config/rustcode/threads/`. | macOS and iOS sidebars and menus, the TUI, the CLI (`rustcode threads …`), storage, logs. |
| **Session** | The Agent Client Protocol's name for a thread. One thread is one ACP session. | ACP only (`session/new`, `session/load`, `sessionId`), and internal code identifiers that have not been renamed. |
| **Turn** | One user message and everything the agent does to answer it, until the answer ends or the user stops it. | "Worked for 1m 29s", the turn summary, Stop. |
| **Round** | One model request inside a turn; tool calls run between rounds. | Logs and `turn.summary` events (`rounds`), not the UI. |
| **Tool call** | One action the agent takes in a round (read, edit, run a command). | The thread's steps. |
| **Run** | A row of consecutive tool calls the thread folds into one line. | "Ran 3 commands", "Show steps". |
| **Agent** (subagent) | A helper the thread's agent starts for a delegated task. It has its own conversation but is not a thread: it is not listed in the sidebar by itself and ends with its parent. | Rows nested under the thread, the Agents panel, `spawn_agent`. |
| **Plan** | The agent's task list (`todo_write`): what it proposes to do, step by step. | The task plan; never proof that a step ran. |

## What stays "session"

- **ACP.** `session/*` and `sessionId` are the
  [Agent Client Protocol](acp.md)'s own names, shared with other ACP clients
  (editors that talk to `rustcode acp`). Renaming them would break those
  clients, so they stay.
- **Internal identifiers.** Types and functions such as `SessionStore` or
  `selectedSession` keep their names; renaming thousands of identifiers buys
  nothing a user sees. New code may say thread.
- **Old names keep working.** `rustcode sessions …` stays as an alias of
  `rustcode threads …`, and `~/.config/rustcode/sessions` is a link to
  `threads/` after the migration.

## Ambiguous references

- **Agent conversations** are "the agent's conversation" (or "agent-2's
  conversation"), never "thread", so a helper inside a thread is not mistaken
  for a thread of its own.
- **Thread ids** are what `--resume` takes and what "Copy Thread ID" copies;
  they are the same strings as ACP session ids.

## Rollout

1. Storage: `~/.config/rustcode/sessions/` is renamed to `threads/` by the
   first start of a new version (storage migration
   `0001-sessions-folder-to-threads`, see `rustcode storage status`), with a
   `sessions` link left for older versions.
2. CLI: `rustcode threads …`, with `sessions` as an alias.
3. Apps: the macOS, iOS and terminal UIs say "thread".
4. Remote protocol (engine ↔ iOS): new field names are accepted alongside the
   old ones first, and only emitted once the installed iOS app reads them.
