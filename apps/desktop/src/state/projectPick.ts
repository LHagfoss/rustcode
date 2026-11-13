// The project picker (issue 3598): the same panel opens from the thread header's project and from
// the folder button under the composer. With no thread open it picks where the next one starts;
// with a thread open it moves that thread to the project picked (`moveThread`, which the engine
// does for a thread with turns too). Both places read the thread's own folder, so a move from one
// shows in the other. Pure, so the rule is tested without the store.

import { cleanProjectPath } from "./orchestration";

/** What picking a project does. */
export type ProjectPick =
  | { kind: "none" }
  | { kind: "start-in"; path: string }
  | { kind: "move"; id: string; path: string }
  | { kind: "blocked"; reason: string };

/** The thread a pick is about: its key, its folder, whether a turn runs and its worktree's repo. */
export interface PickedThread {
  id: string;
  cwd: string;
  running?: boolean;
  repo?: string;
}

/**
 * Why the open thread cannot move to another project now, or nothing when it can: a running
 * turn has to stop first, and an engine without `session/move` cannot move threads at all.
 */
export function moveBlockedReason(thread: PickedThread | undefined, canMove: boolean): string | undefined {
  if (!thread) return undefined;
  if (thread.running) return "Stop the running turn to move this thread to another project.";
  if (!canMove) return "This server cannot move a thread to another project. Update RustCode.";
  return undefined;
}

/** Whether `path` is the project the thread is in: its own folder, or the repo its worktree is of. */
export function isCurrentProject(thread: PickedThread | undefined, cwd: string, path: string): boolean {
  const at = cleanProjectPath(path);
  if (!thread) return cleanProjectPath(cwd) === at;
  return cleanProjectPath(thread.cwd) === at || (!!thread.repo && cleanProjectPath(thread.repo) === at);
}

/** What picking `path` does for the open thread (or the new-thread screen when there is none). */
export function projectPick(thread: PickedThread | undefined, path: string, canMove: boolean): ProjectPick {
  if (!path) return { kind: "none" };
  if (!thread) return { kind: "start-in", path };
  if (cleanProjectPath(thread.cwd) === cleanProjectPath(path)) return { kind: "none" };
  const reason = moveBlockedReason(thread, canMove);
  if (reason) return { kind: "blocked", reason };
  return { kind: "move", id: thread.id, path };
}
