import { describe, expect, test } from "bun:test";
import { folderRows, matchSpans, middleEllipsis } from "./folderList";

const HOME = "/Users/ada";
const CODE = "/Users/ada/code";
const REPO = "/Users/ada/code/rustcode";
const OTHER = "/Users/ada/code/notes";
const WORKTREES = `${HOME}/.config/rustcode/worktrees`;
const CUT = `${WORKTREES}/rustcode/feat-composer-footer`;

describe("the folder picker's rows", () => {
  test("a folder that only holds other projects is not offered", () => {
    const rows = folderRows([{ cwd: "/" }, { cwd: HOME }, { cwd: CODE }, { cwd: REPO }, { cwd: OTHER }]);
    expect(rows.map((row) => row.path)).toEqual([REPO, OTHER]);
  });

  test("the home folder is a place projects live, so it goes too", () => {
    const rows = folderRows([{ cwd: HOME }, { cwd: REPO }]);
    expect(rows.map((row) => row.path)).toEqual([REPO]);
  });

  test("a folder with nothing of its own inside it stays", () => {
    const rows = folderRows([{ cwd: HOME }, { cwd: "/Volumes/work" }]);
    expect(rows.map((row) => row.path)).toEqual([HOME, "/Volumes/work"]);
  });

  test("the newest folder first, once each", () => {
    const rows = folderRows([{ cwd: OTHER }, { cwd: REPO }, { cwd: OTHER }]);
    expect(rows.map((row) => row.name)).toEqual(["notes", "rustcode"]);
  });

  test("a worktree nests under its repo, right after it, named by its branch", () => {
    const rows = folderRows([
      { cwd: CUT, worktree: { path: CUT, repo: REPO, branch: "feat/composer" } },
      { cwd: OTHER },
      { cwd: REPO },
    ]);
    expect(rows.map((row) => row.path)).toEqual([OTHER, REPO, CUT]);
    expect(rows[2]).toEqual({ path: CUT, name: "feat/composer", repo: REPO });
  });

  test("a worktree the engine did not record still finds its repo by name", () => {
    const rows = folderRows([{ cwd: REPO }, { cwd: `${WORKTREES}/rustcode/fix-tray` }]);
    expect(rows.map((row) => row.path)).toEqual([REPO, `${WORKTREES}/rustcode/fix-tray`]);
    expect(rows[1]!.repo).toBe(REPO);
    expect(rows[1]!.name).toBe("fix-tray");
  });

  test("a worktree with no repo on the list is a row of its own, after the projects", () => {
    const rows = folderRows([
      { cwd: OTHER },
      { cwd: CUT, worktree: { path: CUT, repo: REPO, branch: "feat/composer" } },
    ]);
    expect(rows.map((row) => row.path)).toEqual([OTHER, CUT]);
    expect(rows[1]!.repo).toBeUndefined();
  });

  test("a filtering search keeps a worktree under the repo it names", () => {
    const sources = [
      { cwd: REPO },
      { cwd: CUT, worktree: { path: CUT, repo: REPO, branch: "feat/composer" } },
      { cwd: OTHER },
    ];
    expect(folderRows(sources, { filter: "rust" }).map((row) => row.path)).toEqual([REPO, CUT]);
    expect(folderRows(sources, { filter: "composer" }).map((row) => row.path)).toEqual([CUT]);
    expect(folderRows(sources, { filter: "notes" }).map((row) => row.path)).toEqual([OTHER]);
  });

  test("a path matches whatever the folder is called", () => {
    const rows = folderRows([{ cwd: REPO }], { filter: "/code/" });
    expect(rows.map((row) => row.path)).toEqual([REPO]);
  });

  test("the list stops at the cap", () => {
    const sources = Array.from({ length: 8 }, (_, i) => ({ cwd: `/tmp/p${i}` }));
    expect(folderRows(sources, { limit: 3 })).toHaveLength(3);
  });
});

describe("the matched characters", () => {
  test("a row with no search is one plain run", () => {
    expect(matchSpans("rustcode", "")).toEqual([{ text: "rustcode", hit: false }]);
  });

  test("the matched run is marked, either side of it plain", () => {
    expect(matchSpans("rustcode", "ust")).toEqual([
      { text: "r", hit: false },
      { text: "ust", hit: true },
      { text: "code", hit: false },
    ]);
  });

  test("every occurrence is marked", () => {
    expect(matchSpans("banana", "an").filter((span) => span.hit)).toHaveLength(2);
  });

  test("a match at either end needs no empty run", () => {
    expect(matchSpans("rust", "ru")).toEqual([
      { text: "ru", hit: true },
      { text: "st", hit: false },
    ]);
    expect(matchSpans("rust", "st")[0]).toEqual({ text: "ru", hit: false });
  });
});

describe("a path with its middle given up", () => {
  test("both ends stay, the middle goes", () => {
    expect(middleEllipsis("/Users/ada/code/rustcode", 20)).toContain("…");
    expect(middleEllipsis("/Users/ada/code/rustcode", 20).startsWith("/Users")).toBe(true);
    expect(middleEllipsis("/Users/ada/code/rustcode", 20).endsWith("rustcode")).toBe(true);
  });

  test("never longer than the budget", () => {
    for (let max = 6; max < 40; max++) {
      expect(middleEllipsis("/Users/ada/code/rustcode", max).length).toBeLessThanOrEqual(max);
    }
  });

  test("a path that fits comes back whole, and a sliver is still cut", () => {
    expect(middleEllipsis("/tmp/app", 20)).toBe("/tmp/app");
    // Too little room for an ellipsis between two ends: a hard cut, never over the budget.
    expect(middleEllipsis("/tmp/app", 4)).toBe("/tmp");
  });
});
