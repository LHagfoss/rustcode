// Commit, push and open a pull request from the thread header (issue 3724). The engine runs git in the
// thread's folder (`_rustcode/git/state`, `_rustcode/git/run`, rustcode/engine/src/git_actions.rs);
// this module reads its answer and picks the one primary action the repository's state calls for.

import { arr, bool, type Json, num, obj, RpcError, str } from "../acp/jsonrpc";

export const GitMethod = {
  state: "_rustcode/git/state",
  run: "_rustcode/git/run",
} as const;

export interface GitFile {
  path: string;
  /** `M`, `A`, `D`, `R`, `?` (untracked) or `U` (conflicted). */
  status: string;
  /** Present in the git index (staged for commit). */
  staged: boolean;
  /** Has working-tree changes not yet staged. */
  unstaged: boolean;
}

export type StageKind = "staged" | "partial" | "unstaged";

export interface GitState {
  branch?: string;
  defaultBranch?: string;
  lastCommitMessage?: string;
  onDefault: boolean;
  upstream?: string;
  ahead: number;
  behind: number;
  hasRemote: boolean;
  files: GitFile[];
}

export type GitAction = "commit" | "commit_push" | "push" | "pull" | "create_pr";

export const GIT_ACTION_LABEL: Record<GitAction, string> = {
  commit: "Commit",
  commit_push: "Commit & Push",
  push: "Push",
  pull: "Pull",
  create_pr: "Create Pull Request",
};

/** Display label for a git action, naming the staged file count when committing staged changes. */
export function gitActionDisplayLabel(action: GitAction, stagedCount: number): string {
  const base = GIT_ACTION_LABEL[action];
  if (stagedCount > 0 && (action === "commit" || action === "commit_push")) {
    return `${base} (${stagedCount} staged)`;
  }
  return base;
}

export function parseGitState(value: Json | undefined): GitState | undefined {
  const o = obj(value);
  if (!o) return undefined;
  return {
    branch: str(o.branch),
    defaultBranch: str(o.defaultBranch),
    lastCommitMessage: str(o.lastCommitMessage),
    onDefault: bool(o.onDefault) ?? false,
    upstream: str(o.upstream),
    ahead: num(o.ahead) ?? 0,
    behind: num(o.behind) ?? 0,
    hasRemote: bool(o.hasRemote) ?? false,
    files: arr(o.files).flatMap((f) => {
      const file = obj(f);
      const path = str(file?.path);
      if (!path) return [];
      const staged = bool(file?.staged) ?? false;
      const unstaged = bool(file?.unstaged) ?? !staged;
      return [{ path, status: str(file?.status) ?? "M", staged, unstaged }];
    }),
  };
}

/** Whether a file is fully staged, partially staged (index and worktree), or unstaged. */
export function stageKindOf(file: Pick<GitFile, "staged" | "unstaged"> | undefined): StageKind {
  if (!file?.staged) return "unstaged";
  return file.unstaged ? "partial" : "staged";
}

export interface StagingPartition<T> {
  staged: T[];
  unstaged: T[];
  hasPartial: boolean;
}

/** Splits diff rows into staged (including partially staged) and unstaged groups using engine git state. */
export function partitionByStaging<T extends { path: string }>(
  files: readonly T[],
  gitFiles: readonly GitFile[] | undefined,
): StagingPartition<T> {
  if (!gitFiles?.length) return { staged: [], unstaged: [...files], hasPartial: false };
  const byPath = new Map(gitFiles.map((file) => [file.path, file]));
  const staged: T[] = [];
  const unstaged: T[] = [];
  let hasPartial = false;
  for (const file of files) {
    const status = byPath.get(file.path);
    if (status?.staged) {
      staged.push(file);
      if (status.unstaged) hasPartial = true;
    } else {
      unstaged.push(file);
    }
  }
  return { staged, unstaged, hasPartial };
}

/** Files pre-selected in the commit sheet: only staged files when any are staged, else every changed file. */
export function initialCommitSelection(files: readonly GitFile[]): ReadonlySet<string> {
  const staged = files.filter((file) => file.staged);
  return new Set((staged.length > 0 ? staged : files).map((file) => file.path));
}

/** True when the commit sheet selection matches the exact staged index set so git can commit the index directly. */
export function commitsStagedIndex(picked: ReadonlySet<string>, files: readonly GitFile[]): boolean {
  const staged = files.filter((file) => file.staged);
  if (staged.length === 0 || picked.size !== staged.length) return false;
  return staged.every((file) => picked.has(file.path));
}

/** Summary line in the commit sheet showing exactly what will be committed. */
export function commitSummaryLabel(
  pickedCount: number,
  totalCount: number,
  stagedCount: number,
  usingStagedIndex: boolean,
): string {
  const noun = (n: number) => (n === 1 ? "file" : "files");
  if (usingStagedIndex) {
    return stagedCount < totalCount
      ? `${stagedCount} staged ${noun(stagedCount)} (of ${totalCount})`
      : `${stagedCount} staged ${noun(stagedCount)}`;
  }
  return `${pickedCount} of ${totalCount} ${noun(totalCount)}`;
}

