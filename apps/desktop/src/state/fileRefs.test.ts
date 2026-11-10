import { describe, expect, test } from "bun:test";
import { atToken, insertKeyword, insertRef, rankFiles, scopeOf } from "./fileRefs";

describe("atToken", () => {
  test("finds the word under the caret", () => {
    expect(atToken("look at @src/ma", 15)).toEqual({ start: 8, end: 15, query: "src/ma" });
    expect(atToken("@", 1)).toEqual({ start: 0, end: 1, query: "" });
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
    expect(insertRef("see @ma", { start: 4, end: 7, query: "ma" }, "src/main.ts")).toEqual({
      text: "see @src/main.ts ",
      caret: 17,
    });
  });
  test("quotes a path with a space", () => {
    expect(insertRef("@a", { start: 0, end: 2, query: "a" }, "a b.txt").text).toBe('@"a b.txt" ');
  });
  test("does not double the space before later text", () => {
    expect(insertRef("@ma now", { start: 0, end: 3, query: "ma" }, "m.ts").text).toBe("@m.ts now");
  });
});

describe("a group keyword narrows @", () => {
  test("the keyword alone, in either number", () => {
    expect(atToken("@agents", 7)).toEqual({ start: 0, end: 7, query: "", scope: "agents" });
    expect(atToken("@thread", 7)).toEqual({ start: 0, end: 7, query: "", scope: "threads" });
    expect(scopeOf("file")).toBe("files");
    expect(scopeOf("files")).toBe("files");
    expect(scopeOf("agent")).toBe("agents");
    expect(scopeOf("agentsx")).toBeUndefined();
  });

  test("what is looked for inside the group", () => {
    expect(atToken("look at @agents pi", 18)).toEqual({ start: 8, end: 18, query: "pi", scope: "agents" });
    expect(atToken("@agents pi", 10)).toEqual({ start: 0, end: 10, query: "pi", scope: "agents" });
    // The caret mid-word, or a word that is no keyword, is not one.
    // The caret inside the word (the `p` has not been typed yet at 9): the word's rest follows.
    expect(atToken("@agents pi", 8)).toBeNull();
    expect(atToken("@src pi", 7)).toBeNull();
  });

  test("completing a keyword writes it and keeps the drawer on that group", () => {
    expect(insertKeyword("@ag", { start: 0, end: 3, query: "ag" }, "agents")).toEqual({
      text: "@agents ",
      caret: 8,
    });
    expect(insertKeyword("look @th now", { start: 5, end: 8, query: "th" }, "threads").text).toBe(
      "look @threads now",
    );
  });
});
