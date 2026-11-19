// A delegate started from another project: the parent lives in one folder (`~/code`) and the
// thread it started runs in another (`~/code/rustcode`, often in a worktree of its own). Under the
// sidebar's project filter the parent's row is not drawn, so without this the delegate either
// vanished from its own project's list or came back as a plain row with no word of where it came
// from, and read as a stray thread. Here: which nested threads take a row of their own in a
// filtered list, and the "from ‹parent›" line their row and their header show. Pure, so the
// sidebar, the header and their tests share one answer (ui/Sidebar.tsx, ui/Branches.tsx).

import { delegateParent, projectName, type SessionSummary } from "../acp/types";
import type { Link } from "./orchestration";
import { hostOf } from "./sidebarGroups";
import { parentOf } from "./topLevel";

/** A thread's project as the sidebar groups it: one computer's folder, a worktree under its repository. */
export function projectKeyOf(session: Pick<SessionSummary, "host" | "cwd" | "worktree">): string {
  return `${hostOf(session)}\u0000${session.worktree?.repo || session.cwd || ""}`;
}

/** Whether two threads belong to one project on one computer. */
export function sameProject(
  a: Pick<SessionSummary, "host" | "cwd" | "worktree">,
  b: Pick<SessionSummary, "host" | "cwd" | "worktree">,
): boolean {
  return projectKeyOf(a) === projectKeyOf(b);
}

/**
 * The nested threads that take a row of their own in a filtered list: each one the list draws
 * (`drawn`, the threads the project filter kept) whose parent it does not draw. Nested below a row
 * that is not there, it would be drawn nowhere, so it keeps a row in its own project, marked with
 * where it came from. Its own delegates in the same project still nest under it.
 *
 * Without a filter every parent is drawn and this is empty: a cross-project delegate nests under
 * its parent like any other, so it is never drawn twice in one list.
 */
export function detachedDelegates(
  drawn: ReadonlySet<string>,
  parentOf: ReadonlyMap<string, string>,
  listed?: ReadonlySet<string>,
): Set<string> {
  const out = new Set<string>();
  for (const id of drawn) {
    const parent = parentOf.get(id);
    // A parent the app does not list at all (past the list's cap) is not filtered out of view:
    // its delegates stay folded under it instead of each taking a row.
    if (parent && !drawn.has(parent) && (!listed || listed.has(parent))) out.add(id);
  }
  return out;
}

/** The thread a delegate was started from, as its row and header say it. */
export interface DelegateOrigin {
  /** The parent's id, when the record has one. */
  id?: string;
  /** The parent as it is now when it is listed, else as it was named when this was started. */
  title: string;
  /** The parent's project, when it is not the delegate's own: the line says where to look. */
  project?: string;
  /** The parent runs on another computer: that computer's id. */
  host?: string;
  /** The parent is a thread the app lists, so it can be opened. */
  listed: boolean;
}

/**
 * Where a delegate came from: the thread that started it (the newest start link, else the parent
 * its metadata recorded), with that thread's project when it differs from the delegate's own.
 * Nothing for a thread no agent started: a fork or a side chat says so itself (state/branches.ts).
 */
export function delegateOriginOf(
  sessions: readonly SessionSummary[],
  id: string,
  links: readonly Link[],
): DelegateOrigin | undefined {
  const session = sessions.find((x) => x.id === id);
  if (!session) return undefined;
  // A fork or a side chat is the user's branch, not a delegate; an unknown relationship is neither.
  const meta = session.parent ? delegateParent(session.parent) : undefined;
  if (session.parent && !meta) return undefined;
  const parentId = parentOf({ id, parent: meta }, links);
  if (!parentId && !meta?.title) return undefined;
  if (parentId === id) return undefined;
  const listed = parentId ? sessions.find((x) => x.id === parentId) : undefined;
  const linkTitle = links.find(
    (l) => l.started && l.target === id && l.orchestrator === parentId,
  )?.orchestratorTitle;
  return originFrom(session, parentId, listed, meta?.title?.trim() || linkTitle?.trim());
}

/**
 * The origin of `child`, given the thread above it (`parent`, when the app lists it) and the name
 * the child recorded for it: the parent's title as it is now, its project when that is another one,
 * and its computer when that is another one.
 */
export function originFrom(
  child: Pick<SessionSummary, "host" | "cwd" | "worktree">,
  parentId: string | undefined,
  parent: SessionSummary | undefined,
  recordedTitle?: string,
): DelegateOrigin {
  const folder = parent ? parent.worktree?.repo || parent.cwd : undefined;
  return {
    id: parentId,
    title: parent?.title?.trim() || recordedTitle || "another thread",
    ...(parent && folder && !sameProject(parent, child) ? { project: projectName(folder) } : {}),
    ...(parent?.host && parent.host !== child.host ? { host: parent.host } : {}),
    listed: !!parent,
  };
}

/** The words of the line: `from “Title”`, and the parent's project after it when it is another one. */
export function originText(origin: Pick<DelegateOrigin, "title" | "project">): string {
  return origin.project ? `from ${origin.title} · ${origin.project}` : `from ${origin.title}`;
}
