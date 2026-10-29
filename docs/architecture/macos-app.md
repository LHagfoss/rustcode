# macOS app architecture

The SwiftUI app in `macos/` is a native front end to the Rust engine. It runs
`rustcode --acp` as a child process and talks ACP v1 to it over stdio
([ACP](../acp.md)). This page maps where things live and which way the
dependencies point; build and packaging are in
[install.md](../install.md#swiftui-macos-app). Line counts below are
approximate and only meant to show weight.

## Targets and dependency direction

`macos/Package.swift` declares three source targets and three test targets:

```text
RustCode (executable: SwiftUI, AppKit, WebKit)
├── RustCodeTerminal (PTY shells, SwiftTerm)
│   └── RustCodeKit
└── RustCodeKit (Foundation, no UI frameworks)
```

| Target | Owns | Does not |
| --- | --- | --- |
| `RustCodeKit` (`Sources/RustCodeKit`, ~60 files) | JSON-RPC framing (`JSONRPC.swift`), the ACP peer (`ACPConnection`), the engine child process (`EngineProcess`, `EngineLocator`), protocol types (`ACPTypes.swift`: `ACPMethod`, `ACPRequests`, `SessionUpdate`, `PermissionRequest`, `SessionSummary`, `ModelOptions`), the `Transcript` model, and pure policy for chat follow/scroll (`ChatFollow`, `ChatScrollMap`, `TranscriptWindow`), find (`ChatFind`), tool folding, diffs, markdown blocks, session paging/search/retention, subagents, preview access (`PreviewAgentGate`, `LocalPreview`), Smart UI and visualization parsing. | Import SwiftUI, AppKit or WebKit. Everything here is unit-testable without a window or a process. |
| `RustCodeTerminal` (`Sources/RustCodeTerminal`) | The workspace terminal's PTY sessions: `TerminalSession`, `TerminalSessionManager` (shells grouped per project), `TerminalLaunch`, `ProcessTree`, `InlineCommand`. | Talk to the engine. It uses `RustCodeKit` only for `EngineLocator`'s login-shell environment. Kept apart so the engine client does not depend on SwiftTerm. |
| `RustCode` (`Sources/RustCode`, ~40 files) | The app: `RustCodeApp` (entry, menus), `DesktopModel`, all views, `PreviewController` (WKWebView), `TerminalController`, `Updater`, `KeybindingStore`, `TurnNotifier`, and the debug-only `DebugDriver`. | Hold logic that can be a pure function; that goes to `RustCodeKit`. |

The rule of thumb: a type that needs a view, a window, WebKit or the user's
defaults belongs in `RustCode`; a decision that can be stated over values
belongs in `RustCodeKit`, with a test. `RustCodeKit` is an internal library of
this package, not a published or stable SDK.

## Runtime and data flow

```text
rustcode --acp (child process)
   │ stdout lines                         ▲ stdin lines
   ▼                                      │
EngineProcess ── LineFramer ──► ACPConnection (actor)
                                   │ request/response by id
                                   │ inbound: AsyncStream<ACPInbound>
                                   ▼
                 DesktopModel.handle(_:)  (@MainActor, RustCodeApp.swift)
                   │ SessionUpdate → Transcript.apply
                   ▼
   LiveTranscript (shown chat) / OpenSessions (background chats, replays while loading)
                   │ objectWillChange, at most every 33 ms
                   ▼
   ContentView → ChatView → ChatScroller → TranscriptRow (TranscriptRows.swift)
```

1. **Launch.** `RustCodeApp` creates one `DesktopModel` as a `@StateObject`
   and calls `model.start()` from `ContentView`'s `.task`. `start()` resolves
   the binary with `EngineLocator` (Settings path, `RUSTCODE_ENGINE`, the
   bundled copy, then a CLI install), starts `EngineProcess` in the default
   project, spawns the inbound loop, sends `initialize` (giving `AgentInfo`
   capabilities), then `initialized`, lists sessions and restores model
   options.
2. **Process lifecycle.** `EngineProcess` documents its contract at the top of
   `EngineProcess.swift`: stdout is ACP only, stderr is a bounded tail,
   `stop()` closes stdin first and escalates to SIGTERM/SIGKILL, and any exit
   closes the connection after the stdout already read is delivered. An
   unrequested exit ends all turns, drops the engine and shows an error;
   changing the default project restarts the engine (`restartForWorkspace`).
3. **Requests.** The model calls `connection.request(ACPMethod.…, params:
   ACPRequests.…)` for `session/new`, `session/load`, `session/prompt`,
   `session/cancel`, config options and the `_rustcode/*` extensions (steer,
   agents, rewind, session move, remote).
4. **Inbound.** `ACPConnection.inbound` delivers notifications and
   agent-initiated requests in order. `DesktopModel.handle(_:)` routes them:
   - `session/update` for the shown chat goes to `LiveTranscript`; for a chat
     open in the background, to its `OpenSessions` snapshot; for a chat being
     loaded, to its off-screen replay (`OpenSessions.route`). Report/command answers
     (`PendingReports`, `/context`) are diverted to sheets and toasts;
     `stopped` and `notice` become toasts or banners.
   - `session/request_permission` appends to `permissionQueue` (rendered by
     `AttentionCards.swift`), with a toast and a system notification when the
     chat is not on screen.
   - The preview request (`ACPMethod.preview`) opens the preview panel and is
     answered off the inbound loop by `PreviewController.runAgentRequest`
     (`PreviewAgentBridge.swift`, gated by `PreviewAgentGate`).
5. **Views.** `ContentView.swift` composes the window: `SessionSidebar`,
   `ChatView` (transcript, find bar, composer), the terminal drawer, the
   preview panel, and overlays for Settings, the session switcher, sheets and
   toasts. Transcript rows live in `TranscriptRows.swift`; markdown in
   `MarkdownText.swift`.

Where the side surfaces attach:

| Surface | Model hook | Files |
| --- | --- | --- |
| Terminal | `DesktopModel.terminal` (`TerminalController`), follows `chatProject` | `TerminalController.swift`, `TerminalPanel.swift`, `RustCodeTerminal` |
| Preview | `DesktopModel.preview` (`PreviewController`), `showsPreview` | `PreviewController.swift`, `PreviewPanel.swift`, `PreviewAgentBridge.swift`; see [preview](../preview.md) |
| Settings | `showsSettings`, `settingsSection` (a page over the window) | `SettingsView.swift`, `ConnectionsSettings.swift`, `CapabilitiesSettings.swift`, `Keybindings.swift` |
| Subagents | `subagents`, `backgroundAgents`, `viewedAgent` (`_rustcode/agents` polling) | `AgentViews.swift` (`AgentPanel`), `RustCodeKit/Subagents.swift` |
| Smart UI / charts | `engineSmartUI` | `SmartUIView.swift`, `VisualizationView.swift`; see [Smart UI](../smart-ui.md), [visualizations](../visualizations.md) |
| Remote | `showsRemote`, `remoteStatus` | `RemotePanel.swift`, `RustCodeKit/RemoteShare.swift`; see [remote](../remote.md) |

## State ownership

`DesktopModel` (`RustCodeApp.swift`, ~2,500 of its ~2,700 lines) is the one
coordinator: engine and connection, session list, selection, per-chat drafts
and queues, permissions, turn activity, model options, toasts and panel flags.
Views observe it directly. Narrower publishers exist so high-frequency changes
do not redraw every view that watches the model:

| Publisher | Owner | Why it is separate |
| --- | --- | --- |
| `LiveTranscript` | `DesktopModel.chat` | Streamed chunks arrive faster than frames; it announces at most once per 33 ms while reads always see the latest transcript. Only `ChatView` observes it. |
| `ComposerDraft` | `DesktopModel.composer` | A keystroke redraws the composer, not the window. |
| `ChatFind` (RustCodeKit) | `DesktopModel.find` | Find-in-chat query, matches and the current hit, apart from the transcript. |
| `ChatScrollState`, `DrawnRowsState`, `StickyTurnState` | `ChatScroller.swift`, held by `ChatView` | Row measurements and the scroller knob; measuring must not redraw the chat. |
| `FollowState` | `ContentView.swift`, held by `ChatView` | Whether the chat follows the stream; publishes only following/latest changes. |
| `TerminalSessionManager` | `TerminalController` | Shells per project. |

Invariants worth knowing before touching session code. `SessionFlowTests`
covers the switching ones end to end (`testSwitchingAwayMidLoadKeepsTheChat`,
`testReturningToALoadingChatShowsItWhenLoaded`, `testDraftsStayWithTheirChat`);
the stale-engine checks have no direct test.

- **One load per chat.** The engine refuses a second `session/load` of an open
  session, so switching back restores the `OpenSessions` snapshot instead.
  `select(_:)` stashes the outgoing chat (`stashSelectedSession`) and parks its
  draft (`parkDraft`/`restoreDraft`) before showing the next one.
- **Replay is built off screen.** `loadInEngine` collects the replayed
  `session/update`s into the chat's `OpenSessions` replay and shows the whole
  transcript at once; loads are per chat, so picking another chat mid-load
  keeps the first one's replay.
- **Responses can overtake updates.** A response resolves ahead of updates
  queued before it, so `catchUpInbound` waits until `handledInbound` reaches
  the connection's `inboundCount` before a load or a finished turn reads the
  transcript.
- **Stale engines are ignored.** Every async continuation checks
  `engine?.connection === connection` (or `self.engine === launched`) so a
  replaced engine's late messages or exit do not touch the new one's state.
- **Selection can change across an `await`.** After any await, code rechecks
  `selectedSession == id` before writing to the shown transcript.
- **Queued prompts stay with their chat.** `QueuedPrompt` carries its session;
  prompts queued while a new chat starts move to it (`adoptQueued`,
  `QueueTests`).

## Test map

Run with `scripts/swift-test.sh [TestClassOrName]` (it filters the test host's
Contacts noise and fails a filter that matches nothing; `RustCodeTests.X`
matches nothing, use the class name).

| Target | Covers | Examples |
| --- | --- | --- |
| `RustCodeKitTests` (~370 tests) | Protocol and policy without a process or window: framing and ACP types, transcript application, follow/scroll math, find, tool folding, diffs, markdown, session paging/search/retention, subagents, preview gating, Smart UI parsing. | `ProtocolTests`, `TranscriptTests`, `ChatFollowTests`, `ChatScrollMapTests`, `ToolFoldingTests`, `PreviewAgentTests` |
| `RustCodeTerminalTests` (~30 tests) | PTY sessions, launch environment, inline `!` commands. | `TerminalSessionTests`, `TerminalLaunchTests`, `InlineCommandTests` |
| `RustCodeTests` (~80 tests) | App-target behavior that needs AppKit, WebKit or `DesktopModel`: session flows against a scripted fake engine, preview web-view lifecycle and agent bridge, Smart UI web content limits, keybindings, follow state, What's New flow. | `SessionFlowTests`, `PreviewControllerTests`, `PreviewSoakTests`, `SmartUISoakTests`, `KeybindingTests`, `FollowStateTests` |

```bash
scripts/swift-test.sh                  # everything
scripts/swift-test.sh SessionFlowTests # DesktopModel against a fake engine
scripts/swift-test.sh ChatFollowTests  # pure follow policy
```

For behavior in the real window, drive the debug build with `DebugDriver`
(see `AGENTS.md`, "Checking macOS app behavior").

## Refactor and reuse assessment

**Already extracted.** The parts most worth reusing and testing are already in
`RustCodeKit`: the ACP client and process, the transcript model, scroll and
follow math (`ChatFollow`, `ChatScrollMap`, `TranscriptWindow`), find, tool
folding, preview access policy, and session paging. The terminal is its own
target. There is no second consumer of these targets today (the iOS app in
`ios/` is a separate project), so extracting more as a standalone package
would add release overhead without reuse.

**Large files, kept as they are.** `ContentView.swift` (~2,950 lines: window
composition, sidebar, `ChatView` ~600 lines, `Composer` ~400 lines, pickers)
and `TranscriptRows.swift` (~1,450 lines) are large but cohesive view code
that already reads its decisions from `RustCodeKit`. Splitting them into more
files would move code without making anything testable, and `ChatView`'s
scroll and stream handling is timing-sensitive. Not worth doing on its own.

**Extracted: the open-session cache.** The bookkeeping behind the
invariants above lives in `OpenSessions` (RustCodeKit): background chats'
snapshots, replays of chats being opened, the per-chat loading set, and
`route`, which says whether a `session/update` goes to a replay, the shown
chat or a background chat. `DesktopModel` holds one as `openChats` and keeps
the async side: the `session/load` request, `catchUpInbound` and the engine
identity checks. `OpenSessionsTests` covers the races directly (switch
mid-load, update for a background chat, forget on trash or move, reload after
the engine is replaced); `SessionFlowTests` still covers them end to end
(#2602).

**Out of scope.** Splitting `DesktopModel` into several observable objects,
or moving chat scrolling out of `ChatView`, would touch session switching,
replay and streaming at once. Do it only with a measured problem and a staged
plan, as the Rust crate split (#869) did.
