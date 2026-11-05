// Orchestrator v2 extensions (docs/orchestrator.md): worktree threads,
// thread status and per-turn checkpoints. Shapes and parsers; the store
// calls the methods and hides a feature the engine does not implement.

import { arr, bool, type Json, type JsonObject, num, obj, rustcodeMeta, str } from "./jsonrpc";

export const V2Method = {
  worktreeCreate: "_rustcode/worktrees/create",
  worktreeList: "_rustcode/worktrees/list",
  worktreeRemove: "_rustcode/worktrees/remove",
  sessionStatus: "_rustcode/session/status",
  sessionPin: "_rustcode/session/pin",
  checkpointList: "_rustcode/checkpoints/list",
  checkpointDiff: "_rustcode/checkpoints/diff",
  checkpointRestore: "_rustcode/checkpoints/restore",
  orchestration: "_rustcode/orchestration",
  /** A call the engine dropped before it answered: the wait it opened ends there and then. */
  orchestrationCancelled: "_rustcode/orchestration/cancelled",
} as const;

// ---------------------------------------------------------------- status

export type ThreadStatus = "idle" | "running" | "needs_input" | "review" | "done";

export const STATUS_LABEL: Record<ThreadStatus, string> = {
  idle: "Idle",
  running: "Running",
  needs_input: "Needs input",
  review: "Review",
  done: "Done",
};

export function parseStatus(value: unknown): ThreadStatus | undefined {
  const s = str(value);
  return s === "idle" || s === "running" || s === "needs_input" || s === "review" || s === "done"
    ? s
    : undefined;
}

/** The status a `session/update` carries, on the notification or on its update. */
export function statusInUpdate(params: JsonObject): ThreadStatus | undefined {
  return parseStatus(rustcodeMeta(obj(params.update))?.status) ?? parseStatus(rustcodeMeta(params)?.status);
}

// ---------------------------------------------------------------- worktrees

export interface Worktree {
  path: string;
  branch: string;
  repo: string;
  /** The commit it branched from. */
  base?: string;
  /** The branch (or ref) that commit was resolved from, as the engine recorded it. */
  baseRef?: string;
  createdAt?: number;
  dirty?: boolean;
  ahead?: number;
  behind?: number;
  sessionIds?: string[];
}

export function parseWorktree(value: Json | undefined): Worktree | undefined {
  const o = obj(value);
  const path = str(o?.path);
  if (!o || !path) return undefined;
  const created = str(o.created_at);
  return {
    path,
    branch: str(o.branch) ?? "",
    repo: str(o.repo) ?? "",
    base: str(o.base),
    baseRef: str(o.base_ref),
    createdAt: created ? Date.parse(created) || undefined : undefined,
    dirty: bool(o.dirty),
    ahead: num(o.ahead),
    behind: num(o.behind),
    sessionIds: Array.isArray(o.session_ids)
      ? arr(o.session_ids).flatMap((x) => (typeof x === "string" ? [x] : []))
      : undefined,
  };
}

export function parseWorktreeList(result: Json | undefined): Worktree[] {
  return arr(obj(result)?.worktrees).flatMap((w) => parseWorktree(w) ?? []);
}

/** Why the engine refused to remove a worktree (`error.data.reason`). */
export function removeRefusal(data: Json | undefined): "dirty" | "unpushed" | undefined {
  const reason = str(obj(data)?.reason);
  return reason === "dirty" || reason === "unpushed" ? reason : undefined;
}

const TYPES: [RegExp, string][] = [
  [/\b(fix|bug|broken|crash|error|fail)/i, "fix"],
  [/\b(docs?|readme|document)/i, "docs"],
  [/\b(refactor|clean ?up|rename|simplif)/i, "refactor"],
  [/\b(tests?|spec|coverage)\b/i, "test"],
  [/\b(perf|faster|speed|slow)/i, "perf"],
  [/\b(ci|workflow|pipeline)\b/i, "ci"],
  [/\b(bump|upgrade|chore|deps?)\b/i, "chore"],
];

