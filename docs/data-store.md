# Local data store

`~/.config/rustcode/data.sqlite` (`engine/src/data_store.rs`) answers questions
the thread folders cannot answer cheaply: tokens per harness and model per
day, how many turns and tool calls a thread spent, which tools failed.

It is separate from `index.sqlite` on purpose. The thread index is a cache of
the folders and is rebuilt when it breaks; this file holds things the folders
cannot reproduce — above all the per-turn token deltas of an external harness,
whose session counters are cumulative and are gone once the session ends — so
an unreadable file is reported, never deleted and rebuilt.

## Metadata only

The store keeps identifiers, counts, names, timestamps and outcomes. It never
keeps message text, tool arguments, tool output, prompts or file contents;
those stay in the thread's `history.json`. Thread titles and project paths are
stored, and a title can come from the first words of a message, so treat the
file as private.

The link book is the one exception, and it is the app's own record rather than
a harness's: a link keeps the message its parent sent and the answer the child
returned (the `body` column below), because that is what the book is for. It
holds nothing that `history.json` does not already have.

## Schema

| object | holds |
| --- | --- |
| `threads` | one row per thread: `id`, `title`, `project`, `cwd`, `harness`, `model`, `effort`, `created_at`, `updated_at` |
| `turns` | one row per turn: `thread_id`, `started_at`, `finished_at`, `harness`, `model`, `input_tokens`, `output_tokens`, `cached_tokens`, `cache_write_tokens`, `cost_usd`, `tool_calls`, `status`, unique on `(thread_id, started_at)` |
| `tool_calls` | one row per finished tool call: `thread_id`, `turn_id`, `tool_name`, `success`, `exit_code`, `at` |
| `daily_usage` | a view over `turns`: `day`, `harness`, `model`, summed tokens, `cost_usd` and turn count |
| `thread_links` | one row per link between two threads: `id`, `orchestrator`, `target`, `state`, `started_at`, `settled_at`, `harness`, `files_changed`, and `body`, the whole link record as JSON |
| `orchestration_state` | `key`, `body`: the rest of the link book, under `grants`, `counters` and `fanOuts` |
| `store_meta` | internal bookkeeping, such as whether the one-time backfill ran |

Tokens and cost are `NULL` when a harness did not report them, and are never
guessed. A cost is kept only when the harness reports it in USD; another
currency is not converted. Timestamps are RFC 3339 in UTC. Migrations are numbered and recorded with their
names, like the thread index: a recorded name that does not match refuses to
open instead of skipping a step.

## Thread links

The link book — who started or messaged whom, what each link is doing, the
answers, and what a thread may do without asking again — is the app's memory
of delegation (`docs/orchestrator.md`). It used to live only in the webview's
local storage, where a reinstall, a second window or a wiped webview lost it
and no agent could read it. It is here instead:

- `thread_links` holds the links: the columns are the fields a question asks
  about, and `body` is the whole link as JSON, so a field a later release adds
  survives a save and a load. A link's message and answer live in that body.
- `orchestration_state` holds what is beside the links: `grants` (what a
  thread may do without asking), `counters` (the next link id of each parent)
  and `fanOuts` (the prompts run on several harnesses at once).
- Writing replaces the whole book in one transaction, so two windows saving in
  turn cannot leave half of each behind. It is bounded like the app prunes it
  (`prune` in `apps/desktop/src/state/orchestration.ts`): every active link,
  the 50 newest finished ones, and the fan-outs whose links survived (at most
  20). A book over 4 MiB is refused rather than stored.
- Reading answers `{ "stored": bool, "book": {...} }`. `stored` is false when
  nothing was ever saved, so the app moves its local copy in once instead of
  replacing it with an empty book.

The app asks through `_rustcode/data/links/load` and
`_rustcode/data/links/save` (`engine/src/acp/data.rs`); an engine without them
answers `unknown method`, and the app keeps its local copy as before.

## Writers

Two hooks, both chosen so no new code sits in a turn's critical path:

- **The built-in harness** writes one turn beside `track_usage`
  (`network/turn/finish.rs`), with the tool calls it made after its last user
  message. `usage_stats.json` is still written, so nothing that reads it
  changes.
