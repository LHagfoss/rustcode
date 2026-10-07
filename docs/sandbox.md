# Headless runs, approvals and sandbox modes

See [shell approvals](shell-approvals.md) for the full approval rule reference.

## Non-interactive prompt

Run one prompt without opening the TUI:

```bash
rustcode --prompt "inspect this repository and run its tests"
```

Add `--yolo` only in a trusted workspace when the run should automatically
approve all tool confirmations and override saved sandbox restrictions for
the session. Plan-mode capability blocking and saved forbid rules remain. Background commands started by the turn are
tracked until their terminal result is delivered; unrelated tasks already
running in the same session do not delay the turn.

## Approvals

Interactive shell confirmations offer one-time approval, reusable plain-token
command-prefix allows, and reusable token-sequence denies. An allow for
`cargo test` covers `cargo test --lib` but not `cargo testing`; deny rules take
precedence over allows and session auto-confirm. Shell composition, dynamic
shell syntax, environment or background overrides, privileged/network
commands, package installation/publication, deployment/release actions, and
known destructive commands remain ineligible for reusable allows. Approval
rules do not provide operating-system isolation or change the shell process's
permissions. OS sandbox enforcement is available only on supported backends
(currently Linux and macOS); on unsupported platforms such as Windows,
approved commands run with the RustCode process's permissions. Configure the
effective Linux/macOS mode with `sandbox_mode = "read_only"`,
`"workspace_write"`, `"workspace_write_network"`, or `"trusted"` (default) in
the user config, or switch it in a session with `/sandbox <mode>` — `/sandbox`
with no argument lists every mode with its permissions and marks the current
one. The status line and the welcome banner display the effective mode
separately from the approval mode. `network_access: true` requests network
access for one command and needs interactive approval when YOLO is off. The `filesystem_write_path` argument requests one-command write access to
one existing absolute directory outside the active workspace; its canonical path
is shown in the approval card. It also requires
interactive approval when YOLO is off and cannot be covered by a saved command approval. See
[shell approvals](shell-approvals.md).

## Sandbox modes and the default

`trusted` is the default. Shell commands and native filesystem/search tools run
with RustCode's process permissions, including network access and paths outside
the workspace. Relative paths still resolve from the active task directory or
workspace. Restricted modes are opt-in through the global config or `/sandbox`;
project configuration cannot change this selection.

YOLO overrides a saved restricted mode and auto-approves all tool confirmations,
including one-shot network and filesystem requests. It applies to interactive,
headless, and ACP turns and does not rewrite the saved mode. Turning YOLO off
restores that selection. Plan-mode capability restrictions and explicit saved
forbid rules remain in effect.

Failed restricted commands report effective network permissions and actual
writable roots, including one-shot grants. Permission-specific errors suggest
possible sandbox enforcement. DNS errors, refused connections, generic host
filesystem permissions, credentials, SSH keys, and certificates are not treated
as proven sandbox denials. Enabled network access is never blamed as a network
restriction. Trusted execution and platforms without a native OS backend receive
no sandbox attribution.
