# Orchestrator v2

Builds on orchestration M1 ([cross-session-orchestration.md](architecture/cross-session-orchestration.md)):
the engine offers orchestration tools to an agent, forwards each call to the
client as a `_rustcode/orchestration` request, and the client (which hosts the
threads) checks the rules, asks the user, and carries the call out. v2 adds
worktree threads, thread status, starting threads on any harness, fan-out,
per-turn checkpoints, and the same tools for external harnesses.

## Worktree threads

A thread can run in its own git worktree, so parallel threads never edit the
same checkout.

### `_rustcode/worktrees/create` → `Worktree`

```jsonc
// request
{ "repo": "/abs/repo", "branch": "feat/login-form",   // optional; suggested from title when absent
  "title": "Add the login form", "base": "HEAD" }     // base: ref to branch from, default HEAD
// Worktree
{ "path": "/abs/worktrees/<repo-name>/<branch-slug>", "branch": "feat/login-form",
  "repo": "/abs/repo", "base": "abc123…", "created_at": "…" }
```

Worktrees live under `~/.config/rustcode/worktrees/<repo-name>/<slug>`
(outside the repo). Branch names are suggested from the title
(`<type>/<slug>`, lowercase, deduplicated with `-2`, `-3`).

### `_rustcode/worktrees/list` `{ "repo": "/abs/repo" }` → `{ "worktrees": [Worktree & { "dirty": bool, "ahead": n, "behind": n, "session_ids": [...] }] }`

### `_rustcode/worktrees/remove` `{ "path": "…", "force": false }` → `{ "removed": true }`

Refuses (`error.data.reason = "dirty"` or `"unpushed"`) when the worktree has
uncommitted changes or commits not on any remote branch, unless `force`.
Removes the branch only when it is merged into the base.

### Starting a thread in a worktree

`session/new` `_meta.rustcode.worktree = { "repo": "/abs/repo", "branch"?: "…", "title"?: "…" }`
creates the worktree first and uses its path as the session cwd. The response
`_meta.rustcode.worktree` carries the `Worktree`. `session/list` entries carry
`_meta.rustcode.worktree` (path, branch, repo) and `_meta.rustcode.status`.

## Thread status

`_meta.rustcode.status` on a thread: `"idle" | "running" | "needs_input" | "review" | "done"`.

- `running` while a turn runs, `needs_input` while it waits on an approval or
  question (engine-derived, not stored).
- `review` and `done` are set by the user or automatically, and stored with the
  thread: `_rustcode/session/status` `{ "session_id", "status": "review" | "done" | "idle" }` → `{}`.
- A thread whose worktree branch has a PR that merges becomes `done`
  automatically (engine checks via the existing PR watch, `_rustcode/pulls/watch`),
  and sends `session/update` with `_meta.rustcode.status`.

## Checkpoints

