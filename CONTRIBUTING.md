# Contributing

Thanks for wanting to help out. Bug reports, fixes, docs and small features
are all welcome.

## Before you start

- For a bug or a small fix, just open a PR.
- For a bigger change (new feature, new dependency, changes to how the agent
  behaves), open an issue first so we can agree on the approach before you
  spend time on it.
- Security problems: please don't open a public issue. See
  [SECURITY.md](SECURITY.md).

## Building

You need a recent stable Rust (1.85 or newer).

```bash
git clone https://github.com/LHagfoss/rustcode.git
cd rustcode
cargo run -p rustcode-tui        # run the TUI from source
```

More platform notes are in [docs/install.md](docs/install.md).

The repo has a few parts:

| Path | What it is |
| --- | --- |
| `rustcode/engine` | The agent: providers, tools, sessions, ACP server |
| `rustcode/tui` | The terminal app |
| `rustcode/core` | Shared crates |
| `macos/` | Native macOS app (SwiftUI) |
| `ios/` | iOS app for `/remote` |
| `docs/` | User docs |

## Checks

Run these before opening a PR. CI runs the same ones.

```bash
cargo fmt --all
cargo check --workspace --tests
cargo test --workspace
```

Use `--workspace`, otherwise only the engine is built and TUI changes slip
through.

If you touched the macOS app:

```bash
cd macos && swift build && swift test
```

If you touched the iOS app, see [ios/AGENTS.md](ios/AGENTS.md) for the
`xcodebuild` commands.

Docs-only changes don't need any of this.

## Commits and PRs

We use [Conventional Commits](https://www.conventionalcommits.org/) for commit
messages and PR titles:

```
fix(tui): keep transcript selection after drag
feat(engine): add a timeout to MCP startup
docs(mcp): add a setup example
```

Types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `chore`, `ci`,
`build`. Scopes are the area you touched, for example `engine`, `tui`, `mcp`,
`sandbox`, `macos`, `ios`, `ci`. Write the summary in the imperative ("add",
not "added"), no period at the end.

PRs are squash-merged, so the PR title becomes the commit on `main`. Don't
worry about tidy history inside your branch.

Keep PRs focused on one thing. Add a test when you fix a bug, if you can.

## First-time contributors

CI on your first PR waits for a maintainer to approve the run. That's a GitHub
default for forks, nothing wrong with your PR.

## License

By contributing you agree that your work is licensed under the
[Apache License 2.0](LICENSE), same as the rest of the project.
