# Installing and updating

The one-line installers and Homebrew are covered in the [README](../README.md#install).
This page covers supported platforms, building from source, the desktop app and
updating.

## Supported platforms

Official release binaries are published for Linux x86_64, macOS Apple Silicon
(ARM64), and Windows x86_64. Intel macOS is not supported by the prebuilt
installer or Homebrew formula. Building from source may support additional
targets, but those targets are not covered by release CI.

## From Source (Rust / Cargo)

The `rustcode` binary lives in the `rustcode-tui` package; the repository root
is the engine library and has no binary target.

```bash
# Clone and build
git clone https://github.com/lhagfoss/rustcode.git
cd rustcode
cargo install --path rustcode/tui
```

`cargo install` writes to `~/.cargo/bin`. If a prebuilt `rustcode` from
`install.sh` is already on your `PATH` (it installs to `~/.local/bin`, or
`/usr/local/bin` when writable), remove it or ensure `~/.cargo/bin` comes
first — otherwise the older binary keeps winning:

```bash
rustcode --version   # check this resolves to the build you just made
```

## Native desktop app

The native GPUI app is a separate executable from the terminal UI. Build and
run it with Cargo, optionally passing a project directory (the current
directory is used by default):

```bash
cargo build -p rustcode-app
cargo run -p rustcode-app -- /path/to/project
```

On macOS, package the app with the Icon Composer icon from
`images/AppIcon.icon` (requires Xcode 26 or later):

```bash
scripts/build-native-app.sh            # target/debug/RustCode.app
scripts/build-native-app.sh --release  # target/release/RustCode.app
open target/debug/RustCode.app
```

The bundle includes both the layered icon for newer macOS versions and a
fallback `.icns`. Running `cargo run` launches the executable directly, so use
the bundled app to see its Dock and Finder icon.

Build the terminal executable separately with `cargo build -p rustcode-tui`.

### SwiftUI macOS app

The native SwiftUI app is a separate front end to the Rust engine. It launches
`rustcode --acp` as a child process; the reusable protocol client and process
management code live in the UI-independent `RustCodeKit` Swift package target.
Build and test it on macOS with Swift 6:

```bash
cd macos
swift build
swift test
```

To create a local app bundle using an existing executable engine:

```bash
cd ..
cargo build --release -p rustcode-tui
scripts/package-swift-app.sh target/release/rustcode
open target/swift-app/RustCode.app
```

Tagged releases publish `rustcode-macos-aarch64-swift-app.zip` alongside the
CLI archives. The bundle is ad-hoc signed but not notarized, so macOS
Gatekeeper warns when opening a downloaded copy; right-click it and choose
Open, or clear the quarantine flag with
`xattr -d com.apple.quarantine RustCode.app`.

Questions the agent asks (`ask_question`) appear as cards: one button per
option, ticks and Submit for multi-select questions, and a free-text answer.

The Swift app remembers the last project folder and can choose another from
its sidebar. Its workspace is sent as the ACP session working directory; the
engine's configured sandbox and permission policy still govern file and tool
access. Tool permissions are requested through ACP and the app never starts the
engine with `--yolo`.

To look at a web app the agent is building, open the localhost preview with
⇧⌘P (View > Show Preview, or the globe button in the chat's title bar). It is a
panel beside the chat that shows a running dev server in an embedded WebKit
view, with JavaScript, client-side routing and the server's assets working as in
a browser. Enter `http://localhost:PORT`, `127.0.0.1` or `[::1]` (a bare port
such as `5173` means localhost); other hosts are refused with an offer to open
them in the browser, and links to other sites open there too. Back, forward,
reload (⌘R) and Open in Browser are in its toolbar, an unreachable server shows
Retry, and the last URL is remembered per project. The preview is view-only:
the agent cannot see or control the page.


## Updating

RustCode comes with a built-in cross-platform self-updater for macOS, Linux, and Windows.
Native installations update from GitHub Releases; Homebrew installations use Homebrew.

- **In CLI:** Run `rustcode --update` (or `rustcode --upgrade`)
- **Inside RustCode TUI:** Type `/update` (or accept the update modal on startup)
- **Homebrew (macOS):** `brew upgrade rustcode`

### Updating pre-v0.31.0 native installs

Native binaries released before v0.31.0 predate the GitHub Release updater and
cannot bootstrap themselves without Homebrew. Reinstall once from the current
release, then `rustcode --update` will use the matching GitHub archive for
future upgrades:

```bash
curl -fsSL https://rustcode.lhagfoss.com/install.sh | bash
```

The installer verifies the downloaded archive with the release SHA256 manifest
before replacing the existing binary. Homebrew installations should instead
continue to use `brew upgrade rustcode`.