/** What the header's git button offers: its primary action, the rest, or why it can't act. */
export interface GitPlan {
  primary?: GitAction;
  others: GitAction[];
  /** Why there is nothing to do, shown in place of an action. */
  note?: string;
}

/**
 * Whether the branch has an open pull request, from the open pull list (issue 3744): `unknown`
 * while the list is still read or could not be (`gh` not signed in), so the button never offers a
 * second pull request on a guess.
 */
export type PullPresence = "open" | "none" | "unknown";

export function pullPresence<P extends { headRef: string }>(
  branch: string | undefined,
  list: readonly P[] | undefined,
): { presence: PullPresence; pull?: P } {
  if (!branch || !list) return { presence: "unknown" };
  const pull = list.find((p) => p.headRef === branch);
  return pull ? { presence: "open", pull } : { presence: "none" };
}

/**
 * The action the repository's state calls for, as T3 Code's git control picks it: changed files
 * commit first, a branch behind its upstream pulls, commits not on the remote push, and a pushed
 * branch with no pull request opens one. The default branch never offers a pull request, and
 * neither does a branch whose pull list is not known yet.
 */
export function planGitActions(state: GitState, pull: PullPresence): GitPlan {
  if (!state.branch) return { others: [], note: "HEAD is not on a branch" };
  if (state.files.some((f) => f.status === "U"))
    return { others: [], note: "Resolve the merge conflicts first" };
  const canPr = !state.onDefault && state.hasRemote && pull === "none";
  if (state.files.length > 0) return { primary: "commit", others: state.hasRemote ? ["commit_push"] : [] };
  if (!state.hasRemote) return { others: [], note: "Nothing to commit" };
  if (state.behind > 0)
    return state.ahead > 0
      ? { others: [], note: `Diverged from ${state.upstream ?? "upstream"}: merge it in the terminal` }
      : { primary: "pull", others: [] };
  // A branch never pushed has no upstream; one with commits ahead has work to send.
  if (!state.upstream || state.ahead > 0) return { primary: "push", others: canPr ? ["create_pr"] : [] };
  if (canPr) return { primary: "create_pr", others: [] };
  return { others: [], note: pull === "open" ? "Pushed, pull request open" : "Up to date" };
}

/**
 * A first draft of the commit message: the thread's title when it says what was done, else the
 * changed files. The user edits it in the sheet before anything is committed.
 */
export function draftCommitMessage(title: string | undefined, files: readonly GitFile[]): string {
  const subject = (title ?? "").trim().replace(/\s+/g, " ");
  if (subject && subject.toLowerCase() !== "new thread") return subject.slice(0, 72);
  if (files.length === 1) return `Update ${files[0]?.path ?? ""}`;
  const names = files.slice(0, 3).map((f) => f.path.split("/").pop() ?? f.path);
  const more = files.length > 3 ? ` and ${files.length - 3} more` : "";
  return files.length ? `Update ${names.join(", ")}${more}` : "";
}

/** The pull request's title and body from the commit that opens it. */
export function draftPull(message: string): { title: string; body: string } {
  const [first = "", ...rest] = message.trim().split("\n");
  return { title: first.trim(), body: rest.join("\n").trim() };
}

/** The owner/name and number of a GitHub pull request URL. */
export function pullFromUrl(url: string | undefined): { repo: string; number: number } | undefined {
  const match = /https:\/\/github\.com\/([\w.-]+\/[\w.-]+)\/pull\/(\d+)\b/.exec(url ?? "");
  const number = Number(match?.[2]);
  if (match && Number.isSafeInteger(number) && number > 0) return { repo: match[1] ?? "", number };
}

/** The reason and message a refused `_rustcode/git/run` carries. */
export function gitErrorOf(error: unknown): { reason: string; message: string } {
  const data = error instanceof RpcError ? obj(error.data as Json) : undefined;
  const message = str(data?.message) ?? (error instanceof Error ? error.message : String(error));
  return { reason: str(data?.reason) ?? "failed", message };
}

/** A request may finish after the user edits the draft, including editing back to its original value. */
export function acceptsGeneratedDraft(
  edited: boolean,
  cancelled: boolean,
  text: string | undefined,
): text is string {
  return !edited && !cancelled && !!text;
}

/** One answer to `_rustcode/git/state`, with the folder it was read for. */
export interface GitRead {
  cwd?: string;
  state?: GitState;
  error?: string;
}

/**
 * Orders the header's git state reads (issue 3744): `begin` hands each read a `land` that answers
 * the read to keep, or undefined when a newer read started since, so an older folder's answer never
 * lands after a newer one.
 */
export function gitReads() {
  let latest = 0;
  return {
    begin(cwd: string) {
      const token = ++latest;
      return (read: Omit<GitRead, "cwd">): GitRead | undefined =>
        token === latest ? { ...read, cwd } : undefined;
    },
  };
}

/** What the button shows for `cwd`: nothing (Reading git…) until an answer for that folder is in. */
export function gitReadFor(read: GitRead, cwd: string | undefined): GitRead {
  return cwd && read.cwd === cwd ? read : {};
}
