import { arr, type JsonObject, num, str } from "./jsonrpc";

type MockIssue = {
  number: number;
  title: string;
  author: string;
  state: "open" | "closed";
  labels: { name: string; color: string }[];
  assignees: string[];
  body: string;
  updatedAt: string;
  url: string;
};

const MOCK_ISSUES: readonly MockIssue[] = [
  {
    number: 4475,
    title: "Browse GitHub issues and fan out worktree threads",
    author: "LHagfoss",
    state: "open",
    labels: [
      { name: "desktop", color: "1d76db" },
      { name: "enhancement", color: "a2eeef" },
    ],
    assignees: ["LHagfoss"],
    body: "## Problem\nThere is no way to browse open GitHub issues for a project and turn one or several issues directly into parallel worktree threads.\n\n## Acceptance criteria\n- List open issues via `gh`.\n- Support single-issue and multi-select worktree fan-out.",
    updatedAt: "2026-11-26T08:40:00Z",
    url: "https://github.com/LHagfoss/rustcode/issues/4475",
  },
  {
    number: 4476,
    title: "Detect listening ports per thread and surface open/kill actions",
    author: "LHagfoss",
    state: "open",
    labels: [
      { name: "engine", color: "5319e7" },
      { name: "desktop", color: "1d76db" },
    ],
    assignees: [],
    body: "Detect localhost ports started inside each thread's worktree and expose quick Open in Browser and Stop actions.",
    updatedAt: "2026-11-25T19:15:00Z",
    url: "https://github.com/LHagfoss/rustcode/issues/4476",
  },
  {
    number: 4477,
    title: "Support opening multiple app windows",
    author: "alex",
    state: "open",
    labels: [{ name: "desktop", color: "1d76db" }],
    assignees: ["alex"],
    body: "Allow opening a second RustCode window with Cmd/Ctrl+Shift+N so two projects can sit side by side.",
    updatedAt: "2026-11-24T14:20:00Z",
    url: "https://github.com/LHagfoss/rustcode/issues/4477",
  },
  {
    number: 4471,
    title: "Jump between review comments and open diff lines in editor",
    author: "LHagfoss",
    state: "closed",
    labels: [{ name: "desktop", color: "1d76db" }],
    assignees: ["LHagfoss"],
    body: "Completed in a merged pull request.",
    updatedAt: "2026-11-23T11:05:00Z",
    url: "https://github.com/LHagfoss/rustcode/issues/4471",
  },
];

/** Filter mock GitHub issues matching `_rustcode/issues` parameters. */
export function mockIssuesList(params: JsonObject): JsonObject {
  const state = str(params.state) ?? "open";
  const search = (str(params.search) ?? "").trim().toLowerCase();
  const labels = arr(params.labels)
    .flatMap((v) => (typeof v === "string" ? [v.trim().toLowerCase()] : []))
    .filter(Boolean);
  const limit = Math.max(1, Math.min(100, num(params.limit) ?? 50));

  const filtered = MOCK_ISSUES.filter((issue) => {
    if (state !== "all" && issue.state !== state) return false;
    if (labels.length > 0) {
      const names = new Set(issue.labels.map((l) => l.name.toLowerCase()));
      if (!labels.every((l) => names.has(l))) return false;
    }
    if (search) {
      const hay = `${issue.number} ${issue.title} ${issue.body} ${issue.author}`.toLowerCase();
      if (!hay.includes(search)) return false;
    }
    return true;
  });

  return {
    list: {
      provider: "github",
      hasMore: filtered.length > limit,
      issues: filtered.slice(0, limit),
    },
  };
}
