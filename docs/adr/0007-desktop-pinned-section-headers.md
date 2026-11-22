# 0007: Opened chat sections pin their header by script, not CSS `sticky`

Status: accepted, 2026-11-22. Code: `apps/desktop/src/ui/StuckFold.tsx`,
`.rc-sticky-head` and `.rc-stuck` in `apps/desktop/src/index.css`.

- An opened section's header stays under the thread header while what it
  opened is on screen, so it can be closed from anywhere in it. Clicking a
  pinned turn or group header closes it and scrolls back to the real header.
- A turn's `Worked for …` fold and a tool group open into rows of their own.
  The transcript is virtual, so each row is its own box and CSS `sticky` has
  no shared parent to stick in. A copy of each header floats over the
  scroller instead, placed from the virtualizer's measurements; a group's
  copy stacks under its turn's, and the row after the section pushes it out.
- A tool call or a thought opens inside its own row. CSS `sticky` would fit,
  but the rendered rows sit under one translated box (`Transcript.tsx`), and
  Chromium places a sticky box from its untranslated layout, so it pinned at
  the row's foot. The scroll handler moves the header down its row with a
  `translateY` instead, stopping at the row's end, and marks it `data-stuck`.
- Pinned, a header reads as a raised bar: the sidebar fill, a hairline ring
  and the composer's slight shadow. An in-row header draws that bar in a
  `::before` behind itself, so pinning changes no row's height.
