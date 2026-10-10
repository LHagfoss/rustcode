# Remote protocol v1

The wire contract between a remote client (the iOS app) and a host running
RustCode: JSON text frames over WebSocket to the remote gateway. How to run
the gateway, share a session and pair is in [`remote.md`](../remote.md); the
design is
[`Live terminal sessions from an iOS client`](../architecture/mobile-remote-plan.md).
The experimental `rustcode serve` protocol described in
[`mobile.md`](../mobile.md) is separate and unchanged.

The Rust types are the source of truth: `rustcode/engine/src/remote/protocol.rs`
for session frames, `rustcode/engine/src/remote_gateway/handshake.rs` and
`pairing.rs` for the handshake and the pairing QR payload. Everything under
`v1/` is generated from them:

| Path | Contents |
| --- | --- |
| `v1/handshake-request.schema.json` | JSON Schema (2020-12) for the first frame a client sends, `HandshakeRequest`. |
| `v1/handshake-response.schema.json` | JSON Schema for the handshake answers and the bare `error` frame, `HandshakeResponse`. |
| `v1/pairing-qr.schema.json` | JSON Schema for what the pairing QR code encodes, `QrPayload`. |
| `v1/request.schema.json` | JSON Schema for a client → host frame after the handshake, `RemoteRequest`. |
| `v1/frame.schema.json` | JSON Schema for a host → client frame after the handshake, `RemoteFrame`. |
| `v1/golden/handshake/requests/*.json` | One example of every handshake request. |
| `v1/golden/handshake/responses/*.json` | `paired`, `authenticated`, and one bare `error` per code. |
| `v1/golden/handshake/pairing_qr.json` | An example QR payload. |
| `v1/golden/requests/*.json` | One example of every request shape. |
| `v1/golden/frames/*.json` | One example of every response, event and error shape. |

`cargo test` regenerates all of it and fails when a committed file differs, when
a golden frame does not round-trip or does not satisfy the schema, or when a
shape in the schema has no golden frame. After an intended change:

```sh
RUSTCODE_UPDATE_REMOTE_PROTOCOL=1 cargo test --lib remote::contract
```

and commit the result with the code change.

## Compatibility

Within version 1 the contract only grows: new optional fields, new enum
values, new event and result types. Nothing is renamed or removed. A client
must therefore **tolerate what it does not know**: ignore unknown fields, and
decode an unknown `type`, `kind`, `code`, `reason` or other enum value into an
explicit "unknown" case instead of failing the frame or the connection. An
unknown event of an attached session still advances `sequence`; an unknown
error `code` is still an error. A change that an old client could not survive
gets a new `protocol_version`.

The host omits absent optional fields; it never sends `null`, although the
schema tolerates it.

## Connection

1. Open a WebSocket to `ws://<address>/` without an `Origin` header (a request
   that carries one is refused with HTTP 403).
2. Send exactly one handshake frame, `pair` or `authenticate`, within 10
   seconds. Nothing is sent to an unauthenticated socket except the answer to
   that frame.
3. On `paired` or `authenticated` the connection is authenticated. From here
   on the client sends `RemoteRequest` frames and receives `RemoteFrame`
   frames.

The gateway sends a WebSocket ping every 30 seconds and closes a connection it
has heard nothing from for 90 seconds. Every frame counts, including WebSocket
pings and pongs, so a client that pings on its own schedule, or simply answers
the gateway's pings, stays connected.

### Handshake frames

Client → host, the first frame:

```json
{"type": "pair", "protocol_version": 1, "method": "credential", "secret": "<credential from the QR code>", "device_name": "Lars's iPhone"}
{"type": "pair", "protocol_version": 1, "method": "code", "secret": "4821-9034", "device_name": "Lars's iPhone"}
{"type": "authenticate", "protocol_version": 1, "device_id": "0011223344556677", "token": "<device token>"}
```

Host → client:

