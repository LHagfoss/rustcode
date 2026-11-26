import { describe, expect, test } from "bun:test";
import type { PatchLine, ReviewComment } from "./diff";
import type { Conversation, PullDiffFile } from "./pullReview";
import {
  diffLineEditorTarget,
  diffRootForReview,
  orderedPullConversations,
  orderedReviewComments,
  reviewCommentKeyMap,
  stepTarget,
} from "./reviewNav";

const draft = (
  path: string,
  line: number,
  side: "LEFT" | "RIGHT" = "RIGHT",
  body = "note",
  startLine?: number,
): ReviewComment => ({
  path,
  line,
  ...(startLine !== undefined ? { startLine } : {}),
  side,
  body,
  code: "",
});

const conversation = (
  id: number,
  path: string,
  line?: number,
  options: { side?: "LEFT" | "RIGHT"; outdated?: boolean; resolved?: boolean } = {},
): Conversation => ({
  id,
  path,
  line,
  side: options.side ?? "RIGHT",
  resolved: options.resolved ?? false,
  outdated: options.outdated ?? false,
  comments: [
    {
      id,
      author: "alice",
      body: `comment ${id}`,
      path,
      line,
      side: options.side ?? "RIGHT",
      outdated: options.outdated ?? false,
    },
  ],
});

describe("orderedReviewComments", () => {
  test("orders drafts by file tree order, line number, side, and insertion order", () => {
    const a1 = draft("src/b.ts", 20);
    const a2 = draft("src/a.ts", 15, "RIGHT", "second on 15");
    const a3 = draft("src/a.ts", 15, "LEFT", "base on 15");
    const a4 = draft("src/a.ts", 15, "RIGHT", "third on 15");
    const a5 = draft("src/a.ts", 10, "RIGHT", "range", 6);
    const a6 = draft("unlisted.ts", 1);

    const ordered = orderedReviewComments([a1, a2, a3, a4, a5, a6], ["src/a.ts", "src/b.ts"]);
    expect(ordered.map((entry) => entry.comment)).toEqual([a5, a3, a2, a4, a1, a6]);

    const keys = reviewCommentKeyMap([a1, a2, a3, a4, a5, a6]);
    expect(ordered.map((entry) => entry.id)).toEqual([
      keys.get(a5)!,
      keys.get(a3)!,
      keys.get(a2)!,
      keys.get(a4)!,
      keys.get(a1)!,
      keys.get(a6)!,
    ]);
    expect(new Set(ordered.map((entry) => entry.id)).size).toBe(6);
  });
});

describe("orderedPullConversations", () => {
  test("matches PullDiff visual order: elsewhere, then file unplaced, then placed lines", () => {
    const files: Pick<PullDiffFile, "path" | "lines">[] = [
      {
        path: "src/first.ts",
        lines: [
          { kind: "hunk", text: "@@ -1,3 +1,3 @@" },
          { kind: "ctx", text: "a", old: 1, new: 1 },
          { kind: "del", text: "b", old: 2 },
          { kind: "add", text: "b2", new: 2 },
          { kind: "ctx", text: "c", old: 3, new: 3 },
        ],
      },
      {
        path: "src/second.ts",
        lines: [
          { kind: "hunk", text: "@@ -10,2 +10,2 @@" },
          { kind: "add", text: "x", new: 10 },
        ],
      },
    ];
    const cElsewhere = conversation(1, "README.md", 5);
    const cPlacedLater = conversation(2, "src/first.ts", 3);
    const cUnplacedOutdated = conversation(3, "src/first.ts", 2, { outdated: true });
    const cPlacedEarlier = conversation(4, "src/first.ts", 2, { side: "LEFT" });
    const cSecondFile = conversation(5, "src/second.ts", 10);

    const ordered = orderedPullConversations(
      [cPlacedLater, cSecondFile, cElsewhere, cUnplacedOutdated, cPlacedEarlier],
      files,
    );
    expect(ordered.map((entry) => ({ id: entry.id, placed: entry.placed }))).toEqual([
      { id: 1, placed: false },
      { id: 3, placed: false },
      { id: 4, placed: true },
      { id: 2, placed: true },
      { id: 5, placed: true },
    ]);
  });
});

describe("stepTarget", () => {
  const items = [{ id: "a" }, { id: "b" }, { id: "c" }];

  test("starts at ends when nothing is active and wraps in both directions", () => {
    expect(stepTarget(items, null, 1)).toEqual({ item: { id: "a" }, index: 0 });
    expect(stepTarget(items, null, -1)).toEqual({ item: { id: "c" }, index: 2 });
    expect(stepTarget(items, "a", 1)).toEqual({ item: { id: "b" }, index: 1 });
    expect(stepTarget(items, "c", 1)).toEqual({ item: { id: "a" }, index: 0 });
    expect(stepTarget(items, "a", -1)).toEqual({ item: { id: "c" }, index: 2 });
    expect(stepTarget(items, "missing", 1)).toEqual({ item: { id: "a" }, index: 0 });
    expect(stepTarget([], null, 1)).toBeNull();
  });
});

describe("diffLineEditorTarget", () => {
  const addLine: PatchLine = { kind: "add", text: "const x = 1;", new: 42 };
  const ctxLine: PatchLine = { kind: "ctx", text: "return x;", old: 40, new: 43 };
  const delLine: PatchLine = { kind: "del", text: "const old = 0;", old: 41 };
  const hunkLine: PatchLine = { kind: "hunk", text: "@@ -40,3 +42,2 @@" };

  test("resolves added and context lines on local files", () => {
    expect(diffLineEditorTarget("/repo/worktree", "src/app.ts", addLine)).toEqual({
      path: "/repo/worktree/src/app.ts",
      line: 42,
    });
    expect(
      diffLineEditorTarget("/repo/worktree", "src/app.ts", ctxLine, { local: true, fileStatus: "M" }),
    ).toEqual({
      path: "/repo/worktree/src/app.ts",
      line: 43,
    });
  });

  test("returns null for deleted lines, hunk headers, deleted files, and remote threads", () => {
    expect(diffLineEditorTarget("/repo", "src/app.ts", delLine)).toBeNull();
    expect(diffLineEditorTarget("/repo", "src/app.ts", hunkLine)).toBeNull();
    expect(diffLineEditorTarget("/repo", "src/app.ts", addLine, { fileStatus: "D" })).toBeNull();
    expect(diffLineEditorTarget("/repo", "src/app.ts", addLine, { local: false })).toBeNull();
    expect(diffLineEditorTarget("", "src/app.ts", addLine)).toBeNull();
    expect(diffLineEditorTarget("/repo", "", addLine)).toBeNull();
  });

  test("selects worktree root for branch and commit sources", () => {
    expect(diffRootForReview("branch", "/repo", "/wt/feat")).toBe("/wt/feat");
    expect(diffRootForReview("commit", "/repo", "/wt/feat")).toBe("/wt/feat");
    expect(diffRootForReview("uncommitted", "/repo", "/wt/feat")).toBe("/repo");
    expect(diffRootForReview("pull", "/repo", "/wt/feat")).toBe("/repo");
    expect(diffRootForReview("branch", "/repo")).toBe("/repo");
  });
});
