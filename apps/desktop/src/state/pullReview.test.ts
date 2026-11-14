import { describe, expect, test } from "bun:test";
import { type PullDetail, parsePullDetail } from "../acp/rustcode";
import { formatReviewFeedback } from "../lib/reviewFeedback";
import {
  conversationFeedback,
  conversationRange,
  LARGE_FILE_LINES,
  placeConversations,
  pullDiffFiles,
  reviewConversations,
  unresolvedFeedback,
} from "./pullReview";

const DIFF = [
  "diff --git a/src/a.ts b/src/a.ts",
  "index 1..2 100644",
  "--- a/src/a.ts",
  "+++ b/src/a.ts",
  "@@ -1,3 +1,4 @@",
  " one",
  "-two",
  "+TWO",
  "+three",
  " four",
  "diff --git a/src/old.ts b/src/new.ts",
  "similarity index 90%",
  "rename from src/old.ts",
  "rename to src/new.ts",
  "--- a/src/old.ts",
  "+++ b/src/new.ts",
  "@@ -1 +1 @@",
  "-x",
  "+y",
  "diff --git a/src/same.ts b/src/moved.ts",
  "similarity index 100%",
  "rename from src/same.ts",
  "rename to src/moved.ts",
  "diff --git a/gone.ts b/gone.ts",
  "deleted file mode 100644",
  "--- a/gone.ts",
  "+++ /dev/null",
  "@@ -1 +0,0 @@",
  "-bye",
  "diff --git a/img.png b/img.png",
  "Binary files a/img.png and b/img.png differ",
].join("\n");

function detail(extra: Record<string, unknown> = {}): PullDetail {
  const parsed = parsePullDetail({
    pull: {
      number: 7,
      title: "T",
      author: "me",
      state: "open",
      headRef: "h",
      baseRef: "main",
      headSha: "abcdef1234",
      body: "",
      reviewComments: [
        {
          id: 1,
          author: "rev",
          path: "src/a.ts",
          line: 3,
          startLine: 2,
          side: "RIGHT",
          outdated: false,
          at: "2026-11-01T10:00:00Z",
          body: "Range.",
        },
        {
          id: 2,
          author: "me",
          path: "src/a.ts",
          line: 3,
          side: "RIGHT",
          outdated: false,
          at: "2026-11-01T11:00:00Z",
          body: "Fixed.",
          replyTo: 1,
        },
        {
          id: 3,
          author: "rev",
          path: "src/a.ts",
          line: 2,
          side: "RIGHT",
          outdated: false,
          at: "2026-11-01T12:00:00Z",
          body: "Second reply.",
          replyTo: 2,
        },
        {
          id: 4,
          author: "rev",
          path: "src/a.ts",
          line: 2,
          side: "LEFT",
          outdated: false,
          at: "2026-11-01T09:00:00Z",
          body: "Why remove?",
        },
        {
          id: 5,
          author: "rev",
          path: "src/a.ts",
          line: 40,
          outdated: true,
          at: "2026-11-01T08:00:00Z",
          body: "Old.",
        },
        {
          id: 6,
          author: "rev",
          path: "src/a.ts",
          line: 99,
          outdated: false,
          at: "2026-11-01T13:00:00Z",
          body: "Off the hunks.",
        },
      ],
      reviewThreads: [
        { id: "T1", resolved: true, outdated: false, rootComment: 1 },
        { id: "T4", resolved: false, outdated: false, rootComment: 4 },
      ],
      ...extra,
    },
  });
  if (!parsed) throw new Error("no detail");
  return parsed;
}

