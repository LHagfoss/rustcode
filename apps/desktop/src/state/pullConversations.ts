// Writing to a pull request's review conversations from its diff view (issue 3768): reply, resolve or
// unresolve one, or start one on a line or a range of lines. The engine runs each with `gh`
// (`_rustcode/pulls/thread`); after a write the pull request is read again, so what shows is
// GitHub's. The line math is pure, so tests drive it.

import type { JsonObject } from "../acp/jsonrpc";
import { type PullRequest, parsePullError, pullErrorTitle, repositoryOf } from "../acp/rustcode";
import { Method } from "../acp/types";
import type { PatchLine } from "./diff";
import type { PullReference } from "./pullCompletion";
import { readPullDetail } from "./pullQueries";
import { request } from "./store";

/** What `_rustcode/pulls/thread` does. */
export type ThreadAction =
  | { kind: "reply"; threadId: string; body: string }
  | { kind: "resolve"; threadId: string }
  | { kind: "unresolve"; threadId: string }
  | {
      kind: "comment";
      commitId: string;
      path: string;
      line: number;
      side: "LEFT" | "RIGHT";
      startLine?: number;
      body: string;
    };

/** Where a new comment goes: a line, or the lines from `startLine`, on one side of the diff. */
export interface LineTarget {
  path: string;
  line: number;
  side: "LEFT" | "RIGHT";
  startLine?: number;
}

/**
 * The comment target for diff line `end`, from line `start` when a range was picked. A removed
 * line is on the base's side (`LEFT`, its old number), any other on the pull request's (`RIGHT`).
 * A range that crosses sides or has a hunk header keeps only the end line. `null` for a hunk.
 */
export function lineTarget(
  path: string,
  lines: readonly PatchLine[],
  end: number,
  start = end,
): LineTarget | null {
  const at = (i: number) => {
    const line = lines[i];
    if (!line || line.kind === "hunk") return null;
    return line.kind === "del"
      ? line.old !== undefined
        ? { side: "LEFT" as const, n: line.old }
        : null
      : line.new !== undefined
        ? { side: "RIGHT" as const, n: line.new }
        : null;
  };
  const [from, to] = start <= end ? [start, end] : [end, start];
  const last = at(to);
  if (!last) return null;
  const first = at(from);
  const spans = first && first.side === last.side && lines.slice(from, to).every((l) => l.kind !== "hunk");
  return {
    path,
    line: last.n,
    side: last.side,
    ...(spans && first.n < last.n ? { startLine: first.n } : {}),
  };
}

/** The label of a target: `line 12` or `lines 9–12`, `base` first on the left side. */
export function targetLabel(target: LineTarget): string {
  const base = target.side === "LEFT" ? "base " : "";
  return target.startLine !== undefined
    ? `${base}lines ${target.startLine}–${target.line}`
    : `${base}line ${target.line}`;
}

export type ThreadActionOutcome = { ok: true } | { ok: false; error: string };

/**
 * Sends `action` for `pull`, then reads the pull request again so the panel shows the result. A
 * failure comes back as GitHub's message for the caller to show next to what the user typed.
 */
export async function actOnThread(
  cwd: string,
  pull: PullRequest,
  action: ThreadAction,
): Promise<ThreadActionOutcome> {
  const repository = repositoryOf(pull.url) ?? "";
  try {
    const answer = (await request(
      Method.pullThread,
      { cwd, repository, number: pull.number, action },
      60_000,
    )) as JsonObject;
    const error = parsePullError(answer);
    if (error) return { ok: false, error: `${pullErrorTitle(error.code)}: ${error.message}` };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
  void readPullDetail(cwd, pull.number, { force: true });
  return { ok: true };
}

/** How long a repository's reference list is reused before it is read again. */
const REFERENCES_FRESH_MS = 5 * 60_000;
const references = new Map<string, { at: number; read: Promise<PullReference[]> }>();

/**
 * The repository's newest issues and pull requests, for `#` completion: read once per folder and
 * reused for a few minutes. A failure is an empty list, so the field still takes text.
 */
export function loadReferences(cwd: string): Promise<PullReference[]> {
  const known = references.get(cwd);
  if (known && Date.now() - known.at < REFERENCES_FRESH_MS) return known.read;
  const read = request(Method.pullReferences, { cwd }, 30_000)
    .then((answer) => {
      const list = (answer as JsonObject | undefined)?.references;
      if (!Array.isArray(list)) throw new Error("no references");
      return list.flatMap((item): PullReference[] => {
        const o = item as JsonObject;
        const number = typeof o.number === "number" ? o.number : 0;
        if (!number) return [];
        return [
          {
            number,
            title: typeof o.title === "string" ? o.title : "",
            kind: o.kind === "pull" ? "pull" : "issue",
            state: typeof o.state === "string" ? o.state : "",
          },
        ];
      });
    })
    .catch(() => {
      references.delete(cwd);
      return [];
    });
  references.set(cwd, { at: Date.now(), read });
  return read;
}
