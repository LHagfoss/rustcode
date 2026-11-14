// What a thread opened on a pull request starts with (issue 3727): the instruction, then only the
// parts the user ticked (the description, the review comments, the failing checks), as a message
// the user reads and edits in the composer before sending. Pure, so the sheet and tests share it.

import { arr, bool, type Json, num, obj, str } from "../acp/jsonrpc";
import type { PullDetail, PullRequest } from "../acp/rustcode";

export const DEFAULT_PULL_INSTRUCTION = "Address the review comments and fix failing checks.";

export interface PullThreadParts {
  body: boolean;
  comments: boolean;
  checks: boolean;
}

/** The review comments still on the diff: an outdated one was left on lines that are gone. */
export function openComments(detail: PullDetail | undefined): PullDetail["reviewComments"] {
  return (detail?.reviewComments ?? []).filter((c) => !c.outdated);
}

export function failingChecks(detail: PullDetail | undefined): PullDetail["checkRuns"] {
  return (detail?.checkRuns ?? []).filter((c) => c.status === "failed");
}

/**
 * The local branch `_rustcode/pulls/branch` made ready for the thread (issue 3743): its real name,
 * which differs from the head's when a fork's head name was taken, whether pushing can reach the
 * pull request, and the worktree that already has it checked out.
 */
export interface PreparedPullBranch {
  branch: string;
  headRef: string;
  crossRepository: boolean;
  readOnly: boolean;
  note?: string;
  pushUrl?: string;
  stale?: string;
  worktree?: string;
}

export function parsePullBranch(value: Json | undefined): PreparedPullBranch | undefined {
  const o = obj(value);
  const branch = str(o?.branch);
  if (!o || !branch) return undefined;
  return {
    branch,
    headRef: str(o.head_ref) ?? branch,
    crossRepository: bool(o.cross_repository) ?? false,
    readOnly: bool(o.read_only) ?? false,
    note: str(o.note),
    pushUrl: str(o.push_url),
    stale: str(o.stale),
    worktree: str(o.worktree),
  };
}

/** What the draft tells the agent about the branch it works on and what a push does. */
export function pullBranchLines(
  pull: { headRef: string; baseRef: string },
  prepared: PreparedPullBranch | undefined,
): string[] {
  const branch = prepared?.branch ?? pull.headRef;
  const where =
    branch === pull.headRef
      ? `This worktree is on the pull request's branch \`${branch}\` (into \`${pull.baseRef}\`)`
      : `This worktree is on the local branch \`${branch}\`, made from the pull request's head \`${pull.headRef}\` (into \`${pull.baseRef}\`)`;
  const push = prepared?.readOnly
    ? "The pull request comes from a fork that does not allow maintainer edits, so commits stay on this branch: pushing does not update the pull request."
    : prepared?.pushUrl
      ? `Push with \`git push ${prepared.pushUrl} HEAD:${pull.headRef}\` to update the pull request; a plain \`git push\` does not reach it.`
      : "Pushing updates the pull request.";
  return [`${where}. ${push}`, ...(prepared?.stale ? [prepared.stale] : [])];
}

export function pullThreadMessage(
  pull: PullRequest,
  detail: PullDetail | undefined,
  parts: PullThreadParts,
  instruction: string,
  prepared?: PreparedPullBranch,
): string {
  const out = [
    `${instruction.trim() || DEFAULT_PULL_INSTRUCTION}`,
    "",
    `Pull request #${pull.number}: ${pull.title}${pull.url ? ` (${pull.url})` : ""}`,
    ...pullBranchLines(pull, prepared),
  ];
  const body = detail?.body.trim();
  if (parts.body && body) out.push("", "## Description", "", body);
  const comments = openComments(detail);
  if (parts.comments && comments.length) {
    out.push("", "## Review comments");
    for (const c of comments) {
      const where = c.line ? `${c.path}:${c.line}` : c.path;
      out.push("", `- \`${where}\` (${c.author || "reviewer"}): ${c.body.trim().replace(/\n+/g, "\n  ")}`);
    }
  }
  const checks = failingChecks(detail);
  if (parts.checks && checks.length) {
    out.push("", "## Failing checks", "");
    for (const c of checks) out.push(`- ${c.name}${c.url ? ` (${c.url})` : ""}`);
  }
  return out.join("\n");
}

/**
 * The thread already linked to this pull request (`_rustcode/pulls/watches`), so the panel offers
 * it instead of a second one: a link watch on the same number, in the same repository when the
 * watch names one.
 */
export function linkedPullThread(
  watches: Json | undefined,
  number: number,
  repo?: string,
): string | undefined {
  for (const value of arr(obj(watches)?.watches)) {
    const watch = obj(value);
    if (watch?.link !== true || num(watch.number) !== number) continue;
    const named = str(watch.repo);
    if (repo && named && named.toLowerCase() !== repo.toLowerCase()) continue;
    const id = str(watch.sessionId);
    if (id) return id;
  }
  return undefined;
}
