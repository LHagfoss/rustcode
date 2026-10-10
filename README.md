# RustCode Remote

A native iOS app that is a remote control for [RustCode](https://github.com/LHagfoss/rustcode),
a terminal coding agent. You run `/remote` in a RustCode session on your Mac, pair your
iPhone over the same LAN or a NetBird VPN, and then watch the session's live output and
continue it from the phone: send prompts, stop a turn, answer the agent's questions and
approve or deny the actions it asks permission for.

Start a session in a folder on your Mac from the phone, or control an existing terminal
session. App-created sessions run in the background; closing the app never cancels work.

## Status

The app speaks remote protocol v1 and works against a real RustCode
host. What that means, precisely:

- **Checked end to end on one Mac, over loopback.** An integration test starts a real
  gateway and a real RustCode terminal session, shares the session with `/remote`, and
  drives it with this package's client: pair, list, attach, submit a prompt, follow the
  streamed response event by event, queue a second prompt, look up request receipts,
  reconnect and resume, `/remote off`, revoke (see [Tests](#tests)). The model behind the
  session in that test is a local stand-in that streams a fixed reply.
- **Not yet checked from a phone over a network.** Pairing an iPhone with a Mac over a LAN
  or NetBird, the local-network permission prompt and QR scanning with the camera have
  not been exercised; they need a person holding the phone.
- **Questions and cancellation** are also tested through a real terminal session and
  gateway: a local stand-in model calls `ask_question`, the client answers the first
  question, cancels the next, and starts a fresh turn. Slash commands are checked without
  adding model prompts. Approvals, general tools and sub-agents retain golden-frame and
  demo coverage.
- Use a current RustCode build with remote session sharing and timing metadata.
  The checked contract revision is recorded in the package's `Resources/remote-protocol/SOURCE.txt`.

## Pair with a host

Connect the Mac and iPhone to the same Wi-Fi network. In the RustCode terminal session
you want to share, run:

```
/remote
```

RustCode starts the gateway automatically and selects a reachable Mac LAN address.
You do not need to look up an IP address or start `remote serve` for this same-network flow.
If the Mac has no reachable LAN address, RustCode explains how to provide one explicitly.

For access away from home, connect both devices to the same NetBird VPN and explicitly
choose the Mac's NetBird address:

```sh
rustcode remote serve --bind <Mac NetBird address>
```

Then run `/remote` in the session and scan its code. NetBird encrypts the connection;
the automatic LAN flow is for a network you trust (see [Network security](#network-security)).

`/remote` shows a QR code, the address and an eight-digit code. In the app tap **Scan
Pairing Code** and point the camera at the QR code. The first time, iOS asks for camera
access and, on a LAN, for local-network access; allow both. Without a camera, type the
address (`192.168.1.20:17879`) and the code instead. Nearby Macs also appear automatically
on the same Wi-Fi: choose one and enter its pairing code. Native Paste buttons accept an
address/QR payload or the pairing code. The app remembers the last successfully paired
address, never the one-time code.
The scanner shows a square target and offers a retry if camera startup fails.

The session appears in the app's list as soon as it is shared. `/remote off` stops
sharing it; closing the terminal ends it. `rustcode remote devices` lists paired devices
and `rustcode remote revoke <device>` removes one, which sends the app back to the
pairing screen.

A pairing code lasts two minutes and works once. If pairing reports an expired or invalid
code, run `/remote` or `rustcode remote pair` again and scan the fresh code. After too many
wrong attempts the host locks pairing for a few minutes and the app says for how long.

## Session controls and input

The composer combines the current model and thinking effort in one control. Tap it
for a native sheet with model search and an effort picker. Changes become available
when the turn is idle and appear after host confirmation. The session menu opens a
sidebar for finding and switching chats without backing out of the current session.
On iPhone, chat stays the main screen even before a session is selected. The sessions
drawer pushes the whole chat screen sideways, keeps search above the list, and closes
by selection, a left swipe, or a tap on the exposed chat. The composer uses Liquid Glass
on iOS 26 and a translucent material on earlier versions.
After updating RustCode, the background gateway follows the installed binary. Reopen
older terminal sessions once to load new session capabilities; app-created sessions
use the current host binary.
Blank titles use `Session <short ID>`. Turn totals come from the complete host session;
unknown totals (including some compacted sessions) are omitted.

Thinking durations below one second use milliseconds. Thinking-token counts use `~`
when the host identifies them as estimates. Missing usage is not presented as zero.
Touch and hold a response for a compact Copy/Select Text/Share menu; Select Text opens
an expanded native text view. Drag the transcript down to dismiss the keyboard
interactively, or tap its content. Opening the keyboard preserves your reading position.

Questions distinguish selected options from a written answer. Switching modes preserves
the written draft; a failed submission leaves the answer available to retry. Chained
questions advance in the same sheet. Cancel questions ends the chain and turn, matching
Esc in the terminal. Approval failures keep their sheet open. Unconfirmed actions occupy
one review row, and repeated Stop taps cannot resend the same cancellation.

Tap **New Session** in the sessions list to start in your Mac's Home folder, browse a
project directory, or enter an absolute path. Turn totals appear on session rows.

Type `/` for the host's complete terminal command catalog in a compact, scrollable list.
Typing a prefix shows an inline completion; tap a suggestion to fill the draft. `/new [folder]`, `/history`, `/model`, `/models`, and `/copy` have native
app actions. Informational commands and supported session settings run on the host,
with results displayed outside model history. `/cancel` targets the current question
or turn. Commands that still require the Mac interface are marked **On Mac** and keep
your draft when selected; listing a command does not imply mobile execution support.

The `+` button opens an anchored glass menu with **Camera**, **Photos**, and **Files**,
plus **New Session**, **Slash Commands**, and **Choose Folder**. Files accepts images;
other document uploads are not supported by the host. Attach up to four images,
preview or remove them, then send with or without a written message. Images are downsampled to 2048 pixels,
converted to JPEG without location metadata, and uploaded to the paired Mac before the
prompt is submitted. Upload failures preserve the draft. This requires RustCode 0.64.5
and a vision-capable model or a configured vision fallback. Internal attachment paths
appear as attachment labels in chat; copying retains the original message.
The model and effort picker sits on the right of the composer, beside send, queue or stop.

Effort choices come from the host:
explicit provider catalogs take precedence, while legacy profiles with reasoning-effort
support offer the terminal's low, medium and high choices. Settings change while idle.

Once remote sharing is enabled, installed macOS builds manage a per-user background
service that restarts at login and follows upgrades. `rustcode remote stop` explicitly
disables that service. QR/manual pairing and explicit NetBird addresses remain available;
automatic discovery is limited to the local network.

## Requirements

- A Mac with Xcode 26 or later. The app icon is an Icon Composer file, which older Xcode
  versions cannot build. Built and tested with Xcode 27.0 (Swift 6.4); Xcode 26.6 also builds it.
- An iPhone or iPad on iOS 18 or later, and a cable for the first install.
- An Apple ID. A free one is enough; no paid developer membership is needed, and the app
  uses no capability that would require one (no push, no iCloud, no associated domains).

There are no third-party dependencies.

## Install on your iPhone with a free Apple ID

1. Open `RustCode.xcodeproj` in Xcode.
2. Add your Apple ID if Xcode does not have it yet: **Xcode > Settings > Accounts > +**.
3. Select the **RustCode** project in the navigator, the **RustCode** target, then the
   **Signing & Capabilities** tab. Keep **Automatically manage signing** on and choose
   your **Personal Team** under **Team**.
4. If Xcode reports that the bundle identifier is not available, change **Bundle
   Identifier** to something of your own, for example `com.yourname.rustcode`. Bundle
   identifiers are global, and `com.lhagfoss.rustcode` can only belong to one team.
5. On the iPhone, turn on Developer Mode: **Settings > Privacy & Security > Developer
   Mode**, then restart when asked. The switch only appears after the phone has been
   connected to Xcode once.
6. Connect the iPhone, unlock it, tap **Trust** if it asks, and pick it as the run
   destination in Xcode's toolbar.
7. Press **Run** (⌘R). The first launch fails with an "untrusted developer" message. On
   the phone open **Settings > General > VPN & Device Management**, tap your Apple ID
   under **Developer App**, and tap **Trust**. Run again.

Things to know about a free account:

- **The app stops opening after 7 days.** The signing profile of a free account expires
  after a week. Connect the phone and press Run again to re-sign; your pairing is kept.
- You can have a limited number of apps and devices signed this way at a time.

### Keeping your team ID out of git

Choosing a team in the Signing tab writes your team ID into `project.pbxproj`. That
works, but it shows up as a local change. To avoid it, put your settings in a file that
git ignores instead:

```sh
cp Config/Local.xcconfig.example Config/Local.xcconfig
```

and set `DEVELOPMENT_TEAM` (and `PRODUCT_BUNDLE_IDENTIFIER` if you changed it) there.
`Config/Base.xcconfig` is committed and includes `Local.xcconfig` when it exists.

## Demo mode

Tap **Try the Demo** on the first screen. The app connects to a host simulated inside
the app (`DemoRemoteClient`); nothing leaves the device and nothing is stored. It has
three sessions:

- **Add reconnect backoff** streams text, runs two tools, then asks for approval to run
  a command. The action arrives as a preview and the sheet fetches the rest before
  Approve unlocks, as it would for a long action from a real host. Approve or deny it and
  the turn continues accordingly.
- **Page the session store** is waiting on a question with three options.
- **Draft release notes** is idle, with a sub-agent row, a tool row, formatted text and
  one page of earlier history. Send it a message to see a turn; send another while it
  streams to see it queued; press stop to cancel.

Leave the demo under **Settings > Exit Demo**.

Launch arguments, for the Simulator or the scheme's Run arguments:

| Argument | Effect |
| --- | --- |
| `-demo` | Start in demo mode. |
| `-demoSession approval\|question\|questions\|idle` | Open that demo session directly. |
| `-demoSheet` | Present the session's pending question or approval when it appears. |

```sh
xcrun simctl launch booted com.lhagfoss.rustcode -demo -demoSession question -demoSheet
```

## Project layout

```
RustCode.xcodeproj/        One app target, shared scheme. Hand-written; the app folder is
                           a file-system-synchronized group, so new files are picked up
                           without editing the project.
Config/
  Base.xcconfig            Bundle identifier and signing defaults (committed).
  Local.xcconfig.example   Template for your team ID (Local.xcconfig is ignored).
RustCode/                  The SwiftUI app.
  RustCodeApp.swift        Entry point; wires the store, scene phase and launch arguments.
  Views/                   Pairing, QR scanner, session sidebar, transcript, composer,
                           question and approval sheets, settings.
  AppIcon.icon             RustCode's icon (Icon Composer), copied from the RustCode repository.
  Assets.xcassets          Accent color and the logo shown on the pairing screen.
  Info.plist               App Transport Security. The usage descriptions are build settings.
RustCodeRemote/            Local Swift package, no dependencies, iOS 18 / macOS 15.
  Sources/RustCodeRemote/
    Models/                Host, session, transcript, attention, receipts, errors.
    Transport/             RemoteTransport, URLSessionWebSocketTransport, ScriptedTransport.
    Credentials/           CredentialStore, Keychain and in-memory implementations.
    Client/                RemoteClient, LiveRemoteClient, RemoteCodec (the wire boundary).
    Wire/                  Codable DTOs for the protocol and the handshake, and WireCodec.
    Store/                 RemoteStore (@Observable), reconnect policy, attention badges.
    Demo/                  DemoRemoteClient.
    Presentation/          MarkdownBlock.
  Tests/RustCodeRemoteTests/
    Resources/remote-protocol/   The contract: schemas, golden frames, SOURCE.txt.
Scripts/sync-protocol.sh   Refreshes the contract from a RustCode checkout.
docs/screenshots/          Simulator screenshots of the pairing screen, the demo and the icon.
```

### Tests

The shared Xcode scheme includes `RustCodeUITests`, covering pairing-field touch and
keyboard navigation, chat dismissal and draft preservation, sending, collapsible
thinking and Markdown tables. Choose an installed iPhone Simulator and run:

```sh
xcodebuild test -project RustCode.xcodeproj -scheme RustCode \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro'
```

Thinking text supplied by the host as `<think>…</think>` appears in collapsed **Thinking**
sections. Assistant replies render headings, inline formatting and links, lists and task
lists, quotes, tables, horizontal rules and fenced code. Code and tables scroll
horizontally when needed. Images and nested list indentation are not supported.

The package builds and tests on the Mac, without a simulator:

```sh
cd RustCodeRemote
swift test
```

That includes the contract tests: every golden request, frame and handshake frame is
decoded, encoded again and compared as JSON (the round trip RustCode's own tests run), every golden file
has to be covered, and the tags and enum values the DTOs know are compared with what the
schemas allow, so a change on the RustCode side shows up as a failing test here. The
Keychain store is tested against the login keychain of the Mac running the tests.

To pick up a new version of the contract:

```sh
Scripts/sync-protocol.sh /path/to/rustcode            # reads origin/main
Scripts/sync-protocol.sh /path/to/rustcode some-ref
```

It copies the schemas and golden frames from that commit into the test resources and
records the commit in `Resources/remote-protocol/SOURCE.txt`.

The integration tests need a real `rustcode` binary and are skipped unless you name one:

```sh
RUSTCODE_GATEWAY_BIN=/path/to/rustcode swift test --package-path RustCodeRemote
```

The default gateway/session fixtures run on explicit loopback in a temporary configuration
directory. To also verify automatic LAN selection and actual QR-payload pairing on a Mac
with a reachable LAN, set `RUSTCODE_LAN_PAIRING=1` with the same binary:

```sh
RUSTCODE_GATEWAY_BIN=/path/to/rustcode RUSTCODE_LAN_PAIRING=1 \
  swift test --package-path RustCodeRemote
```

The LAN check connects the package's live client to the address the gateway advertises;
it does not install or run the app on a physical phone.

The real-host suites cover:

- `GatewayIntegrationTests` pair by QR payload and by code, authenticate, revoke a device
  and watch the connection close, restart the gateway, and trip the pairing rate limit.
- `SessionEndToEndTests` also starts a RustCode terminal session in a pseudo-terminal,
  types `/remote` into it, and drives the shared session from the client as described
  under [Status](#status). The session's model is a small HTTP server in the test that
  streams a fixed reply, so no provider account is involved. Set `RUSTCODE_E2E_FRAMES` to
  a file path to get every frame of the run written out, with secrets redacted.

The app:

```sh
xcodebuild -project RustCode.xcodeproj -scheme RustCode \
  -destination 'generic/platform=iOS Simulator' build CODE_SIGNING_ALLOWED=NO
```

`CODE_SIGNING_ALLOWED=NO` is fine for checking that it compiles, but an app built that
way has no keychain access and reports a keychain error at launch. To run it in the
Simulator, build without that flag (or just press Run in Xcode).

The transcript shows host-reported response and thought time, logical-turn work time,
completion timestamps, and completed, cancelled or failed outcomes when available.
Work time excludes waits for user input and gaps between runs. Missing timings stay absent.
Reconnecting refreshes the bounded session snapshot so missed activity appears without
replaying historical haptics. New live turns and attention requests give feedback while
the chat is visible and the app is active.

## The wire boundary

Views talk only to `RemoteStore`. The store talks only to the `RemoteClient` protocol.
`LiveRemoteClient` implements that protocol on top of two things it is given:

- a `RemoteTransport` (text frames over a WebSocket), and
- a `RemoteCodec`, declared in `RustCodeRemote/Sources/RustCodeRemote/Client/RemoteCodec.swift`.

`WireCodec` (`Sources/RustCodeRemote/Wire/`) is the codec for remote protocol v1 and the
gateway's handshake frames. The `Wire…` types mirror the JSON exactly; `WireCodec` maps
them to and from the domain types, and nothing else in the package or the app sees them.

A newer host must not break an older app. A frame kind, operation, result, event, error
code or enum value this build does not know decodes to an explicit unknown instead of
failing: an unknown frame is ignored, an unknown result fails only its own request, and an
unknown event still takes its place in the sequence so the events after it apply. A frame
that is not valid protocol at all drops the connection, and reconnecting resynchronises
from a snapshot.

The handshake frames (`pair`, `authenticate`, `paired`, `authenticated`, `error`) have
their own schemas and golden files and are covered the same way. After the handshake every
reply is a response that names its request; an `error` frame without a request ID means
the gateway is closing the connection.

## Network security

The app declares `NSLocalNetworkUsageDescription`, so iOS asks for local-network access
the first time it connects to a host on your LAN, and `NSCameraUsageDescription` for
scanning the pairing code.

**A plain `ws://` connection is not encrypted.** Pairing authenticates the device; it
does not protect the traffic.

- **NetBird (addresses in `100.64.0.0/10`):** the WebSocket itself is plain, but it
  travels inside NetBird's WireGuard tunnel, which encrypts it between the two peers.
  This is the recommended way to connect.
- **Same LAN (`192.168.x.x`, `10.x.x.x`, `.local` names):** nothing encrypts the
  connection. Anyone who can observe that network can read the session, including the
  device token. Use it only on a network you trust.

The app shows which of the two applies next to the address, on the pairing screen and in
Settings.

### App Transport Security

`Info.plist` sets `NSAppTransportSecurity > NSAllowsLocalNetworking = YES` and nothing
else. The gateway refuses any WebSocket upgrade that carries an `Origin` header;
`URLSessionWebSocketTask` does not send one, and a test checks that. In particular it does **not** set `NSAllowsArbitraryLoads`.

iOS cannot scope an ATS exception to an IP range (`NSExceptionDomains` takes domain
names only), and to iOS a NetBird address is not a "local" address, so the open question
was whether `ws://100.x.y.z` needs a wider exception. It was measured rather than
assumed, on the iOS 26.5 Simulator, by connecting `URLSessionWebSocketTask` under four
ATS configurations (no keys, `NSAllowsLocalNetworking`, `NSAllowsArbitraryLoads`, both):

- ATS did not reject a `ws://` connection in any configuration: not to a LAN address,
  not to a `100.64.0.0/10` address, not to a `.local` name, and not even to a fully
  qualified public host name with no ATS keys at all.
- As a control, plain `http://` to that same qualified host name **was** rejected with
  the ATS error (-1022) when no keys were set, so ATS was being enforced; and plain
  `http://` to an IP literal was not rejected.

So no additional exception is needed for NetBird addresses, and the narrowest setting
that works is also the one in use. `NSAllowsLocalNetworking` is kept as the documented
statement of intent, and so that connecting does not hinge on the undocumented detail
that ATS leaves `ws://` alone.

Two honest limits of that measurement:

- It was made in the Simulator. It has not been repeated on a physical iPhone with a
  NetBird tunnel up. The connection to the `100.x` address was not rejected by ATS, but
  it could not be completed either, because the Mac used for the test could not reach
  its own NetBird address.
- If a real device turns out to block `ws://` to a NetBird address after all, the only
  setting that would lift it is `NSAllowsArbitraryLoads`, which disables ATS for every
  host. Note that iOS ignores `NSAllowsArbitraryLoads` whenever `NSAllowsLocalNetworking`
  is also present, so the two cannot simply be combined: it would mean replacing one
  with the other, and accepting that the app may then open unencrypted connections to
  any address.

Because ATS does not restrict where a `ws://` connection may go, the app itself is what
tells you when an address is neither local nor NetBird.

## GitHub releases

Pull requests run the remote package tests, device-helper fixture tests and a
Simulator build on macOS. After merging a version/build update, a matching
`v<VERSION>` tag on `main` runs the release workflow. It publishes a Simulator
app, an unsigned iOS archive and `SHA256SUMS`. Personal signing profiles never
enter those public artifacts; use the local development build below for your
registered iPhone.

## Build a local iPhone release

Keep personal signing settings in the ignored `Config/Local.xcconfig`. With an
existing Xcode development identity and provisioning profile, run:

```sh
Scripts/build-ios-release.sh
```

The helper reads the actual Release version/build settings and creates
`build/releases/v<VERSION>/RustCode.xcarchive`. Its installable app is inside
`Products/Applications/RustCode.app`. This directory is ignored by Git. The helper
verifies the app metadata and signature, stores a SHA256 source fingerprint,
preserves a previous archive when rebuilding, and uses cached signing without
updating Apple provisioning or installing anything.

This is a locally signed development archive for the devices included in its
profile. Keep it local: it contains personal signing/provisioning metadata.
Public unsigned or Simulator release artifacts support inspection and Simulator
testing; they cannot be installed directly on a physical iPhone. These helpers
do not provide App Store or TestFlight distribution.

## Install on your iPhone when ready

Connect and unlock the iPhone, trust this Mac, and enable Developer Mode. Run:

```sh
cd ~/code/rustcode_app && Scripts/install-on-iphone.sh
```

The helper selects the single connected, paired physical iPhone, reuses the
local archive only when its version, build, bundle, signature and source
fingerprint match, otherwise builds it, then installs over the existing app and
launches it. It does not uninstall the app, so existing app data and pairing are
retained.

The fingerprint covers app sources/assets, the Xcode project, remote package
sources/manifest, and shared and local build configuration. Local signing values
are hashed without printing their contents. Generated builds, user Xcode state,
documentation and tests are excluded. Editing app source triggers a rebuild even
if the version and build number have not changed.

If several phones are connected, choose one with
`Scripts/install-on-iphone.sh --device 'YOUR IPHONE NAME OR ID'`. Add `--no-launch`
to install without opening the app. The phone must be included in the cached
development profile. If signing fails, configure it in Xcode first; these helpers
do not register devices or request provisioning changes automatically.
