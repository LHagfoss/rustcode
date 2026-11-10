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
| `v1/golden/handshake/responses/*.json` | `connect_pending`, `connect_approved`, `connect_denied`, `connect_expired`, `paired`, `authenticated`, and one bare `error` per code. |
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

## Thread names

RustCode calls a conversation a **thread** ([terminology](../terminology.md)).
Version 1 was written when it was called a session, so its names say
`session`. A client can ask to use thread names instead: it sends
`"wire_names": "thread"` in its `pair` or `authenticate` frame, and a gateway
that supports it answers with the same field in `paired` or `authenticated`.
From then on every frame on that connection uses the names below, in both
directions. Without the field (an older client, or an older gateway that
ignores it) the connection keeps the v1 names, so either side can be
upgraded first.

Only names change; frames, fields, order and meaning are v1's, and the schema
and golden frames under `v1/` stay in v1 names. Text inside a frame (titles,
prompts, messages) is never rewritten.

| v1 name | Thread name | Where |
| --- | --- | --- |
| `session_id` | `thread_id` | field |
| `session` | `thread` | field |
| `sessions` | `threads` | field, and `kind` of the list push frame |
| `subscribe_sessions`, `list_sessions`, `list_saved_sessions` | `subscribe_threads`, `list_threads`, `list_saved_threads` | operation `type` |
| `create_session`, `attach_session`, `detach_session`, `resume_session`, `set_session_settings` | `create_thread`, `attach_thread`, `detach_thread`, `resume_thread`, `set_thread_settings` | operation `type` |
| `sessions`, `saved_sessions`, `session_created`, `session_resumed`, `session_settings_updated` | `threads`, `saved_threads`, `thread_created`, `thread_resumed`, `thread_settings_updated` | result `type` |
| `session_closed` | `thread_closed` | event `type` |
| `session_changed` | `thread_changed` | close `reason` |
| `stale_session` | `stale_thread` | error `code` |

The gateway translates at the device's socket
(`rustcode/engine/src/remote_gateway/wire_names.rs`); a test fails when the
schema gains a `session` name the table does not cover. The phone app asks for
thread names.

## Connection

1. Open a WebSocket to `ws://<address>/` with no `Origin` header, or with
   exactly `Origin: rustcode-mobile://app`. Any other `Origin` is refused
   with HTTP 403, so a web page open on the host or the LAN cannot reach the
   gateway. A React Native client cannot omit the header (both platforms add
   one on the upgrade), so it must send the app origin explicitly through the
   WebSocket `headers` option:
   `new WebSocket(url, undefined, { headers: { Origin: 'rustcode-mobile://app' } })`.
   Android adds a default `Origin` from the URL unless the caller passes one
   (React Native `WebSocketModule.kt`, `connect`), and iOS lets a
   `request.allHTTPHeaderFields` value replace SocketRocket's own origin
   (`SRHTTPConnectMessage.m`, reached from `RCTWebSocketModule.mm`). Browsers
   can never send a custom-scheme origin — a page cannot set headers on a
   WebSocket it opens — so accepting exactly this value does not reopen the
   gateway to browsers.
