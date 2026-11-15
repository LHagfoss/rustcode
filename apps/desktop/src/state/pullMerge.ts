// Merging a pull request from its view in the right panel. The engine waits for
// the checks and squash-merges (`_rustcode/pulls/merge`, the path agents use
// through `merge_when_green`), with scripts/pr-merge.sh's rules: advisory jobs
// are ignored and a failing check stops it. A merge outlives the panel: it is
// kept here, by project and number, so the view shows it again when reopened.

import { useSyncExternalStore } from "react";
import { type PullRequest, parseMergeOutcome, parsePullError, pullErrorTitle } from "../acp/rustcode";
import { Method } from "../acp/types";
import { PULL_LIMIT, readPullDetail, readPullList } from "./pullQueries";
import { request, toast } from "./store";

/** How long a merge waits for checks: the same 25 minutes `merge_when_green` waits at most. */
export const MERGE_WAIT_MS = 25 * 60_000;

/**
 * The view's one prominent control, from where the pull stands: nothing for a
 * draft or a finished pull, "merge" once every check passed (or there are none),
 * "merge_when_green" while checks run, and "blocked" with the failing count.
 */
export type MergeControl =
  | { kind: "none" }
  | { kind: "merge" }
  | { kind: "merge_when_green"; pending: number }
  | { kind: "blocked"; failed: number };

export function mergeControl(pull: Pick<PullRequest, "state" | "draft" | "checks">): MergeControl {
  if (pull.state !== "open" || pull.draft) return { kind: "none" };
  if (pull.checks.failed > 0) return { kind: "blocked", failed: pull.checks.failed };
  if (pull.checks.pending > 0) return { kind: "merge_when_green", pending: pull.checks.pending };
  return { kind: "merge" };
}

/**
 * A check's name as people read it. A job GitHub skipped before it expanded its
 * matrix keeps the template in its name (`Desktop shell (${{ matrix.name }})`):
 * the template goes, and a name that was only the template becomes "Job".
 */
export function checkName(name: string): string {
  const cleaned = name
    .replace(/\s*\(\s*\$\{\{[^}]*\}\}\s*\)/g, "")
    .replace(/\$\{\{[^}]*\}\}\s*/g, "")
    .replace(/\s+/g, " ")
    .trim();
  return cleaned || "Job";
}

type CheckRun = { name: string; status: string };

/** The checks' headline: what needs the user first, then that everything passed. */
export function checksHeadline(runs: readonly CheckRun[]): string {
  const failed = runs.filter((c) => c.status === "failed").length;
  const pending = runs.filter(
    (c) => c.status !== "failed" && c.status !== "passed" && c.status !== "skipped",
  ).length;
  if (failed) return `${failed} ${failed === 1 ? "check failed" : "checks failed"}`;
  if (pending) return `${pending} ${pending === 1 ? "check running" : "checks running"}`;
  return runs.length ? "Checks passed" : "No checks";
}

const running = new Map<string, string>();
const listeners = new Set<() => void>();
let version = 0;

const keyOf = (cwd: string, number: number) => `${cwd}#${number}`;
const changed = () => {
  version++;
  for (const listener of listeners) listener();
};
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => listeners.delete(listener);
};

/** Whether a merge of this pull is waiting or running. */
export function useMerging(cwd: string, number: number): boolean {
  useSyncExternalStore(subscribe, () => version);
  return running.has(keyOf(cwd, number));
}

/** Stops a merge still waiting for checks; one already merging finishes. */
export function cancelMerge(cwd: string, number: number): void {
  const mergeId = running.get(keyOf(cwd, number));
  if (!mergeId) return;
  void request(Method.pullMergeCancel, { mergeId }, 10_000).catch(() => {});
}

/** Waits for the pull's checks and squash-merges it, then reads it again; the outcome is a toast. */
export async function mergePull(cwd: string, pull: Pick<PullRequest, "number" | "headRef">): Promise<void> {
  const key = keyOf(cwd, pull.number);
  if (running.has(key)) return;
  const mergeId = `panel-${pull.number}-${Date.now().toString(36)}`;
  running.set(key, mergeId);
  changed();
  try {
    const result = await request(
      Method.pullMerge,
      { cwd, number: pull.number, branch: pull.headRef, timeoutMs: MERGE_WAIT_MS, mergeId },
      MERGE_WAIT_MS + 120_000,
    );
    const error = parsePullError(result);
    const outcome = error ? null : parseMergeOutcome(result);
    if (error) toast(`${pullErrorTitle(error.code)}: ${error.message}`, "error", { persist: true });
    else if (!outcome) toast(`#${pull.number} did not merge: no answer`, "error", { persist: true });
    else if (outcome.merged) toast(`Merged #${pull.number}`, "success");
    else if (outcome.failing.length)
      toast(`#${pull.number} not merged: ${outcome.failing.join(", ")} failed`, "error", { persist: true });
    else toast(outcome.message || `#${pull.number} not merged (${outcome.state})`, "info");
  } catch (error) {
    cancelMerge(cwd, pull.number);
    toast(
      `#${pull.number} did not merge: ${error instanceof Error ? error.message : String(error)}`,
      "error",
      {
        persist: true,
      },
    );
  } finally {
    running.delete(key);
    changed();
    void readPullDetail(cwd, pull.number, { force: true });
    void readPullList(cwd, "all", { limit: PULL_LIMIT, force: true });
  }
}
