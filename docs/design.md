# Desktop design system

The rules every change to the desktop app (`apps/desktop`) follows. The
tokens live in one place, `apps/desktop/src/index.css` (CSS variables and
the Tailwind `@theme` that maps them to classes), with the built-in themes
mirrored in `apps/desktop/src/lib/themes.ts`. Theme files (user and project
themes, the CLI and the engine's `_rustcode/themes` methods) are described
in [themes.md](themes.md).

`src/design.test.ts` fails the build on a raw colour (`#hex`, `rgb()`,
`hsl()`) or a Tailwind palette class (`zinc-*`, `white`, `black`, …)
outside the tokens files, on the retired class names listed at the end, and
on a native `<select>`, `transition-colors`, `backdrop-blur` or a hand-made
menu item.

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
| `message` | `text` at 8.5% over `bg` | A sent message's bubble: a step off the pane, so it is told apart from hover fills |
| `--rc-diff-add` / `--rc-diff-del` | `success` / `danger` at 14% | Diff line backgrounds |
| `--rc-shadow-1` / `--rc-shadow-2` | see Elevation | |

Tailwind classes use the token names, except the text family, which is
`fg`: `bg-bg`, `bg-sidebar`, `bg-surface`, `bg-surface-2`, `bg-surface-3`,
`border-border`, `border-border-strong`, `text-fg`, `text-fg-muted`,
`text-fg-faint`, `bg-accent`, `text-accent`, `text-accent-fg`,
`text-danger`, `bg-code-bg`, `bg-fill`, `bg-fill-subtle`,
`bg-fill-strong`, `bg-message`. Opacity modifiers work on any of them
(`bg-danger/10`).

Primary actions are **inverted neutral** (`bg-fg text-bg`), not accent:
the accent marks state (on, selected, linked), it does not shout.

## Radius

| Step | px | Tailwind | Used by |
|---|---|---|---|
| xs | 6 | `rounded-xs` | kbd, inline code (on `bg-fill`), tiny badges, checkboxes |
| sm | 8 | `rounded-sm` | 22px icon buttons inside rows, branch chips, small inputs, a segment inside its track |
| md | 10 | `rounded-md` | Icon buttons, list rows, inputs |
| lg | 12 | `rounded-lg` | Cards (tool output, error card), settings rows and groups, menu items, tooltips, pictures in replies |
| xl | 16 | `rounded-xl` | Settings group cards, menus, popovers, dialogs, the command palette, large panels |
| 2xl | 20 | `rounded-2xl` | The settings panel, toasts |
| composer | 24 | `--rc-composer-radius` | The composer's box and tray, one step above 2xl so the floating box reads rounder than the cards |
| 3xl | 26 | `rounded-3xl` | The largest floating surface, above the composer's step (nothing uses it yet) |
| full | — | `rounded-full` | Switches, avatars, dots, trackbars; never a labelled button |

A theme's `radius` (`sharp`, `default`, `round`) scales the steps
(`sharp`: 2/3/4/6/8/10/12, `round`: 8/10/12/16/20/26/32); components only ever
name a step. No arbitrary radii (`rounded-[14px]`), no `rounded-4xl`. A child
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
  shows focus by its glass stepping from 80% to 88% (see Component recipes).
- `outline-none` is allowed only on a text field whose container already
  shows focus (the composer, a search capsule).

## Elevation

Two levels, both including the 1px edge ring and both light: half the
theme's `--rc-shadow-color` (`--rc-shadow-soft`, so `#00000040` in the dark
theme and `#0000001a` in the light one), a small blur, close to the edge.
A raised surface is told apart by its edge and glass, not by a dark halo.

- `--rc-shadow-1` (`shadow-1`, `0 1px 1px` + ring): things resting above the
  chat: the jump-to-latest button, error cards over the transcript. (The composer and toasts share a slight shadow under them,
  `0 2px 8px -4px`.)
- `--rc-shadow-2` (`shadow-2`, `0 2px 8px` + ring): things that open over
  everything: popovers, menus, tooltips, dialogs, the command palette.

Do not hand-tune a shadow on one component: the tokens are the whole scale.

