# Building the app UI from the TUI's data

The terminal owns the session, model, tools, history and timing. The app renders
the remote projection of that same state and sends the supported commands back
to the terminal. It does not need a second agent loop or its own work timer.

This guide supplements [the wire contract](README.md). Copy the schemas and
golden frames from `v1/` using the app's `Scripts/sync-protocol.sh` after the
RustCode PR merges. No Swift changes are included in the RustCode change.

## Timing fields and TUI labels

All new fields are optional in protocol v1. Decode missing values as unknown,
not zero. A genuinely measured duration can be zero milliseconds. Existing
stored sessions are not backfilled from message timestamps.

| Source | Meaning | App display matching the TUI |
| --- | --- | --- |
| `message.thought_time_ms` | Thinking time for this persisted assistant segment, summed across its thinking blocks and any provider continuation that contributed to it. | One aggregate `Thought for 4s` label for the segment when the value is `4000`. |
| `snapshot.turn.thought_time_ms`, `text_delta.thought_time_ms` | Measured thinking accumulated in the current live assistant segment, including an open thinking interval. Resets for the next provider response. | Update the live segment's thinking label; do not accumulate this value as a delta. |
| `message.response_time_ms` | The existing persisted assistant/phase response timing, exported unchanged. Intermediate phases have their own timing; the final visible response can carry the whole logical turn's work time for the TUI footer. | Preserve for phase details and older stored TUI responses. Do not sum these values to calculate total turn work. |
| `message.completed_at` | Existing persisted finish timestamp on a completed final visible response. | Legacy completed-response display when authoritative turn metadata is unavailable. |
| `turn.elapsed_work_ms` | Total logical turn work, measured by the existing monotonic engine work clock across harness resumes. Excludes approval/question waits and gaps between runs. | `Working …` while active; `Worked for 32s` for `32000` after termination. |
| `turn.started_at` | Actual RFC 3339 wall time of the first run, retained across resumes. | Start-time detail; never subtract it from `ended_at` to obtain work duration. |
| `turn.ended_at` | Actual RFC 3339 terminal boundary, persisted once. | Format in the phone's chosen time zone, e.g. `Completed at 18:42`. |
| `turn.outcome` | `completed`, `cancelled` or `failed`; absent for active/suspended work. | Completed, Stopped or Failed state. Use this outcome rather than assuming every `turn_finished` means success. |
| `turn.turn_id` | Opaque durable logical-turn identity. New IDs are UUID based. | Group phases and render one footer for the logical turn. Do not parse an ID's spelling. |

Here `turn` means the timing object found at `snapshot.turn.timing`,
`snapshot.last_turn`, `message.turn`, or an event's `timing`. The active
`snapshot.turn.turn_id` remains the identity used by `cancel_turn`.

The aggregate thinking label belongs to the assistant segment, not each
individual `<think>` block. For two blocks in one segment with a combined
`thought_time_ms: 4000`, show one `Thought for 4s` label. The protocol does not
provide each block's individual duration; repeating `4s` on both would be
incorrect. Keep different assistant/tool phases separate, even when they share
a logical turn ID. Live response text is a temporary segment; reconcile it
with the persisted transcript rather than adding a second final answer.

## Concrete live and historical examples

These are representative payloads; IDs are illustrative. The committed golden
frames contain complete envelopes for decoder tests.

An active text event can carry the current clocks. A clock-only event has an
empty `text`; append no content, replace the clock values, and consume its
sequence normally. Clock-only updates are emitted at most once per displayed
second; text-bearing updates can arrive more often.

```json
{
  "kind": "event",
  "protocol_version": 1,
  "session_id": "session-example",
  "registration_epoch": 3,
  "sequence": 414,
  "generation": 0,
  "event": {
    "type": "text_delta",
    "text": "",
    "thought_time_ms": 4000,
    "timing": {
      "turn_id": "turn:example",
      "started_at": "2026-10-09T18:41:00+02:00",
      "elapsed_work_ms": 12000
    }
  }
}
```

The terminal event carries the frozen work measurement before active state is
cleared. This example spent 60 seconds on the wall clock, of which 28 seconds
were user waits or gaps between runs. The correct footer uses 32 seconds.

```json
{
  "kind": "event",
  "protocol_version": 1,
  "session_id": "session-example",
  "registration_epoch": 3,
  "sequence": 422,
  "generation": 0,
  "event": {
    "type": "turn_finished",
    "turn_id": "turn:example",
    "timing": {
      "turn_id": "turn:example",
      "started_at": "2026-10-09T18:41:00+02:00",
      "ended_at": "2026-10-09T18:42:00+02:00",
      "elapsed_work_ms": 32000,
      "outcome": "completed"
    }
  }
}
```

A snapshot received after completion has no active `turn` and retains this
timing object as `last_turn`. The transcript tail and `get_history.messages`
carry it on correlated messages, including a user/system message when there
was no assistant answer. A persisted assistant segment can look like:

```json
{
  "message_id": "m62",
  "role": "assistant",
  "content": {
    "text": "<think>Check both paths</think>Done",
    "truncated": false,
    "offset": 0,
    "total_bytes": 35,
    "content_id": "message:h2:62"
  },
  "timestamp": "2026-10-09T18:41:30+02:00",
  "response_time_ms": 12000,
  "thought_time_ms": 4000,
  "completed_at": "2026-10-09T18:42:00+02:00",
  "turn": {
    "turn_id": "turn:example",
    "started_at": "2026-10-09T18:41:00+02:00",
    "ended_at": "2026-10-09T18:42:00+02:00",
    "elapsed_work_ms": 32000,
    "outcome": "completed"
  }
}
```

