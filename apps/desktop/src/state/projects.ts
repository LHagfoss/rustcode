// Finding the same project on another computer (docs/desktop.md › Several machines).
//
// A project is a folder threads start in, and every machine names its own folders. The same
// repository checked out on two computers has two paths, so the composer's **Run on** cannot
// carry a folder over: it has to find it again. The rules, in order: the same path, then the
// same git remote (the project's identity, whatever the folder is called and however the URL is
// spelled), then a folder with the same name. A machine reports a project's remote only when its
// thread list says one (`_meta.rustcode.remote`), so a machine that does not answer it falls back
// to the folder's name.
//
// Pure, so the store and its tests share one answer (state/projects.test.ts).

import { plainPath, projectName, type SessionSummary } from "../acp/types";
import { LOCAL_HOST } from "./threadKeys";

/** A project on one computer: the folder a thread starts in, its name, and its git remote. */
export interface Project {
  /** The folder threads start in: a worktree's repository, or the thread's own folder. */
  path: string;
  /** What people call it: the last part of its path. */
  name: string;
  /** The git remote the project's threads report, when the machine reports one. */
  remote?: string;
}

/**
 * A git remote URL as one repository name, so two checkouts of it compare equal however each was
 * spelled: `git@github.com:LHagfoss/rustcode.git`, `https://github.com/lhagfoss/rustcode` and
 * `ssh://git@github.com/LHagfoss/rustcode` are all `github.com/lhagfoss/rustcode`. A remote that
 * is a path keeps its path, minus the leading slash and `.git`; anything empty is nothing.
 */
export function remoteRepository(remote: string | null | undefined): string | null {
  const trimmed = remote?.trim();
  if (!trimmed) return null;
  let text = trimmed.replace(/\/+$/, "").replace(/\.git$/i, "");
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(text)) {
    try {
      const url = new URL(text);
      text = `${url.hostname}${url.pathname}`;
    } catch {
      return null;
    }
  } else if (/^[^/]+@[^/]+:/.test(text)) {
    // The scp-like form git prints: `user@host:owner/name`.
    text = text.replace(/^[^@/]*@/, "").replace(":", "/");
  } else {
    text = text.replace(/^[^@/]*@/, "");
  }
  text = text.replace(/^\/+/, "").replace(/\/+$/, "");
  return text ? text.toLowerCase() : null;
}

/**
 * Whether a folder is a filesystem root: `/`, `C:\`, a drive on its own. No project lives at one,
 * so a thread that listed it had no folder of its own to begin with (a service manager's working
 * directory, the app's install folder), which is issue 3425.
 */
export function isRootFolder(path: string): boolean {
  const plain = plainPath(path).trim();
  return plain !== "" && /^(?:[A-Za-z]:)?[\\/]*$/.test(plain);
}

/**
 * The projects a machine's thread list names, most recently used first. A worktree thread names
 * its repository, as the sidebar groups it; two threads in one project are one row. A thread with
 * no recorded folder of its own is not one: its machine lists a stand-in such as its home folder,
 * which made "no project" a project a thread could be started in (issue 3425).
 */
export function projectsOn(sessions: readonly SessionSummary[], host = LOCAL_HOST): Project[] {
  const projects = new Map<string, { project: Project; updatedAt: number }>();
  for (const session of sessions) {
    if ((session.host ?? LOCAL_HOST) !== host) continue;
    if (session.workspaceUnknown) continue;
    // One folder is one project, however Windows spelled it: the verbatim prefix an older engine
    // stored is not part of the path (issue 3425).
    const path = plainPath(session.worktree?.repo || session.repo || session.cwd);
    if (!path || isRootFolder(path)) continue;
    const updatedAt = session.updatedAt ?? 0;
    const known = projects.get(path);
    if (known) {
      known.updatedAt = Math.max(known.updatedAt, updatedAt);
      known.project.remote ??= session.remote;
      continue;
    }
    projects.set(path, {
      project: { path, name: projectName(path), remote: session.remote },
      updatedAt,
    });
  }
  return [...projects.values()].sort((a, b) => b.updatedAt - a.updatedAt).map((entry) => entry.project);
}

/** The project a machine has with this folder, exactly, or nothing. */
export function projectAt(
  sessions: readonly SessionSummary[],
  path: string,
  host = LOCAL_HOST,
): Project | undefined {
  return projectsOn(sessions, host).find((project) => project.path === path);
}

/**
 * The project on another machine that is the same project as `current`: the same path first,
 * then the same git remote, then a folder with the same name as the last resort. Nothing when
 * the other machine has none of those.
 */
export function matchProject(
  current: Project | null | undefined,
  candidates: readonly Project[],
): Project | undefined {
  if (!current) return undefined;
  const exact = candidates.find((candidate) => candidate.path === current.path);
  if (exact) return exact;
  const repository = remoteRepository(current.remote);
  if (repository) {
    const same = candidates.find((candidate) => remoteRepository(candidate.remote) === repository);
    if (same) return same;
  }
  const name = current.name.trim().toLowerCase();
  if (!name) return undefined;
  return candidates.find((candidate) => candidate.name.trim().toLowerCase() === name);
}
