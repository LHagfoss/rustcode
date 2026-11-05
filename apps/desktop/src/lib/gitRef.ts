// The base a branch is compared against, as a label says it: `refs/heads/main`,
// `origin/main` and a full commit sha read `main` and a short sha. The full ref
// stays at hand for a tooltip; the short one is what fits beside a total.

/** A full 40-character commit sha, which no label can show whole. */
const COMMIT = /^[0-9a-f]{40}$/i;
/** The remotes whose name is dropped from a remote-tracking branch. */
const REMOTE = /^(?:origin|upstream)\/(.+)$/;
/** Git's own abbreviation, as `git log --oneline` shows it. */
const SHA_CHARS = 7;

/**
 * The short name of a ref: `refs/heads/main`, `refs/remotes/origin/main` and
 * `origin/main` all read `main`, and a full commit sha reads its first seven
 * characters. A local branch keeps its slash (`feat/login` stays `feat/login`):
 * only a known remote, or an explicit `refs/remotes/` prefix, is dropped.
 */
export function shortRef(ref: string): string {
  let short = ref
    .trim()
    .replace(/^refs\/heads\//, "")
    .replace(/^refs\/tags\//, "");
  short = short.replace(/^refs\/remotes\//, "");
  short = REMOTE.exec(short)?.[1] ?? short;
  if (COMMIT.test(short)) return short.slice(0, SHA_CHARS);
  return short;
}

/**
 * What a changes count compares against, for a label: the branch the worktree
 * was made from when the engine recorded it, else the commit itself. Nothing
 * when neither is known.
 */
export function comparisonBase(
  worktree: { baseRef?: string; base?: string } | undefined,
): string | undefined {
  const ref = worktree?.baseRef?.trim() || worktree?.base?.trim();
  return ref || undefined;
}
