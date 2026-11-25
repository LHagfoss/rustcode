import { describe, expect, test } from "bun:test";
import type { ReviewComment } from "./diff";
import {
  draftCounts,
  EMPTY_DRAFT,
  hasDraft,
  MAX_REVIEW_DRAFTS,
  parseReviewDrafts,
  reviewDraftKey,
  withReviewDraft,
} from "./reviewDrafts";

const note = (path: string, line = 1): ReviewComment => ({
  path,
  line,
  side: "RIGHT",
  body: `about ${path}`,
  code: "let x = 1;",
});

describe("review drafts", () => {
  test("a draft is kept per thread and source", () => {
    const turn = reviewDraftKey("t1", "turn:3");
    const branch = reviewDraftKey("t1", "branch");
    let book = withReviewDraft({}, turn, { comments: [note("a.ts")], note: "" });
    book = withReviewDraft(book, branch, { comments: [], note: "looks fine" });
    book = withReviewDraft(book, reviewDraftKey("t2", "uncommitted"), { comments: [note("b.ts")], note: "" });
    expect(book[turn]?.comments.map((c) => c.path)).toEqual(["a.ts"]);
    expect(book[branch]?.comments).toEqual([]);
    expect(draftCounts(book, "t1")).toEqual({ "turn:3": 1, branch: 1 });
    expect(draftCounts(book, "t2")).toEqual({ uncommitted: 1 });
  });

  test("an empty draft forgets its source, and an unchanged book is the same record", () => {
    const key = reviewDraftKey("t1", "uncommitted");
    const book = withReviewDraft({}, key, { comments: [note("a.ts")], note: "" });
    expect(withReviewDraft(book, key, EMPTY_DRAFT)).toEqual({});
    expect(withReviewDraft({}, key, { comments: [], note: "  " })).toEqual({});
    const empty = {};
    expect(withReviewDraft(empty, key, EMPTY_DRAFT)).toBe(empty);
    expect(hasDraft(undefined)).toBe(false);
  });

  test("storage round-trips and drops what is malformed", () => {
    const key = reviewDraftKey("t1", "pull:7");
    const book = withReviewDraft({}, key, { comments: [note("a.ts", 4)], note: "n" });
    expect(parseReviewDrafts(JSON.stringify(book))).toEqual(book);
    const raw = JSON.stringify({
      [key]: { comments: [{ path: "a.ts", line: "4", side: "RIGHT", body: "x" }, note("b.ts")], note: 3 },
      other: "text",
    });
    expect(parseReviewDrafts(raw)).toEqual({ [key]: { comments: [note("b.ts")], note: "" } });
    expect(parseReviewDrafts("not json")).toEqual({});
    expect(parseReviewDrafts(null)).toEqual({});
  });

  test("only the newest sources are kept", () => {
    let book = {};
    for (let i = 0; i <= MAX_REVIEW_DRAFTS; i++)
      book = withReviewDraft(book, reviewDraftKey("t", `turn:${i}`), { comments: [], note: "n" });
    const keys = Object.keys(book);
    expect(keys).toHaveLength(MAX_REVIEW_DRAFTS);
    expect(keys[0]).toBe(reviewDraftKey("t", "turn:1"));
  });
});
