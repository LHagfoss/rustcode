// The sidebar's project folders, for the "Group by project" pref. The list is one flat list of
// every computer's threads (docs/desktop.md › Several machines); matching folder names share a group.

import type { SessionSummary } from "../acp/types";
import { projectName } from "../acp/types";
import { LOCAL_HOST } from "./threadKeys";

/** One project's rows. */
export interface ProjectGroup {
  /** The folder name shared across machines. */
  key: string;
  label: string;
  items: SessionSummary[];
  /** The folder the group's threads run in on this computer: what **New thread** under it starts on. */
  project?: string;
}

/** A thread's machine: its host, or this computer when the list came from here. */
export function hostOf(session: Pick<SessionSummary, "host">): string {
  return session.host ?? LOCAL_HOST;
}

/** Group matching folder names across computers. Worktrees belong to their repository. */
export function sidebarProjects(items: readonly SessionSummary[]): ProjectGroup[] {
  const projects = new Map<string, ProjectGroup>();
  for (const item of items) {
    const project = item.worktree?.repo || item.cwd || "";
    const label = project ? projectName(project) : "No folder";
    const key = project ? `name:${label}` : "no-folder:";
    const group = projects.get(key) ?? { key, label, items: [] };
    group.items.push(item);
    if (!group.project && hostOf(item) === LOCAL_HOST && project) group.project = project;
    projects.set(key, group);
  }
  return [...projects.values()];
}

/**
 * The folders A–Z by name, whatever the rows' sort: a folder never jumps when one of its threads
 * starts or finishes. Names with equal sort order keep a fixed order by key.
 */
export function byProjectName(groups: ProjectGroup[]): ProjectGroup[] {
  return groups.sort(
    (a, b) =>
      a.label.localeCompare(b.label, undefined, { sensitivity: "base" }) ||
      (a.key < b.key ? -1 : a.key > b.key ? 1 : 0),
  );
}

/** The working folder, including a worktree rather than its parent repository. */
export function threadFolder(session: Pick<SessionSummary, "cwd" | "worktree">): string {
  return session.worktree?.path || session.cwd || "";
}

/** Pairing alone does not make a local-only filtered list a mixed list. */
export function spansMachines(items: readonly Pick<SessionSummary, "host">[]): boolean {
  return new Set(items.map(hostOf)).size > 1;
}

/** Count threads in the active filter once, even when they have several inbox items. */
export function filteredNeedsCount(items: readonly { threadId: string }[], ids: ReadonlySet<string>): number {
  return new Set(items.filter((item) => ids.has(item.threadId)).map((item) => item.threadId)).size;
}

/**
 * What a row's second line says about where it runs, most telling first: a worktree's branch, a
 * branch off the default one, a subfolder of its project. A thread in its project's own checkout
 * on the default branch says nothing under a project heading (the heading says it), and its
 * project's name in the flat list. The full path stays on the hover card.
 */
export type RowWhere = { kind: "worktree" | "branch" | "folder"; text: string } | { kind: "none" };

export function rowWhere(input: {
  session: Pick<SessionSummary, "cwd" | "repo" | "worktree">;
  /** The checkout's branch and its default branch, once read (state/checkoutBranch.ts). */
  branch?: string;
  defaultBranch?: string;
  /** Drawn under its project's heading. */
  grouped: boolean;
}): RowWhere {
  const { session, branch, defaultBranch, grouped } = input;
  const root = session.worktree?.repo || session.repo || session.cwd;
  if (!root) return { kind: "folder", text: "No folder" };
  const named = (where: RowWhere): RowWhere => {
    if (grouped) return where;
    const project = projectName(root);
    return where.kind === "none"
      ? { kind: "folder", text: project }
      : { ...where, text: `${project} · ${where.text}` };
  };
  const worktree = session.worktree;
  if (worktree?.branch) return named({ kind: "worktree", text: worktree.branch });
  if (worktree?.path) return named({ kind: "worktree", text: projectName(worktree.path) });
  if (branch && defaultBranch && branch !== defaultBranch) return named({ kind: "branch", text: branch });
  const cwd = session.cwd.replace(/[/\\]+$/, "");
  const base = root.replace(/[/\\]+$/, "");
  const sub = cwd.length > base.length && cwd.startsWith(base) ? cwd.slice(base.length + 1) : "";
  if (sub && /^[/\\]$/.test(cwd[base.length] ?? "")) return named({ kind: "folder", text: sub });
  return named({ kind: "none" });
}