/**
 * A branch name for a new worktree from a title or first message:
 * `<type>/<slug>`, lowercase, at most 48 characters. The engine dedupes it.
 */
export function suggestBranch(title: string): string {
  const text = title.trim().split("\n")[0] ?? "";
  const type = TYPES.find(([re]) => re.test(text))?.[1] ?? "feat";
  const words = text
    .toLowerCase()
    .replace(/[^a-z0-9\s-]/g, " ")
    .split(/\s+/)
    .filter(
      (w) =>
        w && !["the", "a", "an", "to", "of", "and", "in", "on", "for", "please", "can", "you"].includes(w),
    )
    .filter((w, i) => !(i === 0 && w === type));
  let slug = "";
  for (const w of words) {
    const next = slug ? `${slug}-${w}` : w;
    if (next.length > 40) break;
    slug = next;
  }
  return `${type}/${slug || "thread"}`;
}

/** Whether a string is a branch name git accepts, roughly (`git check-ref-format`). */
export function validBranch(name: string): boolean {
  return (
    /^[A-Za-z0-9._/-]{1,120}$/.test(name) &&
    !name.startsWith("-") &&
    !name.startsWith("/") &&
    !name.endsWith("/") &&
    !name.endsWith(".lock") &&
    !name.includes("..") &&
    !name.includes("//")
  );
}

// ---------------------------------------------------------------- checkpoints

export interface Checkpoint {
  turn: number;
  ref: string;
  createdAt?: number;
  prompt: string;
  filesChanged: number;
  insertions: number;
  deletions: number;
}

export function parseCheckpoints(result: Json | undefined): Checkpoint[] {
  return arr(obj(result)?.checkpoints)
    .flatMap((c): Checkpoint[] => {
      const o = obj(c);
      const turn = num(o?.turn);
      if (!o || turn === undefined) return [];
      const created = str(o.created_at);
      return [
        {
          turn,
          ref: str(o.ref) ?? "",
          createdAt: created ? Date.parse(created) || undefined : undefined,
          prompt: str(o.prompt) ?? "",
          filesChanged: num(o.files_changed) ?? 0,
          insertions: num(o.insertions) ?? 0,
          deletions: num(o.deletions) ?? 0,
        },
      ];
    })
    .sort((a, b) => a.turn - b.turn);
}

export interface PatchFile {
  path: string;
  status: "added" | "modified" | "deleted" | "renamed";
  insertions: number;
  deletions: number;
  patch: string;
  oldPath?: string;
}

export function parseTurnDiff(result: Json | undefined): PatchFile[] {
  return arr(obj(result)?.files).flatMap((f): PatchFile[] => {
    const o = obj(f);
    const path = str(o?.path);
    if (!o || !path) return [];
    const status = str(o.status);
    return [
      {
        path,
        status: status === "added" || status === "deleted" || status === "renamed" ? status : "modified",
        insertions: num(o.insertions) ?? 0,
        deletions: num(o.deletions) ?? 0,
        patch: str(o.patch) ?? "",
        oldPath: str(o.old_path),
      },
    ];
  });
}

/**
 * Pairs each finished turn of the transcript with its checkpoint. The
 * engine keeps the latest 50, so the transcript's turns and the list are
 * aligned from the end; a checkpoint whose prompt does not start the turn's
 * message is left out rather than shown on the wrong turn.
 */
export function checkpointsByTurn(
  turns: { id: string; prompt: string }[],
  checkpoints: Checkpoint[],
): Record<string, Checkpoint> {
  const out: Record<string, Checkpoint> = {};
  const offset = checkpoints.length - turns.length;
  turns.forEach((turn, i) => {
    const c = checkpoints[i + offset];
    if (!c) return;
    const want = c.prompt.trim().slice(0, 40);
    if (want && !turn.prompt.trim().startsWith(want) && !turn.prompt.includes(want)) return;
    out[turn.id] = c;
  });
  return out;
}
