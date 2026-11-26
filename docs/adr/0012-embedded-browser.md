# 0012: The browser panel shows the engine's own Chrome

Status: proposed, 2026-11-26 (#4370). Code today: `rustcode/engine/src/computer_use.rs`
(headless Chrome per thread, hand-written CDP client, domain allowlist),
`rustcode/engine/src/preview.rs` and `acp/preview.rs` (the `_rustcode/preview` client
protocol, no client since #2820), `apps/desktop/src-tauri` (Tauri 2, single webview).
Docs: [computer-use.md](../computer-use.md), [preview.md](../preview.md),
[remote.md](../remote.md). Related: [0011](0011-os-computer-use.md) (OS computer use),
#4367 (per-thread site grant and stop chip), #4210 (open a thread's app from another device).

## Why

Agents build web apps, and checking one today means the agent drives a headless Chrome the
user cannot see, while the user opens their own browser by hand. The SwiftUI app had a
preview panel; the Tauri app has none. Before building one there are three coupled
choices: what renders the page, who owns the browser session, and how the agent and the user
share it, including when the thread runs on another machine.

## Decision

### 1. Rendering: stream the engine's Chrome into a panel

The panel shows the **same Chrome the agent drives**, streamed with CDP
`Page.startScreencast` (JPEG frames, acked per frame) and with the user's mouse, wheel and
keyboard forwarded through `Input.dispatchMouseEvent` / `Input.dispatchKeyEvent` /
`Input.insertText`.

Options considered:

| | Tauri child webview | Engine Chrome screencast (chosen) | Hybrid |
| --- | --- | --- | --- |
| Engine | WKWebView / WebView2 / WebKitGTK: three engines, differs from what the agent sees | One Chromium for agent and user | Two |
| Agent access | No CDP on WebKitGTK or WKWebView; agent would need a separate browser | Same session, same cookies, same state the user sees | Split state |
| Remote threads and phone | Cannot show a page from another machine's localhost | Frames travel over the existing gateway | Partial |
| Cost | Native speed | Frame latency (tens of ms locally, more over remote), IME and clipboard need care | Both costs |

Tauri's multi-webview is behind its `unstable` feature, and a native webview cannot show
another machine's `localhost`. The screencast keeps one source of truth and works the same on
macOS, Windows, X11 and Wayland because nothing platform-specific renders the page. Its known
weaknesses are accepted for v1: IME composition goes through `Input.insertText` only, drag and
drop of files is not supported, and frame rate is capped (default 15 fps, on demand).

### 2. Sessions

- **One browser per thread**, as today, started on first use and closed when the thread is
  idle or finished (#4364).
- **Profiles**: throwaway by default. A project may opt into a **persistent isolated profile**
  stored under the engine's data directory (`browser-profiles/<project-id>/`), never the user's
  own Chrome profile; Settings shows its size and a Clear button.
- **Tabs and history**: multiple targets per browser; the panel shows tabs and back/forward
  from `Page.getNavigationHistory`.
- **Recovery**: a crashed renderer (`Inspector.targetCrashed`) or a restarted engine reopens
  the last URL per tab on the next action, and the panel says it did.

### 3. Agent capabilities: one `browser_*` family

`localhost_preview` merges into `browser_*`; localhost is just an allowed origin. Additions,
each its own follow-up: accessibility snapshot with refs (#4366 started refs), console and
runtime errors (`Runtime.consoleAPICalled`, `Runtime.exceptionThrown`, bounded ring buffer),
network log (`Network.*`, bounded, `Authorization`/`Cookie` headers redacted), viewport and
device emulation (`Emulation.setDeviceMetricsOverride`), tabs and history. Screenshots are
attached as images (#4362).

### 4. User to agent: element picker

A pick mode in the panel uses `Overlay.setInspectMode`; the picked node's selector, role,
accessible name, bounding box and a cropped screenshot go into the composer as one attachment.

### 5. Dev-server detection

The engine lists loopback listening ports owned by a thread's process tree (procfs on Linux,
`libproc` on macOS, `GetExtendedTcpTable` on Windows) and offers **Open in browser** on the
thread. Opening a detected port adds that origin to the thread's grant (#4367) for the session.
Vite's `server.allowedHosts` is not an issue because the engine connects to `127.0.0.1`
locally.

### 6. Remote

A thread on another machine streams frames and input over the existing authenticated gateway
connection; no public URL, no tunnel. #4210 (closed) covered sharing a running app by link;
that remains a separate, explicit sharing feature and is not implied by the panel.

### 7. Safety

- Origins need the thread's grant (#4367); the allowlist covers navigation, WebSockets,
  workers and iframes (#4365).
- While the agent acts, the panel shows its cursor and outlines the element; the stop chip
  (#4367) ends agent control at once, and user input in the panel takes over.
- Downloads are denied (as today), file uploads and JavaScript dialogs ask the user, and
  `browser_*` never reads cookies or storage values directly.
- Page content is untrusted; the prompt hint says so and tool results mark it.

### 8. CDP library

Keep the hand-written client in `computer_use.rs`: it is small, has no extra dependencies and
already covers the commands above with a request/response map. Revisit `chromiumoxide` if the
client grows past one module per domain or needs generated types for many domains.
Playwright through a Node sidecar is rejected: it would make Node a runtime dependency of the
engine, which RustCode avoids, and it duplicates what CDP gives directly.

### 9. Visual verification

An agent's "check the UI" step uses `browser_open`, `browser_snapshot` and
`browser_screenshot` on the detected dev server, then reports what it saw with the screenshot
attached. RustCode's own mock-window driver (`apps/desktop/scripts`) is the model for the
input vocabulary: real clicks, typing and key presses rather than injected JavaScript.

### 10. Phased plan (follow-up issues)

1. **Dev-server detection and Open**: engine port detection per thread plus an "Open" action
   that shows the existing headless browser's latest screenshot in a side panel (read-only).
2. **Live panel**: screencast frames and forwarded input, stop chip and agent cursor overlay.
3. **Console and network capture** for agent tools and the panel.
4. **Element picker** into the composer.
5. **Tabs, history and persistent project profiles.**
6. **Remote streaming** over the gateway.
7. **Retire `preview.rs` and `acp/preview.rs`** once 1–2 land.

## Consequences

- Chrome or Chromium stays a requirement for browser features, as today.
- The panel feels slightly less immediate than a native webview, in exchange for one session
  the agent and user share on every platform and across machines.

## Unknowns

- Screencast frame latency over the remote gateway on slow links (measure in phase 6).
- How well `Input.insertText` covers CJK IME composition in practice.

## Sources

- CDP Page.startScreencast: https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-startScreencast
- CDP Input domain: https://chromedevtools.github.io/devtools-protocol/tot/Input/
- CDP Overlay.setInspectMode: https://chromedevtools.github.io/devtools-protocol/tot/Overlay/#method-setInspectMode
- Tauri multiwebview (unstable): https://docs.rs/tauri/latest/tauri/webview/index.html
- chromiumoxide: https://github.com/mattsse/chromiumoxide
