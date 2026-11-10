// `?snoozed=working` in a dev browser: a thread that keeps working while it sleeps, so the row,
// the inbox, the Dock badge and the return can be looked at with no engine of its own. Like
// `?queuePreview` (state/queuePreview.ts) and `?n` (state/updates.ts), it only fills the store: the
// snooze is the app's own (`rustcode.snoozed`), and the turn is the mock's `keep working` prompt.

import { openThread, runPrompt, setSnoozed, useApp } from "./store";

/** How long the staged thread sleeps: long enough that nothing wakes it during a look. */
const HOUR = 3_600_000;

/**
 * Snoozes a thread that is not on screen and sends it the mock's long turn, so it works behind the
 * shut Snoozed shelf and says nothing. Unsnoozing it from its row's menu is the return it stages;
 * `window.__snoozePreview.stage()` stages another one. False until there is a thread to stage on.
 */
export function stageSnoozedWorking(): boolean {
  const s = useApp.getState();
  const row = s.sessions.find((x) => x.id !== s.selectedId) ?? s.sessions[0];
  if (!row) return false;
  void openThread(row.id).then(() => {
    // The turn starts first, so the snooze is the user's wish about a thread already at work: a
    // prompt sent after it would bring the thread back (`runPrompt`).
    void runPrompt(row.id, "keep working");
    setSnoozed(row.id, Date.now() + HOUR);
  });
  return true;
}

/** Installs `?snoozed=working` once the app is ready; any other value of `?snoozed` stages nothing. */
export function installSnoozePreview(): void {
  if (!import.meta.env.DEV) return;
  if (new URLSearchParams(location.search).get("snoozed") !== "working") return;
  Object.assign(window, { __snoozePreview: { stage: stageSnoozedWorking } });
  let done = false;
  const run = () => {
    if (done || useApp.getState().status !== "ready") return;
    // Set before the store notifies its subscribers, so staging cannot happen twice.
    done = true;
    if (stageSnoozedWorking()) off?.();
    else done = false;
  };
  const off = useApp.subscribe(run);
  run();
}
