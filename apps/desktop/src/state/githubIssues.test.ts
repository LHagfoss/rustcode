import { describe, expect, test } from "bun:test";
import {
  collectIssueLabels,
  DEFAULT_ISSUE_INSTRUCTION,
  filterIssues,
  type GitHubIssue,
  issueBranchSlug,
  issueErrorTitle,
  issuesAvailableFor,
  issuesCwd,
  issueThreadMessage,
  issueThreadTitle,
  parseIssue,
  parseIssueList,
} from "./githubIssues";

const SAMPLE_ISSUES: GitHubIssue[] = [
  {
    number: 4475,
    title: "Browse GitHub issues and fan out worktree threads",
    author: "LHagfoss",
    state: "open",
    labels: [{ name: "desktop", color: "1d76db" }, { name: "enhancement" }],
    assignees: ["alice", "bob"],
    body: "Add an issues panel with multi-select worktree fan-out.",
    updatedAt: Date.parse("2026-11-26T09:00:00Z"),
    url: "https://github.com/LHagfoss/rustcode/issues/4475",
  },
  {
    number: 4476,
    title: "Detect listening ports per thread",
    author: "charlie",
    state: "open",
    labels: [{ name: "engine" }],
    assignees: [],
    body: "Surface localhost ports and stop actions.",
    updatedAt: Date.parse("2026-11-25T10:00:00Z"),
    url: "https://github.com/LHagfoss/rustcode/issues/4476",
  },
];

describe("github issue parsing", () => {
  test("parses full issue payload and skips malformed rows", () => {
    const parsed = parseIssueList({
      list: {
        provider: "github",
        hasMore: true,
        issues: [
          {
            number: 4475,
            title: "Browse GitHub issues",
            author: "LHagfoss",
            state: "open",
            labels: [{ name: "desktop", color: "1d76db" }, { name: "  " }, "triage"],
            assignees: ["alice", { login: "bob" }],
            body: "Issue description",
            updatedAt: "2026-11-26T09:00:00Z",
            url: "https://github.com/LHagfoss/rustcode/issues/4475",
          },
          {
            number: 4474,
            title: "Closed issue",
            state: "closed",
          },
          {
            title: "Missing number",
          },
        ],
      },
    });

    expect(parsed.hasMore).toBe(true);
    expect(parsed.issues).toHaveLength(2);
    expect(parsed.issues[0]).toEqual({
      number: 4475,
      title: "Browse GitHub issues",
      author: "LHagfoss",
      state: "open",
      labels: [{ name: "desktop", color: "1d76db" }, { name: "triage" }],
      assignees: ["alice", "bob"],
      body: "Issue description",
      updatedAt: Date.parse("2026-11-26T09:00:00Z"),
      url: "https://github.com/LHagfoss/rustcode/issues/4475",
    });
    expect(parsed.issues[1]).toEqual({
      number: 4474,
      title: "Closed issue",
      author: "",
      state: "closed",
      labels: [],
      assignees: [],
      body: "",
      updatedAt: undefined,
      url: undefined,
    });
  });

  test("returns null for non-object or missing number", () => {
    expect(parseIssue(undefined)).toBeNull();
    expect(parseIssue("bad")).toBeNull();
    expect(parseIssue({ title: "No number" })).toBeNull();
  });

  test("maps error codes to issue-specific and shared titles", () => {
    expect(issueErrorTitle("gh_missing")).toBe("The GitHub CLI is not installed");
    expect(issueErrorTitle("not_connected")).toBe("GitHub is not connected");
    expect(issueErrorTitle("no_provider")).toBe("No issue provider for this remote");
    expect(issueErrorTitle("failed")).toBe("Issues could not be read");
  });
});

describe("github issue filtering and labels", () => {
  test("collects sorted unique labels", () => {
    expect(collectIssueLabels(SAMPLE_ISSUES)).toEqual(["desktop", "engine", "enhancement"]);
  });

  test("filters by number, title, body, author, assignee, and label", () => {
    expect(filterIssues(SAMPLE_ISSUES, "#4475", null).map((i) => i.number)).toEqual([4475]);
    expect(filterIssues(SAMPLE_ISSUES, "localhost", null).map((i) => i.number)).toEqual([4476]);
    expect(filterIssues(SAMPLE_ISSUES, "alice", null).map((i) => i.number)).toEqual([4475]);
    expect(filterIssues(SAMPLE_ISSUES, "charlie", null).map((i) => i.number)).toEqual([4476]);
    expect(filterIssues(SAMPLE_ISSUES, "", "desktop").map((i) => i.number)).toEqual([4475]);
    expect(filterIssues(SAMPLE_ISSUES, "ports", "desktop")).toEqual([]);
  });
});

describe("github issue prompt seeding and worktree naming", () => {
  test("builds a clean git branch slug and capped title", () => {
    expect(issueBranchSlug({ number: 4475, title: "feat(desktop): Browse GitHub issues & fan-out!" })).toBe(
      "issue-4475-feat-desktop-browse-github-issues-fa",
    );
    expect(issueBranchSlug({ number: 7, title: "!!!" })).toBe("issue-7");
    expect(issueThreadTitle({ number: 12, title: "A".repeat(120) })).toHaveLength(80);
  });

  test("seeds prompt with instruction, issue header, metadata, and body", () => {
    const prompt = issueThreadMessage(SAMPLE_ISSUES[0]!);
    expect(prompt).toContain(DEFAULT_ISSUE_INSTRUCTION);
    expect(prompt).toContain(
      "Issue #4475: Browse GitHub issues and fan out worktree threads (https://github.com/LHagfoss/rustcode/issues/4475)",
    );
    expect(prompt).toContain("Labels: desktop, enhancement · Assignees: alice, bob");
    expect(prompt).toContain("## Description\n\nAdd an issues panel with multi-select worktree fan-out.");
  });

  test("uses custom instruction and omits empty description and metadata", () => {
    const minimal: GitHubIssue = {
      number: 9,
      title: "Fix crash",
      author: "dev",
      state: "open",
      labels: [],
      assignees: [],
      body: "   ",
    };
    const prompt = issueThreadMessage(minimal, "Write a minimal regression test first.");
    expect(prompt).toBe("Write a minimal regression test first.\n\nIssue #9: Fix crash");
  });
});

describe("remote threads", () => {
  test("lists issues only for threads on this computer", () => {
    expect(issuesAvailableFor(undefined)).toBe(true);
    expect(issuesAvailableFor("thread-1")).toBe(true);
    expect(issuesAvailableFor("studio/thread-1")).toBe(false);
    expect(issuesCwd("thread-1", "/repo")).toBe("/repo");
    expect(issuesCwd(null, "/repo")).toBe("/repo");
    expect(issuesCwd("thread-1", "")).toBeUndefined();
    expect(issuesCwd("studio/thread-1", "/repo")).toBeUndefined();
  });
});
