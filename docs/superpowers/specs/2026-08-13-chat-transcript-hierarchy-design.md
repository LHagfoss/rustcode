# Chat Transcript Hierarchy Design

## Goal

Improve rustcode's terminal-chat readability without changing its native-scrollback architecture or keyboard interaction model.

## Scope

- Render user turns with an accented `❯ ` gutter and regular-weight body text.
- Render assistant prose with a `• ` first-line gutter and `  ` continuation gutter.
- Preserve Markdown parsing and soft paragraph reflow; do not pre-wrap normal assistant prose before passing it to the Markdown renderer.
- Render the existing activity status line as the live model/tool status, replacing the plain `Working...` row.

## Approach

Assistant rendering will split only fenced-code boundaries before delegating prose blocks unchanged to `render_markdown`. A small line-prefix helper will add the assistant gutter after Markdown reflow while preserving each span's styles. The committed user renderer will retain its current accent marker but use the normal text style for its body. The live tail will call the existing `activity_status_line` when work is active.

## Constraints

- Finalized rows remain terminal scrollback through `Terminal::insert_before`.
- The inline viewport remains limited to the mutable tail, queue, composer, and active status.
- Do not alter tool cards, queue behavior, picker behavior, theme behavior, or resize reflow in this PR.
- Keep code-block rendering and its existing styling intact.

## Tests

Rendering tests will assert assistant gutters and soft-line reflow, user-body style, and that the live status is the formatted Working line rather than the old bare label. The usual `cargo check --tests` and full `cargo test` gates remain required.
