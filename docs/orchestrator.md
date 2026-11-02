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

Offered to root turns while the client advertises
`_meta.rustcode.orchestration >= 2` (M1 clients send `1` and keep the M1
tools only). Each call is forwarded as today: `_rustcode/orchestration`
`{ "tool": <name>, "args": {...} }` carrying the calling session id.

| Tool | Args | Answer |
| --- | --- | --- |
| `list_sessions` | M1 | M1 + `harness`, `status`, `worktree` |
| `read_session` | `{ session_id, last_turns?: 1..10 }` | `{ title, status, turns: [{ user, assistant, files_changed }] }` (bounded, 16 KiB) |
| `start_session` | `{ prompt, title?, harness?, model?, effort?, worktree?: bool, project?: path }` | `{ session_id, link_id }` — the client asks the user once, creates the thread (in a new worktree when `worktree: true`, default true for a git project), sends `prompt` as its first message |
| `message_session` | M1 | M1 |
| `wait_session` | M1 (`link_id` / `link_ids`) | M1 + `status`, `files_changed` |
| `fan_out` | `{ prompt, harnesses: [{ harness, model?, effort? }] (2..4), title? }` | `{ links: [{ session_id, link_id, harness }] }` — one worktree thread per harness, same prompt; the user compares results in the app |

Rules (enforced by the client, as M1): same project unless `project` is given
and the user approves it; at most 4 active links per thread; no chains; the
user can stop any link; nothing starts without the user's approval (*Allow for
This Thread* / *Allow Once* / *Don't Allow*).

## External harnesses as orchestrators

External harnesses get the same tools through an MCP server the engine
injects into their session: `rustcode mcp-orchestrator --session <id>` (stdio
MCP), added to the harness's `session/new` `mcpServers` (ACP) or to Codex's
config for that thread. Its tool calls go to the parent engine over a local
socket (`~/.config/rustcode/run/orchestrator-<pid>.sock`, mode 0600, token
checked) and from there through the same `_rustcode/orchestration` path, so
the client rules and approvals apply unchanged. Capability flag:
`orchestration: true` when injected.

## Capabilities added

`checkpoints: bool`, `worktree: bool` (always true for external harnesses
too, since the engine prepares the cwd), `orchestration: bool`.
