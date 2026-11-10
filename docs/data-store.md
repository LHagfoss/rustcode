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

Command receipts keep even less: the orchestration tool, a hash of its
arguments and the answer. No prompt, message or argument text is stored, so a
receipt cannot leak what a call said.

The request and protocol tables below record metadata by default. They hold a
payload only when the user turns payload capture on ("Captured payloads").

## Schema

| object | holds |
| --- | --- |
| `threads` | one row per thread: `id`, `title`, `project`, `cwd`, `harness`, `model`, `effort`, `parent_id`, `parent_title`, `created_at`, `updated_at` |
| `turns` | one row per turn: `thread_id`, `started_at`, `finished_at`, `harness`, `model`, `input_tokens`, `output_tokens`, `cached_tokens`, `cache_write_tokens`, `cost_usd`, `tool_calls`, `status`, `duration_ms`, `time_to_first_token_ms`, `model_time_ms`, `tool_time_ms`, `context_tokens`, `context_window`, `stop_reason`, unique on `(thread_id, started_at)` |
| `tool_calls` | one row per finished tool call: `thread_id`, `turn_id`, `tool_name`, `success`, `exit_code`, `at`, `started_at`, `duration_ms`, `output_bytes`, `truncated`, `mcp_server`, `command_name`, `error_kind` |
| `compactions` | one row per history compaction, from any harness: `thread_id`, `turn_id`, `harness`, `model`, `at`, `duration_ms`, `trigger`, `mode`, `tokens_before`, `tokens_after`, `messages_before`, `messages_after`, `idle_secs_before`, `count`, `success`, `error_kind`, `external_id`, `source` |
| `daily_usage` | a view over `turns`: `day`, `harness`, `model`, summed tokens, `cost_usd` and turn count |
| `daily_compactions` | a view over `compactions`: compactions, failures, tokens before and after, reclaimed tokens and mean duration per day, harness and trigger |
| `tool_failure_rates` | a view over `tool_calls`: calls, failures, failure rate, mean duration and truncated results per day and tool |
| `turn_latency_daily` | a view over `turns`: turn count, mean wall, first-token, model, tool and context values per day, harness and model |
| `thread_links` | one row per link between two threads: `id`, `orchestrator`, `target`, `state`, `started_at`, `settled_at`, `harness`, `files_changed`, and `body`, the whole link record as JSON |
| `orchestration_state` | `key`, `body`: the rest of the link book, under `grants`, `counters` and `fanOuts` |
| `provider_requests` | one row per built-in provider HTTP attempt or retry: `thread_id`, `turn_id`, `round`, `provider`, `model`, `endpoint_host`, `endpoint_path`, `method`, `attempt`, `started_at`, `headers_at`, `finished_at`, `latency_ms`, `status_code`, `request_bytes`, `response_bytes`, `stream_event_count`, `input_tokens`, `output_tokens`, `cached_tokens`, `error_kind`, `request_payload_json`, `response_summary_json`, `payload_truncated`, `redaction_version` |
| `harness_rpc_messages` | one row per ACP or Codex JSON-RPC message: `thread_id`, `turn_id`, `harness`, `protocol`, `direction`, `message_kind`, `rpc_id`, `method`, `at`, `latency_ms`, `byte_size`, `payload_json`, `payload_truncated`, `redaction_version` |
| `operational_events` | one row per allowlisted event: `thread_id`, `turn_id`, `sequence`, `event_type`, `at`, `fields_json`, `original_bytes`, `captured_bytes`, `payload_truncated`, `redaction_version` |
| `command_receipts` | one row per orchestration call made with a retry key: `caller` (the calling thread), `client_request_id`, `action` (the tool), `args_hash`, `result_json`, `created_at`, keyed on `(caller, client_request_id)` |
| `usage_cursors` | one row per thread, harness and session series (the harness's native session id and its account): the last cumulative session total that series reported (`input_tokens`, `output_tokens`, `cached_tokens`, `cache_write_tokens`, `cost_usd`) and `updated_at` |
| `store_meta` | internal bookkeeping, such as whether the one-time backfill ran |

Tokens and cost are `NULL` when a harness did not report them, and are never
guessed. A cost is kept only when the harness reports it in USD; another
currency is not converted. Timestamps are RFC 3339 in UTC. Migrations are numbered and recorded with their
names, like the thread index: a recorded name that does not match refuses to
open instead of skipping a step.

## Structured rows grow forever, payloads do not

The store is a record of what happened, so the small structured rows are kept:
`threads`, `turns`, `tool_calls`, `compactions`, `operational_events`,
`thread_links` and the usage cursors are never deleted by the retention pass.
They are bytes to a few hundred bytes each.

Only the raw protocol payloads are bulky, and only they are pruned:

- streaming output (`*/outputDelta`, `agent_message_chunk`,
  `agent_thought_chunk`, `user_message_chunk`) is folded per flush window
  into one row carrying only its counts (`coalesced`, `update`, `bytes`). A
  raw chunk body over 512 bytes is dropped on every pass; the folded row's
  counts stay;
- repeated status pings (`account/rateLimits/updated`,
  `mcpServer/startupStatus/updated`) keep their metadata row and lose their
  body on every pass;
- other raw payload bodies older than 7 days are set to `NULL`, and the
  metadata row (`method`, `at`, `latency_ms`, `byte_size`) stays;
- the raw payloads are held under a 256 MiB budget by dropping the oldest
  first, then the oldest raw rows;
- `telemetry_retention` in `config.toml` is `forever` by default. A number of
  days deletes raw `provider_requests` and `harness_rpc_messages` rows older
  than that; it never deletes a structured row.

So the payload side is bounded (7 days, under 256 MiB), while the row count
keeps growing: structured rows are one per turn, tool call, compaction or
operational event, and raw rows are one per protocol frame, with a folded
streaming row per flush window and chunk kind instead of one per token. Under
`forever` nothing deletes those raw rows, so a busy machine still adds
tens of thousands of them a day (a few megabytes). A number of days in
`telemetry_retention` bounds that, and structured history stays either way.
`rustcode data stats` prints each table's row count and size, so the growth
can be watched and the raw window set from it.

## Compactions

Every compaction RustCode can see is one row, whichever harness ran it:

| source | how it is seen | trigger |
| --- | --- | --- |
| built-in harness | `AppState::record_compaction`, where the chat card and `context.compaction` event are written | `auto_context` (the history crossed its budget) or `manual` (`/compact`) |
| Codex | the `contextCompaction` item (`item/completed`) | `manual` for a `/compact` turn, else `harness_auto` |
| Claude Code | the ACP adapter's synthetic `Compact conversation` tool call (`_meta.claudeCode.toolName = "compact"`), terminal update only | Claude's `trigger`: `manual`, `harness_auto` for its automatic pass, `cache_expiry` for the adapter's idle compaction |

`mode` is `ai_summary` or `deterministic_record` for a RustCode compaction and
`harness_summary` when the harness made it. Token counts, message counts and
duration stay `NULL` when the source did not report them, never `0`. A
harness's own event id (`external_id`) makes a repeated protocol update update
its row instead of adding a second one.

A harness that never reports a compaction has no row: RustCode does not guess
one from timing. The Claude Code adapter of this release does forward the
event (as the tool call above), even though it does not send the newer ACP
`compaction_update` notification RustCode does not advertise.

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

## Command receipts

An orchestration call that can start a thread or deliver a message
(`start_session`, `message_session`, `fan_out`) may carry a
`client_request_id` (`docs/orchestrator.md` § Retrying a call). The app asks
this store, through `_rustcode/data/receipts/get` and
`_rustcode/data/receipts/put`, *before* it acts, and records the answer right
after the call succeeds. So a retry whose answer was dropped — at the MCP
layer, or because the app restarted — replays the recorded answer instead of
starting or delivering a second time.

- The key is the calling thread plus the caller's `client_request_id`, so two
  threads may use the same key and one caller's key never answers another's.
- `args_hash` is the engine's sha256 of the tool and the call's arguments with
  object keys sorted (`orchestration::args_hash`). The same key with another
  action or another hash is a conflict, not a replay.
- `result_json` is the answer the app gave; nothing else about the call is
  written, so no prompt or message text is here.
- The first receipt under a key stays: a later write under the same key does
  nothing, so a race cannot overwrite what actually happened.
- Only a successful call is recorded. A refusal (the user said no, a busy
  limit) or a failure records nothing, so a corrected call may use the key.

Receipts age out after 7 days and the newest 10,000 are kept, pruned by the
same hourly pass as the telemetry tables.

## Request and protocol telemetry

Three tables record what a turn did on the wire, so a slow or failing turn can
be read back instead of guessed at:

- `provider_requests`: one row per attempt of the built-in harness's own HTTP
  request, retries included. `endpoint_host` and `endpoint_path` are stored
  without the query or fragment, which can carry a key. `error_kind` is a
  short classification (`timeout`, `connect`, `cancelled`, `stream`,
  `http_500`, `rate_limit`, `insufficient_balance`), never the provider's message text. A row is written when the
  attempt ends, so `started_at`, `headers_at` and `finished_at` bound the
  send, the wait for headers and the stream separately, and `latency_ms` is
  the whole attempt — the backoff before a retry belongs to no row. `attempt`
  counts the sends of one request from 1, and each retry is a separate row
  with its own status, bytes and error kind. `provider` is the profile's
  credential provider (`openai`, `claude`, `github-copilot`), else its engine,
  else the profile name; `round` is the turn round the request belongs to,
  zero-based like `operational_events`. Tokens are the ones that response
  reported and stay `NULL` for a side request whose usage is not parsed.
- `harness_rpc_messages`: one row per JSON-RPC request, response or
  notification that crossed RustCode's ACP or Codex stdio boundary.
  `direction` is `harness_to_rustcode` or `rustcode_to_harness`, and
  `message_kind` is `request`, `response` or `notification`. `protocol` is
  `acp` or `codex-app-server`. `byte_size` is the frame's length and `at` the
  moment it parsed (inbound) or was handed to the writer (outbound); a
  response carries the method of the request it answers and, for a request
  RustCode or the harness sent through this engine, how long it took as
  `latency_ms`.

  Streamed text is folded, not stored frame by frame: per flush window
  (about a second) one row stands for every `session/update` notification of
  the same chunk kind (`agent_message_chunk`, `agent_thought_chunk`,
  `user_message_chunk`) and for Codex's delta notifications
  (`item/agentMessage/delta`, `item/reasoning/*Delta`). Its `byte_size` is
  the total, and its `payload_json` is metadata that is there even with
  payload capture off: `{"coalesced": <frames>, "update": "<chunk kind>",
  "bytes": <total>}`. Every other message gets a row of its own. The
  recorder batches rows and writes them to the store about once a second, a
  full 128-row buffer, or when the harness stops, so a locked store cannot
  stall a turn; a buffer over 4096 rows drops them and logs the count.
- `operational_events`: a small allowlisted projection for an agent to read,
  with a per-thread `sequence` the store assigns (the cursor a reader pages
  with) and safe, bounded fields only.

The projection is written from the same `operational_event` call that writes
`debug.log` (`engine/src/logger.rs`), off the caller's path and only when the
event has an owning thread. **Only these events are projected**, with the
fields listed; every other event and field stays in `debug.log`:

| event | fields kept |
| --- | --- |
| `turn.start` | `wakeup`, `tool_rounds`, `segment_rounds`, `segment_limit`, `total_round_limit` |
| `turn.finish` | `completed_task`, `tool_rounds`, `content_bytes`, `cancelled`, and the scalar members of `metrics` |
| `turn.performance` | the scalar members of `performance` (wall, model, tool and user-wait times, tokens, rounds, calls) |
| `turn.stream_retry` | `attempt`, `output_phase`, `reason` (never `error`) |
| `turn.stream_recovery` | `attempt`, `kind`, `partial_content_bytes`, `outcome` |
| `turn.budget_exceeded` | `limit`, `tool_rounds`, `elapsed_secs`, `tokens_used`, `failed_mutations`, `segment_rounds`, `segment_limit`, `total_round_limit`, `continuation` |
| `turn.recovery_decision` | `round`, `source`, `decision`, `reason`, `loop_recovery_attempts`, `reasoning_recovery_attempts`, `no_progress_streak` |
| `tools.batch.finish` | `count`, `requested`, `executed`, `deferred`, `queued`, `auto_executed`, `successes`, `failed`, `pending`, `changed_paths` |
| `context.compaction` | `trigger`, `mode`, `before_tokens`, `after_tokens`, `reclaimed_tokens`, `count`, `history_tokens`, `budget`, `messages_considered`, `summary_generated`, `originating_path`, `deterministic_record` |
| `harness.reaped` | `harness`, `native_session_id`, `reason`, `idle_ms` |
| `harness.resumed` | `harness`, `native_session_id` |
| `harness.turn_silent` | `harness`, `native_session_id`, `timeout_ms` |
| `subagent.spawn` | `agent_id`, `parent_id`, `agent_type`, `context_inheritance`, `inherited_messages` |
| `subagent.finish` | `agent_id`, `status`, `reason`, `rounds` |

Numbers and booleans are kept as they are, strings are clipped to 96
characters, and an allowlisted object keeps its scalar members only — arrays
(dropped) can name paths with user content, and a provider's error text is
never in the list. The insert bounds the fields object at 8 KiB as well.

**These are protocol messages, not provider traffic.** An external harness
(Claude Code, Codex, Gemini CLI, OpenCode, Pi) is a separate process that owns
its own model login and its outbound provider HTTP; RustCode sees only the
messages that harness sends it. Nothing in these tables is a copy of an
external harness's provider request or response, and no column should be read
as one.

### Captured payloads

Metadata is always recorded; payloads are opt-in, and off by default:

| config key | default | what it adds |
| --- | --- | --- |
| `capture_provider_payloads` | `false` | `provider_requests.request_payload_json` (the serialized request) and `response_summary_json` (a summary, never the raw successful stream) |
| `capture_harness_rpc_payloads` | `false` | `harness_rpc_messages.payload_json` |

`telemetry_retention` (default `"forever"`, or a number of days) is separate
from those two: it only bounds raw protocol rows, never structured rows
("Structured rows grow forever, payloads do not"). It is global only.

`debug_verbose_network_logging` is a separate flag for `debug.log` output and
does not turn any of this on. Project configuration cannot enable either key:
capturing payloads is a choice about the user's own data, so it is global
only, like skill roots.

The app writes both keys from Settings › Advanced › **Record request bodies**
through `_rustcode/config/telemetry` / `_rustcode/config/telemetry/set`
(`engine/src/acp/telemetry.rs`), which edits only these two keys and leaves the
rest of `config.toml` as it was. The engine reads them from the config it
started with, so a change applies after it restarts (the row offers that
directly); the terminal keeps using the keys by hand.

Every payload is redacted and clipped before it is written
(`engine/src/data_store/redact.rs`):

- A value under a key naming a credential — case-insensitive, separators
  ignored, matching `authorization`, `api_key`, `apikey`, `token`, `secret`,
  `credential`, `password`, `cookie` — is replaced with `[redacted]` at any
  depth. The match is deliberately broad, except that a number, `true`/`false`
  or `null` under such a key is kept: `max_tokens` and `input_tokens` are counts,
  and a secret is never a bare number.
- Any `Bearer <token>` inside a string is replaced.
- The result is clipped to the cap below while staying valid JSON: long
  strings are cut, array items and object entries are dropped, and the drop is
  marked (`__truncated` in an object, a `[truncated]` item in an array).
  `payload_truncated` says a row was clipped.
- `redaction_version` records which rules were applied (0 when the row holds no
  payload), so a rule change is visible in the data.

Caps: 64 KiB per provider payload, 16 KiB per RPC message, 8 KiB per
`operational_events` fields object. A caller that sends more is clipped at the
insert, so a mistake cannot write past the cap.

### Retention

A pass runs at most once an hour, from the store's open path (every writer
opens the store) and at the start. Structured rows are never deleted; see
"Structured rows grow forever, payloads do not" above for the policy, and
`telemetry_retention` in `config.toml` for the raw-row window.

