# 0011: OS computer use runs in a separate worker, behind the RustCode MCP server

Status: accepted, 2026-11-27. Phases 1–4 and Phase 6 (cross-machine control via
the controlled computer's desktop app) have landed on macOS
(`rustcode/engine/src/computer_worker/`, `rustcode/engine/src/computer_use/desktop.rs`,
`rustcode/engine/src/acp/desktop_call.rs`, `apps/desktop/src/state/hostDesktop.ts`);
see *Implementation notes* below. Proposed 2026-11-26 (#4369). Code today: `rustcode/engine/src/computer_use.rs`
(browser tools), `rustcode/engine/src/orchestration/mcp_server.rs` (the per-thread
RustCode MCP server), `rustcode/engine/src/desktop_control.rs` (client-owned bridge with
no client), `rustcode/engine/src/clipboard.rs`. Docs: [computer-use.md](../computer-use.md),
[desktop-automation.md](../desktop-automation.md), [permissions.md](../permissions.md),
[remote-security.md](../remote-security.md). Related: the embedded browser decision (#4370).

## Why

RustCode can drive a headless Chrome (`browser_*` tools), but not the operating system:
no screen capture, no mouse or keyboard input, no window list, no accessibility tree.
The only native bridge, `desktop_control`, was implemented by the SwiftUI macOS app and
lost its client when that app was removed (#2820). #3154 planned "native desktop control"
as a second phase and closed after the browser phase.

OS computer use is the most dangerous capability RustCode could offer: it acts with the
user's whole session, including password managers, mail and banking. The four desktop
platforms differ more here than anywhere else in the product. Code written before these
decisions would bake in a process model and a permission model that are hard to change.

## Decision

### 1. Platform matrix

| | macOS | Windows | Linux X11 | Linux Wayland |
| --- | --- | --- | --- | --- |
| Screen and window capture | ScreenCaptureKit (`SCShareableContent`, `SCScreenshotManager`, macOS 14+) [1] | Windows.Graphics.Capture, DXGI Desktop Duplication as fallback [5] | XGetImage / XShm, XComposite for single windows (what `xcap` uses) | Only through the xdg-desktop-portal ScreenCast portal, with a consent dialog per session; restore tokens can skip it next time if the compositor supports them [9] |
| Input | `CGEvent` post (requires Accessibility) [2] | `SendInput` [6] | XTest | RemoteDesktop portal + libei, consent per session [10]; no portable alternative |
| Window list, launch, focus | `CGWindowListCopyWindowInfo`, `NSWorkspace`, `NSRunningApplication.activate` | `EnumWindows`, `ShellExecuteEx`, `SetForegroundWindow` (subject to foreground lock rules) | EWMH via `_NET_CLIENT_LIST`, `_NET_ACTIVE_WINDOW` | No global window list or focus API by design; compositor-specific (e.g. GNOME Shell extensions, wlr foreign-toplevel) |
| Accessibility tree | `AXUIElement` [3] | UI Automation [7] | AT-SPI2 over D-Bus [11] | AT-SPI2 over D-Bus (works; it is not a Wayland protocol) |
| OS permission | TCC: Screen Recording and Accessibility, granted to the signed binary that asks [4] | None for same-integrity windows; UIPI blocks input to elevated windows [8] | None | Portal consent per session |
| Hard blockers | Grants attach to code signature: an unsigned or re-signed build loses them (#2034) | Secure desktop (UAC, Ctrl+Alt+Del) cannot and must not be automated | None technical; any client can read any other (a security concern, not a blocker) | No capture or input without the portals; GNOME and KDE implement both, wlroots compositors implement ScreenCast but RemoteDesktop support varies (unknown per compositor; to verify in phase 2) |

Wayland ships **read-only** in the first version (portal capture plus AT-SPI2), and input
follows once libei support is verified on GNOME and KDE.

### 2. Libraries, one per layer

| Layer | Choice | Notes |
| --- | --- | --- |
| Capture | `xcap` [12] on macOS, Windows and X11 | Apache-2.0, actively maintained, multi-monitor and per-window capture. Its Wayland path uses the portal and is not used for v1; capture on Wayland goes through `ashpd` directly so the restore token is ours to keep. |
| Input | `enigo` [13] on macOS, Windows and X11 | MIT, maintained, `CGEvent`/`SendInput`/XTest. Wayland input via `ashpd` RemoteDesktop + `reis` (libei) in a later phase. |
| Accessibility (read and act) | Platform crates directly: `accessibility-sys`/`objc2` AX bindings (macOS), `windows` crate UIA (Windows), `atspi` (Linux) | `accesskit` is a *provider* library (exposes an app's own tree) and cannot read other apps; not used. |
| Portals | `ashpd` [14] | MIT, the standard Rust portal client. |
| Prior art, not dependencies | agent-desktop (Node CLI), Peekaboo (macOS, Swift), open-source computer-use MCP servers | Useful as behaviour references; a Node or Swift runtime dependency per platform is what we avoid. |

Licence and maintenance are re-checked when each phase's PR adds the crate (`cargo deny`
runs in CI).

### 3. Process model: `rc computer-worker`

A separate, minimal worker process holds the OS permissions and does every capture and input.
The engine talks to it over a local socket with a small typed request/response protocol.

- **TCC grants attach to one stable signed binary.** On macOS the engine binary changes with
  every update and harness; a dedicated worker binary inside the signed app bundle keeps the
  grant across updates.
- **Crash isolation.** Platform accessibility APIs can hang or crash on misbehaving apps; the
  engine and its threads must not.
- **Emergency stop is a kill.** Stopping the worker is a guaranteed stop, whatever it was doing.
- **Least privilege.** The worker has no network access of its own, no config writes, no
  access to credentials; it only executes actions the engine already approved.

In-engine was rejected for the reasons above. A Tauri-side implementation was rejected because
the engine (and `rc` on a headless machine) must own the tools for external harnesses and remote
use.

### 4. Exposure

One tool family, served by the existing per-thread RustCode MCP server for external harnesses
and as native tools for the built-in harness, exactly like `browser_*`:

`computer_windows`, `computer_screenshot`, `computer_snapshot` (accessibility tree with refs),
`computer_click`, `computer_type`, `computer_key`, `computer_scroll`.

- Coordinates are **logical points relative to a display ID**, never raw pixels. Every
  screenshot result carries the display ID, its origin, its scale factor and the image size, and
  the worker converts back. Mixed-DPI multi-monitor setups are tested explicitly.
- Acting by accessibility ref is preferred; coordinates are the fallback for apps with no usable
  tree. The prompt hint says so.
- Screenshots are attached as images (the fix in #4362 for `browser_screenshot` applies).

### 5. Safety model

- **Off by default, per machine.** Enabled in Settings on the machine whose desktop is driven;
  a paired device can never enable it remotely.
- **Grant per thread and app.** The first action in a thread asks the user to grant one app
  (or one window); actions outside it ask again. Grants end with the thread.
- **Always ask** for typing into secure text fields (AX `AXSecureTextField`, UIA `IsPassword`,
  AT-SPI `ROLE_PASSWORD_TEXT`) and for a denylist of sensitive apps (password managers,
  Keychain Access, banking, System Settings security panes), user-configurable.
- **Visible activity.** An always-on-top indicator shows which thread is acting and on what;
  a global emergency-stop shortcut kills the worker. Any physical user input pauses the agent
  at once until the user resumes it.
- **Bounded.** Per-action timeouts, cancellation through the turn's cancel token, a cap on
  actions per turn.
- **Audit.** Each action is an `operational_event` (`computer_use.action`: thread, app, action,
  outcome) with typed text redacted. Screenshots are not stored unless the user turns it on.
- **Untrusted content.** Everything read from the screen is untrusted input; the prompt hint
  says instructions on screen are data, not commands.
- **Remote boundary.** A thread on machine A may drive machine B's desktop only with a grant
  made on B (see [remote-security.md](../remote-security.md)); the grant card names the machine.
- **Never bypass OS security.** No TCC database edits, no UAC automation, no portal spoofing.

### 6. Relation to browser automation

Separate tools and separate settings: the browser tools act on an isolated headless profile,
OS computer use on the user's real session. They share the approval cards, grant storage,
audit event plumbing and the MCP server only.

### 7. `desktop_control.rs`

Delete it when phase 1 lands. Its client-owned model (the app executes, the engine forwards)
does not work for headless machines or external harnesses, and nothing implements it today.

### 8. Phased plan (follow-up issues)

1. **Worker skeleton and permission probe**: `rc computer-worker` with the socket protocol,
   a `permissions` request that reports capture/input/accessibility status per platform, the
   Settings switch, the audit event, and the emergency stop. No actions yet.
2. **Capture**: `computer_windows` and `computer_screenshot` (macOS, Windows, X11; Wayland via
   portal), with display-scale metadata and the grant card.
3. **Input**: `computer_click`, `computer_type`, `computer_key`, `computer_scroll` on macOS,
   Windows and X11, with the user-input pause and secure-field rule.
4. **Accessibility**: `computer_snapshot` and act-by-ref on all three APIs.
5. **Wayland input** via RemoteDesktop portal and libei, after verifying compositor support.
6. **Remote grants**: driving another paired machine's desktop.

## Implementation notes (2026-11-27)

What the first slice built, and where it departs from the plan above:

- **macOS first.** Capture, input and accessibility are written against the platform APIs
  directly (CoreGraphics, the Accessibility C API, ScreenCaptureKit and AppKit through `objc2`)
  instead of `xcap` and `enigo`: background input needs events posted to one process
  (`CGEventPostToPid`) and actions by ref need `AXUIElement`, which neither crate offers.
  Windows, X11 and Wayland report "not supported yet".
- **Process model.** `rc computer-worker` is the same `rc` binary started with one argument,
  speaking JSON lines over stdin and stdout rather than a socket. A closed stdin ends it; a
  dedicated signed helper binary is left for when release signing (#2034) needs it.
- **Background first.** Clicks press the element through Accessibility when they can; other
  clicks, keys and typing go to the target process through WindowServer (`SLEventPostToPid`,
  `macos/skylight.rs`) after a focus record makes its window key without raising it. Keyboard
  events carry an `SLSEventAuthenticationMessage` and left clicks a move plus an off-screen
  primer press, which Chromium and Electron accept as user input (Cua Driver's recipe, MIT).
  These are private interfaces, resolved at run time with the public `CGEventPostToPid` as the
  fallback, accepted because the alternative takes the user's pointer. The user's pointer and
  focus are never taken, so the "pause on physical input" rule is not needed for these actions
  and is not built. `foreground: true` remains for apps that still ignore the input: it brings
  the app to the front (`AXFrontmost`) and posts to the HID event tap, with an approval of its
  own (`<app>#foreground`).
- **Hosted worker.** The app starts `rc computer-worker --listen` as its child, so macOS checks
  RustCode's own TCC grants rather than the bare `rc` binary's; the engine connects over
  `<config>/run/computer-worker.sock` (same binary, or the same stable signature after an update) and starts its own worker only
  when no app is open.
- **Visible activity and stop.** The agent's own cursor (drawn by RustCode, excluded from
  captures) glides to each target on an eased arc (`computer_worker/cursor_motion.rs`) and
  rests where it last acted, with no label. There is no menu bar item: the Settings switch, or quitting the
  app, ends the worker and every grant. A global shortcut is not built.
- **Pixels.** `x` and `y` are pixels of the latest screenshot of that window, converted by the
  worker, rather than display points.
- **Cross-machine control (phase 6).** Every `computer_*` tool accepts an optional `machine`
  argument (orchestration version 23). Remote threads route through the controlled computer's
  open RustCode app to its local-only `_rustcode/desktop/call` handler (blocked in
  `acp_relay.rs`, and remote `_rustcode/settings/set` cannot enable `desktop-control`). Approval
  always happens on the controlled computer and is never bypassed by Full access.
- **Not yet:** the built-in harness's tools, the user-configurable sensitive-app list (a fixed
  list asks every time), and a cap on actions per turn.

## Consequences

- The signed app bundle gains a second binary; release signing (#2034) must sign it.
- Wayland users get read-only computer use first.
- No computer-use action can run without a grant on the machine it affects.

## Unknowns

- Which wlroots compositors implement the RemoteDesktop portal well enough for libei input.
- Whether restore tokens are honoured consistently across GNOME and KDE versions.
- The exact UIA behaviour for UWP apps under UIPI from a non-elevated worker.

## Sources

1. ScreenCaptureKit: https://developer.apple.com/documentation/screencapturekit
2. Quartz Event Services: https://developer.apple.com/documentation/coregraphics/quartz_event_services
3. AXUIElement: https://developer.apple.com/documentation/applicationservices/axuielement_h
4. TCC and signed code (Screen Recording, Accessibility): https://developer.apple.com/documentation/bundleresources/information_property_list/protected_resources
5. Windows.Graphics.Capture: https://learn.microsoft.com/windows/uwp/audio-video-camera/screen-capture
6. SendInput: https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-sendinput
7. UI Automation: https://learn.microsoft.com/windows/win32/winauto/entry-uiauto-win32
8. UIPI: https://learn.microsoft.com/windows/win32/winauto/uiauto-securityoverview
9. ScreenCast portal: https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html
10. RemoteDesktop portal: https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html
11. AT-SPI2: https://www.freedesktop.org/wiki/Accessibility/AT-SPI2/
12. xcap: https://github.com/nashaofu/xcap
13. enigo: https://github.com/enigo-rs/enigo
14. ashpd: https://github.com/bilelmoussaoui/ashpd
