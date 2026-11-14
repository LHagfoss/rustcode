// What a thread opened on a pull request starts with (issue 3727): the instruction, then only the
// parts the user ticked (the description, the review comments, the failing checks), as a message
// the user reads and edits in the composer before sending. Pure, so the sheet and tests share it.

import { arr, type Json, num, obj, str } from "../acp/jsonrpc";
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

export function pullThreadMessage(
  pull: PullRequest,
  detail: PullDetail | undefined,
  parts: PullThreadParts,
  instruction: string,
): string {
  const out = [
    `${instruction.trim() || DEFAULT_PULL_INSTRUCTION}`,
    "",
    `Pull request #${pull.number}: ${pull.title}${pull.url ? ` (${pull.url})` : ""}`,
    `This worktree is on its branch \`${pull.headRef}\` (into \`${pull.baseRef}\`); pushing updates the pull request.`,
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
