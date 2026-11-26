# 0010: Who owns each piece of thread state, and how it comes back

Status: accepted, 2026-11-26. Code: `rustcode/core/rustcode-session/src/`,
`rustcode/engine/src/{sessions,harness,thread_status.rs,thread_index.rs,data_store.rs,worktrees.rs}`,
`rustcode/engine/src/acp.rs`, `apps/desktop/src/state/{store.ts,continueThreads.ts,lostTurns.ts}`.

## Why

A thread's state is spread over a dozen files in its folder, two SQLite
databases, the harness's own session store, git worktrees and the app's
memory. Several processes write the same folder: the app's engine, `rc`
commands, a remote session owner, the standalone TUI. The rules for who may
write what, when it is on disk, and what a restart rebuilds it from lived only
in module comments. The 2026-11 audit found the gaps that follow from that:

- an external harness's turn was held in memory until it ended ([#4405])
- history writes failed silently, and seven writers had different atomic-write
  guarantees ([#4406])
- harness processes outlived an engine that crashed ([#4407])
- every data-store write opened its own connection ([#4408])

This record maps the state so the next gap is visible from one table.

## The map

"Durable" says when a change is on disk; "atomic" means a crash leaves the old
or the new file, never a mix (`rustcode_session::atomic_file::write_atomic`:
unique temporary, `fsync`, rename, directory `fsync`). "After a crash" says
what a restarted engine rebuilds the item from. Each row was checked against
the code on 2026-11-26.

### Thread folder: `threads/<yyyy>/<mm>/<dd>/<id>/`

| Item | Owner | Written by | Durable | After a crash |
| --- | --- | --- | --- | --- |
| `history.json` | the engine running the thread | the history writer thread (`rustcode-session/src/lib.rs`, `queue_history_write`), fed by the built-in agent per round (`controller/apply.rs`, `network/turn/finish.rs`) and by external harnesses at turn start, at tool-call boundaries and at most every 5 s of text, and at turn end (`harness/checkpoint.rs`, `harness/session.rs`) | atomic, debounced 250 ms; flushed on clean exit and in `AcpServer::shutdown` | the last write. A running external turn keeps what it did up to its last checkpoint; calls still running read as interrupted. Verified. |
| `harness.json` | the engine running the thread | `harness/store.rs` `save` | atomic, on change | the record a thread resumes its harness session from. Verified. |
| `status.json`, `pin.json` | whoever sets the status | `thread_status.rs` | atomic | as written; `running` and `needs_input` are never stored (see the locks). Verified. |
| `metadata.json` | the session store | `rustcode-session` `update_session_metadata`, `write_metadata_if_missing` | atomic; read-modify-write without a lock | as written. Two processes updating at once can lose one update. Unverified in practice. |
| `workspace.json` | the session store | `save_session_workspace`, a move (`acp/project_relocate.rs`) | atomic | as written; a move writes it first, so it wins over `harness.json` (`acp/session_repair.rs`). Verified. |
| `title.txt` | the session store | `save_session_title` | atomic | as written. Verified. |
| `segment.json`, `image_cache.json` | the built-in agent | `rustcode-session` | atomic | as written. Verified. |
| `performance.json` | the built-in agent | `network/turn/finish.rs` | at turn end | absent for a turn that never finished. Verified. |
| `logs/debug.log` | every process, appending | `logger.rs` | append, rotated | as far as it got. Verified. |
| `open.lock` | the process that has the thread loaded | `sessions/open.rs` | advisory `flock`, released by the kernel when the process ends | free. Verified. |
| `turn.lock`, `wait.lock` | the process running a turn or waiting on the user | `sessions/activity.rs` | advisory `flock` | free: the thread reads as idle. Verified. |
| `tool_output/`, `artifacts/` | the agent that ran the tool | `tools/exec.rs`, `harness/transcript.rs` | plain writes, before the message that names them | a message may name an artifact that a crash cut short. Unverified. |

Folders are never pruned (`docs/data-store.md`). A thread's folder is created
by the session store (`ensure_session`) and never by a write: `write_atomic`
refuses a missing folder, so a late write cannot bring back a removed thread
or a scratch request's folder.

### Outside the folder

| Item | Owner | Durable | After a crash |
| --- | --- | --- | --- |
| `index.sqlite` | the engine (`thread_index.rs`) | SQLite | a cache of the folders: refreshed from folder mtimes, deleted and rebuilt when unreadable. Verified. |
| `data.sqlite` | the engine (`data_store.rs`) | SQLite WAL, one kept connection per process for the user's store | a record, never rebuilt: per-turn token deltas, tool calls, the link book (`thread_links`, `orchestration_state`). Links that were running are marked interrupted at start (`interrupt_orphaned_links`). Verified. |
| A harness's native session | the harness (Claude Code, Codex, ...) | its own | resumed from `harness.json`'s native session id. RustCode never reads it back into `history.json`. Verified. |
| Harness processes | the engine that started them | `run/harness-groups/<pgid>` records (Unix), a job object per harness (Windows) | killed by the next engine that starts (`harness/orphans.rs`) or with the job's last handle. Verified in tests. |
| Git worktrees | `worktrees.rs` | git | the worktree and branch stay; cleanup records `branch` and `final_commit` first. Verified. |
| Settings, config | the engine (`acp/settings.rs`, `config.rs`) | atomic | as written. Verified. |
| The app's thread state | the desktop store (`apps/desktop/src/state/store.ts`) | memory only | rebuilt from `session/list` and `session/load`. A turn the engine was running is failed with "The server stopped during this turn", and continued if "Continue threads after a restart" is on (`resume-after-restart`, on by default) (`continueThreads.ts`); covered by `apps/desktop/e2e/recovery.spec.ts`. Verified. |
| Drafts, layout | the desktop | `localStorage` | as written. Verified. |

## Rules for writers

1. **One engine runs a thread at a time.** The engine that loads a thread holds
   `open.lock`; the remote gateway probes it before starting a second owner.
   A process that may outlive its claim calls `guard_session_history`: from then
   on it writes `history.json` only over the version it last loaded or wrote,
   and stops writing it for good once another process changed it
   (`HistoryGuard::Diverged`). Other files have no guard; their last writer
   wins.
2. **Every file a crash could leave half-written goes through `write_atomic`.**
   A new writer that builds its own temporary-and-rename is a review finding.
3. **What `running` means is derived, never stored.** A thread runs while a
   process holds its `turn.lock`; a crash releases it. Stored status
   (`review`, `done`) is only what a person or a merged pull request set.
4. **The folders are the source of truth for threads; `index.sqlite` is a
   cache of them; `data.sqlite` is a record they cannot rebuild.** Nothing
   reads a thread's transcript out of either database.

## What a client sees

`session/load` replays `history.json` as `session/update`s; a client that
opens a thread mid-turn is then sent what the turn did since its start from
the engine's memory (`harness/session.rs`, `turn_started`). Live updates go
to every client attached to the session in the order the engine received
them; there is no sequence number, so a client that misses updates while
disconnected reloads the thread instead of catching up.

## Considered: an event log as the source of truth

T3 Code keeps an append-only event store and derives its read models
(projections) from it, with an outbox for side effects
(`apps/server/src/orchestration-v2/{EventStore,ProjectionStore,EffectOutbox}.ts`).
For RustCode that would mean a per-thread `events.jsonl` with sequence
numbers, `history.json` as a projection of it, and clients catching up from
a sequence number instead of reloading.

What it would close that the smaller fixes do not:

- catch-up without a full reload, which matters for a phone on a bad link
- an exact record of the order updates were sent in, for replay and
  debugging
- recovery of a running turn to the last event, not the last checkpoint

What it would cost:

- a second copy of every transcript on disk, and a migration of every thread
  folder
- replay logic in every reader: the TUI, `rc threads`, the remote gateway,
  the search index, export and the desktop
- a write per streamed chunk, or batching that gives back most of the gain
  over checkpoints

Histories on one working machine (326 threads) were at most 3.5 MB, so
rewriting `history.json` per checkpoint is cheap at today's sizes.

## Decision

Keep `history.json` as the source of truth, written as a whole through the
durable writer, with mid-turn checkpoints for external harnesses. Do not add
an event log now. Revisit it when either of these holds:

- remote clients need catch-up from a sequence number (the remote protocol
  work owns that question)
- a measured history passes about 50 MB, at which point whole-file rewrites
  per checkpoint start to cost more than appends

Until then, a new piece of thread state gets a row in the tables above in the
pull request that adds it.

[#4405]: https://github.com/LHagfoss/rustcode/issues/4405
[#4406]: https://github.com/LHagfoss/rustcode/issues/4406
[#4407]: https://github.com/LHagfoss/rustcode/issues/4407
[#4408]: https://github.com/LHagfoss/rustcode/issues/4408
