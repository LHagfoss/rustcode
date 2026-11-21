import { describe, expect, test } from "bun:test";
import {
  lastLine,
  type MachineUpdateEntry,
  parseRemoteUpdate,
  remoteUpdateLine,
  updateLanded,
  updateRunning,
} from "./machineUpdate";

const entry = (update: MachineUpdateEntry["update"], from: string | null = "1.45.0"): MachineUpdateEntry => ({
  update,
  from,
  startedAt: 0,
});

describe("updating a paired machine", () => {
  test("reads the engine's answer", () => {
    expect(parseRemoteUpdate({ phase: "checking", current: "1.45.0" })).toEqual({ phase: "checking" });
    expect(parseRemoteUpdate({ phase: "installing", target: "1.47.0" })).toEqual({
      phase: "installing",
      target: "1.47.0",
    });
    expect(parseRemoteUpdate({ phase: "restarting", target: "1.47.0", runningTurns: 2 })).toEqual({
      phase: "restarting",
      target: "1.47.0",
      runningTurns: 2,
    });
    expect(parseRemoteUpdate({ phase: "app" })).toEqual({ phase: "app" });
    expect(parseRemoteUpdate({ phase: "failed", error: "no network" })).toEqual({
      phase: "failed",
      error: "no network",
    });
    // A restarted server no longer knows; nor is anything else an answer.
    expect(parseRemoteUpdate({ phase: "idle" })).toBeNull();
    expect(parseRemoteUpdate("nope")).toBeNull();
    expect(parseRemoteUpdate({ phase: "later" })).toBeNull();
  });

  test("a running update keeps asking; a finished or failed one does not", () => {
    for (const phase of ["asking", "checking", "installing", "restarting", "app"] as const)
      expect(updateRunning({ phase } as MachineUpdateEntry["update"])).toBe(true);
    expect(updateRunning({ phase: "done", version: "1.47.0" })).toBe(false);
    expect(updateRunning({ phase: "failed", error: "x" })).toBe(false);
    expect(updateRunning({ phase: "upToDate" })).toBe(false);
    expect(updateRunning(undefined)).toBe(false);
  });

  test("lands when the machine comes back newer, and on at least the release it installed", () => {
    const restarting = entry({ phase: "restarting", target: "1.47.0" });
    expect(updateLanded(restarting, "1.45.0")).toBe(false);
    expect(updateLanded(restarting, "1.46.1")).toBe(false);
    expect(updateLanded(restarting, "1.47.0")).toBe(true);
    // No target known (an older machine updated with `rc --update`, or the app there): any newer one.
    expect(updateLanded(entry({ phase: "restarting" }), "1.46.1")).toBe(true);
    expect(updateLanded(entry({ phase: "app" }), "1.47.0")).toBe(true);
    expect(updateLanded(entry({ phase: "app" }), null)).toBe(false);
    // A failed update never lands by itself.
    expect(updateLanded(entry({ phase: "failed", error: "x" }), "1.47.0")).toBe(false);
  });

  test("says where it stands", () => {
    expect(remoteUpdateLine({ phase: "installing", target: "1.47.0" }, "VPS")).toBe(
      "Installing 1.47.0 on VPS…",
    );
    expect(remoteUpdateLine({ phase: "restarting", target: "1.47.0", runningTurns: 2 }, "VPS")).toBe(
      "Installed 1.47.0. Restarts when its 2 running turns finish",
    );
    expect(remoteUpdateLine({ phase: "restarting", target: "1.47.0", runningTurns: 1 }, "VPS")).toBe(
      "Installed 1.47.0. Restarts when its running turn finishes",
    );
    expect(remoteUpdateLine({ phase: "restarting", target: "1.47.0", runningTurns: 0 }, "VPS")).toBe(
      "Installed 1.47.0. Restarting…",
    );
    expect(remoteUpdateLine({ phase: "done", version: "1.47.0" }, "VPS")).toBe("Updated to 1.47.0");
    expect(remoteUpdateLine({ phase: "failed", error: "no network" }, "VPS")).toBe(
      "Update failed: no network",
    );
  });

  test("a failed command is told by its last line", () => {
    expect(lastLine("Checking if new release...\nUpdate failed: HTTP 404\n\n")).toBe(
      "Update failed: HTTP 404",
    );
    expect(lastLine("")).toBe("the command failed");
  });
});
