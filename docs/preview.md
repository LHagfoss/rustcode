# Localhost preview

The macOS app shows a running dev server in a panel beside the chat (⇧⌘P). The
user can browse it there, and the agent can use it through the
`localhost_preview` tool once the user allows it. This page describes the
agent bridge (#2138), its limits, and the preview's resource lifecycle (#2140).

## What the agent can do

| Action       | Arguments                     | Result |
|--------------|-------------------------------|--------|
| `inspect`    | `max_chars` (200–20000, default 6000) | URL, title, the page's visible text (bounded) and up to 80 visible interactive elements with a CSS selector each |
| `screenshot` | none                          | A PNG (JPEG when large) of the visible page, at most 1280 px wide, attached to the conversation after the tool result |
| `navigate`   | `url`: a path (`/settings`), query, fragment or full local URL | The page after it loads |
| `click`      | `selector`                    | The page afterwards, with any navigation it started |
| `fill`       | `selector`, `value`           | Sets a text field's value and fires `input` and `change` |
| `select`     | `selector`, `value` (option value or label) | Chooses the option and fires `input` and `change` |
| `scroll`     | `dy` (pixels) or `selector`   | The new scroll position |

Every action is visible: the panel opens when the agent asks, the element acted
on is outlined briefly, and a badge over the page says what the agent is doing.
There is no action that runs arbitrary JavaScript.

Tool results are text in every provider protocol RustCode speaks, so a
screenshot is saved as an image attachment and added after the tool batch as a
message marked as coming from the tool, using the same `![image](file://…)`
marker a pasted image uses. A model without image input gets it described by
the configured `vision_model`.

## Who gets the tool

The tool exists only for clients that advertise it. At initialize the macOS app
sends `clientCapabilities._meta.rustcode.preview: 1`; the engine then adds
`localhost_preview` to the native tool list and a short hint to the system
prompt of that client's sessions. The terminal UI, the desktop shell and every
other ACP client never see it, and calling it there fails without doing
anything. Plan mode, subagents and compact prompts for small local models do
not get it either.

## Approval and scope

Showing a page grants the agent nothing.

- **First use asks.** The first action a chat requests shows a prompt in the
  preview panel naming the chat and the origin (scheme, host and port), with
  **Deny** and **Allow for This Chat**. Nothing runs until the user answers; an
  unanswered prompt is denied after 120 s. Only one prompt is open at a time.
- **Grants are narrow.** A grant covers one chat, one project and the one
  origin it was approved on. Grants are kept in memory only.
- **Grants end** when the user clicks the **Agent** button in the preview
  toolbar (shown while any chat has access), when the project changes, and
  when the engine restarts. A prompt that is open at that moment is answered
  "revoked".
- **Every action is checked** against the grant, the current project and the
  page's origin, before it runs and again after any wait. If the page has moved
  to another origin (for example a link to another port), further actions are
  refused until the user approves that origin.
- **Navigation stays local.** `navigate` accepts only paths or URLs on the
  approved origin; the engine also rejects non-loopback targets before sending
  anything. While an agent action runs, a link or `window.open` to another site
  is blocked rather than opened in the browser, and the result says so. When
  the user browses, such links still open in the default browser.
- **Downloads are cancelled**, for the agent and the user: the preview never
  saves files.
- **Dialogs do not block.** During an agent action, `alert` returns at once and
  `confirm` answers Cancel; the result reports the message.
- **What the agent can read** is the main frame's visible text and element
  list. Password field values are never returned. Cross-origin iframes are not
  read. The agent gets no cookies, storage, browser profile or file access, and
  scripts run in WebKit's isolated client world, so the page cannot observe or
  tamper with them. There is no script message handler, so the page has no
  channel to the app.

## Failures, timeouts and cancellation

Each action has a 20 s limit in the app; the engine waits at most 150 s for an
answer (enough for the approval prompt) and gives up at once when the turn is
cancelled. The engine reports success only for an explicit `"ok": true`; a
denial, a revoked grant, a timeout, a missing page, a selector that matches
nothing, a transport error or a malformed answer is an error result. A timeout
or cancellation says the outcome is unknown, because WebKit work already
started cannot be undone.

## Protocol

The engine sends the ACP extension request `_rustcode/preview`:

```json
{"sessionId": "…", "operation": {"action": "fill", "selector": "#email", "value": "a@b.c"}}
```

Operation fields are `action`, `url`, `selector`, `value`, `dy` and `maxChars`.
The client answers with a result, never a JSON-RPC error for an expected
failure:

```json
{"ok": true, "url": "http://localhost:3000/", "title": "App", "note": "…"}
{"ok": false, "error": {"code": "denied", "message": "The user denied access."}}
```

`inspect` adds `text`, `truncated` and `elements` (`selector`, `tag`, `type`,
`role`, `text`, `name`, `value`, `href`, `disabled`); `screenshot` adds
`image` (`mimeType`, base64 `data`, `width`, `height`); `scroll` adds `scrollY`.
Error codes: `denied`, `revoked`, `approval_timeout`, `out_of_scope`,
`no_page`, `not_found`, `invalid`, `timeout`, `busy`, `failed`. The engine side
is `rustcode/engine/src/preview.rs`; the app side is
`macos/Sources/RustCodeKit/PreviewAgent.swift` and
`macos/Sources/RustCode/PreviewAgentBridge.swift`.

## Resource lifecycle

`PreviewController` lives as long as the app and owns at most one `WKWebView`,
created when the preview is first used. Switching project tears it down (KVO
observations invalidated, delegates cleared, removed from its host view) so the
old page and its WebContent process are released, and the next project gets a
new web view with its own history. Hiding the panel only removes the web view
from the window: the page keeps running, so a dev server's state and its HMR
connection survive, and WebKit throttles timers and stops animation frames for
a page outside a window. The website data store is the app's persistent
default store, shared with no browser, so a dev app's sign-in survives a
restart; RustCode does not clear it.

### Measurements

`macos/Tests/RustCodeTests/PreviewSoakTests.swift` is a reproducible workload
against the real controller and WebKit: a page with 5,000 DOM rows, a 60 Hz
timer, a canvas animation and 200,000 objects allocated on every load, served
from 127.0.0.1. Run it with
`RUSTCODE_PREVIEW_SOAK=1 swift test --package-path macos --filter PreviewSoakTests`.

macOS 27.0.1 (26A434), Apple silicon, 30 s phases. RSS from `ps`; CPU is
cumulative CPU time.

| Phase | Test process | WebContent |
|-------|--------------|------------|
| Before any web view | 62.9 MB | — |
| Loaded | 91.8 MB, 0.29 s | 78.0 MB, 0.31 s |
| +30 s in window | 91.6 MB, 0.35 s | 77.9 MB, 0.36 s |
| +30 s out of window (hidden) | 91.5 MB, 0.41 s | 77.7 MB, 0.37 s |
| 25 reloads | 82.3 MB | 565.9 MB |
| 50 reloads | 82.8 MB | 615.7 MB |
| 75 reloads | 81.6 MB | 616.0 MB |
| 100 reloads, settled | 81.8 MB, 1.09 s | 616.7 MB, 4.57 s |
| 10 / 20 / 30 project switches | 87.8 / 88.1 / 88.2 MB | 75.8 / 75.6 / 75.8 MB (new process each time) |

After 30 project switches, 31 WebContent processes had been used, none of the
old ones was still running, and no old web view was alive. Reloading the same
page grows that page's WebContent process while JavaScriptCore keeps the
discarded heaps, and it levels off at about 616 MB from 50 reloads on; it is
not reclaimed until the page is replaced or the process is ended. A project
switch releases it completely. The app process stays flat.

Limits of this measurement: it drives the controller from a test process, not
the full app under Instruments for hours; the test window is not frontmost, so
WebKit may already treat the "visible" page as occluded (CPU barely moves in
either state); and it has no WebSocket/HMR server. Sustained use in the app
itself remains to be checked.
