// Drafts kept across restarts: a composer's text, per thread (or "new"), in app storage. Bounded
// and quiet: a draft over 100 KB is not kept, and only the 200 newest survive, so a long-lived
// install cannot fill the web view's storage with half-written messages.
// Pure, so the store and the tests share it (state/store.ts).

/** Where the book lives in app storage. */
export const DRAFTS_KEY = "rustcode.drafts";
/** One draft larger than this is not kept: it would take megabytes to paste back. */
export const MAX_DRAFT_CHARS = 100 * 1024;
/** At most this many drafts are kept, newest first. */
export const MAX_DRAFTS = 200;

export type DraftBook = Record<string, string>;

/** A parsed book: a draft per key, in the order they were last written, bounded. */
export function parseDrafts(raw: string | null): DraftBook {
  if (!raw) return {};
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    // A book that is not JSON (an older build, a hand edit) starts empty.
    return {};
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const entries = Object.entries(value as Record<string, unknown>).filter(
    (entry): entry is [string, string] => typeof entry[1] === "string" && keeps(entry[1]),
  );
  // The newest are kept, and read back in the order they were written.
  return Object.fromEntries(entries.slice(-MAX_DRAFTS));
}

/** Whether a draft is worth keeping: it has something in it and is not too big. */
export function keeps(text: string): boolean {
  return text.length > 0 && text.length <= MAX_DRAFT_CHARS;
}

/**
 * The book with one draft written: an empty text forgets the key, and the key moves to the newest
 * end (so the cap drops the drafts touched longest ago). The same record comes back when nothing
 * changed, so a save that changes nothing has nothing to write.
 *
 * The book in memory is not bounded by size — a draft over the limit is still typed, still
 * editable; it is `persistable` that keeps it out of storage.
 */
export function withDraft(book: DraftBook, key: string, text: string): DraftBook {
  const current = book[key];
  if (!text) {
    if (current === undefined) return book;
    const { [key]: _gone, ...rest } = book;
    return rest;
  }
  if (current === text && Object.keys(book).at(-1) === key) return book;
  const { [key]: _previous, ...rest } = book;
  return { ...rest, [key]: text };
}

/** The book as it goes to storage: nothing empty or over the limit, and only the newest kept. */
export function persistable(book: DraftBook): DraftBook {
  const entries = Object.entries(book).filter(([, text]) => keeps(text));
  return Object.fromEntries(entries.slice(-MAX_DRAFTS));
}
