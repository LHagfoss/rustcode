/**
 * The chat recap card: back in any chat after it went quiet for a while (Settings ›
 * General, an hour by default), a card after its last turn says what the user
 * asked last, where the reply left off and whether the thread still works, waits on
 * them or failed. Each chat keeps its own until it is dismissed or a new message
 * arrives in it.
 */

import { useEffect, useMemo } from "react";
import { chatRecapOf, messageCount, quietSince, type RecapState, recapDue } from "../state/recap";
import {
  dismissChatRecap,
  openChatRecap,
  retryThread,
  settleRecaps,
  storeThreadSeen,
  threadSeenAt,
  useApp,
} from "../state/store";
import { isLoaded, relativeAge } from "../state/thread";
import { isRunning } from "../state/turn";
import { Sparkles, X } from "./icons";
import { ChromeButton, cx, Pill } from "./primitives";

/** A chat's message count once its history is in, for {@link settleRecaps}. */
function loadedMessages(threadId: string): number | undefined {
  const thread = useApp.getState().threads[threadId];
  return isLoaded(thread) ? messageCount(thread.items) : undefined;
}

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
      if (current && s.chatRecaps && !s.recaps[current]) {
        const lastActive = s.sessions.find((x) => x.id === current)?.updatedAt;
        const since = quietSince(seen[current], lastActive);
        if (recapDue(since, now, s.recapMinutes)) openChatRecap(current, since!, loadedMessages(current));
      }
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
      if (s.threads !== prev.threads && Object.keys(s.recaps).length > 0) {
        settleRecaps(loadedMessages);
      }
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
  const open = useApp((s) => (s.chatRecaps ? s.recaps[threadId] : undefined));
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
        "flex flex-col gap-2 rounded-xl border border-border bg-fill-subtle px-3 py-2 text-caption rc-in",
        className,
      )}
    >
      <div className="flex items-center gap-2">
        <Sparkles size={13} className="shrink-0 text-accent" />
        <span className="font-medium text-fg">Recap</span>
        <span className="min-w-0 flex-1 truncate text-fg-faint">Last activity {relativeAge(open.since)}</span>
        <span className={cx("flex shrink-0 items-center gap-1.5", STATE_TONE[recap.state])}>
          <span aria-hidden className="size-1.5 rounded-full bg-current" />
          {STATE_LABEL[recap.state]}
        </span>
        {recap.state === "failed" && (
          <Pill size="sm" onClick={() => void retryThread(threadId)}>
            Retry
          </Pill>
        )}
        <ChromeButton label="Dismiss the recap" onClick={() => dismissChatRecap(threadId)}>
          <X size={12} />
        </ChromeButton>
      </div>
      <dl className="m-0 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1">
        {recap.asked && <RecapLine label="You asked" text={recap.asked} lines={1} />}
        {recap.reply && <RecapLine label="Last reply" text={recap.reply} lines={2} />}
      </dl>
    </section>
  );
}

function RecapLine({ label, text, lines }: { label: string; text: string; lines: 1 | 2 }) {
  return (
    <>
      <dt className="text-fg-faint">{label}</dt>
      <dd title={text} className={cx("m-0 text-fg-muted", lines === 1 ? "truncate" : "line-clamp-2")}>
        {text}
      </dd>
    </>
  );
}
