# Changelog

## 0.1.17 — 2026-10-12

- Compact the empty composer when the keyboard is dismissed, expand it while editing, and add responsive touch feedback ([PR #32](https://github.com/LHagfoss/rustcode_app/pull/32)).

## 0.1.16 — 2026-10-12

- Keep the screen awake during active turns, improve long transcript scrolling, and keep slash completion attached to the typed command ([PR #30](https://github.com/LHagfoss/rustcode_app/pull/30)).

## 0.1.15 — 2026-10-12

- Show chat notices as transient toasts, add a jump-to-latest control and prevent session row swipes from closing the sidebar ([PR #28](https://github.com/LHagfoss/rustcode_app/pull/28)).

## 0.1.1 — 2026-10-10

- Scan the `/remote` QR on the same Wi-Fi with RustCode 0.64.1; the host chooses
  its LAN address automatically. Keep explicit NetBird setup for remote access.
- Preserve host turn IDs, measured work/thought durations, terminal outcomes
  and end times through live updates, history and reconnects. Render one footer
  per logical turn, including failed/cancelled turns with no assistant answer.
- Ignore empty clock deltas as transcript content, protect current streaming
  from stale finish events, and resume using the gateway snapshot identity.
- Retain raw Markdown and thinking content with native thinking disclosures,
  improved touch/keyboard handling and tested draft preservation.
- Add CI and public Simulator/unsigned-device release artifacts with checksums.
  Build a signed development archive locally and install it with one command
  when the phone is ready. Rebuild when source changes to avoid stale updates.

The synchronized v1 protocol is documented in the host's
[app UI guide](https://github.com/LHagfoss/rustcode/blob/main/docs/remote-protocol/APP-UI.md).
Physical iPhone camera/LAN/NetBird acceptance remains deferred; this release
was checked with package tests, real-host integration and an iPhone Simulator.
