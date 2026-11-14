import { describe, expect, test } from "bun:test";
import { RpcError } from "../acp/jsonrpc";
import {
  acceptsGeneratedDraft,
  draftCommitMessage,
  draftPull,
  type GitState,
  gitErrorOf,
  parseGitState,
  planGitActions,
  pullFromUrl,
} from "./gitActions";

const base: GitState = {
  branch: "feat/x",
  defaultBranch: "main",
  onDefault: false,
  upstream: "origin/feat/x",
  ahead: 0,
  behind: 0,
  hasRemote: true,
  files: [],
};
const dirty = [{ path: "a.ts", status: "M" }];

describe("planGitActions", () => {
  test("changed files commit first, with commit and push beside it", () => {
    expect(planGitActions({ ...base, files: dirty }, false)).toEqual({
      primary: "commit",
      others: ["commit_push"],
    });
  });

  test("a repository with no remote only commits", () => {
    expect(planGitActions({ ...base, hasRemote: false, files: dirty }, false)).toEqual({
      primary: "commit",
      others: [],
    });
    expect(planGitActions({ ...base, hasRemote: false, upstream: undefined }, false).primary).toBeUndefined();
  });

  test("commits ahead push, and offer a pull request off the default branch", () => {
    expect(planGitActions({ ...base, ahead: 2 }, false)).toEqual({ primary: "push", others: ["create_pr"] });
    expect(planGitActions({ ...base, ahead: 2, branch: "main", onDefault: true }, false)).toEqual({
      primary: "push",
      others: [],
    });
  });

  test("a branch never pushed pushes", () => {
    expect(planGitActions({ ...base, upstream: undefined }, false).primary).toBe("push");
  });

  test("a pushed branch without a pull request opens one", () => {
    expect(planGitActions(base, false)).toEqual({ primary: "create_pr", others: [] });
    expect(planGitActions(base, true)).toEqual({ others: [], note: "Pushed, pull request open" });
  });

  test("behind pulls, and diverged says so instead", () => {
    expect(planGitActions({ ...base, behind: 3 }, false).primary).toBe("pull");
    const diverged = planGitActions({ ...base, behind: 3, ahead: 1 }, false);
    expect(diverged.primary).toBeUndefined();
    expect(diverged.note).toContain("Diverged");
  });

  test("conflicts and a detached HEAD offer nothing", () => {
    expect(planGitActions({ ...base, files: [{ path: "a", status: "U" }] }, false).primary).toBeUndefined();
    expect(planGitActions({ ...base, branch: undefined }, false).note).toBe("HEAD is not on a branch");
  });

  test("the default branch, up to date, has nothing to do", () => {
    expect(
      planGitActions({ ...base, branch: "main", onDefault: true, upstream: "origin/main" }, false),
    ).toEqual({
      others: [],
      note: "Up to date",
    });
  });
});

describe("drafts", () => {
  test("the thread's title is the commit message", () => {
    expect(draftCommitMessage("Fix the login redirect", dirty)).toBe("Fix the login redirect");
  });

  test("an untitled thread names the files", () => {
    expect(draftCommitMessage("New thread", dirty)).toBe("Update a.ts");
    const files = ["src/a.ts", "src/b.ts", "c.ts", "d.ts"].map((path) => ({ path, status: "M" }));
    expect(draftCommitMessage(undefined, files)).toBe("Update a.ts, b.ts, c.ts and 1 more");
  });

  test("a pull request takes the commit's subject and body", () => {
    expect(draftPull("Fix it\n\nBecause it broke.")).toEqual({ title: "Fix it", body: "Because it broke." });
  });

  test("a pull request URL names its repository and number", () => {
    expect(pullFromUrl("https://github.com/o/r/pull/12")).toEqual({ repo: "o/r", number: 12 });
    expect(pullFromUrl("nope")).toBeUndefined();
  });
});

test("parseGitState reads the engine's answer", () => {
  expect(
    parseGitState({
      branch: "feat/x",
      defaultBranch: "main",
      onDefault: false,
      upstream: null,
      ahead: 1,
      behind: 0,
      hasRemote: true,
      files: [{ path: "a.ts", status: "?" }, { status: "M" }],
    }),
  ).toEqual({
    branch: "feat/x",
    defaultBranch: "main",
    onDefault: false,
    upstream: undefined,
    ahead: 1,
    behind: 0,
    hasRemote: true,
    files: [{ path: "a.ts", status: "?" }],
  });
});

test("gitErrorOf reads the engine's reason", () => {
  expect(
    gitErrorOf(new RpcError(-32602, "x", { reason: "default_branch", message: "This commits to main." })),
  ).toEqual({
    reason: "default_branch",
    message: "This commits to main.",
  });
  expect(gitErrorOf(new Error("boom"))).toEqual({ reason: "failed", message: "boom" });
});

test("generated drafts never replace user input or a closed sheet", () => {
  expect(acceptsGeneratedDraft(false, false, "Generated")).toBe(true);
  expect(acceptsGeneratedDraft(true, false, "Generated")).toBe(false);
  expect(acceptsGeneratedDraft(false, true, "Generated")).toBe(false);
  expect(acceptsGeneratedDraft(false, false, undefined)).toBe(false);
});
