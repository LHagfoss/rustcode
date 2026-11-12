// The composer's folder picker: one line per folder a thread can start in.
//
// A project is a folder a thread ran in. A folder that only holds other projects is not one
// (the root, a home folder, `~/code`), so it is not offered; a worktree is not a project either:
// it nests under the repo it was cut from. Projects are ranked by how much they are used — the
// newest thread in it and how many it has — and the list is drawn with the most used first. Paths
// are drawn middle-ellipsised, because their ends (the folder's own name, and how deep it sits)
// say more than their middle.

import { plainPath } from "../acp/types";
import { isDataFolder } from "../state/projects";
import { clip } from "./clip";
import { basename, dirname, isInside, normalize } from "./paths";

/** Where a folder was seen: a thread's folder, its worktree, and when that thread last ran. */
export interface FolderSource {
  cwd: string;
  worktree?: { path: string; repo?: string; branch?: string };
  /** When the thread last ran, in ms: the newest one in a project is how recent it is. */
  updatedAt?: number;
}

/** A row of the picker: a project, or a worktree under the project it was cut from. */
export interface FolderRow {
  /** The folder a thread starts in. */
  path: string;
  /** What the row is called: the folder's name, or the worktree's branch. */
  name: string;
  /** Set on a worktree row, which nests under `repo`. */
  repo?: string;
}

/**
 * Worktrees the app cuts itself live under this root, one folder per repo and one per branch, so
 * their repo is readable from the path even when the thread says nothing about it.
 */
const WORKTREE_ROOT = /(?:\/\.config|\/AppData\/Roaming)\/rustcode\/worktrees\/([^/]+)\/([^/]+)$/i;

/** A home folder: a place projects live, never a project, whatever is inside it. */
const HOME_FOLDER = /^(?:\/(?:Users|home)\/[^/]+|[A-Za-z]:[\\/]Users[\\/][^\\/]+)$/;

/**
 * The folder names that are places projects live rather than projects: a folder with one of these
 * names, holding another folder a thread ran in, is a container and not a row of its own.
 */
const CONTAINER_NAMES = new Set([
  "code",
  "dev",
  "src",
  "projects",
  "repos",
  "repositories",
  "workspace",
  "workspaces",
  "work",
  "git",
  "github",
  "developer",
  "documents",
  "desktop",
  "downloads",
]);

const DAY_MS = 86_400_000;

/** How many of a repo's worktrees the list shows while it is not being searched. */
const WORKTREE_ROWS = 3;

/** The last segment, which is what a folder is called. */
function nameOf(path: string): string {
  return basename(path) || path;
}

/** Whether `parent` holds `child` at some depth (a folder is not its own container). */
function holds(parent: string, child: string): boolean {
  return isInside(child, parent) && !isInside(parent, child);
}

/** The folder a path sits in. */
function parentOf(path: string): string {
  return dirname(path);
}

/**
 * Whether `path` is a place projects live rather than a project: the root, a home folder, or a
 * container-named folder (`code`, `dev`, `work`) that sits in a place itself and holds another
 * candidate. `~/code` and `~/code/work` are places; `rustcode/apps/desktop` is a folder inside a
 * project, so it keeps its row, and so does a real project that holds a nested project's threads
 * (`rustcode` and `rustcode/apps/desktop/src-tauri`).
 */
function isPlace(path: string, projects: readonly string[]): boolean {
  if (dirname(path) === normalize(path)) return true;
  if (HOME_FOLDER.test(normalize(path))) return true;
  if (!CONTAINER_NAMES.has(nameOf(path).toLowerCase())) return false;
  // A container only groups the projects under it when it is itself in a place: a folder called
  // `desktop` or `src` deep inside a project is that project's own business.
  return projects.some((other) => holds(path, other)) && isPlace(parentOf(path), projects);
}

/** The repo a worktree path names, when the app cut it (`<root>/<repo>/<branch>`). */
function cutFrom(path: string): { repo: string; branch: string } | undefined {
  const at = WORKTREE_ROOT.exec(normalize(path).replace(/\\/g, "/"));
  return at ? { repo: at[1]!, branch: at[2]! } : undefined;
}

/**
 * How much a project is used, from 0 to 1: 70% how recently it was used, 30% how many threads it
 * holds. Recency halves every two weeks, so a project used today beats one used last week however
 * many threads it has, and volume only separates projects of the same age (51 threads is full).
 * `updatedAt` is the project's newest thread; a project with no date known has no recency.
 */
