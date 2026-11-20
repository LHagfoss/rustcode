// The Loop badge on a sidebar row (issue 3937): the thread a loop's run started, or the one thread a
// loop sends every run to. What belongs to which loop comes from the loops list (state/loops.ts).

import { useEffect } from "react";
import { refreshLoopThreads, useLoopThreads } from "../state/loops";
import { Repeat } from "./icons";
import { LinkBadge } from "./Links";
import { Tooltip } from "./primitives";

/** How often the sidebar rereads the loops for its badges; the Loops page updates them on its own. */
const REFRESH_MS = 5 * 60_000;

export function LoopBadge({ threadId }: { threadId: string }) {
  const loop = useLoopThreads((s) => s.byThread[threadId]);
  if (!loop) return null;
  return (
    <Tooltip label={`Run by the loop “${loop}”`}>
      <Repeat size={12} className="shrink-0 text-fg-faint" aria-label={`Run by the loop “${loop}”`} />
    </Tooltip>
  );
}

/** Read the loops once the engine's list is in, and again every few minutes. */
export function useLoopBadges(ready: boolean): void {
  useEffect(() => {
    if (!ready) return;
    void refreshLoopThreads();
    const timer = setInterval(() => void refreshLoopThreads(), REFRESH_MS);
    return () => clearInterval(timer);
  }, [ready]);
}

/** A sidebar row's badges before its title: working for another thread, and run by a loop. */
export function RowBadges({ threadId }: { threadId: string }) {
  return (
    <>
      <LinkBadge threadId={threadId} />
      <LoopBadge threadId={threadId} />
    </>
  );
}
