// The strip's one line and its fold: `3 delegates · 2 running`, the same words a branch header
// uses, and a fold that is remembered per thread for the session and nowhere else.

import { describe, expect, test } from "bun:test";
import {
  foldStrip,
  stripCollapsed,
  stripCounts,
  stripFolded,
  stripSummary,
  subscribeStripFold,
} from "../state/delegateStrip";

describe("stripCounts", () => {
  test("counts a link by its state", () => {
    expect(stripCounts(["working", "needs", "done", "failed", "stopped"])).toEqual({
      delegates: 5,
      running: 1,
      needs: 1,
    });
  });

  test("counts a harness agent and RustCode's own as running, since only those at work are listed", () => {
    expect(stripCounts(["needs"], 2)).toEqual({ delegates: 3, running: 2, needs: 1 });
    expect(stripCounts([], 1)).toEqual({ delegates: 1, running: 1, needs: 0 });
  });

  test("has nothing to count for a strip with no rows", () => {
    expect(stripCounts([])).toEqual({ delegates: 0, running: 0, needs: 0 });
  });
});

describe("stripSummary", () => {
  test("counts the delegates, then what they do", () => {
    expect(stripSummary({ delegates: 3, running: 2, needs: 0 })).toBe("3 delegates · 2 running");
  });

  test("says one delegate, one needs you", () => {
    expect(stripSummary({ delegates: 1, running: 1, needs: 0 })).toBe("1 delegate · 1 running");
    expect(stripSummary({ delegates: 1, running: 0, needs: 1 })).toBe("1 delegate · 1 needs you");
  });

  test("keeps the agreement of the branch header for a wait", () => {
    expect(stripSummary({ delegates: 2, running: 0, needs: 2 })).toBe("2 delegates · 2 need you");
  });

  test("names both kinds of work, in that order", () => {
    expect(stripSummary({ delegates: 4, running: 2, needs: 2 })).toBe("4 delegates · 2 running · 2 need you");
  });

  test("keeps the count alone when nothing runs or waits", () => {
    expect(stripSummary({ delegates: 2, running: 0, needs: 0 })).toBe("2 delegates");
  });
});

describe("stripCollapsed", () => {
  test("folds the rows only where a header is there to unfold them", () => {
    expect(stripCollapsed({ delegates: 3, running: 2, needs: 0 }, true)).toBe(true);
    expect(stripCollapsed({ delegates: 2, running: 0, needs: 2 }, true)).toBe(true);
    expect(stripCollapsed({ delegates: 3, running: 2, needs: 0 }, false)).toBe(false);
  });

  test("draws the row a shorter strip is left with, though the reader folded a longer one", () => {
    expect(stripCollapsed({ delegates: 1, running: 1, needs: 0 }, true)).toBe(false);
    expect(stripCollapsed({ delegates: 0, running: 0, needs: 0 }, true)).toBe(false);
  });
});

describe("foldStrip", () => {
  test("is unfolded until the reader folds it", () => {
    expect(stripFolded("thread-a")).toBe(false);
  });

  test("is remembered per thread, in both directions", () => {
    foldStrip("thread-a", true);
    expect(stripFolded("thread-a")).toBe(true);
    expect(stripFolded("thread-b")).toBe(false);
    foldStrip("thread-a", false);
    expect(stripFolded("thread-a")).toBe(false);
  });

  test("redraws its subscribers once per change, and stops when they leave", () => {
    let seen = 0;
    const unsubscribe = subscribeStripFold(() => {
      seen += 1;
    });
    foldStrip("thread-c", true);
    // Folding an unfolded strip again says nothing new, so nothing redraws.
    foldStrip("thread-c", true);
    expect(seen).toBe(1);
    unsubscribe();
    foldStrip("thread-c", false);
    expect(seen).toBe(1);
    expect(stripFolded("thread-c")).toBe(false);
  });
});
