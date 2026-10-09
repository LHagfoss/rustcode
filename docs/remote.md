# Remote sessions

Share a running terminal session with a paired device, watch it live and
continue it from there. The terminal keeps running the session and must stay
open; the device is a remote control. macOS and Linux only.

Three pieces are involved:

- the **terminal session** you run `/remote` in, which owns the session and
  applies every command;
- the **remote gateway**, a small host process that pairs devices, keeps the
  list of shared sessions and routes each device command to the terminal that
  owns the session. It never runs agent turns;
- the **device** (the iOS app, in its own repository), which speaks the
  [remote protocol](remote-protocol/README.md) to the gateway over WebSocket.

The plan this implements is
[Live terminal sessions from an iOS client](architecture/mobile-remote-plan.md).
The older `rustcode serve` transport in [mobile.md](mobile.md) is separate and
unchanged.

> **Plain LAN traffic is not encrypted.** The gateway has no TLS. Devices are
> authenticated, but on an ordinary LAN anyone on that network can read and
> alter the traffic, including your prompts and the session output. **NetBird
> is the recommended path**: its tunnel encrypts the traffic between phone and
> Mac. Use a plain LAN address only on a network you trust. Never forward the
> port to the internet.

## Quick start

On NetBird (recommended):

```sh
# 1. Find this Mac's NetBird address (100.x.y.z).
netbird status | grep "NetBird IP"

# 2. Start the gateway on it. Leave it running, or let /remote start it (below).
rustcode remote serve --bind 100.92.13.44

# 3. In the terminal session you want to share:
/remote
```

On a trusted LAN:

```sh
# 1. Find this Mac's LAN address.
ipconfig getifaddr en0          # macOS; `ip -4 addr` on Linux

# 2. Start the gateway on it.
rustcode remote serve --bind 192.168.1.20

# 3. In the terminal session you want to share:
/remote
```

`/remote` shows the address, a QR code and a manual code. Scan the QR code in
the app, or type the address and the code. The session then appears in the
app's session list.

To have `/remote` start the gateway itself, put the address in the user
`config.toml` once:

```toml
[remote]
bind = "100.92.13.44"      # the NetBird or LAN address of this machine
# port = 17879             # optional
# advertise = "mac.netbird.cloud"   # optional: the name devices should dial
```

With that, `/remote` in any session starts the gateway in the background if
none is running (its output goes to `remote/gateway.log` in the configuration
directory) and stops nothing when the terminal exits; `rustcode remote stop`
stops it.

## Loopback is the default

Without `--bind` and without `[remote]` in the config, the gateway listens on
`127.0.0.1`. That is deliberate: nothing is exposed until you name an address.
A phone cannot reach loopback, so `/remote`, `rustcode remote pair`,
`rustcode remote serve` and `rustcode remote status` all say so and print the
commands above together with this machine's candidate addresses. A session
shared with a loopback gateway is still registered and can be reached from the
same machine (the iOS Simulator, a test client).

To move a running gateway to another address: `rustcode remote stop`, then
`rustcode remote serve --bind <address>`. Sessions that were shared register
with the new gateway on their own within two minutes; devices stay paired.

## In the terminal

| Command | Behavior |
| --- | --- |
| `/remote` | Share the session on screen. Finds the running gateway or starts one, then shows the gateway address, a QR code and a manual pairing code. Repeating it keeps the registration and shows a fresh pairing challenge. |
| `/remote status` | Whether this session is shared, the gateway address, the devices attached to this session and the devices connected to the gateway. Names only, never a credential. |
| `/remote off` | Stop sharing. The session disappears from every device at once; a running turn is not affected. |

Switching to another session, resuming, forking or starting a new one ends the
registration. The new session is private until `/remote` is run in it.

A second terminal that tries to share a session another terminal already
shares is refused; the first one keeps it.

What a device can do with a shared session: read it, submit a prompt when it
is idle, steer or queue while it runs, cancel the running turn, answer a
question, approve or deny an approval batch. What it cannot do: run slash
commands (a prompt starting with `/` is refused), change the approval mode,
switch sessions, or read anything that is not part of a shared session. The
terminal's own draft, cursor and attachments are never touched.

## Gateway commands

| Command | Behavior |
| --- | --- |
| `rustcode remote serve [--bind <ip>] [--port <port>] [--advertise <host>]` | Run the gateway in the foreground until Ctrl-C, SIGTERM or `remote stop`. |
| `rustcode remote pair` | Ask the running gateway for a pairing challenge and show it: QR code, address and code. |
| `rustcode remote devices [--json]` | List paired devices: identifier, name, pairing time. Never a token. |
| `rustcode remote revoke <device>` | Revoke by identifier, identifier prefix (four characters or more) or unique name. |
| `rustcode remote status` | The running gateway: addresses, connected devices, shared sessions and who is attached to each. |
| `rustcode remote stop` | Stop the running gateway. |

One gateway runs per configuration directory. A second `remote serve` is
refused while the first holds its lock; a gateway that died is detected by
its process birth time and its files are reclaimed.

## Addresses

`--bind` (or `[remote] bind`) is where the WebSocket listener accepts
connections: `127.0.0.1` by default, port `17879`. `--advertise` is the host
that pairing details tell a device to dial; it takes an IP address or host
name and uses the listener's port.

