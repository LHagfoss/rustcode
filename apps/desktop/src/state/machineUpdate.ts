// Updating a paired machine from this one (Settings › Machines › Update, issue 4153).
//
// After a release, a machine nobody sat at stayed on the old version and its row only said
// "update it there". Update asks the machine's engine to install the newest stable release itself
// (`_rustcode/machine/update`, engine/src/acp/machine_update.rs): a server installs it and restarts
// once no turn runs there; a machine whose engine lives inside the RustCode app leaves the request
// for that app, which installs its own update once its threads are idle. The row follows it —
// looking, installing, waiting for N running turns — until the machine reconnects on the new
// version.
//
// A machine whose engine predates the method (it answers -32601) and is not a Mac is updated with
// `rc --update` over `_rustcode/machine/exec`; the server there follows its replaced binary and
// restarts onto it by itself (server/auto_update.rs `watch_binary`). A Mac that old is updated there
// once, as before.

import { create } from "zustand";
import { type JsonObject, obj, RpcError } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { type Machine, machineName, updateAdvice, useMachines } from "./machines";
import { errorText } from "./store";
import { connectionFor } from "./threadMachines";
import { compareVersions } from "./updates";

/** Where an update of another machine stands, as the row shows it. */
export type RemoteUpdate =
  | { phase: "asking" }
  | { phase: "checking" }
  | { phase: "installing"; target?: string }
  | { phase: "restarting"; target?: string; runningTurns?: number }
  /** Left for the RustCode app on that machine. */
  | { phase: "app" }
  | { phase: "upToDate"; target?: string }
  | { phase: "done"; version: string }
  | { phase: "failed"; error: string };

export interface MachineUpdateEntry {
  update: RemoteUpdate;
  /** The version the machine ran when Update was pressed: a newer one means it is done. */
  from: string | null;
  startedAt: number;
}

export const useMachineUpdates = create<{ byMachine: Record<string, MachineUpdateEntry> }>(() => ({
  byMachine: {},
}));

/** How often a running update is asked where it stands. */
export const UPDATE_POLL_MS = 3_000;
/** How long a request answers before it counts as lost. */
const REQUEST_MS = 15_000;
/** `rc --update` on an older machine: a download of a few tens of MB. */
const EXEC_UPDATE_SECS = 300;
/** After this, an update that has not brought the machine back on a new version says so. */
export const UPDATE_DEADLINE_MS = 20 * 60_000;
/** A request the app there has to pick up waits for that app to run, idle: give it longer. */
export const APP_DEADLINE_MS = 6 * 60 * 60_000;

/** The engine's answer, or null for one this app cannot read. */
export function parseRemoteUpdate(value: unknown): RemoteUpdate | null {
  const answer = obj(value);
  if (!answer) return null;
  const target = typeof answer.target === "string" ? answer.target : undefined;
  switch (answer.phase) {
    case "checking":
      return { phase: "checking" };
    case "installing":
      return { phase: "installing", ...(target ? { target } : {}) };
    case "restarting": {
      const running = typeof answer.runningTurns === "number" ? answer.runningTurns : undefined;
      return {
        phase: "restarting",
        ...(target ? { target } : {}),
        ...(running === undefined ? {} : { runningTurns: running }),
      };
    }
    case "app":
      return { phase: "app" };
    case "upToDate":
      return { phase: "upToDate", ...(target ? { target } : {}) };
    case "failed":
      return { phase: "failed", error: typeof answer.error === "string" ? answer.error : "unknown error" };
    case "idle":
      // Asked for status after the server restarted: it no longer remembers; the version will say.
      return null;
    default:
      return null;
  }
}

/** Whether the update is still under way, so the row shows a spinner and keeps asking. */
export function updateRunning(update: RemoteUpdate | undefined): boolean {
  return (
    update?.phase === "asking" ||
    update?.phase === "checking" ||
    update?.phase === "installing" ||
    update?.phase === "restarting" ||
    update?.phase === "app"
  );
}

/**
 * Whether the machine now runs what the update was for: a version newer than the one it had, and
 * at least the release it installed when it said which.
 */
export function updateLanded(entry: MachineUpdateEntry, version: string | null | undefined): boolean {
  if (!version || !updateRunning(entry.update)) return false;
  if (entry.from && compareVersions(version, entry.from) <= 0) return false;
  const target =
    entry.update.phase === "installing" || entry.update.phase === "restarting"
      ? entry.update.target
      : undefined;
  return !target || compareVersions(version, target) >= 0;
}

/** What the row says about the update. */
export function remoteUpdateLine(update: RemoteUpdate, name: string): string {
  switch (update.phase) {
    case "asking":
      return `Asking ${name} to update…`;
    case "checking":
      return `Looking for the latest release on ${name}…`;
    case "installing":
      return update.target
        ? `Installing ${update.target} on ${name}…`
        : `Installing the latest release on ${name}…`;
    case "restarting": {
      const installed = update.target ? `Installed ${update.target}` : "Installed";
      const running = update.runningTurns ?? 0;
      return running > 0
        ? `${installed}. Restarts when ${running === 1 ? "its running turn finishes" : `its ${running} running turns finish`}`
        : `${installed}. Restarting…`;
    }
    case "app":
      return `Asked the RustCode app on ${name} to update. It installs once its threads are idle, the next time it runs`;
    case "upToDate":
      return `${name} already has the latest release${update.target ? ` (${update.target})` : ""}`;
    case "done":
      return `Updated to ${update.version}`;
    case "failed":
      return `Update failed: ${update.error}`;
  }
}

