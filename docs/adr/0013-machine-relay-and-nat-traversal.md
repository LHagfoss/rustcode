# 0013: Machines reach each other through a blind, self-hostable relay

Status: accepted, 2026-11-28; proposed 2026-11-26 (#4390). The relay server is
`rustcode/engine/src/remote_relay/` (`rc remote relay`). Code today: `rustcode/client/src/routes.rs`,
`rustcode/client/src/machine.rs` (route race), `rustcode/client/src/registry.rs`
(`RouteKind`, `is_encrypted`, `may_send_secrets`), `rustcode/engine/src/remote_gateway/tls.rs`
and `transport.rs` (pinned gateway TLS, one port), `remote_gateway/device_key.rs` and
`rustcode/client/src/device_key.rs` (Ed25519 device keys), `remote_gateway/hub_link.rs` and
`rustcode/cli/src/hub_member.rs` (hub membership). Docs: [remote.md](../remote.md),
[remote-security.md](../remote-security.md), [hub-plan.md](../architecture/hub-plan.md),
[release-footprint.md](../release-footprint.md).

## Why

Two computers with no shared LAN and no overlay (NetBird, Tailscale, ZeroTier) can only meet
through a public `wss://` address behind a tunnel such as `cloudflared`. TLS ends at the tunnel
provider's edge, so the provider can read prompts, transcripts and the phone's bearer tokens;
it is the only route where the gateway's pinned certificate is not checked. The hub only
forwards outside agents' MCP calls and stores queued ones in plain JSON. hub-plan.md chose
"self-hosted tunnel, no relay service", and #3199 (relay without a VPN) closed without a PR.

The requirement: an optional, self-hostable relay (the owner has a VPS) that never sees
conversation content, with no mandatory hosted service.

## Options

| | Operator sees | Identity binding | NAT traversal | Size / deps | Phone (`rustcode-mobile`) | Self-hosting |
| --- | --- | --- | --- | --- | --- | --- |
| **1. Blind splice** (hub or `rc relay serve`) | Who talks to whom (machine ids), timing, byte counts | Unchanged: pinned gateway TLS and device-key challenge run inside the pipe | None; all traffic relayed | A few hundred lines on existing tokio/TLS; no new crates | Works with any WebSocket client that accepts a pinned cert (already a follow-up) | One binary already on the VPS, one port |
| **2. iroh 1.0** | Relay sees encrypted QUIC between node ids | Node key is a new identity; must be bound to `host_id` and the TLS pin | Hole punching with relay fallback | Large (QUIC stack, netwatch, portmapper); measurable footprint increase | Rust only; RN would need a native module | `iroh-relay` self-hostable; default discovery uses n0 DNS/pkarr, must be replaced with our own lookup |
| **3. rust-libp2p** circuit relay v2 + DCUtR | Encrypted streams | Peer id is a new identity | Hole punching, relay limited in bytes/time | Heaviest dependency surface | Same as iroh | Self-hostable relay node |
| **4. Embedded WireGuard** (boringtun, or a NetBird/Tailscale client) | Nothing (or the coordination server sees membership) | Keys managed by WireGuard, separate from ours | Via the overlay's coordination | Needs TUN / privileges or userspace netstack | Separate VPN profile | Needs a coordination server |

## Decision

**Ship option 1 now; keep option 2 as the path to direct connections later; leave overlays
user-installed.**

1. **The relay is a blind byte splice.** A machine that wants to be reachable keeps one
   outbound, authenticated connection to a relay (the hub link it already holds when a hub is
   set, or `rc relay serve` on any VPS). A client asks the relay for a pipe to a machine id; the
   relay connects the two streams and copies bytes. The existing gateway TLS, **pinned to the
   machine's certificate**, and the device-key challenge run end to end inside the pipe, so no
   new cryptography is needed and the relay never holds a key that could decrypt traffic. This
   confirms the hypothesis in #4390: pinning plus device keys already give end-to-end security
   over an untrusted byte pipe.
2. **Relay authentication.** Members authenticate to the relay with their device key; a client
   may only request pipes to machines in its own fleet (the relay learns fleet membership from
   members' signed registration, not from content). Abuse controls: per-member concurrent pipe
   cap, idle timeout, byte-rate cap, and no pipes to unregistered ids.
3. **Route kind.** A new `RouteKind::Relay`, label "Relay · Encrypted by RustCode",
   `is_encrypted() == true`, `may_send_secrets() == true` only because the pin is checked.
   Rank after LAN, overlay and SSH and before a public `wss://` address. A relay route whose
   TLS pin does not match fails closed; there is no fallback to unpinned TLS.
4. **Failover and switch-back.** The existing race applies (LAN first, overlay after 1 s, cap
   5 s), with the relay dialled after the overlay grace period. A connection on the relay
   re-races in the background every 60 s with backoff (1, 2, 4 … 300 s) and moves to a better
   route at the next idle point between requests; a live stream is never cut to switch.
5. **Protocol and versions.** The gateway protocol is unchanged; only the transport under it
   is new. Relay registration carries a protocol version; a relay refuses older members with a
   clear error, and a client falls back to other routes.
6. **Hub mailbox.** Queued MCP calls keep living on the hub (out of scope here); #4398 covers
   their delivery reporting. Encrypting them at rest is a separate follow-up.
7. **Direct connections later.** When measured relay latency or VPS bandwidth becomes a real
   problem, adopt iroh for hole punching with `iroh-relay` self-hosted and n0 discovery
   replaced by the relay's own registry, binding the iroh node key to the device key. Until
   there is that evidence, its footprint is not justified.
8. **No managed relay is required.** A RustCode-operated relay may exist as an opt-in default
   address later; everything works with a self-hosted one or none.

9. **Phones use an HTTP entry on the same port.** React Native has only `fetch` and
   `WebSocket`, and cannot pin a certificate. The phone fetches a single-use challenge
   (`GET /v1/challenge`) and opens a WebSocket to `/v1/dial` with the machine id, its device
   key and its signature in the query. The relay checks them, writes the untouched request
   head into the machine's pipe and splices; the gateway answers the upgrade. In place of
   pinned TLS the connection is sealed end to end with Noise NK to the `sealed_key` in the
   pairing QR code, which now also carries the `relay` route. A gateway refuses a relayed
   connection whose first frame is not a seal hello (or TLS), so tokens never cross the relay
   readable. See `docs/remote-protocol/README.md` › Through a relay.

## What the relay operator can see

Machine ids, which device key connects to which machine, when, and how many bytes. Not
prompts, transcripts, files, tokens or tool calls: they travel inside the gateway's pinned TLS
or, for a phone, inside the Noise-sealed WebSocket. This is stated in remote-security.md where
the relay is set up.

## Consequences

- Owners with a VPS get private remote access without a VPN or a tunnel provider reading
  traffic.
- All relay traffic costs VPS bandwidth; no NAT traversal until step 7.
- The phone reaches the relay without certificate pinning, through the HTTP entry and a
  sealed connection (decision 9).

## Follow-up issues (one PR each, in order)

1. `feat(remote): add rc remote relay with device-key registration and blind pipes` (done)
2. `feat(client): add the Relay route kind with pinned TLS and race placement` (done)
3. `feat(remote): let a machine register with a relay from Settings and rc machines` (`rc machines relay` done; Settings next)
4. `feat(remote): re-race relay connections and switch to a better route when idle`
5. `docs(remote): set up a self-hosted relay on a VPS`
6. `feat(remote): encrypt queued hub mailbox calls at rest`
7. (Deferred, needs evidence) `research(remote): measure relay latency and evaluate iroh hole punching`

## Unknowns

- Real-world bandwidth of a typical session (streaming transcripts are small; screenshots and
  file transfers are not); to be measured with step 1.

## Sources

- iroh: https://www.iroh.computer/docs and https://github.com/n0-computer/iroh
- iroh-relay: https://docs.rs/iroh-relay
- libp2p circuit relay v2: https://github.com/libp2p/specs/blob/master/relay/circuit-v2.md
- libp2p DCUtR: https://github.com/libp2p/specs/blob/master/relay/DCUtR.md
- boringtun: https://github.com/cloudflare/boringtun
- #3199 (relay without a VPN, closed)
