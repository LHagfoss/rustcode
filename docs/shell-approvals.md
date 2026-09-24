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

RustCode does not currently provide an operating-system sandbox for shell
commands. A one-time or reusable approval authorizes execution in the normal
RustCode process context; it does not limit filesystem or network access.
These approval rules improve repeated decisions but do not provide
Codex-equivalent OS isolation. See follow-up issue #1358 for sandbox backends.
