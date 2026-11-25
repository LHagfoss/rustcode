import { useEffect, useLayoutEffect, useRef, useState } from "react";

/** The time, ticking each second while `active`. */
export function useNow(active: boolean) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

/** How long the finished turn's live header holds its check before the fold replaces it. */
const TURN_SETTLE_MS = 400;

/**
 * The turn that just completed: its footer's id, for a moment after `running` falls and a new
 * footer appears, so its live header can settle on a check instead of vanishing. A thread switch,
 * the next turn, or a stop that added no footer clears it.
 */
export function useTurnSettle(
  threadId: string,
  running: boolean,
  endId: string | undefined,
): string | undefined {
  const [settling, setSettling] = useState<string | undefined>(undefined);
  const before = useRef({ threadId, running, endId });
  useLayoutEffect(() => {
    const previous = before.current;
    before.current = { threadId, running, endId };
    if (previous.threadId !== threadId || running) {
      setSettling(undefined);
      return;
    }
    if (!previous.running || !endId || endId === previous.endId) return;
    setSettling(endId);
    const timer = setTimeout(() => setSettling(undefined), TURN_SETTLE_MS);
    return () => clearTimeout(timer);
  }, [threadId, running, endId]);
  return settling;
}
