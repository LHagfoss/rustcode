# Installing and updating

The one-line installers and Homebrew are covered in the [README](../README.md#install).
This page covers supported platforms, building from source, the desktop app and
updating.

The installers put the `rustcode` CLI on your machine and, unless you pass
`--no-app` (`-NoApp` on Windows), the RustCode desktop app as well: see
[Native desktop app](#native-desktop-app).

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

The one-line installers install the RustCode desktop app (`apps/desktop`,
Tauri) next to the CLI, natively per platform, and check every download
against the release's `SHA256SUMS`:

| Platform | Release asset | Installed as |
| --- | --- | --- |
| macOS (Apple Silicon) | `RustCode-<ver>-macos-aarch64.app.zip` | `/Applications/RustCode.app` (`~/Applications` when `/Applications` is not writable) |
| Windows x64 | `RustCode-<ver>-windows-x64-setup.exe` | per-user install in `%LOCALAPPDATA%\RustCode`, Start menu entry |
| Linux x86_64 | `RustCode-<ver>-linux-x86_64.AppImage` | `~/Applications/RustCode.AppImage`, menu entry and icon in `~/.local/share` |
| Linux x86_64 (`--deb`) | `RustCode-<ver>-linux-x86_64.deb` | system package via `apt-get`/`dpkg` (asks for sudo) |

The app carries its own engine (`rustcode --acp`) inside the bundle, so it
works without the CLI on `PATH`. Linux ARM64 and Intel macOS get the CLI
only.

On macOS the installer quits a running RustCode, moves an existing
`/Applications/RustCode.app` (including the SwiftUI app it replaces) to the
Trash rather than deleting it, unpacks the new one, checks its code signature
and bundle id, clears the quarantine flag on that verified app only and
registers it with Launch Services. The AppImage needs FUSE (`libfuse2`) to
run; the `.deb` does not, and also installs the engine as `/usr/bin/rustcode`.

Options (`curl … | bash -s -- <options>` on macOS and Linux):

| `install.sh` | `install.ps1` | Effect |
| --- | --- | --- |
| `--no-app` | `-NoApp` or `$env:RUSTCODE_NO_APP = "1"` | CLI only, as before |
| `--no-cli` | `-NoCli` or `$env:RUSTCODE_NO_CLI = "1"` | app only |
| `--deb` | | Linux: the `.deb` instead of the AppImage |
| `--bin-dir DIR` | | CLI to `DIR` instead of `/usr/local/bin` or `~/.local/bin` |

```bash
curl -fsSL https://rustcode.lhagfoss.com/install.sh | bash -s -- --no-app
```

```powershell
& ([scriptblock]::Create((irm https://rustcode.lhagfoss.com/install.ps1))) -NoApp
```

### Testing the installers without a release

Set `RUSTCODE_INSTALL_ARTIFACTS` to a directory holding release assets under
their release names plus a `SHA256SUMS`; the installers read from it instead
of GitHub. On a Mac:

```bash
cd apps/desktop && bun run bundle                      # release engine + RustCode.app
mkdir -p /tmp/rc/art /tmp/rc/cli
cp ../../target/release/rustcode /tmp/rc/cli/rustcode-macos-aarch64
tar czf /tmp/rc/art/rustcode-macos-aarch64.tar.gz -C /tmp/rc/cli rustcode-macos-aarch64
bun scripts/release-assets.ts /tmp/rc/art --sums       # app zip + SHA256SUMS
RUSTCODE_INSTALL_ARTIFACTS=/tmp/rc/art bash ../../scripts/install.sh --bin-dir /tmp/rc/bin
```

`--bin-dir` keeps the test CLI away from the one you use; `--no-cli` skips it.

### Updating the app

- macOS: Settings › Engine › **Check for Updates**, then **Update to X**. The
  app downloads the release's app zip, checks it against `SHA256SUMS`, its
  code signature and bundle id, then quits, swaps `RustCode.app` and reopens.
  It needs write access to the folder the app is in; otherwise the button
  opens the release page. Running the installer again works too.
- Windows and Linux: the button links to the release; run the installer
  again to update (it replaces the app in place).

### Uninstalling

- macOS: quit RustCode and move `/Applications/RustCode.app` to the Trash.
  Its data is in `~/Library/Application Support/com.lhagfoss.rustcode.desktop`,
  `~/Library/Caches/com.lhagfoss.rustcode.desktop` and
  `~/Library/WebKit/com.lhagfoss.rustcode.desktop`; threads and settings stay
  in `~/.config/rustcode`, shared with the CLI.
- Windows: Settings › Apps › Installed apps › RustCode › Uninstall (or
  `%LOCALAPPDATA%\RustCode\uninstall.exe`).
- Linux AppImage: delete `~/Applications/RustCode.AppImage`,
  `~/.local/share/applications/rustcode.desktop` and
  `~/.local/share/icons/hicolor/128x128/apps/rustcode.png`.
- Linux `.deb`: `sudo apt remove rustcode` (the package name `dpkg -l | grep -i rustcode` shows).

### Taking over from the SwiftUI app

The Tauri app ships with the SwiftUI app's bundle id,
`com.lhagfoss.rustcode.desktop`, so the Dock pin, Launch Services, Spotlight
and notification permissions carry over. Their data does not clash: the
SwiftUI app keeps its files in `~/Library/Application Support/RustCode` and
its preferences in `com.lhagfoss.rustcode.desktop.plist`, which the Tauri app
does not read; the Tauri app writes to
`~/Library/Application Support/com.lhagfoss.rustcode.desktop`, and its web
view's storage (origin `tauri://localhost`) is separate from the localhost
pages the SwiftUI preview stored in the shared WebKit folder. Reinstalling
the SwiftUI app later finds its own data untouched. A `tauri dev` build keeps
the identifier `dev.rustcode.desktop` (`tauri.conf.json`); only the bundle
(`tauri.bundle.conf.json`) takes the release identity, so development data
stays apart from the installed app.

Releases publish the macOS app zip a second time as
`rustcode-macos-aarch64-swift-app.zip`, the name the SwiftUI app's updater
downloads: an installed SwiftUI app offers the update and replaces itself
with this app.

Until releases are signed with a Developer ID and notarized
([macOS release signing](macos-release-signing.md)), the app is ad-hoc
signed. The installer's download is not quarantined, so it opens normally; a
copy downloaded with a browser needs right-click › Open once, or
`xattr -d com.apple.quarantine RustCode.app`.

## Desktop app development

The desktop app lives in `apps/desktop` (Tauri 2, React, Tailwind). Build
and run it as described in [desktop.md](desktop.md).

### SwiftUI macOS app (no longer released)

The SwiftUI app in `macos/` is frozen: releases no longer ship it, and its
CI (`macos-swift-ci.yml`) keeps running until it is archived. The rest of
this section describes it for anyone building it from source.

The native SwiftUI app is a separate front end to the Rust engine. It launches
`rustcode --acp` as a child process; the reusable protocol client and process
management code live in the UI-independent `RustCodeKit` Swift package target.
How the app is put together (targets, engine-to-UI data flow, state and
tests) is in [macOS app architecture](architecture/macos-app.md).
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


⌘K (File > Switch Chat…) opens a switcher over the window: type part of a
chat's title, project or model, or something said in it, move with ↑↓ and
press ⏎ to open the highlighted chat, or Esc to close. It searches in the
sidebar's All or Project scope, which it can also switch.

To hand a message to another chat without leaving this one, type it, then
choose **+ > Send Message to Another Chat…** and pick the chat. It runs there as
that chat's own turn, in its folder, after any turn it is running; this chat
stays open. Its answer arrives as for any chat working in the background: a
toast that opens it and a dot in the sidebar. It is a one-way handoff, so the
answer stays in that chat.

Long chats can be read from the keyboard: Page Up and Page Down (fn-↑ and
fn-↓ on a laptop) scroll the chat by a screen, Home goes to its first message
and End to the latest, following new output again like the Latest button. The
keys work while the composer or chat search has focus, since they do not edit
text there; with Shift, Option, Control or Command they are left to the text
field, and they stay with the terminal, the preview and open sheets.

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
- **Desktop app:** see [Updating the app](#updating-the-app)

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
