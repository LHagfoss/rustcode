// The files a folder's `@` picks have pointed at lately, newest first: what the
// drawer offers before it lists the project, since the shell keeps no record of
// which files were touched. Held in memory only — a restart starts the list
// again — and never written anywhere.

/** How many files a folder remembers. */
const KEPT = 20;

const byCwd = new Map<string, string[]>();

/** Remembers that `path`, in `cwd`, is one the reader just pointed at. */
export function noteRecentFile(cwd: string, path: string): void {
  if (!cwd) return;
  const held = byCwd.get(cwd) ?? [];
  byCwd.set(cwd, [path, ...held.filter((known) => known !== path)].slice(0, KEPT));
}

/** The folder's recent files, newest first. */
export function recentFiles(cwd: string): readonly string[] {
  return byCwd.get(cwd) ?? [];
}

/** `paths` with the recent ones first, newest first, and the rest as they came. */
export function recentFirst(paths: readonly string[], recent: readonly string[]): string[] {
  const known = new Set(paths);
  const taken = new Set<string>();
  const out: string[] = [];
  for (const path of recent) {
    if (!known.has(path) || taken.has(path)) continue;
    taken.add(path);
    out.push(path);
  }
  for (const path of paths) if (!taken.has(path)) out.push(path);
  return out;
}

/** Forgets every folder's list (tests, and a fresh mock window). */
export function forgetRecentFiles(): void {
  byCwd.clear();
}