export function folderScore(
  project: { updatedAt?: number; threads: number },
  now: number = Date.now(),
): number {
  const ageDays = project.updatedAt === undefined ? Infinity : Math.max(0, now - project.updatedAt) / DAY_MS;
  const recency = Number.isFinite(ageDays) ? 2 ** (-ageDays / 14) : 0;
  const volume = Math.min(1, Math.log(1 + project.threads) / Math.log(51));
  return 0.7 * recency + 0.3 * volume;
}

/**
 * The picker's rows, most used project first, each followed by the worktrees cut from it.
 *
 * A folder that is only a place projects live is left out: the root, a home folder, and a
 * container-named folder (`~/code`, `~/dev`, `~/code/work`) that sits in a place and holds another
 * project. A worktree's repo is a project even when no thread ran in the repo itself, so a repo only
 * used through worktrees appears with them nested beneath it. A worktree whose repo is unknown stays
 * a row of its own, after the projects, so a thread that only ever ran in a worktree can still be
 * started again. Without a `filter` a repo shows its three most recently used worktrees, so forty of
 * them cannot push every other project past `limit`; with one, rows match on their name or their
 * path and a worktree also stays under a repo whose name matches, so searching for the repo still
 * shows every branch it has.
 */
/** A source with Windows' extended-length prefix taken off each of its paths. */
function plainSource(source: FolderSource): FolderSource {
  const worktree = source.worktree && {
    ...source.worktree,
    path: source.worktree.path && plainPath(source.worktree.path),
    repo: source.worktree.repo && plainPath(source.worktree.repo),
  };
  return { ...source, cwd: source.cwd && plainPath(source.cwd), worktree };
}

export function folderRows(
  sources: readonly FolderSource[],
  { filter = "", limit = 30, now = Date.now() }: { filter?: string; limit?: number; now?: number } = {},
): FolderRow[] {
  const seen = new Set<string>();
  // Every thread, once per thread: how many ran in a folder, and its newest one. A project's use is
  // its own threads plus those of the worktrees cut from it.
  const threads = new Map<string, number>();
  const newest = new Map<string, number>();
  const projects: string[] = [];
  const worktrees: { path: string; repo: string; branch: string }[] = [];
  for (const raw of sources) {
    // One folder is one row, however Windows spelled it (`\\?\C:\…` is `C:\…`, issue 3425), and
    // RustCode's own data or install folder is never a project, only its worktrees (issue 3524).
    const source = plainSource(raw);
    const path = source.cwd;
    if (!path) continue;
    // Worktrees live inside the data folder too, and are rows; anything else there is not.
    if (isDataFolder(path) && source.worktree?.path !== path && !cutFrom(path)) continue;
    threads.set(path, (threads.get(path) ?? 0) + 1);
    if (source.updatedAt !== undefined) {
      newest.set(path, Math.max(newest.get(path) ?? 0, source.updatedAt));
    }
    if (seen.has(path)) continue;
    seen.add(path);
    const cut = cutFrom(path);
    const worktree = source.worktree;
    if (worktree?.path === path) {
      worktrees.push({
        path,
        repo: worktree.repo ?? "",
        branch: worktree.branch || (cut?.branch ?? nameOf(path)),
      });
    } else if (cut && !worktree) {
      // A thread in a worktree the engine did not record: the path still says which repo it is.
      worktrees.push({ path, repo: "", branch: cut.branch });
    } else {
      projects.push(path);
    }
  }
  // A repo used only through its worktrees is a project too: its worktrees belong under it. A
  // worktree that names itself as its repo is not: it is already a row under the project it was
  // cut from, and promoting it would put the same path on screen twice.
  const worktreePaths = new Set(worktrees.map((worktree) => worktree.path));
  for (const worktree of worktrees) {
    const repo = worktree.repo;
    if (repo && !worktreePaths.has(repo) && !projects.includes(repo)) projects.push(repo);
  }
  // What is left of the projects: no root, no home folder, nothing that only holds other projects.
  const real = projects.filter((path) => !isPlace(path, projects));
  // A worktree nests under its repo: the one the engine named, or else the one named like the
  // folder it was cut from.
  const repoOf = (worktree: { repo: string; branch: string }, path: string) => {
    if (worktree.repo && real.includes(worktree.repo)) return worktree.repo;
    const cut = cutFrom(path);
    return cut ? real.find((project) => nameOf(project) === cut.repo) : undefined;
  };
  const nested = new Map<string, FolderRow[]>();
  const loose: FolderRow[] = [];
  for (const worktree of worktrees) {
    const repo = repoOf(worktree, worktree.path);
    const row = { path: worktree.path, name: worktree.branch, repo };
    if (!repo) {
      loose.push(row);
      continue;
    }
    const rows = nested.get(repo) ?? [];
    rows.push(row);
    nested.set(repo, rows);
  }
  // How much each project is used: its own threads, then those of the worktrees cut from it.
  const use = new Map<string, { threads: number; updatedAt?: number }>();
  for (const path of real) {
    const children = nested.get(path) ?? [];
    let count = 0;
    let at: number | undefined;
    for (const folder of [path, ...children.map((child) => child.path)]) {
      count += threads.get(folder) ?? 0;
      const stamp = newest.get(folder);
      if (stamp !== undefined) at = at === undefined ? stamp : Math.max(at, stamp);
    }
    use.set(path, { threads: count, updatedAt: at });
  }
  const ranked = [...real].sort((a, b) => {
    const byUse = folderScore(use.get(b)!, now) - folderScore(use.get(a)!, now);
    return byUse || nameOf(a).localeCompare(nameOf(b));
  });
  // A repo's branches, most recently used first: the ones with a date go by it, the rest keep the
  // order they came in.
  const recentFirst = (children: readonly FolderRow[]) =>
    [...children].sort((a, b) => (newest.get(b.path) ?? 0) - (newest.get(a.path) ?? 0));
  const asked = filter.trim().toLowerCase();
  const rows: FolderRow[] = [];
  for (const path of ranked) {
    const row: FolderRow = { path, name: nameOf(path) };
    rows.push(row);
    const children = nested.get(path) ?? [];
    // Without a search the list is a summary, not an inventory: a repo with forty worktrees would
    // push every other project past the cap, so its newest three branches stand in for it. A
    // search asks for everything that matches, so it keeps them all.
    for (const child of asked ? children : recentFirst(children).slice(0, WORKTREE_ROWS)) rows.push(child);
  }
  for (const row of loose) rows.push(row);

  if (!asked) return rows.slice(0, limit);
  const matches = (name: string, path: string) =>
    name.toLowerCase().includes(asked) || path.toLowerCase().includes(asked);
  return rows
    .filter((row) => {
      if (matches(row.name, row.path)) return true;
      // A worktree stays while the repo above it is on screen: its branch is part of that story.
      return !!row.repo && matches(nameOf(row.repo), row.repo);
    })
    .slice(0, limit);
}

