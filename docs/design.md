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
| `sidebar` | The sidebar | `#131315` | `#f7f7f8` |
| `surface` | Resting cards: composer, settings groups, error cards | `#18181a` | `#f0f0f1` |
| `surface-2` | Raised: popovers, menus, dialogs, tooltips, toasts | `#1e1e21` | `#ffffff` |
| `surface-3` | Raised over raised: a selected segment, kbd | `#232327` | `#e8e8ea` |
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

The surfaces step evenly in lightness (CIE L\*): dark `bg` 3.0, `sidebar`
6.0, `surface` 8.3, `surface-2` 11.4, `surface-3` 13.9; light `bg` 100,
`sidebar` 97.3, `surface` 94.8, `surface-3` 92.1, with light `surface-2`
white (raised surfaces are lifted by their shadow there). A theme that
changes them should keep the steps even.

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
| lg | 12 | `rounded-lg` | Cards (tool output, error card, settings groups), popovers, menus, toasts, pictures in replies |
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
  jump-to-latest button, queued messages, error cards over the
  transcript. (The composer and toasts share a softer two-sided shadow.)
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
turns the pair solid. Headers use the same filter tinted with the pane
instead of a surface (`--rc-glass-pane`, `bg` at 72%; 45% with `.rc-frost`
over a new thread's banner), through `.rc-header-glass`. Panes, cards,
rows and the sidebar stay opaque.

Glass is cheap where it is: WebKit draws `backdrop-filter` as a
CoreAnimation backdrop layer, and turning every blur off changed no frame
or CPU number measurably (docs/desktop.md › Native frame rate). Keep it
to the one filter on floating surfaces and headers: no larger radius, no
second blur stacked on the first, no blur on a pane or a scrolling row.

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
- Panels and sheets never animate a slot's `width`, `height`, `top` or
  grid rows across the chat: every frame would reflow the transcript (40 ms
  of work a frame for the right panel, so well under 120 fps). The slot
  takes its size at once and the panel moves with `transform` and
  `opacity` (`rc-drill` in, `translate`/`opacity` out). Animate only
  `transform`, `opacity` and colour; never `filter` or `box-shadow` on
  something that moves every frame.
- Toasts come in with `rc-in` (200ms, fade + 4px) and leave in 120ms
  `ease-in`; a swipe follows the pointer and snaps back in 160ms.
- Skeletons sweep once every 1.6s (`transform` only, on the compositor).
- `prefers-reduced-motion`: all durations collapse to ~0; shimmer and the
  skeleton sweep stop.

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
- **Toast:** `.rc-toast rounded-lg` (the composer's glass, denser, and its
  two-sided shadow), 340px wide, `py-2 pl-3 pr-1.5 text-callout`: an icon
  (info `text-fg-muted`, success `text-success`, error `text-danger`, a
  spinner while it has progress), the text in `text-fg` (up to 4 lines,
  selectable), at most one action as a secondary pill (`h-6 bg-fill`), and
  a close button. Progress is a 2px accent bar along its foot. One stack
  (`Toaster`), bottom right of the chat column, 12px above the composer
  stack and 16px from the column's edge (the window's corner over
  Settings); newest lowest, three at most. Info and success leave after
  4s, errors stay; hover pauses; swipe right or close dismisses. Use a
  toast for what is not tied to a message (copied, engine, worktree,
  harness and theme events, errors of an action); an error about a
  thread's turn stays an error card over the composer. No banners.
- **Skeleton:** `Skeleton` (`.rc-skeleton`): `fill-subtle` blocks the shape
  of what will appear (a text line `h-3`, a meta line `h-2`, a pill
  `rounded-full`), radius `sm` unless the real thing is rounder, varied
  widths so it does not read as a grid. Use it where content will appear
  (thread rows, the model pill, a transcript, Settings lists), never a
  spinner with "Loading…". A container of skeletons has `role="status"`
  and a label. Long waits on something that is not content (the engine
  starting) get a toast after 2s instead.
- **Header:** `MainHeader`, 42px, `.rc-header-glass` floating over its
  view (`absolute`, z 10): the view starts its content `HEADER_HEIGHT`
  down and scrolls under it, and the glass fades out over 24px below the
  header's edge, as the transcript fades above the composer. Every view
  that has a header uses this one (the chat, new or open; Settings).
- **Settings:** the whole window (the shell stays mounted underneath,
  hidden). Header: Back (a ghost pill with a chevron) and the title, on
  the traffic lights' line. Body: one centred column, `max-w-[760px]`, the
  section list (164px, sticky, rows per the Row recipe) at its left, the
  page (title2 heading, then groups) beside it.
- **Tooltip:** `bg-surface-3 rounded-lg px-2.5 py-1 text-callout shadow-2`
  (see Tooltips).
- **Composer:** `.rc-composer rounded-xl p-2` in the denser glass
  (`--rc-glass-bg-strong`, `surface-2` at 90% on focus), a soft shadow
  above and below plus the edge ring; no ring or colour on focus. Its
  controls rest one step above it: the **+** and the model pill are
  `bg-fill-subtle`, `bg-fill` on hover, `bg-fill-strong` when open. Its
  tray (`.rc-composer-tray`) is tucked under the box as in the SwiftUI app:
  12px narrower each side, its top 18px hidden behind the box (which sits
  above it in z), only its rounded bottom showing; solid `surface-2`, no
  border. The tray is `min-h-[58px]` with `px-1.5 pb-1.5`, so its 28px
  buttons have the same 6px above them (to the box's edge) as at its
  sides and foot. The transcript scrolls under both (its bottom padding is the
  stack's height) and fades out from 56px above the box.
- **Turn fold:** while a turn runs, every step shows as it streams
  (thoughts, tool calls with status and output, subagents). When it ends,
  its work folds into one summary row where the work began, above the
  final answer: `Worked for 1m 20s · 14 steps · 3 files changed · 1 failed`
  (`text-callout text-fg-faint`, hover `text-fg-muted`, a chevron that
  turns 90° when open, a `CircleX` in `danger` when a step failed). What
  stays visible: the message sent, the final answer (the replies after the
  last step), warnings and errors, and the footer (time, copy, Changes),
  which then leaves out the duration. Opening it shows the steps in order,
  each fading in with `rc-expand` (opacity and 4px, so nothing lays out
  under a transition; Reduce motion makes it instant). A `<button
  aria-expanded>`, so ⏎ and Space toggle it. Opened turns are kept per
  thread while the app runs; an opened old thread starts with every turn
  folded; find in thread opens them all while it searches. Folded steps are
  not rows at all, so the virtualizer and the timeline rail have fewer.
  Logic: `state/fold.ts` (`foldTurns`).
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
