// `@` file references in the composer: finding the word being typed, ranking a
// project's files against it, and writing the reference in. A reference is
// plain text in the message (`@src/main.tsx`), which every harness can read.

/** The `@word` the caret is in. The `@` opens a word, so an email address does not count. */
export function atToken(text: string, caret: number): { start: number; query: string } | null {
  const before = text.slice(0, caret);
  const start = before.lastIndexOf("@");
  if (start < 0) return null;
  if (start > 0 && !/\s/.test(before[start - 1] ?? "")) return null;
  const query = before.slice(start + 1);
  if (/\s/.test(query)) return null;
  // The caret mid-word: the rest of the word is part of what gets replaced.
  if (/^\S/.test(text.slice(caret))) return null;
  return { start, query };
}

const subsequence = (needle: string, hay: string) => {
  let at = 0;
  for (const c of needle) {
    at = hay.indexOf(c, at) + 1;
    if (at === 0) return false;
  }
  return true;
};

function score(path: string, query: string): number {
  const lower = path.toLowerCase();
  const name = lower.slice(lower.lastIndexOf("/") + 1);
  if (name === query) return 0;
  if (name.startsWith(query)) return 1;
  if (name.includes(query)) return 2;
  if (lower.includes(query)) return 3;
  return subsequence(query, lower) ? 4 : -1;
}

/** The best matches: the file's name before its folders, then the shorter path. */
export function rankFiles(files: readonly string[], query: string, limit = 8): string[] {
  const q = query.toLowerCase();
  if (!q) return [...files].sort((a, b) => a.length - b.length || a.localeCompare(b)).slice(0, limit);
  return files
    .map((path) => ({ path, rank: score(path, q) }))
    .filter((f) => f.rank >= 0)
    .sort((a, b) => a.rank - b.rank || a.path.length - b.path.length || a.path.localeCompare(b.path))
    .slice(0, limit)
    .map((f) => f.path);
}

/** The text with the token replaced by the reference, and where the caret goes. */
export function insertRef(
  text: string,
  token: { start: number; query: string },
  path: string,
): { text: string; caret: number } {
  const ref = /\s/.test(path) ? `@"${path}" ` : `@${path} `;
  const end = token.start + 1 + token.query.length;
  const rest = text.slice(end).replace(/^ /, "");
  return { text: text.slice(0, token.start) + ref + rest, caret: token.start + ref.length };
}