2. Send exactly one handshake frame, `pair` or `authenticate`, within 10
   seconds. Nothing is sent to an unauthenticated socket except the answer to
   that frame. The one exception is `connect_request`, which keeps the socket
   open for the host owner's answer ([Connection requests](#connection-requests)).
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
{"type": "pair", "protocol_version": 1, "method": "credential", "secret": "<credential from the QR code>", "device_name": "Lars's iPhone", "install_id": "6F9619FF-8B86-D011-B42D-00C04FC964FF", "capabilities": ["token_rotation"]}
{"type": "pair", "protocol_version": 1, "method": "code", "secret": "4821-9034", "device_name": "Lars's iPhone"}
{"type": "authenticate", "protocol_version": 1, "device_id": "0011223344556677", "refresh_token": "<refresh token>", "capabilities": ["token_rotation"]}
{"type": "authenticate", "protocol_version": 1, "device_id": "0011223344556677", "access_token": "<access token>"}
{"type": "authenticate", "protocol_version": 1, "device_id": "0011223344556677", "token": "<static device token>"}
```

Host → client:

```json
{"type": "paired", "protocol_version": 1, "gateway_id": "…", "instance_id": "…", "device_id": "0011223344556677", "device_name": "Lars's iPhone", "token": "<device token>", "host_name": "studio", "expires_at": "2026-08-22T09:15:00Z"}
{"type": "paired", "protocol_version": 1, "gateway_id": "…", "instance_id": "…", "device_id": "0011223344556677", "device_name": "Lars's Windows PC", "refresh_token": "<R1>", "access_token": "<A1>", "access_expires_at": "2026-08-22T09:15:00Z", "host_name": "studio"}
{"type": "authenticated", "protocol_version": 1, "gateway_id": "…", "instance_id": "…", "device_id": "0011223344556677", "device_name": "Lars's iPhone", "host_name": "studio", "expires_at": "2026-08-22T09:15:00Z"}
{"type": "authenticated", "protocol_version": 1, "gateway_id": "…", "instance_id": "…", "device_id": "0011223344556677", "device_name": "Lars's Windows PC", "refresh_token": "<R2>", "access_token": "<A2>", "access_expires_at": "2026-08-22T09:15:00Z", "rotation_id": "5be1c4a07d2e9f63"}
{"type": "error", "code": "pairing_failed", "message": "…"}
{"type": "error", "code": "rate_limited", "message": "…", "retry_after_secs": 240}
```

- `token` appears once, in `paired`, for a device that does not rotate its
  tokens. Store it in the Keychain; the host keeps only its digest. After
  `paired` the same connection is already authenticated.
- `capabilities` in `pair` and `authenticate` lists what the device
  understands; a gateway ignores a capability it does not know. A device that
  names `token_rotation` gets the rotating tokens below instead of a static
  `token`, and must name it on `pair` too. See
  [Rotating tokens](#rotating-tokens-no-expiry).
- `gateway_id` is stable for the host and is what a client keys its stored
  token and host data by. `instance_id` changes every time the gateway
  starts.
- `expires_at` is optional: when the pairing stops being accepted, RFC 3339 in
  UTC, 30 days after pairing. From then on `authenticate` answers
  `unauthorized` and the device pairs again. A gateway that predates it sends
  none, and its pairings do not expire. A device that rotates its tokens never
  gets one: its pairing lasts until it is revoked, so `expires_at` is absent
  and keeping the pairing alive never means pairing again.
- `host_name` is optional: the machine's own name, for display. It is a
  label, not an identity.
- `routes` is optional on both answers: the addresses this host answers on
  right now, best first, each `{ "address": "host:port", "kind": "lan" }`.
  The kinds are `lan` (a private IPv4 address on a local interface), `vpn` (a
  `100.64.0.0/10` NetBird or Tailscale address), `hostname` (a name, such as
  the machine's `.local` name) and `public`. A client keeps them as the routes
  it knows for this host and dials the first that answers, so pairing once is
  enough to reconnect on another network. Absent from an older gateway, and
  when the listener is loopback only.
- `device_name` in the answer is the name as the host stored it (trimmed and
  bounded), which may differ from what was sent.
- `install_id` in `pair` is optional: a random identifier the app generates
  once per install and keeps across pairings (16 to 128 printable ASCII
  characters, for example a UUID; anything else is ignored). It is not tied to
  the user. When a device record was paired with the same `install_id`, the
  host replaces it: the earlier `device_id` and its token stop working, any
  connection still open with them is closed with `revoked`, and the answer
  carries a new `device_id`. Without it, every pairing adds a record. The host
  stores only its SHA-256 digest.
- `device_kind`, `platform` and `host_id` in `pair` are optional and say what
  the device is, for the owner's device list. `device_kind` is `phone`,
  `desktop` (the desktop app on another machine) or `cli`; absent, or a kind
  the gateway does not know, is `phone`. `platform` is `ios`, `android`,
  `macos`, `windows`, `linux`, … (as in `connect_request`); absent is
  `unknown`. `host_id` is the stable identifier of the machine a desktop app
  or command line runs on (16 to 128 printable ASCII characters; anything else
  is ignored). They are labels, not permissions: every kind gets the same
  access. A gateway that records them lists `desktops` in its Bonjour `caps`
  and in the `sessions` result's `capabilities`; an older one ignores them.

### Rotating tokens, no expiry

A device that names `"capabilities": ["token_rotation"]` in `pair` gets no
static token and no `expires_at`: its pairing lasts until it is revoked. The
host hands it a **refresh token**, which every reconnect replaces, and a
short-lived **access token**, which reconnects inside its 15 minutes without
replacing anything. A client that does not name the capability keeps the
static `token` and its 30 days exactly as before. A gateway that takes the
capability lists `token_rotation` in the `sessions` result's `capabilities`.

```text
pair            → paired {refresh_token R1, access_token A1, access_expires_at}
connect         → authenticate {device_id, refresh_token: R1}
                ← authenticated {refresh_token R2, access_token A2, access_expires_at, rotation_id}
                → token_ack {rotation_id}                      (R2 stored durably)
