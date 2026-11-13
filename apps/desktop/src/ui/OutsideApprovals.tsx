// An outside app asking to change something (docs/mcp-outside-agents.md ›
// Approval), answered from anywhere in the app. The engine holds the call
// until you answer or two minutes pass; this polls while the app is open and
// something is connected, and draws one card per call in the window's
// bottom-right corner, like a device asking to connect.

import { useEffect } from "react";
import { answerOutsideCall, OUTSIDE_POLL_MS, refreshOutside, useOutside } from "../state/outside";
import { useApp } from "../state/store";
import { Action } from "./settingsKit";

/** At most this many calls are asked about at once. */
const SHOWN = 3;

/** Without a connected app the engine is asked rarely: nothing can be waiting. */
const IDLE_POLL_MS = 30_000;

export function OutsideApprovals() {
  const engine = useApp((s) => s.status);
  const status = useOutside((s) => s.status);
  const connected = (status?.grants.length ?? 0) > 0;

  useEffect(() => {
    if (engine !== "ready") return;
    void refreshOutside();
    const timer = setInterval(() => void refreshOutside(), connected ? OUTSIDE_POLL_MS : IDLE_POLL_MS);
    return () => clearInterval(timer);
  }, [engine, connected]);

  const pending = (status?.pending ?? []).slice(0, SHOWN);
  if (pending.length === 0) return null;
  return (
    <div className="pointer-events-none fixed right-4 bottom-4 z-[80] flex w-[340px] flex-col gap-2">
      {pending.map((item) => (
        <div
          key={item.id}
          role="alertdialog"
          aria-label={`${item.clientName} asks to run ${item.tool}`}
          className="elevated pointer-events-auto flex flex-col gap-2 rounded-2xl p-4 rc-in"
        >
          <div className="font-medium text-body text-fg">
            {item.clientName} wants to run {item.tool}
          </div>
          <div className="selectable break-words text-callout text-fg-muted">{item.summary}</div>
          <div className="flex justify-end gap-2">
            <Action danger onClick={() => void answerOutsideCall(item.id, false)}>
              Decline
            </Action>
            <Action prominent onClick={() => void answerOutsideCall(item.id, true)}>
              Allow
            </Action>
          </div>
        </div>
      ))}
    </div>
  );
}