```json
{"type": "paired", "protocol_version": 1, "gateway_id": "…", "instance_id": "…", "device_id": "0011223344556677", "device_name": "Lars's iPhone", "token": "<device token>", "host_name": "studio"}
{"type": "authenticated", "protocol_version": 1, "gateway_id": "…", "instance_id": "…", "device_id": "0011223344556677", "device_name": "Lars's iPhone", "host_name": "studio"}
{"type": "error", "code": "pairing_failed", "message": "…"}
{"type": "error", "code": "rate_limited", "message": "…", "retry_after_secs": 240}
```

- `token` appears once, in `paired`. Store it in the Keychain; the host keeps
  only its digest. After `paired` the same connection is already
  authenticated.
- `gateway_id` is stable for the host and is what a client keys its stored
  token and host data by. `instance_id` changes every time the gateway
  starts.
- `host_name` is optional: the machine's own name, for display. It is a
  label, not an identity.
- `device_name` in the answer is the name as the host stored it (trimmed and
  bounded), which may differ from what was sent.

The pairing QR code encodes this JSON, as one line:

```json
{"protocol_version": 1, "address": "100.92.13.44:17879", "gateway_id": "…", "credential": "…", "host_name": "studio"}
```

`address` is `host:port` to dial. `host_name` is optional and cut to 24
characters. Send `credential` as the `secret` of a `pair` frame with
`"method": "credential"`.

### The bare `error` frame

`{"type": "error", "code": …, "message": …}` has no `kind` and no
`request_id`. It is a **connection-level** frame: the gateway sends it once
and then closes the connection. It is never the answer to a request on an
authenticated connection; those are always `response` frames (below).

| When | `code` | What the client does |
| --- | --- | --- |
| Handshake | `invalid_frame` | First frame was not a handshake frame, not JSON, or not text. A bug. |
| Handshake | `frame_too_large` | Handshake frame over 8 KiB. |
| Handshake | `unsupported_version` | `protocol_version` is not 1. Update the app. |
| Handshake | `handshake_timeout` | No frame within 10 seconds. |
| Handshake (`pair`) | `pairing_failed` | Wrong, expired, used or exhausted secret; indistinguishable on purpose. Ask for a new code. |
| Handshake (`pair`) | `rate_limited` | Too many failed pairings on the host; `retry_after_secs` says when to try again. |
| Handshake (`authenticate`) | `unauthorized` | Unknown or revoked device, or wrong token. Stop reconnecting; pair again. |
| Handshake | `busy` | Connection or device limit reached. Retry later. |
| Handshake (`pair`) | `internal` | The host could not store the device. |
| After authentication | `revoked` | The device was revoked. Stop reconnecting, delete the token, pair again. |
| After authentication | `slow_consumer` | The client did not read fast enough. Reconnect and resume. |
| After authentication | `idle_timeout` | Nothing was heard for 90 seconds. Reconnect and resume. |
| After authentication | `frame_too_large` | A frame over 1 MiB was sent. A bug. |
| After authentication | `invalid_frame` | A binary frame was sent. A bug. |
| After authentication | `shutting_down` | The gateway is stopping. Reconnect with backoff. |

`not_implemented` is reserved for a gateway without session routing and is not
sent by this one.

## Session frames

One JSON object per frame. Enums are flat objects tagged by `type` (the host
frame itself by `kind`).

Client → host:

```json
{
  "protocol_version": 1,
  "request_id": "req-0011",
  "session_id": "5f0c2d9e-7a41-4c1b-9d55-0b6c1f2a3e44",
  "registration_epoch": 3,
  "operation": { "type": "cancel_turn", "turn_id": "turn:48213:12" }
}
```

Host → client, a response correlated by `request_id`:

```json
{
  "kind": "response",
  "protocol_version": 1,
  "request_id": "req-0011",
  "receipt": "rejected",
  "result": { "type": "error", "code": "stale_turn", "message": "the named turn is no longer running" }
}
```

**Every request is answered by exactly one `response` frame with its
`request_id`**, whether it succeeded or failed, including a frame the host
could not parse (`invalid_request`, with `request_id` empty when the frame
carried none it could read). Responses to different requests may arrive in
any order, and events may arrive in between.

Host → client, a sequenced event of one attached session:

