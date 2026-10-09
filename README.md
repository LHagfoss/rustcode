# RustCode Remote

A native iOS app that is a remote control for [RustCode](https://github.com/LHagfoss/rustcode),
a terminal coding agent. You run `/remote` in a RustCode session on your Mac, pair your
iPhone over the same LAN or a NetBird VPN, and then watch the session's live output and
continue it from the phone: send prompts, stop a turn, answer the agent's questions and
approve or deny the actions it asks permission for.

The session keeps running in the terminal on the Mac. The phone is a second screen and
keyboard for it; closing the app never cancels work.

## Status

**The app cannot reach a real RustCode host yet.** The RustCode gateway (WebSocket,
pairing, the wire protocol) is still being built, and its JSON schema is not final. This
repository contains everything that does not depend on the exact wire shapes:

- the full UI, running against a simulated host in **demo mode**;
- the domain model, the state store, reconnect policy and receipt handling;
- the connection plumbing over `URLSessionWebSocketTask`, tested against a scripted
  socket and a loopback WebSocket server.

What is missing is the codec that translates between protocol frames and the domain
types (see [Where the wire codec plugs in](#where-the-wire-codec-plugs-in)). Until it
lands, "Pair" and "Scan Pairing Code" end with "Connecting to a real host is not available
in this build yet."

## Requirements

- A Mac with Xcode 26 or later (the project was built with Xcode 26.6, Swift 6.3).
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
  a command. Approve or deny it and the turn continues accordingly.
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
  Info.plist               Camera, local-network and App Transport Security keys.
RustCodeRemote/            Local Swift package, no dependencies, iOS 18 / macOS 15.
  Sources/RustCodeRemote/
    Models/                Host, session, transcript, attention, receipts, errors.
    Transport/             RemoteTransport, URLSessionWebSocketTransport, ScriptedTransport.
    Credentials/           CredentialStore, Keychain and in-memory implementations.
    Client/                RemoteClient, LiveRemoteClient, RemoteCodec (the wire boundary).
    Store/                 RemoteStore (@Observable), reconnect policy, attention badges.
    Demo/                  DemoRemoteClient.
    Presentation/          MarkdownBlock.
  Tests/RustCodeRemoteTests/
docs/screenshots/          Simulator screenshots of the pairing screen and the demo.
```

### Tests

The package builds and tests on the Mac, without a simulator:

```sh
cd RustCodeRemote
swift test
```

The app:

```sh
xcodebuild -project RustCode.xcodeproj -scheme RustCode \
  -destination 'generic/platform=iOS Simulator' build CODE_SIGNING_ALLOWED=NO
```

`CODE_SIGNING_ALLOWED=NO` is fine for checking that it compiles, but an app built that
way has no keychain access and reports a keychain error at launch. To run it in the
Simulator, build without that flag (or just press Run in Xcode).

## Where the wire codec plugs in

Views talk only to `RemoteStore`. The store talks only to the `RemoteClient` protocol.
`LiveRemoteClient` implements that protocol on top of two things it is given:

- a `RemoteTransport` (text frames over a WebSocket), and
- a `RemoteCodec`, declared in `RustCodeRemote/Sources/RustCodeRemote/Client/RemoteCodec.swift`.

`RemoteCodec` is the boundary. It turns a `RemoteCommand` into a frame, a frame into a
`RemoteMessage`, a scanned QR payload or a typed address and code into a `PairingTarget`,
and a host address into the WebSocket URL and headers. Every type on the app's side of it
is a domain type with no knowledge of JSON.

Today the app is constructed with `UnavailableRemoteCodec`, which fails every call with
`RemoteError.codecUnavailable`. The next step is a schema-backed implementation with its
`Codable` DTOs in `RustCodeRemote/Sources/RustCodeRemote/Wire/`, built against the JSON
Schema and golden frames that the RustCode repository will publish under
`docs/remote-protocol/`, and one changed line in `RustCode/RustCodeApp.swift`.

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
else. In particular it does **not** set `NSAllowsArbitraryLoads`.

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
