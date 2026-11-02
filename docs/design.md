# Desktop design system

The rules every change to the desktop app (`apps/desktop`) follows. The
tokens live in one place, `apps/desktop/src/index.css` (CSS variables and
the Tailwind `@theme` that maps them to classes), with the built-in themes
mirrored in `apps/desktop/src/lib/themes.ts`. Theme files (user and project
themes, the CLI and the engine's `_rustcode/themes` methods) are described
in [themes.md](themes.md).

`src/design.test.ts` fails the build on a raw colour (`#hex`, `rgb()`,
`hsl()`) or a Tailwind palette class (`zinc-*`, `white`, `black`, …)
outside the tokens files, and on the retired class names listed at the end.

## Principles

- **Calm.** The transcript is the content; chrome stays quiet. One accent,
  used sparingly. Nothing moves, glows or changes colour unless the user did
  something or the state changed.
- **Neutral.** Greys are true neutrals (no warm or blue cast), and every
  grey comes from a token. Translucent fills are drawn from `--rc-text`, so
  they stay neutral on any background.
- **Dark first.** The dark theme is designed first and is the default; the
  light theme follows the same structure.
- **Minimal borders.** Separate with a step in surface, not a line. A 1px
  hairline only where two equal surfaces meet with nothing else between
  them (a list's rows inside one card, a panel edge).
- **One shell.** Sidebar, header and panes stay mounted; views swap inside
  the main column. Pressing or hovering never remounts anything.
- **Opaque panes, glass floats.** Panes, cards and rows are opaque;
  what floats over them shares one glass (below).

## Tokens

Every theme sets exactly these, as CSS variables `--rc-<token>`. In theme
files and the engine snapshot the names use underscores (`surface_2`,
`text_muted`).

| Token | Use | Default dark | Default light |
|---|---|---|---|
| `bg` | The chat and main panes | `#0b0b0c` | `#ffffff` |
| `sidebar` | The sidebar | `#0e0e10` | `#f7f7f8` |
| `surface` | Resting cards: composer, settings groups, error cards | `#141416` | `#f4f4f5` |
| `surface-2` | Raised: popovers, menus, dialogs, tooltips, toasts | `#1a1a1d` | `#ffffff` |
| `surface-3` | Raised over raised: a selected segment, kbd | `#232327` | `#e9e9eb` |
| `border` | Hairlines and the edge ring of raised surfaces | `#ffffff0f` | `#0000000f` |
| `border-strong` | Inputs, dividers that must read | `#ffffff1c` | `#0000001f` |
| `text` | Primary text and icons | `#ededed` | `#171717` |
| `text-muted` | Secondary text, idle icons | `#a1a1a1` | `#5c5c5c` |
| `text-faint` | Metadata, placeholders, disabled | `#6b6b6b` | `#9a9a9a` |
| `accent` | Links, toggles on, progress, selection marks | `#5b8def` | `#2f6fe4` |
| `accent-text` | Text and icons drawn on `accent` | `#ffffff` | `#ffffff` |
| `danger` | Errors, destructive actions | `#f26d6d` | `#d93d3d` |
| `success` | Passed, connected, added lines | `#4ec27a` | `#1f9d55` |
| `warning` | Needs you, warnings | `#e5a54b` | `#b7791f` |
| `focus` | The keyboard focus ring (drawn at 40%) | `#5b8def` | `#2f6fe4` |
| `selection` | Selected text | `#5b8def4d` | `#2f6fe433` |
| `code-bg` | Code blocks, diffs, terminal | `#111113` | `#f6f6f7` |
| `scrollbar` | Scrollbar thumb | `#ffffff26` | `#00000026` |

Derived values are computed from the tokens in `index.css` and are never
set by a theme:

| Derived | Value | Use |
|---|---|---|
| `fill-subtle` | `text` at 4.5% | Resting fill of quiet controls (search field, cluster), hover on rows |
| `fill` | `text` at 8% | Hover on controls, the selected row, chips |
| `fill-strong` | `text` at 13% | Pressed, an "on" icon button, switch track off |
| `--rc-diff-add` / `--rc-diff-del` | `success` / `danger` at 14% | Diff line backgrounds |
| `--rc-shadow-1` / `--rc-shadow-2` | see Elevation | |

Tailwind classes use the token names, except the text family, which is
`fg`: `bg-bg`, `bg-sidebar`, `bg-surface`, `bg-surface-2`, `bg-surface-3`,
`border-border`, `border-border-strong`, `text-fg`, `text-fg-muted`,
`text-fg-faint`, `bg-accent`, `text-accent`, `text-accent-fg`,
`text-danger`, `bg-code-bg`, `bg-fill`, `bg-fill-subtle`,
`bg-fill-strong`. Opacity modifiers work on any of them (`bg-danger/10`).

Primary actions are **inverted neutral** (`bg-fg text-bg`), not accent:
the accent marks state (on, selected, linked), it does not shout.

## Radius

| Step | px | Tailwind | Used by |
|---|---|---|---|
| xs | 4 | `rounded-xs` | kbd, inline code, tiny badges, checkboxes |
| sm | 6 | `rounded-sm` | 22px icon buttons inside rows, branch chips, small inputs |
| md | 8 | `rounded-md` | Buttons that are not pills, sidebar rows, menu items, list rows, inputs, tooltips |
| lg | 12 | `rounded-lg` | Cards (tool output, error card, settings groups), popovers, menus, toasts |
| xl | 16 | `rounded-xl` | The composer, dialogs, the command palette, large panels |
| full | — | `rounded-full` | Pills, toolbar clusters, switches, segmented controls, avatars, dots |

A theme's `radius` (`sharp`, `default`, `round`) scales the steps
(`sharp`: 2/3/4/6/8, `round`: 6/8/10/16/20); components only ever name a
step. No arbitrary radii (`rounded-[14px]`), no `rounded-2xl`+. A child
inside a padded parent uses the step below the parent's.

## Borders

- Hairline is always 1px `border-border`; never 0.5px, never 2px.
- Prefer a surface step: a card is `bg-surface` on `bg-bg`, with no border.
- Raised surfaces get their edge from `--rc-shadow-*`, which includes a 1px
  `border` ring; do not add `border` to a popover or menu as well.
- Inputs use `border-strong` as an inset ring only when they sit on `bg`
  with no surface around them; inside a card the fill alone is enough.
- No coloured borders for state (an error card is not outlined in red; a
  focused field does not turn blue).

## Focus

- Only `:focus-visible` shows a ring: 2px `focus` at 40%, offset 2px. It is
  global in `index.css`; components do not add their own focus colours.
- Never on mouse click, never on the composer's text field. The composer
  shows focus by its surface stepping from `surface` to `surface-2`.
- `outline-none` is allowed only on a text field whose container already
  shows focus (the composer, a search capsule).

## Elevation

Two levels, both including the 1px edge ring:

- `--rc-shadow-1` (`shadow-1`): things resting above the chat: the
  jump-to-latest button, toasts, queued messages, error cards over the
  transcript. (The composer has its own softer two-sided shadow.)
- `--rc-shadow-2` (`shadow-2`): things that open over everything:
  popovers, menus, tooltips, dialogs, the command palette.

The `elevated` utility is glass (`--rc-glass-bg-strong`) + `shadow-2` +
`rounded-lg`: use it for every popover, menu and tooltip.

## Glass

One translucency, one token pair, for every floating surface: the
composer, popovers, menus, tooltips, dialogs, toasts, the right panel and
the lightbox. `--rc-glass-bg` (`surface-2` at 68%; `--rc-glass-bg-strong`
at 80% for popovers, menus and the focused composer) with
`--rc-glass-filter` (`saturate(1.5) blur(22px)`). Use `.rc-glass` or the
`elevated` utility, never a hand-made `backdrop-filter`. Reduce
transparency (the media query, or the system setting the shell reads)
turns the pair solid. The header over a new thread's banner (`.rc-frost`)
is the one other blur. Panes, cards, rows and the sidebar stay opaque.

## Spacing

Tailwind's 4px grid. Common steps: 4 (icon to label), 8 (between controls
in a row), 12 (card padding block), 14 (card padding inline), 16 (side
gutter, between groups), 24 (between sections). The chat column is at most
980px wide with 28/20/16px gutters by width.

## Type

macOS text styles, SF Pro / SF Mono:

| Class | Size / line | Use |
|---|---|---|
| `text-caption` | 10/13 | Metadata, badges |
| `text-sub` | 11/14 | Sidebar secondary lines, section headers |
| `text-callout` | 12/16 | Controls, chips, secondary copy |
| `text-body` | 13/18 | Body, rows, transcript |
| `text-title3` | 15/20 | Card titles |
| `text-title2` | 17/22 | Page titles (Settings) |
| `text-title` | 22/28 | Empty-state heading |

Weights: 400 body, 500 controls and selected rows, 600 titles. Numbers
that change use `tabular`.

## Motion

- Hover: **in 60ms, out 120ms** (`--rc-hover-in`, `--rc-hover-out`),
  colour only, set once in `index.css` for every button, link, row,
  switch and menu item. Components never add `transition-colors` or their
  own hover duration. Press: `active:bg-fill-strong`, no delay. No
  `transform` on press (no scale): text re-rasterizes and reads as a flash.
- Hover is pure CSS `:hover` (or Radix's `data-highlighted`). No hover
  state in React (`onMouseEnter` setting an id); a keyboard-driven list may
  keep a selected index, shown with the same `bg-fill`. Row menus are
  `modal={false}`, so other rows keep answering the pointer while one is
  open.
- Opening: popovers, menus, tooltips 160ms `--ease-snappy` (fade + 4px);
  closing 120ms `ease-in`. Dialogs 180ms. Panel drill-in 220ms.
- Nothing animates layout on hover. Height transitions only for folding
  sections.
- `prefers-reduced-motion`: all durations collapse to ~0; shimmer stops.

## Tooltips

One `Tooltip` (`ui/primitives.tsx`) under one `Tooltip.Provider`: 400ms
for the first, then instant while the pointer moves between triggers
(`skipDelayDuration` 300ms), closing at once. The tip takes no pointer
events, opens on top by default, 8px from its trigger and 10px from the
window's edges, and flips when there is no room. Controls in the composer
tray open theirs below, clear of the box. Inside a popover, hints are
secondary text, not tooltips.

## Cursor

A pointer on everything clickable (buttons, rows, chips, switches, radios,
tabs, menu items, links), set on the control so its children inherit it;
the default arrow on static text; the text cursor only in fields and
`.selectable` text. Disabled controls show the arrow.

## Icons

Lucide, stroke 2 (2.5 for chevrons at 10–12px). Sizes: 12 inside chips and
dense rows, 13 in menus, 14 in toolbar and composer buttons, 16 in page
headers, 18 in empty states. Icon colour follows text (`text-fg-muted` idle,
`text-fg` on hover).

## Scrollbars

Overlay where the system allows (macOS default), thin elsewhere:
`scrollbar-width: thin` in `scrollbar`, transparent until the scroller is
hovered or scrolling, and `scrollbar-gutter: stable` on long scrollers so
content never shifts when a classic scrollbar appears.

## Component recipes

Shape rule: **labelled buttons are pills** (`rounded-full` with
horizontal padding, never a circle): tray buttons, the model pill, chips,
segmented items, header actions, Settings' "New…" and "Edit config".
Circles only for icon-only round buttons (send, stop, close in a panel
header, toolbar clusters). Icon buttons in dense rows are `rounded-md`.
Rows and menu items are `rounded-md`, cards `rounded-lg`, popovers and
panels `rounded-xl`.

- **Button, primary:** `rounded-full h-[26px] px-3 bg-fg text-bg font-medium`;
  hover 90%. **Secondary:** `bg-fill text-fg`, hover `bg-fill-strong`.
  **Danger:** `bg-danger/12 text-danger`. **Ghost / icon:** transparent,
  `text-fg-muted`, hover `bg-fill text-fg`; 28px square, `rounded-md` (or
  `rounded-full` inside a cluster). Disabled: 40% opacity, no hover.
- **Input:** `h-8 rounded-md bg-fill-subtle px-3 text-callout`,
  placeholder `text-fg-faint`, no ring until `:focus-visible`.
- **Chip:** `h-6 rounded-full bg-fill px-2 text-callout text-fg-muted`;
  status chips tint with their colour at 12–14%.
- **Row (sidebar, list):** `rounded-md px-2.5 py-1`, hover `bg-fill-subtle`,
  selected `bg-fill` + `font-medium`; the hover action button appears with
  opacity only (no layout change).
- **Card:** `rounded-lg bg-surface p-3.5`, no border; rows inside separated
  by `divide-border`.
- **Popover / menu:** `elevated p-1`; items `rounded-md px-2.5 py-[5px]`,
  highlighted `bg-fill` (not accent); destructive items `text-danger`,
  highlighted `bg-danger/12`.
- **Dialog:** `elevated rounded-xl p-5` over a `bg-bg/60` scrim.
- **Toast:** `elevated rounded-full px-4 py-1.5 text-callout`.
- **Tooltip:** `bg-surface-3 rounded-lg px-2.5 py-1 text-callout shadow-2`
  (see Tooltips).
- **Composer:** `.rc-composer rounded-xl p-2` in glass (`--rc-glass-bg`,
  denser on focus), a soft shadow above and below plus the edge ring; no
  ring or colour on focus. The **+** is a filled circle (`bg-fill`). Its
  tray (`.rc-composer-tray`) is tucked under the box as in the SwiftUI app:
  12px narrower each side, its top 18px hidden behind the box (which sits
  above it in z), only its rounded bottom showing; solid `surface-2`, no
  border. The transcript scrolls under both (its bottom padding is the
  stack's height) and fades out from 56px above the box.
- **Right panel:** one `RightPanel` for every side view (Changes, Agents,
  Pull requests): floating, inset 8px from the window's top, right and
  bottom, `rounded-xl bg-surface shadow-2`, no divider. Its header row has
  a segmented switch between the views and a close button; views add a
  slim toolbar (`h-9`) under it. The left edge drags (320–760px, kept);
  it slides and fades in 200ms while its slot's width animates, so the
  chat reflows with it. ⌘⌥B toggles it with the last view.
- **Jump to latest:** `.rc-glass rounded-full shadow-1`, centred 12px
  above the composer stack, above it in z.
- **Error card:** `rounded-lg bg-surface px-3.5 py-3`, the icon in `danger`
  (or `warning` for notices), text `text-fg-muted`, actions at the right
  as secondary pills. No coloured border or tinted background.

## Do / Don't

| Do | Don't |
|---|---|
| Use a token class or `var(--rc-…)` | Write `#hex`, `rgb()`, `zinc-800`, `text-white` |
| Separate with a surface step | Outline every card with a border |
| Name a radius step | Write `rounded-[14px]` |
| Show focus with `:focus-visible` only | Recolour a field's border on focus |
| Mark state with the accent | Fill primary buttons with the accent |
| Keep subtrees mounted; toggle visibility | Remount a view on open, or key it on hover |
| Change colour or opacity on hover | Change size, padding or border width on hover |
| Put hints in secondary text inside popovers | Let a tooltip cover a neighbouring control |
| Use the glass tokens for floating surfaces | Hand-tune a `backdrop-filter` or blur a pane |
| Make a labelled button a pill | Make a labelled button a circle |
| Let `index.css` time hover | Add `transition-colors duration-200` to a control |

Retired class names (failing `design.test.ts`): `fg-2`, `fg-3`, `fg-4`,
`fill-1`…`fill-4`, `hairline`, `brand`, `brand-fill`, `chat`, `field`,
`attention`, `info`, the `surface` utility (now `elevated`), `shadow-float`,
`shadow-panel`.
