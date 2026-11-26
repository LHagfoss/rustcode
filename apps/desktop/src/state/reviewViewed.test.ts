import { describe, expect, test } from "bun:test";
import type { PatchLine } from "./diff";
import {
  MAX_VIEWED,
  parseViewed,
  patchHash,
  stepFile,
  viewedKey,
  viewedState,
  withViewed,
} from "./reviewViewed";

const lines = (text: string): PatchLine[] => [{ kind: "add", text, new: 1 }];

describe("review viewed marks", () => {
  test("a mark holds while the patch is the same and turns to changed when it moves", () => {
    const key = viewedKey("t1", "uncommitted", "src/a.ts");
    const read = patchHash(lines("one"));
    const book = withViewed({}, key, read);
    expect(viewedState(book, key, read)).toBe("viewed");
    expect(viewedState(book, key, patchHash(lines("two")))).toBe("changed");
    expect(viewedState(book, viewedKey("t1", "branch", "src/a.ts"), read)).toBe("unviewed");
    expect(withViewed(book, key, null)).toEqual({});
  });

  test("the hash tells kinds and texts apart", () => {
    expect(patchHash(lines("x"))).toBe(patchHash(lines("x")));
    expect(patchHash(lines("x"))).not.toBe(patchHash([{ kind: "del", text: "x", old: 1 }]));
    expect(patchHash([])).not.toBe(patchHash(lines("")));
  });

  test("storage round-trips, drops what is malformed and keeps the newest", () => {
    let book = {};
    for (let i = 0; i <= MAX_VIEWED; i++) book = withViewed(book, `k${i}`, "h");
    expect(Object.keys(book)).toHaveLength(MAX_VIEWED);
    expect(parseViewed(JSON.stringify(book))).toEqual(book);
    expect(parseViewed(JSON.stringify({ a: "h", b: 3 }))).toEqual({ a: "h" });
    expect(parseViewed("{")).toEqual({});
  });

  test("stepping moves through the files, wrapping, and can skip viewed ones", () => {
    const order = ["a", "b", "c", "d"];
    expect(stepFile(order, "a", 1)).toBe("b");
    expect(stepFile(order, "a", -1)).toBe("d");
    expect(stepFile(order, "d", 1)).toBe("a");
    expect(stepFile(order, null, 1)).toBe("a");
    expect(stepFile(order, null, -1)).toBe("d");
    const viewed = (p: string) => p === "b" || p === "c";
    expect(stepFile(order, "a", 1, viewed, true)).toBe("d");
    expect(stepFile(order, "d", 1, (p) => p !== "d", true)).toBeNull();
    expect(stepFile([], null, 1)).toBeNull();
  });
});
