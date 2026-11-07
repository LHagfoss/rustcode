# The execution graph

Status: decision recorded (#2879), implementation planned in
[#2882](https://github.com/LHagfoss/rustcode/issues/2882). Nothing in this
note except the "Today" column exists in the tree yet. Terms follow
[terminology.md](../terminology.md): a **thread** is the conversation, a
**session** is the provider's handle on one.

## The decision

RustCode's domain object becomes T3 Code's Orchestration V2 shape:

```text
Project → Thread → Run → ExecutionNode tree
```

with `ProviderThread` handles and `ContextTransfer` / `ContextHandoff`
between them. Threads stay the user-visible root, and the sidebar, transcript
and CLI keep saying thread. What changes is that a session stops being an
object of its own: no folder, no id the app hands out, no user-facing name. It
becomes a provider binding a thread can have more than one of.

This replaces the earlier question of whether to adopt the graph. The
question now is migration order, which is #2882.

## Why

Today one thread is one conversation, one id, one folder, one harness and one
provider session, fixed at `session/new`. Orchestration is a layer over that:
links and mailboxes
(`rustcode/engine/src/orchestration.rs`), with the app holding the book
(`apps/desktop/src/state/orchestration.ts`) and the engine keeping what
survives a restart in `thread_links` and `orchestration_state`
(`rustcode/engine/src/data_store.rs`).

That layer cannot express a thread continuing on another harness, a fork or
side chat as a relationship rather than a copied conversation, subagents as
child threads with lineage, durable runs and execution nodes, work that
outlives a turn, or a child's approvals surfacing in its parent. None of those
is a wording problem: each one needs a state model the session cannot hold.

## Today, as built

| Piece | Where it lives now |
| --- | --- |
| Thread | Folder `~/.config/rustcode/threads/<yyyy>/<mm>/<dd>/<id>/`, `metadata.json`, `history.json`, `logs/debug.log`; searchable through `index.sqlite` (`rustcode/engine/src/thread_index.rs`) and queryable through the data store's `threads` table (`docs/data-store.md`). |
| Session | The same thread: its id is the ACP `sessionId` and the folder name. One thread, one session, decided at `session/new`. See [Cross-session orchestration](cross-session-orchestration.md). |
| Turn | One user message and the work that answers it, from a harness. The data store records it as a `turns` row; the engine also writes `turn.summary` events. |
| Round | One model request inside a turn, in the built-in harness (`rustcode/engine/src/network/turn`). |
| Tool call | One `tool_calls` row, one row in the thread's steps. |
| Approval | ACP `session/request_permission`, answered by the user or a grant; not a row of its own. |
| Compaction | `SessionCompaction` in `metadata.json` (`rustcode/core/rustcode-session/src/lib.rs`). |
| Subagent | `spawn_agent` inside one thread (`rustcode/engine/src/tools/mod.rs`): its own conversation, not a thread, not in the sidebar, ends with its parent. |
| Delegate | A thread another thread started or messaged: `start_session`, `fan_out`, `message_session`. A *link* (`<orchestrator>/<n>`) in the app's book and in `thread_links`; the parent is recorded in the child's `metadata.json` as `SessionParent` so pruning links never loses it (`rustcode/engine/src/thread_parent.rs`). |
| Checkpoint | `refs/rustcode/checkpoints/<session_id>/<turn>`, one per turn (`rustcode/engine/src/checkpoints.rs`). |

So the graph is already half there, held in three places that do not know
about each other: the folder, the link book, and the data store.

## Current → target

| Today | Target | Notes |
| --- | --- | --- |
| Thread folder and `metadata.json` | `Thread` | The user-visible root; keeps its id. |
| ACP `sessionId`, one per thread | `ProviderThread` handle | A thread gets more than one over its life. |
| A turn (a `turns` row) | `Run` | Counted per thread, with its user message, attempts, provider thread and checkpoint. |
| Tool call, approval, compaction, handoff row | `ExecutionNode` | `parentNodeId`, `rootNodeId`, kind, status, `countsForRun`. |
| `spawn_agent` | Child thread or child node with lineage | One shape for both, visible in the graph. |
| Link and mailbox | `ContextTransfer` plus a graph command | Cheap to record, resolved when a run needs it. |
| `history.json` + `index.sqlite` | Event log + projections | Sidebar, transcript, search and usage rebuild from the log. |
| Harness fixed at `session/new` | Provider switch with handoff | Same thread, a new provider thread, a `ContextTransfer`. |

The UI half of the last two rows is #2880. The store is #2873. Lineage and
subagents are #2872. Provider binding is #2868.

## Hard constraints

- **ACP stays.** `session/*` and `sessionId` keep their wire names and their
  meaning to other ACP clients; they name a provider binding, not the app's
  object. `_meta.rustcode.parent` keeps working for external harnesses.
- **Existing ids stay valid.** Every thread on disk today becomes one `Thread`
  with its current id; its folder is read, not rewritten, during migration.
- **No user-facing or storage object is called a session.** The glossary rule
  in [terminology.md](../terminology.md) holds after the migration as before.
- **Orchestration tools keep their names for one release.** `start_session`,
  `fan_out`, `message_session` and `wait_session` become aliases of the graph
  commands rather than disappearing in the same release.
- **No code change in this note.** The decision is recorded here; #2882 does
  the work.

## What we do not adopt

- T3's server/worker split. RustCode runs the engine on the user's machine and
  the app drives it over ACP; there is no worker pool to model.
- A graph as the only write path for transcripts. The event log is the source
  of truth for the new fields; `history.json` stays readable as it is today.
- Renaming the user-visible turn. The sidebar, CLI and transcripts keep
  saying thread and turn.

## Migration order

1. **Ids and schema alongside today's storage.** `ThreadId`, `RunId`,
   `NodeId`, `ProviderThreadId`, and events written next to the thread folder
   so nothing is lost (#2873).
2. **Backfill.** Each thread becomes a `Thread`, each turn a `Run`,
   transcript items become nodes; current ids stay valid.
3. **Projections.** Sidebar, transcript, search and usage read projections
   instead of scanning folders, rebuildable from the log.
4. **Provider binding.** `session/new` creates a `ProviderThread` bound to a
   `Thread`; a harness or model switch mid-thread adds one plus a
   `ContextTransfer` (#2868).
5. **Lineage and subagents.** Fork, side chat, merge-back, subagent
   spawn/result are context transfers; child threads and child nodes share one
   shape (#2872).
6. **Orchestration commands.** `start_thread`, `run_on_thread`, `wait_for_run`,
   `fork_thread`, `handoff_thread`, `spawn_subagent`, with the current tool
   names as aliases.
7. **Naming.** Drop user-facing "session"; keep ACP `sessionId` in the provider
   binding only, and update [terminology.md](../terminology.md) with it.

## Open

- **`Run` already means something else.** In
  [terminology.md](../terminology.md) a "Run" is a row of consecutive tool
  calls the transcript folds into one line ("Ran 3 commands"), while the
  graph's `Run` is a turn. One of the two names has to move before any UI says
  "Run" for a turn; #2882 decides which. The code should not use `Run` for the
  graph until then.
- **Whether a subagent becomes a thread.** Today `spawn_agent` is not a
  thread by design. #2882 makes child threads and child nodes one shape, which
  settles the storage question but not what the sidebar shows.
- **How much of the event log is backfilled** versus recorded from the
  migration point on. #2882's backfill slice accepts losing per-round detail
  for old threads.
- **Bounded fan-out and mailbox limits** (2–4 harnesses per `fan_out`, a
  32-message mailbox) are properties of the link layer. Whether the graph
  commands keep them, or replace them with run limits, is open in #2882.

#2882 is the place to answer these; this note records only what was decided
and what exists today.
