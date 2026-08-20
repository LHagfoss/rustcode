<div align="center">
<h1>rustcode</h1>

<img src="./images/header.png" alt="rustcode screenshot 1"/>
</div>

## about

`rustcode` is a lightweight Terminal User Interface (TUI) agent harness.
Originally made for testing Apple's on-device Foundation Models. Turned into a way deeper project.
Now supports ollama or openai compatible APIs.

## Installation

### macOS & Linux (curl)

Run the one-line installer in your terminal:

```bash
curl -fsSL https://raw.githubusercontent.com/LHagfoss/rustcode/main/install.sh | bash
```

### Windows (PowerShell)

Run the one-line installer in PowerShell:

```powershell
irm https://raw.githubusercontent.com/LHagfoss/rustcode/main/install.ps1 | iex
```

### macOS via Homebrew

```bash
# 1. Tap the repository
brew tap lhagfoss/tap

# 2. Trust the tap (required by Homebrew for new/custom taps)
brew trust lhagfoss/tap

# 3. Install the harness
brew install rustcode
```

### From Source (Rust / Cargo)

```bash
# Clone and build
git clone https://github.com/lhagfoss/rustcode.git
cd rustcode
cargo install --path .
```

## Keeping it upgraded

RustCode comes with a built-in cross-platform self-updater for macOS, Linux, and Windows!

- **In CLI:** Run `rustcode --update` (or `rustcode --upgrade`)
- **Inside RustCode TUI:** Type `/update` (or accept the update modal on startup)
- **Homebrew (macOS):** `brew upgrade rustcode`

## ACP runtime

For editors and agent orchestrators that support the Agent Client Protocol, run
rustcode headlessly over stdio:

```bash
rustcode --acp
```

The process speaks stable ACP v1 JSON-RPC on stdin/stdout. A runtime such as
Multica can launch it as a subprocess, create a session with `session/new`, and
send work with `session/prompt`. The working directory supplied to
`session/new` becomes the workspace root for rustcode's tools. Rustcode stores
its canonical configuration in `config.toml`. On macOS and Linux this is
`${XDG_CONFIG_HOME:-~/.config}/rustcode/config.toml`; on Windows it is
`%APPDATA%\rustcode\config.toml`. `RUSTCODE_CONFIG_DIR` overrides the
directory on every platform, which is useful for portable installs and tests.

Older installations using `models.json` and `config.json` are still read. On
the next normal save, Rustcode writes the merged configuration to
`config.toml` and leaves the legacy files intact as a rollback copy. Missing
fields use compiled defaults. Malformed or newer unsupported TOML is preserved
and reported instead of being overwritten.
Configured MCP servers are started by Rustcode before ACP prompts are handled;
ACP's optional MCP-over-ACP transport is not required.

## Configuration

### Configuration files

The TOML file contains the `default` model selection, `models` array, runtime
preferences, MCP servers, tool protocol, agent mode, verbosity, theme, active
session, and the per-turn `max_tool_rounds` safety backstop. It defaults to 40
rounds and is only a final limit after semantic loop and failure guards.
Writes use a temporary file and replacement so an interrupted save does not
leave a truncated configuration. On Unix, the file is written with owner-only
permissions because model profiles may contain API keys.

### Project configuration

Create a project-local override with:

```bash
rustcode init
# or: rustcode --init
```

This creates `.rustcode/config.toml` from the global model defaults and adds
`.rustcode/config.toml` to the project `.gitignore`. It intentionally does not
copy API keys, MCP servers, or session state. Project configuration is loaded
from parent to child directories, so the nearest file wins:

```text
CLI overrides > nearest project config > global config > built-in defaults
```

Project files are partial overrides; omitted fields continue to come from the
lower-precedence layer.

## IMPORTANT

If you wanna run `rustcode` using Apple FM you NEED to be on [MacOS 27 and have XCode v27](https://developer.apple.com/videos/play/wwdc2026/334/) for this to work. As this was introduced in the Beta version of MacOS 27.

Also not recmomended to use FM system model. as it only have like 2k context window...

Made with [rust](https://www.rust-lang.org/) by goat (me) and models inside [rustcode](README) harness

