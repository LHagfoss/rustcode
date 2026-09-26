# Native First-Run Readiness Design

Date: 2026-09-26
Issue: [#1417](https://github.com/LHagfoss/rustcode/issues/1417)

## Purpose

Keep RustCode's GPUI application as the primary desktop UI and remove the most
visible first-run friction found through source inspection and a live macOS
interaction pass. The existing graphite visual system is retained; this work
improves focus, state truthfulness, transition feedback, and native semantics.

## Architecture Decision

GPUI remains the right default for RustCode. The app already has a clean
controller boundary, a virtualized streamed transcript, native input handling,
and a substantial tested view layer. Tauri would improve frontend ecosystem,
packaging, and familiar accessibility tooling, but it would replace essentially
all current UI code and add a high-frequency IPC boundary. Revisit Tauri only if
broad installer coverage, web-team iteration speed, or audited screen-reader
support becomes a higher priority than the all-Rust native stack.

## Goals

- Focus the composer after the first frame so a fresh window is immediately
  typeable without stealing focus from later search, settings, or dialogs.
- Treat every controller snapshot's session list as authoritative, including an
  empty list, so deleted sessions cannot remain visible.
- Distinguish initial controller loading and session creation from the ordinary
  empty state, while guarding against duplicate new-session commands.
- Expose primary pointer controls through GPUI/AccessKit with button roles and
  descriptive labels without changing their visual treatment.
- Preserve the existing controller protocol, visual identity, and session flow.

## State and Interaction Design

`AppView` tracks whether its first controller response has arrived. Before that
response, the start screen communicates that RustCode is loading sessions. When
a workspace start is in flight, it instead communicates that the chat is being
prepared and ignores repeated New Chat actions. A snapshot or controller error
ends the initial-loading state.

The composer retains its existing deferred focus mechanism. Initialization sets
that intent immediately, allowing the first rendered frame to focus the actual
textarea rather than the root container. Existing explicit focus transitions
for chat search and restored sessions remain authoritative.

Snapshot application always replaces `recent_sessions`. This mirrors the
controller's snapshot contract and removes the current stale-cache exception.

Primary pointer-authored controls use toolkit `Button`s with concise accessible
labels and the existing click handlers. This pass covers new chat, settings,
back navigation, and the permission-mode toggle; transcript log semantics remain
unchanged.

## Error Handling

Controller errors end the initial loading state and continue to render through
the existing visible error path. A failed session-start command clears the
in-flight flag and pending prompt as it does today. Accessibility metadata has
no alternate behavior path and cannot change authorization policy.

## Testing

- Pure state tests cover authoritative empty session snapshots, loading/start
  copy, and the duplicate-start guard.
- Focus intent is verified in a GPUI-focused test where practical and by a live
  packaged-app keyboard probe before and after the change.
- The packaged app is inspected through macOS accessibility state to confirm the
  newly semantic controls appear as buttons.
- Final verification runs `cargo fmt --check`, `cargo check --tests`,
  `cargo test`, and `cargo test -p rustcode-app`.

## Non-goals

- Rewriting the GUI in Tauri or maintaining two complete frontends.
- Redesigning the existing theme, settings information architecture, or
  transcript renderer.
- Adding new settings or changing tool approval policy.
- Solving every screen-reader concern in one pass; this establishes and verifies
  the semantic pattern for the highest-frequency custom controls.
