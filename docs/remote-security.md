# Remote access: threat model and transport security

What protects a paired device's connection to a RustCode gateway today, what
does not, and what a future encrypted transport has to beat. The behavior of
remote access itself is in [remote.md](remote.md); the wire format is in
[remote-protocol](remote-protocol/README.md). This page is the security view
of both, for issue #2651.

> **The gateway has no TLS, so a plain LAN WebSocket is authenticated but not
> encrypted.** Anyone on the same network as the device or the computer can
> read the traffic (prompts, session output, file contents the agent prints)
> and, because nothing is signed per frame, alter it. A tunnel such as NetBird,
> Tailscale or ZeroTier encrypts what it carries; RustCode itself encrypts
> nothing. Every place the apps show a route says which of the two it is.

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
  stops that attacker. Direct TLS with a pinned certificate would be the
  first RustCode-owned host proof.
- **Pairing** is bound to the physical presence of the owner: only the local
  user can ask for a challenge (the control socket), and a *connect request*
  needs the owner to press Accept in the app and then to read the code to the
  device on a channel the network does not see (screen to screen).
- **Tokens.** A static token lasts 30 days. A device that asks for
  `token_rotation` holds a refresh token that every reconnect replaces, plus a
  15-minute access token; presenting a retired refresh token revokes the whole
  pairing (`token_replayed`). Rotation shortens how long a token lifted off a
  plain network stays useful, but does not stop its use within that window.

## Encryption per route

Encryption belongs to the route, not to RustCode. The gateway always speaks
plain `ws://`. The engine classifies each address it listens on
(`rustcode/engine/src/remote_gateway/address.rs`: `route_kind_for`,
`ConnectionRoute`) and reports it; the apps label it.