The `elevated` utility is glass (`--rc-glass-bg-strong`) + `shadow-2` +
`rounded-xl`: use it for every popover, menu and dialog. Tooltips have their
own `rounded-lg` recipe below.

## Glass

One translucency, one recipe, for every floating surface: popovers, menus,
tooltips, dialogs, toasts and the lightbox.
`--rc-glass-bg` (`surface-2` at 84%; `--rc-glass-bg-strong` at 93% for
popovers, menus and floating pills) with `--rc-glass-filter`
(`saturate(1.2) blur(13px)`). Use `.rc-glass` or the `elevated` utility,
never a hand-made `backdrop-filter`. Reduce transparency (the media query,
or the system setting the shell reads) turns the pair solid. Headers use
the same filter tinted with the pane instead of a surface
(`--rc-glass-pane`, `bg` at 86%; 60% with `.rc-frost` over a new thread's
banner), through `.rc-header-glass`. Panes, cards, rows and the sidebar
stay opaque.

What floats just above the composer is that same dense glass as one class,
`.rc-glass-overlay` (strong glass + `shadow-1`'s edge ring): the strip of
threads working for this one, its group card — a row on the strip's
surface, not a second glass — and the jump-to-latest pill, so the three
read as one surface. Only their radius differs. Latest is both the
surface and the control, so its hover is a fill layer over the glass
rather than a `hover:bg-…` utility a class rule would beat.

The composer floats over the transcript in two parts: the main box (field and send), and a tray
tucked behind its foot with every other control. Its slash and `@` menus are an opaque drawer
attached to the box's top edge.

The composer is its own, lighter pair, because it is the one surface text
scrolls under all day: `surface-2` at **75%** (84% focused), its own
`--rc-composer-filter` (`saturate(1.14) blur(16px)`; `saturate(1.08)
blur(20px)` in dark), and a 1px `--rc-composer-outline` (`black` at 8% in
light, `white` at 5% in dark) with a 3% `white` inner highlight in dark.
It is the one surface with a radius of its own, one step above the
largest card's.

Near-solid and lightly blurred on purpose: text scrolling under a panel
should be a hint, not a smear. One filter per surface, two recipes in all
(the shared glass and the composer's): no larger radius, no second blur
stacked on the first, no blur on a pane or a scrolling row.

## Spacing

Tailwind's 4px grid. Common steps: 4 (icon to label), 8 (between controls
in a row), 12 (card padding block), 14 (card padding inline), 16 (between
groups), 24 (between sections). The chat lane is **832px**
(`--rc-chat-content-max-width`, 52rem) for the transcript's rows and the
composer alike, inset by a 12px gutter, 20px from the `sm` breakpoint.
Settings › Chat › Chat width moves that one cap and nothing else: Wide is
72rem, Full is the pane's own width. While a right-hand card is open the
lane centres in the room the card leaves (`pane - card - its two 12px
gaps`), narrowed to that room when it is below the cap, so the conversation
reads centred beside the card instead of off-centre under it.

## Type

macOS text styles one point up (read at a laptop's distance), SF Pro
(Settings › Appearance › Rounded font switches to SF Pro Rounded) / SF Mono.
Replies read at 14px on a 1.625 line height (22.75px, `leading-relaxed`,
set on the reply's own scope so the app's rows keep 19px), in `fg` at 90%,
with headings at 20/18/16/14 in semibold:

| Class | Size / line | Use |
|---|---|---|
| `text-caption` | 11/14 | Metadata, badges |
| `text-sub` | 12/15 | Sidebar secondary lines, section headers |
| `text-callout` | 13/17 | Controls, chips, secondary copy |
| `text-body` | 14/19 | Body, rows, tool rows |
| `text-title3` | 16/21 | Card titles |
| `text-title2` | 18/23 | Onboarding, a document's headings |
| `text-title` | 23/29 | Empty-state heading |

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
  of work a frame for the right panel, so well under 120 fps). A card takes
  no slot at all — it floats over the chat — so opening one refits the lane
  once and the card moves with `transform` and `opacity` (`rc-drill` in,
  `translate`/`opacity` out); only its left edge, dragged by hand, widens it
  frame by frame. Animate only
  `transform`, `opacity` and colour; never `filter` or `box-shadow` on
  something that moves every frame.
- Toasts drop in with `rc-toast-in` (220ms, from 10px above) and lift away
  in 140ms `ease-in`; a drag follows the pointer and snaps back or flies off
  in 160ms, and a card shown meanwhile takes the top of the stack.
- A new transcript row rises in with `rc-row-in` (260ms, fade + 6px), once:
  rows there when the thread opened, or remounted by the virtualizer, do not.
  A status dot that appears pops in (`rc-dot-in`); a segmented control's
  selection slides (`transform`) to the new segment.
- A streaming reply's new words fade in once each (`rc-reveal`, 280ms, from
  `opacity: 0` and a 4px blur, 18ms apart, no word waiting past 180ms).
  `lib/reveal.ts` wraps only the words that arrived in the chunk, and only in
  the last block, so the text already on screen is plain text: nothing fades
  twice, and a finished reply has no spans and no animation. The blur is the
  one `filter` that animates: 280ms on a handful of words, never on a box that
  moves. Settings › Appearance turns it off; reduced motion does too.
