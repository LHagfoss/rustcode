// The live background in the chat (ui/LiveBackdrop.tsx): the whole pane behind a new thread, and
// the band an open thread's composer stands on. A turn ending plays a beat on the band: `done`
// settles onto the composer, `fail` dips the light.

import { type RefObject, useEffect, useRef, useState } from "react";
import { useLiveBackdrop } from "../lib/backdrop/config";
import { fieldBeat } from "../lib/backdrop/events";
import { newThreadChoice, statusOf, useApp } from "../state/store";
import { isRunning } from "../state/thread";
import { LiveBackdrop } from "./LiveBackdrop";

/** Whether any thread in this window is working: a new thread's light drifts quicker meanwhile. */
function useAnyRunning() {
  return useApp((s) => {
    for (const id in s.threads) if (isRunning(s.threads[id])) return true;
    return false;
  });
}

/** Settings is open over the chat: the chat's light holds still under it (its own swatches move). */
function useCovered() {
  return useApp((s) => s.settings !== null);
}

/** Mounted after the first paint, in idle time: the light is never what the screen waits for. */
function useIdleMount() {
  const [ready, setReady] = useState(false);
  useEffect(() => {
    if (typeof requestIdleCallback !== "function") {
      const t = window.setTimeout(() => setReady(true), 120);
      return () => window.clearTimeout(t);
    }
    const id = requestIdleCallback(() => setReady(true), { timeout: 700 });
    return () => cancelIdleCallback(id);
  }, []);
  return ready;
}

export function NewThreadLight() {
  const harness = useApp((s) => newThreadChoice(s)?.harness);
  const busy = useAnyRunning();
  const covered = useCovered();
  const ready = useIdleMount();
  const cfg = useLiveBackdrop();
  if (!ready || cfg.source === "off") return null;
  return (
    <div aria-hidden className="pointer-events-none absolute inset-0">
      <LiveBackdrop surface="new" harness={harness} busy={busy} covered={covered} />
    </div>
  );
}

/**
 * The band behind an open thread's composer. Its box is sized from the composer stack: its middle
 * sits just under the stack's top edge, so the arc passes behind the card and only its glow rises
 * above it.
 */
export function ThreadLight({
  threadId,
  stack,
  stackHeight,
}: {
  threadId: string;
  stack: RefObject<HTMLElement | null>;
  stackHeight: number;
}) {
  const status = useApp((s) => statusOf(s, threadId));
  const harness = useApp((s) => s.threads[threadId]?.harness?.id);
  const failed = useApp((s) => !!s.threads[threadId]?.error);
  const covered = useCovered();
  const ready = useIdleMount();
  const cfg = useLiveBackdrop();

  // The turn's end, as a beat from the composer: only a turn this view watched end, never a
  // thread opened after its turn finished.
  const was = useRef<{ id: string; running: boolean }>({ id: threadId, running: status === "running" });
  useEffect(() => {
    const prev = was.current;
    const running = status === "running";
    was.current = { id: threadId, running };
    if (prev.id !== threadId || !prev.running || running) return;
    fieldBeat(failed ? "fail" : "done", stack.current, 1, "top");
  }, [threadId, status, failed, stack]);

  if (!ready || cfg.source === "off" || cfg.threadAmount <= 0) return null;
  const height = `min(64%, ${Math.round(Math.max(0, stackHeight - 3) * 1.89)}px)`;
  return (
    <div aria-hidden className="rc-live-band rc-fade-in" style={{ height }}>
      <LiveBackdrop surface="thread" harness={harness} status={status} covered={covered} />
    </div>
  );
}
