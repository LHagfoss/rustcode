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

## Crates, startup and memory from CI

The [Release footprint workflow](../.github/workflows/footprint.yml) measures
these on GitHub's runners: on demand (`gh workflow run footprint.yml`) and when
it or `scripts/footprint.py` changes. Its job summary and artifact hold the
full report. The numbers below are from revision
`a1c386dc2b4f6093245dc29e9d56b374aa5fa97a`, rustc 1.100.0, on `ubuntu-24.04`
(x86-64, 4 CPUs) and `macos-26` (arm64, 3 CPUs).

`cargo bloat` needs symbols, so the workflow builds with
`CARGO_PROFILE_RELEASE_STRIP=false` and measures stripped copies, as shipped.
The desktop shell is built with `tauri/custom-protocol`, as `tauri build`
builds it.

### Largest crates by `.text`

| Crate | `rc` Linux | `rc` macOS | Desktop Linux | Desktop macOS |
| --- | ---: | ---: | ---: | ---: |
| `rustcode` (engine) | 27.3 MiB (42.7%) | 21.9 MiB (42.8%) | – | – |
| `std` | 10.7 MiB (16.8%) | 10.1 MiB (19.8%) | 1.6 MiB (9.0%) | 1.3 MiB (12.0%) |
| `agent_client_protocol` | 6.6 MiB (10.3%) | 5.4 MiB (10.6%) | – | – |
| `rustcode_desktop_lib` | – | – | 4.4 MiB (24.2%) | 3.8 MiB (35.3%) |
| `zbus` + `zvariant` + `secret_service` | 2.1 MiB (3.2%) | – | 4.0 MiB (22.0%) | – |
| `aws_lc_sys` | 1.3 MiB (2.0%) | 687 KiB (1.3%) | 1.3 MiB (7.0%) | 684 KiB (6.2%) |
| `rc` (CLI crate) | 1.5 MiB (2.3%) | 1.3 MiB (2.5%) | – | – |
| `tokio` | 710 KiB | 624 KiB | 213 KiB | 177 KiB |
| `rustls` | 507 KiB | 453 KiB | 560 KiB | 496 KiB |
| `mdns_sd` | 449 KiB | 381 KiB | 447 KiB | 386 KiB |
| **Total `.text`** | 63.9 MiB | 51.1 MiB | 18.0 MiB | 10.8 MiB |
| Stripped file | 92,867,480 B | 77,333,280 B | – | 20,600,176 B |

`.text` is about half of each `rc` file; the rest is mostly read-only data and
unwind tables. The desktop shell's Linux `.text` is two thirds larger than
macOS, almost all of it D-Bus (`zbus` for the keyring's Secret Service and
notifications).

Until #4140 that was two D-Bus stacks: `keyring` 3's Secret Service backend
used `zbus` 4, and the notification and single-instance plugins `zbus` 5.
`keyring` 4 runs its Secret Service store on `zbus` 5, so Linux links one.
Linux `.text`, measured by the same workflow before (`a1c386dc`) and after
(`perf/desktop-linux-dbus`):

| Linux binary | D-Bus crates before | D-Bus crates after | Total `.text` before | Total `.text` after |
| --- | ---: | ---: | ---: | ---: |
| Desktop shell | 4.0 MiB | 2.5 MiB | 18.0 MiB | 16.3 MiB |
| `rc` | 2.1 MiB | 1.5 MiB | 63.9 MiB | 63.8 MiB |

The D-Bus crates are `zbus`, `zvariant` (with `zvariant_utils`) and
`secret_service`. On `rc` the total drops by less than the D-Bus crates do;
the report does not show which other crates grew.

### `rc` startup and peak RSS

30 warm samples (after one warm-up) and 5 cold samples, each after dropping
the page cache (`drop_caches` on Linux, `sudo purge` on macOS). Median, with
the range.

| Command | Linux warm ms | Linux cold ms | macOS warm ms | macOS cold ms | Peak RSS KiB (Linux / macOS) |
| --- | ---: | ---: | ---: | ---: | ---: |
| `rc --version` | 5.2 (4.6–6.7) | 32.3 (30.3–34.4) | 11.8 (8.1–20.7) | 856 (614–2,170) | 13,932 / 13,872 |
| `rc --help` | 5.3 (5.1–5.7) | 29.8 (29.4–34.3) | 22.2 (14.6–42.3) | 538 (427–1,675) | 13,932 / 13,968 |
| `rc threads list` | 27.0 (26.4–30.4) | 112.5 (110.5–121.1) | 27.9 (23.9–91.3) | 612 (543–821) | 17,304 / 16,960 |

macOS cold launch includes re-reading the unsigned binary after `purge` on a
shared runner, so treat it as an upper bound rather than a laptop's figure.

An idle `rc --acp`, the process the desktop app spawns, holds 30.3–30.9 MiB
resident on Linux and 25.6–26.0 MiB on macOS from 1 to 30 seconds after spawn.

### Desktop app launch (macOS)

The stripped release shell with an empty configuration, sampled with `ps`
every 50 ms. WebKit's helpers are launchd XPC services rather than children of
the app; the new ones on the idle runner are counted as the app's.

- The `rc --acp` engine is spawned 2.4 s after launch, and the first WebKit
  helper (the webview) appears at 5.3 s.

| Seconds after launch | App KiB | Engine KiB | WebKit helpers KiB | Total KiB |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 35,600 | 0 | 0 | 35,600 |
| 2 | 62,336 | 0 | 0 | 62,336 |
| 5 | 80,928 | 25,776 | 27,168 | 133,872 |
| 10 | 102,656 | 28,608 | 172,912 | 304,176 |
| 20 | 105,984 | 33,936 | 205,584 | 345,504 |

About 340 MiB idle with an empty store, 60% of it in WebKit. The Linux shell
is not launched: the runner has no display. Memory under an active thread
(streaming, many messages) is what `bun run perf:native` measures for the
renderer.
