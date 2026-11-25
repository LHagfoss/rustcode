/**
 * The chat recap card: back in a chat after it went unseen for a while (Settings ›
 * General, an hour by default), a card after its last turn says what the user
 * asked last, where the reply left off and whether the thread still works, waits on
 * them or failed. One chat at a time, like Claude Code's recap; dismissing it puts
 * it away until the next return after a gap.
 */

import { useEffect, useMemo } from "react";
import { chatRecapOf, type RecapState, recapDue } from "../state/recap";
import {
  dismissChatRecap,
  openChatRecap,
  retryThread,
  storeThreadSeen,
  threadSeenAt,
  useApp,
} from "../state/store";
import { relativeAge } from "../state/thread";
import { isRunning } from "../state/turn";
import { Sparkles, X } from "./icons";
import { ChromeButton, cx, Pill } from "./primitives";

/**
 * Watches which chat is on screen and starts its recap when the user comes back
 * to it after a long enough gap: by opening it, or by returning to the window
 * with it open. Mounted once by the shell. A chat counts as seen while it is
 * selected in a visible, focused window; the gap runs from the last such moment.
 */
export function useChatRecapWatch() {
  useEffect(() => {
    const seen = threadSeenAt();
    let current = useApp.getState().selectedId;
    let away = document.visibilityState !== "visible" || !document.hasFocus();
    /** App storage takes a write per few seconds at most: a write per keystroke would put
     * localStorage in the typing path, and the gap is measured in minutes anyway. */
    let written = 0;
    const save = (now: number) => {
      written = now;
      storeThreadSeen(seen);
    };
    const mark = (now: number) => {
      if (current) seen[current] = now;
    };
    const arrive = (now: number) => {
      const s = useApp.getState();
      const since = current ? seen[current] : undefined;
      if (current && s.chatRecaps && recapDue(since, now, s.recapMinutes)) openChatRecap(current, since!);
      else if (s.chatRecap && s.chatRecap.threadId !== current) dismissChatRecap();
      mark(now);
      save(now);
    };
    const leave = () => {
      if (away) return;
      away = true;
      const now = Date.now();
      mark(now);
      save(now);
    };
    const back = () => {
      if (!away || document.visibilityState !== "visible") return;
      away = false;
      arrive(Date.now());
    };
    if (!away) arrive(Date.now());
    const unsubscribe = useApp.subscribe((s, prev) => {
      if (s.selectedId === prev.selectedId) return;
      const now = Date.now();
      if (!away) mark(now);
      current = s.selectedId;
      if (!away) arrive(now);
    });
    const active = () => {
      if (away) return back();
      const now = Date.now();
      mark(now);
      if (now - written >= 5_000) save(now);
    };
    const visible = () => (document.visibilityState === "visible" ? back() : leave());
    window.addEventListener("pointerdown", active);
    window.addEventListener("keydown", active);
    window.addEventListener("focus", back);
    window.addEventListener("blur", leave);
    document.addEventListener("visibilitychange", visible);
    return () => {
      unsubscribe();
      window.removeEventListener("pointerdown", active);
      window.removeEventListener("keydown", active);
      window.removeEventListener("focus", back);
      window.removeEventListener("blur", leave);
      document.removeEventListener("visibilitychange", visible);
      leave();
    };
  }, []);
}

const STATE_LABEL: Record<RecapState, string> = {
  running: "Still working",
  needs: "Waiting on you",
  failed: "The last turn failed",
  done: "Finished",
};

const STATE_TONE: Record<RecapState, string> = {
  running: "text-accent",
  needs: "text-warning",
  failed: "text-danger",
  done: "text-success",
};

/** The recap of the chat on screen: a row after its last turn (ui/Transcript.tsx). */
export function ChatRecapCard({ threadId, className }: { threadId: string; className?: string }) {
  const open = useApp((s) => (s.chatRecaps && s.chatRecap?.threadId === threadId ? s.chatRecap : null));
  const items = useApp((s) => s.threads[threadId]?.items);
  const pending = useApp((s) => s.threads[threadId]?.permissions.length ?? 0);
  const running = useApp((s) => {
    const thread = s.threads[threadId];
    return !!thread && (isRunning(thread) || !!thread.enginePhase);
  });
  const recap = useMemo(
    () => (open && items ? chatRecapOf({ items, pending, running }) : null),
    [open, items, pending, running],
  );
  if (!open || !recap) return null;
  return (
    <section
      aria-label="Recap"
      className={cx(
        "flex flex-col gap-0.5 border-border border-l-2 py-0.5 pl-3 text-caption rc-in",
        className,
      )}
    >
      <div className="flex items-center gap-1.5 text-fg-faint">
        <Sparkles size={12} className="shrink-0 text-accent" />
        <span className="font-medium text-fg-muted">Recap</span>
        <span aria-hidden>·</span>
        <span className={cx("shrink-0", STATE_TONE[recap.state])}>{STATE_LABEL[recap.state]}</span>
        <span aria-hidden>·</span>
        <span className="min-w-0 flex-1 truncate">Last seen {relativeAge(open.since)}</span>
        {recap.state === "failed" && (
          <Pill size="sm" onClick={() => void retryThread(threadId)}>
            Retry
          </Pill>
        )}
        <ChromeButton label="Dismiss the recap" onClick={dismissChatRecap}>
          <X size={12} />
        </ChromeButton>
      </div>
      {recap.asked && <RecapLine label="You asked" text={recap.asked} />}
      {recap.reply && <RecapLine label="Last reply" text={recap.reply} />}
    </section>
  );
}

function RecapLine({ label, text }: { label: string; text: string }) {
  return (
    <p title={text} className="truncate text-fg-muted">
      <span className="text-fg-faint">{label} </span>
      {text}
    </p>
  );
}
