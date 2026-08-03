# AGENTS.md

## Workflow (user-mandated, follow for every task)

1. Checkout a `feature/...` or `fix/...` branch from `main`.
2. Do the work; verify with `cargo check --tests` / `cargo test`.
3. Commit, push, create a PR into `main` with `gh pr create`.
4. Merge the PR into `main` with `gh pr merge`.
5. Checkout `main` and `git pull`.

Split into multiple smaller PRs when changes are independent (e.g. feature vs harness fix).

## Tooling notes

- `replace_file_content` only honors line anchoring when **both** `start_line` and `end_line` are passed together; pass both, and prefer multi-line `target_content` that is unique in the file.
- Sessions/history live in `~/.config/rustcode/sessions/<id>/history.json`.
