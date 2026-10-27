# Compaction

Compaction replaces the oldest part of a session's history with a handoff
record so the agent can keep working on the same task in a bounded context
window. The code is in `rustcode/engine/src/network/context/compact.rs`.

## When it runs

- **Automatically**, before a request, once the history estimate reaches the
  proactive budget. That budget is the soft context target minus the tool
  reserve, provider overhead margin, completion reserve and the measured
  overhead of the previous request, so the summarizer call and the next
  request both still fit. The estimate starts from the provider's reported
  prompt tokens and adds only the messages appended since.
- **Manually** with `/compact`. The result reports tokens before and after and
  is shown in the context panel; the visible transcript is not rewritten.

## What it keeps

- A raw recent tail of up to 20,000 tokens (at most 30% of a small window),
  cut at a complete turn so no tool result loses its call.
- The original task, re-inserted verbatim as a pinned marker.
- Recent user requests, verbatim, up to a fifth of the budget.
- A typed boundary on the summary message, and the exact compacted prefix as a
  JSONL archive that `zoom_context` reads back.

## The handoff

Hosted providers get an AI handoff from the active model, written under fixed
headings: goal, current task, user constraints, decisions, files changed and
inspected, verified results (naming the command or test that showed them),
unverified assumptions, failures, unresolved work and next steps.

Repeated compaction does not re-summarize earlier summaries:

- The earlier handoff is carried forward verbatim and shown to the summarizer
  as context only, so the new section reports what changed.
- Evidence lists (commands, reads) are rebuilt from each pass's archive rather
  than carried, so they do not repeat.
- Once the carried record passes 12 KiB, it is merged once into a single
  record (goal and constraints kept word for word). If that merge fails, the
  first section and the newest sections are kept and the rest are named as
  omitted, with the archives holding the exact history.

Local engines, plan-backed profiles and compactions between tool calls use the
deterministic structured record instead of a summarizer request. The context
panel labels it as a reduced record.

## Safeguards

- Cancellation, a 25-second timeout, an empty or invalid response, or a
  history changed underneath the summarizer leave the live history untouched;
  automatic compaction falls back to the deterministic record.
- If an automatic compaction reclaimed under a tenth of the history, the next
  one skips the summarizer: the retained tail itself fills the window, and
  another round-trip would refill and recompact on every turn.
- When one turn alone fills the window, its oldest tool output is shortened.
  If the request still cannot fit, the turn stops with a context-full notice
  rather than dropping messages silently.

Compaction lets a long task continue across several windows; it does not make
a session unbounded. Window size, cost and an irreducibly large current turn
still apply.
