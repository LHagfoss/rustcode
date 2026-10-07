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

Tagged releases publish the terminal binaries for Linux, macOS and Windows.
The `RustCode.app` bundle is not published for now; build it locally with
the script above.

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
