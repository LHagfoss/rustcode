# Remote sessions

Share a running terminal session with a paired device, watch it live and
continue it from there. A shared terminal session keeps running in its
terminal, which must stay open; the device is a remote control. A device can
also start a new session, or reopen any saved one, without a terminal (see
[Sessions started from a device](#sessions-started-from-a-device)). macOS,
Linux and Windows; see [Windows](#windows) for what differs there.

Three pieces are involved:

- each **terminal session**, which owns its session and applies every
  command;
- the **remote gateway**, a small host process that pairs devices, keeps the
  list of shared sessions and routes each device command to the terminal that
  owns the session. It never runs agent turns;
- the **device** (the Expo phone app in the `rustcode-mobile` repository), which speaks the
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
address to advertise, preferring private IPv4 LAN addresses over VPN addresses.
The OS route breaks ties between LAN interfaces. Virtual bridges and
link-local addresses are excluded.

The same plan also opens a listener on every other LAN address and on every
overlay address a tunnel interface has: a `100.64.0.0/10` address as NetBird,
Tailscale and ZeroTier hand out, and Tailscale's `fd7a:115c:a1e0::/48`. The
machine therefore answers at home on the LAN and away from home over the mesh,
without `--bind` or `--advertise`. Tunnel interfaces are recognised by name on
every OS (`utun*`, `tun*`, `wg*`, `wt0`, `tailscale0`, `zt*`, `wintun`, and the
Windows adapter names `NetBird`, `Tailscale`, `WireGuard Tunnel`,
`ZeroTier One`); a private address on one of them is an overlay address, and on
Wi-Fi or Ethernet it is the LAN. A `100.64.0.0/10` address on Wi-Fi or
Ethernet is a carrier's CGNAT address, not a mesh one: it is never a candidate,
never bound, and never reported as a route. `--advertise` still replaces the
address pairing details name.

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
network, Wi-Fi came up after the gateway did, a mesh VPN came up or went down)
the listeners follow on the same port, without a restart: the primary listener
moves to the new LAN address and the extra LAN and overlay listeners open and
close with their interfaces. Pairing details, the QR code, `remote status` and
`/remote status` name the new address, shared sessions stay registered and
devices stay paired. A device that still dials the old address
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

## In the desktop app

Settings › **Remote access** is the same switch, without a terminal. Turn it
on and every thread this computer has is listed for paired devices — across
all projects, from the thread store, not only the ones the app has open. Every
open thread is shared live as well, on the built-in harness and on Codex, Claude
Code, Pi, OpenCode or any other external harness ([harnesses](harnesses.md)); a
thread opened later is shared as soon as it exists, and closing it ends its
live share. Opening a thread that is not open reopens it the way a device
reopens a saved one (see [Saved sessions](#saved-sessions)). Switching it off
stops the gateway and every device, exactly as `/remote off` does; pairings are
kept.

| In the app | What it does |
| --- | --- |
| **Remote access › Network access** | On: starts the gateway and lets paired devices reach every thread on this computer, and installs the RustCode server as a background service first if it is not installed (after a confirmation: it stays up when the app closes and starts when you log in, see [server](server.md)). Off: stops the gateway. |
| **Remote access › RustCode server** | Whether the RustCode server is installed to run in the background (*Installed · Running*), with *Install* when it is not. Installed, the shared engine stays up when the app closes and starts when you log in; the threads this app starts are still its own engine's and end with it, until the app is switched to the server. |
| **Remote access › Gateway address** | The `host:port` a device dials, how many devices are connected, and whether it is reachable from another machine at all. |
| **Remote access › Pair a device** | Calls the same pairing challenge as `/remote pair`: a QR code, the address, the manual code, the countdown and the wrong codes left. Remote access is switched on first if it was off. |
| **Pairing** | The challenge on screen, its countdown, the wrong codes left, a lockout, and *Paired ‹name›* once a device pairs. |
| **A phone asking to connect** | A card in the window's corner, over any page: *‹name› wants to connect*, the platform and address, *Deny* and *Accept*. Accept shows the code to enter on the phone (large, with *Copy* and its countdown) until the phone pairs or the code runs out. See [Connection requests](#connection-requests). |
| **Remote access › Asking to connect** | The same requests with the same answers, while any are open. |
| **Paired devices** | How many devices are paired, with their names and when each was last seen. *Revoke* forgets one and disconnects it at once, as `rustcode remote revoke` does. |
| **Remote access › Refused connections** | Devices the gateway turned away, newest first, for a quarter of an hour after each stopped trying: an app that speaks another remote protocol version, a pairing code that did not work, an unknown or revoked pairing, or an app that was refused before it could say anything. Each says what happened and what to do, so a phone whose only message is that it cannot connect is answered here. See [Refused connections](#refused-connections). |
| **Routes** | The addresses the phone can dial this computer at, best first: each LAN address it listens on, each overlay address (NetBird, Tailscale or ZeroTier, with its provider when the interface name says) and the machine's `.local` name. The handshake reports the same list, so a phone learns it and keeps using the first route that answers. |
| A thread's header chip | Shown while a thread is shared; it carries the number of devices attached to that thread. |

Each shared thread tells a device which harness runs it and its model
(`harness` and `model` on the session list and on every snapshot), so a phone
labels a Claude Code thread as such. The app and a terminal share one setting:
`/remote on` in a terminal shares the app's open threads too, and the app's
switch shows that it is on. While a
thread runs on an external harness, a prompt sent from a device is run through
the harness exactly as the app's own message is, and the engine echoes it to
the app as a `user_message_chunk`, so the chat shows it live; cancelling,
steering, answering a question and resolving an approval work the same way.
One rough edge: an approval a device resolves leaves the app's approval card
on screen until the turn ends, because ACP has no way to withdraw a request
already sent to the client.

A host streams at most 32 live shares at once, the threads used most recently.
That bound limits streaming, not what a device sees: every thread on the
machine is in the paged `list_saved_sessions` list, newest first, with its
project, title, last write and state, and a device opens one that is not
shared with `resume_session` (see [Saved sessions](#saved-sessions)). Prompts a
device queues on a thread are dropped when sharing ends, and a share the
gateway turns down is retried no more than once a minute.

The ACP methods behind the page (`_rustcode/remote`, `/start`, `/pair`,
`/stop`, `/answer`, `/revoke`, in `engine/src/acp/remote.rs`) take an
optional `sessionId` (the first four): with it
they describe and drive that thread's share, without it the machine's own
switch, gateway and challenge. Their answers add `qrRows` (the QR code as rows
of `0` and `1`, quiet zone included), `target` (the harness a shared thread
runs on), `notShared` (why this thread is held back on a thread's answer, the
count of held-back threads on the machine's) and, for the machine,
`gateway.pairing`, `gateway.connectedDevices` and `gateway.routes` (each
route's `address`, `kind` `lan`, `overlay`, `hostname` or `public`, and the
`provider` of an overlay address) from the gateway's own status, plus `devices` (each paired device's `id`, `name`, `kind` `phone`,
`desktop` or `cli`, `platform`, `hostId`, `lastSeenAt` and, on the machine's
answer, `connected`), `connectRequests` (open: `id`, `deviceName`,
`deviceKind`, `platform`,
`remoteAddr`, `ageSecs`, `expiresInSecs`, `state` `pending` or `approved`) and
`endedConnectRequests` (the last eight: `id`, `deviceName`, `end`, `deviceId`
when it paired, `endedSecsAgo`) and `refusedConnections` (the last eight
attempts the gateway turned away, newest first: `reason`, `attempts`,
`deviceName`, `deviceKind`, `platform`, `protocolVersion`,
`supportedVersions`, `remoteAddr` and `refusedSecsAgo`). `_rustcode/remote/answer { id, accept }`
answers with the machine's status plus `connectAnswer` (`code`,
`expiresInSecs` and `instanceId` on acceptance) or `connectError`;
`_rustcode/remote/revoke { deviceId }` with `revoked` or `revokeError`. The
app reads the machine's status every 2.5 s while the gateway runs, and every
15 s otherwise, whichever page is open. The wire protocol to the gateway
([remote-protocol](remote-protocol/README.md)) changed only additively:
session entries gained an optional `harness`, and every other field and frame
works as before.

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
| `rustcode remote devices allow <id>` / `deny <id>` | Grant full ACP control, or deny it and immediately disconnect the device. |
| `rustcode remote devices [--json]` | List paired devices: identifier, kind (phone, desktop or cli), platform, how long ago each was last seen, pairing time, expiry and name. JSON includes the host-controlled `acp` grant. Never a token. |
| `rustcode remote revoke <device>` | Revoke by identifier, identifier prefix (four characters or more) or unique name. |
| `rustcode remote status` | The running gateway: every address the WebSocket listener is bound to, one line each with its interface and the kind of network it is on (so an `auto` bind shows its LAN and its overlay addresses), and the advertised address with whether each follows address changes; who started it (the RustCode server, the app or a terminal), the machine's current addresses with a recovery command when the gateway's address is gone, paired devices and which are connected, the state of pairing (time and wrong codes left, or the lockout), devices the gateway turned away recently with the reason, shared sessions and who is attached to each, session owners that run without being shared, and the paths of the gateway log and the session logs. |
| `rustcode remote stop` | Stop the running gateway. |

One gateway runs per configuration directory. A second `remote serve` is
refused while the first holds its lock; a gateway that died is detected by
its process birth time and its files are reclaimed. `remote start` adopts a
gateway that already runs — the RustCode server's, for instance — instead of
starting a second one.

## Addresses

`--bind` (or `[remote] bind`) is where the WebSocket listener accepts
connections: `auto` by default, port `17879`. `--advertise` is the host
that pairing details tell a device to dial; it takes an IP address or host
name and uses the listener's port.

- A loopback or specific bind advertises itself unless `--advertise` is given.
- `auto` selects the LAN-first address to advertise and opens the other LAN
  and overlay listeners, as described above; `bind = "auto"` also works in the
  user configuration.
- `0.0.0.0` and `::` are never advertised. Binding a wildcard requires
  `--advertise` unless exactly one interface address is a candidate; the
  error lists the candidates.
- The pairing details and the QR code carry the listener's other addresses as
  `alternates`, so a device that cannot reach the advertised address (say, a
  mesh address on a phone without the mesh VPN) tries the LAN instead: private
  IPv4 on Wi-Fi/Ethernet first, then private and mesh addresses on tunnel
  interfaces (100.64.0.0/10 included), then global IPv6, at most four. The
  advertised address, loopback, link-local and container/VM bridges are left
  out. A wildcard bind offers every candidate of its family; an `auto` bind
  offers the addresses it opened; a specific bind offers none.
- The `[remote]` section is read from the user configuration only. A project's
  `.rustcode/config.toml` cannot choose the interface your sessions are
  exposed on.

After authentication the gateway reports every address it answers on
(`routes` on `paired` and `authenticated`), best first: the advertised
address, each LAN address, each overlay address on a tunnel interface, and the
machine's `.local` name. Each is `{ "address": "host:port", "kind": … }`, where
`kind` is one of

| Kind | What it is |
| --- | --- |
| `lan` | a private IPv4 address on Wi-Fi or Ethernet |
| `overlay` | an overlay or VPN address, with `provider` `netbird`, `tailscale` or `zerotier` when the interface name says which; absent on macOS's shared `utunN` |
| `hostname` | a name, such as the machine's `.local` name |
| `public` | a public address or name; a client refuses to send a token over one until there is encrypted transport (#2651) |

Only addresses the gateway is actually listening on are reported: an address
it does not answer on is not a route. A device adds them to the routes it
keeps for this host, so pairing once is enough to reach it on another network.
The same list is in `rustcode remote status` and Settings › Remote access; the
pairing QR code's `alternates` stay compatible with it, and with an `auto` bind
they name the LAN and overlay addresses a device may dial instead.

## Pairing

A pairing challenge (from `/remote` or `rustcode remote pair`) has two
representations:

- a QR code. It encodes compact JSON with `protocol_version`, `address`,
  `gateway_id`, a 256-bit `credential`, the host's name and, on a wildcard
  bind, `alternates` (see Addresses);
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

A device that asks for `token_rotation` in its `pair` frame has no expiry: the
pairing lasts until it is revoked. (The gateway takes the capability today;
the phone app's client change belongs to its own repository.) It holds a refresh
token, which every reconnect replaces, and a 15-minute access token that
reconnects without replacing anything. Only digests are stored, and the
access token's digest stays in memory, so a gateway restart asks for a
refresh. Presenting a refresh token the host has already replaced revokes the
whole pairing: the device's connections close as revoked with
`token_replayed`, the record is removed as if revoked, and the gateway logs
`remote.token_replayed` for the owner. The removal is written before anything
closes; if the device store cannot be written, the revocation is noted in
`remote/revocations.json`, retried, and the device is refused meanwhile. That
note is applied before the gateway serves anyone: if it cannot be read the
gateway refuses to start and names it, rather than let a revoked family back
in; repair it, or delete it after `rustcode remote revoke`. Pair again if that
was you.
Until the device acknowledges the new token, presenting the previous one again
is a retry that gets the same tokens back, so a lost acknowledgement never
locks it out and a copy of the previous token cannot rotate the device's token
away; after a gateway restart such a retry is a replay. A client acknowledges a
rotation once it has stored the new token; the phone app's client change
belongs to its repository. A family that has rotated more tokens than the host
keeps history for (1024) is revoked rather than letting an old token be
answered as a wrong one.

A device that does not ask for rotation keeps the static token, and its
pairing lasts 30 days. Within them the device reconnects on its own, with no
code. After them its token is refused like an unknown one and the device
pairs again; a connection that is open at that moment stays open until it
drops. `rustcode remote devices` shows when each pairing expires, or `until
revoked` for a device that rotates its tokens. An expired device is forgotten
when the next one pairs and does not count against the device limit. Pairings
made before this limit existed are counted from the day they were made.

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

## Connection requests

Instead of racing a two-minute QR code, a phone can ask. It finds the Mac on
the local network (Bonjour `_rustcode._tcp`, whose record lists
`caps=connect_requests`) and taps *Connect*; the gateway holds the request
and the desktop app shows *‹name› wants to connect* wherever the user is.

1. The phone opens a WebSocket and sends `connect_request` with its name,
   platform and install identifier. It carries no secret. A desktop app on
   another machine asks the same way with `device_kind: "desktop"` and its
   machine's identifier (the record lists `caps=connect_requests,desktops`),
   and is shown and listed as, for example, *Windows desktop*.
2. The Mac shows the request. *Deny* ends it, and that address cannot ask
   again for 30 seconds. Nobody answering ends it after two minutes.
3. *Accept* issues a pairing challenge bound to that request: it replaces any
   outstanding challenge (a QR code on screen, or another accepted request),
   and the Mac shows its eight-digit code.
4. The user types or pastes the code on the phone, which sends it on the same
   connection as an ordinary `pair` frame. Only that connection can redeem it,
   and it may try once: a wrong code ends the request, and the phone asks
   again. Wrong codes count toward the host-wide lockout like any other.
5. The phone is paired as with a QR code, including the install identifier
   that replaces an earlier record of the same app.

At most four requests are open on a host and one per network address, so a
peer cannot flood the owner with prompts; a phone that reconnects with the
same install identifier replaces its own open request. Pressing *Pair a
device* while a code is on screen for a request replaces that code too.
`rustcode remote status` lists open requests. The frames and their order are
in [the protocol](remote-protocol/README.md#connection-requests).

## Refused connections

A device that does not get in is refused with one `error` frame and the
connection closes (see [the protocol](remote-protocol/README.md#the-bare-error-frame)).
The phone shows what its app makes of that code, which leaves the host's owner
looking at a phone that only says it cannot connect. So the gateway also keeps
the refusals it can help with, newest first, for fifteen minutes after each
stopped trying: eight at most, with repeats of the same attempt from the same
address and app counted rather than listed again.

| Recorded as | What happened | What the owner can do |
| --- | --- | --- |
| `unsupported_version` | The app speaks a remote protocol version this host does not. | Update RustCode on the device or on this computer so both match. The answer to the device says the same, with `supported_versions`. |
| `pairing_failed` | The pairing code or QR credential was wrong, already used, or had expired. | Show a fresh code with *Pair a device*. |
| `rate_limited` | Too many wrong codes on this host, so even the right one is refused until the lockout ends. | Wait for the lockout, then show a fresh code. |
| `unauthorized` | A pairing this host does not accept: unknown, revoked or expired. | Pair the device again. |
| `token_replayed` | A retired refresh token was presented, so the whole pairing was removed. | Pair the device again. |
| `unexpected_origin` | The WebSocket upgrade carried an `Origin` no native app sends, so it was refused before a frame arrived. | That is a RustCode app from before it sent the native app origin, or a web page trying the port: update the app. Nothing was shared either way. |

Settings › Remote access lists them under *Refused connections*, with the
device's own label when its frame named one, its kind and platform, the
address it came from, how many tries and how long ago. `rustcode remote status`
prints the same lines, and `refusedConnections` carries them to any ACP client.
Nothing that is only a bug on the device (a malformed frame, a binary frame, an
oversized one) is recorded, so it cannot crowd out the reasons a person can fix.
Secrets never reach the record: only the label, kind, platform, protocol
version and address a refused frame named are read from it.

## Machines

A computer can also be the *client*: `rustcode machines` pairs this one with
another computer's gateway and keeps it, so the same machine is one entry with
every address it answers on. This is the command-line side of the desktop app's
multi-machine client (`rustcode/client`, the `rustcode-client` crate); the app
fills in the same saved machines.

| Command | Behavior |
| --- | --- |
| `rustcode machines add <address> [--name <name>] [--code <digits>]` | Ask the machine to let this one in; the code is exactly the eight digits it shows (spaces and dashes are ignored). It shows *‹name› wants to connect* on that machine; accept it there, then type the eight-digit code it shows (or pass `--code`). Prints `Waiting for … to accept…` while the request is open. Pairs, stores the token and saves the machine. |
| `rustcode machines add --discover` | The same, choosing the machine from Bonjour when exactly one answers. |
| `rustcode machines list [--threads] [--json]` | Name, platform, the route in use and whether the machine answers now. Under a machine that is reachable one way and not another, it prints the hint: e.g. `Not reachable over NetBird (100.90.16.237:17879). On that machine, check that NetBird allows inbound connections and that an access policy allows TCP 17879 between the two machines.` The platform comes from the gateway (`macos`, `windows` or `linux`) and is refreshed whenever the machine is reached; a gateway older than that reports none, and the row keeps `unknown`. With `--threads`, each connected machine's thread titles over ACP (`session/list`), printed under the machine's row and after its hint. `--json` for scripts, with the same hint under `hint`. |
| `rustcode machines remove <name-or-id>` | Forget a machine and delete its token. It still lists this computer until it is revoked there. |
| `rustcode machines discover [--secs <n>] [--json]` | Machines on this network whose record lists `caps=desktops`: name, address and machine id. |

An address is `host`, `host:port`, `[v6]:port` or `ws://host:port`; the port
defaults to `17879`. `wss://` is refused: the gateway has no TLS listener.

**A machine token travels unencrypted on the LAN**, exactly like the phone
app's: the gateway has no TLS, so anything on that network can read a prompt,
a thread and — during pairing — the code. Use a network you trust, or put both
machines on Tailscale or NetBird, whose tunnel encrypts the traffic. Because a
*public* address has no trusted network around it, this client refuses to pair
over one, or to send a saved machine's token to one, and says so instead; that
holds until encrypted transport (#2651) lands.

- The command line pairs as a **desktop** client, because that is the pairing
  the machine's owner grants ACP access to when accepting the request. A
  `cli`-kind device reaching the same code is allowed the session protocol
  until the owner runs `rustcode remote devices allow <device>` there.
- Saved machines live in `machines.json` in the configuration directory
  (`0600` on Unix). Each is keyed by the machine's stable `host_id` — its
  gateway identifier — with an ordered list of routes (`lan`, `vpn`,
  `tailscale`, `public`), the device record, the platform and the RustCode
  version it last reported, and when it was added and last connected. The
  platform and the version are optional fields of the `paired` and
  `authenticated` frames (`docs/remote-protocol`); an older gateway sends
  neither, so the row keeps `unknown` for the platform and says nothing about
  a version rather than guessing one. A route keeps the product
  an overlay address is on (`netbird`,
  `zerotier`; a Tailscale address is its own kind and provider). Every machine is a peer; a `main` field in a file written
  by an older release is read and ignored. A change takes
  `machines.lock` next to the file and re-reads it first, so two
  `rustcode machines` runs cannot drop each other's machine.
- Tokens are kept in the operating system credential store (Keychain,
  Credential Manager, Secret Service) under `rustcode-machines`, one entry per
  machine *and profile*: the account names this configuration directory's
  install id and the machine's `host_id`, so two `RUSTCODE_CONFIG_DIR`
  profiles never overwrite each other's token. They are never written to
  `credentials.json` or `provider-accounts.json`. On a machine without a credential store, set
  `RUSTCODE_MACHINE_TOKEN_STORE=file` to keep them in
  `machines/tokens.json` (`0600` in a `0700` directory) instead; without it,
  the command says so rather than writing a credential to disk silently.
- The store is opened *before* a request is sent, off the runtime and with a
  deadline (`rustcode_client::tokens::bounded`, ten seconds). It is reached
  over the operating system's own IPC, where a keychain unlock or a Secret
  Service prompt can block for as long as nobody answers; a store that does not
  answer stops the pairing at this end, with the message naming the file
  fallback, instead of doing so after the other machine has accepted the
  request and enrolled this computer (#3325). The token write after the code is
  bounded the same way.
- Routes are raced, not walked in order. Every LAN route starts at once; after
  one second the overlay routes start too; the first connection that
  authenticates *as the machine we saved* — same `host_id`, and the device
  record the token belongs to — wins, and the rest are cancelled. The whole
  race ends after five seconds. An address that now belongs to another
  computer is refused rather than trusted. Because a connection that drops is
  re-raced, the client moves to a route that works again (a LAN after
  returning home, an overlay after leaving it) on the next attempt.
- The addresses a machine reports after authentication are learned and
  saved: a new LAN address goes before the overlay routes, a new overlay
  address after them, and an address already saved only updates its kind and
  product. Pairing once is therefore enough to reach a machine on a network it
  was not paired on.
- The app's Add sheet lists more than Bonjour: a saved machine's overlay
  address, learned from its handshake, is listed too, so a machine away from
  home is one click rather than a typed IP. The two sources are merged by host
  id, one row per machine, and an address found that way never needs typing.
  `rustcode machines discover` itself stays Bonjour only, since it is a
  one-shot browse with no saved machines behind it.
- A hint is shown when one *kind* of route answers and another does not, in
  `machines list` and in the app's machine status: an overlay address that
  fails while the LAN works names the product and what to check there (and a
  LAN that fails while the overlay works is expected away from home). The
  overlay probe is a plain TCP connect after the LAN wins — it never carries a
  token — so the diagnosis costs nothing when there is no overlay route.
- An overlay route earns no hint when this computer is not on that overlay,
  and none when another overlay route to the same machine answers (#3421). A
  machine that runs Radmin VPN as well as NetBird reports both; on a computer
  with only NetBird there is no interface in Radmin's address space, so its
  route can never answer, and telling the owner to check Radmin's firewall
  would send them to a network they are not on. Those routes are still kept
  and dialled, but only after every route that can answer.

### In the desktop app

Settings › **Machines** is the same pairing without a terminal: *This Mac* (or
*This PC*) first — the app's own RustCode server — then one row per saved
machine, then the machines found for adding. `?machines=paired|connect|offline|revoked`
in the mock shows the page without a second computer; `connect` starts with one
saved machine on an older RustCode reached over NetBird and the background
server not installed, so the add sheet, the version skew line and the install
are all visible at once.

| In the app | What it does |
| --- | --- |
| **Machines found** | The machines the Add sheet lists without an address typed, filled in when the page opens (*Look again* to repeat): Bonjour, the same browse as `machines discover`, for every machine whose record lists `caps=desktops`; and the *saved* machines whose handshake reported a NetBird, Tailscale or ZeroTier address, which no browse on the LAN can hear. One machine found both ways is one row, merged by its host id. Each row names the machine, its platform, the RustCode version it runs, the network its address is on (`LAN`, `NetBird`, `Tailscale`, `VPN`, `Public`) and the address. A machine not yet paired has *Ask to connect*; one already saved is *Saved*. When the local browse itself fails (a container or a network without multicast), the rows from the saved machines are still listed and the sheet says *Could not look for machines on this network* with the reason. |
| **Machines found › Enter an address** | The `host:port` a machine is reached on when it is not announced (an overlay, or another subnet). The fallback, never the first step. |
| **Waiting for ‹host› to accept on that machine…** | The request is on that machine's screen; *Cancel* drops it. Once its owner accepts, the eight digits it shows are typed here, one box per digit. |
| **Checking the code** | The code is being checked with that machine, which answers within ten seconds. After half a minute the page stops waiting and says so — *‹host› did not answer* — with **Ask again** and **Cancel**: a machine that accepted the code but could not store the token still lists this computer as a device, so it is removed there and the pairing started again. No stage of the flow is a spinner without a way out. |
| A machine row | Its name and platform, the state chip at the row's top right — *Connected*, *Connecting*, *Reconnecting…* (a machine that is away while the app retries it on its own), *Offline*, *Needs pairing* — the route **in use** under the name as `LAN · Unencrypted` or `NetBird`/`Tailscale`/`VPN · Encrypted`, then when it was last reached and how many threads it has. **Remove** sits in a row of its own below. Under the route, the hint for one kind of route that answers while another does not (see [Machines](#machines)), and a version skew line when that machine runs another RustCode: *RustCode 0.96.0 on that machine — update it there: Settings › About › Check for updates, or `brew upgrade rustcode`* (the platform's own way, from [install](install.md#updating); a development build never shows the line), or *update RustCode here* when that machine is newer. |
| **This computer · RustCode server** | Whether the RustCode server is installed as a background service: *Server: Installed · Running*, or *Not installed* with **Install server**. Installed, that engine stays up when the app closes and starts when you log in; the threads this app starts still run in its own engine and end with it. What `rustcode server status` reports, including a running version that differs from the app's, is on the same line. |
| **Remove** | Forgets it here: token, routes and its threads' rows go. The machine keeps listing this computer until it is revoked there. |
| **Pair again** | On a machine whose pairing it ended (*Needs pairing*): asks it again, so a new code can be typed. |

The app keeps one connection per machine, retried with jittered backoff from a
second up to a minute (reset after half a minute connected), so a machine that
is asleep or away comes back on its own. Each attempt tries every known route —
the LAN, the `.local` name Bonjour gave it, every overlay address — starting
with the route the machine answered on last time, so a dropped connection comes
back the way it came. A change to this computer's own network (a VPN coming up
or going away, another Wi-Fi) ends the wait at once instead of after the
backoff, and while it waits the row's chip says *Reconnecting…* next to when
the machine was last reached, rather than *Offline*, which reads as something
the user has to fix. The first read of the machine list at app start is tried
again on a doubling wait too (`state/machines.ts` `startMachines`, 1 s to
30 s), so a shell that was still starting up does not leave every machine
without a connection until a page is opened. A pairing that was refused is not retried:
nothing changes until it is paired again, and the row says so.
The connection and the token live in the app's Rust shell
(`apps/desktop/src-tauri/src/machines.rs`); the page only shows what it is
told, and the thread count comes from the machine's own `session/list` over
ACP. Showing and driving those threads in the sidebar is the next step
(#3202).

## Revoking and stopping

- Settings › Remote access › *Revoke* on a paired device does the same as
  `rustcode remote revoke` below.

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
for every device at once.

### Started in the app

While the RustCode app is running with Remote access on, the session a device
starts is created **in the app**: the thread appears in the app's sidebar
within a second and the phone is attached to it. It runs on the harness the
device names — Claude Code, Codex, Pi, OpenCode or any other external harness,
or the built-in one — and on the model it names, exactly as a thread started in
the app. `create_session` takes optional `harness` and `model`; a device offers
that choice only when the gateway advertises `thread_create_options`, which it
does while the app shares at least one thread. An unknown harness, a harness
that is turned off, or a model the harness does not list is refused with
`invalid_request` naming the value, and nothing is started.

A thread started this way is the app's, like any other: it is shared while
Remote access is on, it ends when the app closes it, and it is `open` in the
app until then, so reopening it from a device or with `rustcode --resume` is
refused with `open_elsewhere`. The app runs it, so it follows the app's
harness settings, accounts and models; a device cannot name an account, an
effort or a worktree.

### Started as a session owner

With no such share live — Remote access off, an app with no thread open, or a
terminal's engine, which never hosts threads — no terminal is involved either: the gateway starts a **session owner**, a
`rustcode` process without a terminal that runs in that directory, owns the
session and registers it like `/remote` does. It keeps running after the
device disconnects and across gateway restarts, and ends when it is killed or
the machine restarts. It always runs on the built-in harness, ignores
`harness` and `model`, and its session appears in the Mac app and the
terminal's session lists after its first exchange.

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

### Reviewing a session's changes

From a shared session, More ▸ Review Changes on the phone shows the session's
uncommitted changes: how many files and lines changed, when they were read,
the branch and commit, each changed file, and each file's diff. Tap lines to
select them, then **Ask About … Lines** to quote them, with their numbers,
in the session's message field. Nothing is sent until you send it, and the
draft stays through a disconnect. Reading the changes runs no Git command
that writes. Large diffs are cut and say so. The gateway advertises this with
the `session_changes` capability.

To continue a device-started session in a terminal, run
`/adopt <session-id>` there: it asks the detached owner to exit (gracefully,
saving history first), waits for its lease to release, then opens the session
exactly like a resume. The terminal shares it again by itself; devices
re-attach with a fresh epoch. `/adopt` needs the full session id, never runs
from a device, and refuses when the owner cannot be identified safely.

## Reachable with the app closed

The gateway is its own process, and whoever starts it is only its launcher.
With the RustCode server installed (`docs/server.md`), the **server** starts
the gateway whenever remote access is on and starts it again if it stops, so
quitting the app leaves port 17879 answering and this machine's server threads
reachable from the other machines. Only one gateway runs at a time: the app
uses the running one instead of binding its own, and the server adopts the
app's, so the port never has two owners (`rustcode server status` › `Gateway`
says which one runs and who started it). What the server starts it with —
bind, port, advertised address, interface preference — is the settings the
last gateway recorded in `remote/settings.json`, which only the gateway that
won the owner lock writes, or `[remote]` in `config.toml` on a machine that has
not run one since that file existed.

Two honest limits: the threads *this app* starts run in the app's own engine
and still end with it, because they are not the server's threads; and a
gateway the app or a terminal started keeps running only as long as that
process tree does, until the server restarts it.

On macOS an installed `rustcode` runs the gateway as a login agent when no
server is installed. The agent keeps the search path it was installed with, so
asking for the gateway from a terminal with another environment no longer
rewrites the agent and restarts the gateway under its devices. Installing the
RustCode server removes the agent: the server supervises the gateway instead,
and two supervisors would race for the same port. Only the process launchd was
running for the agent is replaced — its pid is read before the job is booted
out — so a plist left behind after the job was unloaded never stops a gateway
the app or a terminal started, which the server adopts as usual. `rustcode remote stop`
removes the agent; the next start installs it with the current environment.

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
- **Away from home:** join the same NetBird, Tailscale or ZeroTier network on
  both machines. With an `auto` bind the gateway already listens on the
  overlay address (see [Automatic address selection](#automatic-address-selection)),
  and a paired client learns it from `routes` and tries it after the LAN
  (see [Machines](#machines)). Nothing has to be typed. If the overlay still
  does not answer, `rustcode machines list` says what to check on that machine:
  NetBird's **Block inbound connections**, a NetBird access policy that allows
  TCP `17879` between the two machines, Tailscale's ACLs or grants for that
  port, or ZeroTier's flow rules.
- **A fixed setup:** `rustcode remote serve --bind <netbird ip> --advertise <host>.netbird.cloud`
  (or the same two values as `[remote] bind` and `advertise`) pins the mesh
  address and a name that never changes. On Linux `--prefer-interface netbird`
  makes the mesh address the advertised one while LAN and overlay listeners
  stay open.
- **Bonjour** announces the service name, port, protocol version,
  gateway identifier and optional handshakes (`caps=connect_requests,desktops`), but
  no address on purpose: the phone resolves the
  announcement itself on the network it is on, so the record cannot go stale
  after a DHCP change and never points at an interface the phone cannot
  reach. A paired phone that finds its gateway identifier at a new address
  reconnects there with its token.

### Bonjour

The gateway announces itself with its own mDNS responder (the `mdns-sd`
crate) on macOS, Linux and Windows alike; it needs neither `dns-sd` nor Avahi
installed. The record:

| Field | Value |
| --- | --- |
| Service type | `_rustcode._tcp` in `local.` |
| Instance name | the machine's name (the computer name on macOS and Windows, the host name on Linux), else `RustCode` |
| Host | `rustcode-<gateway id prefix>.local.`, RustCode's own name, so it never competes with the name the system's responder already answers for |
| Port | the gateway's port |
| TXT | `protocol=1`, `gateway=<gateway id>`, `caps=connect_requests,desktops` |

Each network interface answers with its own addresses, and the record follows
address changes. A gateway that listens on loopback only is not announced:
no phone could reach it. Stopping the gateway (`remote stop`, Ctrl-C,
SIGTERM, or the console closing on Windows) withdraws the record with a
goodbye, so phones drop the machine from their nearby list at once; a gateway
that is killed outright disappears when the record's lifetime runs out.

To see the announcement from another machine: `dns-sd -B _rustcode._tcp` on
macOS, `avahi-browse -r _rustcode._tcp` on Linux. Windows has no built-in
browser for it; the phone app's nearby list is the check there. Windows
Firewall must allow `rustcode` on private networks, for UDP 5353 (mDNS) and
the gateway's TCP port; Windows asks the first time the gateway listens.

## Security posture

- **Transport:** no TLS. On a plain LAN anyone on the network can read and
  alter the traffic. Use it only on a network you trust; NetBird encrypts the
  traffic end to end and is the recommended path off the home LAN. Never
  forward the port to the internet. The app shows an unencrypted-LAN banner
  for a plain address.
- **Pairing:** a 2-minute, single-use challenge with five wrong codes; ten
  failures in five minutes lock pairing for the host. Only the local user can
  ask for a challenge (control socket `0600`, or on Windows the endpoint's
  token; see [Local sockets](#how-it-holds-together)).
- **Access is per host.** A paired device can read and drive every session
  this machine shares, now and later, for 30 days or until revoked. There are
  no per-session grants and no per-session opt-out. Per-session grants have
  been considered; they are not planned for now.
- **Ending access:** `rustcode remote revoke <device>` closes that device's
  connections in the same step and forgets its token; `/remote off` or
  `rustcode remote stop` ends access for every device (see
  [Revoking and stopping](#revoking-and-stopping)).
- **Browsers** cannot reach the gateway: an upgrade request with an `Origin`
  header is refused, except the exact `rustcode-mobile://app` a native phone
  app sends, which no browser can produce.

## How it holds together

**Settings.** Every gateway writes what it listens on and announces to
`remote/settings.json` as it starts: `bind` (`auto` or an address), `port`,
`advertise`, `prefer_interface` and who started it. The RustCode server reads
it to start the gateway at login with no app or terminal around. The address
an `auto` bind resolved to is never stored; it belongs to the machine of the
moment.

**Local sockets.** The gateway has two Unix sockets in `remote/` under the
configuration directory (`0700`), each `0600`: `control.sock` for the
`rustcode remote …` commands and `owner.sock` for terminals that share a
session. Only the user who runs the gateway can connect; the owner socket also
checks the peer's user ID. If the configuration path is too long for a socket
address, both sockets go to `/tmp/rustcode-<uid>/<hash>/` instead, created
`0700` and verified to belong to you.

On Windows, where Tokio has no Unix sockets, each endpoint listens on
`127.0.0.1` at a port the system picks, and `remote/control.endpoint` and
`remote/owner.endpoint` hold that port and a random 256-bit token. Those files
are in your profile, whose permissions admit only you (and the system and
administrators), like the device store. A client sends the token before
anything else; a connection that sends anything else, or nothing within five
seconds, is closed unanswered. The token is new every time the gateway
starts.

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
| Answer to a `create_session` the desktop app starts | 30 s; a harness that needs longer gets `owner_unavailable` with receipt `unknown`, and `get_request_status` reports `received` until the thread exists |
| Wait for a new session's owner to register | 60 s, then the owner is stopped and its receipt withdrawn |
| Interval at which the listen address is resolved again | 5 s |
| Replay kept per session | 4 MiB of events |
| Mutation receipts per registration | 1024; more mutations are refused with `receipt_capacity` |
| Silence before a device connection is closed | 90 s; any frame or WebSocket ping from the device counts (the gateway pings every 30 s) |

An upgrade request with an `Origin` header is refused with 403, unless it is
exactly `rustcode-mobile://app`: browsers send an `http(s)` origin and a page
cannot set headers on a WebSocket, so a web page cannot reach the gateway. The
native app sends that value because React Native always adds an `Origin`
header; see [the connection step](remote-protocol/README.md#connection).

## Windows

The gateway, pairing, connection requests, Bonjour and Settings › Remote
access work as on the other systems. What differs:

- **No gateway login service of its own.** Remote access started from the app,
  `/remote` or `rustcode remote start` runs the gateway as a detached
  background process (no console window, its own process group); macOS also
  installs a LaunchAgent for it when no RustCode server is installed. With the
  server installed, the server's own logon task hosts the gateway, so the
  machine stays reachable the same way as on the other systems. Without a
  server, the gateway stops when you sign out or restart, and the next
  RustCode start brings it back while Remote access is on.
- **Local endpoints** are loopback ports with a token instead of Unix sockets
  (see [How it holds together](#how-it-holds-together)).
- **`/adopt`** of a thread a device started is refused: Windows cannot ask
  its session owner to exit gracefully. End it on the device, then resume it
  from `/history`.
- **Firewall:** allow `rustcode` on private networks when Windows asks, or a
  phone can neither find nor reach the gateway.

## Not covered yet

- No TLS; see the warning at the top.
- The terminal owns execution. Closing it ends the session's availability. A
  session started or reopened from a device is owned by its session owner
  process instead.
- A device starts sessions only in folders inside the home folder or a
  configured `[remote] roots` entry, and cannot create folders.
- A session the gateway started as a session owner shows up in the Mac app
  and the terminal's session lists only after its first exchange; one the app
  started is in the app's sidebar at once.
- Opening a thread the app started for a device from the app itself, while
  the app still runs it, answers "session is already active": a live session
  cannot be opened twice. Close the thread in the app and open it again.
- No push notifications.
- `rustcode machines` reaches a machine over its WebSocket only: no SSH route,
  no relay, and a route list is not reordered or learned yet (steps 1.8 and
  1.11 of the machine network plan).
- Access is per host: a paired device sees every session open in a terminal.
  There are no per-session grants and no per-session opt-out.
- Reachability from a real iPhone (local-network permission, NetBird routing)
  is verified with the app, not by this repository's tests.

### Desktop ACP channel

After authentication, a device with the host-controlled `acp` grant can send
`{"type":"acp_open"}`. The gateway opens ACP on the engine running in its
configuration directory, taking the first that accepts a connection: an engine
the app started (`run/engine.json`, which holds the threads the app is showing)
or a shared server (`run/server.json`). A description left behind by an engine
that has since stopped is skipped. It answers `{"type":"acp_opened"}`. Subsequent
WebSocket text frames each contain one ACP
JSON-RPC message, in either direction. These additive frames keep protocol
version 1. The grant defaults to false, including for existing device records.
Self-reported device kind never grants access. Accepting a desktop's ask-to-connect
request grants ACP after it enters the request-bound code; the approval says
"Can see and run all threads on this machine". Pairing with a standalone short
code never grants ACP. The owner can run `rustcode remote devices allow <id>`
or `rustcode remote devices deny <id>`; deny immediately disconnects the device.
Devices without the grant receive a gateway
`error` with code `unauthorized_access` when opening ACP. When no engine is
running, or none of the described ones answers, the connection is closed with
`internal` and the gateway writes why to its log.

Client requests and notifications are restricted to these exact methods:

- `initialize`
- `_rustcode/harnesses`

The only allowed prefixes are `session/` and `_rustcode/pulls`. Everything else is denied, including
provider credentials and login, pairing and remote management, `_rustcode/server/*`,
MCP server configuration and data-store SQL. Denied requests receive a JSON-RPC
error with the same `id`, numeric code `-32601` and
`data: {"code":"unauthorized_access"}`. Denied notifications are dropped.
JSON-RPC responses pass through only for an outstanding server request ID,
once per request. Unknown or repeated answers are dropped. At most 1,024 server
requests may be outstanding; exceeding this closes with `slow_consumer`.
Batch messages are refused.

`session/load` resume metadata (`_meta.rustcode.resume: {cursor}`) passes through
without modification. The gateway translates WebSocket messages to the local
server's newline-delimited JSON; JSON is compacted without changing JSON string contents or resume metadata. Local-server frames retain the gateway's 1 MiB
frame limit. ACP has its own outbound queue capped at 1,024 frames and 16 MiB, including
the frame being written. Overflow
closes the device with `slow_consumer`; it never blocks other gateway clients.
Revocation and gateway shutdown still close ACP connections, and closing either
endpoint closes the other.
