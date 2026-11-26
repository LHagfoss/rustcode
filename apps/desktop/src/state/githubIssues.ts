import { arr, bool, type Json, type JsonObject, num, obj, str } from "../acp/jsonrpc";
import { type PullError, parsePullError, pullErrorTitle } from "../acp/rustcode";
import { Method } from "../acp/types";
import { count } from "../lib/plural";
import { newThreadAccountOn } from "./logins";
import {
  createSession,
  errorText,
  harnessesOn,
  newThreadChoice,
  projectThreadChoice,
  request,
  runPrompt,
  setDraft,
  toast,
  useApp,
} from "./store";
import { isRemoteThread, LOCAL_HOST } from "./threadKeys";

export const ISSUE_LIMIT = 50;
export const DEFAULT_ISSUE_INSTRUCTION = "Investigate and implement this GitHub issue.";

export type IssueListState = "open" | "closed" | "all";

export interface IssueLabel {
  name: string;
  color?: string;
}

export interface GitHubIssue {
  number: number;
  title: string;
  author: string;
  state: "open" | "closed";
  labels: IssueLabel[];
  assignees: string[];
  body: string;
  updatedAt?: number;
  url?: string;
}

export interface IssueListResult {
  issues: GitHubIssue[];
  hasMore: boolean;
  error?: PullError;
}

export interface IssueLaunchOptions {
  instruction?: string;
  worktree?: boolean;
  harness?: string;
  model?: string | null;
  effort?: string | null;
  account?: string | null;
  select?: boolean;
  send?: boolean;
}

export interface IssueFanOutResult {
  created: string[];
  failed: { issue: GitHubIssue; error: string }[];
}

/** Parse one issue object from `_rustcode/issues`. */
export function parseIssue(value: Json | undefined): GitHubIssue | null {
  const o = obj(value);
  const number = num(o?.number);
  if (!o || number === undefined) return null;
  const updated = num(o.updatedAt) ?? Date.parse(str(o.updatedAt) ?? "");
  const labels = arr(o.labels).flatMap((entry) => {
    const label = obj(entry);
    const name = str(label?.name) ?? (typeof entry === "string" ? entry : undefined);
    if (!name?.trim()) return [];
    const color = str(label?.color)?.trim() || undefined;
    return [{ name: name.trim(), ...(color ? { color } : {}) }];
  });
  const assignees = arr(o.assignees).flatMap((entry) => {
    const login = str(obj(entry)?.login) ?? (typeof entry === "string" ? entry : undefined);
    return login?.trim() ? [login.trim()] : [];
  });
  return {
    number,
    title: str(o.title) ?? "",
    author: str(o.author) ?? "",
    state: str(o.state) === "closed" ? "closed" : "open",
    labels,
    assignees,
    body: str(o.body) ?? "",
    updatedAt: Number.isFinite(updated) ? updated : undefined,
    url: str(o.url),
  };
}

/** Parse the `_rustcode/issues` response payload. */
export function parseIssueList(result: Json | undefined): { issues: GitHubIssue[]; hasMore: boolean } {
  const list = obj(obj(result)?.list);
  return {
    issues: arr(list?.issues).flatMap((entry) => {
      const issue = parseIssue(entry);
      return issue ? [issue] : [];
    }),
    hasMore: bool(list?.hasMore) ?? false,
  };
}

/** Human-readable headline for an issues read error. */
export function issueErrorTitle(code: string): string {
  switch (code) {
    case "no_provider":
      return "No issue provider for this remote";
    case "gh_missing":
    case "not_connected":
    case "not_a_repository":
    case "folder_missing":
    case "timed_out":
      return pullErrorTitle(code);
    default:
      return "Issues could not be read";
  }
}

/** Sorted unique label names present across `issues`. */
export function collectIssueLabels(issues: readonly GitHubIssue[]): string[] {
  const names = new Set<string>();
  for (const issue of issues) {
    for (const label of issue.labels) names.add(label.name);
  }
  return [...names].sort((a, b) => a.localeCompare(b));
}

/** Instant client-side filter by search query and optional exact label name. */
export function filterIssues(
  issues: readonly GitHubIssue[],
  query: string,
  label: string | null,
): GitHubIssue[] {
  const tokens = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const wantedLabel = label?.trim().toLowerCase() || null;
  return issues.filter((issue) => {
    if (wantedLabel && !issue.labels.some((item) => item.name.toLowerCase() === wantedLabel)) {
      return false;
    }
    if (!tokens.length) return true;
    const haystack = [
      `#${issue.number}`,
      String(issue.number),
      issue.title,
      issue.body,
      issue.author,
      ...issue.assignees,
      ...issue.labels.map((item) => item.name),
    ]
      .join(" ")
      .toLowerCase();
    return tokens.every((token) => haystack.includes(token));
  });
}

/** Safe, readable git branch name for a worktree created from an issue. */
export function issueBranchSlug(issue: Pick<GitHubIssue, "number" | "title">): string {
  const slug = issue.title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 36)
    .replace(/-+$/g, "");
  return slug ? `issue-${issue.number}-${slug}` : `issue-${issue.number}`;
}

/** Compact thread title for an issue thread. */
export function issueThreadTitle(issue: Pick<GitHubIssue, "number" | "title">): string {
  return `#${issue.number} ${issue.title}`.trim().slice(0, 80);
}

