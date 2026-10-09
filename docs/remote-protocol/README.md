# Remote protocol v1

The wire contract between a remote client (the iOS app) and a host running
RustCode. It is the first step of
[`Live terminal sessions from an iOS client`](../architecture/mobile-remote-plan.md);
the transport that carries these frames (gateway, WebSocket, pairing, `/remote`)
does not exist yet. The experimental `rustcode serve` protocol described in
[`mobile.md`](../mobile.md) is separate and unchanged.

The Rust types in `rustcode/engine/src/remote/protocol.rs` are the source of
truth. Everything under `v1/` is generated from them:

| Path | Contents |
| --- | --- |
| `v1/request.schema.json` | JSON Schema (2020-12) for a client → host frame, `RemoteRequest`. |
| `v1/frame.schema.json` | JSON Schema for a host → client frame, `RemoteFrame`. |
| `v1/golden/requests/*.json` | One example of every request shape. |
| `v1/golden/frames/*.json` | One example of every response, event and error shape. |

`cargo test` regenerates all of it and fails when a committed file differs, when
a golden frame does not round-trip or does not satisfy the schema, or when a
shape in the schema has no golden frame. After an intended change:

```sh
RUSTCODE_UPDATE_REMOTE_PROTOCOL=1 cargo test --lib remote::contract
```

and commit the result with the code change.

## Frames

One JSON object per frame. Enums are flat objects tagged by `type` (the host
frame itself by `kind`). The host omits absent optional fields; it never sends
`null`, although the schema tolerates it.

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

Host → client, a sequenced event of one session:

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
after `subscribe_sessions`.

`session_id` and `registration_epoch` are required on every operation except
`list_sessions`, `subscribe_sessions` and `get_request_status`. Device identity
comes from authentication and is never part of a frame.

## Operations

| `operation.type` | Success `result.type` | Typed rejections |
| --- | --- | --- |
| `list_sessions`, `subscribe_sessions` | `sessions` | |
| `attach_session` | `attached` (snapshot) or `resumed` (replay follows) | `stale_session`, `not_found` |
| `detach_session` | `detached` | |
| `get_history` | `history` | `stale_cursor` |
| `get_content` | `content` | `not_found`, `invalid_request` |
| `submit_prompt` | `prompt_accepted` | `busy`, `unsupported_operation` |
| `steer` | `prompt_accepted` | `not_running`, `unsupported_operation` |
| `queue` | `prompt_accepted` | `not_running`, `unsupported_operation` |
| `cancel_turn` | `turn_cancelled` | `stale_turn` |
| `answer_question` | `question_answered` | `stale_question`, `invalid_answer` |
| `resolve_approval` | `approval_resolved` | `stale_approval` |
| `get_request_status` | `request_status` | |

Any request can also be rejected with `incompatible_version` (carrying
`supported_versions`), `invalid_request`, `unsupported_operation` (an operation
this version does not know), `stale_session`, `unauthorized`, `rate_limited`,
`frame_too_large` or `internal`. The version is checked before the operation is
parsed, so a newer client learns that the version is the problem.

A prompt whose first character is `/` is refused with `unsupported_operation`.
Remote text is never dispatched as a slash command, and v1 does not send it to
the model as literal text either.

## Identity

A command that acts on something the user saw must name it, and the name is
checked by the session owner under the same lock that performs the change:

| Command | Names | Where the client finds it |
| --- | --- | --- |
| `cancel_turn` | `turn_id` | `snapshot.turn.turn_id`, `turn_started` |
| `answer_question` | `question_id` | `snapshot.pending_question`, `question_requested` |
| `resolve_approval` | `batch_id` | `snapshot.pending_approval`, `approval_requested` |

Every ID is issued once per process and never reused. A turn ID names one
prompt run; the next queued prompt has a new one. Each question in a chain has
its own ID. The first valid answer wins: a second device, or the terminal,
answering the same question or batch afterwards receives `stale_question` or
`stale_approval`.

## Receipts, ordering and size

These shapes are defined here; the machinery behind them arrives with the
gateway.

- `receipt` on a response to a mutation is `applied` or `rejected` once the
  owner has decided, or `unknown` (with `owner_unavailable`) when the owner
  could not be reached. `get_request_status` additionally reports `received`.
  A client must never resend a mutation on its own after `unknown`.
- `sequence` increases by exactly one per event within a registration. A
  snapshot's `sequence` is the last event it already contains. On a gap, or on
  a `resync_required` event, attach again.
- No frame exceeds 256 KiB. Text that was cut is a `BoundedText` with
  `truncated`, `offset`, `total_bytes` and usually a `content_id` to fetch the
  rest with `get_content`. Approval `details` that are truncated must be
  fetched in full before the batch is resolved.

## Using the contract from Swift

Write the `Codable` types against the two schemas and copy `v1/golden/` into the
package's test resources. Decode every file in `golden/frames` as the host
frame type and every file in `golden/requests` as the request type, re-encode,
and compare as JSON: that is the same round trip this repository's tests run.
Decode optionals with `decodeIfPresent`, and treat an unknown `type`, `kind` or
error `code` as a decoding failure for that frame rather than guessing.