```json
{
  "kind": "event",
  "protocol_version": 1,
  "session_id": "5f0c2d9e-7a41-4c1b-9d55-0b6c1f2a3e44",
  "registration_epoch": 3,
  "sequence": 414,
  "generation": 1,
  "event": { "type": "text_delta", "text": "The failure comes from " }
}
```

The third host frame, `"kind": "sessions"`, is the live session list pushed
after `subscribe_sessions` whenever it changes. It replaces the list the
client holds.

`session_id` and `registration_epoch` are required on every operation except
`list_sessions`, `subscribe_sessions` and `get_request_status`. Device identity
comes from authentication and is never part of a frame.

## Operations

| `operation.type` | Answered by | Success `result.type` | Typed rejections |
| --- | --- | --- | --- |
| `list_sessions`, `subscribe_sessions` | gateway | `sessions` | |
| `attach_session` | gateway | `attached` (snapshot) or `resumed` (replay follows) | `not_found`, `stale_session`, `owner_unavailable` |
| `detach_session` | gateway | `detached` | |
| `get_history` | terminal | `history` | `stale_cursor` |
| `get_content` | terminal | `content` | `not_found`, `invalid_request` |
| `submit_prompt` | terminal | `prompt_accepted` | `busy`, `unsupported_operation` |
| `steer` | terminal | `prompt_accepted` | `not_running`, `unsupported_operation` |
| `queue` | terminal | `prompt_accepted` | `not_running`, `unsupported_operation` |
| `cancel_turn` | terminal | `turn_cancelled` | `stale_turn` |
| `answer_question` | terminal | `question_answered` | `stale_question`, `invalid_answer` |
| `resolve_approval` | terminal | `approval_resolved` | `stale_approval` |
| `get_request_status` | terminal that holds the receipt | `request_status` | |

Any request can also be rejected with:

| `code` | Meaning |
| --- | --- |
| `incompatible_version` | `protocol_version` is not supported; `supported_versions` lists what is. Checked before the operation is parsed. |
| `invalid_request` | Not a well-formed request for its operation. |
| `frame_too_large` | The request is over 256 KiB. |
| `unsupported_operation` | An operation this version does not know. |
| `not_found` | No session with this `session_id` is shared. It was never shared, or sharing ended. |
| `stale_session` | The session is shared, but under another `registration_epoch`: it was shared again. List the sessions and use the new epoch. |
| `rate_limited` | Too many of this connection's requests are waiting on terminals (16), or the terminal's queue is full. Nothing was applied; retry later with the same `request_id`. |
| `owner_unavailable` | The terminal did not answer within 30 seconds or went away. For a mutation the receipt is `unknown`. |
| `request_conflict` | This `request_id` was already used by this device with different content. |
| `receipt_capacity` | The terminal cannot record another receipt. Nothing was applied. |
| `internal` | The answer did not fit in a frame, or the host failed. |

A prompt whose first character is `/` is refused with `unsupported_operation`.
Prompt text is never dispatched as a slash command or sent to the model as
literal slash text. Use `execute_command` for supported commands and
`set_session_settings` for model/effort controls. Approval mode, the terminal
session on screen, and host-wide configuration remain local-only.

## Sessions

`sessions` (result and frame) lists the sessions that are shared right now:

```json
{
  "kind": "response", "protocol_version": 1, "request_id": "req-0001",
  "result": {
    "type": "sessions",
    "gateway_id": "…", "instance_id": "…",
    "subscribed": true,
    "sessions": [ { "session_id": "…", "registration_epoch": 3, "title": "…", "workspace": "…", "model": "…", "activity": "idle", "attention": {"approval": false, "question": false}, "health": "live" } ]
  }
}
```

A session leaves the list the moment its terminal stops sharing, switches
session or exits. `health` is `unresponsive` when the terminal has been silent
for 15 seconds; after 45 it is removed. A session that is shared again has a
new `registration_epoch`; nothing held for the old one applies to it.

## Attaching, ordering and resuming

`attach_session` subscribes the connection to one session's events. Its
response and the subscription are one atomic cut:

