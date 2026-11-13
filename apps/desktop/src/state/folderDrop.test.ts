// Folders dropped on the sidebar (issue 3608): which are added, which open an existing project,
// and what the toasts say about the rest.

import { describe, expect, test } from "bun:test";
import { dropMessages, dropTarget, planFolderDrop } from "./folderDrop";

describe("planFolderDrop", () => {
  test("adds new folders and opens the ones that are projects already", () => {
    const plan = planFolderDrop(["/code/app", "/code/site"], ["folder", "folder"], ["/code/site"]);
    expect(plan).toEqual({ add: ["/code/app"], open: ["/code/site"], rejected: [] });
    expect(dropTarget(plan)).toBe("/code/app");
  });

  test("takes several folders at once, each once however it is spelled", () => {
    const plan = planFolderDrop(["/code/a", "/code/b/", "/code/a/"], ["folder", "folder", "folder"], []);
    expect(plan.add).toEqual(["/code/a", "/code/b"]);
  });

  test("matches a known project with a trailing slash", () => {
    const plan = planFolderDrop(["/code/site/"], ["folder"], ["/code/site"]);
    expect(plan.open).toEqual(["/code/site"]);
    expect(plan.add).toEqual([]);
  });

  test("refuses files, missing paths and unreadable folders", () => {
    const plan = planFolderDrop(
      ["/code/notes.md", "/gone", "/locked", "/code/app"],
      ["file", "missing", "denied", "folder"],
      [],
    );
    expect(plan.add).toEqual(["/code/app"]);
    expect(plan.rejected).toEqual([
      { path: "/code/notes.md", kind: "file" },
      { path: "/gone", kind: "missing" },
      { path: "/locked", kind: "denied" },
    ]);
  });

  test("a path the shell gave no kind for counts as missing", () => {
    expect(planFolderDrop(["/a"], [], []).rejected).toEqual([{ path: "/a", kind: "missing" }]);
  });

  test("a drop of only files leaves nothing to open", () => {
    expect(dropTarget(planFolderDrop(["/a.txt"], ["file"], []))).toBeUndefined();
  });
});

describe("dropMessages", () => {
  test("names one folder added", () => {
    expect(dropMessages({ add: ["/code/app"], open: [], rejected: [] })).toEqual({
      done: "Added app.",
      refused: undefined,
    });
  });

  test("counts several and says which were projects already", () => {
    const { done } = dropMessages({ add: ["/x/a", "/x/b"], open: ["/x/c"], rejected: [] });
    expect(done).toBe("Added 2 projects: a, b. c is already a project.");
  });

  test("says why a file is refused", () => {
    const { done, refused } = dropMessages({
      add: [],
      open: [],
      rejected: [{ path: "/x/notes.md", kind: "file" }],
    });
    expect(done).toBeUndefined();
    expect(refused).toBe("notes.md is a file, not a folder. Only folders can be added as projects.");
  });

  test("lists each refused item with its reason", () => {
    const { refused } = dropMessages({
      add: [],
      open: [],
      rejected: [
        { path: "/x/gone", kind: "missing" },
        { path: "/x/locked", kind: "denied" },
      ],
    });
    expect(refused).toContain("2 items were not added");
    expect(refused).toContain("gone could not be found");
    expect(refused).toContain("locked could not be read (permission denied)");
  });
});

test("keeps the filesystem root as a folder", () => {
  expect(planFolderDrop(["/"], ["folder"], []).add).toEqual(["/"]);
  expect(planFolderDrop(["/"], ["folder"], ["/"]).open).toEqual(["/"]);
});
