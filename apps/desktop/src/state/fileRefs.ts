// `@` references in the composer: finding the word being typed, narrowing it to
// one group (`@agents`, `@threads`, `@files`), ranking a project's files
// against it, and writing the reference in. A reference is plain text in the
// message (`@src/main.tsx`), which every harness can read.

import type { TokenRange } from "./compose";

/** The groups an `@` token can name to look in one of them. */
export type MentionScope = "agents" | "threads" | "files";

/**
 * What narrows `@` to a group, in the order the drawer offers them: the word it writes, the name it
 * shows, and what the group holds. `agent`, `thread` and `file` narrow the same groups.
 */
export const SCOPES: { scope: MentionScope; word: string; label: string; hint: string }[] = [
  { scope: "agents", word: "agents", label: "Agents", hint: "Harnesses and models to delegate to" },
  { scope: "threads", word: "threads", label: "Threads", hint: "Other threads to point the agent at" },
  { scope: "files", word: "files", label: "Files", hint: "This project's files and folders" },
];

/** The group a word narrows to, in either number. */
export function scopeOf(word: string): MentionScope | undefined {
  const asked = word.toLowerCase();
  return SCOPES.find((scope) => scope.word === asked || scope.word.slice(0, -1) === asked)?.scope;
}

/** What the drawer writes to narrow it: `agents`. */
export function scopeWord(scope: MentionScope): string {
  return SCOPES.find((entry) => entry.scope === scope)!.word;
}

/**
 * The `@word` the caret is in. The `@` opens a word, so an email address does not count, and a
 * word naming a group narrows the drawer: `@agents` looks in the agents, and `@agents pi` looks in
 * them for `pi`, the token reaching past the keyword's own word.
 */
export function atToken(text: string, caret: number): (TokenRange & { scope?: MentionScope }) | null {
  const before = text.slice(0, caret);
  const start = before.lastIndexOf("@");
  if (start < 0) return null;
  if (start > 0 && !/\s/.test(before[start - 1] ?? "")) return null;
  // The caret mid-word: the rest of the word is part of what gets replaced.
  if (/^\S/.test(text.slice(caret))) return null;
  const rest = before.slice(start + 1);
  // A keyword, then what is looked for in that group: `@agents pi`.
  const narrowed = /^(\S+)\s+(\S*)$/.exec(rest);
  if (narrowed) {
    const scope = scopeOf(narrowed[1]!);
    return scope ? { start, end: caret, query: narrowed[2]!, scope } : null;
  }
  if (/\s/.test(rest)) return null;
  // The keyword itself, on its way to being completed: the drawer is already narrow.
  const scope = scopeOf(rest);
  if (scope) return { start, end: caret, query: "", scope };
  return { start, end: caret, query: rest };
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
export function insertRef(text: string, token: TokenRange, path: string): { text: string; caret: number } {
  const ref = /\s/.test(path) ? `@"${path}" ` : `@${path} `;
  const rest = text.slice(token.end).replace(/^ /, "");
  return { text: text.slice(0, token.start) + ref + rest, caret: token.start + ref.length };
}

/**
 * The text with the token replaced by the group's keyword and a space, so the drawer stays open on
 * that group and the next word is what is looked for in it.
 */
export function insertKeyword(
  text: string,
  token: TokenRange,
  scope: MentionScope,
): { text: string; caret: number } {
  const ref = `@${scopeWord(scope)} `;
  const rest = text.slice(token.end).replace(/^ /, "");
  return { text: text.slice(0, token.start) + ref + rest, caret: token.start + ref.length };
}
