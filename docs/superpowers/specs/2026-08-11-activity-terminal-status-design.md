# Activity-Aware Terminal Status Design

## Goal

Make RustCode's terminal title and footer communicate the same compact, animated activity state while keeping session names short and readable.

## Current behavior

- `src/main.rs` emits the terminal title through OSC sequences, using `rustcode` plus a custom or prompt-derived session title.
- `src/ui/mod.rs` renders the footer activity row with six animated blocks, random working messages, and a fixed-width Auto-Confirm center block.
- `AppState` exposes `AppStatus`, `running_tools`, generation timing, and the active session title.

## Design

### Terminal title

The title remains short and follows this shape:

```text
rustcode · Ready · <session>
[·] Working · <session>
[••] Working · <session>
[•••] Working · <session>
[!] Action Required · <session>
[>] Queued · <session>
```

The session name is sanitized for terminal-title control characters and capped to a compact display length. The title must use the active custom/generated session title when available, otherwise a safe fallback such as `session`.

Animation is active while a request is queued, the model is responding, tools are running, or the user must act. Idle titles are stable and must not emit unnecessary OSC updates.

### Activity states

Map application state deterministically:

- `Idle` → `Ready`
- `Queued` → `Queued`
- `Streaming` → `Working` / `Responding`
- non-empty `running_tools` → `Running · <tool>`
- `AwaitingToolConfirmation` or `AwaitingQuestion` → `Action Required`
- other interactive pickers → `Action Required`

Tool execution takes precedence over generic streaming text, and action-required states take precedence over background activity.

### Footer

The left activity row becomes a wider, reusable status block with 12–16 animated block cells, a deterministic label, and elapsed time when applicable. Example forms:

```text
[■■■■□□□□□□□□]  Working · Analyzing code · 12s
[■■■■■■■■■■■■]  Running · replace_file_content · 18s
[■■□□□□□□□□□□]  Queued · waiting for model
[!!!!!!!!!!!!]  ACTION REQUIRED · approve tool
[□□□□□□□□□□□□]  Ready
```

The middle Auto-Confirm block expands from its current fixed width to provide more breathing room. The right-side tokens, context, quota, and command hints remain intact.

Random status phrases are removed so the displayed state is trustworthy and testable.

### Code boundaries

Add pure formatting helpers for:

- Session-title sanitization and truncation
- Activity-state classification
- Animation-frame generation
- Terminal-title formatting
- Footer status text

Keep terminal escape-sequence emission in `src/main.rs` and UI rendering in `src/ui/mod.rs`. Share the activity classification/frame helpers between them so the title and footer cannot drift apart.

## Testing

Add unit tests covering:

- Title truncation at the configured limit
- Removal of OSC/control characters and title separators
- Idle, queued, streaming, tool-running, and action-required state mapping
- Tool/action precedence
- Animation frames reaching both ends of the block row
- Stable idle output

Run `cargo check --tests` and `cargo test` before completion.

## Non-goals

- No changes to model providers, rate-limit handling, tool parsing, or orchestration behavior.
- No redesign of the full TUI layout.
- No new session persistence format.
