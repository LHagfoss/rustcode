import type { UsageWindow } from "../acp/rustcode";

export type PlanPace =
  | { kind: "runsOut"; at: number; early: number; evenLeft: number }
  | { kind: "atReset"; percent: number; evenLeft: number };

const EARLIEST_SHARE = 0.1;

/**
 * Where a plan window is heading if it keeps filling at the rate it has so far: empty before the
 * reset, or some share used when it resets. `evenLeft` is the share that would be left at an even
 * pace, for a mark on the meter. Nothing while the window is too young to say, or has no length.
 */
export function planPace(w: UsageWindow, now = Date.now()): PlanPace | undefined {
  if (!w.minutes || !w.resetsAt) return undefined;
  const length = w.minutes * 60_000;
  const elapsed = now - (w.resetsAt - length);
  if (elapsed < length * EARLIEST_SHARE || elapsed >= length) return undefined;
  const used = Math.max(0, Math.min(100, w.usedPercent));
  if (used < 1 || used >= 100) return undefined;
  const evenLeft = Math.round(100 - (elapsed / length) * 100);
  const projected = (used * length) / elapsed;
  if (projected < 100) return { kind: "atReset", percent: Math.round(projected), evenLeft };
  const at = now + ((100 - used) / used) * elapsed;
  return { kind: "runsOut", at, early: w.resetsAt - at, evenLeft };
}

function when(at: number, now: number): string {
  const time = { hour: "2-digit", minute: "2-digit" } as const;
  return new Date(at).toDateString() === new Date(now).toDateString()
    ? `at ${new Date(at).toLocaleTimeString(undefined, time)}`
    : new Date(at).toLocaleString(undefined, { weekday: "short", ...time });
}

function span(ms: number): string {
  const minutes = Math.max(1, Math.round(ms / 60_000));
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.round(minutes / 60);
  return hours < 48 ? `${hours} h` : `${Math.round(hours / 24)} days`;
}

/** "Runs out at 14:20, 2 h early" or "About 38% left at reset". */
export function paceLabel(pace: PlanPace, now = Date.now()): string {
  return pace.kind === "runsOut"
    ? `Runs out ${when(pace.at, now)}, ${span(pace.early)} early`
    : `About ${100 - pace.percent}% left at reset`;
}
