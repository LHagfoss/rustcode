# Output style

How the agent formats its replies, so they read well in the desktop app and
stay good in the terminal. The rules live in the system prompt; the
frontends only render what the agent writes.

## The rules

The root agent's prompt gets a `# Output style` section
(`OUTPUT_STYLE_PROMPT_HINT` in `rustcode/engine/src/tools/schema.rs`):

- Answer or result first. Never restate the question or announce what
  follows.
- Short paragraphs. Headings only for long, multi-part answers.
- Bullets for parallel items, numbered steps for sequences, a table for
  comparisons.
- Code fenced with its language; changes shown as ```` ```diff ````.
- Code cited as `` `path:line` `` in inline code, with the full repo-relative
  path. The citation rule itself is in the base `# Workflow` section, so
  subagent reports keep it too.
- No emoji, filler, or recap of what the reply already said.
- A next step only when the user has one to take.

Frontends that draw `rustcode-viz` get one more sentence
(`OUTPUT_STYLE_VIZ_HINT`): a block replaces a table or list instead of
repeating it, and its `summary` carries the takeaway. When to draw a block at
all is the `# Visualizations` section ([visualizations.md](visualizations.md));
when to use an interactive component is the `# Smart UI` section and the
`smart-ui` skill ([smart-ui.md](smart-ui.md)). This section does not repeat
either.

## Who gets what

| Client | Visualization level | Smart UI | Output style | Extra tokens |
| --- | --- | --- | --- | --- |
| TUI | 1 | none | rules + viz sentence | ~130 |
| Desktop app (Tauri) | 2 | per setting | rules + viz sentence | ~130 (plus the existing viz and Smart UI sections) |
| macOS app | 2 | per setting | rules + viz sentence | ~130 |
| Other ACP clients (Zed, …) | 0 | none | rules only | ~110 |
| Subagents, compact prompts (small local models) | any | any | none | 0 |
| Codex harness | n/a | n/a | runtime info + rules + citation rule, as `developerInstructions` | ~190 |
| ACP harnesses that take a client system prompt (Claude Code) | n/a | n/a | runtime info + rules + citation rule, as `systemPrompt` (`append`) | ~190 |
| Other ACP harnesses (Gemini, OpenCode) | n/a | n/a | none | 0 |

Every RustCode frontend renders CommonMark with tables and highlighted
fenced code, so the rules go to all root prompts. The section is appended
after the cached base prompt, like the visualization and Smart UI hints, so
clients with different capabilities still share the cached prefix. The
context modal counts it under the system prompt
(`controller/context_breakdown.rs`).

External harnesses never see RustCode's prompt. Codex takes the rules on
`thread/start` and `thread/resume` (see [harnesses.md](harnesses.md)). ACP has
no field for it, so ACP harnesses keep their own style.

## What the renderers draw

| Element | Desktop app | TUI |
| --- | --- | --- |
| Headings, lists, quotes, rules, strikethrough | yes | yes |
| Tables | yes (GFM) | yes |
| Fenced code with highlighting | Shiki, any bundled language | yes, including `diff` |
| `diff` fences | Shiki `diff` grammar | per-line add/remove colours |
| Task lists `- [ ]` | GFM checkbox | marker dropped |
| Bare URLs | link chip | link text |
| `path:line` | inline code, not clickable | inline code |
| Images, Mermaid, math | no | no |
| `rustcode-viz` | levels 1–2 | level 1 |
| `rustcode-ui v1` | when Smart UI is on | no |

The prompt asks only for what both draw. Task lists, images and Mermaid are
left out on purpose.

## Where the idea came from

Another orchestrator injects its client contract
per provider: Codex `additionalContext` entries kept under Codex's per-entry
size limit, a system-prompt append for harnesses that have one, and a
wrapped first prompt for ACP agents. It tells the agent what the client can
show (charts through its HTML preview, files linked by absolute path so they
open in its viewer) rather than a long style guide. Two lessons carried over:
gate each instruction on what the client can render, and keep it short and
specific, since a hedge such as "render a visual only when asked" switched
the behaviour off entirely.

## Changing the rules

1. Edit `OUTPUT_STYLE_PROMPT_HINT` or `OUTPUT_STYLE_VIZ_HINT` in
   `rustcode/engine/src/tools/schema.rs`.
2. Keep it inside the budget: `output_style_reaches_root_prompts_within_budget`
   in `rustcode/engine/src/tools/tests.rs` caps the section and the desktop
   app's total at 250 extra tokens.
3. Ask only for what every renderer in the table above draws, or gate the
   rule on a capability the client sends at `initialize`
   (`_meta.rustcode.visualizations`, `_meta.rustcode.smartUi`).
4. Run `cargo test --workspace`; the Codex harness test checks the
   instructions it sends.
