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

- macOS: Settings › Advanced › **Check for Updates**, then **Update to X**. The
  app downloads the release's app zip, checks it against `SHA256SUMS`, its
  code signature and bundle id, then quits, swaps `RustCode.app` and reopens.
  It needs write access to the folder the app is in; otherwise the button
  opens the release page. Running the installer again works too.
- Linux AppImage: **Update to X** downloads the new AppImage next to the
  running one, checks it against `SHA256SUMS`, replaces the file and reopens.
  It needs write access to the AppImage's folder (`~/Applications` is); no
  root.
- Linux `.deb`: **Update to X** downloads the package, checks it and installs
  it with `pkexec apt-get install -y` (or `pkexec dpkg -i`), which asks for
  your password through polkit, then reopens. Without `pkexec` or a polkit
  agent, install the new `.deb` yourself (`sudo apt install ./RustCode-<ver>-linux-x86_64.deb`).
- Windows: **Update to X** downloads the setup, checks it, runs it silently
  (`/S /UPDATE /R`) and quits; the installer replaces the app in place and
  reopens it. No administrator prompt.
- Other installs (a tarball, Flatpak, a folder RustCode cannot write, arm
  Linux or Windows): the button links to the release;
  run the installer again to update.

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

An old SwiftUI RustCode app no longer finds updates: releases up to 1.2.0
carried it over to this app, and later ones do not. Download the app zip from
the release page instead.

Until releases are signed with a Developer ID and notarized
([macOS release signing](macos-release-signing.md)), the app is ad-hoc
signed. The installer's download is not quarantined, so it opens normally; a
copy downloaded with a browser needs right-click › Open once, or
`xattr -d com.apple.quarantine RustCode.app`.

## Desktop app development

The desktop app lives in `apps/desktop` (Tauri 2, React, Tailwind). Build
and run it as described in [desktop.md](desktop.md).

## Updating

RustCode comes with a built-in cross-platform self-updater for macOS, Linux, and Windows.
Native installations update from GitHub Releases; Homebrew installations use Homebrew.

- **In CLI:** Run `rustcode --update` (or `rustcode --upgrade`)
- **Inside RustCode TUI:** Type `/update` (or accept the update modal on startup)
- **Homebrew (macOS):** `brew upgrade rustcode`
- **Desktop app:** see [Updating the app](#updating-the-app)

A machine can hold several copies, and the one first on `PATH` is what a shell
runs: the app's bundled engine, Homebrew's, the install script's copy under
`~/.rustcode/bin` (`%USERPROFILE%\.rustcode\bin\rustcode.exe` on Windows), or
`cargo install`'s. `rustcode server status` reports when the `rustcode` a shell
finds is another version than the engine that answered, with both paths and the
command that fixes it: update that copy, or remove it so the shell finds the
one the app runs ([server](server.md#status)).

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
