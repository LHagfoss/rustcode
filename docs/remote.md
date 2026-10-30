# Remote sessions

Share a running terminal session with a paired device, watch it live and
continue it from there. A shared terminal session keeps running in its
terminal, which must stay open; the device is a remote control. A device can
also start a new session, or reopen any saved one, without a terminal (see
[Sessions started from a device](#sessions-started-from-a-device)). macOS and
Linux only.

Three pieces are involved:

- each **terminal session**, which owns its session and applies every
  command;
- the **remote gateway**, a small host process that pairs devices, keeps the
  list of shared sessions and routes each device command to the terminal that
  owns the session. It never runs agent turns;
- the **device** (the iOS app in [`ios/`](../ios/README.md)), which speaks the
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

Connect the phone and Mac to the same trusted Wi-Fi or LAN. In any terminal
session, run:

```text
/remote
```

The gateway starts automatically and chooses this machine's Wi-Fi or Ethernet
address. `/remote` shows its address and a QR code in a full-height panel. Open
the app, tap **Scan QR**, and scan it. Every session open in a terminal on
this machine appears in the app's session list, and so does every session
opened later: `/remote` switches remote access on for the machine, not for one
session. No IP address needs to be typed. Address and code are available
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
needs the new one: on the same network the app finds it by itself from the
Bonjour announcement, without a code; otherwise a fresh QR code gives it. An explicit `--bind <ip>` is never moved.

For an address that does not change at all, give the gateway a stable name
instead of an IP: `--advertise mac.netbird.cloud` (or `[remote] advertise`)
with the NetBird address as `--bind`, or `--bind 0.0.0.0 --advertise <name>`
on a trusted network. Prefer an IPv4 address or such a name over the Mac's
`.local` name: `.local` also resolves to IPv6 and to other interfaces (VM
bridges, VPNs) where the gateway does not listen. Bonjour announces the
service name, port, protocol version and the gateway's identifier, never
pairing material. The identifier lets a paired device recognise this host at
a new address and reconnect there with the token it already holds.

`rustcode remote status` and `/remote status` list the machine's current
addresses next to the listen and advertised address, and say whether the bind
is `auto` (follows address changes) or fixed and whether the advertised host
was given with `--advertise`. When the listener sits on an address the machine
no longer has, or a fixed advertised IP is gone, they warn and print the
command that recovers it:

```text
  warning        192.168.31.237 is no longer an address of this machine and the bind is fixed: no device can reach the gateway
  recover        `rustcode remote stop && rustcode remote serve --bind auto` (follows address changes from now on)
                 or `--bind 192.168.31.185` for the address it has now; a `[remote] bind` in config.toml needs the same change
```

With an `auto` bind the warning only says the gateway is about to move.

To choose another address by hand: `rustcode remote stop`, then
`rustcode remote serve` with an explicit `--bind`. Sessions that were shared
register with the new gateway on their own within two minutes; devices stay
paired.

## In the terminal

| Command | Behavior |
| --- | --- |
| `/remote` (`/remote on`) | Switch remote access on for this machine. Finds the running gateway or starts one and shares the session on screen; every other terminal shares its session within a couple of seconds, and so does every terminal started later. While no device is paired it shows the gateway address, a QR code and a manual pairing code; afterwards it shows the status. |
| `/remote pair` | Same, and always shows a fresh pairing challenge, to add another device. The panel counts the challenge down and shows the wrong codes left, a lockout, and how it ended (see [Pairing](#pairing)). |
| `/remote status` | Whether this session is shared, the gateway address, the devices attached to this session and the devices connected to the gateway, followed by everything `rustcode remote status` shows (addresses, paired devices, pairing state, shared sessions with this terminal's marked, log paths) once the gateway answers. Names only, never a credential. |
| `/remote off` | Switch remote access off for this machine: the gateway stops, every session disappears from every device, and nothing is shared until `/remote` is run again. Pairings are kept; running turns are not affected. |

The switch is remembered across restarts. While it is on, each interactive
terminal shares the session on its screen by itself: at start, and again under
a new registration after switching, resuming, forking or starting a session.
There is no per-session opt-out; a paired device can read and drive every
session that is open in a terminal. Headless runs (`-p`) are not shared.

A second terminal that tries to share a session another terminal already
shares is refused; the first one keeps it.

What a device can do with a shared session: read it, submit a prompt when it
is idle, steer or queue while it runs, cancel the running turn, answer a
question, approve or deny an approval batch, and run session slash commands
such as `/yolo`. What it cannot do: switch sessions, or read anything that is
not part of a shared session. The terminal's own draft, cursor and
attachments are never touched.

## Approvals and trusted mode

Every tool batch that needs confirmation waits for a tap on the device, which
is slow over intermittent links. `/yolo on` (also from the device) enables
full auto-approve for the session: batches run without asking. `/yolo off`
restores per-batch approval. The mode is per session: it is recorded with the
session (also when `--yolo` or a terminal toggle sets it), so the session owner
that serves it after a gateway restart or upgrade starts the same way. It never
changes your config. A fresh session starts asking unless `yolo = true` is set
in the user `config.toml` (see [configuration](configuration.md#yolo-by-default)).
Plan mode still blocks mutating tools either way. Prefer `/yolo on` only on
sessions whose workspace you trust, and turn it back off when leaving the
device unattended.

A device can also ask a terminal to share a session it has open but never
shared (`request_share` with the session id). The terminal announces its open
session to a running gateway on its own; announcing shares nothing. The
terminal shows `Device '<name>' asks to share this session. Press y to share
it, n to decline.` Answering y runs `/remote` there and resolves the request
as accepted — the device attaches with a fresh epoch. No answer within two
minutes, a decline, or the terminal going away resolves it as denied, which
`get_request_status` on the request id also reports. Any paired device can
ask. This applies only while a terminal's session is not shared, which with
remote access on is the short moment before it has registered.

## Gateway commands

| Command | Behavior |
| --- | --- |
| `rustcode remote serve [--bind <ip>] [--port <port>] [--advertise <host>] [--prefer-interface <name>]` | Run the gateway in the foreground until Ctrl-C, SIGTERM or `remote stop`. |
| `rustcode remote on` | Enable the persistent background gateway (alias of `start`). |
| `rustcode remote off` | Stop the running gateway (alias of `stop`). |
| `rustcode remote pair` | Ask the running gateway for a pairing challenge and show it: QR code, address, code and expiry time. In a terminal it then counts the challenge down until a device pairs or it ends (Ctrl-C leaves earlier); otherwise it prints and exits. |
| `rustcode remote devices [--json]` | List paired devices: identifier, name, pairing time, when each was last seen and expiry. Never a token. |
| `rustcode remote revoke <device>` | Revoke by identifier, identifier prefix (four characters or more) or unique name. |
| `rustcode remote status` | The running gateway: listen and advertised address (and whether each follows address changes), the machine's current addresses with a recovery command when the gateway's address is gone, paired devices and which are connected, the state of pairing (time and wrong codes left, or the lockout), shared sessions and who is attached to each, session owners that run without being shared, and the paths of the gateway log and the session logs. |
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

The pairing details state the lifetime, the wall-clock time the challenge
expires and the number of wrong codes allowed. While they are on screen a
line above them counts down, once a second, the time and the wrong codes
left, and says when pairing is locked and for how long:

```text
Waiting for a device: expires in 1:27, 4 wrong codes left.
Pairing is locked for 4:10 after too many wrong codes on this host: even the right code is refused until then. This code expires in 40 s.
```

When the challenge ends the line says how, and the QR code is taken down:
`Paired "Lars's iPhone".`, used up (five wrong codes), expired, replaced by a
newer challenge, or gone because the gateway stopped or restarted. In the
`/remote` panel this runs until the panel is closed or replaced;
`rustcode remote pair` in a terminal keeps running until the challenge ends
(Ctrl-C leaves earlier, the challenge stays valid). When output is not a
terminal, `rustcode remote pair` prints the details once and exits. If pairing
is already locked when the challenge is issued, the details say so first.
`rustcode remote status` shows the same numbers at any time. An expired or
used-up challenge is replaced by running `/remote pair` or
`rustcode remote pair` again.

A paired device receives a random token once. The host stores only its
SHA-256 digest in `remote/devices.json` under the configuration directory
(directory `0700`, file `0600`, replaced atomically). Device names are
labels chosen by the device, not identities. Pairing gives a device access to
every session this host shares, now and later, until it is revoked or the
pairing expires.

A pairing lasts 30 days. Within them the device reconnects on its own, with
no code. After them its token is refused like an unknown one and the device
pairs again; a connection that is open at that moment stays open until it
drops. `rustcode remote devices` shows when each pairing expires. An expired
device is forgotten when the next one pairs and does not count against the
device limit. Pairings made before this limit existed are counted from the
day they were made.

Pairing the same app install again replaces its device record instead of
adding one: the app sends a random install identifier that it keeps, and the
host drops the earlier record and its token. A connection still open with the
old token is closed as revoked. Apps that do not send an identifier, and
records paired before the host kept one, add a record each time; those
duplicates are not merged. They expire 30 days after they were paired, or can
be revoked earlier by identifier. `rustcode remote devices` and
`rustcode remote status` show when each device last paired or connected
(`unknown` for records stored before this was kept), which tells the stale
ones from the one in use.

## Revoking and stopping

- `/remote off` (or `rustcode remote stop`) ends access to every session for
  every device until `/remote` is run again.
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
one (`list_directories`, `create_session`). It sees and may use only folders
inside your home folder and the `roots` listed under `[remote]` in
`config.toml` (see [configuration](configuration.md#remote-gateway-address));
a folder elsewhere, one that does not exist, or a file is refused with
`invalid_request` and a message that says which, and nothing is started. The
device attaches to the new session as soon as it has registered. It is listed
for every device at once; in the Mac app, `/resume` and `rustcode threads` it
appears after its first exchange, like any other session. No terminal is involved: the
gateway starts a **session owner**, a `rustcode` process without a terminal
that runs in that directory, owns the session and registers it like `/remote`
does. It keeps running after the device disconnects and across gateway
restarts, and ends when it is killed or the machine restarts.

- Each owner writes to `remote/owners/<session id>.log`: its start, its
  registration and the reason whenever it gives up. The request's receipt is
  `remote/created/<hash>.json`.
- The device always gets an answer within 15 seconds of asking, well inside
  the app's own limit for starting a session (75 seconds; older apps 20), including any time spent waiting for another
  session to start.
- An owner that exits before registering is reported at once: the device gets
  an `internal` error with receipt `rejected` that names the session, the
  reason, the owner's last output and its log. Nothing is left behind, so the
  device may ask again, with the same request ID or a new one.
- An owner that is still starting after 15 seconds (a slow login shell, say)
  is not stopped yet: the device gets `owner_unavailable` with receipt
  `unknown`, naming the session, and asks again with the same request ID.
  The gateway keeps waiting for that owner for up to 60 seconds; once it
  registers, the receipt holds the session and the retry is answered with it.
  An owner that has not registered by then is stopped and the receipt
  withdrawn, so the next attempt starts afresh.
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
  are running without being shared under `idle owners`.

### Saved sessions

A device also sees the sessions saved on the host that are not shared right
now, newest first and searchable (`list_saved_sessions`), and can reopen one
(`resume_session`). Reopening works exactly like starting a new session: the
gateway starts a session owner on the saved session, in the workspace
recorded with it, and the device attaches once it has registered. The owner
picks up the transcript, the session's settings and its task worktree. The
same receipt, retry, 20-second start limit and owner log rules apply.

Only sessions in the host's session store can be reopened, by their full ID.
A session that a terminal, the RustCode app or another owner has open without
sharing it is refused instead of being opened twice (the app shows where it is
open: turn on Remote access for it in the Mac app, or run `/remote` in the
terminal), and so is one with no recorded
workspace or whose workspace directory no longer exists: reopen those in a
terminal with `rustcode --resume`. The gateway advertises this with the
`saved_sessions` capability; an older gateway lists only shared sessions.

To continue a device-started session in a terminal, run
`/adopt <session-id>` there: it asks the detached owner to exit (gracefully,
saving history first), waits for its lease to release, then opens the session
exactly like a resume. The terminal shares it again by itself; devices
re-attach with a fresh epoch. `/adopt` needs the full session id, never runs
from a device, and refuses when the owner cannot be identified safely.

On macOS an installed `rustcode` runs the gateway as a login agent. The agent
keeps the search path it was installed with, so asking for the gateway from a
terminal with another environment no longer rewrites the agent and restarts
the gateway under its devices. `rustcode remote stop` removes the agent; the
next start installs it with the current environment.

## Choosing the address a device dials

- **Same Wi-Fi or LAN:** nothing to choose. `auto` picks this machine's private
  IPv4 address, the QR code carries it, and the listener follows DHCP changes
  by itself (see [Automatic address selection](#automatic-address-selection)).
- **Do not type the `.local` name.** `mac.local` resolves over mDNS to every
  address the machine has, IPv6 and VM bridges or VPNs included, and a phone
  may try one where the gateway does not listen (`-1004 could not connect` in
  the app). Use the IPv4 address from the QR code or `remote status`.
- **IPv6:** `auto` prefers IPv4 and uses a routable IPv6 address only when
  the machine has no LAN IPv4 address. Link-local `fe80::` addresses are never
  used: they need a scope the phone cannot know. `--bind <ipv6>` chooses one
  explicitly; pairing details then show it as `[address]:port`.
- **Away from home, or a name that never changes:** NetBird. On both devices
  join the same NetBird network, then on the Mac
  `rustcode remote stop && rustcode remote serve --bind <netbird ip> --advertise <host>.netbird.cloud`
  (or the same two values as `[remote] bind` and `advertise`). On Linux
  `--prefer-interface netbird` finds the address by itself.
- **Bonjour** announces the service name, port, protocol version and
  gateway identifier, but no address on purpose: the phone resolves the
  announcement itself on the network it is on, so the record cannot go stale
  after a DHCP change and never points at an interface the phone cannot
  reach. A paired phone that finds its gateway identifier at a new address
  reconnects there with its token.

## Security posture

- **Transport:** no TLS. On a plain LAN anyone on the network can read and
  alter the traffic. Use it only on a network you trust; NetBird encrypts the
  traffic end to end and is the recommended path off the home LAN. Never
  forward the port to the internet. The app shows an unencrypted-LAN banner
  for a plain address.
- **Pairing:** a 2-minute, single-use challenge with five wrong codes; ten
  failures in five minutes lock pairing for the host. Only the local user can
  ask for a challenge (control socket `0600`).
- **Access is per host.** A paired device can read and drive every session
  this machine shares, now and later, for 30 days or until revoked. There are
  no per-session grants and no per-session opt-out. Per-session grants have
  been considered; they are not planned for now.
- **Ending access:** `rustcode remote revoke <device>` closes that device's
  connections in the same step and forgets its token; `/remote off` or
  `rustcode remote stop` ends access for every device (see
  [Revoking and stopping](#revoking-and-stopping)).
- **Browsers** cannot reach the gateway: an upgrade request with an `Origin`
  header is refused.

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
| Answer to `create_session` or `resume_session` | within 15 s of the request; an owner still starting then gets `owner_unavailable` |
| Wait for a new session's owner to register | 60 s, then the owner is stopped and its receipt withdrawn |
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
  session started or reopened from a device is owned by its session owner
  process instead.
- A device starts sessions only in folders inside the home folder or a
  configured `[remote] roots` entry, and cannot create folders.
- A session started from a device shows up in the Mac app and the terminal's
  session lists only after its first exchange.
- No push notifications.
- Access is per host: a paired device sees every session open in a terminal.
  There are no per-session grants and no per-session opt-out.
- Reachability from a real iPhone (local-network permission, NetBird routing)
  is verified with the app, not by this repository's tests.
