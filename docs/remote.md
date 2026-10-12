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

Connect the phone and Mac to the same trusted Wi-Fi or LAN. In the terminal
session you want to share, run:

```text
/remote
```

The gateway starts automatically and chooses this machine's Wi-Fi or Ethernet
address. `/remote` shows its address and a QR code in a full-height panel. Open
the app, tap **Scan QR**, and scan it. The shared session appears in the app's
session list. No IP address needs to be typed. Address and code are available
as a fallback when the camera cannot scan.

The QR uses the Mac's address, not the router's address or its public internet
IP. A public IP alone cannot route through your router to the Mac. For access
away from home, connect both devices to NetBird and set this machine's NetBird
address once in the user `config.toml`:

```toml
[remote]
bind = "100.92.13.44"      # this machine's NetBird address
# port = 17879             # optional
# advertise = "mac.netbird.cloud"   # optional: the name devices should dial
```

Alternatively, run `rustcode remote serve --bind 100.92.13.44` first. Obtain
the address with `netbird status`. Ordinary same-LAN pairing needs no config.

The gateway runs in the background when started by `/remote`; output goes to
`remote/gateway.log` in the configuration directory. It keeps running when the
terminal exits; `rustcode remote stop` stops it.

## Automatic address selection

Without `--bind` or a configured bind, `auto` chooses an active Wi-Fi/Ethernet
address, preferring private IPv4 LAN addresses over VPN addresses. The OS route
breaks ties between LAN interfaces. Virtual bridges and link-local addresses
are excluded. The listener binds only the selected address and advertises it.

To prefer the mesh instead (stable across Wi-Fi changes, reachable off-LAN),
set `[remote] prefer_interface = "netbird"` — or
`rustcode remote serve --prefer-interface netbird`. An `auto` bind then picks
the NetBird interface address first (`wt`-prefixed devices, or any interface
with netbird in its name) and falls back to the LAN default when NetBird is
down. On macOS NetBird uses a generic `utun` device shared with other VPNs,
so there prefer `--advertise <host>.netbird.cloud` instead.

With no usable network interface, it falls back to `127.0.0.1` and explains
that a phone cannot reach it. Explicit `--bind 127.0.0.1` keeps local-only
access for a Simulator or test client.

The selection is repeated every five seconds while the gateway runs. When the
answer changes (DHCP handed out another address, the machine joined another
network, Wi-Fi came up after the gateway did) the listener moves to the new
address on the same port, without a restart: pairing details, the QR code,
`remote status` and `/remote status` name the new address, shared sessions
stay registered and devices stay paired. A device that still dials the old address
needs the new one: the app's nearby list or a fresh QR code gives it. An explicit `--bind <ip>` is never moved.

For an address that does not change at all, give the gateway a stable name
instead of an IP: `--advertise mac.netbird.cloud` (or `[remote] advertise`)
with the NetBird address as `--bind`, or `--bind 0.0.0.0 --advertise <name>`
on a trusted network. Prefer an IPv4 address or such a name over the Mac's
`.local` name: `.local` also resolves to IPv6 and to other interfaces (VM
bridges, VPNs) where the gateway does not listen. Bonjour announces only the
service name, port and protocol version, never an address or pairing material.

`rustcode remote status` lists the machine's current addresses next to the
listen and advertised address, and warns when the listener sits on an address
the machine no longer has.

To choose another address by hand: `rustcode remote stop`, then
`rustcode remote serve` with an explicit `--bind`. Sessions that were shared
register with the new gateway on their own within two minutes; devices stay
paired.

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

A device can also ask a terminal to share a session it has open but never
shared (`request_share` with the session id). The terminal announces its open
session to a running gateway on its own; announcing shares nothing. The
terminal shows `Device '<name>' asks to share this session. Press y to share
it, n to decline.` Answering y runs `/remote` there and resolves the request
as accepted — the device attaches with a fresh epoch. No answer within two
minutes, a decline, or the terminal going away resolves it as denied, which
`get_request_status` on the request id also reports. Any paired device can
ask; nothing is shared without the terminal's explicit yes.

## Gateway commands

