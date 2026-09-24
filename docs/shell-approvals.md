# Shell command approvals

RustCode checks `run_command` calls before they execute. Known read-only
commands may run without a prompt. Mutating, unclassified, or shell-composed
commands ask for confirmation. A user can approve a command once, save an
exact-command allow rule, or forbid a matching token sequence from the
confirmation panel.

Reusable allow rules cover only the same normalized argv the user approved.
For example, approving `git add src/main.rs` does not allow
`git add src/main.rs .`, and approving `make test` does not allow
`make test upload-prod`. Whitespace differences are normalized. Shell syntax,
quotes, substitutions, redirections, globs, tilde expansion, and environment
or background overrides are ineligible for reusable allow rules. Privilege
escalation, network clients, package installation/publication,
deployment/release actions, and known destructive commands are also ineligible.
The config field `approved_command_prefixes` is retained for compatibility,
but its entries are treated as exact normalized argv until an operating-system
sandbox is available. Parent and subagent shell calls use the same saved rules.

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

On Linux, shell commands run through bubblewrap with the host filesystem
read-only and the active workspace and session scratch directory writable.
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

Native sandbox backends for macOS and Windows are not implemented yet. Those
platforms retain the existing command execution behavior, so approval is not
an operating-system isolation boundary there. The Linux sandbox isolates
filesystem writes and IP networking. The seccomp fallback preserves AF_UNIX
socket creation and socketpairs, but blocks `connect` and server-side network
syscalls, including for AF_UNIX. Additional Codex controls such as fine-grained
read restrictions, protected metadata, and approval-aware permission
escalation remain future work.