- streaming-delta bodies and status-ping payloads are dropped on every pass;
- payload columns of raw rows older than 7 days are set to `NULL`, and the
  metadata row stays;
- when `telemetry_retention` is a number of days, raw protocol rows older
  than that are deleted; the default `forever` keeps them;
- command receipts older than 7 days are deleted, then the oldest until at
  most 10,000 are left: they exist to make a retry safe for a while, not to
  keep history (`COMMAND_RECEIPT_RETENTION_DAYS`, `COMMAND_RECEIPT_MAX_ROWS`);
- if the raw tables hold more than 256 MiB of payload together (estimated as
  the summed length of the payload columns), payloads are dropped first and
  the oldest raw rows only after that;
- the WAL is checkpointed (`PRAGMA wal_checkpoint(TRUNCATE)`), and freed pages
  are reclaimed with `PRAGMA incremental_vacuum` only when the database already
  vacuums incrementally — turning that on for an existing file needs a full
  `VACUUM`, which is not something a background pass starts on a user's store.

Every write logs and returns: a locked or full store cannot fail a turn.

The inserts are the API other modules call: `record_provider_request`,
`record_rpc_message`/`record_rpc_messages` (one transaction for a batch) and
`record_operational_event` (which returns the per-thread sequence). The
built-in harness's provider hook is in `network/stream_request.rs` and the
harness RPC hook in `harness/rpc_capture.rs`.

