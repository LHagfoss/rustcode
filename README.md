<p align="center">
  <img src="./images/rustcode-logo.png" alt="RustCode logo" width="100"/>
</p>

<h1 align="center">RustCode</h1>
<p align="center">
  <b>An agent harness (or sum), made in Rust</b>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/license-Apache%202.0-blue" alt="License: Apache 2.0">
  <img src="https://img.shields.io/badge/rust-1.85%2B-orange?logo=rust" alt="Rust 1.85+">
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-blue" alt="macOS, Linux, Windows">
</p>

<p align="center">
  <a href="https://rustcode.lhagfoss.com">rustcode.lhagfoss.com</a> · <a href="mailto:hello@lhagfoss.com">hello@lhagfoss.com</a>
</p>

<img src="./images/header.png" alt="RustCode running in a terminal"/>

`rustcode` is a lightweight terminal (TUI) coding agent. It started as a way to
test Apple's on-device Foundation Models and turned into a much deeper project.
It works with Ollama, any OpenAI-compatible API, and your existing ChatGPT,
GitHub Copilot or Claude subscription.

## Install

**macOS and Linux**

```bash
curl -fsSL https://rustcode.lhagfoss.com/install.sh | bash
```

**Windows (PowerShell)**

```powershell
irm https://rustcode.lhagfoss.com/install.ps1 | iex
```

**Homebrew (macOS)**

```bash
brew tap lhagfoss/tap
brew trust lhagfoss/tap
brew install rustcode
```

**From source**

```bash
git clone https://github.com/lhagfoss/rustcode.git
cd rustcode
cargo install --path rustcode/tui
```

Prebuilt binaries cover Linux x86_64, macOS Apple Silicon and Windows x86_64.
Update with `rustcode --update`, `/update` inside the TUI, or
`brew upgrade rustcode`. Platform details, the desktop app and source builds
are in the [install guide](docs/install.md).

## Quick start

```bash
rustcode                                  # open the TUI in the current project
rustcode --prompt "run the tests"         # one prompt, no TUI
rustcode --resume                         # pick up the last session here
rustcode --acp                            # Agent Client Protocol server over stdio
rustcode doctor                           # check config, binaries and skills
```

Connect a provider from inside the TUI:

| Command | Connects |
| --- | --- |
| `/login` | ChatGPT plan sign-in or an API-key provider |
| `/login github-copilot` | GitHub Copilot ([setup](docs/github-copilot.md)) |
| `/login claude` | A Claude subscription through the local Claude Code CLI |
| `/accounts`, `/account`, `/logout` | Manage saved accounts |
| `/model` | Switch model profile |

See [provider authentication](docs/provider-auth.md) for every option.

## What it does

- **Any model** — Ollama, OpenAI-compatible endpoints, the Responses API and
  subscription logins, with per-model profiles in one `config.toml`.
- **Tools with guardrails** — file edits, shell commands and search, with
  one-time or reusable approvals and optional OS sandboxing on macOS and Linux
  ([sandbox modes](docs/sandbox.md)).
- **Background work** — detach long commands and get the result delivered when
  they finish ([background tasks](docs/background-tasks.md)).
- **Subagents** — spawn, message, wait on and cancel child agents with isolated
  context ([delegation](docs/multi-agent.md)).
- **MCP and skills** — connect MCP servers and drop Markdown skills into
  `~/.agents/skills` or a project ([configuration](docs/configuration.md)).
- **Editor integration** — runs headless as an ACP server ([ACP](docs/acp.md)).
- **Measured, not guessed** — `/perf` shows where each turn spent its time
  ([performance](docs/performance.md)).

## Configuration

Settings live in `~/.config/rustcode/config.toml` (`%APPDATA%\rustcode` on
Windows). Run `rustcode init` for a project-local override; precedence is:

```text
CLI overrides > nearest project config > global config > built-in defaults
```

A model profile is a few lines:

```toml
[[models]]
name = "my-model"
url = "https://example.invalid/v1/chat/completions"
model = "my-model"
env_key = "MY_API_KEY"
```

Everything else — skills, MCP servers, prompt templates, config sync,
scrollback and motion settings — is in the
[configuration guide](docs/configuration.md).

## Documentation

| Topic | Guide |
| --- | --- |
| Install, update, desktop app | [docs/install.md](docs/install.md) |
| Configuration, skills, MCP, sync | [docs/configuration.md](docs/configuration.md) |
| Providers and sign-in | [docs/provider-auth.md](docs/provider-auth.md) · [GitHub Copilot](docs/github-copilot.md) |
| Approvals and sandbox modes | [docs/sandbox.md](docs/sandbox.md) · [shell approvals](docs/shell-approvals.md) |
| Slash commands and the TUI | [docs/command-panels.md](docs/command-panels.md) · [TUI notes](docs/tui.md) |
| Background tasks | [docs/background-tasks.md](docs/background-tasks.md) |
| Subagents | [docs/multi-agent.md](docs/multi-agent.md) |
| ACP server | [docs/acp.md](docs/acp.md) |
| Remote frontends (experimental) | [docs/mobile.md](docs/mobile.md) · [frontends](docs/frontends.md) |
| Audio and video tools | [docs/media.md](docs/media.md) |
| Thinking routing | [docs/thinking-router.md](docs/thinking-router.md) |
| Performance and traces | [docs/performance.md](docs/performance.md) · [stream traces](docs/provider-stream-traces.md) · [build benchmark](scripts/bench-build-boundaries.md) |
| Architecture | [docs/architecture.md](docs/architecture.md) |
| Discord Rich Presence | [docs/discord-rich-presence.md](docs/discord-rich-presence.md) |

## Apple Foundation Models

Running `rustcode` on Apple's on-device model needs
[macOS 27 and Xcode 27](https://developer.apple.com/videos/play/wwdc2026/334/).
It is not recommended for real work: the system model has roughly a 2k context
window.

## License

Licensed under the [Apache License, Version 2.0](LICENSE).

Made with [Rust](https://www.rust-lang.org/) by goat (me) and models inside the
RustCode harness.
