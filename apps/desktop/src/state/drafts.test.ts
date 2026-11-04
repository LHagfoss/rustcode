import { describe, expect, test } from "bun:test";
import {
  DRAFTS_KEY,
  type DraftBook,
  keeps,
  MAX_DRAFT_CHARS,
  MAX_DRAFTS,
  parseDrafts,
  persistable,
  withDraft,
} from "./drafts";

describe("the drafts kept across restarts", () => {
  test("reads back what was written, in the order it was written", () => {
    const book = withDraft(withDraft({}, "a", "one"), "b", "two");
    expect(book).toEqual({ a: "one", b: "two" });
    expect(parseDrafts(JSON.stringify(book))).toEqual({ a: "one", b: "two" });
  });

  test("clearing a draft forgets it, and a large one is still typed", () => {
    const book = withDraft({ a: "one", b: "two" }, "a", "");
    expect(book).toEqual({ b: "two" });
    // In memory an over-large draft stays: it is being written, it is just not stored.
    const big = "x".repeat(MAX_DRAFT_CHARS + 1);
    expect(withDraft({}, "c", big)).toEqual({ c: big });
    expect(keeps("")).toBe(false);
    expect(keeps("x".repeat(MAX_DRAFT_CHARS))).toBe(true);
  });

  test("what goes to storage drops empty and over-large drafts", () => {
    const big = "x".repeat(MAX_DRAFT_CHARS + 1);
    expect(persistable({ a: "one", b: "", c: big })).toEqual({ a: "one" });
  });

  test("writing the same text again changes nothing, so nothing is saved", () => {
    const book: DraftBook = { a: "one" };
    expect(withDraft(book, "a", "one")).toBe(book);
  });

  test("writing a key again moves it to the newest end", () => {
    const book = withDraft(withDraft(withDraft({}, "a", "one"), "b", "two"), "a", "three");
    expect(Object.keys(book)).toEqual(["b", "a"]);
    expect(book.a).toBe("three");
  });

  test("only the newest drafts are kept", () => {
    let book: DraftBook = {};
    for (let i = 0; i <= MAX_DRAFTS + 5; i++) book = withDraft(book, `t${i}`, `draft ${i}`);
    expect(Object.keys(persistable(book)).length).toBe(MAX_DRAFTS);
    const keys = Object.keys(persistable(book));
    expect(keys.at(-1)).toBe(`t${MAX_DRAFTS + 5}`);
    expect(keys).not.toContain("t0");
  });

  test("a book that is not a book reads back empty, and keeps what it can", () => {
    expect(parseDrafts(null)).toEqual({});
    expect(parseDrafts("{ not json")).toEqual({});
    expect(parseDrafts("[]")).toEqual({});
    const big = "x".repeat(MAX_DRAFT_CHARS + 1);
    expect(parseDrafts(`{"a":1,"b":"two","c":"","d":"${big}"}`)).toEqual({ b: "two" });
  });

  test("an old book over the cap is trimmed on the way in", () => {
    const raw = Object.fromEntries(
      Array.from({ length: MAX_DRAFTS + 10 }, (_, i) => [`t${i}`, `draft ${i}`]),
    );
    const book = parseDrafts(JSON.stringify(raw));
    expect(Object.keys(book).length).toBe(MAX_DRAFTS);
    expect(Object.keys(book).at(-1)).toBe(`t${MAX_DRAFTS + 9}`);
  });

  test("the storage key is the app's own", () => {
    expect(DRAFTS_KEY).toBe("rustcode.drafts");
  });
});
