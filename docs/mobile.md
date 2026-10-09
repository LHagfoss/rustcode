# Mobile (remote only)

Sharing a running terminal session with a paired device is implemented by the
remote gateway and `/remote`; **[remote.md](remote.md) is the guide**: how to
start the gateway, configure a NetBird or LAN address, pair, check status, stop
sharing and revoke a device. The wire contract the iOS app is built against is
[remote-protocol/README.md](remote-protocol/README.md), and the design behind
both is
[`Live terminal sessions from an iOS client`](architecture/mobile-remote-plan.md).
The app itself lives in its own repository.

Plain LAN traffic to the gateway is authenticated but not encrypted; NetBird is
the recommended way to reach it from a phone.

The rest of this page describes the older experimental `rustcode serve`
transport, which is separate from the gateway and unchanged.

Decision: the phone never links Rust and never embeds the agent loop. Mobile
is a remote control for a PC or Mac that already runs RustCode. There is no
on-device core, no shared renderer strategy, and no Expo app in the release
matrix. Expo is explicitly out of scope until the transport below has soaked.

## Transport (`rustcode serve`, experimental MVP)

`rustcode serve --port 17878` (see README) drives one controller session
worker — the same seam the desktop uses. Loopback bind by default; `--bind`
a LAN address only with `--allow-remote`. No TLS, no multi-session routing:
loopback or trusted LAN only.

Frames are newline-delimited JSON, at most `MAX_FRAME_BYTES` (1 MiB), in the
style of `engine/src/daemon/protocol.rs`. The first frame on every
connection must authenticate; anything else closes the connection.

### Client → server (`ServeRequest`, `#[serde(tag = "type")]`)

```json
{"type": "auth", "token": "…"}
{"type": "list_sessions"}
{"type": "submit", "prompt": "…"}
{"type": "answer_question", "answer": "…"}
{"type": "approve", "batch_id": "controller:1:2", "choice": "approve"}
{"type": "cancel"}
```

`choice` is `approve` or `deny`. `batch_id` comes from the
`approval_batch_requested` update below; unknown ids are rejected with an
error frame (idempotent stop semantics do not apply to approvals).

### Server → client (`ServeResponse`, `#[serde(tag = "type")]`)

```json
{"type": "ready", "version": 1}
{"type": "event", "generation": 7, "update": {"type": "snapshot", …}}
{"type": "error", "code": "session", "message": "…"}
```

After `ready`, the server replays the latest snapshot, then streams worker
events. The live subscription starts before snapshot replay. A replay can
therefore be followed by a duplicate snapshot or update; clients should treat
snapshots as authoritative and ignore events from older `generation` values.
A client that falls more than 64 queued updates behind is disconnected and
must reconnect for a fresh snapshot. There is no per-update sequence number
or exact resume position in protocol v1. `update` is a flat tagged map —
never a nested envelope:

- `{"type": "snapshot", …}` — full `ControllerSnapshot` (transcript,
  pending prompts/approvals/questions, models, sessions, background tasks
  with `elapsed_secs`, turn flags).
- `{"type": "prompt_restored", …}` — restored pending prompt echo.
- Turn updates carry their own tag inline: `prompt_started`, `text_delta`,
  `tool_started`, `tool_finished`, `approval_requested`,
  `approval_batch_requested`, `question_requested`, `turn_finished`,
  `cancelled`, each with its fields beside the tag.

Worker errors are returned as top-level `{"type": "error", "code", "message"}`
frames, never as an `update`, so a client needs one error path. Codes are
stable snake_case: `no_active_session`, `invalid_workspace`, `session`,
`model`, `provider`, `channel_closed`, plus `unauthorized` for a failed
handshake. Field order within a frame is not significant.

MVP limits: every snapshot contains the full transcript. Once its serialized
frame exceeds 1 MiB, that client connection closes; transcript paging or
bounded snapshots are follow-up work. Authentication compares all token bytes
without an early exit, but there is no connection cap or auth rate limit.
Use loopback or a trusted LAN only; the transport has no TLS.

A typical round-trip: `auth` → `ready` + snapshot → `list_sessions` →
snapshot → `submit` → snapshot/turn stream → (`approve` with the batch id
from `approval_batch_requested`, or `cancel`) → snapshots confirm.

Any thin client can build against this without touching the Rust workspace.
The iOS app does not use this transport; it uses the remote gateway.
