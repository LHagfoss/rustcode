# Themes

One theme system colours both the terminal UI and the desktop app. A theme is
a TOML file; the engine (`rustcode/engine/src/themes/`) parses, derives and
validates it, the terminal UI renders its terminal keys, and the desktop app
gets its desktop tokens over ACP. Users edit theme files by hand, with
`rustcode theme`, or by asking an agent (the built-in `rustcode-themes` skill).

## Where themes come from

| Source | Location | Notes |
| --- | --- | --- |
| `builtin` | compiled in (`rustcode/engine/src/themes/builtin/`) | also copied to the user directory on first run |
| `user` | `<config>/themes/*.toml` (`rustcode theme path`) | synced by `rustcode sync` |
| `project` | `.rustcode/themes/*.toml` in the workspace or an ancestor | closer directory wins |

Themes are matched by `name`, case-insensitively: a project theme overrides a
user theme of the same name, which overrides a built-in. A user file that is
byte-identical to a built-in is reported as `builtin`. A copy of a built-in
written by an older release (the terminal keys alone, or an earlier version of
the whole file, unedited) is upgraded to the current built-in at startup; an
edited copy is never touched. Files that do not parse are left out of the
list; `rustcode theme validate` says why.

The active theme is `theme` in the config (`/theme`, `rustcode theme set`,
`_rustcode/themes/set`). The default is `rustcode`, a neutral graphite dark
(new configs only: an existing config keeps the theme it names). Built-ins:
`rustcode`, `catppuccin-mocha`, `default`, `rain`, `cozy-rain`, `light`,
`nord`, `dracula`, `tokyo-night`, `sky`.

## Format

```toml
name = "mine"                      # lowercase letters, digits, - and _
description = "My theme"           # optional

# Terminal UI: all required. "#rrggbb", [r, g, b], or "reset"
# ("none", "transparent") for the terminal's own colour.
bg = "reset"
panel = "#141416"
element = "#212124"
text = "#ededed"
muted = "#a1a1a1"
primary = "#93a3e8"
secondary = "#8a8f98"
green = "#5fbf8a"
selection = "#ededed"
tip = "#e0b25c"
status_border = "#2c2c30"
turn_separator = "#3a3a3f"
notice_bg = "reset"
hover_bg = "#1a1a1d"

# Desktop app: optional, and so is every entry in it.
[desktop]
mode = "dark"                      # dark | light
radius = "default"                 # sharp | default | round
bg = "#0b0b0c"
# ... any of the tokens below, "#rrggbb" (or [r, g, b]) only
```

`rustcode theme new <name> [--from <theme>]` writes a file like this with every
key and token filled in from the base theme and a comment on each.

### Terminal keys

| Key | Colours |
| --- | --- |
| `bg` | terminal background |
| `panel` | panels, composer, cards |
| `element` | selected rows, raised elements |
| `text` | body text |
| `muted` | secondary text, hints |
| `primary` | accent: active items, the prompt mark |
| `secondary` | second accent |
| `green` | success, added lines |
| `selection` | selected text |
| `tip` | tips and warnings |
| `status_border` | status bar and box borders |
| `turn_separator` | line between turns |
| `notice_bg` | notice background |
| `hover_bg` | hovered rows |

### Desktop tokens

| Token | Colours | Derived from (when left out) |
| --- | --- | --- |
| `bg` | window and chat background | `bg`; if `reset`: `panel` mixed 25% toward black (dark) or 50% toward white (light) |
| `sidebar` | sidebar | `panel`, else `step(2%)` |
| `surface` | cards, composer, popovers | `step(4%)` |
| `surface_2` | hovered, raised surfaces | `hover_bg`, else `step(7%)` |
| `surface_3` | selected rows, pressed controls | `element`, else `step(11%)` |
| `border` | hairlines between areas | `bg` mixed 60% toward `status_border`, else `step(8%)` |
| `border_strong` | control outlines | `status_border`, else `step(16%)` |
| `text` | body text | `text` |
| `text_muted` | secondary text, labels | `muted`, else `text` mixed 35% toward `bg` |
| `text_faint` | placeholders, timestamps, disabled | `text_muted` mixed 40% toward `bg` |
| `accent` | primary buttons, links, active state | `primary` |
| `accent_text` | text on an accent fill | `#ffffff` or `#0b0b0c`, whichever contrasts more with `accent` |
| `danger` | errors, destructive actions | `#f26d6d` (dark), `#c62828` (light) |
| `success` | success, added lines | `green` |
| `warning` | warnings, pending approvals | `tip` |
| `focus` | keyboard focus ring | `accent` |
| `selection` | selected text background | `bg` mixed 25% toward `accent` |
| `code_bg` | code blocks, terminal output | `step(2.5%)` |
| `scrollbar` | scrollbar thumb | `step(15%)` |

