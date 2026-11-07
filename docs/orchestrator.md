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

### From the CLI

A shell changes the same stored status, through the same engine function the
app's `_rustcode/session/status` and the MCP `update_session` use (#3151):

```bash
rustcode threads settle <id>...      # done: Settled in the app
rustcode threads unsettle <id>...    # idle: back to Active, from Settled
rustcode threads review <id>...      # review: needs a look
```

Each prints one line per thread (`settled <id> <title>`, `active <id> <title>`,
`review <id> <title>`), so a script sees which ones changed. Ids may be unique
prefixes, as in `rustcode threads settle 018f0`, or a full id whose thread
folder exists but holds no `history.json` yet (a settle right after a start); an
ambiguous or unknown id is reported on stderr and exits non-zero, and the other
ids on the line are still applied. `unsettle` only moves a thread out of
Settled: a thread in review is left as it is, and prints `kept <id> review`,
because settling (or the app's Review board) is what ends a review. Only the
stored status is written (`status.json` in the thread's directory), so a thread
with a turn running stays `running`; the app shows the change on its next
`session/list` refresh (connect, window focus, or after a turn), and the
terminal picker shows it on its next list.

## Checkpoints

Before every turn of any thread (built-in or external harness; the engine sees
both), the engine snapshots the thread's cwd into a hidden ref
`refs/rustcode/checkpoints/<session_id>/<turn>` (a commit of the working tree
including untracked, non-ignored files, built with a temporary index; the
user's index, HEAD and stash are never touched). Not in a git repo → no
checkpoints, capability `checkpoints: false`.

- `_rustcode/checkpoints/list` `{ "session_id" }` → `{ "checkpoints": [{ "turn": 3, "ref": "…", "created_at": "…", "prompt": "first 80 chars", "files_changed": 2, "insertions": 10, "deletions": 1 }] }`
  — plus `"attempt": true` on a checkpoint kept for the state a rewind
  abandoned (see `keep` below); it has no prompt and belongs to no turn.
- `_rustcode/checkpoints/diff` `{ "session_id", "turn" }` → `{ "files": [{ "path", "status": "added|modified|deleted|renamed", "insertions", "deletions", "patch": "unified diff" }] }`
  — what that turn changed: its checkpoint vs the next one (or the working
  tree for the latest turn).
- `_rustcode/checkpoints/restore` `{ "session_id", "turn", "rewind": bool, "keep": bool }` → `{ "restored": true }`
  — puts the files back to before that turn. With `rewind: true` it also calls
  the existing rewind for the conversation when the harness supports it
  (`capabilities.rewind`); otherwise files only, and the response says
  `"conversation": "kept"`.
- With `keep: true`, the work tree is checkpointed **before** anything changes
  and answered as `"kept": { "turn": N, "ref": "…" }`: the state the rewind
  abandons, so "go back to this attempt" can restore it with a second
  `restore` of that turn (files only; a conversation cannot be brought
  forward). If it cannot be kept, the request fails and nothing has changed,
  so the client can ask before restoring without a way back. `restore` of a
  kept attempt is allowed, and rewinds nothing.
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
- `turn` numbers count checkpoints (1, 2, …), not chat messages, and a kept
  attempt takes the next number like a turn's. The initialize meta carries
  `checkpoints: 2` for `keep` (and `"attempt"` on the list); `1` is the
  version without it, so a client asks before a rewind it cannot undo.
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
`_rustcode/orchestration` `{ "tool": <name>, "args": {...}, "sessionId": <calling session>, "callId": <the engine's name for this call> }`, plus `clientRequestId` and `argsHash` when the call carries a retry key (see [Retrying a call](#retrying-a-call)).

The engine drops a call when the harness stops waiting for it (its own tool
timeout, a cancel, the MCP server closing). It then sends the client
`_rustcode/orchestration/cancelled` `{ sessionId, callId }`, and the client ends
that call where it is: a `wait_session` releases its links without marking
anything heard — an answer the wait had already sent is handed back to the
ordinary wake, since the engine threw it away unread — so nothing is lost and
nothing arrives twice. A harness whose drop the engine never sees has a second
signal: the caller's own `wait_session` row ends in the app's transcript, and
the wait is released the same way.

| Tool | Args | Answer |
| --- | --- | --- |
| `list_sessions` | M1 + `all_projects?: bool` | M1 + `harness`, `status`, `worktree`, and `project` with `all_projects` |
| `read_session` | `{ session_id, last_turns?: 1..10, offset?: bytes }` (default 3 turns, offset 0) | `{ session_id, title, status, project, offset, text, next_offset?, complete }`: the rendered answer as text, one page at a time (16 KiB); when `next_offset` is present, pass it as `offset` for the rest. Pages join with nothing lost or repeated, cut on a UTF-8 character boundary. Any project's thread |

| `start_session` | `{ prompt, title?, harness?, model?, effort?, account? (v5), worktree?: bool, project?: path, client_request_id? }` | `{ session_id, link_id, harness, account?, access, model?, effort?, links_left, replayed? }` — `links_left` is the smaller of this thread's room and the app's total — the client asks the user once, creates the thread (in a new worktree when `worktree: true`, default true for a git project), sends `prompt` as its first message. `harness` must be installed, on and signed in, `model` one the harness lists (a missing provider prefix is filled in when only one model fits), and `account` a named login that harness's `list_harnesses` entry lists; otherwise nothing starts and the error names the usable harnesses, the closest models or the accounts it does have. Without `account` the child runs on the caller's own account when it stays on the same harness, else on the login a new thread would start on. The answer reports the access, account, model and effort the thread actually runs, and the approval card shows harness · account name (id) · model · effort |
| `message_session` | M1 + `client_request_id?` | M1 + `replayed: true` when the call was answered from its receipt |
| `wait_session` (a settled link whose child still works) | — | answers that link as `state: "working"` with `turns`, `last_tool` and `idle_ms`, and keeps waiting: the answer it would carry is not the child's last |
| `wait_session` | M1 (`link_id` / `link_ids`) | M1 + `status`, `files_changed`, and per link `turns`, `last_tool` and `idle_ms` (how long the child has done nothing) so a timeout says whether it thinks or is stuck; a timeout also carries `status: "timeout"`, `waited_until` and a note that a wake will come. An answer cut at 8 KiB carries `answer_truncated: true` and the `session_id`, whose `read_session` has the rest |
| `fan_out` | `{ prompt, harnesses: [{ harness, model?, effort?, account? (v5) }] (2..4), title?, client_request_id? }` | `{ links: [{ session_id, link_id, harness, account?, access, model?, effort? } \| { harness, error }], fan_out_id, links_left, replayed? }` — one worktree thread per harness, same prompt, each checked like `start_session`; the user compares results in the app |

Checked in the engine before anything is sent (`validate`): `prompt` and
`message` are required and at most 32 KiB; `title` at most 200 characters;
`harness`, `model`, `effort` non-empty strings when given; `account` is the
same but only a version 5 client may send it (below); `worktree` a boolean;
`project` an absolute path; `offset` a whole number of bytes; `fan_out` takes
2 to 4 entries, each with a `harness`, and no entry twice (the same harness on
two models, or on two named accounts, is fine); `client_request_id`, when
given, is 1 to 128 characters of `A-Z`, `a-z`, `0-9`, `.`, `_`, `:` and `-`
(see [Retrying a call](#retrying-a-call)). The engine waits for the
client's answer 15 min for `start_session` and `fan_out` (approval, worktrees,
harness start),
`timeout_ms` + 30 s for `wait_session`, and 10 min otherwise. The client bounds
an answer: `wait_session` at 8 KiB (with `answer_truncated`), `read_session`
at a 16 KiB page (with `next_offset`).

The version 2 prompt hint replaces the M1 one (about 110 tokens, tested), and
it says what the tools make a thread: one started with `start_session` or
`fan_out` is a delegate of the caller, shown nested under it, so it gets a
whole task, while a quick read-only lookup belongs to the harness's own
subagents.

Delegates inherit neither the caller's conversation nor its local
instructions. Repeat the repository rules that apply, the acceptance criteria
and any file:line pointers in the prompt, and tell the delegate to read
`AGENTS.md`. Never rebase a delegated branch: merge `origin/main` with
`--no-edit`, and pass `-m` to commits rather than expecting an editor. A
delegated shell has no TTY and no default timeout, so a command that waits for
input, or a one-off script whose timers keep it alive, holds the turn until
someone interrupts it: make scripts exit and bound anything that can block with
`timeout`, using a longer explicit limit for a known long check. A command that
timed out leaves the worktree mid-state, so inspect it before rerunning.

Rules (enforced by the client, as M1): same project unless `project` is given
and the user approves it, or `all_projects` reads it (see the v3 note below);
at most 8 delegates working for one thread at once by default, set to 2 to 32
in Settings › Agents, and at most 16 delegates running at once across this app
instance, set to 2 to 64 (a `busy` refusal names the limit it hit and the
current count, says `links_left` and that `wait_session` on one comes first).
The CLI and TUI start no delegates, so the total is one app's own. A message
to a thread it already works with is a follow-up: it queues on that thread and
takes no slot, though its link is still created and waitable. Delegation
goes two levels deep — a top-level thread may start children, and they may
start grandchildren, while a thread at that depth is refused with the limit in
the reason (`MAX_DEPTH` in `state/orchestration.ts`); both limits count live
delegates at every depth and free a slot as soon as one finishes; a message
that would close a loop — to a thread that already works for the caller,
directly or further up — is refused; the user can stop any link;
nothing starts without the user's approval (*Allow for This Thread* / *Allow
Once* / *Don't Allow*). A delegated thread's login is never chosen silently when
the agent named one: an `account` that harness does not list is refused with
the ids it does list, and an ambiguous one is refused with the ids it matches
instead of picking one. The approval card names the login's display name and
id. `fan_out` refuses two entries that resolve to the same harness, model,
effort and login, however they were spelled. `account` and the answers'
`account` are version 5 only: an older client is not shown the field, and the
engine refuses one it sends rather than letting that app ignore it (see
[Orchestration tools v5](#orchestration-tools-v5)).

### Retrying a call

An agent's `start_session`, `message_session` or `fan_out` can time out at the
MCP layer after the app already started the thread or delivered the message,
and a blind retry starts or delivers a second time. Those three tools take an
optional `client_request_id`: a key the caller picks, which makes the call safe
to repeat.

- Same key, same tool, same arguments: the call is not made again. The app
  answers with the result it recorded the first time, plus `replayed: true`, so
  the retry gets the same `session_id` and `link_id` as the original.
- Same key, another tool or other arguments: refused with
  `client_request_id reused with different arguments`. Nothing is started or
  sent; the model should use a new key for a different call.
- No key: exactly the behaviour above, unchanged.
- A second copy of the same call while the first is still running (the user
  has not answered the card yet) waits for the first and answers with its
  result, marked `replayed`, instead of asking the user twice.
- Only a call that succeeded is recorded, so a refusal or a failure leaves
  nothing behind and a corrected call under the same key runs.

The check happens in the app **before** it acts and the receipt is written
**right after** the call succeeds, before the answer goes back — which is what
makes it work when the answer is what got lost. Receipts live in the engine's
data store (`command_receipts` in `docs/data-store.md`), so they survive an app
restart: a key is usable for 7 days and the store keeps the newest 10,000. The
engine forwards `clientRequestId` and an `argsHash` (sha256 of the tool and the
call's arguments, keys sorted, so a reordered retry matches) with the request;
the app asks `_rustcode/data/receipts/get`
(`rustcode/engine/src/acp/data.rs`) and writes `_rustcode/data/receipts/put`.
Nothing about the prompt or message is stored: only the tool, the hash and the
answer. Keys are scoped to the calling thread, so two threads may use the same
one.

The field is additive and needs no new orchestration version: an app that does
not know receipts ignores it, and such a retry behaves as it did before. The
dropped-call errors the engine returns say whether the call carried a key.

### A delegated thread's access

A thread started by an agent runs at `effectiveMode(parentAccess, defaultAccess)`
(`state/permissions.ts`): the lower of the access of the thread that started it
and Settings › General › Default access, which is what a new thread would get
anyway. So a thread at Ask or Auto-edit never hands out Full access, whatever
the default is, and the cap holds at every depth — a grandchild is capped by its
own parent, which is already capped by the thread above it. The app sets it
before the child's first turn (`newThread` in `state/host.ts`), and it is stored
for that thread like the composer's own choice, so changing the default later
does not move it.

- The approval card says the access the new thread will run at, and
  `start_session` / `fan_out` answer with `access`, so an agent is not misled
  about what it started. `configure_session` reports it and never changes it.
- A person can still raise a child above its parent in that child's own
  composer (the shield under its message box): that is the user's explicit
  choice, not an agent asking for more. Lowering a parent later leaves the
  children it already started as they were — the grant was made when they
  started, as with every other grant in the link book.

### A delegated thread's place in the sidebar

A thread an agent started is never a stray top-level row. The parent is kept in
the child's own `metadata.json` (`parent` in `rustcode-session`, written at
`session/new` from `_meta.rustcode.parent`), so pruning the link book's oldest
finished links (`prune` in `state/orchestration.ts`) cannot forget it.
`topLevel` (`apps/desktop/src/state/topLevel.ts`) picks one of two places:
*nested* under a parent that exists, with no row of its own (the parent's row
shows the ones still working or waiting on you as a stack of marks,
`ui/AgentStack.tsx`); or *shown* where it would be anyway, which is what a
child whose parent is gone from the app entirely gets — with no parent row to
sit under anywhere, hiding it would take it out of the app. Nesting is resolved
against every session, not the filtered list: a project filter or a search
changes what is drawn, never who a thread belongs to, so neither can turn a
delegate into a top-level row. While the user has a delegated thread open it is
drawn as an indented row, with a subtle tree line, directly under its parent's
row (and the threads between it and its nearest row of its own); navigating away
folds that row out again and leaves the child in the parent's stack of marks.
If the parent sits in the Settled shelf, the shelf opens and its slice grows far
enough to show the row the child hangs under. A search still lists every match,
including delegates, and each such row says `in <parent title>` instead of
un-nesting the tree. The same
record is on the row `session/new` adds to the list, so a thread being spawned
is nested from its first frame instead of showing as a row of its own until the
engine lists it. The user's own actions win: a
pinned child stays in Pinned, and one the user sent a message to is a row of its
own — a message a person typed, not a wake, a restart resume, a link or the turn
that continues after a usage limit (`userTypedPrompt`).
Threads started before the record existed are classified once, before
the first session list, from the first message's `From thread “…” (agent):`
heading, with a surviving `thread_links` row for the parent's id when there is
one (`rustcode/engine/src/thread_parent.rs`).

## Waking a parent thread

Built in the app (`apps/desktop/src/state/host.ts`, rules in
`state/orchestration.ts`). A thread that starts or messages others is told when
they finish or ask, instead of having to poll with sleeps.

- **Done or failed.** When every link under a parent has settled, the parent
  gets one message in a turn of its own. While any of its links is still
  running, a settled child waits in a coalescing window of 3 s
  (`WAKE_COALESCE_MS`) for its siblings, so a fan-out that finishes together is
  one message. A settled child is reported only when its own subtree is quiet:
  no link of its is active, none of its own children waits to be reported to
  it, and no turn of it runs (`quietTree` in `state/orchestration.ts`). A
  middle thread that ended its turn after starting a leaf has *not* finished
  for its parent then — its answer was "I started it" — so the parent waits
  through the leaf's result and the wake turn that follows, and is told the
  middle's **latest** answer: a settled, not-yet-reported link takes the answer
  and `settledAt` of each later turn of its target (`restateLinks`). The message
  names each delegate: `link_id`, status, title, harness, how long it worked,
  its `session`, files changed, how long ago, and its answer cut to 1 KiB each
  (`WAKE_ANSWER_CHARS`); a link that settled without one (its reply's rows
  reached the app a moment later) is completed from the child's last reply
  before anyone reads it, and a later turn
  that answers nothing never erases an answer; it says
  the rest is in `read_session` on the child's session or `wait_session` on a
  `link_id`, and that the message is the notification. A child that was
  stopped, or interrupted and left interrupted, is no result to wait for:
  nothing ever reports it, so it does not hold its parent back.
- **After a restart.** A restart resumes the threads it cut off
  (`state/store.ts` `resumeInterrupted`), and the resume is what answers the
  links the quit severed: it is tied to that one turn, never to a turn that
  merely happens later. As the resume turn starts, each interrupted link to
  that thread goes back to `working` with its clock, its wait and a `resumedAt`
  mark reset from the resume (`host.ts` `linksResumed`), and the turn carries
  the links, so the ordinary link path settles them when it ends
  (`linksFinished`) — the parent that was waiting reads the answer in a wake,
  and the time it says the branch worked is the time after the restart, not
  the time before the quit. Several links one resume turn answers settle in one
  write to the book and one wake tick, so a parent hears about them in one
  message that says each was *resumed after a restart* (`orchestration.ts`
  `linkLine`). A user's own turn in the child never settles an interrupted
  link; one whose message the quit caught still queued is no resume's (its row
  is read back and run on its own, see *Across a restart*). A thread whose own
  turn outlived the window takes the resume behind it as a queued row, which
  starts a turn that does carry the links, and at most 8 resumes are started.
  A resumed turn the user stops is a stopped link, and reports nothing.
- **Waiting on the user.** A child that goes `needs_user` (an approval or a
  question) is reported the same way, once per pending request (its rpc id):
  the parent is told to use `list_pending_requests` and that the user answers,
  because an agent cannot answer an approval. The user sees the same card in
  the child's chat and at the foot of the parent's transcript, and answers it
  in either place.
- **Not twice.** Each link is marked in the link book (`notifiedAt`,
  `askedRequest`), which now lives in the data store (`docs/data-store.md`),
  so a restart does not deliver it again. A queued row is guarded the same
  way on its own: the id of a row that left a loaded thread's queue is kept as
  spent in app storage, so a queue read back after a restart holds nothing that
  was already sent (see *Across a restart* below). A parent blocked in `wait_session`
  on that link already has the answer — the wait marks it heard — and is not
  sent a second copy. That hold lasts only while the turn that asked runs: a
  wait whose turn ended (the user stopped it, or the harness dropped the call)
  stops at its next poll and marks nothing, so the result still comes as a wake
  instead of being answered into a turn nobody reads. A thread that is itself a child is woken the same way a
  top-level parent is: its children's results are its to read, and its own
  parent hears about it when its own subtree is quiet, not before. A wake turn
  carries no link, so being woken never settles the woken thread in its
  parent's book while that turn runs. A link that was already reported and whose
  target then answers again is reported a second time only when that turn was
  started by the wake from the target's own child — once, marked by
  `answerChangedAt`, so nothing loops and a user's own turn in the child does
  not wake the parent again. A parent whose thread is gone is dropped, and the
  link is marked so a thread reopened under the same id cannot be woken for old
  work.
- **Stopping takes the tree.** Stopping a thread — `interrupt_session`, or the
  user's own stop — also stops the links it is working through, and theirs, once
  each; the tool's answer says how many (`stopped`). A normal finish does not
  cascade: the children keep running and are woken into the thread that started
  them.
- **No loops.** A wake is a message from the app, not a delegation: it creates
  no link and grants nothing, and waking a parent does not in turn wake anyone.
  A wake turn that starts new children is ordinary work and its own completion
  is reported when it settles.
- **Delivery.** If the parent is idle the wake starts a turn at once. While it
  is running, a child that **needs the user** (an approval or a question) is
  steered into that running turn when its harness takes a live steer that leaves
  its tools running — Codex, and an ACP agent that advertised
  `_session/steering` — by the same rule a typed ⌘⏎ message follows
  (`sendDecision` / `wakeDelivery` in `apps/desktop/src/state/queue.ts`, read in
  `state/host.ts` `sendWake`): the parent sees the request now, no tool call is
  interrupted and no turn is stopped. Its links are marked heard once the engine
  takes the steer, exactly as a queued row's marks are written when its turn
  starts. Where the harness takes no live steer, a steer would have to stop the
  turn (`interrupt_restart`) or there is none (`queue`), the wake is **queued**
  behind the turn as a typed message is — a child's notice never takes a
  parent's tool calls down. A turn that waits on the user reads no steer on the
  built-in harness (`while_awaiting_user`), and it stays queued there too. A
  **completion** still waits for the next turn, so a busy parent is not steered
  by every finished grandchild.
- **Across a restart.** A queue is the app's memory, so what waits in one is
  kept in app storage per thread id (`state/queues.ts`, `state/host.ts`): the
  id, text, attachments, kind (`user` or `wake`), order and `created_at` of
  every row, written as the queue changes and read back when a thread is opened
  again. A thread that kept a queue is opened at start-up and, once idle, its
  head starts a turn (`resumeQueue`), so a message or a child's result queued
  before a quit, a crash or an update is not left sitting in storage. An id that
  left a loaded thread's queue was sent or dropped, and is recorded as spent: a
  restart never delivers it twice. A wake row whose links the parent already
  heard — the marks were written and the queue was not — is dropped when the
  queue is read back (`wakeStillOwed`), and one whose links another waiting row
  already reports is not added beside it. Edit, remove and send now work on a
  restored row as on any other. Settings' *Threads can start and message other
  threads* turns the waking half off with the rest of orchestration; the queue
  itself is kept either way.
- **In the transcript** a wake turn is a quiet divider row built from the
  message, not a message that reads as the user's own. Its first line names the
  delegate ("Delegate finished: Fix composer focus") and its detail lines add
  the harness and how long it worked; the row shows all of it. Several
  delegates show up to three harness marks, their titles and the longest worked
  time. A wake steered into a running turn is a steer row there, which says the
  turn reads it after its current step.

## Orchestration tools v3

Built. Clients advertising `_meta.rustcode.orchestration >= 3` also get these,
so an agent can see what can run, find threads and manage them. Forwarded and
validated like the others; the client carries them out (`state/host.ts`).

| Tool | Args | Answer | Approval |
| --- | --- | --- | --- |
| `list_harnesses` | none | `{ current, harnesses: [{ id, name, available, reason?, accounts? (v5), models?, omitted_models? }] }`, at most 25 models each; `accounts` is `[{ id, name, auth, signed_out? }]` for a harness with named logins, where `signed_out` is what the plan read found (and what makes naming it fail), and `current` names the harness, account, model and effort this thread runs on | None |
| `search_sessions` | `{ query, all_projects? }` | `{ sessions: [{ session_id, title, status, project?, snippet? }] }`: this project, or every project with `all_projects`, title and full text, at most 30 | None |
| `list_worktrees` | `{ all_projects? }` | `{ worktrees: [{ session_id, title, project?, branch, path }] }`: this project, or every project with `all_projects` | None |
| `update_session` | `{ session_id?, title?, pinned?, unread?, status?, snoozed_until? }` (`active`, `review`, `settled`; `1h`, `evening`, `tomorrow`, `next_week`, an ISO 8601 time, or `null` to wake) | `{ session_id, title, pinned, status, snoozed_until }` | Own thread none. Another: asked, allowed by Auto-edit and Full access |
| `interrupt_session` | `{ session_id }` | `{ session_id, interrupted, stopped, status? }`, with `stopped` the links under it that stopped too; a thread cannot stop itself | Asked, allowed by Full access |
| `configure_session` | `{ session_id?, model?, effort? }` | `{ session_id, harness, access, model?, effort? }`; no `model` or `effort` only reads | Reading and its own thread none. Another: asked, allowed by Full access |
| `transfer_session` | `{ session_id, harness }` | `{ session_id, harness, moved, model?, effort? }`; the thread must be idle, and a thread cannot move itself | Asked, allowed by Full access |
| `list_pending_requests` | none | `{ requests: [{ session_id, title, kind, asks }] }`: approvals and questions this project's open threads wait on. Agents cannot answer them | None |
| `list_projects` | none | `{ projects: [{ path, name, threads, working }] }`: every project the user has a thread in, most recently updated first, at most 50 | None |
| `list_pull_requests` | `{ state?: "open" \| "closed" \| "merged" \| "all" }` (default `open`) | `{ pull_requests: [{ number, title, state, branch, url? }] }`: this project's, at most 30 | None |

Every tool that writes, or that hands work to another thread, stays within the
caller's project. A project is the repository a thread runs for — a main
checkout and every worktree of it, resolved by the engine from the folder
(`_meta.rustcode.repo`, `git rev-parse --git-common-dir`) — so a thread working
in a worktree reaches the repository's other threads and its own worktree
delegates; a thread it started itself is reachable wherever it runs. Reads — `list_sessions`, `search_sessions`, `read_session`,
`list_worktrees`, `list_projects` — reach any project while **Let agents read
threads in other projects** is on (Settings › Agents, on by default;
`rustcode.readAcrossProjects`). With it off, an `all_projects: true` call and a
`read_session` of another project's thread answer `not_allowed`. An
`all_projects` answer names each thread's `project` path, so two threads with
the same title are told apart. A grant is per thread and per kind of change
(*Allow for This Thread*): it is not per harness, model or login, so a thread
already allowed to start threads may name another harness, model or account
without another card. The choice is still named on every card it does ask
(`harness · account name (id) · model · effort`) and in the answer's `account`,
and the grant goes away with the thread (`forget` in `state/orchestration.ts`).
`list_projects` spans projects since it only reads
names and counts. `list_pull_requests` uses the same
`_rustcode/pulls` request as the panel and answers its failures (`gh_missing`,
`not_connected`, `not_a_repository`, `timed_out`) as an error with that
reason, so the agent can say what to fix.

## Orchestration tools v4

Built. Clients advertising `_meta.rustcode.orchestration >= 4` also get the
local data store (`docs/data-store.md`) as two read tools. The version is
bumped rather than the tools added to v3 because the *client* carries them out:
a v3 app has no `_rustcode/data/*` to ask and would answer "unknown tool", so
an old app keeps exactly the v3 set (and the v3 prompt hint).

| Tool | Args | Answer | Approval |
| --- | --- | --- | --- |
| `thread_usage` | `{ since?, group_by?, project? }` — `harness` (default), `model`, `day`, `project`, `thread`; `since` is `YYYY-MM-DD` or `YYYY-MM` | `{ group_by, since, project, scope, rows, totals, truncated, note }`: tokens and the USD a harness reported, per group, with `turns_without_tokens` so a silent harness is visible | None |
| `query_data` | `{ sql, limit? }`, one read-only statement, at most 1000 rows | `{ columns, rows, truncated }` | None |

The app answers them (`state/host.ts`): it passes the caller's project while
**Let agents read threads in other projects** is off — `thread_usage` then
reports `scope: "this project"` — and refuses `query_data`, which cannot be
scoped to a project safely, with a `not_allowed` that names the setting. It
asks the engine `_rustcode/data/usage` / `_rustcode/data/query`
(`engine/src/acp/data.rs`), which only ever reads: the one-time backfill runs
on first use, and `query` keeps the read-only connection, the single-statement
rule, no `ATTACH` and the two-second stop.

## Orchestration tools v5

Built. Version 5 adds no tool: it adds named-account targeting to two existing
ones, and the accounts to discovery. Clients advertising
`_meta.rustcode.orchestration >= 5` get `account` on `start_session`, on each
`fan_out` entry, and in `list_harnesses` (plus in `start_session` / `fan_out`
answers when the thread runs on a named login). The app advertised 5 from
`CAPABILITY_VERSION` while v5 was newest, and a v6 app (below) still answers
it.

An older client must not silently ignore the argument: it would start the
child on whatever login it inherited. So the engine does both halves:

- **Schema.** The built-in native schema (`ToolSchemaPolicy`,
  `tools/schema.rs`) and the MCP server's `tools()` (`orchestration/mcp_server.rs`)
  build through `orchestration::schema_for(name, version, schema)`, which strips
  `account` from `start_session` and from `fan_out`'s entries below version 5.
  A v4 client is never offered a field its app cannot answer, and its prompt
  hint stays `PROMPT_HINT_V4`; v5 gets `PROMPT_HINT_V5`, which names the argument.
- **Execution.** `orchestration::execute_versioned` refuses a call that carries
  `account` from a client below 5 with *needs a newer RustCode app. Nothing was
  sent.* (`UnavailableDependency`) before anything reaches the client. The
  app's own `CAPABILITY_VERSION` is 6, so a matching app and engine agree; an
  older app on a newer engine gets the older schema and a clear refusal instead
  of a child on an unexpected login. The MCP server also refuses the argument in
  its own `call` before forwarding, and the hub registers every external thread
  with that thread client's version and passes it to `execute_versioned` — not
  the engine's newest version — so a v4 thread cannot reach the app by calling
  the socket directly.

On the app side, an explicit login is resolved before the approval card
(`state/logins.ts`, `state/host.ts`). An exact account id wins, even when
another login's display name is the same, so an id stays reachable; failing
that it matches an id or display name case-insensitively, and refuses an
**unknown** one (with the ids it has), an **ambiguous** one (more than one
case-insensitive id, or more than one display name matches, with those ids) and
a **signed-out** one (`harness#account` in the plan read's sign-out record).
`list_harnesses` reports that `signed_out` flag per account, so an agent can
choose a login that will run before it asks. The approval card shows both the
display name and the id, e.g. `Claude Code · Work (work) · opus`; the fan-out
Compare card shows the same label next to the harness, so two logins of one
harness read apart. Omitting `account` keeps the inherited behaviour exactly as
before (the parent's login on the same harness, else what a new thread would
start on). `fan_out` resolves every entry before asking and refuses two entries
that land on the same harness, model, effort and login — including `work` and
`Work`, which the engine cannot see as the same.

## Orchestration tools v6

Built. Clients advertising `_meta.rustcode.orchestration >= 6` also get
`read_activity`, a page over what the store recorded about one thread. The
version is bumped rather than the tool added to v5 for the same reason as v4:
the *client* carries it out, and a v5 app has no `_rustcode/data/activity` to
ask. The app advertised 6 from `CAPABILITY_VERSION`
(`state/orchestration.ts`; v7 raised it to 7, v8 to 8), and a v5 client keeps exactly the
v5 set and `PROMPT_HINT_V5`.

| Tool | Args | Answer | Approval |
| --- | --- | --- | --- |
| `read_activity` | `{ session_id, after?, limit? }` — `limit` 1..200 (default 50); `after` is the opaque `next` token an earlier page returned | `{ session_id, items, next, has_more }`: `operational_events`, provider attempts and harness messages merged in time order, oldest first, with `source` on each item and no payload columns. `next` encodes the last item's `(at, source, row id)` | None |

The app gates it like `read_session`'s reach — any project while **Let agents
read threads in other projects** is on, this project's threads otherwise, and
the calling thread itself is refused — but it asks no approval, since it only
reads metadata and never a prompt or payload. It asks the engine
`_rustcode/data/activity` (`engine/src/acp/data.rs`), which reads through the
same read-only connection as `query` with at most 200 items and no payload
column selected. The allowlist of projected events is in `docs/data-store.md`
(§ Request and protocol telemetry).

## Orchestration tools v7

Built. Clients advertising `_meta.rustcode.orchestration >= 7` also get
`manage_queue`: the messages this thread queued on another one with
`message_session`, and the three things it may still do to them. Until v7 a
message to a busy thread waited unread and uncorrectable: the agent that sent it
could neither see nor change what it had queued. The version is bumped rather
than the tool added to v6 for the same reason as v4 and v6 — the *client* holds
the queue (`state/queues.ts`, delivered by `state/host.ts`). The app advertises
7 from `CAPABILITY_VERSION` (`state/orchestration.ts`), and a v6 app keeps
exactly the v6 set and `PROMPT_HINT_V6`.

| Tool | Args | Answer | Approval |
| --- | --- | --- | --- |
| `manage_queue` | `{ session_id, action, queue_id?, text? }` — `action` is `list`, `edit`, `cancel` or `steer`; `queue_id` is required but for `list`; `text` is required by `edit`, at most 32 KiB, the same limit as `message_session` | `list`: `{ session_id, title, running, held?, queued: [{ queue_id, preview, queued_at? }], others? }` — this thread's own messages in send order, at most 50, `preview` the first 200 characters, `others` how many wait there that a person or another thread queued (a count only: their text is not this thread's to read). `edit`, `cancel`, `steer`: `{ session_id, queue_id, preview \| cancelled: true \| steered: true }` | `list` none, like `read_session`. The three that change something: asked, allowed by Full access, with the grant `message_session` keeps for that thread |

A queued row carries the thread that queued it (`from` on
`QueuedPrompt`/`QueueRow`, written by `deliver`), so only that thread's own
messages are listed in full and only those can be edited, cancelled or steered;
a row queued before this change, or by a person, has no `from` and belongs to no
caller. Anything that has left the queue — because it started, or because a
person removed it — answers `conflict: that message has already started`;
an id that was never in that queue answers `not_found`. `edit` replaces the text
and keeps the `delivered` attribution line (`From thread “…” (agent):`) and the
link, so the target still knows who asked and the answer still goes back;
`cancel` is the row's own ✕, so its link settles as cancelled; `steer` delivers
it into the running turn by the same rule ⌘⏎ follows (`sendDecision`,
`state/queue.ts`) and refuses with `not_allowed: this harness cannot steer`
where the harness takes no message mid-turn — including a harness that would
have to stop the turn (`interrupt_restart`), which is not steering. The queue
lives in the app, so the engine only validates the call
(`orchestration.rs` `validate`, `QUEUE_ACTIONS`).

## Orchestration tools v8

Built. Clients advertising `_meta.rustcode.orchestration >= 8` get no new tool:
v8 is the prompt hint and the MCP server's `instructions`. It says a Claude Code
thread has no subagent tool, so delegate even a quick lookup through
`start_session` or `fan_out`, while a harness that has its own subagent tool may
use it for quick lookups. A `claude-code` thread RustCode starts with the
orchestrator attached has `Task` and `Agent` disallowed in the adapter's session
`_meta` ([harnesses.md](harnesses.md#starting-a-thread-on-a-harness)), so every
subagent it starts is a RustCode delegate; without the orchestrator its own tool
stays, since there is nothing to delegate through. The app advertises 8 from
`CAPABILITY_VERSION` (`state/orchestration.ts`), and a v7 app keeps exactly the
v7 set and `PROMPT_HINT_V7`.

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

The same environment carries `RUSTCODE_ORCHESTRATION_VERSION` (the client's
version), so the server lists exactly that version's tools and prompt hint — a
v3 app is never offered `thread_usage` or `query_data`, which it could not
answer, a v5 app is never offered `read_activity`, and a v6 app is never
offered `manage_queue`.

While the user has Smart UI on, the same environment carries
`RUSTCODE_SMART_UI=1` and the server also lists **`smart_ui_check`**
(`smart_ui::CHECK_TOOL_NAME`): a static check of a `rustcode-ui v1` block — its
size, a missing title, unclosed tags, an unbundled `rustcode-lib`, and what the
page's sandbox refuses (network, storage, `vh`, system colours on a canvas),
each with its line. The server answers it itself, without the socket, so it
works on a thread whose client never advertised Smart UI at all; the engine and
the harness pay one tool description for it. It does not run the page, so it
returns no script errors, height or screenshot (`docs/smart-ui.md`).

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
`{ success, content }` out; closing early cancels the call, and so does MCP's
own `notifications/cancelled` for a call in flight. The engine checks
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
`checkpoints` and `threadStatus` (each `1`). The app advertises
`_meta.rustcode.orchestration = 8` while threads may orchestrate
(`state/orchestration.ts` `CAPABILITY_VERSION`), and the newest client gets the
v4 data-store tools, the v6 activity read and the v7 queue control; v8 adds no
tool, only its prompt hint.
