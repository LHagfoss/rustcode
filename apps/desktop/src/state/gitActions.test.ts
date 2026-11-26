import { describe, expect, test } from "bun:test";
import { RpcError } from "../acp/jsonrpc";
import {
  acceptsGeneratedDraft,
  commitSummaryLabel,
  commitsStagedIndex,
  draftCommitMessage,
  draftPull,
  type GitFile,
  type GitState,
  gitActionDisplayLabel,
  gitErrorOf,
  gitReadFor,
  gitReads,
  initialCommitSelection,
  parseGitState,
  partitionByStaging,
  planGitActions,
  pullFromUrl,
  pullPresence,
  stageKindOf,
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
const file = (path: string, status = "M", staged = false, unstaged = !staged): GitFile => ({
  path,
  status,
  staged,
  unstaged,
});
const dirty = [file("a.ts")];

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
    expect(planGitActions({ ...base, files: [file("a", "U")] }, "none").primary).toBeUndefined();
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
    const files = ["src/a.ts", "src/b.ts", "c.ts", "d.ts"].map((path) => file(path));
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
      files: [
        { path: "a.ts", status: "?", staged: false, unstaged: true },
        { path: "b.ts", status: "M", staged: true, unstaged: true },
        { path: "c.ts", status: "A", staged: true },
        { status: "M" },
      ],
    }),
  ).toEqual({
    branch: "feat/x",
    defaultBranch: "main",
    onDefault: false,
    upstream: undefined,
    ahead: 1,
    behind: 0,
    hasRemote: true,
    files: [
      { path: "a.ts", status: "?", staged: false, unstaged: true },
      { path: "b.ts", status: "M", staged: true, unstaged: true },
      { path: "c.ts", status: "A", staged: true, unstaged: false },
    ],
  });
});

describe("staging state and commit selection", () => {
  const staged = file("staged.ts", "M", true, false);
  const partial = file("partial.ts", "M", true, true);
  const unstaged = file("unstaged.ts", "?", false, true);

  test("stageKindOf distinguishes staged, partial, and unstaged files", () => {
    expect(stageKindOf(staged)).toBe("staged");
    expect(stageKindOf(partial)).toBe("partial");
    expect(stageKindOf(unstaged)).toBe("unstaged");
    expect(stageKindOf(undefined)).toBe("unstaged");
  });

  test("partitionByStaging groups staged and partial files separately from unstaged", () => {
    const rows = [
      { path: "unstaged.ts", added: 3 },
      { path: "staged.ts", added: 1 },
      { path: "partial.ts", added: 2 },
    ];
    expect(partitionByStaging(rows, [staged, partial, unstaged])).toEqual({
      staged: [
        { path: "staged.ts", added: 1 },
        { path: "partial.ts", added: 2 },
      ],
      unstaged: [{ path: "unstaged.ts", added: 3 }],
      hasPartial: true,
    });
    expect(partitionByStaging(rows, undefined)).toEqual({
      staged: [],
      unstaged: rows,
      hasPartial: false,
    });
  });

  test("initialCommitSelection picks staged files when any are staged, else all files", () => {
    expect([...initialCommitSelection([staged, partial, unstaged])]).toEqual(["staged.ts", "partial.ts"]);
    expect([...initialCommitSelection([unstaged, file("other.ts")])]).toEqual(["unstaged.ts", "other.ts"]);
  });

  test("commitsStagedIndex is true only when selection matches the staged set exactly", () => {
    const files = [staged, partial, unstaged];
    expect(commitsStagedIndex(new Set(["staged.ts", "partial.ts"]), files)).toBe(true);
    expect(commitsStagedIndex(new Set(["staged.ts"]), files)).toBe(false);
    expect(commitsStagedIndex(new Set(["staged.ts", "partial.ts", "unstaged.ts"]), files)).toBe(false);
    expect(commitsStagedIndex(new Set(["unstaged.ts"]), [unstaged])).toBe(false);
  });

  test("commitSummaryLabel and gitActionDisplayLabel reflect staged counts", () => {
    expect(commitSummaryLabel(1, 3, 1, true)).toBe("1 staged file (of 3)");
    expect(commitSummaryLabel(2, 2, 2, true)).toBe("2 staged files");
    expect(commitSummaryLabel(2, 3, 1, false)).toBe("2 of 3 files");
    expect(gitActionDisplayLabel("commit", 0)).toBe("Commit");
    expect(gitActionDisplayLabel("commit", 2)).toBe("Commit (2 staged)");
    expect(gitActionDisplayLabel("commit_push", 1)).toBe("Commit & Push (1 staged)");
    expect(gitActionDisplayLabel("push", 2)).toBe("Push");
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
