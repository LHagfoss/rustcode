// The while-you-were-away recap: one card, shown when the app is opened or
// refocused after a long enough break (Settings › Notifications, four hours by
// default). It says what needs you, what failed and what finished, each with the
// one action that settles it. Dismiss puts it away until the next break; the
// inbox keeps a row that brings the same card back.

import { ArrowUpRight, CircleCheck, Hand, RotateCw, Sparkles, X } from "lucide-react";
import { useEffect } from "react";
import { type PermissionRequest, projectName } from "../acp/types";
import { useRecap } from "../state/inboxRead";
import { allowOption } from "../state/permissions";
import { awayDue } from "../state/recap";
import {
  answerPermission,
  beginRecap,
  dismissRecap,
  lastActiveAt,
  markActive,
  retryThread,
  selectThread,
  useApp,
} from "../state/store";
import { relativeAge } from "../state/thread";
import { ProjectMark } from "./ProjectMark";
import { ChromeButton, cx, Pill } from "./primitives";

/**
 * Watches the app's use and starts a recap when a return comes after a long
 * enough break. Mounted once by the shell. A tick records the moment the user
 * leaves; focus (or a window becoming visible again) measures the gap from it.
 */
export function useAwayWatch() {
  useEffect(() => {
    let last = lastActiveAt() || Date.now();
    // App storage takes a write per few seconds at most: a write per keystroke would
    // put localStorage in the typing path, and the gap is measured in hours anyway.
    let written = last;
    const leave = () => {
      last = Date.now();
      written = last;
      markActive(last);
    };
    const check = () => {
      const now = Date.now();
      const s = useApp.getState();
      if (document.visibilityState === "visible" && s.recapWhileAway && awayDue(last, now, s.recapHours)) {
        beginRecap(last);
      }
      last = now;
      written = now;
      markActive(now);
    };
    // A cold start: the stored time is when the app was last used.
    check();
    const active = () => {
      last = Date.now();
      if (last - written < 5_000) return;
      written = last;
      markActive(last);
    };
    const visible = () => {
      if (document.visibilityState === "visible") check();
      else leave();
    };
    window.addEventListener("pointerdown", active);
    window.addEventListener("keydown", active);
    window.addEventListener("focus", check);
    window.addEventListener("blur", leave);
    document.addEventListener("visibilitychange", visible);
    return () => {
      window.removeEventListener("pointerdown", active);
      window.removeEventListener("keydown", active);
      window.removeEventListener("focus", check);
      window.removeEventListener("blur", leave);
      document.removeEventListener("visibilitychange", visible);
      leave();
    };
  }, []);
}

function GroupLabel({ children, n }: { children: React.ReactNode; n: number }) {
  return (
    <div className="flex items-center gap-1.5 text-caption text-fg-muted">
      {children}
      <span className="text-fg-faint tabular">{n}</span>
    </div>
  );
}

/** The one thing a recap row offers: approve/open for what needs you, Retry, Open. */
function rowAction(
  kind: "need" | "failed" | "done",
  threadId: string,
  request?: PermissionRequest,
): { label: string; run: () => void } {
  if (kind === "failed") return { label: "Retry", run: () => void retryThread(threadId) };
  if (kind === "done") return { label: "Open", run: () => selectThread(threadId) };
  if (request && !request.question) {
    const allow = allowOption(request);
    if (allow) return { label: "Approve", run: () => answerPermission(request, allow) };
  }
  return { label: "Open", run: () => selectThread(threadId) };
}

/**
 * The recap card. It floats over the chat and sits in the inbox's own flow:
 * the same content in both places, so dismissing one dismisses both.
 */
export function WhileAwayCard({ className }: { className?: string }) {
  const recap = useRecap();
  if (!recap) return null;
  const more = (n: number) => Math.max(0, n - 3);
  return (
    <section
      aria-label="While you were away"
      className={cx("elevated flex flex-col gap-2 rounded-xl p-4 rc-in", className)}
    >
      <div className="flex items-center gap-2">
        <Sparkles size={14} className="shrink-0 text-accent" />
        <span className="min-w-0 flex-1 truncate font-semibold text-body">While you were away</span>
        <span className="shrink-0 text-caption text-fg-faint">{relativeAge(recap.since)}</span>
        <ChromeButton label="Dismiss the recap" onClick={dismissRecap}>
          <X size={13} />
        </ChromeButton>
      </div>

      {recap.needs.length > 0 && (
        <div className="flex flex-col gap-1">
          <GroupLabel n={recap.needs.length}>
            <Hand size={12} className="text-warning" /> Needs you
          </GroupLabel>
          {recap.needs.slice(0, 3).map((item) => {
            const action = rowAction("need", item.threadId, item.request);
            return (
              <div key={item.key} className="flex items-center gap-2 rounded-lg px-1 py-0.5">
                <ProjectMark name={projectName(item.project)} size={14} />
                <span className="min-w-0 flex-1 truncate text-callout">
                  <span className="font-medium text-fg">{item.title}</span>
                  <span className="text-fg-muted"> · {item.detail}</span>
                </span>
                <Pill size="sm" onClick={action.run}>
                  {action.label}
                </Pill>
              </div>
            );
          })}
          {more(recap.needs.length) > 0 && (
            <span className="px-1 text-caption text-fg-faint">
              +{more(recap.needs.length)} more in the inbox
            </span>
          )}
        </div>
      )}

      {recap.failed.length > 0 && (
        <div className="flex flex-col gap-1">
          <GroupLabel n={recap.failed.length}>
            <RotateCw size={12} className="text-danger" /> Failed
          </GroupLabel>
          {recap.failed.slice(0, 3).map((item) => (
            <div key={item.threadId} className="flex items-center gap-2 rounded-lg px-1 py-0.5">
              <ProjectMark name={projectName(item.project)} size={14} />
              <span className="min-w-0 flex-1 truncate text-callout font-medium text-fg">{item.title}</span>
              <Pill size="sm" onClick={() => void retryThread(item.threadId)}>
                Retry
              </Pill>
            </div>
          ))}
          {more(recap.failed.length) > 0 && (
            <span className="px-1 text-caption text-fg-faint">
              +{more(recap.failed.length)} more in the inbox
            </span>
          )}
        </div>
      )}

      {recap.done.length > 0 && (
        <div className="flex flex-col gap-1">
          <GroupLabel n={recap.done.length}>
            <CircleCheck size={12} className="text-success" /> Done
          </GroupLabel>
          {recap.done.slice(0, 3).map((item) => (
            <div key={item.threadId} className="flex items-center gap-2 rounded-lg px-1 py-0.5">
              <ProjectMark name={projectName(item.project)} size={14} />
              <span className="min-w-0 flex-1 truncate text-callout font-medium text-fg">{item.title}</span>
              <Pill size="sm" onClick={() => selectThread(item.threadId)}>
                Open <ArrowUpRight size={11} />
              </Pill>
            </div>
          ))}
          {more(recap.done.length) > 0 && (
            <span className="px-1 text-caption text-fg-faint">
              +{more(recap.done.length)} more in the inbox
            </span>
          )}
        </div>
      )}
    </section>
  );
}
