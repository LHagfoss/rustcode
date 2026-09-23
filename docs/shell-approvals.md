# Shell command approvals

RustCode checks `run_command` calls before they execute. Known read-only
commands may run without a prompt. Mutating, unclassified, or shell-composed
commands ask for confirmation. A user can approve a command once, save a
reusable prefix, or forbid a reusable prefix from the confirmation panel.

Reusable allow rules can cover ordinary plain command prefixes such as
`make test`, `git add src/main.rs`, `npm test`, and `go test ./...`. Matching
compares complete tokens, so `cargo test` does not match `cargo testing`. The
rule is the token prefix the user approved. Privilege escalation, network
clients, package installation/publication, deployment/release actions, and
known destructive commands are never eligible for reusable allow rules. Saved
rules only match a single plain command; quoting, shell operators,
substitutions, redirections, globbing, and environment or background overrides
continue to require a new confirmation. Parent and subagent shell calls use the
same saved rules.

Forbid rules persist in `~/.config/rustcode/config.toml` as
`denied_command_prefixes`, are user-level only, and take precedence over saved
allow rules and session auto-confirm. A denied command prefix cannot be
approved through the normal prompt. Remove or edit an entry in the global
config to change it. One-time approval remains available for commands without
a saved forbid rule.

RustCode does not currently provide an operating-system sandbox for shell
commands. A one-time or reusable approval authorizes execution in the normal
RustCode process context; it does not limit filesystem or network access.
These approval rules improve repeated decisions but do not provide
Codex-equivalent OS isolation. See follow-up issue #1358 for sandbox backends.
