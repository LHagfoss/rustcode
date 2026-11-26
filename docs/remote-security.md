# Remote access: threat model and transport security

What protects a paired device's connection to a RustCode gateway today, what
does not, and what a future encrypted transport has to beat. The behavior of
remote access itself is in [remote.md](remote.md); the wire format is in
[remote-protocol](remote-protocol/README.md). This page is the security view
of both, for issue #2651.

> **A plain LAN WebSocket is authenticated but not encrypted.** The gateway
> port also answers its own TLS with a self-signed certificate that clients
> pin ([Direct TLS](#direct-tls-with-a-pinned-certificate)), and other
> computers use it on every route. The phone app does not yet, so its
> traffic on a plain LAN can be read (prompts, session output, file contents
> the agent prints) and, because nothing is signed per frame, altered by
> anyone on the same network. A tunnel such as NetBird, Tailscale or ZeroTier
> encrypts what it carries. Every place the apps show a route says which it is.

## What is protected, and from whom

Assets, in the order they matter:

1. **Control of the computer.** A paired device with the `acp` grant (every
   desktop that was accepted, see [Desktop ACP channel](remote.md#desktop-acp-channel))
   can start threads and run tools on this computer. A phone can read and
   drive every shared thread. Either is, in effect, a shell as the user.
2. **Thread content.** Prompts, answers, tool output and diffs, which may hold
   source code and secrets.
3. **The device token.** Whoever holds it is that device until it is revoked.
4. **The pairing code or QR credential,** for the two minutes it is valid.

Attackers considered:

| Attacker | What they can do today | Why |
| --- | --- | --- |
| Another device on the same Wi-Fi or LAN, passive | Read all traffic on a plain route, including the token of a device that reconnects with a static or access token | No TLS; see [Encryption per route](#encryption-per-route) |
| Same, active (ARP spoofing, rogue access point) | Alter or inject frames on a plain route; take over a connection after its handshake; answer on a stale address and collect whatever a device sends first | No per-frame integrity; the gateway proves its identity only by returning its `gateway_id` over the same unauthenticated channel |
| Same, trying to pair | Guess the eight-digit code | Five wrong codes per challenge and ten per five minutes for the whole host lock pairing; a challenge lasts two minutes and is single use (`docs/remote.md` › Pairing) |
| A web page in the user's browser | Nothing: it cannot reach the gateway | Upgrade requests with an `Origin` other than `rustcode-mobile://app` are refused with 403 (`rustcode/engine/src/remote_gateway/transport.rs`) |
| Another user on the same computer | Nothing through the gateway | Control and owner sockets are `0600` in a `0700` directory; the device store is `0600`; on Windows the local endpoints need a token (`docs/remote.md` › How it holds together) |
| An attacker on the internet | Reach the port only if the owner forwarded it, which the docs forbid | The gateway binds a LAN or overlay address, never `0.0.0.0` by default |
| A stolen or lost device | Everything that device could do, until revoked | Revocation closes its connections at once; a device that rotates tokens can also lose a copied token to replay detection |
| A compromised overlay network account | Join the mesh and then act as any other peer on it | The mesh gives the attacker the same standing as a LAN peer: the pairing and token still stand in the way |

Out of scope: a compromised paired device that is still paired, a compromised
computer, malware that reads the keychain as the user, and denial of service
by a peer on the network (the limits in `docs/remote.md` › Limits only bound
it).

## Authentication

Authentication is two-way in intent and one-way in proof:

- **Device to host: strong.** A device proves itself with a secret only the
  host and that device hold: the pairing code or QR credential once, then a
  token. The host stores SHA-256 digests only (`remote/devices.json`,
  `rustcode/engine/src/remote_gateway/devices.rs`). Tokens are random;
  digests are compared in constant time.
- **Host to device: weak.** Before it sends a token, a client asks who answers
  (`describe`, answered with the host's `gateway_id` and name) and drops a route
  whose gateway is another machine (`docs/remote.md` › Machines › *Identity
  before credentials*; `rustcode/client/src/machine.rs`). The `gateway_id` is
  not a secret and is not signed: it stops an honest mix-up (a recycled DHCP
  address) and not an active attacker on a plain route who forwards the
  handshake. With a tunnel, the tunnel's own peer authentication is what
  stops that attacker. A computer that pinned the gateway's certificate has a
  RustCode-owned host proof: the TLS handshake proves the peer holds the key
  of the certificate it pinned, and a client never dials a pinned machine in
  the clear ([Direct TLS](#direct-tls-with-a-pinned-certificate)).
- **Pairing** is bound to the physical presence of the owner: only the local
  user can ask for a challenge (the control socket), and a *connect request*
  needs the owner to press Accept in the app and then to read the code to the
  device on a channel the network does not see (screen to screen).
- **Tokens.** A static token lasts 30 days. A device that asks for
  `token_rotation` holds a refresh token that every reconnect replaces, plus a
  15-minute access token; presenting a retired refresh token revokes the whole
  pairing (`token_replayed`). Rotation shortens how long a token lifted off a
  plain network stays useful, but does not stop its use within that window.
- **Device keys.** A gateway that lists `key_auth` gives every socket a fresh
  challenge, and a device that registered an Ed25519 public key signs it
  instead of sending a token (`docs/remote-protocol/README.md` › Device
  keys). A signature is bound to the gateway, the socket's challenge and the
  device, and is spent on first use, so one read at a tunnel's edge or off a
  plain network cannot be used again. Registering the key drops the record's
  token digests, and a token for that device is refused with `key_required`.
  The computer's client registers its key on its next sign-in to a machine
  that offers it.

## Encryption per route

Encryption belongs to the route, or to the gateway's own TLS when the device
dials it. The engine classifies each address it listens on
(`rustcode/engine/src/remote_gateway/address.rs`: `route_kind_for`,
`ConnectionRoute`) and reports it; the apps label it.

| Route | Reported as | Encrypted? | Notes |
| --- | --- | --- | --- |
| LAN WebSocket (Wi-Fi, Ethernet; a private IPv4 address) | `lan` | **No** | Plain WebSocket. Trust only a network where every device is yours. Bonjour discovery announces the service, never credentials. |
| NetBird, Tailscale, ZeroTier (an address on a tunnel interface) | `overlay`, with `provider` when the interface name says it | **Yes, by the tunnel** | WireGuard (NetBird, Tailscale) or ZeroTier's own encryption protects the hop between the two machines. The tunnel authenticates each peer. RustCode does not check that a tunnel is up; it classifies by the interface carrying the address. |
| A generic VPN whose interface is named `utunN`, `tun*`, `wg*` | `overlay`, no provider | **Yes, assumed** | The interface being a tunnel is all the engine can see. A VPN that does not encrypt (a plain GRE or IP-in-IP tunnel) would be mislabelled; none of the supported products behave so. |
| `100.64.0.0/10` on Wi-Fi or Ethernet | not a route | n/a | A carrier's CGNAT address, never advertised, never bound, never labelled encrypted. |
| A host name (the machine's `.local` name, or `--advertise <name>`) | `hostname` | **Unknown** | The name resolves to whichever address the client picks. The *connection* is classified by the address it actually reached. |
| A public address or name on plain `ws://` | `public` | **No** | The client refuses to send a pairing code or token to one (`rustcode/client/src/registry.rs`: `may_send_secrets`). |
| The machine's public address, `wss://<host>` (a tunnel such as `cloudflared` to the engine's HTTP listener, `/ws`) | `public` on the client; the connection is `tunnel` | **Yes, to the tunnel's edge** | The client verifies the certificate against the operating system's trust store and refuses one it cannot verify. TLS ends at the tunnel provider, **which can read the traffic**; from there it is plain loopback to the engine, which relays it to the gateway. See [The public address](#the-public-address). |
| Loopback (a Simulator or a client on this computer) | `loopback` | n/a | Never leaves the machine. |
| The gateway's own TLS, on any of the above (a computer that pinned the certificate) | the network's path, with `tls: true` | **Yes, by RustCode** | Self-signed, pinned by its SHA-256 fingerprint. A handshake that fails (another certificate, a device dialling `wss://` with the trust store) is recorded as `secure_connection` and closes the socket before any frame. Nothing is shared. See [Direct TLS](#direct-tls-with-a-pinned-certificate). |

Per connection, the gateway classifies the local address the device dialled and
reports its `path` (`loopback`, `lan`, `overlay`, `tunnel` or `other`), the
overlay `provider`, the `localAddress` and whether the network `encrypted` it.
An address on no interface is `other` and unencrypted: the engine never
assumes a tunnel. `encrypted` is true only for `overlay` and for `tunnel`, a
connection relayed from the public address (it arrives on the gateway's
loopback ingress, which only the engine's `/ws` relay uses).

### The public address

With `[outside] public_url` set and a tunnel forwarding that host to the
engine's HTTP listener (`docs/mcp-outside-agents.md` › Hosted agents), the
same origin serves `/mcp` for hosted agents and `/ws` for paired phones and
machines (`wss://rustcode.example.com`).

- **Who can read it:** the tunnel provider. TLS ends at its edge, and the
  hop from `cloudflared` to the engine is plain loopback. Use it when you
  accept that provider seeing prompts, threads and device tokens in transit;
  a VPN overlay keeps them end to end.
- **Host and Origin:** `/ws` is answered only for this computer's loopback
  address and the public host, like `/mcp`. The gateway's browser rule applies
  to the relayed upgrade unchanged: an `Origin` other than the native app's is
  refused, so a page on the public host cannot open the gateway.
- **Audiences stay apart:** `/ws` speaks the remote protocol and accepts only
  device and machine credentials; an MCP token means nothing there, and a
  device token means nothing on `/mcp`.
- **Limits:** every relayed connection reaches the gateway from loopback, so
  they all share one per-address budget for unauthenticated sockets. Pairing
  attempts are limited for the whole host, whichever route they come over.
- **No gateway running:** the engine answers `/ws` with HTTP 503
  (`remote_access_off`) instead of leaving the device waiting.
- **Replay:** device and machine tokens are still bearer tokens, so one seen
  at the tunnel's edge could be replayed from anywhere. Key-pair
  authentication (#3956) removes that.

Where this shows:

- Settings › Remote access › Paired devices: a chip per connected device,
  `LAN · Unencrypted`, `NetBird · Encrypted`, `Tailscale · Encrypted`,
  `VPN · Encrypted`, `This computer`.
- Settings › Machines: a machine this computer connects to shows
  `LAN · Unencrypted` or `NetBird · Encrypted` for the route in use
  (`apps/desktop/src/state/machines.ts`: `routeLabel`). A client cannot see
  which interface it used, so it judges the route it dialled by its kind
  (`rustcode/client/src/registry.rs`: `RouteKind::is_encrypted`): only a
  tunnel is encrypted, and a public or unknown address is not. The gateway's
  own answer above is the authority for a device that connects *to* this
  computer.
- `rc machines list`: a NETWORK column for each connected machine,
  in the same words as `rc remote status` (`LAN (not encrypted)`,
  `NetBird (encrypted by the tunnel)`, `an unrecognised network (not
  encrypted)`), `—` for a machine that is not connected. `--json` carries
  `network` with `path`, `provider`, `encrypted` and `label` (null when the
  machine is not connected). The route is mapped onto the gateway's
  `ConnectionRoute` in `rustcode/cli/src/machines.rs`, so the label and the
  rule come from `address.rs`.
- Route change: when this computer reconnects to a machine over another kind
  of network than before (a tunnel at the office, the LAN at home), the app
  shows a short notice once, `Studio: Now on LAN · Unencrypted` (an error tone when it moved off a tunnel onto a network that
  does not encrypt, plain otherwise). The shell records it per machine as
  `routeChange` (`apps/desktop/src-tauri/src/machines.rs`: `record_route`);
  the window tells each change once and records where a machine stands the
  first time it lists it, so a change from before the window opened is not
  news. A new address on the same kind of network is not a change.
- `rc remote status`: each connected device says `connected over LAN (not
  encrypted)` or `connected over NetBird (encrypted by the tunnel)`.
- ACP: `_rustcode/remote` carries `routes` on each connected device. A
  device that was revoked is not listed, so it is never shown as connected,
  and a route reported `encrypted` on any path but `overlay` is sent as
  unencrypted (`rustcode/engine/src/acp/remote.rs`: `devices_json`).

Downgrade: a computer that pinned a machine's certificate dials every route
to it but a `wss://` public address with TLS pinned to it, and a dial that
fails is never retried in the clear (`rustcode/client/src/machine.rs`:
`open_described`). An address that answers plain WebSocket, or TLS with
another certificate, is refused with "nothing was sent". A pin learned from a
plain `described` frame is saved only after the sign-in over TLS succeeds.
The other refusals are protocol-version mismatch (`unsupported_version`, both
directions, `rustcode/engine/src/remote_gateway/handshake.rs`) and the
public-address rule above. The desktop never reports a pinned machine's move
between networks as a downgrade, because none loses encryption.

## Credential storage and revocation

| Where | What | Protection |
| --- | --- | --- |
| Host: `remote/devices.json` | SHA-256 digest of each device token, name, kind, platform, pairing and last-seen times | `0600` file in a `0700` directory, replaced atomically. Never a token. |
| Host: `remote/revocations.json` | Revoked device families that could not be removed from the store yet | Same permissions; the gateway refuses to start if it cannot read it rather than readmit a revoked family |
| Host: control and owner sockets | Who may ask for a pairing challenge or share a session | `0600` Unix sockets in a `0700` directory; on Windows, loopback ports with a per-start 256-bit token |
| Computer as a client: machine tokens | The token this computer holds for each machine it connects to | The operating system credential store (Keychain, Credential Manager, Secret Service) under `rustcode-machines`; `machines/tokens.json` (`0600`) only when `RUSTCODE_MACHINE_TOKEN_STORE=file` is set |
| Computer as a client: device key | This install's Ed25519 private key | The same credential store as the machine tokens, under the slot `device-key`; never sent, never logged. Machines store only its public key. |
| Computer as a client: `machines.json` | Routes, host ids and device ids, no secrets | `0600` |
| Phone | The phone's token and refresh token | Held by the mobile app (`rustcode-mobile`); not verified by this repository |

Credentials never go to `credentials.json` or `provider-accounts.json`, never to
logs (the gateway logs device names and ids), and not to Bonjour (service name,
port, protocol version, gateway id and capabilities only).

Revocation, from the quickest to the widest:

1. Settings › Remote access › *Revoke*, or `rc remote revoke <device>`:
   the device's connections close in the same step with `revoked`, and its
   token is forgotten.
2. Remove on the device's side (Settings › Machines › *Remove*, or
   `rc machines remove`): the device sends `unpair` and the host revokes the
   record it signed in as, as in 1. A host that cannot be reached keeps the
   record, and the device says so.
3. Revoke everywhere (Settings › Machines › ‹machine› › *Revoke everywhere*,
   or `rc machines revoke-everywhere`): for a lost computer, every machine
   this one reaches revokes the device records whose host id is that
   computer's (`_rustcode/remote/revoke_machine`, on the paired-desktop
   relay allowlist; a paired desktop could already run commands there, so it
   adds no power). Offline machines are asked when they next connect.
4. `rc remote devices deny <id>`: removes the ACP grant and disconnects.
5. `/remote off` or `rc remote stop`: nobody reaches the gateway;
   pairings are kept.
6. Replay of a retired refresh token: the whole pairing is revoked by the host.
7. Expiry: a static token ends after 30 days; a rotating pairing lasts until
   revoked.

A revoked device is refused with `unauthorized` or `revoked` when it returns,
and the refusal is listed under *Refused connections* for a quarter of an hour.

## Direct TLS with a pinned certificate

Built as recommended below:

- **The certificate.** The gateway makes a self-signed P-256 certificate the
  first time it starts and keeps it with its key in `remote/tls.json` (`0600`,
  like `gateway.json`; `rustcode/engine/src/remote_gateway/tls.rs`). Its
  validity runs to the year 4096: nothing renews it, and a wrong clock on a
  device cannot fail it. A file that exists but cannot be read stops the
  gateway instead of replacing the certificate every device pinned.
- **One port.** The first byte of a connection says which it is: a TLS record
  (`0x16`) is a TLS handshake with that certificate, anything else is the
  plain WebSocket upgrade as before.
- **The pin.** `described` frames and long connect codes carry
  `tls_fingerprint`, the SHA-256 of the certificate's DER. A long connect code
  vouches for it out of band (it travels from the other computer's screen); a
  short code or a connection request learns it from the machine's first
  plain `described` frame, which is trust on first use: an active attacker on
  the LAN at that moment could pin their own certificate, and the machine
  would then refuse the real one until it is paired again. The client then
  dials again over TLS pinned to it before the request, code or token goes
  out, and saves the pin with the machine (`machines.json`,
  `tls_fingerprint`). Only pairing again replaces it.
- **Failures.** A changed certificate (RustCode reinstalled, `tls.json`
  deleted) is refused on every route until the machine is paired again; the
  host records the failed handshake as `secure_connection`.
- **The phone app** (`rustcode-mobile`) does not pin yet: the QR payload has no
  fingerprint, and whether React Native's WebSocket accepts a pinned
  certificate without a native module is still to be checked there.

## Direct TLS and the existing VPN options

This compares what the issue asks about.

| | Plain LAN | VPN overlay (NetBird, Tailscale, ZeroTier) | Direct TLS on the gateway (computers only) |
| --- | --- | --- | --- |
| Encrypts traffic | No | Yes, by the tunnel | Yes, by RustCode |
| Host proof | Weak (`gateway_id` only) | Peer key of the tunnel | Certificate, if the client pins or validates it |
| Works away from home | No | Yes | Only with a reachable address: a port forward, a tunnel, or a relay |
| Setup per computer | None | Install and sign in to the overlay on both machines, allow TCP `17879` in its policy | Create or obtain a certificate, give the gateway the private key, hand the client the trust anchor |
| Certificate cost | None | None (the product manages keys) | See below |
| External service | None | The overlay's account and coordination service, chosen and signed in by the user | None for a pinned or private-CA certificate; a public CA (ACME) contacts one |
| New moving parts in RustCode | None | None | TLS listener, certificate storage and renewal, pinning in two clients, a version bump |
| Failure modes | Spoofing on the LAN | Overlay down, ACL blocks the port, device not signed in | Expired or changed certificate, clock skew, a client that does not trust the anchor |

Certificate options for direct TLS, each with its operational cost:

- **A self-signed certificate, pinned at pairing.** The QR code (or connect
  code) carries the certificate's fingerprint; the client accepts only that
  certificate. No CA, no renewal pressure, no external service, and it is
  proof of the host the first time because the QR code travels screen to
  camera. The cost: a certificate change (a reinstall, a rotated key) breaks
  every pairing until it is paired again, and each client must implement
  pinning. Whether the phone app's WebSocket stack accepts a pinned
  certificate without a native module is not verified here and must be
  checked in `rustcode-mobile` before this is chosen.
- **A private CA the owner creates and installs on each device.** Renews
  centrally, but putting a root certificate on a phone is heavy for the user
  and widens what that root can vouch for.
- **A public CA certificate (ACME).** Needs a public DNS name for the
  machine and an issuance flow that contacts the CA, and a machine behind a
  home router has no routable name unless the user also runs a tunnel or
  dynamic DNS. A public CA does not vouch for a private address, so this
  does not cover the LAN, which is the case that is unencrypted today.

Decision: direct TLS is a pinned self-signed certificate delivered in the
pairing payload, because that is the only option that covers the LAN with no
external service and no CA. It opens no new port and contacts nothing, so
it needs no consent of its own. The overlay stays the recommendation for the
phone app until it pins too.

**External services.** RustCode installs, starts and configures no overlay
network and contacts none: the user installs NetBird, Tailscale or ZeroTier,
signs in to its account and writes its policy. A hosted relay is a separate
decision (#3199); it must be opt-in, name the operator and what it can see,
and stay off by default.

## Migration

Today's behavior is the baseline everything below must keep working:

1. **Old client, new host.** The host keeps answering plain `ws://` on
   protocol version 1. A device on an older protocol is refused as
   `unsupported_version` with the versions the host supports, and the owner
   sees it under *Refused connections*.
2. **New client, old host.** A client with no pin asks `describe` in the
   clear; an old host names no fingerprint, so the connection stays plain as
   before. A host that has been pinned is never dialled over plain `ws://`
   again: that is the downgrade the issue asks to refuse, and pairing again
   is the owner's way to choose otherwise.
3. **Pairings.** A pairing made over plain `ws://` stays valid; the first
   successful TLS connection pins the certificate fingerprint to it. Nothing
   is re-paired for the upgrade.
4. **QR and connect codes** gain a fingerprint field, additively. A client
   that does not read it ignores it; a client that does verifies the
   certificate against it.
5. **Device keys.** The engine takes both a signature and a token today: a
   device that has registered a key can only sign, and every other device
   keeps its token. The computer's client moves itself over on its next
   sign-in. The phone app (`rustcode-mobile`) moves when it ships key
   support; only after that does the gateway stop accepting tokens from
   devices without a key, and until then a phone's token is as exposed on the
   public address as it was before.
6. **Version.** Adding the TLS listener is a minor protocol change if the
   handshake frames stay as they are; a mandatory-TLS mode is a new protocol
   version, and the host's `supported_versions` list tells a client which it
   may use.

## What is tested, and what is not

Covered in CI (not by this page):

- Pairing limits, lockout, single use and expiry; revocation closing live
  connections; replayed refresh tokens; a revoked family not re-admitted after
  a restart (`rustcode/engine/src/remote_gateway/e2e_tests.rs`,
  `gateway/tests.rs`, `devices.rs`).
- Browser `Origin` refusal; a TLS ClientHello refused as `secure_connection`;
  version mismatch reported as `unsupported_version` (`transport.rs`,
  `handshake.rs`, `e2e_tests.rs`).
- Device keys: pairing and a token sign-in that register a key, a signature
  accepted once per socket, a token refused with `key_required` after the
  key, wrong and missing signatures refused (`gateway/tests.rs`), and a
  token-paired machine moved over to the install's key by the client
  (`rustcode/client/tests/gateway_round_trip.rs`).
- Identity before credentials: a token is not sent to an address that
  answers as another machine, and a public address never receives a secret
  (`rustcode/client/src/machine.rs`).
- Route and encryption labelling: which interface kinds the engine calls
  `lan` and `overlay`, that an unknown address or CGNAT address on Wi-Fi is
  never labelled encrypted, and what the desktop app prints for each
  (`address.rs` tests, `apps/desktop/src/acp/remote.test.ts`,
  `apps/desktop/src/state/machines.test.ts`).
- Downgrade and identity, with no new transport: a device that moves from the
  overlay to the LAN is reported on the LAN and unencrypted; a revoked device
  is not listed as connected; an address nothing is known about is never
  encrypted, on the gateway (`acp/remote.rs` tests), in the client
  (`registry.rs` tests) and in `rc machines list` (`machines.rs`
  tests); a reconnection over the LAN after a tunnel raises a downgrade
  notice (`src-tauri/src/machines.rs`, `state/machines.test.ts`).
- Direct TLS, against the real gateway (`rustcode/client/tests/gateway_round_trip.rs`):
  pairing pins the certificate and every sign-in after it is TLS, reported as
  `tls` and encrypted by the host; a machine paired in the clear is pinned on
  its next sign-in; a changed certificate is refused with nothing sent and the
  pairing survives; a pinned machine whose address answers plain WebSocket is
  never dialled in the clear; a revoked device is refused over TLS; a stranger
  on a saved address fails the TLS handshake before any frame. The certificate
  is made once, kept `0600` and never replaced when unreadable (`tls.rs`).

Not covered, and listed so nobody assumes otherwise:

- **A real iPhone.** Local-network permission, Wi-Fi to cellular changes and
  NetBird routing on iOS are verified with the app and a device, not by the
  Simulator or by this repository's tests.
- **The phone app over TLS,** which does not pin yet.
- **An active attacker on a plain route** (spoofed gateway, injected frames).
  The doc above states the result; no test exercises it.
- **Older mobile clients** against a gateway that adds fields. The protocol's
  golden files (`docs/remote-protocol/v1/golden`) keep frames compatible;
  an actual older phone build is a manual check.

## Follow-ups

- The phone app (`rustcode-mobile`): read a fingerprint from the QR payload
  (the payload gains the field) and pin it, once its WebSocket stack is
  confirmed to take a pinned certificate.
- Per-connection route in the phone app (`rc machines list` and the
  desktop show it today).
- A device-test checklist for an iPhone on LAN, NetBird and cellular.
- Key support in the phone app (`rustcode-mobile`), then refusing tokens from
  devices without a key.
- Optional hosted relay: #3199, opt-in.