## Writers

Two hooks, both chosen so no new code sits in a turn's critical path:

- **The built-in harness** writes one turn beside `track_usage`
  (`network/turn/finish.rs`), with the tool calls it made after its last user
  message. `usage_stats.json` is still written, so nothing that reads it
  changes.
- **An external harness** writes one turn when its prompt returns
  (`harness/session.rs`). Its `usage_update` is a session total, so the store
  keeps the growth over the last total the same *session series* reported — a
  `usage_cursors` row keyed by thread, harness and series, where the series is
  the harness's native session id and the account it ran as. It is written in
  the same transaction as the turn, under `BEGIN IMMEDIATE` so a concurrent
  writer cannot leave it with a read snapshot it may no longer upgrade.

  The summed rows are not the baseline: after a restarted harness that sum
  stays above the running total, and every later turn would be recorded whole
  again (#3169). A turn that reports the same total as the one before it
  records zero, which is what a failed turn that changed nothing gets. A
  series the store has not seen (a restart, a harness switch, another account)
  reports a whole session total, which is that turn's. Within one series only
  every reported counter dropping together — or the series changing — is a
  restart; a single counter that dipped (a rotated or truncated log, a missing
  account directory) records zero and keeps the higher cursor, instead of
  re-counting a session whose other counters say never restarted. No counter
  ever records a negative number.

  A thread recorded before cursors existed (an older release, or a backfill)
  has no cursor to read. Its rows are summed, and the lower of that sum and the
  current reading is used as the baseline, because the sum can already be
  inflated: the first turn after the release counts nothing it had already
  counted rather than adding one more whole total on top. The cost is a
  restart that happened between those rows and this reading, which is not
  counted; that undercounts one turn instead of double counting a session. The
  rows themselves are left alone (see "What the store does not repair").

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

A second one-time pass runs at engine start, before the first session list
is read (`thread_parent::ensure_ready`): it records the parent of threads
delegated before that record existed, reading each thread's first
`From thread “…” (agent):` heading (a byte scan; only a match is parsed) and
writing `metadata.json`'s `parent`, then marks itself done with
`store_meta.thread_parents_version`. It changes no transcript and calls no
model; after the first run it reads that one row and returns.

The backfill cannot invent per-turn history for threads that ran before this
release: an external thread gets one summary turn, a built-in thread gets a
thread row and no turns. It also carries each thread's recorded parent
(`metadata.json`) into `threads.parent_id`/`parent_title`, so a usage read can
roll delegates up without the app's session list; bumping the backfill version
refreshes those columns for existing rows.

## What the store does not repair

Rows written before migration 6 can hold a whole session total where one
turn's growth belongs (#3169). No pass rewrites them. A row does not say
whether it is a growth or a total, so recomputing each row as the difference
from the previous one would undercount the rows that are already right, and
the harness logs cannot stand in: `harness.json` keeps only the thread's
*current* native session id, so the logs of earlier sessions cannot be found
from the store, and a log's per-message usage carries no turn boundary. New
turns are exact from the first turn after the upgrade, and `usage_cursors`
keeps them exact from there.

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
rustcode data query "SELECT * FROM daily_compactions ORDER BY day DESC LIMIT 7"
rustcode data schema
rustcode data stats            # rows and bytes per table
```

`rustcode usage` reports tokens and the cost the harness reported in USD — a
blank cost means the harness reported none, and RustCode does not price
requests itself. The answer counts missing data per column:
`turns_without_input_tokens`, `turns_without_output_tokens`,
`turns_without_cached_tokens`, `turns_without_cache_write_tokens`,
`turns_without_any_tokens` (any of the four missing), and
`turns_without_cost`. `turns_without_tokens` is the older, narrower count of
turns where **both** input and output are missing. A row with no token counts
prints `—`, not `0`, and the last row totals the table; a total summed from
only the turns that reported a column is marked partial. `rustcode storage
usage` is a different command: it reports disk space, and lists `data.sqlite`
among the stores.

## For agents

Three orchestration tools (v4 and v6, `docs/orchestrator.md`) read the store
for an agent in a thread, forwarded through the app like `list_sessions`:

| Tool | Args | Answer |
| --- | --- | --- |
| `thread_usage` | `{ since?, group_by?, project? }` — `group_by` is `harness` (default), `model`, `day`, `project` or `thread`; `since` is `YYYY-MM-DD` or `YYYY-MM`; `project` is an absolute path | `{ group_by, since, project, scope, rows, totals, truncated, note }`, each row with input, output, cached and cache-write tokens, `cost_usd`, `turns`, `turns_without_tokens`, `turns_without_any_tokens`, `turns_without_input_tokens`, `turns_without_output_tokens`, `turns_without_cached_tokens`, `turns_without_cache_write_tokens`, and `turns_without_cost`; a group that reported nothing keeps `NULL`, never a `0`, and a thread row also carries its `model` and `model_harness` (only when every turn ran the same harness and model, else `NULL`), `parent_id` and `parent_title` |
| `query_data` | `{ sql, limit? }`, one read-only statement, at most 1000 rows | `{ columns, rows, truncated }` through the guards above; its description lists the tables and their key columns, including `thread_links`, `provider_requests`, `harness_rpc_messages` and `operational_events`, so an agent needs no extra call |
| `read_activity` | `{ session_id, after?, limit?, order? }` — `limit` 1..200 (default 50); `order` is `oldest` (default) or `newest`; pass `next` as `after` with the same order | `{ session_id, items, next, has_more, last_provider_error }`, one merged time-ordered list: `{ source: "event", at, type, fields }`, `{ source: "provider_request", at, provider, model, status_code, latency_ms, error_kind, input_tokens, output_tokens, cached_tokens, attempt }` and `{ source: "rpc_message", at, direction, kind, method, latency_ms, byte_size }`. Items are ordered by `(at, source, row id)`; `next` encodes the last one, so pages join with no gap and no repeat. Payload columns (`request_payload_json`, `response_summary_json`, `payload_json`) are never selected |

They are reads, so none asks the user. They follow **Let agents read threads
in other projects** (Settings › Harnesses, on by default), because the store
crosses projects and holds thread titles and paths:

- With it on, `thread_usage` covers every project, or the one `project` names.
- With it off, `thread_usage` is scoped to the calling thread's project even
  when another project is asked for, and a request naming another project is
  refused.
- `query_data` needs it on: SQL cannot be narrowed to one project safely (a
  join, a subquery, a view), so with the setting off it answers `not_allowed`
  and names the setting.
- `read_activity` applies the same reach `read_session` does: any project
  while the setting is on, this project's threads otherwise. It is a read, so
  it asks no approval, and it refuses the calling thread itself, which has its
  own transcript.

Because `query_data` is refused while that setting is off, `thread_links` (and
its `body`, which carries a link's message and answer) does not widen what a
caller can read: with the setting on, the caller could read those threads
anyway.

A shell has the same data without an app: `rustcode usage` and
`rustcode data query`. The engine answers the app through
`_rustcode/data/usage`, `_rustcode/data/query` and `_rustcode/data/activity`
(`engine/src/acp/data.rs`).

## Not in this store

Prices (only a cost a harness reports is kept), message text, tool arguments
and output, and per-turn history for turns recorded before this file existed.
Opencode reports no token counts RustCode can read, so its turns keep `NULL`
tokens and only its reported cost. Payloads are kept only when captured and
only for 7 days; an external harness's provider HTTP is never visible here at
all ("Request and protocol telemetry"). Per-call tool duration is `NULL`: a
harness's transcript does not carry it, so the store does not invent one; a
turn's `tool_time_ms` is recorded when the caller measured it.
