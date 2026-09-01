<div align="center">
<h1>rustcode</h1>

<img src="./images/header.png" alt="rustcode screenshot 1"/>
</div>

## about

`rustcode` is a lightweight Terminal User Interface (TUI) agent harness.
Originally made for testing Apple's on-device Foundation Models. Turned into a way deeper project.
Now supports ollama or openai compatible APIs.

## Documentation

- [Background tasks and cancellation](docs/background-tasks.md)
- [ACP server integration](docs/acp.md)
- [Provider stream traces](docs/provider-stream-traces.md)
- [Runtime and workspace architecture](docs/architecture.md)
- [Build-boundary benchmark](scripts/bench-build-boundaries.md)

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

Official release binaries are published for Linux x86_64, macOS Apple Silicon
(ARM64), and Windows x86_64. Intel macOS is not supported by the prebuilt
installer or Homebrew formula. Building from source may support additional
targets, but those targets are not covered by release CI.

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

ACP supports background command completion and continuation. A background tool
call is reported as `InProgress`, its terminal update retains the provider's
original tool-call ID, and the same logical turn resumes after completion.
Cancelling a prompt never revives that prompt when its detached process later
finishes; the completion is still persisted in the session. `session/close`
cancels the active turn and that session's running tasks. See
[docs/acp.md](docs/acp.md) for lifecycle and integration details.

## Non-interactive prompt

Run one prompt without opening the TUI:

```bash
rustcode --prompt "inspect this repository and run its tests"
```

Add `--yolo` only in a trusted workspace when the run should automatically
approve tool confirmations. Background commands started by the turn are
tracked until their terminal result is delivered; unrelated tasks already
running in the same session do not delay the turn.

## Background commands

The `run_command` tool can detach long-running work with `background: true`.
RustCode returns a task ID immediately and delivers the final output
automatically. The model should not poll `manage_task` in a loop. `manage_task`
is intended for an occasional `list`, `status`, or explicit `kill` operation.

Tasks are isolated by session. A task ID from one session cannot be used to
terminate another session's process. Cancellation terminates the complete
process group on Unix and the process tree on Windows, including descendants.
See [docs/background-tasks.md](docs/background-tasks.md) for exact behavior.

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

### Optional local audio generation (Apple Silicon)

RustCode can generate project-local WAV effects and instrumental music through
external MLX backends. Audio tools are enabled by default and discover the
backends automatically; override them in `config.toml` when needed:

```toml
[audio]
enabled = true
sfx_backend = "auto"
music_backend = "auto"
```

The explicit backend values are `"mlx-speech"` for sound effects and
`"musicgen-mlx"` for music.

For sound effects, create an Apple Silicon Python environment and install the
`mlx-speech` package (Python 3.13+):

```bash
python3 -m venv ~/.local/share/rustcode/audio-venv
source ~/.local/share/rustcode/audio-venv/bin/activate
pip install mlx-speech
```

RustCode discovers the venv's `bin` directory automatically, including when
launched from the macOS Dock.

For music (Python 3.10+), keep the audio venv active and install the
`musicgen-mlx` project:

```bash
git clone https://github.com/andrade0/musicgen-mlx.git
cd musicgen-mlx
make install
```

`make install` installs `musicgen-mlx` under `~/.local/bin`; RustCode also
discovers that directory automatically.
The sound-effect command is `mlx-speech`; RustCode invokes its sound-effect
model through the backend interface. See the upstream
[mlx-speech documentation](https://github.com/appautomaton/mlx-speech) and
[musicgen-mlx documentation](https://github.com/andrade0/musicgen-mlx) for
current Apple Silicon and Python requirements. The first generation downloads
the model lazily, so the first call can take substantially longer. The initial
music model is about 1.2 GB, while the sound-effect model and larger music
variants can require several GB. RustCode never permanently loads these models
into its own process. The initial native path intentionally accepts and
inspects WAV output only; music longer than 30 seconds and additional audio
formats are deferred.

### Native declarative video editing

RustCode can inspect and compose project-local media through external
`ffprobe` and `ffmpeg` processes. Install FFmpeg through the package manager for
your platform, then use `inspect_media`, `validate_video_project`, and
`render_video`. RustCode never accepts raw FFmpeg arguments from the model.

Video edits are stored in a reusable, versioned project file:

```json
{
  "version": 1,
  "output": "output/final.mp4",
  "video": { "width": 1920, "height": 1080, "fps": 30 },
  "clips": [
    { "path": "media/intro.mp4", "trim": { "start": 1.5, "end": 8.0 } },
    { "path": "media/demo.mp4" }
  ],
  "transitions": [
    { "after_clip": 0, "type": "crossfade", "duration": 0.5 }
  ],
  "audio": {
    "music": {
      "path": "media/music.wav",
      "volume": 0.2,
      "fade_in": 1.0,
      "fade_out": 2.0
    },
    "keep_clip_audio": true,
    "clip_audio_volume": 1.0
  }
}
```

Only `output` and `clips` are required. Defaults are 1920x1080 at 30 FPS with
clip audio preserved. Supported transitions are `crossfade`, `fade`,
`wipe-left`, `wipe-right`, `slide-left`, and `slide-right`. Inputs are
normalized before composition and output is MP4/H.264 with optional AAC audio.

## IMPORTANT

If you wanna run `rustcode` using Apple FM you NEED to be on [MacOS 27 and have XCode v27](https://developer.apple.com/videos/play/wwdc2026/334/) for this to work. As this was introduced in the Beta version of MacOS 27.

Also not recmomended to use FM system model. as it only have like 2k context window...

Made with [rust](https://www.rust-lang.org/) by goat (me) and models inside [rustcode](README) harness