- **An external harness** writes one turn when its prompt returns
  (`harness/session.rs`). Its `usage_update` is a session total, so the store
  keeps the growth over what the thread already has for that harness — the
  baseline includes a backfilled summary, so nothing is counted twice. A
  counter that goes backwards means a new session (a harness switch or a
  restarted harness): its whole total is that turn's, never a negative
  number.

  Not every harness reports tokens there: Claude Code and Pi send only the
  context gauge (`used`/`size`), which is not a token count and is never
  summed. Their real counts come from their own session logs, read off the
  async runtime at turn end (`engine/src/harness_usage.rs`):

  | harness | log | taken |
  | --- | --- | --- |
  | Claude Code | `<CLAUDE_CONFIG_DIR, ~/.claude and ~/.claude-*>/projects/<slug>/<native id>.jsonl`, every one that holds the session | `message.usage` of `type: assistant` lines, deduplicated by `message.id` because one message spans several lines |
  | Pi | `~/.pi/agent/sessions/<dir>/<started>_<native id>.jsonl` | `message.usage` of `type: message` lines, including its own USD cost |

  Only numbers are read: token counts, cost, timestamps, message and model
  names. Nothing is stored but the numbers, files over 512 MiB and lines over
  8 MiB are skipped, unparseable lines are ignored, and a missing log leaves
  `NULL`.

Every write logs and returns: a full disk or a locked database cannot fail a
turn.

## Backfill

On first open the store scans the thread folders once, on a background thread
at engine start (`thread_index::warm` has a sibling), and writes a `threads`
row for every thread from `metadata.json`, `title.txt`, `workspace.json` and
`harness.json`, plus one summary turn per external thread with
`status = 'backfill'`. The summary's tokens come from the harness's own log
when it has one, else from the session totals in its `usage_update`; its cost
is the harness's reported USD cost, else the log's. The pass carries a
version (`store_meta.backfill_version`); a later release that can read more
runs it again and rewrites only rows still marked `backfill`, so a live turn
is never overwritten and a refreshed summary is never added twice.
`rustcode usage` and `rustcode data` run the same pass if they are the first
thing to open the file.

The backfill cannot invent per-turn history for threads that ran before this
release: an external thread gets one summary turn, a built-in thread gets a
thread row and no turns.

## Reading

`rustcode data query "<sql>"` runs one statement through a read-only
connection (`SQLITE_OPEN_READ_ONLY`, `PRAGMA query_only = ON`), with `ATTACH`
disabled, a progress handler that stops a query after two seconds, and at most
1000 rows. Writes, `ATTACH`, a second statement and reads of another database
file are refused. `rustcode data schema` prints the tables and view.

## CLI

```console
rustcode usage                 # tokens per harness and model, all turns
rustcode usage --since 2026-11-01 --json
rustcode data query "SELECT * FROM daily_usage ORDER BY day DESC LIMIT 7"
rustcode data schema
```

`rustcode usage` reports tokens and the cost the harness reported in USD — a
blank cost means the harness reported none, and RustCode does not price
requests itself. A row with no token counts prints `—`, not `0`, and the last
row totals the table. `rustcode storage usage` is a different command: it
reports disk space, and lists `data.sqlite` among the stores.

## For agents

Two orchestration tools (v4, `docs/orchestrator.md`) read the store for an
agent in a thread, forwarded through the app like `list_sessions`:

| Tool | Args | Answer |
| --- | --- | --- |
| `thread_usage` | `{ since?, group_by?, project? }` — `group_by` is `harness` (default), `model`, `day`, `project` or `thread`; `since` is `YYYY-MM-DD` or `YYYY-MM`; `project` is an absolute path | `{ group_by, since, project, scope, rows, totals, truncated, note }`, each row with input, output, cached and cache-write tokens, `cost_usd`, `turns` and `turns_without_tokens`; a group that reported nothing keeps `NULL`, never a `0` |
| `query_data` | `{ sql, limit? }`, one read-only statement, at most 1000 rows | `{ columns, rows, truncated }` through the guards above; its description lists every table and column, including `thread_links`, so an agent needs no extra call |

Both are reads, so neither asks the user. They follow **Let agents read
threads in other projects** (Settings › Agents, on by default), because the
store crosses projects and holds thread titles and paths:

- With it on, `thread_usage` covers every project, or the one `project` names.
- With it off, `thread_usage` is scoped to the calling thread's project even
  when another project is asked for, and a request naming another project is
  refused.
- `query_data` needs it on: SQL cannot be narrowed to one project safely (a
  join, a subquery, a view), so with the setting off it answers `not_allowed`
  and names the setting.

Because `query_data` is refused while that setting is off, `thread_links` (and
its `body`, which carries a link's message and answer) does not widen what a
caller can read: with the setting on, the caller could read those threads
anyway.

A shell has the same data without an app: `rustcode usage` and
`rustcode data query`. The engine answers the app through
`_rustcode/data/usage` and `_rustcode/data/query` (`engine/src/acp/data.rs`).

## Not in this store

Prices (only a cost a harness reports is kept), message text, tool arguments
and output, and per-turn history for turns recorded before this file existed.
Opencode reports no token counts RustCode can read, so its turns keep `NULL`
tokens and only its reported cost.
