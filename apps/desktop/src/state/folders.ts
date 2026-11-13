// ⌘O and ⌘⇧O: the folder a thread works in. ⌘O picks the folder of the
// thread in front: a new one starts there, an open one moves there
// (`moveThread`). ⌘⇧O picks where new threads start by default.

import { type PathKind, pathKinds, pickFolder } from "../lib/native";
import { dropMessages, dropTarget, planFolderDrop } from "./folderDrop";
import { registeredProjects, registerProject } from "./projectBook";
import { type PickedThread, projectPick } from "./projectPick";
import { projectsOn } from "./projects";
import { moveThread, setDefaultCwd, setProjectCwd, startNewThread, toast, useApp } from "./store";
import { LOCAL_HOST } from "./threadKeys";

export async function chooseFolder(asDefault: boolean): Promise<void> {
  const s = useApp.getState();
  if (asDefault) {
    const picked = await pickFolder(s.defaultCwd || s.projectCwd);
    if (picked) {
      setDefaultCwd(picked);
    }
    return;
  }
  const id = s.selectedId;
  // A running thread cannot move: say so before the picker, not after it.
  if (id && s.threads[id]?.running) {
    toast("Stop the running turn before moving this thread to another folder.");
    return;
  }
  const picked = await pickFolder((id ? s.threads[id]?.cwd : undefined) ?? s.projectCwd);
  if (!picked) return;
  if (id) void moveThread(id, picked);
  // No thread open: the folder is on this disk, so it starts a new thread here rather than
  // leaving a machine named over a path that machine may not have.
  else startNewThread(picked);
}

/** The open thread as the project picker sees it (state/projectPick.ts). */
export function pickedThread(id: string | null | undefined): PickedThread | undefined {
  const thread = id ? useApp.getState().threads[id] : undefined;
  if (!thread) return undefined;
  return { id: thread.id, cwd: thread.cwd, running: thread.running, repo: thread.worktree?.repo };
}

/**
 * A project picked in the project picker, from the header or the composer's folder button: the
 * open thread moves there, or with none open the next thread starts there.
 */
export async function pickProject(threadId: string | undefined, path: string): Promise<void> {
  const s = useApp.getState();
  const thread = pickedThread(threadId);
  const pick = projectPick(thread, path, !!s.agent?.sessionMove);
  if (pick.kind === "start-in") setProjectCwd(pick.path);
  else if (pick.kind === "move") await moveThread(pick.id, pick.path);
  else if (pick.kind === "blocked") toast(pick.reason);
}

/**
 * Folders dropped on the sidebar (issue 3608): each new one is registered as a project as
 * ⌘K › "Add project folder" does, one that is a project already is opened instead, and the
 * new-thread screen comes up in the first of them. Anything else is refused with a toast.
 */
export async function addDroppedFolders(paths: string[]): Promise<void> {
  if (!paths.length) return;
  let kinds: PathKind[];
  try {
    kinds = await pathKinds(paths);
  } catch (error) {
    toast(`Could not read the dropped folders: ${String(error)}`, "error");
    return;
  }
  const s = useApp.getState();
  const known = [
    ...projectsOn(s.sessions, LOCAL_HOST).map((project) => project.path),
    ...registeredProjects().map((entry) => entry.path),
  ];
  const plan = planFolderDrop(paths, kinds, known);
  for (const path of plan.add) registerProject(path);
  const target = dropTarget(plan);
  if (target) startNewThread(target);
  const { done, refused } = dropMessages(plan);
  if (done) toast(done, "success");
  if (refused) toast(refused, plan.add.length || plan.open.length ? "info" : "error");
}
