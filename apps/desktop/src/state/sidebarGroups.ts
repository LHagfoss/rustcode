// The sidebar's machine groups: one heading per paired computer, this one first, and the threads
// by project inside it (docs/desktop.md › Several machines). It is a pure function of the rows,
// the machines and the section, so the rule for an empty machine — shown in Threads, skipped in
// Working and Pinned — is testable without a window (state/sidebarGroups.test.ts).

import type { SessionSummary } from "../acp/types";
import { projectName } from "../acp/types";
import type { SectionId } from "./sidebarRows";
import { LOCAL_HOST } from "./threadKeys";

/** What a machine heading says: its name, and how its channel stands. */
export interface MachineHead {
  id: string;
  name: string;
  state: "connecting" | "connected" | "offline" | "needsPairing" | "stopped";
  /** Seconds until the next attempt, while the supervisor waits. */
  retryInSecs: number | null;
}

/** The machines a group can be drawn for: the saved ones, as the store keeps them. */
export type GroupedMachine = MachineHead;

/** One project's rows in one machine's group. */
export interface ProjectGroup {
  /** Unique across machines: the project's path cannot collide with another machine's. */
  key: string;
  label: string;
  items: SessionSummary[];
  /** The folder the group's threads run in: what **New thread** under it starts on. */
  project?: string;
  /** The machine the group belongs to, drawn above its first project. */
  machine?: MachineHead;
  /** The first project of its machine: the heading goes here. */
  first?: boolean;
  /** The machine's only project: its rows are drawn under the machine's name, with no folder. */
  only?: boolean;
}

/** A thread's machine: its host, or this computer when the list came from here. */
export function hostOf(session: Pick<SessionSummary, "host">): string {
  return session.host ?? LOCAL_HOST;
}

/**
 * The machine groups of one section's rows.
 *
 * The Threads list keeps a heading for a machine that has listed no thread yet (away, or just
 * paired), so the list still says it is there. Working and Pinned show work, not machines: an
 * idle machine there would draw a heading with nothing under it, so it is left out.
 */
export function machineGroups(
  section: SectionId,
  items: readonly SessionSummary[],
  machines: readonly GroupedMachine[],
): ProjectGroup[] {
  const keepEmpty = section === "threads";
  const groups: ProjectGroup[] = [];
  // A thread whose machine was just removed is still in the list for a frame: it keeps a group of
  // its own rather than vanishing from the section, which draws only what is grouped here.
  const known = new Set([LOCAL_HOST, ...machines.map((machine) => machine.id)]);
  const extra = [...new Set(items.map(hostOf))].filter((host) => !known.has(host));
  for (const host of [...known, ...extra]) {
    const saved = machines.find((machine) => machine.id === host);
    const head: MachineHead = saved ?? {
      id: host,
      name: host === LOCAL_HOST ? "This computer" : "Another computer",
      state: host === LOCAL_HOST ? "connected" : "offline",
      retryInSecs: null,
    };
    const mine = items.filter((session) => hostOf(session) === host);
    if (!mine.length && (host === LOCAL_HOST || !keepEmpty)) continue;
    if (!mine.length) {
      groups.push({ key: `${host}\u0000`, label: "", items: [], machine: head, first: true, only: true });
      continue;
    }
    // The rows by project: a worktree thread belongs to its repository.
    const perProject = new Map<string, SessionSummary[]>();
    for (const session of mine) {
      const key = session.worktree?.repo || session.cwd || "";
      perProject.set(key, [...(perProject.get(key) ?? []), session]);
    }
    let first = true;
    for (const [project, rows] of perProject) {
      groups.push({
        key: `${host}\u0000${project}`,
        label: project ? projectName(project) : "No folder",
        ...(project ? { project } : {}),
        items: rows,
        machine: head,
        first,
        only: perProject.size === 1,
      });
      first = false;
    }
  }
  return groups;
}