| Command | Behavior |
| --- | --- |
| `rustcode remote serve [--bind <ip>] [--port <port>] [--advertise <host>] [--prefer-interface <name>]` | Run the gateway in the foreground until Ctrl-C, SIGTERM or `remote stop`. |
| `rustcode remote pair` | Ask the running gateway for a pairing challenge and show it: QR code, address and code. |
| `rustcode remote devices [--json]` | List paired devices: identifier, name, pairing time. Never a token. |
| `rustcode remote revoke <device>` | Revoke by identifier, identifier prefix (four characters or more) or unique name. |
| `rustcode remote status` | The running gateway: listen and advertised address, the machine's current addresses, paired devices and which are connected, the state of pairing (time and wrong codes left, or the lockout), shared sessions and who is attached to each, session owners that run without being shared, and the paths of the gateway log and the session logs. |
| `rustcode remote stop` | Stop the running gateway. |

One gateway runs per configuration directory. A second `remote serve` is
refused while the first holds its lock; a gateway that died is detected by
its process birth time and its files are reclaimed.

## Addresses

`--bind` (or `[remote] bind`) is where the WebSocket listener accepts
connections: `auto` by default, port `17879`. `--advertise` is the host
that pairing details tell a device to dial; it takes an IP address or host
name and uses the listener's port.

- A loopback or specific bind advertises itself unless `--advertise` is given.
- `auto` selects one local address as described above; `bind = "auto"` also
  works in the user configuration.
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

The pairing details state the lifetime and the number of wrong codes allowed.
They are a snapshot, not a countdown: `rustcode remote status` shows how many
seconds and wrong codes the open challenge has left, or for how long pairing
is locked. An expired or used-up challenge is replaced by running `/remote` or
`rustcode remote pair` again.

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

## Sessions started from a device

A paired device can browse the host's directories and start a new session in
one (`list_directories`, `create_session`). No terminal is involved: the
gateway starts a **session owner**, a `rustcode` process without a terminal
that runs in that directory, owns the session and registers it like `/remote`
does. It keeps running after the device disconnects and across gateway
restarts, and ends when it is killed or the machine restarts.

- Each owner writes to `remote/owners/<session id>.log`: its start, its
  registration and the reason whenever it gives up. The request's receipt is
  `remote/created/<hash>.json`.
- An owner has 20 seconds to register. One that exits first, or does not make
  it, is stopped by the gateway, and the device gets an `internal` error with
  receipt `rejected` that names the session, the reason, the owner's last
  output and its log. Nothing is left behind, so the device may ask again,
  with the same request ID or a new one.
- A session has at most one owner process. A retry that finds the owner of an
  earlier attempt still running (after a gateway restart, say) waits for that
  one instead of starting another; if it still does not register, the answer
  is `owner_unavailable` with receipt `unknown`, naming the receipt and the
  log. `get_request_status` with the request ID reports `applied` and the
  session once the owner has registered.
- An owner never starts or replaces a gateway. One whose first registration is
  refused exits with the reason in its log. A registered owner that the
  gateway drops (the machine slept, for example) registers again by itself;
  after five refusals in a row it exits.
- `rustcode remote stop` stops the gateway only. Owners keep running and
  register with the next gateway. `rustcode remote status` lists owners that
  are running without being shared under `idle owners`; end one with `kill`.

On macOS an installed `rustcode` runs the gateway as a login agent. The agent
keeps the search path it was installed with, so asking for the gateway from a
terminal with another environment no longer rewrites the agent and restarts
the gateway under its devices. `rustcode remote stop` removes the agent; the
next start installs it with the current environment.

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
| Wait for a new session's owner to register | 20 s, then the owner is stopped and the request fails with `internal` |
| Interval at which the listen address is resolved again | 5 s |
| Replay kept per session | 4 MiB of events |
| Mutation receipts per registration | 1024; more mutations are refused with `receipt_capacity` |
| Silence before a device connection is closed | 90 s; any frame or WebSocket ping from the device counts (the gateway pings every 30 s) |

An upgrade request with an `Origin` header is refused with 403: browsers
send one and the native client does not, so a web page cannot reach the
gateway.

## Not covered yet

- No TLS; see the warning at the top.
- The terminal owns execution. Closing it ends the session's availability. A
  session started from a device is owned by its session owner process instead.
- No push notifications.
- Pairing details show the challenge's lifetime but do not count it down.
- Access is per host: a paired device sees every shared session. There are no
  per-session grants.
- Reachability from a real iPhone (local-network permission, NetBird routing)
  is verified with the app, not by this repository's tests.
