// A read that starts before the click: when the pointer or the keyboard rests
// on something for `INTENT_REST_MS`, a screen may read what the click would
// need (a pull request's detail, its diff). Passing over reads nothing, and the
// read goes through the same cache key as the click, so it is never wasted.

import { useCallback, useEffect, useRef } from "react";

/** How long the pointer or the focus rests before an intent counts. */
export const INTENT_REST_MS = 150;

export interface IntentRest<T> {
  /** The pointer or focus arrived: reads once it has rested here. */
  start: (value: T) => void;
  /** The pointer or focus left: no read. */
  cancel: () => void;
}

/** Runs `onIntent` when the pointer or focus rests on the handlers' element. */
export function useIntentRest<T>(onIntent: (value: T) => void): IntentRest<T> {
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const cancel = useCallback(() => {
    clearTimeout(timer.current);
    timer.current = undefined;
  }, []);
  const start = useCallback(
    (value: T) => {
      cancel();
      timer.current = setTimeout(() => {
        timer.current = undefined;
        onIntent(value);
      }, INTENT_REST_MS);
    },
    [cancel, onIntent],
  );
  // Leaving the screen cancels: a timer must not read for a panel that is gone.
  useEffect(() => cancel, [cancel]);
  return { start, cancel };
}