/** The last line of a command's output that says something, for a failure. */
export function lastLine(output: string): string {
  const lines = output
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  return lines[lines.length - 1] ?? "the command failed";
}

function put(id: string, entry: MachineUpdateEntry) {
  useMachineUpdates.setState((s) => ({ byMachine: { ...s.byMachine, [id]: entry } }));
}

function setUpdate(id: string, update: RemoteUpdate) {
  const entry = useMachineUpdates.getState().byMachine[id];
  if (entry) put(id, { ...entry, update });
}

/** Forget what the row says (the line under a finished or failed update). */
export function dismissMachineUpdate(id: string) {
  useMachineUpdates.setState((s) => {
    const { [id]: _, ...rest } = s.byMachine;
    return { byMachine: rest };
  });
}

async function ask(machineId: string, params: JsonObject): Promise<RemoteUpdate | null> {
  const connection = connectionFor(machineId);
  if (!connection) throw new Error("that machine is not connected");
  return parseRemoteUpdate(await connection.request(Method.machineUpdate, params, REQUEST_MS));
}

/**
 * An older machine: `rc --update` there. The server follows its replaced binary and restarts onto
 * it once no turn runs, so this ends at "restarting" and the version says the rest.
 */
async function updateWithCommand(machine: Machine): Promise<RemoteUpdate> {
  const connection = connectionFor(machine.id);
  if (!connection) throw new Error("that machine is not connected");
  setUpdate(machine.id, { phase: "installing" });
  const result = obj(
    await connection.request(
      Method.machineExec,
      { command: "rc --update", timeoutSecs: EXEC_UPDATE_SECS },
      (EXEC_UPDATE_SECS + 15) * 1000,
    ),
  );
  const output = typeof result?.output === "string" ? result.output : "";
  if (result?.success !== true) return { phase: "failed", error: lastLine(output) };
  if (/up to date/i.test(output)) return { phase: "upToDate" };
  return { phase: "restarting" };
}

/** What an older machine that cannot be updated from here says. */
function updateThere(machine: Machine): RemoteUpdate {
  return {
    phase: "failed",
    error: `RustCode ${machine.version ?? "there"} cannot be updated from here. Update it there once: ${updateAdvice(machine.platform)}`,
  };
}

/** Press Update on a machine's row. */
export async function updateMachine(machine: Machine): Promise<void> {
  if (updateRunning(useMachineUpdates.getState().byMachine[machine.id]?.update)) return;
  put(machine.id, { update: { phase: "asking" }, from: machine.version, startedAt: Date.now() });
  let next: RemoteUpdate;
  try {
    next = (await ask(machine.id, { action: "start" })) ?? { phase: "checking" };
  } catch (error) {
    if (!(error instanceof RpcError && error.isMethodNotFound)) {
      next = { phase: "failed", error: errorText(error) };
    } else if (machine.platform.toLowerCase() === "macos") {
      next = updateThere(machine);
    } else {
      try {
        next = await updateWithCommand(machine);
      } catch (inner) {
        next =
          inner instanceof RpcError && inner.isMethodNotFound
            ? updateThere(machine)
            : {
                phase: "failed",
                error: errorText(inner),
              };
      }
    }
  }
  setUpdate(machine.id, next);
  watch();
}

/** Update every connected machine that runs an older RustCode than this app. */
export function updateMachines(machines: readonly Machine[]): void {
  for (const machine of machines) void updateMachine(machine);
}

/**
 * One look at every running update: done when the machine came back on a newer version, else ask
 * it where it stands (a machine that is restarting does not answer; that is expected), and give up
 * with a reason after the deadline.
 */
export async function pollMachineUpdates(now = Date.now()): Promise<void> {
  const machines = useMachines.getState().machines;
  const entries = Object.entries(useMachineUpdates.getState().byMachine);
  await Promise.all(
    entries.map(async ([id, entry]) => {
      if (!updateRunning(entry.update) || entry.update.phase === "asking") return;
      const machine = machines.find((m) => m.id === id);
      if (updateLanded(entry, machine?.version)) {
        setUpdate(id, { phase: "done", version: machine?.version ?? "" });
        return;
      }
      const deadline = entry.update.phase === "app" ? APP_DEADLINE_MS : UPDATE_DEADLINE_MS;
      if (now - entry.startedAt > deadline) {
        setUpdate(id, {
          phase: "failed",
          error: `${machine ? machineName(machine) : "the machine"} has not come back on a new version yet. Its server log (rc server logs) says why.`,
        });
        return;
      }
      if (entry.update.phase === "app" || machine?.state !== "connected") return;
      try {
        const update = await ask(id, { action: "status" });
        // A status from the machine's earlier run, or a restarted one that forgot: keep the row.
        if (update && update.phase !== "upToDate") setUpdate(id, update);
        else if (update?.phase === "upToDate" && entry.update.phase === "checking") setUpdate(id, update);
      } catch {
        // Restarting, or a machine on the old version that only took `rc --update`: the version says.
      }
    }),
  );
}

let timer: ReturnType<typeof setInterval> | undefined;

/** Keep asking while any update runs. */
function watch() {
  if (timer) return;
  timer = setInterval(() => {
    const running = Object.values(useMachineUpdates.getState().byMachine).some((e) =>
      updateRunning(e.update),
    );
    if (!running) {
      clearInterval(timer);
      timer = undefined;
      return;
    }
    void pollMachineUpdates();
  }, UPDATE_POLL_MS);
}