- Skeletons sweep once every 1.6s (`transform` only, on the compositor).
- `prefers-reduced-motion`: all durations collapse to ~0; shimmer and the
  skeleton sweep stop.

## Tooltips

One `Tooltip` (`ui/primitives.tsx`) under one `Tooltip.Provider`: 400ms
for the first, then instant while the pointer moves between triggers
(`skipDelayDuration` 300ms), closing at once. The tip takes no pointer
events, opens on top by default, 8px from its trigger and 10px from the
window's edges, and flips when there is no room. Controls in the composer
open theirs below, clear of the box. Inside a popover, hints are
secondary text, not tooltips.

## Cursor

A pointer on everything clickable (buttons, rows, chips, switches, radios,
tabs, menu items, links), set on the control so its children inherit it;
the default arrow on static text; the text cursor only in fields and
`.selectable` text. Disabled controls show the arrow.

## Icons

Lucide, stroke 1.75 (2.5 for chevrons at 10–12px). Sizes: 12 inside chips,
13 in menus, 14 in toolbar and composer buttons, 16 in page headers and tool
rows, 18 in empty states. Icon colour follows text (`text-fg-muted` idle,
`text-fg` on hover).

## Scrollbars

Overlay where the system allows (macOS default), thin elsewhere:
`scrollbar-width: thin` in `scrollbar`, transparent until the scroller is
hovered or scrolling, and `scrollbar-gutter: stable` on long scrollers so
content never shifts when a classic scrollbar appears.

## Component recipes

Shape rule: **labelled buttons are pills** (`rounded-full` with
horizontal padding, never a circle): the composer's footer buttons, the
model pill, chips, segmented items, header actions, Settings' "New…" and
"Edit config".
Circles only for icon-only round buttons (send, stop, close in a panel
header, toolbar clusters). Icon buttons in dense rows are `rounded-md`.
Rows are `rounded-md` (sidebar thread rows `rounded-lg`), menu items and cards `rounded-lg`, menus, popovers and
panels `rounded-xl`.

The shared controls are in `ui/primitives.tsx`; use them instead of restyling
a raw element. `?catalog` in a dev build shows them all on one page.

- **Button:** `Pill`, 24, 26 (default) or 30px high. Secondary by default
  (`bg-fill text-fg`, hover `bg-fill-strong`); `prominent` (`bg-fg text-bg`,
  hover 90%) for the one primary action; `danger` (`bg-danger/12 text-danger`);
  `quiet` (transparent, `text-fg-muted`, hover `bg-fill text-fg`). **Icon:**
  `ChromeButton`, or `ClusterButton` inside a `Cluster`. Disabled: 40% opacity,
  no hover.
- **Select:** `Select` for one choice from a list, native `<select>` never: a
  pill trigger with a chevron, opening a menu with a check on the current
  option. `Segments` when two to four short options should all show.
- **Input:** `h-8 rounded-md bg-fill-subtle px-3 text-callout`,
  placeholder `text-fg-faint`, no ring until `:focus-visible`.
- **Chip:** `h-6 rounded-full bg-fill px-2 text-callout text-fg-muted`;
  status chips tint with their colour at 12–14%.
