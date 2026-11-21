# 0001: Ban code comments with a ratchet

Status: accepted, 2026-11-21

## Context

About 19,400 lines of `//` and `/* */` comments sat in the Rust and
TypeScript sources. A sample showed most explained a decision, some described
code that had since changed, a few repeated the line below, and some excused a
workaround that then spread. Comments are not checked by anything, so they rot
silently, and an excuse in a comment lets a hack pass review.

## Decision

- New code comments are banned. `scripts/check-comments.py` counts the
  comment lines in every tracked `.rs`, `.ts`, `.tsx`, `.js` and `.mjs` file
  and CI fails when a file has more than `scripts/comment-baseline.txt`
  allows.
- The baseline is a ratchet: when a file has fewer, CI fails until
  `python3 scripts/check-comments.py --update` is run and the smaller
  baseline committed, so a removed comment cannot come back.
- Not counted:
  - doc comments (`///`, `//!`, `/** */`): rustdoc, editor hovers and, for
    clap, the CLI's `--help` text;
  - `// SAFETY:` notes on `unsafe` and their continuation lines;
  - tool directives (`biome-ignore`, `@ts-expect-error`, `eslint-…`,
    `prettier-ignore`, `rustfmt::…`) and a first-line shebang.
- An existing comment is removed by its kind: a tautology or stale note is
  deleted; an excuse is deleted and the hack gets an issue; a decision moves
  into an ADR here or the doc for its area. `/delete-comment`
  (`.agents/skills/delete-comment/SKILL.md`) runs the pass on one area.

## Consequences

- A reason that matters is written once, in `docs/`, where review sees it.
- Small reasons go into code: a name (`template_safe_about`), a computed
  expected value in a test, or an assertion message.
- The check reads source text with a small lexer, not a compiler, so a rare
  construct may be counted wrongly. It counts the same way every run, so the
  ratchet still holds; fix the lexer if it misfires.
