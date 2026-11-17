// The current time as React state, for labels that count up ("3m ago", "1m 05s").

import { useEffect, useState } from "react";

/**
 * `Date.now()`, ticking every second while `active`. The first render after work starts takes a
 * fresh `now` instead of one from when the view was idle.
 */
export function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1_000);
    return () => clearInterval(timer);
  }, [active]);
  return now;
}

/** `Date.now()`, ticking every `every` ms for as long as the view is mounted. */
export function useClock(every = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), every);
    return () => clearInterval(timer);
  }, [every]);
  return now;
}