```json
{
  "kind": "response", "protocol_version": 1, "request_id": "req-0002",
  "result": { "type": "attached", "gateway_id": "…", "instance_id": "…", "snapshot": { "sequence": 412, "…": "…" } }
}
```

`snapshot.sequence` is the watermark: the snapshot contains every event up to
and including it. The events that follow start at `sequence + 1`; none of
them is already in the snapshot and none is missing.

Rules for `sequence`:

- Every event of a session has `sequence` equal to the previous one plus one,
  **except** `snapshot`, `resync_required` and `session_closed`. Those three
  are produced by the gateway and do not consume a number: they carry the
  watermark of the state they refer to.
- A `snapshot` event replaces everything the client holds for the session and
  sets its cursor to `snapshot.sequence`. It arrives when something changed
  that no event describes (a queued prompt, a background task result, a
  rewritten transcript), and after `resync_required`.
- `resync_required` means events were lost for this subscriber (`reason`:
  `lagged`, `sequence_gap`, `history_changed`). What the client holds can no
  longer be trusted; a `snapshot` event follows on the same subscription. A
  client may also simply attach again without a cursor.
- If a client ever sees a gap that these rules do not explain, it attaches
  again without a cursor.
- `session_closed` ends the subscription (`reason`: `sharing_disabled`,
  `session_changed`, `owner_exited`).

**Resuming.** A client that was attached keeps, per session: `gateway_id`,
`instance_id`, `registration_epoch` and the `sequence` of the last event it
applied (or of the last snapshot). After reconnecting and authenticating it
attaches with that cursor:

```json
{
  "protocol_version": 1, "request_id": "req-0003",
  "session_id": "…", "registration_epoch": 3,
  "operation": { "type": "attach_session", "resume": { "gateway_id": "…", "instance_id": "…", "last_sequence": 412, "snapshot_id": "…" } }
}
```

The gateway decides; the client does not have to compare anything itself.

- If every event after `last_sequence` is still held, the answer is

  ```json
  { "type": "resumed", "gateway_id": "…", "instance_id": "…", "next_sequence": 413 }
  ```

  and exactly those events follow. The client keeps its state.
- Otherwise the answer is `attached` with a fresh snapshot and a `resync`
  field saying why. **This is the resync signal on attach**: discard what is
  held for the session and start from the snapshot.

  ```json
  { "type": "attached", "gateway_id": "…", "instance_id": "…", "resync": "gateway_restarted", "snapshot": { "…": "…" } }
  ```

  | `resync` | Why the cursor was not replayed |
  | --- | --- |
  | `gateway_restarted` | The cursor's `gateway_id` or `instance_id` is not this gateway instance. |
  | `lagged` | The events after the cursor are no longer all held (4 MiB per session). |
  | `sequence_gap` | The state changed in a way events do not describe since the cursor, or the cursor is not one this registration issued. |

`resume.instance_id` is optional. Without it the gateway decides from the
sequence alone, which also refuses every cursor from before a restart (as
`sequence_gap` rather than `gateway_restarted`). A cursor for another
`registration_epoch` is `stale_session`, as for any request.

Store `snapshot.snapshot_id` alongside the applied sequence and send it as
`resume.snapshot_id`. This optional opaque identifier lets a cursor exactly
at the current snapshot watermark resume, including sequence zero with no
missed events. Snapshots can replace state without advancing the sequence:
a replaced snapshot has a new identifier, so its old cursor gets a fresh
`attached` response instead. Keep the identifier while applying later events;
replace it whenever a snapshot replaces the local state. Older clients without
the identifier still work, but get a fresh snapshot at this boundary.

New transcript messages use RFC3339 `timestamp` values with a timezone.
Legacy time-only values have no known date and are omitted from the remote
projection. An untitled session with no user prompt has an empty title;
the app can choose its own display placeholder.

An `attached` without a requested `resume` has no `resync` field.

Attaching again on a connection that is already attached to the session
replaces that subscription. `detach_session` drops it. Subscriptions end with
the connection.

