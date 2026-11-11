// Orchestrator v2 extensions (docs/orchestrator.md): worktree threads,
// thread status and per-turn checkpoints. Shapes and parsers; the store
// calls the methods and hides a feature the engine does not implement.

import { arr, bool, type Json, type JsonObject, num, obj, rustcodeMeta, str } from "./jsonrpc";

export const V2Method = {
  worktreeCreate: "_rustcode/worktrees/create",
  worktreeList: "_rustcode/worktrees/list",
  worktreeRemove: "_rustcode/worktrees/remove",
  worktreeCleanup: "_rustcode/worktrees/cleanup",
  worktreeState: "_rustcode/worktrees/state",
  worktreeHandoff: "_rustcode/worktrees/handoff",
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
  done: "Settled",
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

/** Where one worktree stands now, as `_rustcode/worktrees/state` answers a row. */
export interface WorktreeState {
  path: string;
  /** False when the worktree's folder is gone (it may have been removed since a list). */
  exists: boolean;
  /** The branch HEAD is on now; a detached HEAD has none. */
  branch?: string;
  /** The ref `ahead`/`behind` are counted against (a branch's upstream, else origin/<branch>, else its base). */
  against?: string;
  ahead?: number;
  behind?: number;
  /** Files with uncommitted changes, untracked ones included. */
  dirty?: number;
  /** The last commit's committer time, ISO 8601. */
  lastCommit?: string;
  lastCommitSubject?: string;
  /** Why the counts are unknown: a git that could not answer (`git status failed: …`). */
  error?: string;
}

export function parseWorktreeState(value: Json | undefined): WorktreeState | undefined {
  const o = obj(value);
  const path = str(o?.path);
  if (!o || !path) return undefined;
  return {
    path,
    exists: bool(o.exists) ?? false,
    branch: str(o.branch),
    against: str(o.against),
    ahead: num(o.ahead),
    behind: num(o.behind),
    dirty: num(o.dirty),
    lastCommit: str(o.last_commit),
    lastCommitSubject: str(o.last_commit_subject),
    error: str(o.error),
  };
}

export function parseWorktreeStates(result: Json | undefined): WorktreeState[] {
  return arr(obj(result)?.worktrees).flatMap((w) => parseWorktreeState(w) ?? []);
}

/** Why `_rustcode/worktrees/handoff` refused (`error.data.reason`). */
export type HandoffRefusal =
  | "no_worktree"
  | "has_worktree"
  | "not_checkout"
  | "dirty"
  | "dirty_checkout"
  | "unpushed_checkout"
  | "ignored_files"
  | "detached_checkout"
  | "not_git"
  | "no_folder"
  | "unknown"
  | "failed";

const HANDOFF_REFUSALS: readonly string[] = [
  "no_worktree",
  "has_worktree",
  "not_checkout",
  "dirty",
  "dirty_checkout",
  "unpushed_checkout",
  "ignored_files",
  "detached_checkout",
  "not_git",
  "no_folder",
  "unknown",
  "failed",
];

export function handoffRefusal(data: Json | undefined): HandoffRefusal | undefined {
  const reason = str(obj(data)?.reason);
  return reason && HANDOFF_REFUSALS.includes(reason) ? (reason as HandoffRefusal) : undefined;
}

/** What a handoff did, as `_rustcode/worktrees/handoff` answers it. */
export interface HandoffAnswer {
  sessionId?: string;
  to: "checkout" | "worktree";
  moved: boolean;
  /** `to: "checkout"`: whether the worktree was removed. Always true when it moved. */
  worktreeRemoved?: boolean;
  /** `to: "checkout"`: whether the checkout switched to the branch. False means it stayed where it was; `detail` says why. */
  switched?: boolean;
  branch?: string;
  /** The folder the thread runs in now: the checkout, or the new worktree. */
  path?: string;
  repo?: string;
  detail?: string;
}

export function parseHandoff(result: Json | undefined): HandoffAnswer | undefined {
  const o = obj(result);
  if (!o) return undefined;
  return {
    sessionId: str(o.session_id),
    to: str(o.to) === "worktree" ? "worktree" : "checkout",
    moved: bool(o.moved) ?? false,
    worktreeRemoved: bool(o.worktree_removed),
    switched: bool(o.switched),
    branch: str(o.branch),
    path: str(o.path),
    repo: str(o.repo),
    detail: str(o.detail),
  };
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
  /**
   * The state a rewind abandoned, kept rather than taken before a turn: it belongs to no turn of
   * the chat, so no turn's Changes chip pairs with it (`checkpointsByTurn`).
   */
  attempt?: boolean;
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
          attempt: bool(o.attempt) ?? false,
        },
      ];
    })
    .sort((a, b) => a.turn - b.turn);
}

/** What a restore answered: whether the conversation came back too, and what it kept.
 *
 * `kept` is the checkpoint of the state the restore abandoned, when `keep` asked for it: the
 * attempt a rewind leaves behind, which going back can read.
 */
export interface RestoreAnswer {
  conversation: "rewound" | "kept";
  kept?: number;
  /** Why the conversation stayed, when a rewind was asked for and refused. */
  rewindError?: string;
}

export function parseRestoreAnswer(result: Json | undefined): RestoreAnswer {
  const o = obj(result);
  return {
    conversation: str(o?.conversation) === "rewound" ? "rewound" : "kept",
    kept: num(obj(o?.kept)?.turn),
    rewindError: str(o?.rewind_error),
  };
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
 * Pairs each finished turn of the transcript with its checkpoint. The engine takes a checkpoint as
 * a turn starts, so the list holds one for a turn still running: it is left out (it is written
 * after the last finished turn ended), never shifted onto the turn above it. What is left is
 * matched by the message that started each turn, from the end, because the engine keeps only the
 * latest 50 and the transcript may hold fewer or more than either. A checkpoint kept for the state
 * a rewind abandoned starts no turn, so it is never paired (`Checkpoint.attempt`).
 */
export function checkpointsByTurn(
  turns: { id: string; prompt: string; at?: number }[],
  checkpoints: Checkpoint[],
): Record<string, Checkpoint> {
  const out: Record<string, Checkpoint> = {};
  const ended = turns.reduce((last, turn) => Math.max(last, turn.at ?? 0), 0);
  const finished = checkpoints.filter(
    (c) => !c.attempt && (!ended || c.createdAt === undefined || c.createdAt <= ended),
  );
  let at = finished.length - 1;
  for (let i = turns.length - 1; i >= 0 && at >= 0; i--) {
    const prompt = promptKey(turns[i]!.prompt);
    if (!starts(finished[at]!, prompt)) {
      // A checkpoint that starts no turn of the transcript sits above the turn's own (a turn still
      // running, one whose timing was not kept): step over it, and never over the turn's own.
      if (at > 0 && starts(finished[at - 1]!, prompt)) at--;
      else continue;
    }
    out[turns[i]!.id] = finished[at]!;
    at--;
  }
  return out;
}

/** A prompt as the engine stores it in a checkpoint's message: whitespace collapsed. */
function promptKey(text: string): string {
  return text.split(/\s+/).join(" ").trim();
}

/** Whether a checkpoint's message starts the turn's prompt, so the two belong together. */
function starts(checkpoint: Checkpoint, prompt: string): boolean {
  const want = checkpoint.prompt.trim();
  // A turn the engine resumed itself records no prompt: it can be told apart from the others only
  // by its place, which is what the walk from the end gives it.
  if (!want) return true;
  return prompt.startsWith(want) || prompt.includes(want);
}
