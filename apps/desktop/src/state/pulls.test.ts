// The sidebar's pull request marks: which pull a row shows, and how its mark reads.

import { describe, expect, test } from "bun:test";
import { parsePull } from "../acp/rustcode";
import { projectFolders, pullForBranch, pullState } from "./pulls";

const pull = (over: Record<string, unknown> = {}) =>
  parsePull({
    number: 12,
    title: "feat(desktop): a branch",
    author: "you",
    state: "open",
    draft: false,
    headRef: "feat/desktop-branch",
    baseRef: "main",
    additions: 3,
    deletions: 1,
    changedFiles: 2,
    ...over,
  })!;

describe("the sidebar's pull requests", () => {
  test("a row finds the pull its branch has", () => {
    const pulls = [pull(), pull({ number: 13, headRef: "fix/engine" })];
    expect(pullForBranch(pulls, "fix/engine")?.number).toBe(13);
    expect(pullForBranch(pulls, "feat/desktop-branch")?.number).toBe(12);
  });

  test("a branch without a pull, or no branch at all, says nothing", () => {
    const pulls = [pull()];
    expect(pullForBranch(pulls, "feat/other")).toBeUndefined();
    expect(pullForBranch(pulls, undefined)).toBeUndefined();
    expect(pullForBranch(undefined, "feat/desktop-branch")).toBeUndefined();
  });

  test("a draft is its own state; a merged or closed pull keeps its own", () => {
    expect(pullState(pull())).toBe("open");
    expect(pullState(pull({ draft: true }))).toBe("draft");
    expect(pullState(pull({ state: "merged" }))).toBe("merged");
    expect(pullState(pull({ state: "closed", draft: true }))).toBe("closed");
  });

  test("one read per project, deduplicated and capped", () => {
    expect(projectFolders(["/a", "/a", "", "/b", "/c", "/d", "/e"])).toEqual(["/a", "/b", "/c", "/d"]);
  });
});
