# Mobile (remote only)

Sharing a running terminal session with a paired device is implemented by the
remote gateway and `/remote`; the macOS app shares the threads it has open from
Settings › Setup › Remote access, without a terminal. **[remote.md](remote.md)
is the guide**: how to start the gateway, configure a NetBird or LAN address,
pair, check status, stop sharing and revoke a device. The wire contract an iOS
client is built against is [remote-protocol/README.md](remote-protocol/README.md),
and the design behind both is
[`Live terminal sessions from an iOS client`](architecture/mobile-remote-plan.md).

The phone app is an Expo (React Native) app in its own repository,
`rustcode-mobile`, built against the remote gateway and the wire contract in
this one; the gateway and the wire contract stay here and stay supported. The
older SwiftUI client that lived in `ios/` has been removed from this
repository.

Plain LAN traffic to the gateway is authenticated but not encrypted; NetBird is
the recommended way to reach it from a phone.

Decision: the phone never links Rust and never embeds the agent loop. Mobile
is a remote control for a PC or Mac that already runs RustCode. There is no
on-device core and no shared renderer strategy; the Expo app above is a thin
client of the remote gateway, which owns nothing but routing to the terminal
that shares a session.

The rest of this page describes the older experimental `rustcode serve`
transport, which is separate from the gateway and unchanged.

## Host features for the paired phone

The remote gateway exposes a small, allowlisted set of desktop features to a
paired device. Each one has a **host capability flag** in the `capabilities`
list of a `sessions` result, so the phone offers a control only when the Mac
it is talking to lists that flag. An older Mac lists none and answers the
operation with `unsupported_operation`.

| Feature | Operation | Capability flag | Success `result.type` |
| --- | --- | --- | --- |
| Usage summary, last 30 days | `get_usage` | `usage_summary` | `usage` |
| Read host defaults | `get_settings` | `settings_get` | `settings` |
| Write host defaults | `set_settings` | `settings_set` | `settings` |
| Pin a thread | `pin_thread` | `thread_pin` | `thread_pinned` |
| Unpin a thread | `unpin_thread` | `thread_unpin` | `thread_pinned` |
| Settle a thread | `settle_thread` | `thread_settle` | `thread_settled` |
| Unsettle a thread | `unsettle_thread` | `thread_unsettle` | `thread_settled` |

### Usage

`get_usage` reads the store behind `rustcode usage` for the last 30 days,
today included. Token fields are what each harness reported; `cost_usd` is
only the USD a harness reported. A value no turn reported is absent, not zero.

```json
{"protocol_version": 1, "request_id": "req-usage", "operation": {"type": "get_usage"}}
```

```json
{
  "kind": "response", "protocol_version": 1, "request_id": "req-usage",
  "result": {
    "type": "usage", "since": "2026-06-15", "until": "2026-07-14",
    "totals": {"input_tokens": 120000, "output_tokens": 9000, "cached_tokens": 40000, "cache_write_tokens": 8000, "cost_usd": 1.42, "turns": 12},
    "by_day": [{"day": "2026-07-14", "usage": {"input_tokens": 20000, "output_tokens": 1500, "cached_tokens": 6000, "cache_write_tokens": 1000, "cost_usd": 0.24, "turns": 2}}],
    "by_harness": [{"harness": "codex", "usage": {"input_tokens": 90000, "output_tokens": 7000, "cached_tokens": 30000, "cache_write_tokens": 6000, "cost_usd": 1.1, "turns": 8}}],
    "by_model": [{"harness": "codex", "model": "gpt-5.5", "usage": {"input_tokens": 90000, "output_tokens": 7000, "cached_tokens": 30000, "cache_write_tokens": 6000, "cost_usd": 1.1, "turns": 8}}]
  }
}
```

### Host settings

Only the default harness, model and effort are visible or writable. A key
outside that allowlist is rejected with `invalid_request`; nothing is written.
An absent field keeps its current value. Both operations answer with the
complete `settings` result after the read or write.

```json
{"protocol_version": 1, "request_id": "req-settings", "operation": {"type": "get_settings"}}
{"protocol_version": 1, "request_id": "req-set-settings", "operation": {"type": "set_settings", "settings": {"harness": "codex", "model": "gpt-5.5", "effort": "high"}}}
```

```json
{"kind": "response", "protocol_version": 1, "request_id": "req-settings", "result": {"type": "settings", "harness": "codex", "model": "gpt-5.5", "effort": "high"}}
{"kind": "response", "protocol_version": 1, "request_id": "req-set-settings", "receipt": "applied", "result": {"type": "settings", "harness": "codex", "model": "gpt-5.5", "effort": "high"}}
```

### Pin and settle

These use the engine's own thread paths, so the state shown on the phone is
the same as in the desktop app. They work for a saved thread as well as a
shared one; the thread is named in the operation, so no shared-session
envelope is required. Settling stores `done`; unsettling returns the thread
to `idle`.

```json
{"protocol_version": 1, "request_id": "req-pin", "operation": {"type": "pin_thread", "session_id": "20260714-091502-3fa2c1"}}
{"protocol_version": 1, "request_id": "req-unpin", "operation": {"type": "unpin_thread", "session_id": "20260714-091502-3fa2c1"}}
{"protocol_version": 1, "request_id": "req-settle", "operation": {"type": "settle_thread", "session_id": "20260714-091502-3fa2c1"}}
{"protocol_version": 1, "request_id": "req-unsettle", "operation": {"type": "unsettle_thread", "session_id": "20260714-091502-3fa2c1"}}
```

```json
{"kind": "response", "protocol_version": 1, "request_id": "req-pin", "receipt": "applied", "result": {"type": "thread_pinned", "session_id": "20260714-091502-3fa2c1", "pinned": true}}
{"kind": "response", "protocol_version": 1, "request_id": "req-settle", "receipt": "applied", "result": {"type": "thread_settled", "session_id": "20260714-091502-3fa2c1", "settled": true}}
```

Snooze is deliberately not offered: it lives in the desktop app's own local
storage, not in the engine or the host's session store, so the phone cannot
set it and no capability advertises it.

The committed schema and golden frames under
[`remote-protocol/v1/`](remote-protocol/README.md) are the source of truth for
every shape; the shapes above are the ones the phone uses.

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