**A slow client.** Events wait in the gateway while the client's queue is
full. If the client falls behind what the gateway holds it receives
`resync_required` (`lagged`) and a `snapshot` when it reads again. A client
that stops reading for good is closed with `slow_consumer`. In neither case
are events silently skipped.

## Receipts

`receipt` is present on the response to every mutation (`submit_prompt`,
`steer`, `queue`, `cancel_turn`, `answer_question`, `resolve_approval`):

| `receipt` | Meaning |
| --- | --- |
| `applied` | The terminal applied it. `result` is the outcome. |
| `rejected` | Nothing was applied. `result` is the error. |
| `unknown` | The terminal could not be reached or did not answer (`owner_unavailable`). It may or may not have been applied. |

The terminal records what it did per device and `request_id`, for as long as
the session stays shared under that registration:

- Sending the **same request again** (same `request_id`, same content)
  returns the original response and applies nothing. This is how to retry
  after a lost response.
- The same `request_id` with **different content** is `request_conflict`.
- A request refused before it reached the terminal (`not_found`,
  `stale_session`, `rate_limited`) is answered `rejected` and leaves no
  receipt: its ID can be sent again.

After `unknown`, or when a response never arrived, **do not resend on your
own**. Ask:

```json
{ "protocol_version": 1, "request_id": "req-0040", "session_id": "…", "registration_epoch": 3,
  "operation": { "type": "get_request_status", "target_request_id": "req-0031" } }
```

```json
{ "type": "request_status", "target_request_id": "req-0031", "receipt": "applied", "result": { "type": "prompt_accepted", "disposition": "started" } }
```

How it is resolved: by this device and `target_request_id`.

- With `session_id` (and optionally `registration_epoch`) on the envelope, the
  terminal that shares that session under that registration is asked. Send
  them when you have them; it is the precise form.
- Without them, every session currently shared is asked and the first that
  knows the ID answers. Request IDs should therefore be unique per device,
  not per session.

What comes back:

| `receipt` | `result` | Meaning |
| --- | --- | --- |
| `applied` / `rejected` | the original result | The terminal decided; this is the outcome the lost response carried. |
| `received` | absent | The terminal has it and has not decided yet. Ask again. |
| `unknown` | absent | No live terminal has a record: the ID was never seen (the request never arrived, or was refused before reaching the terminal), the terminal exited, or the session was shared again under a new epoch. |

`unknown` is final for that ID. Show the uncertainty, let the user look at the
transcript, and send a new request with a new ID only if they ask for it.

The guarantee is at-most-once per device and `request_id` while the terminal
lives. Receipts survive a gateway restart; they do not survive the terminal.

## Identity

