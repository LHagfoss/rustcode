import { describe, expect, test } from "bun:test";
import type { Json } from "../acp/jsonrpc";
import type { PullDetail } from "../acp/rustcode";
import { DEFAULT_PULL_INSTRUCTION, linkedPullThread, pullThreadMessage } from "./pullThread";

const pull = {
  number: 7,
  title: "Fix login",
  author: "a",
  state: "open",
  draft: false,
  checks: { passed: 1, failed: 1, pending: 0 },
  headRef: "fix/login",
  baseRef: "main",
  headSha: "abc",
  url: "https://github.com/o/r/pull/7",
  additions: 1,
  deletions: 1,
  changedFiles: 1,
} as unknown as PullDetail;
const detail: PullDetail = {
  ...pull,
  body: "Fixes the redirect.",
  checkRuns: [
    { name: "test", status: "failed", url: "https://ci/1" },
    { name: "lint", status: "passed" },
  ],
  files: [],
  activity: [],
  reviewComments: [
    { id: 1, author: "bob", path: "src/a.ts", line: 3, outdated: false, body: "Rename this" },
    { id: 2, author: "bob", path: "src/b.ts", outdated: true, body: "Old" },
  ],
};

describe("pullThreadMessage", () => {
  test("carries the ticked parts only", () => {
    const all = pullThreadMessage(pull, detail, { body: true, comments: true, checks: true }, "");
    expect(all.startsWith(DEFAULT_PULL_INSTRUCTION)).toBe(true);
    expect(all).toContain("Pull request #7: Fix login (https://github.com/o/r/pull/7)");
    expect(all).toContain("`fix/login`");
    expect(all).toContain("Fixes the redirect.");
    expect(all).toContain("- `src/a.ts:3` (bob): Rename this");
    expect(all).not.toContain("Old");
    expect(all).toContain("- test (https://ci/1)");
    expect(all).not.toContain("lint");

    const none = pullThreadMessage(
      pull,
      detail,
      { body: false, comments: false, checks: false },
      "Try it locally.",
    );
    expect(none.startsWith("Try it locally.")).toBe(true);
    expect(none).not.toContain("## ");
  });
});

describe("linkedPullThread", () => {
  const watches = {
    watches: [
      { sessionId: "s0", number: 7, link: false },
      { sessionId: "s1", number: 7, link: true, repo: "other/r" },
      { sessionId: "s2", number: 7, link: true, repo: "O/R" },
    ],
  } as unknown as Json;
  test("finds the thread linked to the same pull request", () => {
    expect(linkedPullThread(watches, 7, "o/r")).toBe("s2");
    expect(linkedPullThread(watches, 8, "o/r")).toBeUndefined();
    expect(linkedPullThread({ watches: [{ sessionId: "s3", number: 7, link: true }] }, 7, "o/r")).toBe("s3");
  });
});
