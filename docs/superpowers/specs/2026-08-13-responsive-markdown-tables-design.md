# Responsive Markdown Tables Design

## Goal

Keep Markdown tables readable on narrow terminal widths without changing their
comfortable wide-terminal appearance.

## Inspiration

Codex's terminal renderer classifies table columns and switches sufficiently
cramped tables from an aligned grid to vertical key/value records. RustCode
will adopt the same presentation decision with a smaller local implementation.

## Design

Wide tables retain the existing bordered grid. When a multi-column table is
too narrow to give each column a useful text width, each body row renders as a
compact record using the header as its field label:

```text
  Name: rustcode
  Purpose: terminal coding agent
```

The fallback threshold is based on a minimum readable width per column rather
than a fixed terminal size. Field labels are muted and bold; values use normal
text styling. Values retain ordinary word wrapping. Header-only and single
column tables keep their existing rendering.

## Non-goals

- No Markdown parser replacement or dependency change.
- No changes to code fences, inline formatting, links, lists, or blockquotes.
- No attempt to port Codex's full table column classifier or hyperlink model.