| Route | Reported as | Encrypted? | Notes |
| --- | --- | --- | --- |
| LAN WebSocket (Wi-Fi, Ethernet; a private IPv4 address) | `lan` | **No** | Plain WebSocket. Trust only a network where every device is yours. Bonjour discovery announces the service, never credentials. |
| NetBird, Tailscale, ZeroTier (an address on a tunnel interface) | `overlay`, with `provider` when the interface name says it | **Yes, by the tunnel** | WireGuard (NetBird, Tailscale) or ZeroTier's own encryption protects the hop between the two machines. The tunnel authenticates each peer. RustCode does not check that a tunnel is up; it classifies by the interface carrying the address. |
| A generic VPN whose interface is named `utunN`, `tun*`, `wg*` | `overlay`, no provider | **Yes, assumed** | The interface being a tunnel is all the engine can see. A VPN that does not encrypt (a plain GRE or IP-in-IP tunnel) would be mislabelled; none of the supported products behave so. |
| `100.64.0.0/10` on Wi-Fi or Ethernet | not a route | n/a | A carrier's CGNAT address, never advertised, never bound, never labelled encrypted. |
| A host name (the machine's `.local` name, or `--advertise <name>`) | `hostname` | **Unknown** | The name resolves to whichever address the client picks. The *connection* is classified by the address it actually reached. |
| A public address or name | `public` | **No** | The client refuses to send a pairing code or token to one (`rustcode/client/src/registry.rs`: `may_send_secrets`). |
| Loopback (a Simulator or a client on this computer) | `loopback` | n/a | Never leaves the machine. |
| TLS (`wss://`) | not supported | n/a | The gateway answers a TLS ClientHello with a refusal recorded as `secure_connection` and closes the socket. Nothing is shared. |

Per connection, the gateway classifies the local address the device dialled and
reports its `path` (`loopback`, `lan`, `overlay` or `other`), the overlay
`provider`, the `localAddress` and whether the network `encrypted` it. An
address on no interface is `other` and unencrypted: the engine never assumes
a tunnel. `encrypted` is true only for `overlay`.

Where this shows:

- Settings › Remote access › Paired devices: a chip per connected device,
  `LAN · Unencrypted`, `NetBird · Encrypted`, `Tailscale · Encrypted`,
  `VPN · Encrypted`, `This computer`.
- Settings › Environments: a machine this computer connects to shows
  `LAN · Unencrypted` or `NetBird · Encrypted` for the route in use
  (`apps/desktop/src/state/machines.ts`: `routeLabel`; the client side
  classifies by the route kind it saved, `rustcode/client/src/registry.rs`:
  `RouteKind::is_encrypted`).
- `rustcode remote status`: each connected device says `connected over LAN (not
  encrypted)` or `connected over NetBird (encrypted by the tunnel)`.
- ACP: `_rustcode/remote` carries `routes` on each connected device.

Downgrade: there is nothing to downgrade from. No encrypted transport exists
to negotiate, so a client cannot be talked out of one. The two refusals that
exist are protocol-version mismatch (`unsupported_version`, both directions,
`rustcode/engine/src/remote_gateway/handshake.rs`) and the public-address
rule above. When TLS lands, a client that has seen a host offer it must
refuse a `ws://` fallback for that host; that rule is part of the migration
below.

## Credential storage and revocation

| Where | What | Protection |
| --- | --- | --- |
| Host: `remote/devices.json` | SHA-256 digest of each device token, name, kind, platform, pairing and last-seen times | `0600` file in a `0700` directory, replaced atomically. Never a token. |
| Host: `remote/revocations.json` | Revoked device families that could not be removed from the store yet | Same permissions; the gateway refuses to start if it cannot read it rather than readmit a revoked family |
| Host: control and owner sockets | Who may ask for a pairing challenge or share a session | `0600` Unix sockets in a `0700` directory; on Windows, loopback ports with a per-start 256-bit token |
| Computer as a client: machine tokens | The token this computer holds for each machine it connects to | The operating system credential store (Keychain, Credential Manager, Secret Service) under `rustcode-machines`; `machines/tokens.json` (`0600`) only when `RUSTCODE_MACHINE_TOKEN_STORE=file` is set |
| Computer as a client: `machines.json` | Routes, host ids and device ids, no secrets | `0600` |
| Phone | The phone's token and refresh token | Held by the mobile app (`rustcode-mobile`); not verified by this repository |

Credentials never go to `credentials.json` or `provider-accounts.json`, never to
logs (the gateway logs device names and ids), and not to Bonjour (service name,
port, protocol version, gateway id and capabilities only).

Revocation, from the quickest to the widest:

1. Settings › Remote access › *Revoke*, or `rustcode remote revoke <device>`:
   the device's connections close in the same step with `revoked`, and its
   token is forgotten.
2. `rustcode remote devices deny <id>`: removes the ACP grant and disconnects.
3. `/remote off` or `rustcode remote stop`: nobody reaches the gateway;
   pairings are kept.
4. Replay of a retired refresh token: the whole pairing is revoked by the host.
5. Expiry: a static token ends after 30 days; a rotating pairing lasts until
   revoked.

A revoked device is refused with `unauthorized` or `revoked` when it returns,
and the refusal is listed under *Refused connections* for a quarter of an hour.

## Direct TLS and the existing VPN options

This compares what the issue asks about. Only the first two columns exist.

| | Plain LAN | VPN overlay (NetBird, Tailscale, ZeroTier) | Direct TLS on the gateway (not built) |
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

Recommendation, not a decision: keep the overlay recommendation while it is
the only encrypted route, and design direct TLS as a pinned certificate
delivered in the pairing payload, because that is the only option that covers
the LAN with no external service and no CA. That needs an implementation
plan and an owner-visible consent: nothing here opens a port to the internet
or contacts a service the user did not set up.

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
2. **New client, old host.** A client that tries TLS first and finds a plain
   gateway reads the first byte of its answer, falls back only if the host has
   never offered TLS, and records that. A host that has offered TLS before
   must never be dialled over plain `ws://` again without the owner choosing
   it: that is the downgrade the issue asks to refuse.
3. **Pairings.** A pairing made over plain `ws://` stays valid; the first
   successful TLS connection pins the certificate fingerprint to it. Nothing
   is re-paired for the upgrade.
4. **QR and connect codes** gain a fingerprint field, additively. A client
   that does not read it ignores it; a client that does verifies the
   certificate against it.
5. **Version.** Adding the TLS listener is a minor protocol change if the
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
- Identity before credentials: a token is not sent to an address that
  answers as another machine, and a public address never receives a secret
  (`rustcode/client/src/machine.rs`).
- Route and encryption labelling: which interface kinds the engine calls
  `lan` and `overlay`, that an unknown address or CGNAT address on Wi-Fi is
  never labelled encrypted, and what the desktop app prints for each
  (`address.rs` tests, `apps/desktop/src/acp/remote.test.ts`,
  `apps/desktop/src/state/machines.test.ts`).

Not covered, and listed so nobody assumes otherwise:

- **A real iPhone.** Local-network permission, Wi-Fi to cellular changes and
  NetBird routing on iOS are verified with the app and a device, not by the
  Simulator or by this repository's tests.
- **Certificate and identity failures,** because there is no TLS to fail.
  These tests come with the transport.
- **An active attacker on a plain route** (spoofed gateway, injected frames).
  The doc above states the result; no test exercises it.
- **Older mobile clients** against a gateway that adds fields. The protocol's
  golden files (`docs/remote-protocol/v1/golden`) keep frames compatible;
  an actual older phone build is a manual check.

## Follow-ups

- Choose and build direct TLS with a pinned certificate in the pairing
  payload (needs the mobile client's capabilities confirmed first).
- A TLS-aware `describe`/`authenticate` and the downgrade refusal described
  under Migration, with tests for a changed certificate, an unknown
  certificate and a revoked device over TLS.
- Per-connection route in the phone app and in `rustcode machines list`
  (the desktop shows it today).
- A device-test checklist for an iPhone on LAN, NetBird and cellular.
- Optional hosted relay: #3199, opt-in.