reconnect <15m  → authenticate {device_id, access_token: A2}
                ← authenticated {access_expires_at}             (nothing rotated)
```

- `refresh_token` and `access_token` appear exactly once, in the `paired` or
  `authenticated` frame that issued them. Store the refresh token durably
  (Keychain) before anything else; the host keeps only digests.
- `access_expires_at` is RFC 3339 in UTC, 15 minutes after the access token was
  minted. Until then `authenticate` with `access_token` reconnects without
  rotating. Any other access token of a rotating device is answered
  `access_expired` — the one a rotation replaced, one a restart forgot, or a
  wrong one — because the only right answer is to authenticate with the refresh
  token, which never expires. `access_token` itself is never sent again, so a
  client keeps the one it has.
- Every accepted refresh token rotates: the answer carries a new
  `refresh_token` and the `rotation_id` of that rotation. Replace the stored
  one and send `{"type": "token_ack", "rotation_id": "5be1c4a07d2e9f63"}` on
  the same connection. It is a session-path frame the gateway itself consumes;
  it is never routed to a session, and one that names an older rotation is
  ignored.
- **Retry in the two-phase window**: until the acknowledgement arrives, the
  token before the new one is still accepted, and presenting it again is an
  idempotent retry: the host answers with the *same* `refresh_token`,
  `access_token` and `rotation_id` it issued before, and rotates nothing. A
  lost `authenticated` frame therefore cannot lock a device out, and a second
  holder of the previous token gains nothing beyond the answer the device
  already received. The answer is kept in memory only; after a gateway restart
  the host has nothing to re-issue, and a retry with the previous token is a
  replay.
- **Replay**: presenting a refresh token the host has retired for good revokes
  the whole pairing. The host closes the device's connections with `revoked`,
  answers this attempt with `token_replayed`, and removes the record as if
  revoked. The revocation is written before anything is closed; when the device
  store cannot be written, it is noted in `remote/revocations.json` for the
  next start and retried, and the device is refused in memory either way. The
  owner sees `remote.token_replayed` in the gateway log. Pair again. A token
  this host never issued is only `unauthorized`: a guessed `device_id` cannot
  revoke a device. Notes left by a failed write are read and applied before the
  gateway serves anyone; if that file cannot be read the gateway refuses to
  start and names it, and never overwrites it, so a revoked family cannot come
  back. The owner repairs it, or deletes it after revoking the device with
  `rustcode remote revoke`.
- Two connections racing with the same refresh token (a client trying several
  routes at once) are not a replay: the first rotates, the second gets the same
  answer, and both are accepted. Keep the answer from the route that stays up;
  it is the same on every route.
- The host stores digests only: the current refresh token, the previous one
  until it is acknowledged, and a 64-bit fingerprint of every refresh token it
  retired, so any token it issued is recognized as a replay rather than answered
  as a wrong one. A family that retires more than 1024 tokens is revoked instead
  of forgetting one: its connections close with `revoked`, the device is refused
  (`unauthorized` with the pairing gone) and the owner sees
  `remote.pairing_retired`. The access token's digest stays in memory and never
  reaches disk, so after a gateway restart an access token answers
  `access_expired` and the refresh token reconnects as usual: the pairing
  survives.
- `rotation_id` is absent when nothing rotated: an access token reconnected, or
  the device does not rotate. `refresh_token` and `access_token` are absent on
  an access-token reconnect.

The pairing QR code encodes this JSON, as one line:

```json
{"protocol_version": 1, "address": "100.92.13.44:17879", "gateway_id": "…", "credential": "…", "host_name": "studio", "alternates": ["192.168.1.20:17879"]}
```

`address` is `host:port` to dial. `host_name` is optional and cut to 24
characters. Send `credential` as the `secret` of a `pair` frame with
`"method": "credential"`.

`alternates` is optional: other `host:port` addresses of the same listener,
best first, at most four (IPv6 in brackets, `[2001:db8::1]:17879`). Dial
`address` first; when it cannot be reached, try each alternate in order and
pair with the same `credential` on the first that connects. Every address
reaches the same gateway, so `gateway_id` is the same whichever answered.
It is absent when the gateway listens on one specific address, and a client
that does not know it ignores it.

### The bare `error` frame

`{"type": "error", "code": …, "message": …}` has no `kind` and no
`request_id`. It is a **connection-level** frame: the gateway sends it once
and then closes the connection. It is never the answer to a request on an
authenticated connection; those are always `response` frames (below).
`retry_after_secs` (with `rate_limited`) and `supported_versions` (with
`unsupported_version`, an array of the handshake versions the host speaks) are
optional and absent otherwise.

| When | `code` | What the client does |
| --- | --- | --- |
| Handshake | `invalid_frame` | First frame was not a handshake frame, not JSON, or not text. A bug. |
| Handshake | `frame_too_large` | Handshake frame over 8 KiB. |
| Handshake | `unsupported_version` | `protocol_version` is not 1. Update the app on the device, or the host, so both speak the same version. `supported_versions` lists the versions the host speaks, and `message` says the same in words that can be shown as they are. |
| Handshake | `handshake_timeout` | No frame within 10 seconds. |
| Handshake (`pair`) | `pairing_failed` | Wrong, expired, used or exhausted secret; indistinguishable on purpose. Ask for a new code. |
| Handshake (`pair`) | `rate_limited` | Too many failed pairings on the host; `retry_after_secs` says when to try again. |
| Handshake (`authenticate`) | `unauthorized` | Unknown or revoked device, or wrong token. Stop reconnecting; pair again. |
| Handshake (`authenticate`) | `access_expired` | The access token has expired or was replaced. The pairing is still valid: authenticate with the refresh token. |
| Handshake (`authenticate`) | `token_replayed` | A retired refresh token was presented; the pairing was revoked. Stop, drop the tokens, pair again. |
| After authentication | `invalid_frame` | A frame tagged `token_ack` without a `rotation_id`. A bug. |
| Handshake | `busy` | Connection or device limit reached. Retry later. |
| Handshake (`connect_request`) | `connect_busy` | The host holds four requests already, or this address has one open. Retry later. |
| Handshake (`connect_request`) | `connect_rate_limited` | This address was denied less than 30 seconds ago; `retry_after_secs` says when to ask again. |
| Connection request | `invalid_frame` | A text frame before `connect_approved`, or anything but a `pair` frame after it. A bug. |
| Handshake (`pair`) | `internal` | The host could not store the device. |
| After authentication | `revoked` | The device was revoked. Stop reconnecting, delete the token, pair again. |
| After authentication | `slow_consumer` | The client did not read fast enough. Reconnect and resume. |
| After authentication | `idle_timeout` | Nothing was heard for 90 seconds. Reconnect and resume. |
| After authentication | `frame_too_large` | A frame over 1 MiB was sent. A bug. |
| After authentication | `invalid_frame` | A binary frame was sent. A bug. |
| After authentication | `shutting_down` | The gateway is stopping. Reconnect with backoff. |

`not_implemented` is reserved for a gateway without session routing and is not
sent by this one.

### Connection requests

A phone that found the host on the local network can ask its owner to let it
pair, instead of scanning a QR code. The gateway takes it when its Bonjour
record (`_rustcode._tcp`) carries `caps=connect_requests` in its TXT record
(`caps=connect_requests,desktops` when a desktop app may pair too)
(next to `protocol=1` and `gateway=<gateway_id>`); a paired device also sees
`connect_requests` in the `sessions` result's `capabilities`. A gateway
without it answers `connect_request` with `invalid_frame`.

The exchange, all on one WebSocket:

1. Client → host, the first frame, within 10 seconds:

   ```json
   {"type": "connect_request", "protocol_version": 1, "device_name": "Lars's iPhone", "platform": "ios", "install_id": "6F9619FF-8B86-D011-B42D-00C04FC964FF"}
   ```

   No secret. `device_name` is a label shown to the owner (trimmed and
   bounded like in `pair`); `platform` is `ios`, `android`, `macos`,
   `windows`, `linux`, … (lowercase letters, digits, `-_.`, up to 32);
   `install_id`, `device_kind` and `host_id` are optional, as in `pair`. A
   desktop app sends `"device_kind": "desktop"`, and the owner sees *Windows
   desktop* rather than a phone. The device is recorded with the kind,
   platform and host of its request, which is what the owner accepted,
   whatever its `pair` frame says.

2. Host → client: `{"type": "connect_pending", "request_id": "5be1c4a07d2e9f63", "expires_in_secs": 120}`.
   The request is in front of the owner. Wait on the same socket, sending
   nothing but WebSocket pings or pongs (the gateway pings every 30 s); any
   text frame now is `invalid_frame` and closes the connection. The 10 s
   handshake deadline no longer applies; the request's own does.

3. Host → client, one of:
   - `{"type": "connect_denied"}`: the owner said no. The connection closes,
     and this address cannot ask again for 30 seconds
     (`connect_rate_limited`).
   - `{"type": "connect_expired"}`: nobody answered in `expires_in_secs`. The
     connection closes; ask again.
   - `{"type": "connect_approved", "request_id": "5be1c4a07d2e9f63", "code_length": 8, "expires_in_secs": 120}`:
     the owner accepted and the host shows an eight-digit code, formatted
     `4821-9034`.

4. After `connect_approved`, client → host, exactly one frame within
   `expires_in_secs`: the ordinary code `pair` frame with what the user typed
   (with or without the `-`):

   ```json
   {"type": "pair", "protocol_version": 1, "method": "code", "secret": "4821-9034", "device_name": "Lars's iPhone", "install_id": "6F9619FF-8B86-D011-B42D-00C04FC964FF"}
   ```

   The answer is the usual `paired` frame, and the connection is
   authenticated from there, or an `error` (`pairing_failed`,
   `rate_limited`, `busy`, `internal`) and a close. A wrong code ends the
   request: one code per accepted request; ask again. Anything but a `pair`
   frame is `invalid_frame`.

   Until that frame is sent the host may still send
   `{"type": "connect_expired"}` and close: the code ran out, or a newer
   challenge on the host replaced it (the owner pressed *Pair a device* or
   accepted another request).

The code is bound to that connection: the same code sent by any other
socket is `pairing_failed`, and the request's socket cannot redeem a QR
code's challenge. Wrong codes count toward the host-wide limit shared with
every other pairing (10 failures in 5 minutes lock pairing). The host holds
at most four requests and one per network address (`connect_busy`); a new
request with the same `install_id` from the same address replaces the open
one, whose socket gets `connect_expired`, so a reconnecting app need not wait.
Closing the socket withdraws the request and destroys its code.

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
the host-wide ones: `list_sessions`, `subscribe_sessions`, `get_request_status`,
`list_directories`, `create_session`, `request_share`, `list_saved_sessions`,
`resume_session`, `get_usage`, `get_settings`, `set_settings`, `pin_thread`,
`unpin_thread`, `settle_thread` and `unsettle_thread`. Device identity
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
| `list_saved_sessions` | gateway (`saved_sessions`) | `saved_sessions` | `invalid_request` |
| `resume_session` | gateway (`saved_sessions`) | `session_resumed` | `not_found`, `busy`, `open_elsewhere`, `invalid_request`, `internal` |
| `get_changes` | terminal (`session_changes`) | `changes` | `not_found` (no workspace), `stale_session` |
| `get_usage` | gateway (`usage_summary`) | `usage` | `internal` |
| `get_settings` | gateway (`settings_get`) | `settings` | `invalid_request`, `internal` |
| `set_settings` | gateway (`settings_set`) | `settings` | `invalid_request`, `internal` |
| `pin_thread`, `unpin_thread` | gateway (`thread_pin`, `thread_unpin`) | `thread_pinned` | `invalid_request` |
| `settle_thread`, `unsettle_thread` | gateway (`thread_settle`, `thread_unsettle`) | `thread_settled` | `invalid_request` |

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
| `open_elsewhere` | `resume_session` only: another process on the host (the app, a terminal) has the saved session open without sharing it. `message` says where and is ready to show. Nothing was started. Clients that do not know the code show `message`. |
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
    "sessions": [ { "session_id": "…", "registration_epoch": 3, "title": "…", "workspace": "…", "model": "…", "harness": "claude-code", "activity": "idle", "attention": {"approval": false, "question": false}, "health": "live" } ]
  }
}
```

