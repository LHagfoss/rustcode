import { describe, expect, test } from "bun:test";
import { atToken, insertRef, rankFiles } from "./fileRefs";

describe("atToken", () => {
  test("finds the word under the caret", () => {
    expect(atToken("look at @src/ma", 15)).toEqual({ start: 8, query: "src/ma" });
    expect(atToken("@", 1)).toEqual({ start: 0, query: "" });
  });
  test("an email address is not a reference", () => {
    expect(atToken("mail me@example.com", 19)).toBeNull();
  });
  test("a finished word is left alone", () => {
    expect(atToken("@src/main.ts and more", 21)).toBeNull();
    expect(atToken("no at sign", 10)).toBeNull();
  });
  test("the caret inside a word opens nothing", () => {
    expect(atToken("@src/main.ts", 4)).toBeNull();
  });
  test("the caret before the word opens nothing", () => {
    expect(atToken("@main", 0)).toBeNull();
  });
});

describe("rankFiles", () => {
  const files = [
    "docs/main-guide.md",
    "src/main.tsx",
    "src/ui/Main.tsx",
    "src/domain.ts",
    "main",
    "apps/m/a/i/n.txt",
  ];
  test("prefers the exact name, then a name that starts with it, then the shorter path", () => {
    expect(rankFiles(files, "main")).toEqual([
      "main",
      "src/main.tsx",
      "src/ui/Main.tsx",
      "docs/main-guide.md",
      "src/domain.ts",
      "apps/m/a/i/n.txt",
    ]);
  });
  test("drops what does not match and keeps to the limit", () => {
    expect(rankFiles(files, "zzz")).toEqual([]);
    expect(rankFiles(files, "", 2)).toEqual(["main", "src/main.tsx"]);
  });
});

describe("insertRef", () => {
  test("replaces the token and leaves a space to keep typing", () => {
    expect(insertRef("see @ma", { start: 4, query: "ma" }, "src/main.ts")).toEqual({
      text: "see @src/main.ts ",
      caret: 17,
    });
  });
  test("quotes a path with a space", () => {
    expect(insertRef("@a", { start: 0, query: "a" }, "a b.txt").text).toBe('@"a b.txt" ');
  });
  test("does not double the space before later text", () => {
    expect(insertRef("@ma now", { start: 0, query: "ma" }, "m.ts").text).toBe("@m.ts now");
  });
});
