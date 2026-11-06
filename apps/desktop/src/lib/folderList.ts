// The composer's folder picker: one line per folder a thread can start in.
//
// A project is a folder a thread ran in. A folder that only holds other projects is not one
// (the root, the home folder, `~/code`), so it is not offered; a worktree is not a project either:
// it nests under the repo it was cut from. Paths are drawn middle-ellipsised, because their ends
// (the folder's own name, and how deep it sits) say more than their middle.

import { clip } from "./clip";

/** Where a folder was seen: a thread's folder, and the worktree it belongs to when it is one. */
export interface FolderSource {
  cwd: string;
  worktree?: { path: string; repo?: string; branch?: string };
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
const WORKTREE_ROOT = /\/\.config\/rustcode\/worktrees\/([^/]+)\/([^/]+)$/;

/** The last segment, which is what a folder is called. */
function nameOf(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** Whether `parent` holds `child` at some depth (a folder is not its own container). */
function holds(parent: string, child: string): boolean {
  if (parent === child) return false;
  if (parent === "/") return child.startsWith("/");
  return child.startsWith(`${parent}/`);
}

/** The repo a worktree path names, when the app cut it (`<root>/<repo>/<branch>`). */
function cutFrom(path: string): { repo: string; branch: string } | undefined {
  const at = WORKTREE_ROOT.exec(path);
  return at ? { repo: at[1]!, branch: at[2]! } : undefined;
}

/**
 * The picker's rows, newest folder first, each project followed by the worktrees cut from it.
 *
 * A folder that holds another folder a thread ran in is left out (the root, the home folder,
 * `~/code`): those are places projects live, not projects. A worktree whose repo is not in the list
 * stays a row of its own, so a thread that only ever ran in a worktree can still be started again.
 * With a `filter`, rows match on their name or their path; a worktree also stays under a repo whose
 * name matches, so searching for the repo still shows its branches.
 */
export function folderRows(
  sources: readonly FolderSource[],
  { filter = "", limit = 30 }: { filter?: string; limit?: number } = {},
): FolderRow[] {
  const seen = new Set<string>();
  const projects: string[] = [];
  const worktrees: { path: string; repo: string; branch: string }[] = [];
  for (const source of sources) {
    const path = source.cwd;
    if (!path || seen.has(path)) continue;
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
  // What is left of the projects: no root, and nothing that only holds another of them.
  const real = projects.filter((path) => path !== "/" && !projects.some((other) => holds(path, other)));
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
  const rows: FolderRow[] = [];
  for (const path of real) {
    const row: FolderRow = { path, name: nameOf(path) };
    rows.push(row);
    for (const child of nested.get(path) ?? []) rows.push(child);
  }
  for (const row of loose) rows.push(row);

  const asked = filter.trim().toLowerCase();
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
  // Start the tail on a `/` when one lands in it, so it reads as folders, not letters.
  const at = path.indexOf("/", start);
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