/**
 * `path` with its middle given up for an ellipsis, keeping both ends: `/Users/lagos/code/rustcode`
 * reads as `/Users/…/rustcode`, and a name too long for the whole budget still keeps its last
 * characters. Cuts land on character boundaries (`clip`), never inside an emoji or an accent.
 */
export function middleEllipsis(path: string, max: number): string {
  // A budget with no room for an ellipsis and a readable end on both sides is a hard cut.
  if (path.length <= max || max < 6) return clip(path, Math.max(0, max));
  const room = max - 1;
  // A little over half to the tail: the folder's own name is what the row is about.
  const tail = Math.ceil(room * 0.55);
  const head = clip(path, Math.max(1, room - tail));
  const start = path.length - tail;
  // Start the tail on a separator when one lands in it, so it reads as folders, not letters.
  const offset = path.slice(start).search(/[\\/]/);
  const at = offset < 0 ? -1 : start + offset;
  const from = at > 0 && at < path.length - 1 ? at : start;
  return `${head}…${path.slice(from)}`;
}

/** `text` in runs, the ones the search matched (`hit`) marked, so a row can show what it answered. */
export function matchSpans(text: string, query: string): { text: string; hit: boolean }[] {
  const asked = query.trim().toLowerCase();
  if (!asked) return [{ text, hit: false }];
  const lower = text.toLowerCase();
  const spans: { text: string; hit: boolean }[] = [];
  let at = 0;
  for (let found = lower.indexOf(asked); found >= 0; found = lower.indexOf(asked, at)) {
    if (found > at) spans.push({ text: text.slice(at, found), hit: false });
    spans.push({ text: text.slice(found, found + asked.length), hit: true });
    at = found + asked.length;
  }
  if (at < text.length) spans.push({ text: text.slice(at), hit: false });
  return spans;
}
