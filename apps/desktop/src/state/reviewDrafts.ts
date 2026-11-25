import { create } from "zustand";
import { readStorage, writeStorage } from "../lib/storage";
import type { ReviewComment } from "./diff";

/**
 * Review drafts: the line comments and note written on the Review screen, per thread and per source
 * (the uncommitted changes, the branch, one turn, one pull request), kept in app storage so leaving
 * the screen or reloading the app loses none. Drafts written on one source never travel with
 * another: their lines belong to that diff.
 */
export const REVIEW_DRAFTS_KEY = "rustcode.reviewDrafts";
/** At most this many sources keep drafts, the ones written longest ago dropped first. */
export const MAX_REVIEW_DRAFTS = 100;

export interface ReviewDraft {
  comments: ReviewComment[];
  note: string;
}

export type ReviewDraftBook = Record<string, ReviewDraft>;

export const EMPTY_DRAFT: ReviewDraft = { comments: [], note: "" };

/** The key of one source's drafts in one thread. */
export function reviewDraftKey(threadId: string, source: string): string {
  return `${threadId}\n${source}`;
}

/** Whether a draft holds anything to keep. */
export function hasDraft(draft: ReviewDraft | undefined): draft is ReviewDraft {
  return !!draft && (draft.comments.length > 0 || draft.note.trim().length > 0);
}

function comment(value: unknown): ReviewComment | null {
  if (!value || typeof value !== "object") return null;
  const c = value as Record<string, unknown>;
  if (typeof c.path !== "string" || typeof c.body !== "string") return null;
  if (typeof c.line !== "number" || (c.side !== "LEFT" && c.side !== "RIGHT")) return null;
  return {
    path: c.path,
    line: c.line,
    side: c.side,
    body: c.body,
    code: typeof c.code === "string" ? c.code : "",
  };
}

/** A stored book, with anything malformed left out. */
export function parseReviewDrafts(raw: string | null): ReviewDraftBook {
  if (!raw) return {};
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return {};
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const out: ReviewDraftBook = {};
  for (const [key, entry] of Object.entries(value as Record<string, unknown>)) {
    if (!entry || typeof entry !== "object") continue;
    const e = entry as Record<string, unknown>;
    const draft: ReviewDraft = {
      comments: Array.isArray(e.comments)
        ? e.comments.map(comment).filter((c): c is ReviewComment => c !== null)
        : [],
      note: typeof e.note === "string" ? e.note : "",
    };
    if (hasDraft(draft)) out[key] = draft;
  }
  return Object.fromEntries(Object.entries(out).slice(-MAX_REVIEW_DRAFTS));
}

/** The book with one source's draft written: an empty draft forgets it, a written one moves last. */
export function withReviewDraft(book: ReviewDraftBook, key: string, draft: ReviewDraft): ReviewDraftBook {
  const { [key]: _previous, ...rest } = book;
  if (!hasDraft(draft)) return key in book ? rest : book;
  return Object.fromEntries(Object.entries({ ...rest, [key]: draft }).slice(-MAX_REVIEW_DRAFTS));
}

/** How many comments each source of a thread holds in drafts, by source key. */
export function draftCounts(book: ReviewDraftBook, threadId: string): Record<string, number> {
  const prefix = reviewDraftKey(threadId, "");
  const out: Record<string, number> = {};
  for (const [key, draft] of Object.entries(book)) {
    if (key.startsWith(prefix)) out[key.slice(prefix.length)] = Math.max(draft.comments.length, 1);
  }
  return out;
}

export const useReviewDrafts = create<{ book: ReviewDraftBook }>(() => ({
  book: parseReviewDrafts(readStorage(REVIEW_DRAFTS_KEY)),
}));

/** Writes one source's draft and keeps the book in app storage. */
export function setReviewDraft(key: string, draft: ReviewDraft): void {
  const before = useReviewDrafts.getState().book;
  const book = withReviewDraft(before, key, draft);
  if (book === before) return;
  useReviewDrafts.setState({ book });
  writeStorage(REVIEW_DRAFTS_KEY, Object.keys(book).length ? JSON.stringify(book) : null);
}
