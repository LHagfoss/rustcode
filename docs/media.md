# Audio and video tools

## Optional local audio generation (Apple Silicon)

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

## Native declarative video editing

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
    "transitions": [{ "after_clip": 0, "type": "crossfade", "duration": 0.5 }],
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
