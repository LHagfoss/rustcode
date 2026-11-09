// The project's pull requests as the sidebar's rows read them: a thread whose
// branch has one shows the pull's state beside its project (ui/Sidebar.tsx).
//
// The answers themselves live in `state/pullQueries.ts`, shared with the panel,
// the detail and the review view, so one read answers whoever asks next. This is
// the wide "all" read the rows need, trusted for minutes where a panel trusts a
// read for seconds: `gh` is a process per read, and a row is not worth another.

import type { PullRequest } from "../acp/rustcode";
import {
  pullList,
  pullListFailedAt,
  pullQueriesVersion,
  readPullList,
  subscribePullQueries,
} from "./pullQueries";
import { useApp } from "./store";

/** How long a project's read is trusted: a pull request changes on the host, not under our hands. */
export const PULLS_TTL = 5 * 60_000;

/** The most pull requests one read keeps: enough to hold the branches of a project's threads. */
const LIMIT = 50;

/** How many projects the sidebar reads for: a list is one or two projects in practice. */
export const MAX_PROJECTS = 4;

/** A number that changes whenever a read lands, for `useSyncExternalStore`. */
export const pullsVersion = pullQueriesVersion;

export function subscribePulls(listener: () => void): () => void {
  return subscribePullQueries(listener);
}

/** A project's pull requests, when they were read recently enough to show (or saved on the device). */
export function cachedPulls(cwd: string): PullRequest[] | undefined {
  const answer = pullList(cwd, "all", LIMIT);
  if (!answer) return undefined;
  return answer.saved || Date.now() - answer.readAt < PULLS_TTL ? answer.pulls : undefined;
}

/**
 * Reads a project's pull requests once, unless they were read in the last
 * `PULLS_TTL`, or the last try failed in that time: a project that cannot answer
 * keeps its failure for the same while, so a row says nothing about a branch and
 * a re-render does not spawn another `gh`. A read in flight is not repeated
 * either: the rows and the panel that asks at the same moment wait on one process.
 */
export function loadPulls(cwd: string): void {
  if (!cwd || !useApp.getState().agent?.pullRequests) return;
  const failedAt = pullListFailedAt(cwd, "all", LIMIT);
  if (failedAt !== undefined && Date.now() - failedAt < PULLS_TTL) return;
  void readPullList(cwd, "all", { limit: LIMIT, maxAge: PULLS_TTL });
}

/** The projects a list of branches would read for, most useful first and capped. */
export function projectFolders(folders: Iterable<string>): string[] {
  const seen = new Set<string>();
  for (const cwd of folders) if (cwd) seen.add(cwd);
  return [...seen].slice(0, MAX_PROJECTS);
}

/**
 * The pull request a thread's branch has. A pull that was merged or closed
 * still names its branch, so the row keeps telling you what became of it.
 */
export function pullForBranch(
  pulls: PullRequest[] | undefined,
  branch: string | undefined,
): PullRequest | undefined {
  if (!pulls || !branch) return undefined;
  return pulls.find((pull) => pull.headRef === branch);
}

/** What a pull's mark says: a draft is its own state, and the rest is the pull's own. */
export function pullState(pull: PullRequest): "open" | "draft" | "merged" | "closed" {
  if (pull.state === "merged" || pull.state === "closed") return pull.state;
  return pull.draft ? "draft" : "open";
}
