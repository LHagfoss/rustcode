import { describe, expect, test } from "bun:test";
import { RpcError } from "../acp/jsonrpc";
import {
  acceptsGeneratedDraft,
  draftCommitMessage,
  draftPull,
  type GitState,
  gitErrorOf,
  gitReadFor,
  gitReads,
  parseGitState,
  planGitActions,
  pullFromUrl,
  pullPresence,
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
    expect(planGitActions({ ...base, files: dirty }, "none")).toEqual({
      primary: "commit",
      others: ["commit_push"],
    });
  });

  test("a repository with no remote only commits", () => {
    expect(planGitActions({ ...base, hasRemote: false, files: dirty }, "none")).toEqual({
      primary: "commit",
      others: [],
    });
    expect(
      planGitActions({ ...base, hasRemote: false, upstream: undefined }, "none").primary,
    ).toBeUndefined();
  });

  test("commits ahead push, and offer a pull request off the default branch", () => {
    expect(planGitActions({ ...base, ahead: 2 }, "none")).toEqual({ primary: "push", others: ["create_pr"] });
    expect(planGitActions({ ...base, ahead: 2, branch: "main", onDefault: true }, "none")).toEqual({
      primary: "push",
      others: [],
    });
  });

  test("a branch never pushed pushes", () => {
    expect(planGitActions({ ...base, upstream: undefined }, "none").primary).toBe("push");
  });

  test("a pushed branch without a pull request opens one", () => {
    expect(planGitActions(base, "none")).toEqual({ primary: "create_pr", others: [] });
    expect(planGitActions(base, "open")).toEqual({ others: [], note: "Pushed, pull request open" });
  });

  test("behind pulls, and diverged says so instead", () => {
    expect(planGitActions({ ...base, behind: 3 }, "none").primary).toBe("pull");
    const diverged = planGitActions({ ...base, behind: 3, ahead: 1 }, "none");
    expect(diverged.primary).toBeUndefined();
    expect(diverged.note).toContain("Diverged");
  });

  test("conflicts and a detached HEAD offer nothing", () => {
    expect(planGitActions({ ...base, files: [{ path: "a", status: "U" }] }, "none").primary).toBeUndefined();
    expect(planGitActions({ ...base, branch: undefined }, "none").note).toBe("HEAD is not on a branch");
  });

  test("the default branch, up to date, has nothing to do", () => {
    expect(
      planGitActions({ ...base, branch: "main", onDefault: true, upstream: "origin/main" }, "none"),
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

describe("the open pull request (issue 3744)", () => {
  test("a pull list not known yet never offers a pull request", () => {
    expect(planGitActions(base, "unknown")).toEqual({ others: [], note: "Up to date" });
    expect(planGitActions({ ...base, ahead: 2 }, "unknown")).toEqual({ primary: "push", others: [] });
    expect(planGitActions({ ...base, upstream: undefined }, "unknown").others).toEqual([]);
  });

  test("the presence follows the list for the branch", () => {
    const list = [{ headRef: "feat/x", number: 4 }];
    expect(pullPresence("feat/x", undefined)).toEqual({ presence: "unknown" });
    expect(pullPresence(undefined, list)).toEqual({ presence: "unknown" });
    expect(pullPresence("feat/x", list)).toEqual({ presence: "open", pull: list[0] });
    expect(pullPresence("feat/y", list)).toEqual({ presence: "none" });
    expect(pullPresence("feat/y", [])).toEqual({ presence: "none" });
  });
});

describe("the header's git state across threads (issue 3744)", () => {
  test("another folder's answer is never shown", () => {
    const state = parseGitState({ branch: "a" });
    expect(gitReadFor({ cwd: "/a", state }, "/b")).toEqual({});
    expect(gitReadFor({ cwd: "/a", state }, "/a")).toEqual({ cwd: "/a", state });
    expect(gitReadFor({}, undefined)).toEqual({});
  });

  test("an older read that answers late is dropped", () => {
    const reads = gitReads();
    const first = reads.begin("/a");
    const second = reads.begin("/b");
    expect(first({ state: parseGitState({ branch: "a" }) })).toBeUndefined();
    expect(second({ error: "not a repository" })).toEqual({ cwd: "/b", error: "not a repository" });
    const third = reads.begin("/b");
    expect(second({ error: "late" })).toBeUndefined();
    expect(third({ error: "x" })?.cwd).toBe("/b");
  });
});
