import { describe, expect, test } from "bun:test";
import type { Json } from "../acp/jsonrpc";
import type { PullDetail } from "../acp/rustcode";
import {
  DEFAULT_PULL_INSTRUCTION,
  linkedPullThread,
  type PreparedPullBranch,
  parsePullBranch,
  pullThreadMessage,
} from "./pullThread";

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

  const parts = { body: false, comments: false, checks: false };
  const fork = (over: Partial<PreparedPullBranch>): PreparedPullBranch => ({
    branch: "fix/login",
    headRef: "fix/login",
    crossRepository: true,
    readOnly: false,
    ...over,
  });

  test("names the branch the engine prepared (issue 3743)", () => {
    const same = pullThreadMessage(pull, detail, parts, "", fork({}));
    expect(same).toContain(
      "on the pull request's branch `fix/login` (into `main`). Pushing updates the pull request.",
    );

    const renamed = pullThreadMessage(
      pull,
      detail,
      parts,
      "",
      fork({ branch: "alice-fix/login", pushUrl: "https://github.com/alice/r.git" }),
    );
    expect(renamed).toContain(
      "on the local branch `alice-fix/login`, made from the pull request's head `fix/login`",
    );
    expect(renamed).toContain("`git push https://github.com/alice/r.git HEAD:fix/login`");
    expect(renamed).not.toContain("Pushing updates the pull request.");
  });

  test("a fork without maintainer edits does not claim a push updates the pull request", () => {
    const text = pullThreadMessage(
      pull,
      detail,
      parts,
      "",
      fork({ branch: "alice-fix/login", readOnly: true }),
    );
    expect(text).toContain("commits stay on this branch: pushing does not update the pull request.");
    expect(text).not.toContain("Pushing updates the pull request.");
    expect(text).not.toContain("git push");
  });

  test("says when the branch was not brought up to the head", () => {
    const text = pullThreadMessage(
      pull,
      detail,
      parts,
      "",
      fork({ stale: "The local branch fix/login has commits." }),
    );
    expect(text).toContain("The local branch fix/login has commits.");
  });
});

describe("parsePullBranch", () => {
  test("reads the engine's answer", () => {
    expect(
      parsePullBranch({
        branch: "alice-fix",
        head_ref: "fix",
        cross_repository: true,
        read_only: false,
        push_url: "https://github.com/alice/r.git",
        worktree: "/wt/alice-fix",
      }),
    ).toEqual({
      branch: "alice-fix",
      headRef: "fix",
      crossRepository: true,
      readOnly: false,
      note: undefined,
      pushUrl: "https://github.com/alice/r.git",
      stale: undefined,
      worktree: "/wt/alice-fix",
    });
    expect(parsePullBranch({ head_ref: "fix" })).toBeUndefined();
    expect(parsePullBranch(undefined)).toBeUndefined();
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
