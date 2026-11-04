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

Built (`rustcode/engine/src/worktrees.rs`). Details:

- Errors are `invalid_params` with `data: { "reason", "message" }`; besides
  `dirty` and `unpushed`, `reason` is `not_git`, `no_base`, `not_managed`
  (remove of a path RustCode did not create) or `failed`.
- `Worktree` also carries `base_ref`: the branch `HEAD` pointed at (or the
  `base` given), which remove checks "merged" against. Remove answers
  `{ "removed": true, "branch_removed": bool }`.
- `<repo-name>` is the main checkout's directory name, so a worktree made
  from inside another worktree files under the same repository. A record
  beside each worktree (`worktrees/<repo-name>/.<slug>.json`) keeps `base`,
  `base_ref` and `session_ids`; git is the source of truth for which exist.
- Title suggestion: the leading verb picks the type (`fix…`, `docs…`,
  `refactor…`, `test…`, `perf…`, `chore`/`bump`/`update…`, else `feat`),
  filler words are dropped, the slug is cut at a word within 40 bytes;
  an empty title gives `feat/thread`.
- `unpushed` counts commits of the branch that are neither on a remote
  branch nor in `base`; `ahead`/`behind` are against the upstream when set,
  else `base`.

### Starting a thread in a worktree

`session/new` `_meta.rustcode.worktree = { "repo": "/abs/repo", "branch"?: "…", "title"?: "…" }`
creates the worktree first and uses its path as the session cwd. The response
`_meta.rustcode.worktree` carries the `Worktree`. `session/list` entries carry
`_meta.rustcode.worktree` (path, branch, repo) and `_meta.rustcode.status`.

Built for the built-in and every external harness (the worktree is made
before the harness starts, `base` is also taken). The thread's worktree is
kept in its directory (`worktree.json`) and `session/load` answers carry it
too, with `_meta.rustcode.status` and `_meta.rustcode.checkpoints`. A failed
create fails `session/new` with the error above. The worktree is created on
the request's dispatch, so a very large checkout delays other requests
while git checks it out.

## Thread status

`_meta.rustcode.status` on a thread: `"idle" | "running" | "needs_input" | "review" | "done"`.

- `running` while a turn runs, `needs_input` while it waits on an approval or
  question (engine-derived, not stored).
- `review` and `done` are set by the user or automatically, and stored with the
  thread: `_rustcode/session/status` `{ "session_id", "status": "review" | "done" | "idle" }` → `{}`.
- A thread whose worktree branch has a PR that merges becomes `done`
  automatically (engine checks via the existing PR watch, `_rustcode/pulls/watch`),
  and sends `session/update` with `_meta.rustcode.status`.

Built (`rustcode/engine/src/thread_status.rs`). Every change of a thread's
status, derived or stored, is sent as `session/update` with a
`session_info_update` whose only content is `_meta.rustcode.status`.
`needs_input` covers tool approvals and questions of built-in turns and
permission requests of external harnesses. A new turn on a `review` or
`done` thread clears the stored status (it is worked on again; `idle` once
the turn ends). The stored status is `status.json` in the thread's
directory. Auto-`done` needs a watch on the pull request
(`_rustcode/pulls/watch/start` from that thread): when a check of it sees the
pull request merged and the thread runs in a worktree, the thread becomes
`done`. Without a watch nothing polls GitHub.

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

Built (`rustcode/engine/src/checkpoints.rs`). Details:

- The snapshot is taken once the turn holds the thread (a queued prompt waits
  for the turn before it), for built-in turns from the app or a remote, and
  for external harness turns. It covers the whole work tree of the
  repository the cwd is in, starting from a copy of the user's index so
  unchanged files are not hashed again. It is skipped, with a debug line,
  past 3 s or when the repository's index is over 48 MiB; the turn runs
  either way. The commit's message holds the prompt (up to 4 KiB) and, for
  built-in turns, the chat position `_rustcode/rewind` takes.
- `turn` numbers count checkpoints (1, 2, …), not chat messages.
- `list` also answers `"available": bool` (whether the thread's cwd is in a
  git repository); per-thread `checkpoints: false` is in the `session/new` /
  `session/load` answer (`_meta.rustcode.checkpoints` and
  `_meta.rustcode.harness.capabilities.checkpoints`).
- `diff` files carry `old_path` for a rename and `"truncated": true` when the
  patch was cut (256 KiB per file, 2 MiB in all).
- `restore` refuses while the thread runs. It writes back the files the
  checkpoint has that changed, and deletes files that are new since and not
  ignored (with directories that end up empty); ignored files, the index,
  HEAD and the stash are left alone. It answers
  `{ "restored": true, "conversation": "rewound" | "kept" }`: `rewound` only
  for an open built-in thread whose message still matches, otherwise `kept`
  (with `rewind_error` when the rewind was refused). The rewind runs first,
  so a busy chat changes nothing.