- A loopback or specific bind advertises itself unless `--advertise` is given.
- `0.0.0.0` and `::` are never advertised. Binding a wildcard requires
  `--advertise` unless exactly one interface address is a candidate; the
  error lists the candidates.
- The `[remote]` section is read from the user configuration only. A project's
  `.rustcode/config.toml` cannot choose the interface your sessions are
  exposed on.

## Pairing

A pairing challenge (from `/remote` or `rustcode remote pair`) has two
representations:

- a QR code. It encodes compact JSON with `protocol_version`, `address`,
  `gateway_id`, a 256-bit `credential` and the host's name;
- the advertised address plus an eight-digit code, for typing by hand.

The code is drawn with half-block characters in explicit black on white with a
four-module quiet zone, so it scans in light and dark terminal themes. If the
terminal is too narrow for it the panel says so; use the address and code, or
`rustcode remote pair` in a wider window. When output is not a terminal,
`rustcode remote pair` prints the payload as text instead.

A challenge lasts two minutes and is single use: pairing with either
representation destroys both. It tolerates five wrong attempts, shared by
the code and the credential, and a new challenge replaces the previous one.
Ten failed attempts within five minutes, counted across all connections and
challenges, lock pairing for the rest of that window; while locked, even a
correct code is refused. A peer on the network can therefore block pairing for
a while, but cannot buy more guesses by reconnecting.

A paired device receives a random token once. The host stores only its
SHA-256 digest in `remote/devices.json` under the configuration directory
(directory `0700`, file `0600`, replaced atomically). Device names are
labels chosen by the device, not identities. Pairing gives a device access to
every session this host shares, now and later, until it is revoked.

## Revoking and stopping

- `/remote off` ends access to one session for every device.
- `rustcode remote revoke <device>` forgets the device and, when the gateway
  is running, closes all of its connections and subscriptions in the same
  step; the device receives a `revoked` error and must be paired again. With
  the gateway stopped, the device is removed from the store directly. If a
  running gateway does not answer, nothing is revoked and the command says so.
- `rustcode remote stop` stops the gateway. Shared sessions keep running in
  their terminals and look for a gateway again for two minutes; after that
  they report that sharing stopped.

## How it holds together

**Local sockets.** The gateway has two Unix sockets in `remote/` under the
configuration directory (`0700`), each `0600`: `control.sock` for the
`rustcode remote …` commands and `owner.sock` for terminals that share a
session. Only the user who runs the gateway can connect; the owner socket also
checks the peer's user ID. If the configuration path is too long for a socket
address, both sockets go to `/tmp/rustcode-<uid>/<hash>/` instead, created
`0700` and verified to belong to you.

**Liveness.** A terminal's connection to the owner socket is what keeps its
session listed. The terminal sends a heartbeat every five seconds; after 15
seconds of silence the session is listed as `unresponsive`, after 45 it is
removed. When the terminal exits, stops sharing or switches session, the
session is removed at once.

**Receipts.** Every mutation a device sends carries a `request_id`. The
terminal that owns the session keeps, per device and request ID, what it did
with it. Sending the same request again returns the original outcome and
applies nothing; the same ID with different content is refused. Because the
receipts live in the terminal, they survive a gateway restart. If the terminal
is gone, the answer is `unknown`: the device shows that it does not know and
never resends on its own.

**Ordering and reconnect.** Each terminal numbers its updates. The gateway
keeps the last snapshot and up to 4 MiB of the updates after it per session.
A device that reconnects with its last sequence number gets exactly the
updates it missed when they are all still there, and a fresh snapshot
otherwise. A cursor from before a gateway restart always gets a snapshot.

**Slow devices.** Each device has its own bounded queue. A device that does
not read fast enough is held back, told to resynchronise and given current
state when it catches up; one that stops reading altogether is closed. Neither
delays the terminal or another device.

## Limits

| Bound | Value |
| --- | --- |
| Handshake, from TCP accept to first frame | 10 s |
| Unauthenticated sockets | 16 in total, 4 per peer address; extra sockets are dropped on accept |
| Handshake frame | 8 KiB |
| Any frame from a device | 256 KiB for a request; 1 MiB closes the connection |
| Any frame to a device | 256 KiB |
| Authenticated connections | 32 |
| Paired devices | 32 |
| Shared sessions | 32 |
| Frames queued for one device | 64; responses that overflow close that device with `slow_consumer` |
| Requests one connection may have waiting on terminals | 16; more are refused with `rate_limited` |
| Wait for a terminal's answer | 30 s, then `owner_unavailable` |
| Replay kept per session | 4 MiB of events |
| Mutation receipts per registration | 1024; more mutations are refused with `receipt_capacity` |
| Silence before a device connection is closed | 90 s; any frame or WebSocket ping from the device counts (the gateway pings every 30 s) |

An upgrade request with an `Origin` header is refused with 403: browsers
send one and the native client does not, so a web page cannot reach the
gateway.

## Not covered yet

- No TLS; see the warning at the top.
- The terminal owns execution. Closing it ends the session's availability.
- No push notifications and no starting of new sessions from a device.
- Reachability from a real iPhone (local-network permission, NetBird routing)
  is verified with the app, not by this repository's tests.
