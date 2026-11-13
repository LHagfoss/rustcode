// The project picker (issue 3598): one rule for the header and the composer's folder button.

import { describe, expect, test } from "bun:test";
import { isCurrentProject, moveBlockedReason, projectPick } from "./projectPick";

const thread = { id: "t1", cwd: "/code/app" };

describe("projectPick", () => {
  test("with no thread open, picks where the next thread starts", () => {
    expect(projectPick(undefined, "/code/site", true)).toEqual({ kind: "start-in", path: "/code/site" });
  });

  test("moves the open thread to another project, turns or not", () => {
    expect(projectPick(thread, "/code/site", true)).toEqual({ kind: "move", id: "t1", path: "/code/site" });
  });

  test("picking the thread's own project does nothing", () => {
    expect(projectPick(thread, "/code/app/", true)).toEqual({ kind: "none" });
  });

  test("a running thread is not moved, and says why", () => {
    const pick = projectPick({ ...thread, running: true }, "/code/site", true);
    expect(pick.kind).toBe("blocked");
  });

  test("an engine without session/move cannot move a thread", () => {
    expect(projectPick(thread, "/code/site", false).kind).toBe("blocked");
  });
});

describe("moveBlockedReason", () => {
  test("is empty for an idle thread and for no thread", () => {
    expect(moveBlockedReason(thread, true)).toBeUndefined();
    expect(moveBlockedReason(undefined, false)).toBeUndefined();
  });

  test("asks to stop a running turn first", () => {
    expect(moveBlockedReason({ ...thread, running: true }, true)).toContain("Stop the running turn");
  });
});

describe("isCurrentProject", () => {
  test("is the thread's folder, or the repo its worktree was cut from", () => {
    const inWorktree = { id: "t2", cwd: "/wt/app-feature", repo: "/code/app" };
    expect(isCurrentProject(inWorktree, "", "/code/app")).toBe(true);
    expect(isCurrentProject(inWorktree, "", "/wt/app-feature")).toBe(true);
    expect(isCurrentProject(inWorktree, "", "/code/site")).toBe(false);
  });

  test("with no thread, is the folder the next thread starts in", () => {
    expect(isCurrentProject(undefined, "/code/app", "/code/app")).toBe(true);
  });
});
