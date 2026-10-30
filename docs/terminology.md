# Terminology

What RustCode calls the pieces of a conversation, for the people using it and
in code (#2632). The short version: people see **chats**; code, files and
protocols say **session**. Neither is renamed to *thread*.

## Glossary

| Term | Means | Where it shows |
| --- | --- | --- |
| **Chat** | A conversation the user sees and returns to: its messages, its project folder, its model. | macOS and iOS sidebars, menus, toasts ("New Chat", "Search chats", "Mark as Done"). |
| **Session** | The engine's record of a chat: its id, history, logs and folder under `~/.config/rustcode/sessions/`. One chat is one session. | ACP (`session/new`, `session/load`, `sessionId`), the remote protocol, the CLI (`rustcode sessions list`, `--resume <id>`), storage, logs, "Copy Session ID". |
| **Turn** | One user message and everything the agent does to answer it, until the answer ends or the user stops it. | "Worked for 1m 29s", the turn summary, Stop. |
| **Round** | One model request inside a turn; tool calls run between rounds. | Logs and `turn.summary` events (`rounds`), not the UI. |
| **Tool call** | One action the agent takes in a round (read, edit, run a command). | The chat's steps. |
| **Run** | A row of consecutive tool calls the chat folds into one line. | "Ran 3 commands", "Show steps". |
| **Agent** (subagent) | A helper the chat's agent starts inside its own session for a delegated task. It has its own conversation but is not a chat: it is not listed in the sidebar by itself and ends with its parent. | Rows nested under the chat, the Agents panel, `spawn_agent`. |
| **Plan** | The agent's task list (`todo_write`): what it proposes to do, step by step. | The task plan; never proof that a step ran. |

## Why not *thread*

*Thread* was considered for the user-facing conversation (as in
[t3code](https://github.com/pingdotgg/t3code/blob/main/docs/user/thread-sidebar.md)).
It is not adopted:

- **The apps already say chat.** The macOS and iOS apps use "chat" for every
  user-facing string; switching to "thread" relabels everything for no new
  meaning.
- **"Thread" collides.** Agents, OS threads and message threads within a chat
  would all be "threads"; a subagent would read as a thread of a thread.
- **Session cannot change underneath.** `session/*` and `sessionId` are the
  [Agent Client Protocol](acp.md)'s own names, shared with other ACP
  clients; the remote protocol, the iOS app, the CLI's `sessions` commands and
  every stored `sessions/` folder use them. Renaming them would break other
  clients and older apps and move user data for no user-visible gain. The
  issue's own rule holds: no blanket internal rename.

## Ambiguous references

- **Agent conversations** are "the agent's conversation" (or "agent-2's
  conversation"), never "chat", so a helper inside a chat is not mistaken for
  a chat of its own.
- **Session ids** stay "session id" wherever a user copies or pastes one
  (Copy Session ID, `--resume`), because that is what the CLI and logs call it.
- **The terminal UI** still says "session" in some user-facing strings
  (`/session`, the picker). That is the remaining step below.

## Rollout

1. Done: this glossary; macOS and iOS already use "chat".
2. Next, optional: the TUI's user-facing labels say "chat" where they mean the
   conversation (picker titles, status lines), keeping `--resume`, `/session`
   and `sessions` subcommands as they are, with "chat" aliases if added.
   Help text names both once: "chat (session)".
3. Never: protocol fields, CLI commands, config keys, the `sessions/` folder
   or log names. A future change there needs its own migration plan.
