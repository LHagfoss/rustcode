# Queue Preview Design

## Goal

Keep queued follow-up prompts visible as a compact part of the composer while
the model is working.

## Design

The input area will reserve only the rows needed for a queue preview. A visible
preview consists of a muted header and at most three one-line prompt rows:

```text
queued (4) · ↑ edit last
  › second prompt
  › third prompt
  › fourth prompt
```

Rows appear in queue order, with the newest prompt closest to the composer.
When more than three user prompts are pending, the header keeps the full user
prompt count but the oldest prompts are omitted from the preview. Internal
`__task_wakeup__:` entries are excluded from both the count and the display.
Each row is truncated to the terminal width rather than consuming more composer
height.

## Non-goals

- No queue ordering, submission, or edit-key behavior changes.
- No new queue interaction beyond the existing Up-arrow edit-last action.
- No changes to native scrollback or the live transcript.
