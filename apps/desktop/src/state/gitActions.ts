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
}

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
      return path ? [{ path, status: str(file?.status) ?? "M" }] : [];
    }),
  };
}

/** What the header's git button offers: its primary action, the rest, or why it can't act. */
export interface GitPlan {
  primary?: GitAction;
  others: GitAction[];
  /** Why there is nothing to do, shown in place of an action. */
  note?: string;
}

/**
 * The action the repository's state calls for, as T3 Code's git control picks it: changed files
 * commit first, a branch behind its upstream pulls, commits not on the remote push, and a pushed
 * branch with no pull request opens one. The default branch never offers a pull request.
 */
export function planGitActions(state: GitState, hasOpenPull: boolean): GitPlan {
  if (!state.branch) return { others: [], note: "HEAD is not on a branch" };
  if (state.files.some((f) => f.status === "U"))
    return { others: [], note: "Resolve the merge conflicts first" };
  const canPr = !state.onDefault && state.hasRemote && !hasOpenPull;
  if (state.files.length > 0) return { primary: "commit", others: state.hasRemote ? ["commit_push"] : [] };
  if (!state.hasRemote) return { others: [], note: "Nothing to commit" };
  if (state.behind > 0)
    return state.ahead > 0
      ? { others: [], note: `Diverged from ${state.upstream ?? "upstream"}: merge it in the terminal` }
      : { primary: "pull", others: [] };
  // A branch never pushed has no upstream; one with commits ahead has work to send.
  if (!state.upstream || state.ahead > 0) return { primary: "push", others: canPr ? ["create_pr"] : [] };
  if (canPr) return { primary: "create_pr", others: [] };
  return { others: [], note: hasOpenPull ? "Pushed, pull request open" : "Up to date" };
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
