# Remote gateway (foundation)

Status: gateway foundation; no sessions are shared yet. This page describes
the host process that devices will pair with and authenticate against. The
`/remote` terminal command, session sharing and the iOS app from the
[remote plan](architecture/mobile-remote-plan.md) are not implemented: a
paired device can connect, and every frame it sends after authenticating is
answered with a `not_implemented` error.

The gateway is separate from the experimental `rustcode serve` transport in
[mobile.md](mobile.md), which is unchanged. It never runs agent turns.
macOS and Linux only.

## Commands

| Command | Behavior |
| --- | --- |
| `rustcode remote serve [--bind <ip>] [--port <port>] [--advertise <host>]` | Run the gateway in the foreground until Ctrl-C, SIGTERM or `remote stop`. |
| `rustcode remote pair` | Ask the running gateway for a pairing challenge and print it. |
| `rustcode remote devices [--json]` | List paired devices: identifier, name, pairing time. Never a token. |
| `rustcode remote revoke <device>` | Revoke by identifier, identifier prefix (four characters or more) or unique name. |
| `rustcode remote status` / `stop` | Inspect or stop the running gateway. |

One gateway runs per configuration directory. A second `remote serve` is
refused while the first holds its lock; a gateway that died is detected by
its process birth time and its files are reclaimed.

## Addresses

`--bind` is where the WebSocket listener accepts connections: `127.0.0.1`
by default, port `17879`. `--advertise` is the host that pairing details
tell a device to dial; it takes an IP address or host name and uses the
listener's port.

- A loopback or specific bind advertises itself unless `--advertise` is given.
- `0.0.0.0` and `::` are never advertised. Binding a wildcard requires
  `--advertise` unless exactly one interface address is a candidate; the
  error lists the candidates.
- There is no TLS. A non-loopback bind is authenticated but not encrypted;
  use it only over a trusted network such as NetBird. The gateway prints
  this warning when it starts.

## Pairing

`rustcode remote pair` prints one challenge with two representations:

- the advertised address plus an eight-digit code, for typing by hand;
- a QR payload, compact JSON with `protocol_version`, `address`,
  `gateway_id` and a 256-bit `credential`. The terminal prints the payload
  as text; drawing it as a QR code is not implemented.

A challenge lasts two minutes and is single use: pairing with either
representation destroys both. It tolerates five wrong attempts, shared by
the code and the credential, and a new `remote pair` replaces the previous
challenge. Ten failed attempts within five minutes, counted across all
connections and challenges, lock pairing for the rest of that window; while
locked, even a correct code is refused. A peer on the network can therefore
block pairing for a while, but cannot buy more guesses by reconnecting.

A paired device receives a random token once. The host stores only its
SHA-256 digest in `remote/devices.json` under the configuration directory
(directory `0700`, file `0600`, replaced atomically). Device names are
labels chosen by the device, not identities.

## Revocation

`rustcode remote revoke <device>` forgets the device and, when the gateway
is running, closes all of its connections in the same step; the device
receives a `revoked` error. With the gateway stopped, the device is removed
from the store directly. If a running gateway does not answer, nothing is
revoked and the command says so.

## Handshake frames

Devices connect with WebSocket and exchange JSON text frames. The first
frame must pair or authenticate, within ten seconds of connecting; nothing
else is sent to an unauthenticated socket except one error frame.

```json
{"type": "pair", "protocol_version": 1, "method": "code", "secret": "1234-5678", "device_name": "…"}
{"type": "pair", "protocol_version": 1, "method": "credential", "secret": "…", "device_name": "…"}
{"type": "authenticate", "protocol_version": 1, "device_id": "…", "token": "…"}
```

```json
{"type": "paired", "protocol_version": 1, "gateway_id": "…", "instance_id": "…", "device_id": "…", "device_name": "…", "token": "…"}
{"type": "authenticated", "protocol_version": 1, "gateway_id": "…", "instance_id": "…", "device_id": "…", "device_name": "…"}
{"type": "error", "code": "pairing_failed", "message": "…"}
{"type": "error", "code": "rate_limited", "message": "…", "retry_after_secs": 240}
```

After `paired` the same connection is authenticated. `gateway_id` is stable
for the host; `instance_id` changes each time the gateway starts. A failed
handshake closes the connection. Error codes: `invalid_frame`,
`frame_too_large`, `unsupported_version`, `handshake_timeout`,
`pairing_failed`, `rate_limited`, `unauthorized`, `busy`, `revoked`,
`slow_consumer`, `idle_timeout`, `shutting_down`, `not_implemented`,
`internal`.

An upgrade request with an `Origin` header is refused with 403: browsers
send one and the native client does not, so a web page cannot reach the
gateway.

These frames are provisional: they will be folded into the versioned remote
protocol envelope when it lands.

## Limits

| Bound | Value |
| --- | --- |
| Handshake, from TCP accept to first frame | 10 s |
| Unauthenticated sockets | 16 in total, 4 per peer address; extra sockets are dropped on accept |
| Handshake frame | 8 KiB |
| Any frame from a device | 1 MiB; a larger one closes the connection |
| Authenticated connections | 32 |
| Paired devices | 32 |
| Frames queued for one device | 64; overflow closes that device with `slow_consumer` |
| Silence before a connection is closed | 90 s (the gateway pings every 30 s) |
