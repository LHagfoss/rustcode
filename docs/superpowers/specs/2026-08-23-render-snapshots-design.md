# Render Snapshots Design

## Goal

Reduce contention on the shared `AppState` mutex during terminal redraws without
changing visible UI behavior or allowing the renderer to observe mutable state
after the snapshot is released.

## Current problem

The runtime currently holds the `AppState` mutex while it computes the desired
inline height and while ratatui renders the frame. Rendering is mostly read-only,
but the lock remains held across history projection, markdown/tool-result
formatting, popup construction, and terminal drawing. Network and background
tasks that need to update the state wait for this whole interval.

The renderer currently writes only two application bookkeeping values:

- `conversation_content_height`
- `input_text_area`

The rest of its mutable inputs are terminal-local (`TranscriptState` and
thread-local render caches) or can be represented by an immutable frame view.

## Design

Add an immutable `RenderSnapshot` owned by the UI layer. The draw loop will:

1. Lock `AppState` briefly and capture the current render revision plus the
   values needed by the frame.
2. Release the lock before height calculation, widget layout, markdown/tool
   rendering, and terminal drawing.
3. Render exclusively from the snapshot and the existing terminal-local
   `TranscriptState`.
4. Lock `AppState` briefly after drawing and publish only bookkeeping values if
   the session/render revision is still current. A newer state revision must
   not be overwritten by an older frame.

The snapshot will share large stable data rather than deep-copying it on every
frame. `History` will use copy-on-write shared storage so cloning a read view is
O(1) until a writer mutates it. The active streaming response will have a
revisioned shared render representation refreshed by the existing response
mutation paths. Small transient UI values may remain ordinary owned clones.

UI rendering functions will accept the immutable snapshot (or immutable
references to it) instead of `&mut AppState`. The public compatibility wrapper
used by unit tests will continue to accept `&mut AppState`, construct a snapshot,
render it, and apply the two bookkeeping fields synchronously.

## Consistency and behavior

- A frame is internally consistent: all widgets read from one snapshot.
- Input, approval, question, picker, hover, and selection event handling remain
  on the live `AppState` mutex and keep their current behavior.
- A frame produced while state changes may be one redraw behind; the mutation's
  existing redraw request schedules the next frame.
- Stale frames cannot overwrite newer `conversation_content_height` or
  `input_text_area` values because publication is revision-checked.
- No application channels, cancellation tokens, network clients, or persistence
  handles are copied into the snapshot.
- Existing transcript caching and streaming cadence remain in place; this change
  only moves rendering work outside the shared state lock.

## Testing

Add focused tests that prove:

1. A snapshot preserves the current rendered output for representative idle,
   streaming, picker, approval, question, and selected-subagent states.
2. Shared history snapshots remain stable after the live history is mutated.
3. Stale bookkeeping publication is rejected after a newer render revision.
4. Existing UI and full-project tests continue to pass.

Verification remains `cargo check --tests` followed by `cargo test`.

## Scope boundary

This change does not split the application into multiple locks, move event
handling to background threads, redesign transcript markdown caching, or change
provider/network behavior. Those are separate performance projects.
