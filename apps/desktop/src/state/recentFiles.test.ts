import { beforeEach, describe, expect, test } from "bun:test";
import { forgetRecentFiles, noteRecentFile, recentFiles, recentFirst } from "./recentFiles";

beforeEach(() => forgetRecentFiles());

describe("recentFirst", () => {
  test("puts the recent ones first, newest first, and leaves the rest as they came", () => {
    expect(recentFirst(["a", "b", "c", "d"], ["c", "a"])).toEqual(["c", "a", "b", "d"]);
  });

  test("a file the folder no longer lists, or one named twice, changes nothing", () => {
    expect(recentFirst(["a", "b"], ["gone", "b", "b"])).toEqual(["b", "a"]);
    expect(recentFirst([], ["a"])).toEqual([]);
  });
});

describe("what a folder remembers", () => {
  test("the last file pointed at comes first, and each is named once", () => {
    noteRecentFile("/one", "a.ts");
    noteRecentFile("/one", "b.ts");
    noteRecentFile("/one", "a.ts");
    expect(recentFiles("/one")).toEqual(["a.ts", "b.ts"]);
  });

  test("a folder keeps only its own, and no path means nothing to remember", () => {
    noteRecentFile("/one", "a.ts");
    noteRecentFile("", "a.ts");
    expect(recentFiles("/two")).toEqual([]);
    expect(recentFiles("")).toEqual([]);
  });

  test("it remembers twenty files at most", () => {
    for (let i = 0; i < 25; i++) noteRecentFile("/one", `f${i}.ts`);
    expect(recentFiles("/one")).toHaveLength(20);
    expect(recentFiles("/one")[0]).toBe("f24.ts");
  });
});
