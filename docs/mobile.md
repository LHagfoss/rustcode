# Mobile (remote only)

Decision: the phone never links Rust and never embeds the agent loop. Mobile
is a remote control for a PC or Mac that already runs RustCode. There is no
on-device core, no shared renderer strategy, and no Expo app in the release
matrix. Expo is explicitly out of scope until the transport below exists.

## Planned transport (not built yet)

A `rustcode serve` mode on the machine that owns the sessions:

- Binds LAN (never `0.0.0.0` without an explicit flag), authenticates with a
  per-launch token shown on the host.
- Speaks length-prefixed JSON frames in the same style as
  `engine/src/daemon/protocol.rs` (`PROTOCOL_VERSION`, `MAX_FRAME_BYTES`,
  `#[serde(tag = "type")]` envelopes). Reuse that framing; define a session
  protocol beside it.
- Operations: session list, prompt submit, event stream (transcript deltas,
  approval/question prompts), approval answers, cancel.

When this exists, any thin client — including a future Expo app under
`apps/` with generated types under `packages/` — can be built against it
without touching the Rust workspace. Until then, do not add mobile shells;
they would have nothing to talk to.
