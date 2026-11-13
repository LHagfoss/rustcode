# System setup

The desktop app runs a short list of per-OS steps after an install and after
every update, and Settings › About › **System setup** shows each one with what
it found. Code: `apps/desktop/src-tauri/src/system_setup.rs`; page:
`SystemSetupGroup` in `apps/desktop/src/ui/Settings.tsx`.

## How a step runs

Every step is **checked first** and applied only when the check says it is
missing, so running the steps again, at the next update or with **Run again**,
changes nothing that is already right. A check answers one of four things:

| Check | Status shown | What happens |
| --- | --- | --- |
| already as it should be | *Done* | nothing |
| missing, and RustCode can fix it | *Not done*, then *Done* or *Failed* | the step is applied (an automatic step at once; the others on their **Set up** button) |
| wrong, and only you can fix it | *Needs you* | the message holds the command, with **Copy command**; nothing runs |
| not for this machine | *Not needed* | nothing; the message says why |

RustCode never asks for elevation silently and never edits your shell profile:
a step that wants an administrator, a profile line or a package install shows
the command instead.

The automatic steps run once for each app version at launch (the first launch
after an install or an update; a development build leaves the machine alone).
**Run again** on the group runs the automatic steps now; the button on a row runs
that step alone, which is also how the background server is installed.

## Steps

| Step | System | Automatic | What it does |
| --- | --- | --- | --- |
| `quarantine` | macOS | yes | `xattr -dr com.apple.quarantine` on the app bundle when it still carries the download flag |
| `stable-signature` | macOS | yes (check only) | says when the app is signed ad hoc, which makes macOS treat every update as a new app (see below), and gives the command for `scripts/macos-stable-signature.sh` |
| `cli-link` | macOS, Linux | yes | links `~/.local/bin/rustcode` to the app's engine when no `rustcode` is on the PATH or in the usual install folders; a link to nothing is replaced, a real file is never touched |
| `path-hint` | macOS, Linux | yes (check only) | asks the login shell for its PATH; when `~/.local/bin` is missing it shows the line to add for that shell |
| `user-path` | Windows | yes | adds the folder with `rustcode.exe` to the **user** PATH (`HKCU\Environment`), keeping the entries and the value's type (`REG_EXPAND_SZ`); open a new terminal to use it |
| `git` | Windows | yes (check only) | looks for Git for Windows; shows `winget install --id Git.Git -e` when missing |
| `long-paths` | Windows | yes (check only) | reads `LongPathsEnabled`; turning it on needs an administrator, so the PowerShell command is shown |
| `desktop-entry` | Linux | yes | for an AppImage, writes `~/.local/share/applications/rustcode.desktop` (marked `X-RustCode-Managed=true`, rewritten when the AppImage moves); an entry another tool made is left alone; a `.deb` has its own |
| `server-service` | all | no | `rustcode server install`: the LaunchAgent, systemd user unit or logon task of [the server](server.md) |

The WebView2 and webkit2gtk checks are not steps: the app cannot start without
its web view, so a check would always pass.

## The record

`system-setup.json` in the config folder keeps the last outcome of each step
(`status`, `message`, `at`) and `ran_for`, the app version the automatic steps
last ran for. Settings shows *last run* from it. A missing or unreadable file is
an empty record.

## Why macOS asks again after an update

Releases are signed ad hoc (no Developer ID), and an ad-hoc signature is the hash
of that exact build, so every release is a different identity to macOS. That is
what raises the *Background Items added* / *allow in the background* prompt
(the LaunchAgent of [the server](server.md) is tied to the app that registered
it) and the Keychain password prompts after each update. The updater already
re-signs an ad-hoc bundle with the *RustCode Local Signing* certificate when it
exists (`locally_signed_bundle` in `updater.rs`), which keeps one identity across
updates, but it does not create the certificate: that needs the login keychain.
Without it the updater logs *Local update signing skipped* and installs the ad-hoc
bundle. The `stable-signature` step shows when that is the case and the one
command that fixes it. The real fix is a Developer ID signature on the release.
