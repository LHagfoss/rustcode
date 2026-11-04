// The project's pull requests, read once for the sidebar's rows: a thread
// whose branch has one shows the pull's state beside its project
// (ui/Sidebar.tsx). The engine reads them through `gh` (`_rustcode/pulls`), which
// is too slow to do per row, so one read per project is kept for a few minutes
// and every row reads the same answer. The panel and the review view keep
// their own reads; this one is only what the list needs.

import { type PullRequest, parsePullList } from "../acp/rustcode";
import { Method } from "../acp/types";
import { request, useApp } from "./store";

/** How long a project's read is trusted: a pull request changes on the host, not under our hands. */
export const PULLS_TTL = 5 * 60_000;

/** The most pull requests one read keeps: enough to hold the branches of a project's threads. */
const LIMIT = 50;

/** How many projects the sidebar reads for: a list is one or two projects in practice. */
export const MAX_PROJECTS = 4;

interface Entry {
  at: number;
  pulls: PullRequest[];
}

const cache = new Map<string, Entry>();
const reading = new Set<string>();
const listeners = new Set<() => void>();
let version = 0;

/** A number that changes whenever a project's read lands, for `useSyncExternalStore`. */
export const pullsVersion = () => version;

export function subscribePulls(listener: () => void): () => void {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

/** A project's pull requests, when they were read recently enough to show. */
export function cachedPulls(cwd: string): PullRequest[] | undefined {
  const entry = cache.get(cwd);
  return entry && Date.now() - entry.at < PULLS_TTL ? entry.pulls : undefined;
}

/**
 * Reads a project's pull requests once, unless they were read in the last
 * `PULLS_TTL`. A read in flight is not repeated, and a project that cannot
 * answer keeps an empty answer for the same while: the row then says nothing
 * about a branch instead of asking again on every render.
 */
export function loadPulls(cwd: string): void {
  if (!cwd || !useApp.getState().agent?.pullRequests) return;
  if (cachedPulls(cwd) || reading.has(cwd)) return;
  reading.add(cwd);
  void request(Method.pulls, { cwd, state: "all", limit: LIMIT }, 60_000)
    .then((result) => parsePullList(result).pulls)
    .catch(() => [] as PullRequest[])
    .then((pulls) => {
      cache.set(cwd, { at: Date.now(), pulls });
      reading.delete(cwd);
      version += 1;
      for (const listener of listeners) listener();
    });
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
