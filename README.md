# RustCode Remote

A native iOS app that is a remote control for [RustCode](https://github.com/LHagfoss/rustcode),
a terminal coding agent. You run `/remote` in a RustCode session on your Mac, pair your
iPhone over the same LAN or a NetBird VPN, and then watch the session's live output and
continue it from the phone: send prompts, stop a turn, answer the agent's questions and
approve or deny the actions it asks permission for.

The session keeps running in the terminal on the Mac. The phone is a second screen and
keyboard for it; closing the app never cancels work.

## Status

The app speaks the real protocol, and the RustCode side is not finished yet.

- **Pairing and authentication work against a real gateway.** The app pairs with
  `rustcode remote serve` by QR payload or by address and code, stores the device token in
  the Keychain, authenticates with it on every later connection, and goes back to the
  pairing screen when the device is revoked. This is checked by an integration test that
  drives a real gateway over loopback (see [Tests](#tests)).
- **No session can be shared yet.** The gateway that exists today (RustCode PR #1912) is a
  foundation: it answers everything after the handshake with `not_implemented`. The app
  shows that as connected with an empty session list. The `/remote` command and session
  routing are still being built in RustCode.
- **The session protocol is implemented against its contract, not against a host.**
  Requests, responses and events are encoded and decoded from the JSON Schemas and the 56
  golden frames published in the RustCode repository. No host produces those frames over
  a socket yet, so the path from a live session to the transcript has only been exercised
  with the golden frames and the in-app demo host.
- **Demo mode** shows the whole interface against a simulated host, without a Mac.

## Pair with a host

On the Mac (a RustCode build that has `remote serve`):

```sh
rustcode remote serve --bind <address> --advertise <address>   # default: loopback, port 17879
rustcode remote pair
```

`remote pair` prints an address, an eight-digit code and a QR payload. In the app either:

- type the address (`192.168.1.20:17879`) and the code, or
- paste the QR payload (the `{"protocol_version":1,…}` line) into the address field, or
- scan it, once something draws it as a QR code. The terminal prints the payload as text
  only; the scanner accepts a QR code that encodes exactly that text.

A challenge lasts two minutes and works once. After too many wrong attempts the host locks
pairing for a few minutes and the app says for how long. `rustcode remote devices` lists
paired devices and `rustcode remote revoke <device>` removes one.

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
| `-demoSession approval\|question\|idle` | Open that demo session directly. |
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

The package builds and tests on the Mac, without a simulator:

```sh
cd RustCodeRemote
swift test
```

That includes the contract tests: every golden request and frame is decoded, encoded
again and compared as JSON (the round trip RustCode's own tests run), every golden file
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

The integration tests need a real gateway and are skipped unless you name one:

```sh
RUSTCODE_GATEWAY_BIN=/path/to/rustcode swift test --filter GatewayIntegrationTests
```

They start `rustcode remote serve` on loopback in a temporary configuration directory and
pair by QR payload and by code, authenticate, check the empty session list, revoke the
device and watch the connection close, restart the gateway, and trip the pairing rate limit.

The app:

```sh
xcodebuild -project RustCode.xcodeproj -scheme RustCode \
  -destination 'generic/platform=iOS Simulator' build CODE_SIGNING_ALLOWED=NO
```

`CODE_SIGNING_ALLOWED=NO` is fine for checking that it compiles, but an app built that
way has no keychain access and reports a keychain error at launch. To run it in the
Simulator, build without that flag (or just press Run in Xcode).

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

The handshake frames (`pair`, `authenticate`, `paired`, `authenticated`, `error`) are
provisional on the RustCode side and are expected to move into the versioned envelope. That
change is contained in `Wire/WireHandshake.swift` and `WireCodec`.

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