`step(t)` is the resolved desktop `bg` mixed `t` toward the resolved desktop
`text` (per channel, rounded). Tokens resolve in table order and each
derivation uses the tokens already resolved, so setting `[desktop] bg` alone
re-steps every surface from it. A terminal key that is `reset` or invalid
counts as missing; then `text` falls back to `#ededed` (dark) or `#1a1a1a`
(light), `accent` to `#93a3e8` / `#3b5bdb`, `success` to `#5fbf8a` /
`#1f7a35`, `warning` to `#e0b25c` / `#9a5f12`.

`mode`, when left out, is `light` when the desktop `bg` (else the terminal
`bg`) is light by the terminal UI's test (`(299r + 587g + 114b) / 1000 > 140`);
with a `reset` background it is `light` when `text` is dark. `radius` defaults
to `default`.

## Validation

`rustcode theme validate [<name>|<path>]` (no argument: every built-in, user
and project file) prints each file's problems and exits 1 on any error.

- Errors: invalid TOML, a missing or mistyped terminal key, an empty `name`,
  a colour that is not `#rrggbb`/`reset`, `reset` in `[desktop]`, a `mode`
  other than `dark`/`light`, a `radius` other than `sharp`/`default`/`round`,
  `desktop` not a table. A bad colour is ignored where it is used: the
  terminal UI draws it as `reset` (as it always has), the desktop derives it.
- Warnings: an unknown key (top level or in `[desktop]`) is ignored; the
  resolved desktop `text` on `bg` below 4.5:1 contrast (WCAG AA).

## CLI

```bash
rustcode theme list                         # * marks the active theme
rustcode theme show <name> [--json]         # resolved tokens; --json as ACP reports it
rustcode theme new <name> [--from <theme>]  # commented file in the user directory
rustcode theme set <name>                   # make it active in the user config
rustcode theme validate [<name>|<path>]
rustcode theme path [<name>]                # user directory, or the theme's file
```

## ACP

- `_rustcode/themes` (`{"cwd"?}`): the snapshot below. `cwd` adds that
  project's themes and is remembered for the watcher.
- `_rustcode/themes/set` (`{"name", "cwd"?}`): writes `theme` to the user
  config (as `/theme` does) and answers with the snapshot; an unknown name is
  `invalid_params`.
- `_rustcode/themes/changed` (notification, params = the snapshot): sent when
  a theme file in the user or project directory, or the user config, changed
  and then held still for 200 ms. It starts once the client first sends
  `_rustcode/themes` or `_rustcode/themes/set`.

```json
{
  "active": "rustcode",
  "themes": [
    {
      "name": "rustcode",
      "description": "Neutral graphite dark, the RustCode default",
      "source": "builtin",
      "path": "/Users/me/.config/rustcode/themes/rustcode.toml",
      "mode": "dark",
      "tokens": {
        "bg": "#0b0b0c", "sidebar": "#151517", "surface": "#1d1d20",
        "surface_2": "#242428", "surface_3": "#2a2a2e", "border": "#1f1f22",
        "border_strong": "#2c2c30", "text": "#ededed", "text_muted": "#a1a1a1",
        "text_faint": "#6b6b6b", "accent": "#93a3e8", "accent_text": "#0b0b0c",
        "danger": "#f26d6d", "success": "#5fbf8a", "warning": "#e0b25c",
        "focus": "#93a3e8", "selection": "#252a3d", "code_bg": "#101012",
        "scrollbar": "#2a2a2e"
      },
      "radius": "default"
    }
  ]
}
```

`source` is `builtin`, `user` or `project`; `path` is absent for a built-in
with no file on disk. Every token is always present, lowercase `#rrggbb`. The
default theme comes first, the rest by name. `active` is the configured theme
as the list spells it, or the default when the config names a missing theme.

## Skill

The built-in skill `rustcode-themes`
(`rustcode/engine/src/skills_builtin/rustcode-themes.md`) teaches agents the
format, tokens, derivation, CLI and design rules (neutral palette, contrast,
no pure black, a restrained accent). An agent edits the file, runs
`rustcode theme validate`, and the desktop app restyles on save through
`_rustcode/themes/changed`.
