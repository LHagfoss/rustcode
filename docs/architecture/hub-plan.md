# Public access and the hub

How RustCode becomes reachable from the internet (hosted agents such as
ChatGPT and Claude.ai, phones away from home, a browser) without a main
machine, and in which order it is built.

## Decisions

- **Machine stays the word.** A RustCode install on a computer is a
  *machine* (`docs/terminology.md`). The always-on role is a **hub**, not a
  "main" machine: `machines.json` ignores an old `main` field on purpose.
- **Every machine is a peer.** A machine owns its threads, projects,
  harness accounts and settings; nothing replicates thread history. The hub
  is a role an ordinary machine takes on (a VPS, a desktop that stays on), and
  the fleet keeps working directly when it is down.
- **Built into the engine, not a separate service.** Grants, access levels,
  per-call approval and the audit log already live in the engine
  (`orchestration/outside*.rs`). A separate front service would either
  duplicate them or hold an all-powerful credential to the engine. A tunnel
  (`cloudflared`, run as a system service or in Docker) is the only extra
  process.
- **Self-hosted tunnel, no relay service.** T3 Code's T3 Connect is a
  multi-tenant relay (a Cloudflare Worker that provisions a tunnel per
  environment under its own zone, with Clerk accounts). One owner with their
  own domain needs only a named Cloudflare Tunnel per public machine.
- **TLS ends at the tunnel's edge.** The tunnel provider can read the traffic.
  This is stated wherever the public address is set up
  (`docs/mcp-outside-agents.md`, `docs/remote-security.md`).
- **Separate audiences.** An outside-agent (MCP) login only works on `/mcp`;
  device, machine and web-app sessions never work there, and an MCP login
  never works on their routes. Learned from T3 Code's environment auth, where
  the RPC surface would otherwise let an agent act above its ceiling.

## How T3 Code does it (for reference)

- Each environment's server has `/mcp` with a small OAuth server: stateless
  dynamic client registration (a signed client id), public clients with
  PKCE, loopback callbacks on any port and any `https` callback for hosted
  agents, approved with a one-time code or a signed-in browser session. The
  grant is read-only or a runtime-mode ceiling, not a list of scopes. 30 days.
- Reachability: LAN pairing, Tailscale Serve, desktop-managed SSH, and T3
  Connect (above). Clients keep an ordered list of routes per environment and
  connect over the first that answers.
- No main environment: peer links let one environment's agents sign in to
  another's `/mcp` as an outside client.

## Phases

| Phase | What | Issue |
| --- | --- | --- |
| 1 | Public MCP on one machine: `[outside] public_url`, Host validation for it, client registration, `https` callbacks, refresh tokens (30-day sign-in), sign-in page approved with `rustcode outside code` | this PR |
| 2 | Approve sign-ins with the owner's passkey | #3954 |
| 3 | One listener for the gateway (`/ws`) and `/mcp`, so one tunnel serves both; `wss://` routes | #3955 (built) |
| 4 | Machines and devices authenticate with key pairs instead of bearer tokens | #3956 |
| 5 | The hub role: outbound connections from other machines, `machine` for outside agents, a mailbox for offline machines | #3957 |
| 6 | The app served from a hub, with passkey login | #3958 |

Phase 1 alone makes `https://rustcode.example.com/mcp` work for hosted agents
against the machine that runs the tunnel. Phases 3–5 extend that to the rest
of the fleet; 2 and 6 replace the terminal code with a login.

## Phase 3 in practice

The engine's HTTP listener (the `/mcp` one, `43871`) relays a WebSocket
upgrade at `/ws` to the remote gateway, which may run in another process
(`rustcode remote`, the server). The gateway binds a loopback-only *ingress*
listener on an ephemeral port, records it as `ingress_address` in its
registration, and serves every socket from it as a `tunnel` connection
(encrypted to the tunnel's edge). The relay checks `Host` exactly as `/mcp`
does and leaves the `Origin` rule to the gateway. The gateway's own port
(`17879`) is unchanged, so LAN and VPN routes keep working as before.

Clients dial `wss://host` with TLS verified by the operating system's trust
store (`rustls-platform-verifier`), port `443` and path `/ws` by default, and
may send credentials over it. The phone app (`rustcode-mobile`) still needs
the same change.

## Phase 1 in practice

On a VPS: `rustcode server run` as a systemd user service (`docs/server.md`),
`[outside] public_url` in the user `config.toml`, and `cloudflared` forwarding
the hostname to `127.0.0.1:43871`. The steps are in
[mcp-outside-agents.md › Hosted agents](../mcp-outside-agents.md#hosted-agents-chatgpt-claude-and-other-cloud-clients).

Still to verify with live clients: ChatGPT's custom connector and Claude.ai's
custom connector against the stateless JSON Streamable HTTP transport, their
registration metadata, and whether either needs Client ID Metadata Documents
instead of dynamic registration.
