// The last answer each Settings page read, kept in memory so reopening a page
// draws it at once and refreshes behind it. In memory only: answers hold account
// names and plan numbers, which must not outlive the app (or its engine).

import type { JsonObject } from "../acp/jsonrpc";

/** A page read this recently is fresh enough to skip a refetch when reopened. */
export const PAGE_FRESH_MS = 30_000;

/** One answer and when it was read. */
export interface PageAnswer<T> {
  data: T;
  readAt: number;
}

const answers = new Map<string, PageAnswer<unknown>>();
/** The latest read started per key; an answer whose read is older is ignored. */
const reads = new Map<string, number>();

/**
 * A key for one ask: the method and its params, order-independent, so a page
 * asking the same thing reads the same answer and a different ask never does.
 */
export function pageKey(method: string, params: JsonObject = {}): string {
  const keys = Object.keys(params).sort();
  if (!keys.length) return method;
  const ordered = Object.fromEntries(keys.map((key) => [key, params[key]]));
  return `${method} ${JSON.stringify(ordered)}`;
}

export function cachedPage<T>(key: string): PageAnswer<T> | undefined {
  return answers.get(key) as PageAnswer<T> | undefined;
}

/** True when a page can draw this answer and skip the read for now. */
export function pageIsFresh(answer: PageAnswer<unknown> | undefined, now = Date.now()): boolean {
  return !!answer && now - answer.readAt < PAGE_FRESH_MS;
}

/** Starts a read: its token is the latest, so an answer to an older one is dropped. */
export function beginPageRead(key: string): number {
  const token = (reads.get(key) ?? 0) + 1;
  reads.set(key, token);
  return token;
}

export function isLatestPageRead(key: string, token: number): boolean {
  return reads.get(key) === token;
}

/** Keeps an answer, unless a read that started later has already been asked. */
export function commitPage<T>(key: string, token: number, data: T, now = Date.now()): PageAnswer<T> | null {
  if (!isLatestPageRead(key, token)) return null;
  const answer = { data, readAt: now };
  answers.set(key, answer);
  return answer;
}

/** Writes an answer a page got from an action, which outranks reads already in flight. */
export function rememberPage<T>(key: string, data: T, now = Date.now()): PageAnswer<T> {
  beginPageRead(key);
  const answer = { data, readAt: now };
  answers.set(key, answer);
  return answer;
}

/** Drops an answer and any read in flight for it. */
export function forgetPage(key: string): void {
  beginPageRead(key);
  answers.delete(key);
}

/** One engine's answers never show for another, after an engine restart or a test. */
export function clearPageCache(): void {
  answers.clear();
  reads.clear();
}

/** Whether two answers are the same value, so a page keeps its state (and rows) unchanged. */
export function sameValue(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object") return false;
  return JSON.stringify(a) === JSON.stringify(b);
}
