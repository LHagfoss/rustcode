---
name: rustcode-themes
description: Create or edit a RustCode theme, the TOML file that colours both the terminal UI and the desktop app. Use when the user asks to change RustCode's colours, make a theme, or restyle the app.
triggers: rustcode theme, theme file, restyle the app, app colours, app colors
keywords: theme, palette, colour, color, dark mode, light mode, accent, contrast, desktop
---

# RustCode themes

One TOML file per theme. User themes: `rustcode theme path` (the
`themes/` directory of the config). Project themes: `.rustcode/themes/`.
A file with a built-in's name overrides it.

## Format

```toml
name = "mine"            # file is mine.toml; lowercase, digits, - _
description = "..."
# Terminal UI, all required. "#rrggbb", or "reset" for the terminal's own.
bg panel element text muted primary secondary green selection tip
status_border turn_separator notice_bg hover_bg

[desktop]                # optional; every entry optional
mode = "dark"            # dark | light
radius = "default"       # sharp | default | round
bg sidebar surface surface_2 surface_3 border border_strong
text text_muted text_faint accent accent_text danger success warning
focus selection code_bg scrollbar      # "#rrggbb" only
```

Desktop tokens: `bg` window, `sidebar`, `surface` cards and composer,
`surface_2` hover, `surface_3` selected or pressed, `border` hairlines,
`border_strong` control outlines, `text`, `text_muted` labels, `text_faint`
placeholders and timestamps, `accent` primary buttons and links,
`accent_text` text on accent, `danger`, `success`, `warning`, `focus` ring,
`selection` selected text background, `code_bg`, `scrollbar` thumb.

## Derivation

A token left out is derived from the terminal keys: sidebar = panel,
surface_2 = hover_bg, surface_3 = element, border_strong = status_border,
text_muted = muted, accent = focus = primary, success = green, warning = tip;
the rest are mixes of `bg` toward `text`. So a terminal-only theme still
styles the app, and setting `[desktop] bg` re-steps every surface from it.
Set a token only when the derived one is wrong.

## CLI

- `rustcode theme new <name> --from <theme>`: commented copy to edit.
- `rustcode theme show <name>` (`--json`): resolved tokens, derived marked.
- `rustcode theme validate <name>`: errors (bad colour, bad mode or
  radius, missing key) and warnings (unknown key, text on bg below 4.5:1).
- `rustcode theme set <name>`, `rustcode theme list`.

## Design rules

- Neutral greys: one hue family. Do not mix warm and cool greys.
- Surfaces step up 3-5% lightness from `bg` (dark) or down (light);
  borders barely above the surface they sit on.
- No pure `#000000` or `#ffffff` backgrounds; dark bg near `#0b0b0c`.
- Text contrast at least 4.5:1 on `bg`; `text_muted` at least 3:1.
- One restrained accent, used for primary actions, links and focus only.
  `accent_text` must read on it.
- `danger`, `success`, `warning` muted enough to sit beside the accent.

## Iterate

1. `rustcode theme new mine --from rustcode` (or edit an existing file).
2. Edit colours; keep the file valid TOML.
3. `rustcode theme validate mine`; fix every error.
4. `rustcode theme set mine`. The desktop app restyles within a second of
   each save; the terminal UI picks it up on `/theme` or restart.
