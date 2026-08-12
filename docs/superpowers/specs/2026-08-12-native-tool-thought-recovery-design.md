# Native Tool Thought Recovery Design

## Problem

When a provider emits reasoning content together with a structured native tool
call, RustCode currently passes only the streamed text into the continuation
collector. If that text contains only `<think>` content, the collector treats
the response as cut off and asks the model to continue. This can regenerate
the same planning text several times before the tool is executed.

The renderer also extracts only the first `<think>` block. Repeated or
multi-block reasoning can therefore leak into the visible assistant answer,
and the user sees more than the intended single-line thought preview.

Finally, dynamic environment metadata is appended directly to the latest user
message. An incomplete user request can cause a local model to interpret the
metadata headings as part of the requested output.

## Goals

- Stop continuation as soon as a valid native tool call has been received,
  even when the accompanying text is reasoning-only.
- Render all reasoning blocks as one compact, non-interactive preview.
- Keep reasoning text out of the normal assistant answer.
- Clearly delimit injected environment metadata from user instructions.
- Add regression tests for each failure mode without changing model selection
  or the existing one-line preview style.

## Non-goals

- Changing the selected model, Ollama configuration, or thinking settings.
- Redesigning the thought-preview visual treatment beyond ensuring one compact
  preview.
- Changing continuation behavior for ordinary text-only responses.
- Changing the contents of the environment metadata itself.

## Design

### Native tool-call completion signal

`stream_request` already accumulates structured native tool calls in
`StreamBuffer`. The response passed to the continuation collector will carry
both the streamed text and whether native calls were captured. The collector
will stop continuation when either a complete answer exists or a native tool
call exists. Reasoning-only text will remain eligible for continuation only
when no structured tool call was received.

The existing text-protocol behavior remains unchanged: fenced or inline tool
syntax is still detected from response text by the existing parser.

### Thought normalization for rendering

The UI will use a pure helper that scans the complete assistant content for
all complete and incomplete `<think>` spans. It will return:

- the non-thinking assistant content with every thought span removed;
- one preview string formed from the first meaningful non-empty thought line.

The existing width-aware truncation helper remains responsible for limiting
the preview to one terminal line. The renderer will never expose an
expand/click interaction for thought content.

### Runtime metadata boundary

Dynamic environment text appended to the latest message will be wrapped in a
RustCode-owned metadata boundary with an explicit instruction that it is
context, not a continuation of the user request. The existing cache-friendly
placement at the tail of the request is retained.

The boundary is treated as prompt assembly data, not as user-visible answer
content, and existing environment values remain unchanged.

## Error handling

- A native tool call with empty reasoning or empty text is still a valid model
  response and must proceed to tool execution.
- An incomplete reasoning-only response without a tool call may still use the
  existing bounded continuation policy.
- If continuation is exhausted, existing recovery/finalization behavior is
  preserved.
- Malformed or invalid native tool calls continue through the existing tool
  validation path.

## Tests

Add focused unit tests that prove:

1. A reasoning-only text chunk plus a native tool-call signal does not request
   continuation.
2. A reasoning-only response without a tool call still requests bounded
   continuation.
3. Multiple thought blocks are reduced to one preview and removed from the
   answer content.
4. An unclosed thought block is removed from answer content without causing a
   renderer panic.
5. Environment metadata is delimited and remains appended after the user text.

Run `cargo check --tests` and `cargo test` before publishing.
