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
| xs | 4 | `rounded-xs` | kbd, inline code (on `bg-fill`), tiny badges, checkboxes |
| sm | 6 | `rounded-sm` | 22px icon buttons inside rows, branch chips, small inputs |
| md | 8 | `rounded-md` | Icon buttons, sidebar rows, list rows, inputs |
| lg | 12 | `rounded-lg` | Cards (tool output, error card, settings groups), menu items, tooltips, pictures in replies |
| xl | 16 | `rounded-xl` | Menus, popovers, dialogs, the command palette, large panels, composer picture thumbnails |
| 2xl | 22 | `rounded-2xl` | The composer and its tray, toasts |
| full | — | `rounded-full` | Pills, toolbar clusters, switches, segmented controls, avatars, dots |

A theme's `radius` (`sharp`, `default`, `round`) scales the steps
(`sharp`: 2/3/4/6/8/10, `round`: 6/8/10/16/20/26); components only ever name a
step. No arbitrary radii (`rounded-[14px]`), no `rounded-3xl`+. A child
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

Two levels, both including the 1px edge ring and both light: half the
theme's `--rc-shadow-color` (`--rc-shadow-soft`, so `#00000040` in the dark
theme and `#0000001a` in the light one), a small blur, close to the edge.
A raised surface is told apart by its edge and glass, not by a dark halo.

- `--rc-shadow-1` (`shadow-1`, `0 1px 1px` + ring): things resting above the
  chat: the jump-to-latest button, queued messages, error cards over the
  transcript. (The composer and toasts share a slight shadow under them,
  `0 2px 8px -4px`.)
- `--rc-shadow-2` (`shadow-2`, `0 2px 8px` + ring): things that open over
  everything: popovers, menus, tooltips, dialogs, the command palette.

Do not hand-tune a shadow on one component: the tokens are the whole scale.

The `elevated` utility is glass (`--rc-glass-bg-strong`) + `shadow-2` +
`rounded-xl`: use it for every popover, menu and dialog. Tooltips have their
own `rounded-lg` recipe below.

## Glass

One translucency, one token pair, for every floating surface: the
composer, popovers, menus, tooltips, dialogs, toasts, the right panel and
the lightbox. `--rc-glass-bg` (`surface-2` at 84%; `--rc-glass-bg-strong`
at 93% for popovers, menus and the focused composer) with
`--rc-glass-filter` (`saturate(1.2) blur(13px)`). Use `.rc-glass` or the
`elevated` utility, never a hand-made `backdrop-filter`. Reduce
transparency (the media query, or the system setting the shell reads)
turns the pair solid. Headers use the same filter tinted with the pane
instead of a surface (`--rc-glass-pane`, `bg` at 86%; 60% with `.rc-frost`
over a new thread's banner), through `.rc-header-glass`. Panes, cards,
rows and the sidebar stay opaque.

Near-solid and lightly blurred on purpose: text scrolling under a panel
should be a hint, not a smear. Keep it to the one filter on floating
surfaces and headers: no larger radius, no second blur stacked on the
first, no blur on a pane or a scrolling row.

## Spacing

Tailwind's 4px grid. Common steps: 4 (icon to label), 8 (between controls
in a row), 12 (card padding block), 14 (card padding inline), 16 (side
gutter, between groups), 24 (between sections). The chat column is at most
980px wide with 28/20/16px gutters by width.

## Type

macOS text styles one point up (read at a laptop's distance), SF Pro Rounded
(`ui-rounded`; Settings › Appearance › Rounded font switches to SF Pro) /
SF Mono:

| Class | Size / line | Use |
|---|---|---|
| `text-caption` | 11/14 | Metadata, badges |
| `text-sub` | 12/15 | Sidebar secondary lines, section headers |
| `text-callout` | 13/17 | Controls, chips, secondary copy |
| `text-body` | 14/19 | Body, rows, transcript |
| `text-title3` | 16/21 | Card titles |
| `text-title2` | 18/23 | Page titles (Settings) |
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
  of work a frame for the right panel, so well under 120 fps). The slot
  takes its size at once and the panel moves with `transform` and
  `opacity` (`rc-drill` in, `translate`/`opacity` out). Animate only
  `transform`, `opacity` and colour; never `filter` or `box-shadow` on
  something that moves every frame.
- Toasts drop in with `rc-toast-in` (220ms, from 10px above) and lift away
  in 140ms `ease-in`; a swipe follows the pointer and snaps back in 160ms.
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
tray open theirs below, clear of the box. Inside a popover, hints are
secondary text, not tooltips.

## Cursor

A pointer on everything clickable (buttons, rows, chips, switches, radios,
tabs, menu items, links), set on the control so its children inherit it;
the default arrow on static text; the text cursor only in fields and
`.selectable` text. Disabled controls show the arrow.

## Icons