- Pruned on thread delete where the engine deletes it
  (`config::delete_session_file`: the TUI and CLI). A client that deletes a
  thread's directory itself leaves its refs; they are ordinary refs under
  `refs/rustcode/checkpoints/<session_id>/`.

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
| `start_session` | `{ prompt, title?, harness?, model?, effort?, worktree?: bool, project?: path }` | `{ session_id, link_id, harness, model?, effort? }` — the client asks the user once, creates the thread (in a new worktree when `worktree: true`, default true for a git project), sends `prompt` as its first message. `harness` must be installed, on and signed in, and `model` one the harness lists (a missing provider prefix is filled in when only one model fits); otherwise nothing starts and the error names the usable harnesses or the closest models. The answer reports the model and effort the thread actually runs, and the approval card shows harness · model · effort |
| `message_session` | M1 | M1 |
| `wait_session` | M1 (`link_id` / `link_ids`) | M1 + `status`, `files_changed` |
| `fan_out` | `{ prompt, harnesses: [{ harness, model?, effort? }] (2..4), title? }` | `{ links: [{ session_id, link_id, harness, model?, effort? }] }` — one worktree thread per harness, same prompt, each checked like `start_session`; the user compares results in the app |

Checked in the engine before anything is sent (`validate`): `prompt` is
required and at most 8 KiB; `title` at most 200 characters; `harness`,
`model`, `effort` non-empty strings when given; `worktree` a boolean;
`project` an absolute path; `fan_out` takes 2 to 4 entries, each with a
`harness`, and no entry twice (the same harness on two models is fine).
The engine waits for the client's answer 15 min for `start_session` and
`fan_out` (approval, worktrees, harness start), `timeout_ms` + 30 s for
`wait_session`, and 10 min otherwise. Answers over 16 KiB are cut.

The version 2 prompt hint replaces the M1 one (about 80 tokens, tested).

Rules (enforced by the client, as M1): same project unless `project` is given
and the user approves it; at most 4 active links per thread; no chains; the
user can stop any link; nothing starts without the user's approval (*Allow for
This Thread* / *Allow Once* / *Don't Allow*).

## Orchestration tools v3

Built. Clients advertising `_meta.rustcode.orchestration >= 3` also get these,
so an agent can see what can run, find threads and manage them. Forwarded and
validated like the others; the client carries them out (`state/host.ts`).

| Tool | Args | Answer | Approval |
| --- | --- | --- | --- |
| `list_harnesses` | none | `{ current, harnesses: [{ id, name, available, reason?, models?, omitted_models? }] }`, at most 25 models each | None |
| `search_sessions` | `{ query }` | `{ sessions: [{ session_id, title, status, snippet? }] }`: this project, title and full text, at most 30 | None |
| `list_worktrees` | none | `{ worktrees: [{ session_id, title, branch, path }] }`: this project | None |
| `update_session` | `{ session_id?, title?, pinned?, unread?, status? }` (`active`, `review`, `settled`) | `{ session_id, title, pinned, status }` | Own thread none. Another: asked, allowed by Auto-edit and Full access |
| `interrupt_session` | `{ session_id }` | `{ session_id, interrupted, status? }`; a thread cannot stop itself | Asked, allowed by Full access |
| `configure_session` | `{ session_id?, model?, effort? }` | `{ session_id, harness, access, model?, effort? }`; no `model` or `effort` only reads | Reading and its own thread none. Another: asked, allowed by Full access |
| `list_projects` | none | `{ projects: [{ path, name, threads, working }] }`: every project the user has a thread in, most recently updated first, at most 50 | None |
| `list_pull_requests` | `{ state?: "open" \| "closed" \| "merged" \| "all" }` (default `open`) | `{ pull_requests: [{ number, title, state, branch, url? }] }`: this project's, at most 30 | None |

Every tool stays within the caller's project. A grant is per thread and per
kind of change (*Allow for This Thread*). `list_projects` spans projects since
it only reads names and counts. `list_pull_requests` uses the same
`_rustcode/pulls` request as the panel and answers its failures (`gh_missing`,
`not_connected`, `not_a_repository`, `timed_out`) as an error with that
reason, so the agent can say what to fix.

## External harnesses as orchestrators

Built (`rustcode/engine/src/orchestration/{hub,mcp_server}.rs`). When the
client advertises orchestration `>= 2`, an external harness thread gets the
same tools through an MCP server the engine injects into its session:

```
RUSTCODE_ORCHESTRATOR_TOKEN=<t> rustcode mcp-orchestrator --session <thread id> --socket <path>
```

The per-thread token travels in the server's environment (ACP `env`, Codex
`env`), never in its arguments, so other local users cannot read it from the
process list. `--token` still works for manual testing.

a stdio MCP server (`tools/list`, `tools/call`, `ping`; server name
`rustcode`), started by the harness with the engine's own executable:

- ACP children: one stdio entry in `session/new` / `session/load`
  `mcpServers` (`{ name: "rustcode", command, args, env: [] }`).
- Codex: `thread/start` / `thread/resume` `config` override
  `"mcp_servers.rustcode": { command, args, tool_timeout_sec }`, for that
  thread only. `tool_timeout_sec` is 3660, since Codex stops MCP calls after
  60 s by default and `wait_session` may wait an hour.

- Pi: `pi-acp` ignores `mcpServers`, so the engine starts it with
  `PI_ACP_PI_COMMAND` set to a small script that runs `pi --extension`. The
  extension (`harness/pi_extension.ts`) starts the same server and registers
  what it lists as `mcp__rustcode__*`, and removes the token from the
  environment the agent's shell commands inherit.

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
`checkpoints` and `worktree` are `true` for every harness in the snapshot (the
engine does both); a thread's answer sets `checkpoints` to whether its cwd is
in a git repository. `initialize` also advertises `_meta.rustcode.worktrees`,
`checkpoints` and `threadStatus` (each `1`).