`thought_time_ms: 4000` gives **Thought for 4s** once for that segment.
`turn.outcome`, `turn.ended_at` and `turn.elapsed_work_ms` give
**Completed at 18:42 · Worked for 32s** when displayed in Europe/Oslo.
The two values `response_time_ms: 12000` and `turn.elapsed_work_ms: 32000`
demonstrate why phase timing and logical-turn timing must stay separate.

Cancellation uses `turn_cancelled` with `timing.outcome: "cancelled"`.
Failure uses `turn_finished` with `timing.outcome: "failed"`. Both have their
own measured work and terminal time; neither should display `Completed`.
`v1/golden/frames/event_turn_failed.json` and `event_turn_cancelled.json`
provide complete examples. An old checkpoint may have an identifiable resumed
turn but unknown original start/work: those fields stay absent, while its
observed terminal time and outcome can still be supplied.

## Reducer and reconnect rules

1. Key session state by `session_id` and `registration_epoch`. Within it, key
   logical turn metadata by `turn_id` and durable messages by
   `(history_revision, message_id)`.
2. An attached snapshot replaces session state at `snapshot.sequence`.
   Keep its optional `snapshot_id` in the reconnect cursor as well as the
   applied sequence, gateway ID, instance ID and registration epoch. Send
   `snapshot_id` in `attach_session.resume` to resume at the current watermark;
   refresh it on every replacement snapshot, and retain it across later events.
   Restore active clocks from `turn.timing`, terminal footers from `last_turn`
   and `transcript[].turn`, and thinking labels from each assistant segment.
3. Apply subsequent numbered events exactly once. Replace timing values;
   never add an aggregate clock to a previous clock. Empty `text_delta` is a
   normal numbered event. A clock value can decrease when a user wait is
   excluded or a new assistant segment begins; it is not a sequence reset.
4. On `turn_finished`/`turn_cancelled`, update the named turn from `timing`.
   Clear active UI only if its ID matches. When timing is absent on an older
   host, use the event's legacy lifecycle semantics without inventing clocks.
5. On `resumed`, retain state and apply replay. On `attached.resync` or a
   replacement snapshot, use the snapshot's authoritative state and watermark.
   Live and terminal timing use the existing replay ring and frame limits.
6. Use `get_history` and `get_content` for older/truncated data. Each page's
   message timing is self-contained; deduplicate repeated turn metadata so
   multiple phases produce one turn footer. `last_turn` is the latest terminal
   record even when its original prompt is outside the bounded transcript tail.

An internal harness/background continuation retains its logical ID, actual
first start and prior measured work. Its intermediate run boundary is not a
terminal completion event. During a gap, history can contain timing with no
`outcome`; background task/session activity describes what is currently
happening. A new user prompt has its own ID. Do not infer completion from an
idle session, an absent active `turn`, assistant prose, or `message.timestamp`.

## TUI surfaces and app controls

| TUI surface | Remote data | App action |
| --- | --- | --- |
| Shared-session picker | `sessions`, title/workspace/model/activity/attention/health | `list_sessions`, `subscribe_sessions`, `attach_session`, `detach_session` |
| Transcript and live answer | `snapshot.transcript`, `turn.live_response`, `text_delta`, history revision/cursors | `get_history`, `get_content` |
| Working/thinking/footer rows | Active timing, segment thought timing, terminal turn records | Render measured values using the rules above |
| Composer when idle | `session.activity`, prompt receipts | `submit_prompt` with ordinary text |
| Follow-up while working | `turn.can_steer`, `pending_prompts` | `steer` when supported, or `queue` while a turn is running |
| Stop current work | `turn.turn_id` | `cancel_turn` naming that exact ID |
| Question modal | Pending question ID, header/text/options/descriptions, multiple flag, chain position/length | `answer_question` with selected labels or non-empty custom text |
| Approval modal | Batch ID, tool names/summaries/risk/details | Fetch truncated details with `get_content`; `resolve_approval` for the whole batch |
| Tool progress/results | Active tools and `tool_started`/`tool_finished`; transcript tool metadata/content | Render progress, success/failure/pending state, and result previews |
| Subagent/task panels | Subagents: ID/name/task/model/status/parent/depth/active/message count; background tasks: ID/command/elapsed | Render and reconcile updates/snapshots |
| Connection/retry status | Owner health, registration, receipt states and resync/session-close reasons | Reconnect/attach; query `get_request_status` before deciding whether a mutation was applied |

The phone can drive the supported work flows without touching the terminal's
draft. The terminal must stay open and the session must be explicitly shared.
Use the engine's rejections as authoritative when the UI state races a command:
`busy`, `not_running`, `unsupported_operation`, and the stale identity errors.

Full TUI parity still needs future protocol work. v1 does not expose remote
model/provider/configuration changes, approval-mode changes, arbitrary slash
commands, creating/loading unshared sessions, filesystem editing, or commands
to interrupt/reconfigure individual subagents/background tasks. Shared session
titles/models are display data, not editable settings. Tool command text is
currently display content rather than a separately typed copy-command field.
`can_steer` is supplied in snapshots, question options use labels, and approvals
act on a batch. Build these existing surfaces from the contract; add explicit,
tested operations for further controls instead of sending terminal slash
commands through `submit_prompt`.
