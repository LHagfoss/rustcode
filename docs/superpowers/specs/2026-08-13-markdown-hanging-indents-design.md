# Markdown Hanging Indents Design

## Goal

Keep wrapped Markdown lists and blockquotes visually associated with their
opening marker in terminal chat transcripts.

## Design

The Markdown renderer will retain a continuation prefix while flushing each
paragraph. A wrapped ordinary blockquote repeats its muted `│ ` gutter. A
wrapped list item receives spaces equal to its opening marker width. List items
inside a blockquote combine both prefixes, so the quote gutter remains visible
and wrapped content stays aligned under its list text.

```text
• A long list item that wraps
  under its bullet.

│ A long quote that wraps
│ with its gutter retained.
```

The existing width-aware word wrapper remains responsible for breaking prose;
the new prefix is simply seeded onto every continuation output line.

## Non-goals

- No parser or dependency change.
- No task-list checkbox presentation yet.
- No changes to normal paragraphs, tables, code fences, or native scrollback.
