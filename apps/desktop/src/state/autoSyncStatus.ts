/**
 * What the app's automatic syncs last did with each machine, for Settings › Machines: settings
 * follow (`settingsFollow.ts`, with its files) and thread copies (`mirrorBackfill.ts`). Each keeps
 * one outcome per machine: synced, skipped with why, or failed with the error. Held for this app
 * run only; the next connection writes it again.
 */
import { create } from "zustand";

export type AutoSyncKind = "follow" | "backfill";

export interface AutoSyncStatus {
  outcome: "synced" | "skipped" | "failed";
  at: number;
  detail?: string;
}

export const useAutoSync = create<{
  byMachine: Record<string, Partial<Record<AutoSyncKind, AutoSyncStatus>>>;
}>(() => ({ byMachine: {} }));

/** Keep what a sync of `kind` with machine `id` just did. */
export function recordAutoSync(
  id: string,
  kind: AutoSyncKind,
  outcome: AutoSyncStatus["outcome"],
  detail?: string,
  at = Date.now(),
): void {
  const byMachine = useAutoSync.getState().byMachine;
  const status: AutoSyncStatus = detail ? { outcome, at, detail } : { outcome, at };
  useAutoSync.setState({ byMachine: { ...byMachine, [id]: { ...byMachine[id], [kind]: status } } });
}

/** A short "how long ago" for a status line. */
function ago(at: number, now: number): string {
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  return `${Math.round(hours / 24)} d ago`;
}

/** The line Settings › Machines shows for one status, or null when that sync has not run yet. */
export function autoSyncLine(status: AutoSyncStatus | undefined, now = Date.now()): string | null {
  if (!status) return null;
  const when = ago(status.at, now);
  const detail = status.detail ? `: ${status.detail}` : "";
  if (status.outcome === "synced") return `Synced ${when}${status.detail ? `. ${status.detail}` : ""}`;
  if (status.outcome === "skipped") return `Skipped ${when}${detail}`;
  return `Failed ${when}${detail}`;
}

/** An error as the status line says it. */
export function errorDetail(error: unknown): string {
  if (error instanceof Error) return error.message;
  return typeof error === "string" ? error : String(error);
}
