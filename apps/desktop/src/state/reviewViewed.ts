import { create } from "zustand";
import { readStorage, writeStorage } from "../lib/storage";
import type { PatchLine } from "./diff";

/**
 * Files marked viewed on the Review screen, per thread, source and path, each with a hash of the
 * patch that was read: when the agent changes the file again the hash no longer matches, the mark
 * drops and the file says it changed since it was viewed. Kept in app storage, bounded.
 */
export const REVIEW_VIEWED_KEY = "rustcode.reviewViewed";
/** At most this many files keep a mark, the ones marked longest ago dropped first. */
export const MAX_VIEWED = 2000;

export type ViewedBook = Record<string, string>;

export type ViewedState = "viewed" | "changed" | "unviewed";

export function viewedKey(threadId: string, source: string, path: string): string {
  return `${threadId}\n${source}\n${path}`;
}

/** A short, stable hash of a file's patch (FNV-1a over each line's kind and text). */
export function patchHash(lines: readonly PatchLine[]): string {
  let hash = 0x811c9dc5;
  const mix = (text: string) => {
    for (let i = 0; i < text.length; i++) {
      hash ^= text.charCodeAt(i);
      hash = Math.imul(hash, 0x01000193) >>> 0;
    }
  };
  for (const line of lines) mix(`${line.kind}\u0000${line.text}\n`);
  return `${lines.length.toString(36)}-${hash.toString(36)}`;
}

export function viewedState(book: ViewedBook, key: string, hash: string): ViewedState {
  const seen = book[key];
  if (seen === undefined) return "unviewed";
  return seen === hash ? "viewed" : "changed";
}

/** The book with one file marked (with the hash read) or unmarked (`null`). */
export function withViewed(book: ViewedBook, key: string, hash: string | null): ViewedBook {
  const { [key]: previous, ...rest } = book;
  if (hash === null) return previous === undefined ? book : rest;
  if (previous === hash && Object.keys(book).at(-1) === key) return book;
  return Object.fromEntries(Object.entries({ ...rest, [key]: hash }).slice(-MAX_VIEWED));
}

export function parseViewed(raw: string | null): ViewedBook {
  if (!raw) return {};
  try {
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    const entries = Object.entries(value as Record<string, unknown>).filter(
      (entry): entry is [string, string] => typeof entry[1] === "string",
    );
    return Object.fromEntries(entries.slice(-MAX_VIEWED));
  } catch {
    return {};
  }
}

/**
 * The path `step` files from `current` in `order` (wrapping), or with `unviewedOnly` the next one
 * after `current` that is not viewed; null when there is none.
 */
export function stepFile(
  order: readonly string[],
  current: string | null,
  step: 1 | -1,
  viewed: (path: string) => boolean = () => false,
  unviewedOnly = false,
): string | null {
  if (!order.length) return null;
  const at = current ? order.indexOf(current) : -1;
  const from = at < 0 && step < 0 ? 0 : at;
  for (let n = 1; n <= order.length; n++) {
    const i = (((from + step * n) % order.length) + order.length) % order.length;
    const path = order[i]!;
    if (!unviewedOnly || (!viewed(path) && path !== current)) return path;
  }
  return null;
}

export const useReviewViewed = create<{ book: ViewedBook }>(() => ({
  book: parseViewed(readStorage(REVIEW_VIEWED_KEY)),
}));

export function setViewed(key: string, hash: string | null): void {
  const before = useReviewViewed.getState().book;
  const book = withViewed(before, key, hash);
  if (book === before) return;
  useReviewViewed.setState({ book });
  writeStorage(REVIEW_VIEWED_KEY, Object.keys(book).length ? JSON.stringify(book) : null);
}
