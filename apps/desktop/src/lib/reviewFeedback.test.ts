import { describe, expect, test } from "bun:test";
import { type PullDetail, parsePullDetail } from "../acp/rustcode";
import {
  appendToDraft,
  feedbackItems,
  feedbackLocation,
  formatReviewFeedback,
  keepSelected,
} from "./reviewFeedback";

const pr = {
  number: 42,
  title: "Fix the thing",
  url: "https://github.com/o/r/pull/42",
  headSha: "abcdef1234",
};

function detail(extra: Record<string, unknown> = {}): PullDetail {
  const parsed = parsePullDetail({
    pull: {
      number: 42,
      title: "Fix the thing",
      author: "me",
      state: "open",
      headRef: "fix/thing",
      baseRef: "main",
      headSha: "abcdef1234",
      body: "",
      activity: [
        { author: "rev", kind: "approved", at: "2026-10-30T15:00:00Z", body: "" },
        { author: "rev", kind: "changes_requested", at: "2026-10-30T16:00:00Z", body: "Needs tests." },
      ],
      reviewComments: [
        {
          id: 7,
          author: "rev",
          path: "src/a.rs",
          line: 12,
          startLine: 10,
          outdated: false,
          at: "2026-10-30T15:30:00Z",
          body: "Rename this.",
          url: "https://github.com/o/r/pull/42#discussion_r7",
        },
        { id: 8, author: "", path: "src/b.rs", line: 3, outdated: true, body: "Old\n\nnote", replyTo: 7 },
        { id: 9, author: "rev", path: "src/c.rs", outdated: false, body: "  " },
      ],
      ...extra,
    },
  });
  if (!parsed) throw new Error("no detail");
  return parsed;
}

describe("review feedback items", () => {
  test("line comments come first, and only feedback with a body is offered", () => {
    const items = feedbackItems(detail());
    expect(items.map((i) => i.key)).toEqual([
      "line:7",
      "line:8",
      "activity:changes_requested:rev:1793376000000:",
    ]);
    expect(items[1]?.outdated).toBe(true);
    expect(items[1]?.reply).toBe(true);
  });

  test("an engine without line comments still gives the activity", () => {
    const old = parsePullDetail({
      pull: { number: 1, title: "t", activity: [{ author: "a", kind: "commented", body: "hi" }] },
    });
    expect(old?.reviewComments).toEqual([]);
    expect(old?.reviewCommentsError).toBeUndefined();
    expect(feedbackItems(old!).map((i) => i.kind)).toEqual(["commented"]);
  });

  test("a read that could not fetch the line comments says why", () => {
    expect(detail({ reviewComments: undefined, reviewCommentsError: "HTTP 403" }).reviewCommentsError).toBe(
      "HTTP 403",
    );
  });

  test("a picked comment that is gone after a refresh leaves the selection", () => {
    const items = feedbackItems(detail());
    expect([...keepSelected(new Set(["line:7", "line:99"]), items)]).toEqual(["line:7"]);
  });

  test("a location is path:line, a range, or the path alone", () => {
    expect(
      feedbackLocation({ key: "", author: "", kind: "line", body: "", path: "a", line: 4, startLine: 2 }),
    ).toBe("a:2-4");
    expect(
      feedbackLocation({ key: "", author: "", kind: "line", body: "", path: "a", line: 4, startLine: 4 }),
    ).toBe("a:4");
    expect(feedbackLocation({ key: "", author: "", kind: "line", body: "", path: "a" })).toBe("a");
    expect(feedbackLocation({ key: "", author: "", kind: "commented", body: "" })).toBeUndefined();
  });
});

describe("the draft", () => {
  test("names the pull request and each comment with its author, place and quoted body", () => {
    const text = formatReviewFeedback(feedbackItems(detail()), pr);
    expect(text).toBe(
      [
        "Address this review feedback on PR #42 (Fix the thing):",
        "https://github.com/o/r/pull/42 at commit abcdef1",
        "",
        "- **rev** on `src/a.rs:10-12` https://github.com/o/r/pull/42#discussion_r7:",
        "  > Rename this.",
        "",
        "- **a deleted user** on `src/b.rs:3` (outdated: the code has changed since, reply):",
        "  > Old",
        "  >",
        "  > note",
        "",
        "- **rev** requested changes:",
        "  > Needs tests.",
        "",
      ].join("\n"),
    );
  });

  test("nothing picked is no draft", () => {
    expect(formatReviewFeedback([], pr)).toBe("");
  });

  test("is added after what the composer already holds", () => {
    expect(appendToDraft("", "B\n")).toBe("B\n");
    expect(appendToDraft("  \n", "B\n")).toBe("B\n");
    expect(appendToDraft("Also fix lint.\n", "B\n")).toBe("Also fix lint.\n\nB\n");
  });
});
