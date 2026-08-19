<div align="center">
<h1>rustcode</h1>

<img src="./images/header.png" alt="rustcode screenshot 1"/>
</div>

## about

`rustcode` is a lightweight Terminal User Interface (TUI) agent harness.
Originally made for testing Apple's on-device Foundation Models. Turned into a way deeper project.
Now supports ollama or openai compatible APIs.

## running it

Needs [homebrew](https://brew.sh/) or [rust toolchain](https://rust-lang.org/tools/install/) installed.

### installation (cargo)

1. Clone repo:

```bash
git clone https://github.com/lhagfoss/rustcode.git
```

2. Build and run:

```bash
 cargo build --release
 cargo run --release
```

OR you can install it via `cargo install` and run it from anywhere:

```bash
 cargo install --path .
 rustcode
```

### ACP runtime

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

### Configuration files

The TOML file contains the `default` model selection, `models` array, runtime
preferences, MCP servers, tool protocol, agent mode, verbosity, theme, active
session, and the per-turn `max_tool_rounds` safety backstop. It defaults to 40
rounds and is only a final limit after semantic loop and failure guards.
Writes use a temporary file and replacement so an interrupted save does not
leave a truncated configuration. On Unix, the file is written with owner-only
permissions because model profiles may contain API keys.

### via homebrew

You can easily install it using Homebrew (Needs Apple Silicon).
Just run the following command in your terminal:

```bash
# 1. Tap the repository
brew tap lhagfoss/tap

# 2. Trust the tap (required by Homebrew for new/custom taps)
brew trust lhagfoss/tap

# 3. Install the harness
brew install rustcode
```

## keeping it upgraded

To update to the latest release in the future ,just run:

```bash
brew upgrade rustcode
```

## IMPORTANT

If you wanna run `rustcode` using Apple FM you NEED to be on [MacOS 27 and have XCode v27](https://developer.apple.com/videos/play/wwdc2026/334/) for this to work. As this was introduced in the Beta version of MacOS 27.

Also not recmomended to use FM system model. as it only have like 2k context window...

Made with [rust](https://www.rust-lang.org/) by goat (me) and models inside [rustcode](README) harness
