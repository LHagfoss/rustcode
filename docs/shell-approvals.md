# Shell command approvals

RustCode checks `run_command` calls before they execute. Known read-only
commands may run without a prompt. Mutating, unclassified, or shell-composed
commands ask for confirmation. A user can approve a command once or save a
reusable prefix from the confirmation panel.

Reusable rules are available only for vetted test/build actions. RustCode
currently supports Cargo's `test`, `check`, `build`, `clippy`, `fmt`, and `doc`
actions, plus `python -m pytest` and `python -m unittest`. The action (and, for
Python, module) is part of the rule: approving `cargo test --lib` saves
`cargo test`, while `cargo +stable test` is not eligible because the selector
comes before the action. `python -m` alone is never saved. Matching compares
complete command tokens, so `cargo test` does not match `cargo testing`.
Package installers and publishers, arbitrary interpreters/modules, and
unreviewed command families remain one-time approvals. Saved rules only match
a single plain command; quoting, shell operators, substitutions, redirections,
globbing, and environment or background overrides continue to require a new
confirmation. Parent and subagent shell calls use the same saved rules.

Privilege escalation, network utilities, container and infrastructure tools,
and known destructive command families cannot receive reusable rules through
the prompt. Existing configuration entries are checked against the same
restrictions before they can match. Users can inspect or remove entries in
`~/.config/rustcode/config.toml` under `approved_command_prefixes`.

RustCode does not currently provide an operating-system sandbox for shell
commands. A one-time or reusable approval authorizes execution in the normal
RustCode process context; it does not limit filesystem or network access. The
reusable choice is intended for routine developer commands such as test and
format workflows, and should only be granted to commands the user trusts.
