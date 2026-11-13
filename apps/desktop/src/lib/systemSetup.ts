// System setup (src-tauri/src/system_setup.rs, docs/system-setup.md): the per-OS
// steps that make an installed RustCode behave, each checked before it is
// applied, so running them again changes nothing that is already right.

import { hasShell, shellCall } from "./shell";

/** How a step stands; `needs-you` is one only the user can do, with its command in the message. */
export type SetupStatus = "done" | "pending" | "needs-you" | "skipped" | "failed";

export interface SetupStep {
  id: string;
  title: string;
  about: string;
  /** Runs by itself after an install or an update; the others wait for their button. */
  auto: boolean;
  status: SetupStatus;
  message: string;
  /** When it last ran (RFC 3339), or null. */
  lastRun: string | null;
}

export function systemSetupAvailable(): boolean {
  return hasShell();
}

export function loadSystemSetup(): Promise<SetupStep[]> {
  return shellCall<SetupStep[]>("system_setup_status");
}

/** Run one step, or every automatic one when `step` is omitted; answers the list as it stands after. */
export function runSystemSetup(step?: string): Promise<SetupStep[]> {
  return shellCall<SetupStep[]>("system_setup_run", { step: step ?? null });
}

/** The chip a status shows. */
export function setupChip(status: SetupStatus): { tone: "ok" | "warn" | "bad" | "idle"; label: string } {
  switch (status) {
    case "done":
      return { tone: "ok", label: "Done" };
    case "pending":
      return { tone: "idle", label: "Not done" };
    case "needs-you":
      return { tone: "warn", label: "Needs you" };
    case "failed":
      return { tone: "bad", label: "Failed" };
    case "skipped":
      return { tone: "idle", label: "Not needed" };
  }
}

/**
 * The command a "needs you" message ends with (`… Run: echo 'export …'`,
 * `… with: winget install …`), for its Copy button; null when it names none.
 */
export function commandIn(message: string): string | null {
  const match = /(?:\b[Rr]un|\bwith):\s+(\S[\s\S]*)$/.exec(message);
  return match?.[1]?.trim() || null;
}
