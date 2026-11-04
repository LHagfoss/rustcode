// A project's mark: its initials in a small rounded square, tinted from its
// name so two projects are told apart before you read either (the sidebar's
// rows and the chat header).

import { cx } from "./primitives";

/**
 * A project's initials: the first letter of its first two words ("Rust Code" is
 * RC), else the first and last letter of its one word ("rustcode" is RE), so a
 * name's ends stand for it rather than its first two letters.
 */
export function projectInitials(name: string): string {
  const words = name.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  const first = words[0];
  if (!first) return "?";
  const letters = [...first];
  const head = letters[0] ?? "?";
  const tail = letters[letters.length - 1] ?? head;
  if (words.length > 1) return (head + (words[1]?.charAt(0) ?? "")).toUpperCase();
  return (letters.length > 2 ? head + tail : letters.join("")).toUpperCase();
}

/** The tints a mark picks from, all from the tokens: the name picks one, always the same. */
const TINTS = [
  "bg-accent/15 text-accent",
  "bg-success/15 text-success",
  "bg-warning/15 text-warning",
  "bg-danger/15 text-danger",
  "bg-fill-strong text-fg-muted",
];

/** Which tint a name gets: a small hash, so it is the same in every place the mark shows. */
export function projectTint(name: string): string {
  let hash = 0;
  for (const ch of name) hash = (hash * 31 + (ch.codePointAt(0) ?? 0)) % 9973;
  return TINTS[hash % TINTS.length] ?? "bg-fill-strong text-fg-muted";
}

export function ProjectMark({
  name,
  size = 16,
  className,
}: {
  name: string;
  size?: number;
  className?: string;
}) {
  return (
    <span
      aria-hidden
      title={name}
      style={{ width: size, height: size, fontSize: Math.max(8, Math.round(size * 0.44)) }}
      className={cx(
        "flex shrink-0 items-center justify-center rounded-xs font-semibold uppercase tracking-tight",
        projectTint(name),
        className,
      )}
    >
      {projectInitials(name)}
    </span>
  );
}