Before every turn of any thread (built-in or external harness; the engine sees
both), the engine snapshots the thread's cwd into a hidden ref
`refs/rustcode/checkpoints/<session_id>/<turn>` (a commit of the working tree
including untracked, non-ignored files, built with a temporary index; the
user's index, HEAD and stash are never touched). Not in a git repo → no
checkpoints, capability `checkpoints: false`.

- `_rustcode/checkpoints/list` `{ "session_id" }` → `{ "checkpoints": [{ "turn": 3, "ref": "…", "created_at": "…", "prompt": "first 80 chars", "files_changed": 2, "insertions": 10, "deletions": 1 }] }`
- `_rustcode/checkpoints/diff` `{ "session_id", "turn" }` → `{ "files": [{ "path", "status": "added|modified|deleted|renamed", "insertions", "deletions", "patch": "unified diff" }] }`
  — what that turn changed: its checkpoint vs the next one (or the working
  tree for the latest turn).
- `_rustcode/checkpoints/restore` `{ "session_id", "turn", "rewind": bool }` → `{ "restored": true }`
  — puts the files back to before that turn. With `rewind: true` it also calls
  the existing rewind for the conversation when the harness supports it
  (`capabilities.rewind`); otherwise files only, and the response says
  `"conversation": "kept"`.
- Refs are pruned when the thread is deleted, and kept to the last 50 turns.

## Orchestration tools v2

Built (`rustcode/engine/src/orchestration.rs`). Offered to root coding turns
while the client advertises `_meta.rustcode.orchestration >= 2` (and
delegation is on, as in M1). M1 clients send `1` and get exactly the M1
tools, descriptions and prompt hint; a version 2 call from a version 1
session (a made-up one) is refused in the engine and nothing is sent. Plan
mode and subagents never get any of them. Each call is forwarded as today:
`_rustcode/orchestration` `{ "tool": <name>, "args": {...}, "sessionId": <calling session> }`.

| Tool | Args | Answer |
| --- | --- | --- |
| `list_sessions` | M1 | M1 + `harness`, `status`, `worktree` |
| `read_session` | `{ session_id, last_turns?: 1..10 }` (default 3) | `{ title, status, turns: [{ user, assistant, files_changed }] }` (bounded, 16 KiB) |
| `start_session` | `{ prompt, title?, harness?, model?, effort?, worktree?: bool, project?: path }` | `{ session_id, link_id }` — the client asks the user once, creates the thread (in a new worktree when `worktree: true`, default true for a git project), sends `prompt` as its first message |
| `message_session` | M1 | M1 |
| `wait_session` | M1 (`link_id` / `link_ids`) | M1 + `status`, `files_changed` |
| `fan_out` | `{ prompt, harnesses: [{ harness, model?, effort? }] (2..4), title? }` | `{ links: [{ session_id, link_id, harness }] }` — one worktree thread per harness, same prompt; the user compares results in the app |

Rules (enforced by the client, as M1): same project unless `project` is given
and the user approves it; at most 4 active links per thread; no chains; the
user can stop any link; nothing starts without the user's approval (*Allow for
This Thread* / *Allow Once* / *Don't Allow*).

## External harnesses as orchestrators

Built (`rustcode/engine/src/orchestration/{hub,mcp_server}.rs`). When the
client advertises orchestration `>= 2`, an external harness thread gets the
same six tools through an MCP server the engine injects into its session:

```
rustcode mcp-orchestrator --session <thread id> --socket <path> --token <t>
```

a stdio MCP server (`tools/list`, `tools/call`, `ping`; server name
`rustcode`), started by the harness with the engine's own executable:

- ACP children: one stdio entry in `session/new` / `session/load`
  `mcpServers` (`{ name: "rustcode", command, args, env: [] }`).
- Codex: `thread/start` / `thread/resume` `config` override
  `"mcp_servers.rustcode": { command, args, tool_timeout_sec }`, for that
  thread only. `tool_timeout_sec` is 3660, since Codex stops MCP calls after
  60 s by default and `wait_session` may wait an hour.

Each tool call goes to the parent engine over a local unix socket,
`<config>/run/orchestrator-<pid>.sock` (socket 0600, directory 0700; sockets
of engines no longer running are removed). The engine binds it lazily, when
the first external thread with orchestration starts or loads. Wire format: one
connection per call, one JSON line `{ session_id, token, tool, args }` in, one
`{ success, content }` out; closing early cancels the call. The engine checks
that the session is registered and the token (256 random bits, one per thread,
dropped when the thread closes) matches, then forwards through the same
`_rustcode/orchestration` request a built-in call uses, with the external
thread's id as `sessionId`, so the client rules and approvals apply unchanged.
The MCP server also validates arguments, so a bad call fails without
reaching the engine.

Capability flag `orchestration` (see below): in the `session/new` /
`session/load` answer, `true` only when the server was injected. Not on
Windows (no unix socket): `false`.

## Capabilities added

`checkpoints: bool`, `worktree: bool` (always true for external harnesses
too, since the engine prepares the cwd), `orchestration: bool`: always `true`
for the built-in harness (the tools still need a version 2 client); for an
external one, `true` in the `_rustcode/harnesses` snapshot where the engine
can inject the server (unix), and in a thread's answer whether it did.
Checked in the engine before anything is sent (`validate`): `prompt` is
required and at most 8 KiB; `title` at most 200 characters; `harness`,
`model`, `effort` non-empty strings when given; `worktree` a boolean;
`project` an absolute path; `fan_out` takes 2 to 4 entries, each with a
`harness`, and no entry twice (the same harness on two models is fine).
The engine waits for the client's answer 15 min for `start_session` and
`fan_out` (approval, worktrees, harness start), `timeout_ms` + 30 s for
`wait_session`, and 10 min otherwise. Answers over 16 KiB are cut.

The version 2 prompt hint replaces the M1 one (about 80 tokens, tested).
