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
  return useMemo(
    () =>
      waitingOn({
        id: threadId,
        running: running.has(threadId),
        commands,
        links,
        state: (link) => stateOf(useApp.getState(), link),
        quiet: subtreeQuiet,
      }),
    [threadId, running, commands, links],
  );
}
