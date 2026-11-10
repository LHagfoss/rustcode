// "Waiting on background work": a thread that runs no turn and is not finished
// either, because the threads it started or messaged still work, or because
// commands its last turn parked on still run. Not a turn — the composer stays
// idle — so the transcript's foot and the sidebar say it in one place. The
// decision is pure, so both, and their tests, read it the same way.

import { useMemo } from "react";
import { stateOf, subtreeQuiet } from "./host";
import { isActive, type Link, type LinkState } from "./orchestration";
import { useApp } from "./store";

/** What {@link waitingOn} reads of the app. */
export interface WaitingRead {
  /** The thread itself. */
  id: string;
  /** A turn of it runs: then it works, and is not waiting. */
  running: boolean;
  /** Commands its last turn parked on (state/thread.ts `waiting`). */
  commands?: number;
  /** Every link in the book. */
  links: readonly Link[];
  /** A link's state as the app shows it (state/host.ts `stateOf`). */
  state: (link: Link) => LinkState;
  /** Whether a link's target owes it nothing more (state/host.ts `subtreeQuiet`). */
  quiet: (target: string) => boolean;
}

/** The background work a thread still waits on. */
export interface Waiting {
  /** The links it waits on, in book order: live, or settled with work still running under them. */
  links: Link[];
  /** How many background commands its last turn parked on. */
  commands: number;
}

/**
 * The one state "waiting on background work": no turn of its own runs, and
 * something it handed out is still going. A link counts while it is live
 * (`queued`, `working`, `needs_user`) and while its target's own subtree is not
 * quiet: a link that settled on a turn which only started the work below it is
 * not an answer yet, which is how `wait_session` reads it too (state/host.ts
 * `wait`). Undefined when nothing is left, so a caller draws nothing.
 */
export function waitingOn(read: WaitingRead): Waiting | undefined {
  if (read.running) return undefined;
  const links = read.links.filter(
    (link) => link.orchestrator === read.id && (isActive(read.state(link)) || !read.quiet(link.target)),
  );
  const commands = read.commands ?? 0;
  return links.length || commands ? { links, commands } : undefined;
}

/** The line itself: `Waiting on 2 threads`, `Waiting on a background command`, or both. */
export function waitingLabel(wait: Waiting): string {
  const parts: string[] = [];
  const n = wait.links.length;
  if (n) parts.push(`${n} thread${n === 1 ? "" : "s"}`);
  if (wait.commands) {
    parts.push(wait.commands === 1 ? "a background command" : `${wait.commands} background commands`);
  }
  return `Waiting on ${parts.join(" and ")}`;
}

/** The threads it waits on, by title: the line's tooltip. */
export const waitingTitle = (wait: Waiting) => wait.links.map((l) => l.targetTitle).join(", ");

/**
 * A thread's background work, read once for the transcript and the sidebar. The
 * store's threads object changes with every streamed chunk, so the turns that
 * run are read as one string: a turn starting or ending somewhere below a link
 * changes whether that link still owes an answer, and no link's own state says
 * so.
 */
export function useWaiting(threadId: string): Waiting | undefined {
  const turns = useApp((s) =>
    Object.keys(s.threads)
      .filter((id) => s.threads[id]!.running)
      .join(" "),
  );
  const running = useMemo(() => new Set(turns.split(" ")), [turns]);
  const commands = useApp((s) => s.threads[threadId]?.waiting ?? 0);
  const links = useApp((s) => s.links.links);
  // The links this thread handed out: the ones `waitingOn` asks about, and no other thread's.
  const mine = useMemo(() => links.filter((link) => link.orchestrator === threadId), [links, threadId]);
  // What each of them shows now, as one string: a link reads its target's own status (`stateOf`), so
  // a delegate starting, asking or finishing changes this even when no turn of this thread's moves.
  const shown = useApp((s) => mine.map((link) => stateOf(s, link)).join(" "));
  const states = useMemo(() => statesOf(mine, shown), [mine, shown]);
  return useMemo(
    () =>
      waitingOn({
        id: threadId,
        running: running.has(threadId),
        commands,
        links: mine,
        state: (link) => states.get(link.id) ?? link.state,
        quiet: subtreeQuiet,
      }),
    [threadId, running, commands, mine, states],
  );
}

/**
 * The links' shown states, read back from the joined reading the hooks watch (`stateOf`, one word
 * per link in book order): the dependency they need, and the answer per link.
 */
function statesOf(links: readonly Link[], shown: string): Map<string, LinkState> {
  const words = shown.split(" ");
  return new Map(links.map((link, i) => [link.id, (words[i] as LinkState | undefined) ?? link.state]));
}

/** What {@link waitingIds} reads of the app: one {@link waitingOn} for every thread at once. */
export interface WaitingIdsRead {
  /** The threads the list draws a row for. */
  ids: Iterable<string>;
  /** The threads whose own turn runs: they work, and are not waiting. */
  running: ReadonlySet<string>;
  /** How many commands each thread's last turn parked on (state/thread.ts `waiting`). */
  parked: ReadonlyMap<string, number>;
  links: readonly Link[];
  state: (link: Link) => LinkState;
  quiet: (target: string) => boolean;
}

/**
 * Every thread waiting on background work, by the rule one row reads ({@link waitingOn}), asked of
 * the whole list in one pass: the sidebar draws these under Working with the threads that run, so a
 * thread that handed work out is not lost among the idle ones. The links are grouped by the thread
 * that handed them out, so a long book costs one pass rather than one per thread.
 */
export function waitingIds(read: WaitingIdsRead): Set<string> {
  const byParent = new Map<string, Link[]>();
  for (const link of read.links) {
    const list = byParent.get(link.orchestrator);
    if (list) list.push(link);
    else byParent.set(link.orchestrator, [link]);
  }
  const waiting = new Set<string>();
  for (const id of read.ids) {
    if (read.running.has(id)) continue;
    const links = byParent.get(id) ?? [];
    const live = links.some((link) => isActive(read.state(link)) || !read.quiet(link.target));
    if (live || (read.parked.get(id) ?? 0) > 0) waiting.add(id);
  }
  return waiting;
}

/**
 * {@link waitingIds} over the whole app, read once for the list rather than once per row: the
 * turns that run and the commands parked on are read as strings, so a streamed chunk changes
 * neither and the set keeps its identity.
 */
export function useWaitingIds(ids: readonly string[]): ReadonlySet<string> {
  const turns = useApp((s) =>
    Object.keys(s.threads)
      .filter((id) => s.threads[id]!.running)
      .join(" "),
  );
  const running = useMemo(() => new Set(turns.split(" ")), [turns]);
  const parked = useApp((s) =>
    Object.keys(s.threads)
      .flatMap((id) => (s.threads[id]!.waiting ? [`${id}:${s.threads[id]!.waiting}`] : []))
      .join(" "),
  );
  const links = useApp((s) => s.links.links);
  // The links' shown states, so a delegate that starts, asks or finishes redraws this set.
  const shown = useApp((s) => links.map((link) => stateOf(s, link)).join(" "));
  const states = useMemo(() => statesOf(links, shown), [links, shown]);
  return useMemo(() => {
    const counts = new Map<string, number>();
    for (const row of parked.split(" ")) {
      if (!row) continue;
      const at = row.indexOf(":");
      counts.set(row.slice(0, at), Number(row.slice(at + 1)));
    }
    return waitingIds({
      ids,
      running,
      parked: counts,
      links,
      state: (link) => states.get(link.id) ?? link.state,
      quiet: subtreeQuiet,
    });
  }, [ids, running, parked, links, states]);
}