For TUI label mappings, live/historical timing examples and app controls, see
[Building the app UI from the TUI's data](APP-UI.md).

A command that acts on something the user saw must name it, and the name is
checked by the terminal under the same lock that performs the change:

| Command | Names | Where the client finds it |
| --- | --- | --- |
| `cancel_turn` | `turn_id` | `snapshot.turn.turn_id`, `turn_started` |
| `answer_question` | `question_id` | `snapshot.pending_question`, `question_requested` |
| `resolve_approval` | `batch_id` | `snapshot.pending_approval`, `approval_requested` |

New turn IDs are durable UUID based identities for one logical turn, retained
through harness/background resumes. The next user prompt has a new one.
Question and approval IDs are issued once per process. Each question in a chain has
its own ID. The first valid answer wins: a second device, or the terminal,
answering the same question or batch afterwards receives `stale_question` or
`stale_approval`.

## Size

No frame in either direction exceeds 256 KiB. Text that was cut is a
`BoundedText` with `truncated`, `offset`, `total_bytes` and usually a
`content_id` to fetch the rest with `get_content`, in chunks of at most
64 KiB (`next_offset` is absent at the end). History is paged with
`get_history` from `snapshot.history_cursor`, newest page first, until
`next_cursor` is absent. Approval `details` that are truncated must be fetched
in full before the batch is resolved.

## Known gaps in v1

These are limits of the current shapes, listed so a client does not have to
discover them:

- `text_delta` has no durable target message ID. Its optional `timing.turn_id`
  correlates the logical turn; its text extends the active response segment.
  Empty text can update timing only. A failed logical turn also uses
  `turn_finished`, distinguished by `timing.outcome`.
- Items a client builds from events have no `message_id`; the IDs appear with
  the next snapshot or history page. Match by position after a turn ends, or
  re-read the tail.
- Changes to `pending_prompts`, `background_tasks` and the idle transcript are
  not events. The terminal publishes a `snapshot` event when they change
  (a prompt queued from a device shows up that way), so they are current, but
  as whole-state replacements.
- `can_steer` exists only in the snapshot.
- Question options are identified by their label, and nothing says whether a
  custom answer is allowed; the terminal accepts a non-empty `custom` answer
  for every question.
- Approval actions have no ID; a batch is approved or denied as a whole.
- Session-list rows have no creation or update timestamp. Use the supplied
  order or activity for the picker; known transcript and turn timestamps are
  for conversation labels, and do not establish when a session was created.
- Mutation receipts are bounded by count (1024 per registration), with no age
  eviction. At capacity, new mutations return `receipt_capacity`; known IDs
  remain queryable. Stop and re-share the session to create a new receipt store.
- Tool calls expose bounded summary/input/output text, without a separate
  structured command field. A tool-specific command copy button needs an
  additive structured payload; do not reconstruct commands from summaries.
- On Unix, a fallback `/tmp/rustcode-<uid>` socket directory can remain after
  gateway shutdown. Socket files are removed; the private directory is retained
  to avoid racing another process using it. Clients never need that path.
- `resync_required` with reason `gateway_restarted` is never sent as an
  event: a restart closes the connection, and the reason reaches the client
  in `attached.resync`.

## Using the contract from Swift

Write the `Codable` types against the schemas and copy `v1/golden/` into the
package's test resources. Decode every file in `golden/frames` as the host
frame type, every file in `golden/requests` as the request type and the files
under `golden/handshake` as the handshake types; re-encode and compare as
JSON: that is the same round trip this repository's tests run. Decode
optionals with `decodeIfPresent`, and give every enum an unknown case as
described under [Compatibility](#compatibility).

### Session model and reasoning controls

New hosts include optional `settings` in snapshots: `models` contains safe
configured profile IDs (`id`), provider model labels (`model`), and
`reasoning_efforts`; `selected_model` is the exact profile ID;
`reasoning_effort` is the current override or `default`; and `can_change`
reports whether the owner is idle. Profiles never include URLs or credentials.
Only provider-advertised or explicitly configured effort choices are offered.
A catalog that exceeds the control payload budget is omitted from snapshots;
its settings mutations return `frame_too_large` without changing state. Profile
IDs are never truncated and a partial catalog is never presented as complete.
`default` clears the effort override; it does not mean thinking is disabled.
Legacy profiles without an effort catalog offer only their configured value
and `default`. Refreshing the provider catalog obtains advertised choices.

Send `{"type":"set_session_settings","model":"profile-id","reasoning_effort":"high"}`
inside a normal session request envelope. Both fields are required and applied
atomically. The operation uses the existing authenticated registration, receipt,
idempotency, and request-status rules. Unknown profiles or unsupported efforts
return `invalid_request`; active, queued, or suspended turns return `busy`.
An applied receipt carries `{"type":"session_settings_updated","settings":{...}}`.
The owner publishes an authoritative snapshot after local or remote settings
changes. Settings affect this session's runtime configuration and are recorded
in its settings log; they do not rewrite host-wide user defaults. Clients
connected to older hosts must hide or disable these controls when `settings`
is absent.

Session rows may include `turn_count`, counted across the full host transcript
using logical turn IDs (including the active turn), with unattributed legacy
user prompts counted separately. Assistant/tool phases and client history
pagination do not increase it. The field is absent when a compaction boundary
means the archived prefix prevents an exact total; clients must not substitute
the count of loaded transcript rows.

Assistant history messages, live turns, and `text_delta` updates may include
`thought_tokens` and `thought_tokens_estimated`. These are the engine's existing
estimates from observed reasoning text, scoped to one assistant segment,
not provider billing totals or a cumulative session total. The estimate flag
is `true` for these values. Missing values mean unavailable. Empty text deltas
can update the observation without appending content, using the normal event
sequence and replay rules.

### Question cancellation and remote commands

Snapshots advertise optional `capabilities`: `cancel_question` and
`execute_command`. Missing capabilities mean that the owner or gateway needs an
upgrade; clients must not emulate these operations by sending provider prompts.

`{"type":"cancel_question","question_id":"…"}` dismisses the entire active
question chain with the same behavior as Escape in the terminal: it cancels the
turn token, resolves the question waiter as cancelled, clears all remaining
questions, and returns to idle. The question identity is checked under the same
lock as cancellation. A stale identity returns `stale_question` and changes
nothing. Success returns `{"type":"question_cancelled","question_id":"…"}`.

`{"type":"execute_command","command":"/status"}` runs an explicitly supported
host command and returns `{"type":"command_executed","command":"/status",
"title":"Status","output":"…"}`. Output is presentation-only Markdown, bounded
to 32 KiB plus a truncation marker, and is never added to provider history.
Both operations use the existing mutation receipts and request-status lookup.

Supported commands are `/help`, `/status`, `/info`, `/about`, `/usage`, `/perf`,
`/context`, `/tasks`, `/mcp`, `/model [exact profile name]`, `/effort [value]`, and
`/title <title>` (alias `/change_title`). Model and effort mutations use the same
idle-only validation and session persistence as `set_session_settings`.
`/context`, `/tasks`, and `/mcp` are read-only summaries. Unsupported commands or
arguments return `unsupported_operation`. Terminal lifecycle commands such as
`/new`, `/clear`, `/compact`, provider authentication, host configuration changes,
and interactive terminal pickers are not exposed. Use the identity-bound Stop
control for turn cancellation. Slash text in prompt/queue/steer stays rejected.

After upgrading the executable, restart the gateway as well as the session owner
before using newly added controls. A gateway launched before the upgrade can
discard new snapshot fields while decoding owner IPC, even when the terminal
owner is current. In a separate terminal run `rustcode remote stop`, then
`rustcode remote serve` with the same bind/advertise options previously used.
Shared session owners reconnect automatically; pairings remain saved. Reattach
in the app. Do not restart a running session merely to reconnect the gateway.

### Background gateway and mobile-created sessions

Once remote sharing is enabled, an installed macOS RustCode registers a private,
per-user LaunchAgent. The gateway survives terminal closure, starts at login, and
restarts after a crash or binary upgrade. `rustcode remote start` enables it,
`remote pair` starts it as needed, and `remote stop` explicitly disables the
LaunchAgent. Development binaries run detached without installing a login job.
The gateway advertises `_rustcode._tcp` on the local network; its Bonjour record
contains only the host name, port, and protocol version. Discovery does not pair
a device or grant access.

Authenticated clients can use `list_directories { path? }` to browse existing
folders (default `~/`) and `create_session { path? }` to create a session there.
The latter returns `session_created { session }`. It starts a detached session
owner with the same runtime, tools, questions, approvals, and turn cancellation
as the TUI. Closing the app or restarting the gateway does not stop that owner.
Folder listings omit dot-folders, cap their count and encoded size, and report
`truncated`; direct absolute and `~/` paths remain accepted.

Creation is a host-wide mutation: no session envelope is required. Its durable
receipt is scoped to the authenticated device and request ID. After a timeout,
reuse the same ID or query `get_request_status`; never generate a new ID merely
because confirmation has not arrived. Session owners hold a private per-session
lease, so concurrent launch/reconnect attempts cannot create duplicate owners.

The gateway preserves pairing credentials when upgrading. Existing terminal-owned
sessions keep running and reconnect, but retain their owner's older capabilities
until that terminal session is reopened with the updated binary. Newly created
mobile sessions immediately use the installed owner version. Automatic network
selection is retained in the login service instead of persisting a temporary
Wi-Fi/DHCP address; explicitly configured bind and advertised addresses remain
explicit.
