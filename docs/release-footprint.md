# Release footprint baseline

Measured on a QEMU x86-64 Linux guest (kernel 6.12.117+deb13-amd64)
using [v1.45.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.45.0)
(`4c11fb4fe886297b6d8f3e1b49bef73dbc1a8bf5`).

## Linux release binaries

| Executable | File bytes | File MiB |
| --- | ---: | ---: |
| `rc` | 91,362,464 | 87.13 |
| `rustcode-desktop` | 31,276,800 | 29.83 |

The CLI archive is 32,392,957 bytes (30.89 MiB). The desktop archive is
47,112,195 bytes (44.93 MiB). Its bundled `rc` is byte-identical to the
standalone executable.

Download and verify from a scratch directory:

```sh
gh release download v1.45.0 -R LHagfoss/rustcode -p 'rc-linux-x86_64.tar.gz' \
  -p 'RustCode-*-linux-x86_64.tar.gz' -p SHA256SUMS
sha256sum --ignore-missing -c SHA256SUMS
mkdir cli desktop
tar -xzf rc-linux-x86_64.tar.gz -C cli
tar -xzf RustCode-1.45.0-linux-x86_64.tar.gz -C desktop
stat -c '%s %n' cli/rc-linux-x86_64 desktop/usr/bin/{rc,rustcode-desktop}
cmp cli/rc-linux-x86_64 desktop/usr/bin/rc
```

## Warm CLI startup

One warm-up and 30 samples per command. Each sample ran a fresh process with
stdout redirected to `/dev/null` and an isolated home, XDG directories and
`RUSTCODE_CONFIG_DIR`. Wall time is the monotonic interval from immediately
before fork and exec to process exit. Peak RSS is Linux `wait4`'s
`ru_maxrss` for that child. `threads list` used an empty local store.

| Command | Wall ms, median (range) | Peak RSS KiB, median (range) |
| --- | ---: | ---: |
| `rc --version` | 5.744 (5.070–7.462) | 12,114 (11,956–12,404) |
| `rc --help` | 5.359 (4.496–8.247) | 12,208 (11,984–12,352) |
| `rc threads list` | 12.557 (11.290–15.847) | 18,774 (18,512–19,124) |

## Outstanding measurements

- Crate attribution requires release builds retaining symbols for both
  `cargo bloat --release -p rustcode-cli --bin rc --crates` and
  `cargo bloat --release --manifest-path apps/desktop/src-tauri/Cargo.toml --crates`.
  Both projects strip their release builds by default. Set
  `CARGO_PROFILE_RELEASE_STRIP=false` for the attribution runs and compare the
  resulting crate text sizes separately from shipped file sizes.
- Cold CLI startup and resident memory during a long-lived CLI workload
  remain unmeasured. Peak RSS of the short commands above covers only those
  commands.
- The guest lacks WebKitGTK and related libraries for the Linux shell.
  The macOS native benchmark also requires macOS. Its standalone WKWebView
  runner measures renderer workloads; launch timing and idle/active RSS for
  the packaged app require a separate process-tree measurement including
  the engine sidecar and WebKit helper processes.