- **Row (list):** `rounded-md px-2.5 py-1`, hover `bg-fill-subtle`,
  selected `bg-fill` + `font-medium`; the hover action button appears with
  opacity only (no layout change).
- **Card:** `rounded-lg bg-surface p-3.5`, no border; rows inside separated
  by `divide-border`.
- **Menu:** `menuSurface` (`elevated p-1.5`) with `menuItem` rows
  (`rounded-lg px-3 py-[7px]`), highlighted `bg-fill` (not accent);
  destructive rows add `menuItemDanger`; `menuSeparator` and `menuLabel`
  between them. **Popover:** `popoverSurface` (`elevated p-3`).
- **Dialog:** `elevated rounded-xl p-5` over a `bg-bg/60` scrim.
- **Toast:** `.rc-toast rounded-2xl` (the composer's glass, denser, and its
  slight shadow), 380px wide, `py-1.5 pl-3.5 pr-1.5 text-callout`: an icon
  (info `text-fg-muted`, success `text-success`, error `text-danger`, a
  spinner while it has progress), the text in `text-fg` (up to 3 lines), at
  most one action as a secondary pill (`h-6 bg-fill`), and a close button.
  Progress is a 2px accent bar along its foot. `Toaster` shows at most three
  (`state/toasts.ts` enforces the queue), newest on top, stacked with 8px
  between, centred at the top of the chat column 8px under its header (the
  window's top over Settings). The same message again refreshes its card and
  restarts its time instead of stacking; past three, the oldest is dropped.
  Info and success leave after 3s, errors after 7s. The pointer or focus on
  the stack, a drag, and a window in the background stop every clock, which
  resumes with what was left. ×, Escape (after a draft or an open overlay has
  taken it), or a drag past 48px up or sideways dismisses; a drag follows the
  pointer and snaps back in 160ms, and never selects text. Use a
  toast for what is not tied to a message (copied, engine events, errors of an
  action), not to repeat what the screen already shows (no pin, archive,
  theme, harness-switch or steering notice); an error about a thread's turn
  stays an error card over the composer. No banners.
- **Sidebar sections:** one list, with a header only where a group needs one:
  Working (threads running now, on top; off with the filter menu or when
  grouping by project), then pinned and the rest, and at the foot the Snoozed
  and Settled shelves. A snoozed row shows when it wakes where the age goes.
  Snooze (presets, or Custom…) is a submenu of the row's menu; a thread comes
  back on time, marked unread, or sooner when it needs you, works or is messaged.
  A row's second line reads the worktree branch, then what the thread's calls
  changed (`font-mono +N −N` in `success`/`danger`), for a thread the app has
  already read; one it has not opened shows neither, rather than a made-up zero.
- **Delegated threads:** a thread an agent started has no row of its own. Its
  parent's second line ends with a stack of at most three 18px round marks
  (`ui/AgentStack.tsx`), overlapping by 6px with a 2px ring in the sidebar's
  surface, then `+N` past the third and one quiet accent spinner while anything
  works. What needs you comes first, with a small `warning` dot; finished
  delegates show nothing, no dot and no count. The slot is a fixed 74px, so a
  count going from one to three never moves the title's start, the age or the
  status. Hovering lists the running threads and their state, and the row opens
  the parent. An open child keeps its parent's row lit as the context. In the
  chat's group cards the same stack reads at 24px with a `surface` ring; once
  all are done it shows the finished marks instead, with no spinner. Logic:
  `state/agentStack.ts`.
- **Sidebar selection:** one tab stop for the whole list, on the row the
  keyboard is on (the open thread, or the parent row kept lit for an open
  delegated thread, else the first): ↑ ↓ move it through the rows on screen,
  skipping shut sections and project groups, ⏎ opens the thread, Esc lets the
  selection go. Keys in a field inside the list (the
  rename box) stay the field's, and ⌘/⌃/⌥ stay the app's. ⌘-click toggles a
  row into the selection and ⇧-click takes the range from the last one clicked;
  picked rows are `bg-accent/12` + `font-medium`, which is not the open row's
  `bg-fill`, and a plain click opens the thread and lets the selection go. A
  thread that leaves the list leaves the selection. With more than one picked,
  the row's menu and a bar under the list (`3 selected`, Pin, Settle, Clear)
  act on all of them, with one toast and one failure message for the lot.
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
  that has a header uses this one (the chat, new or open); the Settings
  panel brings its own.
- **Settings:** a floating panel over the app, not a page of it: a centred
  `max-w-[80rem] max-h-[60rem]` panel inside `p-1 sm:p-6 lg:p-10` (about a
  hundred px in from the sides at 1512×945, a small inset in a narrow
  window), `rounded-3xl` over our raised shadow and a hairline edge, on a
  `bg-bg/60` scrim; the shell stays mounted and inert behind it. A press on
  the scrim, Esc, ⌘, or the header's close button puts you back where you
  were. The panel takes focus when it opens, never its first control. A
  240px nav column in `bg-sidebar` with a hairline on its end edge holds a
  32px search field, then the pages under short `text-callout` headings
  (`Setup`, `App`, `System`), or the results while there is a query. Nav rows
  are `h-8 rounded-md px-2` with a `size-4` icon, `text-body`; the open one is
  `bg-fill` in `fg`, the others muted and filled on hover. The content has its
  own 48px header — the breadcrumb `Settings / General` (the first part
  muted) at its start, "Restore defaults" and the close button at its end —
  with a hairline under it, and the page scrolls beneath. One page shows at a
  time in a centred `max-w-[760px]` column, `pt-5`, its rows keeping the
  `px-4` inset the section titles share. "Restore defaults" shows only while
  a row on that page differs from its default, and puts back every changed
  row at once with one toast. Picking a result opens its page and lights the
  row it found (`rc-setting-hit`, a `fill` fading out over 1.2s).
- **Settings section and row:** a section is its name in `text-base
  font-semibold text-fg`, `px-3`/`px-4`, `space-y-2.5` above one group card,
  and `32px` of gap before the next section. The card is `rounded-xl` with a
  1px `border-strong` edge over a soft step of the page
  (`bg-surface-2/40`), rows divided by a `border` hairline. A row is
  `px-4 py-3.5`: its title `text-sm font-medium`, its description
  `text-callout leading-normal text-fg-muted`, its control at the right, and
  the control stacks under the text when the row's own container is narrow.
  Controls in a row — buttons, selects, the segmented control — are
  `h-8 rounded-lg` with a thin `border-strong` edge; switches keep their own
  shape.
- **Settings row, reset:** rows show their description in full and keep the
  reset button of the bullet below where the value differs from its default.
- **Tooltip:** `bg-surface-3 rounded-lg px-2.5 py-1 text-callout shadow-2`
  (see Tooltips).
- **Composer:** one card, `.rc-composer` (radius `--rc-composer-radius`,
  24px, one step above 2xl), with no padding of its own: the field is
  padded `px-3` and the foot `px-3 pb-2`. Glass at
  `--rc-composer-bg` (75%, 84% focused) with `--rc-composer-filter` and a
  1px `--rc-composer-outline`; no ring or colour on focus, the glass step
  is the focus. The card clips to its own corner. The editor reads at
  `text-body leading-relaxed` (22.75px), one line at rest above the foot
  and 217px (nine lines) at most, then it scrolls; its text carries 6px
  above and below inside the scroller, so the padding scrolls with it and
  the first and last lines keep their room (the box is sized to whole
  lines plus that padding). The edge it is cut at fades over 20px
  (`mask-image`, `.rc-field-fade`, whose two edges `lib/fade.ts` decides):
  the top once it is scrolled down, the bottom while there is text below,
  and neither when the draft fits, so a short message never dims. A
  slash-command chip in front of it
  is that same line box, so it keeps the field's baseline. The foot is one
  row **inside** the card: attach (**+**) on the left, send/queue/stop on
  the right. The tray tucked behind the foot carries the rest: the folder,
  the worktree or branch and the permissions mode on the left; the plan
  meter, the context ring, the model pill and the working agents on the
  right. What rests on the card's own surface (the **+**, the model pill) is
  `bg-fill-subtle`, `bg-fill` on hover, `bg-fill-strong` when open. The
  card is a container (`@container`), so a narrow window drops labels (the
  folder name shortens, then the permission label, the branch chip and
  the agents pill go) rather than dropping controls or overflowing. The
  transcript scrolls under the card (its bottom padding is the stack's
  height) and fades out from 56px above it. A draft is kept in app storage
  under `rustcode.drafts` (400ms after typing stops, and at once when the
  window closes), so an unsent message is still there after a restart:
  sending clears the entry, a draft over 100 KB is not kept, and only the
  200 newest survive (`state/drafts.ts`).
- **Queued message:** what waits for the thread, at the end of its transcript
  (`ui/QueuedMessages.tsx`), as a sent bubble's twin: right-aligned, at most
  80% wide, `rounded-2xl`, a dashed `border-strong` edge and `bg-message/40`
  instead of the fill, the command and links as chips over the text, pictures
  and files above it. The dashed edge is what says it waits, so under it the
  `Clock` and `Queued` and its 22px icon buttons — Send now, edit, move up and
  down (with two or more), remove — are one `.rc-meta-reveal`, hidden at rest
  on the row's reserved 22px and in together on hover or keyboard focus, the
  way a sent message's time and actions are (and shown on a coarse pointer,
  which cannot hover). `Held` stays at rest when a stopped or failed turn left
  the queue waiting, since that is why nothing runs; so does `Editing in the
  composer` with Cancel. Send now follows one rule, shared with ⌘⏎ and
  `state/queue.ts` (`steerability`): plain text steers; pictures steer on a
  harness that takes images mid-turn and is not the built-in one; a command, a
  link chip or a file waits for the next turn, and the dimmed arrow's tooltip
  is that reason (`Files go with the next turn`, `Commands go with the next
  turn`, `Links go with the next turn`, or the harness cannot take a message
  mid-turn). ⌘⏎ reads the message rather than the thread's learned `steer`
  flag for text and pictures, so a thread started before its harness learned it
  can steer is still tried against the engine; the row stays dimmed on such a
  thread. It is edited in the composer,
  never in a field of its own: its pencil, or ↑ at the start of an empty
  composer, loads its text, command, links and attachments there, ⏎ saves it
  back in the queue and Esc leaves it. A held queue ends with Resume, which
  starts its first message. A wake turn the app sends is one quiet line (its
  first line, in the notice divider's style), never a bubble; a message another
  thread sent says `From “…”` and is not the user's to edit.
- **Jump to latest:** the pill over the composer's foot (`shadow-1`,
  `.rc-glass-overlay` — the strip below it shares that surface — 30px)
  says `Latest` and, while a turn runs, what
  that turn is doing now after a `·` (`Reading files`, `Thinking · …`), so
  a reader scrolled up knows what waits. One line, truncated.
