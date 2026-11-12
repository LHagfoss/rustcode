// The sidebar's project folders, for the "Group by project" pref. The list is one flat list of
// every computer's threads (docs/desktop.md › Several machines); a folder holds one computer's
// threads of one project, so two computers' same-named projects never share a key.

import type { SessionSummary } from "../acp/types";
import { projectName } from "../acp/types";
import { LOCAL_HOST } from "./threadKeys";

/** One project's rows. */
export interface ProjectGroup {
  /** Unique across machines: the project's path cannot collide with another machine's. */
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

/** The rows by project and computer, in the order each first appears. A worktree thread belongs to its repository. */
export function sidebarProjects(items: readonly SessionSummary[]): ProjectGroup[] {
  const projects = new Map<string, { host: string; project: string; rows: SessionSummary[] }>();
  for (const item of items) {
    const host = hostOf(item);
    const project = item.worktree?.repo || item.cwd || "";
    const key = host === LOCAL_HOST ? project : `${host}\u0000${project}`;
    const group = projects.get(key) ?? { host, project, rows: [] };
    group.rows.push(item);
    projects.set(key, group);
  }
  return [...projects].map(([key, { host, project, rows }]) => ({
    key,
    label: project ? projectName(project) : "No folder",
    // A folder on another computer is not one this computer can start a thread in.
    project: host === LOCAL_HOST ? project || undefined : undefined,
    items: rows,
  }));
}