/** Seeded prompt text carrying the issue's title, number, URL, metadata, and body. */
export function issueThreadMessage(issue: GitHubIssue, instruction?: string): string {
  const lines = [
    instruction?.trim() || DEFAULT_ISSUE_INSTRUCTION,
    "",
    `Issue #${issue.number}: ${issue.title}${issue.url ? ` (${issue.url})` : ""}`,
  ];
  const meta: string[] = [];
  if (issue.labels.length) meta.push(`Labels: ${issue.labels.map((label) => label.name).join(", ")}`);
  if (issue.assignees.length) meta.push(`Assignees: ${issue.assignees.join(", ")}`);
  if (meta.length) lines.push(meta.join(" · "));
  const body = issue.body.trim();
  if (body) lines.push("", "## Description", "", body);
  return lines.join("\n");
}

/** Read GitHub issues for `cwd` through `_rustcode/issues`. */
export async function fetchIssues(
  cwd: string,
  options: { state?: IssueListState; search?: string; labels?: string[]; limit?: number } = {},
): Promise<IssueListResult> {
  const params: JsonObject = {
    cwd,
    state: options.state ?? "open",
    limit: options.limit ?? ISSUE_LIMIT,
  };
  const search = options.search?.trim();
  if (search) params.search = search;
  const labels = (options.labels ?? []).map((label) => label.trim()).filter(Boolean);
  if (labels.length) params.labels = labels;
  try {
    const answer = await request(Method.issues, params, 30_000);
    const error = parsePullError(answer);
    if (error) return { issues: [], hasMore: false, error };
    return parseIssueList(answer);
  } catch (error) {
    return {
      issues: [],
      hasMore: false,
      error: { code: "failed", message: errorText(error) },
    };
  }
}

function resolveLaunchChoice(cwd: string, options: IssueLaunchOptions) {
  const s = useApp.getState();
  const fallback =
    projectThreadChoice(s, cwd) ??
    newThreadChoice({
      newThread: s.newThread,
      harnesses: s.harnesses,
      remoteHarnesses: s.remoteHarnesses,
      newThreadHost: LOCAL_HOST,
      projectCwd: cwd,
    });
  const harnessId = options.harness ?? fallback?.harness;
  const harness = harnessesOn(s, LOCAL_HOST)?.harnesses.find((item) => item.id === harnessId);
  const model = options.model !== undefined ? options.model : (fallback?.model ?? null);
  const effort = options.effort !== undefined ? options.effort : (fallback?.effort ?? null);
  const account =
    options.account !== undefined
      ? options.account
      : newThreadAccountOn(harness, s.newThreadAccount, s.signedOutLogins);
  return { harness: harnessId, model, effort, account };
}

/** Create one thread (in a fresh worktree by default) seeded with `issue`. */
export async function startIssueThread(
  cwd: string,
  issue: GitHubIssue,
  options: IssueLaunchOptions = {},
): Promise<string | undefined> {
  const choice = resolveLaunchChoice(cwd, options);
  const useWorktree = options.worktree ?? true;
  const prompt = issueThreadMessage(issue, options.instruction);
  try {
    const id = await createSession({
      cwd,
      harness: choice.harness,
      account: choice.account,
      model: choice.model,
      effort: choice.effort,
      ...(useWorktree
        ? {
            worktree: {
              repo: cwd,
              branch: issueBranchSlug(issue),
              title: issue.title,
            },
          }
        : {}),
      select: options.select ?? true,
      title: issueThreadTitle(issue),
    });
    if (options.send) void runPrompt(id, prompt);
    else setDraft(id, prompt);
    return id;
  } catch (error) {
    toast(`Could not start thread for #${issue.number}: ${errorText(error)}`, "error");
    return undefined;
  }
}

/** Fan out one parallel worktree thread per selected issue using the chosen harness and model. */
export async function fanOutIssueThreads(
  cwd: string,
  issues: readonly GitHubIssue[],
  options: IssueLaunchOptions = {},
): Promise<IssueFanOutResult> {
  const created: string[] = [];
  const failed: { issue: GitHubIssue; error: string }[] = [];
  const settled = await Promise.allSettled(
    issues.map(async (issue, index) => {
      const choice = resolveLaunchChoice(cwd, options);
      const prompt = issueThreadMessage(issue, options.instruction);
      const id = await createSession({
        cwd,
        harness: choice.harness,
        account: choice.account,
        model: choice.model,
        effort: choice.effort,
        worktree: {
          repo: cwd,
          branch: issueBranchSlug(issue),
          title: issue.title,
        },
        select: index === 0,
        title: issueThreadTitle(issue),
      });
      if (options.send) void runPrompt(id, prompt);
      else setDraft(id, prompt);
      return id;
    }),
  );
  settled.forEach((outcome, index) => {
    const issue = issues[index];
    if (!issue) return;
    if (outcome.status === "fulfilled") created.push(outcome.value);
    else failed.push({ issue, error: errorText(outcome.reason) });
  });
  if (created.length && failed.length) {
    toast(
      `Started ${count(created.length, "worktree thread")}; ${count(failed.length, "issue")} failed.`,
      "error",
    );
  } else if (created.length) {
    toast(
      options.send
        ? `Started ${count(created.length, "worktree thread")}`
        : `Prepared ${count(created.length, "worktree thread")} with drafted prompts`,
      "success",
    );
  } else if (failed.length) {
    toast(`Could not start worktree threads: ${failed[0]?.error ?? "unknown error"}`, "error");
  }
  return { created, failed };
}

/** Whether the Issues panel can list issues for `threadKey`: not for a thread on another computer, whose repos this machine cannot read. */
export function issuesAvailableFor(threadKey: string | null | undefined): boolean {
  return !threadKey || !isRemoteThread(threadKey);
}

/** The repository the Issues panel lists for the thread `threadKey`, if it can list any. */
export function issuesCwd(threadKey: string | null | undefined, cwd: string | undefined): string | undefined {
  return issuesAvailableFor(threadKey) ? cwd || undefined : undefined;
}