- **Tool group:** two or more finished tool calls in a row (with the
  thoughts between them) are one row, closed by a hairline
  (`border-b border-border/60`, `pb-2`) so the answer below it is told
  apart from the work: the icon of the kind most of them are in the 18px
  column, then a sentence of the two most frequent actions,
  `Read 4 files and ran 2 commands`, the rest as `performed N other
  actions`, in `text-body text-fg-muted`, the failed count in `danger`,
  and the fold chevron. It sits 2px from neighbouring tool rows, like they
  do. A tool row itself is quiet: 24px high (`min-h-6`), a 16px icon in
  the 18px column, its title in `text-body text-fg-muted`, the summary in
  `text-body text-fg-faint`, the chevron only on hover.
- **Subagent row:** a subagent (Claude Code's `Agent`/`Task` call,
  RustCode's `spawn_agent`) is one row in the tool rows' grammar: the `Orb`
  (a 3×3 dot ring whose light travels clockwise) while it works, else a
  `CircleCheck`/`CircleX`, in the 18px column; its type in `font-medium`,
  the task in `text-fg-muted`, and while it works its current step in
  `text-accent`, done `3 steps · 42s`. Its own steps never become
  transcript rows; the chevron opens a bordered card
  (`rounded-lg border bg-fill-subtle/30`) of one-line steps and its
  answer. Two or more subagents in a row are one row:
  `3 subagents`, then `2 working · 42s` in accent (or `1 failed`,
  `done`), opening to a card with each agent's row. Over the composer,
  `● 2 agents working` (accent pill with the `Orb`) opens on hover a card
  of each working agent and its step.
- **Turn fold:** while a turn runs, `Working for 12s` heads it under the
  message that started it (`text-body text-fg-muted` over a
  `border-border/60` hairline), its replies and the call it makes now
  stream below, finished calls fold into tool groups, and the foot shows
  what it does now in `rc-shimmer` when no running row does. When it ends,
  its work folds under one divider where the work began, above the final
  answer: `Worked for 1m 3s ›` or `You stopped after 12s`, in the same
  type over the same hairline, which a divider right above it already draws
  (so it drops its own). The duration lives here and nowhere else; a red
  `CircleX` marks only a turn that itself failed, and a step that failed
  keeps its own count inside. A turn that wrote no answer keeps its time in
  the header (`Worked for 4s · 09:55 PM`) and has no footer, since there is
  nothing under it to copy or fork. A turn with no steps folds nothing and
  so has no header, and its duration goes with it: a short direct answer
  needs no `Worked for 2s`, and its footer says only the time.
  What stays visible: the message sent (a `rounded-2xl bg-message` bubble at
  most 80% wide, `p-3`; consecutive messages soften the corners they share,
  `rounded-tr-md` / `rounded-br-md`; its time and actions on hover, 12px,
  clipped to the bubble), the final answer (the replies after the last
  step), warnings and errors, and the footer (time, copy, Changes), which
  says only the time: no duration, so a folded turn never states it twice.
  Copy and fork wait for a hover or keyboard focus on the footer, and stay in
  view on the newest finished turn. Opening it shows the steps in order,
  each fading in with `rc-expand` (opacity and 4px, so nothing lays out
  under a transition; Reduce motion makes it instant). A `<button
  aria-expanded>`, so ⏎ and Space toggle it. Opened turns are kept per
  thread while the app runs; an opened old thread starts with every turn
  folded; find in thread opens them all while it searches. Folded steps are
  not rows at all, so the virtualizer and the timeline rail have fewer.
  Logic: `state/fold.ts` (`foldTurns`).
- **Settings reset:** a row whose value differs from its default shows a small
  reset icon before its control, on hover or keyboard focus (`Row`'s `reset`).
  When the control stacks under the text, the icon leaves the flow for the
  row's top right, so stacked controls share one left edge.
- **Right panel:** one `RightPanel` for every side view (Details, Changes,
  Pull requests): a floating card at the window's right edge, inset 12px from
  its top, right and foot, `rounded-2xl` in the same glass and elevation as
  every other floating surface (`.rc-glass` + `-strong` + `shadow-2`, the
  details card's own surface), so the transcript scrolls under it and the chat
  keeps its full width. The details card is that surface, small (280px) and at
  the top right. A pane's header row is a `HEADER_HEIGHT` band inside the card
  with a segmented switch between the views and a close button, divided from the
  view by a hairline; views add a slim toolbar (`h-9`) under it. It never draws
  past the column's edges (320–760px, kept as asked, fitted when drawn), and its
  foot stops above the composer (its top measured into `--rc-composer-top` by
  `ui/ChatView.tsx`), so the composer stays usable; a new thread's centred hero
  and composer leave it the pane's full height. The left edge drags and
  answers the keyboard too: ←/→ 24px from the
  width on screen (Shift 96px), Home/End to the ends, ⏎ the default. Escape
  inside either surface closes the panel (unless a field in it is being edited),
  focus goes into it when a shortcut opens it with nothing focused, and closing
  hands focus back. The lane centres in the room the card leaves, narrowed to it
  when that room is under its cap, and a column too narrow for a lane that
  reads has the card float over the chat instead. Every control under the
  transcript — composer, its slash drawer, approvals, linked threads, a report —
  paints above the card, so none of it is ever out of reach; a new thread's hero
  is not a control, and on a narrow column the card floats over it. Each stack
  child rides the lane's shift on its own
  (`LaneItem`, `empty:hidden` so one that renders nothing leaves no gap), not the
  stack, so the hero and those
  controls are separate stacking contexts, and the whole stack sits on a scrim
  as tall as itself, so a queued message or the strip of linked threads never
  reads through the composer's glass. The card slides and fades in 200ms (see
  Motion), its width published for the lane (`usePanelCardWidth`). ⌘⌥B toggles it
  with the last
  view. A thread opens with the panel on the view it last showed (Changes
  when that view has nothing to show here; `state/store.ts`
  `panelForThread`); closing it by hand is kept for the thread on screen, so
  picking that thread again leaves it closed, and viewing another thread opens
  it again. Opening the thread its Thread tab shows ends the peek and returns
  the panel to Changes, and a new thread ends it too. The new-thread screen
  opens without a panel, though ⌘⌥B still opens one there. Settling the selected
  thread while the chat is showing (its menu, the palette, the sidebar) closes
  the panel and leaves it for the new-thread screen in the same folder, with
  the same Undo the sidebar offers; settling from the board, review or compare
  stays on that view, and a status set by an agent or the engine never moves
  the screen.
  Details lists what a thread is, in titled sections that drop out when
  they have nothing to say: harness, workspace, linked threads, subagents with
  status and elapsed time, plan and usage.
- **Switch row:** where a thread's model or effort changed after its first
  message, a quiet divider (`gpt-6-luna → gpt-6-astra · Medium → High`). Picks
  with no message between them are one row; the engine records it in the
  history on harness threads, so it survives a reload.
- **Approval row:** an approval waits as a row in the transcript, at the call
  it is about (`ui/AttentionCards.tsx` `ApprovalRow`): the `Hand` in
  `warning` in the 18px column, `Waiting for approval` in `font-medium`, then
  the command or title in `font-mono`, with `1 of N` when the turn waits on
  a batch. Under it, indented 26px, one body at most 160px tall that scrolls:
  the command in a `bg-fill-subtle` code well (`$ ` in `text-fg-muted`), the
  change it would make (`DiffView`), and its reason in `warning` when it has
  one. Then its answers as 24px pills: Decline `esc`, Approve `⏎`,
  Always allow, and More for the rest. It sits 2px from the call above it,
  like the tool rows do. An approval whose call is not on screen yet (or is
  folded away) goes to the foot of the transcript, where it can still be
  answered.
- **Approval bar:** the card over the composer keeps one slim line for the
  same approval — `Waiting for approval · <title>` with the same answers
  (`elevated rounded-2xl`, `min-h-[34px]`, `py-1`) — because it is the target
  for the keys: 1–9 pick an answer in the engine's order (reject, always,
  allow once), `⏎` approves and `esc` declines, whenever no field, menu or
  dialog has the keyboard. The body is not repeated there; it is read in the
  row. A message sent from the composer meanwhile declines it and tells the
  agent what to do instead.
- **Question row:** an answered question stays in the transcript
  (`ui/Rows.tsx` `QuestionRow`, `bg-fill-subtle rounded-lg px-3 py-2`): the
  header in `font-medium` after a `MessageCircleQuestion` in accent, the
  question itself in `text-fg-muted` (three lines at most), and the answer
  under a `CornerDownRight` in `success`. It is not folded away with its
  turn. A question still waiting keeps its form over the composer, since its
  answer is a choice; only the answer leaves a row.
- **Plan card:** a paused thread's plan, at the foot of the transcript
  (`ui/PlanCard.tsx`): `rounded-2xl bg-surface p-4`, a `ListChecks` and
  `Plan` in `font-semibold` with `3 of 8 left` in `text-fg-muted tabular`, a
  Copy button and Implement (a prominent 24px pill that sends `Implement the
  plan.` as an ordinary message). Its steps are one line each, `text-body`
  with a 6px status dot (`success` done, `accent` now, `fill-strong` next)
  and done steps in `text-fg-muted`. Past 220px it clips and fades over its
  last 24px, with `Expand · N steps` under it; expanded, it shows whole and
  offers `Collapse`. It is drawn only while the thread is not running and
  something is unfinished, so a finished plan leaves no card; the header's
  popover and the composer's meter are unchanged.
- **Jump to latest:** `.rc-glass-overlay rounded-full`, centred 8px
  above the composer stack and above its drawer when one is open, over it
  in z.
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
