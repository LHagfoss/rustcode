# Shell command approvals

RustCode checks `run_command` calls before they execute. Known read-only
commands may run without a prompt. Mutating, unclassified, or shell-composed
commands ask for confirmation. A user can approve a command once or save a
reusable prefix from the confirmation panel.

The reusable option saves the first two whitespace-delimited command tokens.
For example, approving `cargo test --lib` can save `cargo test`, which also
covers `cargo test --doc`. Matching compares complete tokens, so `cargo test`
does not match `cargo testing`. Saved rules only match a single plain command;
quoting, shell operators, substitutions, redirections, globbing, and
environment or background overrides continue to require a new confirmation.

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
