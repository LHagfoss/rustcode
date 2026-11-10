// The strip of delegates above the composer, folded: what its rows add up to in one line, and
// whether the reader put it away. The fold is a view preference for this session, per thread,
// never part of the thread: reopening the app shows the rows again, which is the state that says
// what the delegates do now.

import type { AgentState } from "./delegates";

/** What the strip's rows count as: the delegates it draws, and those working or waiting. */
export interface StripCounts {
  /** A row for every delegate the strip draws: threads, harness agents and RustCode's own. */
  delegates: number;
  /** Rows working now; a queued delegate reads as working, as its own row does. */
  running: number;
  /** Rows waiting on the user. */
  needs: number;
}

/**
 * The strip's rows added up. A harness's own agent and RustCode's are listed only while they work,
 * so they arrive as `workingAgents` and count as running; the links carry their own state.
 */
export function stripCounts(linkStates: readonly AgentState[], workingAgents = 0): StripCounts {
  return {
    delegates: linkStates.length + workingAgents,
    running: linkStates.filter((state) => state === "working").length + workingAgents,
    needs: linkStates.filter((state) => state === "needs").length,
  };
}

/**
 * The strip's one line: `3 delegates · 2 running`, and `· 1 needs you` beside it. The words after
 * the count are the ones a branch's header uses (`state/delegates.ts`), so the two surfaces never
 * name the same state two ways. A count with nothing live left keeps its first part alone.
 */
export function stripSummary({ delegates, running, needs }: StripCounts): string {
  const count = `${delegates} ${delegates === 1 ? "delegate" : "delegates"}`;
  const doing = [
    running ? `${running} running` : "",
    // `1 needs you`, `2 need you`: the same agreement as the branch header.
    needs ? `${needs} ${needs === 1 ? "needs" : "need"} you` : "",
  ].filter(Boolean);
  return doing.length ? `${count} · ${doing.join(" · ")}` : count;
}

/**
 * Whether the strip draws its header alone, or its header and its rows. Only where there is a
 * header to unfold them with: a fold is remembered per thread, so a strip that held three rows can
 * shrink to one while it is folded, and folding then would draw the card empty and swallow the one
 * delegate left (a fold hides the rows, never the header, so there would be no way back either).
 */
export function stripCollapsed({ delegates }: StripCounts, folded: boolean): boolean {
  return folded && delegates > 1;
}

// Session state, not thread data: a Set that outlives the component, and the listeners that
// redraw it when the reader folds the strip in one thread.
const folded = new Set<string>();
const listeners = new Set<() => void>();

/** Whether this thread's strip is folded to its one line. Unfolded until the reader says otherwise. */
export function stripFolded(threadId: string): boolean {
  return folded.has(threadId);
}

/** Fold or unfold one thread's strip, for the rest of the session. */
export function foldStrip(threadId: string, value: boolean): void {
  if (value === folded.has(threadId)) return;
  if (value) folded.add(threadId);
  else folded.delete(threadId);
  for (const listener of listeners) listener();
}

/** `useSyncExternalStore`'s subscription. The same for every strip, so it is a stable argument. */
export function subscribeStripFold(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