A session's `harness` names the agent that runs it: `rustcode` for the
built-in one, otherwise the harness id (`claude-code`, `codex`, `pi`, …). It
is optional and absent from a host that predates harness threads; a client
then shows no harness. It rides on every session entry — the `sessions`
result, the snapshot's `session`, and `session_created`/`session_resumed` —
and `model` is the model that harness runs the thread on (empty when the host
does not know it). Both are additive: a client that predates them decodes the
same frames unchanged.

The `sessions` result also carries `capabilities`, the optional host-wide
operations the gateway implements. It is absent from gateways that implement
none; the flags are `saved_sessions`, `session_changes`,
`thread_create_options`, `usage_summary`, `settings_get`, `settings_set`,
`thread_pin`, `thread_unpin`, `thread_settle` and `thread_unsettle`. A client
offers a feature only when its flag is listed; an older owner answers
`get_changes` with `unsupported_operation`.
`thread_create_options` is not a fixed feature of the gateway: it is listed
while a share that can start threads (the desktop app's) is live, so a client
reads it from every `list_sessions`/`subscribe_sessions` answer rather than
once at connect. A `sessions` frame pushed on a change does not repeat it.

`get_changes` reads the attached session's uncommitted changes in its
workspace for review. It is read-only: the owner runs `git status` and
`git diff` with optional locks off, never a Git command that writes. The
`changes` result carries `captured_at` (when the changes were read, for the
client to show how fresh they are), the workspace, `head` and `branch`, and up
to 100 files. Each file has its status, added and removed lines, and its
unified diff, at most 32 KiB per file and 160 KiB in all. A file past either
bound has `diff_truncated` set, and changed files past the 100 listed are
counted in `omitted_files`. A workspace that is not a Git repository, or a
read that takes over 10 seconds, answers `available: false` with a `reason`.
Reading again is always safe.

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

Session rows may include `last_active_at`, the RFC 3339 timestamp of the
newest transcript message. Clients show it as the session's last activity and
must tolerate its absence.

Assistant history messages, live turns, and `text_delta` updates may include
`thought_tokens` and `thought_tokens_estimated`. These are the engine's existing
estimates from observed reasoning text, scoped to one assistant segment,
not provider billing totals or a cumulative session total. The estimate flag
is `true` for these values. Missing values mean unavailable. Empty text deltas
can update the observation without appending content, using the normal event
sequence and replay rules.

### Question cancellation and remote commands

Snapshots advertise optional `capabilities`: `cancel_question`,
`execute_command`, `upload_image` and `host_threads`. Missing capabilities mean
that the owner or gateway needs an upgrade; clients must not emulate these
operations by sending provider prompts. `host_threads` is on the desktop app's
shares only: it says this owner starts threads a device asks for, and is what
the gateway turns into `thread_create_options` on the session list.

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
contains only the host name, port, protocol version (`protocol=1`), gateway
identifier (`gateway=…`) and optional handshakes (`caps=connect_requests,desktops`).
Discovery does not pair a device or grant access; a
[connection request](#connection-requests) only asks the owner.

Authenticated clients can use `list_directories { path? }` to browse existing
folders (default `~/`) and `create_session { path?, harness?, model? }` to
create a session there. The latter returns `session_created { session }`.

A gateway routes that request to the running desktop app when one is sharing a
thread: the app's shares advertise `host_threads` in their snapshot's
`capabilities`, and while one is live the gateway lists
`thread_create_options` and starts the thread in the app, on the harness and
model the device names (`rustcode` for the built-in one). It then shares it
like any other thread, and the client attaches to it as usual. An unknown
harness, a harness that is turned off on the host, or a model the host knows
the harness does not list is answered with `invalid_request` whose message
names the value, and nothing starts. The delivery rules are the existing ones:
the share's receipt store answers a retry with the original outcome, and a
retry with other arguments is `request_conflict`; a lost answer is found with
`get_request_status`, and a request the app did not answer within 30 seconds is
`owner_unavailable` with receipt `unknown`, which the client must not resend on
its own (the status reads `received` until the thread exists).

With no such share live (no app, Remote access off, or an app with no thread
open) nothing advertises the capacity: `harness` and `model` are ignored and
the gateway starts a detached session owner with the same runtime, tools,
questions, approvals, and turn cancellation as the TUI. Closing the app or
restarting the gateway does not stop that owner.

Folder listings omit dot-folders, cap their count and encoded size, and report
`truncated`; direct absolute and `~/` paths remain accepted. Both operations
accept only existing directories inside the host user's home folder or a
`roots` entry under `[remote]` in the host's `config.toml`; anything else is
refused with `invalid_request` and a message saying why, and nothing is
started. A listing's `parent` is absent when it would lead outside them.

Creation is a host-wide mutation: no session envelope is required. Its durable
receipt is scoped to the authenticated device and request ID. After a timeout,
reuse the same ID or query `get_request_status`; never generate a new ID merely
because confirmation has not arrived. Session owners hold a private per-session
lease, so concurrent launch/reconnect attempts cannot create duplicate owners.

The gateway answers every `create_session` and `resume_session` within 15
seconds of receiving it, waiting for other starts included, so clients with a
20-second request timeout always see the answer. The iOS app waits up to 75
seconds for these two, longer than the 60 s an owner may take to register. A start that cannot even
begin in that time is refused with `busy` and nothing recorded. When the owner
exits before registering, the gateway withdraws the receipt and answers with
error `internal` and receipt `rejected`: the session was not created, and the
message names the session, the reason, the owner's last output and its log on
the host. That outcome is final for the attempt, so the client may ask again
with the same or a new request ID.

An owner that is still starting at the deadline is answered with
`owner_unavailable` and receipt `unknown`; the client asks again with the same
request ID (or `get_request_status`). The gateway keeps the owner for up to 60
seconds from its launch: once it registers, the receipt holds the result and a
retry returns it; when it has not registered by then it is stopped and the
receipt withdrawn, and a retry then starts a new attempt. A retry that finds
the owner of an earlier attempt gone without registering answers `internal`
with receipt `rejected`.

The gateway preserves pairing credentials when upgrading. Existing terminal-owned
sessions keep running and reconnect, but retain their owner's older capabilities
until that terminal session is reopened with the updated binary. Newly created
mobile sessions immediately use the installed owner version. Automatic network
selection is retained in the login service instead of persisting a temporary
Wi-Fi/DHCP address; explicitly configured bind and advertised addresses remain
explicit.

### Saved sessions

Gateways that list `saved_sessions` in their `capabilities` let a device see
every session saved on the host, not only the shared ones, and open one that
nothing holds.

`list_saved_sessions { cursor?, limit?, query? }` returns one page,
`saved_sessions { sessions, next_cursor? }`, newest first by the time each
session was started; pass `next_cursor` back as `cursor` for the next page.
`limit` is 1 to 100 (30 when absent). `query` keeps sessions whose title,
workspace or ID contains it, ignoring case. Each entry has `session_id`,
`title`, `project` (the recorded workspace directory, so a client groups
threads by project; absent when unknown), `workspace` (absent for sessions
that predate workspace recording), `updated_at` (when the transcript was last
written), `message_count` and `state`:

| `state` | Meaning | What the client does |
| --- | --- | --- |
| `saved` | No process holds it. | `resume_session` |
| `shared` | It is in the live list. | attach to it |
| `open` | A terminal, the app or a session owner holds it without sharing it. | nothing, or `request_share` for a terminal |

An `open` entry may also carry `open_in`, what holds it: `app` (the RustCode
app on the Mac), `terminal` or `remote` (a session owner started for a device).
It is absent from older hosts and when the holder did not say; clients then
name no place, and treat a value they do not know the same way.

Only sessions the host's session store lists, whose files lie inside the
store, are reported: the same rule as ACP's `session/list`.

`resume_session { session_id }` opens a `saved` session. The gateway checks the
ID against the store, then starts a detached session owner on it in its
recorded workspace, exactly as `create_session` starts one for a new session,
and answers `session_resumed { session }` once it is shared; attach to it as
usual. The owner loads the saved transcript, its recorded settings and its task
worktree, if any. It is a host-wide mutation with the same durable receipt,
retry and `get_request_status` rules as `create_session`. A session that is
already shared is answered at once. One that a shared terminal or another
owner holds is refused with `busy`; one that another process has open without
sharing it (the RustCode app on the Mac, a terminal) with `open_elsewhere`. Every
process that runs a session holds an advisory lock on `open.lock` in the
session's directory while it has the session loaded; the kernel releases it
when the process ends, so a lock left by a dead process does not count. A
session without a recorded workspace, or whose
workspace no longer exists, with `invalid_request`, an unknown ID with
`not_found`; nothing is started for any of them.

### Usage, host settings and thread status

These five features are host-wide: they name no shared session, so they need
no `session_id` or `registration_epoch` on the envelope. Each is offered only
when its capability flag is listed in a `sessions` result, so a phone can tell
an older Mac from one that supports the operation.

`get_usage` (capability `usage_summary`) returns the last 30 days from the
store `rustcode usage` reads, today included. Every token field is what a
harness reported; `cost_usd` is only ever the USD a harness reported, never a
price RustCode computed. A field no turn reported is absent, not zero. Days
and groups are bounded to 1000 rows; `by_day` is newest first, the other two
are largest token total first.

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

`get_settings` (capability `settings_get`) returns only the host's default
harness, model and effort. No other host setting is visible over the remote
protocol.

```json
{"protocol_version": 1, "request_id": "req-settings", "operation": {"type": "get_settings"}}
```

```json
{"kind": "response", "protocol_version": 1, "request_id": "req-settings", "result": {"type": "settings", "harness": "codex", "model": "gpt-5.5", "effort": "high"}}
```

`set_settings` (capability `settings_set`) writes the same three fields.
`harness`, `model` and `effort` are the whole allowlist; a key outside it is
rejected with `invalid_request` and nothing is written. A field that is absent
(rather than `null`) keeps its current value. The answer is the complete
`settings` result after the write, with a `receipt`.

```json
{"protocol_version": 1, "request_id": "req-set-settings", "operation": {"type": "set_settings", "settings": {"harness": "codex", "model": "gpt-5.5", "effort": "high"}}}
```

```json
{"kind": "response", "protocol_version": 1, "request_id": "req-set-settings", "receipt": "applied", "result": {"type": "settings", "harness": "codex", "model": "gpt-5.5", "effort": "high"}}
```

`pin_thread`, `unpin_thread`, `settle_thread` and `unsettle_thread` use the
engine's own thread paths, so a pin or a settled state is the same one the
desktop app shows. They work for a saved thread as well as a shared one, and
name the thread in the operation because the shared-session envelope is not
required. Settling stores `done`; unsettling returns the thread to `idle`.
Answers are `thread_pinned { session_id, pinned }` and
`thread_settled { session_id, settled }`, each with a `receipt`.

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

Snooze is not in this list: it lives in the desktop app's own local storage,
not in the engine or the host's session store, so a paired device cannot set
it and no capability advertises it.

### Image attachments

Owners advertising `upload_image` accept authenticated, session-scoped JPEG/PNG
uploads. Send `attachment_id` (1–64 ASCII letters/digits, hyphens or underscores),
`mime_type`, byte `offset`, `total_bytes`, and `data_base64` in an `upload_image`
operation. Each decoded chunk is at most 128 KiB, each image at most 8 MiB and
4096 × 4096 pixels. Full image decoding checks the actual format and pixel limits.
An owner rejects uploads when neither the selected model nor a configured vision
fallback can handle images; selecting another model does not submit a turn.

`image_uploaded` replies contain `attachment_id`, `next_offset`, `total_bytes`,
and `complete`. Only the final reply includes `prompt_reference`. Append that
exact reference to the ordinary submit/queue/steer prompt. This uses the same real
multimodal provider input as terminal image paste; uploading itself never starts
a turn. Keep request IDs stable per image and offset when retrying. Replayed chunks
return their original progress even after completion; changed metadata or bytes
are rejected. The app should resize large photos before uploading.

Files are private (0700 directories, 0600 files) under the session's artifacts,
with a separate authenticated-device namespace. Wire input cannot supply device
identity or a destination path. Prompt references to incomplete uploads, other
sessions/devices, or arbitrary host files are rejected. A prompt may contain at
most eight images. Each session reserves at most 64 MiB and 256 attachments;
abandoned partial uploads expire after one hour and are removed on the next
upload. Completed images remain alongside session history for replay.
