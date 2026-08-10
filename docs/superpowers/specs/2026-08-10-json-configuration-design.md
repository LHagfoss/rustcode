# JSON Configuration Design

## Goal

Move Rustcode's persistent configuration from the deprecated `config.toml` file
to `models.json` and `config.json`, while keeping built-in Rust defaults as the
fallback and ensuring startup never overwrites user configuration.

## Decisions

- `config.toml` is deprecated immediately. It is no longer read, written, or
  migrated automatically.
- `models.json` contains model defaults and model profiles.
- `config.json` contains runtime preferences and integrations: tool protocol,
  MCP servers, agent mode, verbosity, theme, active session, and debug logging.
- The files are optional. If either file is absent or invalid, Rustcode uses the
  corresponding in-code defaults for that file.
- Invalid JSON is reported with a warning, but the invalid file is preserved and
  never replaced with defaults. No automatic `.bak` file is created by this
  configuration loader.
- Explicit settings changes continue to persist, but only to the relevant JSON
  file. Startup and fallback loading never persist defaults.
- ACP and interactive execution use the same configuration loader, so model
  selection and tool configuration do not depend on the frontend.

## File formats

`models.json`:

```json
{
  "default": { "big": "gemini-3.6-flash", "small": "gemini-3.6-flash" },
  "models": [
    {
      "name": "gemini-3.6-flash",
      "url": "http://localhost:3000/v1/chat/completions",
      "model": "gemini-3.6-flash",
      "context_window": 128000,
      "engine": "openai"
    }
  ]
}
```

`config.json`:

```json
{
  "tool_protocol": "json",
  "mcp_servers": [],
  "agent_mode": "build",
  "verbosity": "normal",
  "debug_verbose_network_logging": false,
  "theme": "default",
  "last_active_session_id": null
}
```

Model profile fields retain their current semantics, including optional API
keys, environment-key references, protocol overrides, thinking controls, and
token limits. Unknown JSON fields remain forward-compatible through Serde's
default behavior.

## Loading and saving

The existing `AppConfig` remains the in-memory configuration consumed by the
rest of the application. `load_config_from` reads the two JSON documents and
merges them with independent defaults. Model endpoint resolution continues to
use the configured big-model name, then the first configured model, then the
built-in default profile.

Persistence is split by ownership: model/default changes write `models.json`,
while runtime and integration changes write `config.json`. The old TOML path is
removed from the persistence code so no normal operation can recreate it.

## ACP implications

ACP session creation supplies the workspace root, but does not supply model
profiles. Rustcode must load `models.json` and `config.json` inside
`AppState::new()` exactly as interactive mode does. The ACP path must also use
the same enabled-MCP-server initialization as interactive/headless startup so
configured MCP tools are available consistently.

## Error handling

- Missing JSON: use in-code defaults silently or with an informational log.
- Malformed JSON: warn with the path and parse error; preserve the file and use
  that file's in-code defaults.
- Empty model list: use built-in model profiles so endpoint resolution remains
  valid.
- Invalid selected default: resolve to the first available model, then the
  built-in endpoint.

## Testing

Tests will cover independent loading of `models.json` and `config.json`, missing
files, malformed files without replacement, split persistence, default model
resolution, and ACP startup using the same configuration/MCP initialization
path.
