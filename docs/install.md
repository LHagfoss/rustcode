# Installing and updating

The one-line installers and Homebrew are covered in the [README](../README.md#install).
This page covers supported platforms, building from source, the desktop app and
updating.

## Supported platforms

Official release binaries are published for Linux x86_64, Linux ARM64
(aarch64, from v0.79.0), macOS Apple Silicon (ARM64), and Windows x86_64.
Linux binaries need glibc (Arch, Arch Linux ARM, Debian, Ubuntu, Fedora);
musl distributions such as Alpine build from source. Intel macOS is not supported by the prebuilt
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
CLI archives. The bundle is ad-hoc signed but not notarized yet, so macOS
Gatekeeper warns when opening a downloaded copy; right-click it and choose
Open, or clear the quarantine flag with
`xattr -d com.apple.quarantine RustCode.app`. The release workflow switches
to Developer ID signing and notarization once its signing secrets are set
(see [macOS release signing](macos-release-signing.md)); this workaround is
removed from here with the first notarized release.

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
Retry, and the last URL is remembered per project. Showing a page gives the
agent nothing: it can inspect, screenshot or drive the preview with the
`localhost_preview` tool only after you choose Allow for This Chat in the
panel's prompt, only on the approved localhost origin, and only until you click
the Agent button in the preview's toolbar to revoke it, switch project or
restart the engine. Details and limits are in [docs/preview.md](preview.md).

For your own commands, open the terminal with ⇧⌘T (View > Show Terminal, the
terminal button in the chat's title bar, or `/terminal` in the composer, which
works even while the engine is not running). It is a resizable drawer under the
chat running your login shell (`$SHELL`, else the account's shell) on a real
PTY, so interactive programs, colors, Unicode, resizing and Ctrl-C behave as in
Terminal. It starts with the same environment the app gives the engine (the
usual tool directories appended to `PATH`, plus `TERM=xterm-256color` and a
UTF-8 locale when none is set), and your shell profile runs as a login shell.

- Terminals belong to the project they were opened in and start in its folder;
  the panel header shows the project and directory. Switching projects shows
  that project's terminals only: the others keep running, are never sent input
  and never move. A project folder that no longer exists is reported instead of
  starting the shell elsewhere.
- Use + (or View > New Terminal Tab) for more tabs, up to 8 per project. A shell
  that exits shows Relaunch; closing a tab ends its shell and everything it
  started, after asking if something is still running. Hiding the panel never
  stops a process.
- Quitting RustCode hangs up every terminal and kills what remains after a
  short grace period, so dev servers do not outlive the app.
- Each terminal keeps 10,000 lines of scrollback; older lines are dropped.
- A dev server started here works with the preview: click its
  `http://localhost:PORT` link to load it in the preview (other web links open
  in the browser), or type the address there. The preview works without the
  terminal.
- Start a composer message with `!` to run it in the terminal instead of
  sending it to the agent: `! ls` (or `!ls`) types `ls` into the open project's
  selected terminal tab and presses Return, so it uses that shell's folder,
  environment and history, and its output appears there. It runs at once, even
  while the chat is working. A line you had half typed in that shell is cleared
  first (⌃Y brings it back in zsh and bash). With no terminal one is opened; an
  exited shell is started again. If a program holds the terminal (an editor, a
  REPL, a dev server not sent to the background) the command is refused rather
  than typed into it; open another tab or finish the program. A lone `!` and
  commands of more than one line are refused. To send the agent a message that
  starts with `!`, begin it with `!!` (`!!important` sends `!important`). Only
  you can issue these: the agent never runs `!` commands, and their output is
  not sent to it.

The terminal is not an agent tool. What you type runs directly as you, without
the engine's sandbox or tool approvals, and the agent never sees its input or
output. The agent's own shell commands still go through the engine, its
sandbox and its permission prompts. The emulator is
[SwiftTerm](https://github.com/migueldeicaza/SwiftTerm) (MIT, pinned in
`macos/Package.swift`; its license ships in the app's `Contents/Resources`).


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
