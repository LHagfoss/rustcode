# Command guard

A project can make RustCode refuse or rewrite the shell commands its agents
run. The rules live in the repository, so every checkout, worktree and harness
gets the same ones. This repository's own policy encodes the shell rules of
`AGENTS.md`: no local `cargo test` (tests run in CI), `nice` for heavy
cargo, and no `git rebase`, `git stash`, `git reset --hard` or force-push.

## The policy file

`.rustcode/command-policy.toml`, found by walking up from the directory a
command runs in and stopping at the repository root (`.git`). Without the
file, nothing changes. A file that does not parse is logged to `debug.log` and
ignored, so a typo never blocks every command; the unit tests of this
repository's file keep it valid.

```toml
# enabled = false        # keep the file, turn the guard off

[[rule]]
name = "no-workspace-tests"        # named in the message the agent sees
action = "deny"                    # or "rewrite"
program = "cargo"                  # the program's file name
subcommand = ["test", "nextest"]   # first non-option argument; empty: any
flags = ["--workspace", "--all"]   # any of these (or --flag=value); empty: any
allow_env = "RUSTCODE_ALLOW_WORKSPACE_TESTS"   # a way through, see below
message = "Run `cargo test -p <package> <module>` instead."

[[rule]]
name = "cargo-low-priority"
action = "rewrite"
program = "cargo"
subcommand = ["build", "check", "clippy", "test"]
prefix_macos = "nice -n 10"
prefix_linux = "nice -n 10"   # `prefix` for any system
skip_if_wrapped = ["taskpolicy", "nice"]
```

- **deny** refuses the command. The agent is told the rule's name, your
  `message` (why, and what to run instead), the override if there is one, and
  the policy path.
- **rewrite** puts `prefix` in front of the command it matched. A command that
  already runs under one of `skip_if_wrapped` is left alone, so a rewrite is
  never applied twice. A rewrite is skipped on a system with no prefix for it
  and when the prefix's program is not installed.
- **allow_env** names a variable that lets a denied command through when it is
  set to anything but empty, `0` or `false`: in front of the command
  (`RUSTCODE_ALLOW_WORKSPACE_TESTS=1 cargo test --workspace`), in the exec
  tool's `env`, or in the environment RustCode runs in. It is a guard rail for
  an agent that forgot the rule, not a security boundary: an agent that sets
  the variable has made a deliberate choice, and the violation is still
  counted.

A deny rule beats a rewrite rule in the same command.

## What the matcher sees

It reads a command the way the shell groups it, without running anything:

- quotes keep a word together, so `echo 'git stash'` and
  `git commit -m "no cargo test --workspace"` match nothing;
- `&&`, `||`, `;`, `|`, newlines, `( )` and backticks separate commands, so
  `cd rustcode && cargo test --workspace` matches `cargo`;
- leading `VAR=value` assignments and the wrappers `env`, `command`, `exec`,
  `nohup`, `time`, `timeout`, `nice`, `ionice`, `taskpolicy`, `caffeinate` and
  `sudo` are looked through to the program that runs;
- `sh|bash|zsh -c '…'` scripts are read too, up to three levels deep;
- `git -C dir stash` and `cargo +nightly test` find their subcommand past the
  global options.

It does not expand variables, aliases, functions or `$(…)` inside double
quotes, and a script file is not read. The guard is there to stop honest
mistakes, not an agent set on getting around it.

## Where it is applied

| Harness | How | Rewrite |
| --- | --- | --- |
| RustCode (built-in) | In the exec tool (`run_command`), as the command is about to run. | Runs the rewritten command; its output starts with a line naming the rule. |
| Claude Code | A `PreToolUse` hook on `Bash`, added to the session when the project has a policy (`_meta.claudeCode.options.settings`, running the hidden `rustcode guard-command`). Unix only. | Refused with the command to run instead. |
| Codex, Gemini CLI, OpenCode, Pi | Through the approval request the harness sends (`session/request_permission`): a command the policy refuses is rejected without asking the user. | Rejected, and noted in `debug.log`. |

The last row guards only what a harness asks about. A harness running in a
mode that never asks (full access, auto-approve) runs the command without a
request, and RustCode cannot see it; those harnesses have no hook RustCode can
install. Use the built-in harness or Claude Code for a project that depends on
the rules. The rules of an external harness's session are read when the thread
starts (Claude Code) or per request (the others), so a policy added to a
project takes effect for a running Claude Code thread on its next start.

## Counting violations

Every refused or rewritten command is one `command_guard.violation` row in
`operational_events`, with the thread, `fields_json` `{rule, action,
harness}` (`action` is `deny` or `rewrite`), and no command text.

```sh
rustcode data query "SELECT thread_id, json_extract(fields_json,'$.rule') AS rule, \
  json_extract(fields_json,'$.action') AS action, COUNT(*) AS violations \
  FROM operational_events WHERE event_type = 'command_guard.violation' \
  GROUP BY thread_id, rule, action ORDER BY violations DESC"
```