Lucide, stroke 1.75 (2.5 for chevrons at 10–12px). Sizes: 12 inside chips and
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
Rows are `rounded-md`, menu items and cards `rounded-lg`, menus, popovers and
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
- **Row (sidebar, list):** `rounded-md px-2.5 py-1`, hover `bg-fill-subtle`,
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
  most one action as a secondary pill (`h-6 bg-fill`), "+N" when more wait
  (it clears them all), and a close button. Progress is a 2px accent bar
  along its foot. One slot (`Toaster`), centred at the top of the chat
  column 8px under its header (the window's top over Settings): only the
  newest shows, four more wait, older ones are dropped. Info and success
  leave after 3s, errors after 7s; hover pauses; a swipe up or sideways,
  Escape or × dismisses. Use a
  toast for what is not tied to a message (copied, engine, worktree,
  harness and theme events, errors of an action); an error about a
  thread's turn stays an error card over the composer. No banners.
- **Sidebar sections:** one list, with a header only where a group needs one:
  Working (threads running now, on top; off with the filter menu or when
  grouping by project), then pinned and the rest, and at the foot the Snoozed
  and Settled shelves. A snoozed row shows when it wakes where the age goes.
  Snooze (presets, or Custom…) is a submenu of the row's menu; a thread comes
  back on time, marked unread, or sooner when it needs you, works or is messaged.
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
- **Settings:** a panel over the dimmed app, the shell mounted and inert
  behind it: `elevated rounded-2xl`, `fixed inset-8` and at most 1180px
  wide, over a `bg-bg/60` scrim. A 220px nav column in `bg-fill-subtle`
  holds the search field, then the sections (rows per the Row recipe), or
  the results while there is a query. Beside it the page scrolls on its
  own in a centred `max-w-[760px]` column, under the section's name and a
  `ChromeButton` close. Picking a result opens its section and lights the
  row it found (`rc-setting-hit`, a `fill` fading out over 1.2s). Rows
  show their description in full.
- **Tooltip:** `bg-surface-3 rounded-lg px-2.5 py-1 text-callout shadow-2`
  (see Tooltips).
- **Composer:** `.rc-composer rounded-2xl p-2.5` in the denser glass
  (`--rc-composer-bg`, the text colour at 7%; 10% on focus), a soft shadow
  above and below plus the edge ring; no ring or colour on focus. Its
  controls rest one step above it: the **+** and the model pill are
  `bg-fill-subtle`, `bg-fill` on hover, `bg-fill-strong` when open. Its
  tray (`.rc-composer-tray`) is tucked under the box as in the SwiftUI app:
  12px narrower each side, its top 18px hidden behind the box (which sits
  above it in z), only its rounded bottom showing; solid `surface-2`, no
  border. The tray is `min-h-[58px]` with `px-2 pb-1.5`, so its 28px
  buttons have the same 6px above them (to the box's edge) as at its
  sides and foot. The transcript scrolls under both (its bottom padding is the
  stack's height) and fades out from 56px above the box.
- **Tool group:** two or more finished tool calls in a row (with the
  thoughts between them) are one row: the icon of
  the kind most of them are in the 18px column, then a sentence of the two
  most frequent actions, `Read 4 files and ran 2 commands`, the rest as
  `performed N other actions`, in `text-callout text-fg-muted`, the failed
  count in `danger`, and the fold chevron. It sits 2px from neighbouring
  tool rows, like they do. A tool row itself is quiet: its title in
  `text-fg-muted`, the summary in `text-fg-faint`, the chevron only on
  hover.
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
  type over the same hairline, a `CircleX` in `danger` when a step failed.
  What stays visible: the message sent (a `rounded-2xl` bubble at most 80%
  wide; its time and actions on hover), the final answer (the replies after
  the last step), warnings and errors, and the footer (time, copy,
  Changes), which then leaves out the duration. The chat lane is 768px
  wide. Opening it shows the steps in order,
  each fading in with `rc-expand` (opacity and 4px, so nothing lays out
  under a transition; Reduce motion makes it instant). A `<button
  aria-expanded>`, so ⏎ and Space toggle it. Opened turns are kept per
  thread while the app runs; an opened old thread starts with every turn
  folded; find in thread opens them all while it searches. Folded steps are
  not rows at all, so the virtualizer and the timeline rail have fewer.
  Logic: `state/fold.ts` (`foldTurns`).
- **Right panel:** one `RightPanel` for every side view (Details, Changes,
  Pull requests): floating, inset 8px from the window's top, right and
  bottom, `rounded-xl bg-surface shadow-2`, no divider. Its header row has
  a segmented switch between the views and a close button; views add a
  slim toolbar (`h-9`) under it. The left edge drags (320–760px, kept);
  its slot takes its width at once and the panel slides and fades in 200ms
  (see Motion). ⌘⌥B toggles it with the last view.
  Details lists what a thread is, in titled sections that drop out when
  they have nothing to say: harness, workspace, linked threads, subagents with
  status and elapsed time, plan and usage.
- **Approval:** over the composer, `elevated rounded-2xl`: what it wants (`From
  <thread>` when a thread this one started is asking), the command in a code
  well, then Always allow, More, Decline `esc`, Approve `⏎` (the keys work when
  no field, menu or dialog has the keyboard). A message sent from the composer
  meanwhile declines it and tells the agent what to do instead.
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
