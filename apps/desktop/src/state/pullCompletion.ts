// Completion in a pull request's reply and comment fields (issue 3769): `#` offers the repository's
// issues and pull requests by number or title, `@` the people taking part in the pull request.
// Pure, so tests drive it; the field draws the list and the engine reads the references.

import type { PullDetail } from "../acp/rustcode";
import { inCode } from "./compose";

/** An issue or pull request a `#` reference can name. */
export interface PullReference {
  number: number;
  title: string;
  kind: "issue" | "pull";
  state: string;
}

/** The trigger the caret is in: `#` or `@`, where it starts and what follows it. */
export interface CompletionToken {
  trigger: "#" | "@";
  start: number;
  end: number;
  query: string;
}

/** One choice in the list: what it shows and what it writes. */
export interface CompletionItem {
  key: string;
  label: string;
  detail?: string;
  insert: string;
}

export const MAX_COMPLETIONS = 8;

/**
 * The `#` or `@` token that ends at `caret`: the trigger starts the text or follows a space or an
 * opening bracket, what follows it has no space, the caret is not mid-word, and it is not in code.
 */
export function completionToken(text: string, caret: number): CompletionToken | null {
  const before = text.slice(0, caret);
  const start = Math.max(before.lastIndexOf("#"), before.lastIndexOf("@"));
  if (start < 0) return null;
  if (start > 0 && !/[\s([]/.test(before[start - 1] ?? "")) return null;
  const query = before.slice(start + 1);
  if (!/^[\w.-]*$/.test(query)) return null;
  if (/^[\w-]/.test(text.slice(caret))) return null;
  if (inCode(text, start)) return null;
  return { trigger: before[start] as "#" | "@", start, end: caret, query };
}

/** The people in the pull request: its author, then everyone who reviewed or commented, once each. */
export function pullPeople(
  detail: Pick<PullDetail, "author" | "activity" | "reviewComments"> | undefined,
): string[] {
  if (!detail) return [];
  const seen = new Set<string>();
  const out: string[] = [];
  for (const login of [
    detail.author,
    ...detail.activity.map((a) => a.author),
    ...detail.reviewComments.map((c) => c.author),
  ]) {
    if (!login || seen.has(login.toLowerCase())) continue;
    seen.add(login.toLowerCase());
    out.push(login);
  }
  return out;
}

/** The people matching `query`: those whose login starts with it first, then those containing it. */
export function rankPeople(people: readonly string[], query: string): CompletionItem[] {
  const q = query.toLowerCase();
  const starts = people.filter((p) => p.toLowerCase().startsWith(q));
  const contains = q
    ? people.filter((p) => !p.toLowerCase().startsWith(q) && p.toLowerCase().includes(q))
    : [];
  return [...starts, ...contains]
    .slice(0, MAX_COMPLETIONS)
    .map((login) => ({ key: `@${login}`, label: `@${login}`, insert: `@${login}` }));
}

/**
 * The references matching `query`: by number when it is digits (those starting with it, lowest
 * first), else by every word of it in the title. Newest first otherwise.
 */
export function rankReferences(references: readonly PullReference[], query: string): CompletionItem[] {
  const q = query.trim().toLowerCase();
  const newest = [...references].sort((a, b) => b.number - a.number);
  let picked: PullReference[];
  if (!q) picked = newest;
  else if (/^\d+$/.test(q)) {
    const exact = newest.filter((r) => String(r.number) === q);
    picked = [...exact, ...newest.filter((r) => String(r.number) !== q && String(r.number).startsWith(q))];
  } else {
    const words = q.split(/[\s_.-]+/).filter(Boolean);
    picked = newest.filter((r) => words.every((w) => r.title.toLowerCase().includes(w)));
  }
  return picked.slice(0, MAX_COMPLETIONS).map((r) => ({
    key: `#${r.number}`,
    label: `#${r.number}`,
    detail: `${r.kind === "pull" ? "Pull request" : "Issue"} · ${r.title}`,
    insert: `#${r.number}`,
  }));
}

/** `text` with the token replaced by `insert` and a space, and the caret after it. */
export function applyCompletion(
  text: string,
  token: Pick<CompletionToken, "start" | "end">,
  insert: string,
): { text: string; caret: number } {
  const after = text.slice(token.end);
  const spaced = after.startsWith(" ") ? insert : `${insert} `;
  return { text: text.slice(0, token.start) + spaced + after, caret: token.start + spaced.length };
}
