# Shell command approvals

RustCode checks `run_command` calls before they execute. Known read-only
commands may run without a prompt. Mutating, unclassified, or shell-composed
commands ask for confirmation. A user can approve a command once, save a
reusable plain-token command-prefix rule, or forbid a matching token sequence
from the confirmation panel.

Newly saved reusable allow rules match complete leading tokens, so
`cargo test` covers `cargo test --lib` but not `cargo testing`. Existing saved
entries without a prefix marker continue to match only the exact normalized
command the user approved. Whitespace differences are normalized. A new prefix
rule intentionally covers plain arguments after the approved prefix; choose a
narrow prefix when extra arguments could widen the action.
Shell syntax, quotes, substitutions, redirections, globs, tilde expansion, and
environment or background overrides are ineligible for reusable allow rules.
Privilege escalation, network clients, package installation/publication,
deployment/release actions, and known destructive commands are also ineligible.
Forbid rules take precedence over allows and session auto-confirm. Parent and
subagent shell calls use the same saved rules.

Approval rules decide when RustCode asks the user. They do not provide
operating-system isolation or change the shell process's permissions. OS
permissions are enforced separately only on platforms with a supported
sandbox backend. On unsupported platforms such as Windows, commands run with
the RustCode process's permissions.

Forbid rules persist in `~/.config/rustcode/config.toml` as
`denied_command_prefixes`, are user-level only, and take precedence over saved
allow rules and session auto-confirm. A denied command cannot be approved
through the normal prompt. Denies match literal token sequences regardless of
whitespace, basic quoting, environment overrides, or background/detached
mode. Known shell wrappers, `git submodule foreach`, and command composition
are inspected conservatively; ambiguous syntax is blocked while a deny rule
is active. An inline `git -c alias.name=...` is blocked whenever a Git deny
rule exists. Aliases already stored in Git configuration are not expanded by
the matcher; save a deny for the alias command itself if it can run a
forbidden action. Remove or edit an entry in the global config to change it.
One-time approval remains available for commands without a matching saved
forbid rule.

The `/sandbox` command and `sandbox_mode` setting in
`~/.config/rustcode/config.toml` control
effective shell permissions on Linux and macOS. Its values are
`read_only`, `workspace_write` (the default), and
`workspace_write_network`. The startup banner shows the effective mode
separately from the command approval mode. Project config files cannot change
this user-level security setting. `/sandbox` with no argument shows the current
effective permissions; pass one of the mode names to change and persist it.

On Linux, shell commands run through bubblewrap with the host filesystem
read-only and, in `workspace_write` modes, the active workspace and session
scratch directory writable. `read_only` leaves the entire host filesystem
read-only.
RustCode first probes whether bubblewrap can create a private network
namespace. When the host denies bubblewrap's loopback setup, RustCode uses the
same filesystem and process isolation plus a seccomp filter. The filter
restricts `socket` and `socketpair` to `AF_UNIX`, denies IP and other socket
families, blocks network syscalls (`connect`, `accept`/`accept4`, `bind`,
`listen`, peer/name/shutdown/send/receive-mmsg/options calls), and blocks
`io_uring_setup`, `io_uring_enter`, and `io_uring_register`. `recvfrom` and
`sendmsg` remain allowed for Unix-domain subprocess IPC. `.git` remains
writable inside the workspace so approved Git operations work; mutating Git
commands still require approval under the shell guard. RustCode requires
`bwrap` in a root-owned system PATH directory and an active workspace; if
sandbox setup or the seccomp filter fails, it refuses to run the command.
Install bubblewrap with your distribution's package manager.

On macOS, shell commands run under Seatbelt through the fixed
`/usr/bin/sandbox-exec` executable. The policy allows host file reads but
restricts writes to the canonical active workspace and session scratch
directory in `workspace_write` modes, and denies network connections by
default. `read_only` grants no filesystem writes. Temporary-file
environment variables point inside the workspace so build tools keep their
temporary output within the writable policy. Symlinked scratch directories,
working directories outside writable roots, and workspaces resolving to `/`
are rejected before launch. `.git` remains writable inside the workspace so
approved Git operations work. If `sandbox-exec` is unavailable or Seatbelt
rejects the profile, RustCode refuses to run the command; it does not fall
back to an unsandboxed shell. Apple has deprecated `sandbox-exec`, but it is
the system Seatbelt interface RustCode currently uses.

The `workspace_write_network` mode allows network access for every shell
command. A command may also request `network_access: true` for one-shot
network permission. RustCode adds the requested permission to the approval
card and requires an interactive approval even in YOLO mode; saved command
approvals do not grant it. Approval only widens network access for that one
command and does not change the configured mode.

The `filesystem_write_path` argument requests write access to one existing
absolute directory outside the active workspace. The approval card shows its
canonical resolved path and RustCode requires an interactive decision,
including in YOLO mode; saved command approvals do not grant filesystem
access. On Linux and macOS, that directory is the only additional writable
root for the command, so read-only mode continues to protect the workspace
and other paths.

Windows does not yet have an operating-system sandbox backend. Shell commands
continue to run with the RustCode process permissions there, and the startup
banner reports that no OS sandbox is active. Linux and macOS currently give
commands broad host read access while restricting writes and network access
according to the configured mode. Reusable command approvals remain separate
from operating-system permissions and do not widen the sandbox.

On Linux, the seccomp fallback preserves AF_UNIX socket creation and
socketpairs, but blocks `connect` and server-side network syscalls, including
for AF_UNIX.