describe("pull diff files", () => {
  const files = pullDiffFiles(DIFF);
  test("each file keeps its status, counts and the old path of a rename", () => {
    expect(files.map((f) => [f.path, f.status, f.oldPath, f.added, f.removed])).toEqual([
      ["src/a.ts", "M", undefined, 2, 1],
      ["src/new.ts", "R", "src/old.ts", 1, 1],
      ["src/moved.ts", "R", "src/same.ts", 0, 0],
      ["gone.ts", "D", undefined, 0, 1],
      ["img.png", "M", undefined, 0, 0],
    ]);
    expect(files[4]!.binary).toBe(true);
    expect(files[0]!.lines.filter((l) => l.kind === "add").map((l) => l.new)).toEqual([2, 3]);
  });
  test("a file past the limit is large", () => {
    const long = [
      "diff --git a/big.ts b/big.ts",
      `@@ -0,0 +1,${LARGE_FILE_LINES + 1} @@`,
      ...Array.from({ length: LARGE_FILE_LINES + 1 }, (_, i) => `+line ${i}`),
    ].join("\n");
    expect(pullDiffFiles(long)[0]!.large).toBe(true);
    expect(files[0]!.large).toBe(false);
  });
});

describe("review conversations", () => {
  const conversations = reviewConversations(detail());
  test("replies, and replies to replies, join their first comment's conversation", () => {
    expect(conversations.map((c) => [c.id, c.comments.map((x) => x.id)])).toEqual([
      [5, [5]],
      [4, [4]],
      [1, [1, 2, 3]],
      [6, [6]],
    ]);
  });
  test("resolved state comes from the review threads, sides from the first comment", () => {
    const one = conversations.find((c) => c.id === 1)!;
    expect([one.resolved, one.threadId, one.side, one.startLine, one.line]).toEqual([
      true,
      "T1",
      "RIGHT",
      2,
      3,
    ]);
    const four = conversations.find((c) => c.id === 4)!;
    expect([four.resolved, four.side]).toEqual([false, "LEFT"]);
    // No side read: the pull request's version.
    expect(conversations.find((c) => c.id === 5)!.side).toBe("RIGHT");
  });
  test("an engine without review threads leaves every conversation unresolved", () => {
    expect(reviewConversations(detail({ reviewThreads: undefined })).every((c) => !c.resolved)).toBe(true);
  });

  test("each goes under its line on its side; outdated and off-hunk ones above the file", () => {
    const file = pullDiffFiles(DIFF)[0]!;
    const { atLine, unplaced } = placeConversations(file, conversations);
    const at = (id: number) => [...atLine.entries()].find(([, cs]) => cs.some((c) => c.id === id))?.[0];
    // A multi-line one sits under its last line: new line 3, the second addition.
    expect(file.lines[at(1)!]).toMatchObject({ kind: "add", new: 3 });
    // A base-side one sits under the removed line, not the added line with the same number.
    expect(file.lines[at(4)!]).toMatchObject({ kind: "del", old: 2 });
    expect(unplaced.map((c) => c.id)).toEqual([5, 6]);
  });

  test("a range reads as GitHub writes it", () => {
    expect(conversationRange({ line: 3, startLine: 2 })).toBe("2-3");
    expect(conversationRange({ line: 3, startLine: 3 })).toBe("3");
    expect(conversationRange({})).toBe("");
  });
});

describe("conversations into the composer", () => {
  const conversations = reviewConversations(detail());
  test("a conversation becomes its comments with file, range and author, replies marked", () => {
    const items = conversationFeedback(conversations.find((c) => c.id === 1)!);
    expect(items.map((i) => [i.key, i.author, i.path, i.startLine, i.line, i.reply])).toEqual([
      ["line:1", "rev", "src/a.ts", 2, 3, false],
      ["line:2", "me", "src/a.ts", 2, 3, true],
      ["line:3", "rev", "src/a.ts", 2, 3, true],
    ]);
    const text = formatReviewFeedback(items, { number: 7, title: "T", url: "u", headSha: "abcdef1" });
    expect(text).toContain("- **rev** on `src/a.ts:2-3`");
    expect(text).toContain("  > Range.");
  });
  test("all unresolved leaves the resolved conversation out", () => {
    const keys = unresolvedFeedback(conversations).map((i) => i.key);
    expect(keys).toEqual(["line:5", "line:4", "line:6"]);
  });
});
