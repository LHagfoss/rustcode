import { create } from "zustand";
import type { AcpConnection, Json } from "../acp/jsonrpc";
import { obj } from "../acp/jsonrpc";
import { readStorage, writeStorage } from "../lib/storage";
import { forgetMachine, type Machine, machineLinks, machineName } from "./machines";
import { askConfirm, connectionFor, errorText, toast } from "./store";
import { LOCAL_HOST } from "./threadKeys";

export const REVOKE_MACHINE_METHOD = "_rustcode/remote/revoke_machine";
const PENDING_KEY = "rustcode.pendingRevocations";
const REVOKE_TIMEOUT_MS = 10_000;

/** What became of the revocation on one machine. */
export type RevokeOutcome = "revoked" | "notPaired" | "waiting" | "failed";

export interface RevokeResult {
  name: string;
  outcome: RevokeOutcome;
}

/** A revocation still owed by machines that were offline: the lost computer and who to ask. */
export interface PendingRevocation {
  hostId: string;
  name: string;
  machines: string[];
}

/** The outcome an answer to `_rustcode/remote/revoke_machine` stands for. */
export function answerOutcome(answer: Json): RevokeOutcome {
  const object = obj(answer);
  if (!object || typeof object.error === "string") return "failed";
  return typeof object.revoked === "number" && object.revoked > 0 ? "revoked" : "notPaired";
}

/** The notice after Revoke everywhere: where it took, what waits, and what failed. */
export function revokeSummary(name: string, results: readonly RevokeResult[]): string {
  const names = (outcome: RevokeOutcome) =>
    results.filter((result) => result.outcome === outcome).map((r) => r.name);
  const parts = [`Removed ${name}.`];
  const revoked = names("revoked");
  if (revoked.length > 0) parts.push(`Revoked on ${revoked.join(", ")}.`);
  const waiting = names("waiting");
  if (waiting.length > 0) parts.push(`Waiting for ${waiting.join(", ")} to connect.`);
  const failed = names("failed");
  if (failed.length > 0)
    parts.push(`Could not revoke on ${failed.join(", ")}: revoke it there, or update RustCode there.`);
  return parts.join(" ");
}

/** `list` with `machines` added to the revocation of `hostId`. */
export function addPending(
  list: readonly PendingRevocation[],
  hostId: string,
  name: string,
  machines: readonly string[],
): PendingRevocation[] {
  const earlier = list.find((item) => item.hostId === hostId)?.machines ?? [];
  const merged = [...new Set([...earlier, ...machines])];
  const rest = list.filter((item) => item.hostId !== hostId);
  return merged.length > 0 ? [...rest, { hostId, name, machines: merged }] : rest;
}

/** The revocations a machine still has to be asked for. */
export function pendingFor(list: readonly PendingRevocation[], machineId: string): PendingRevocation[] {
  return list.filter((item) => item.machines.includes(machineId));
}

function readPending(): PendingRevocation[] {
  try {
    const parsed: unknown = JSON.parse(readStorage(PENDING_KEY) ?? "[]");
    if (!Array.isArray(parsed)) return [];
    return parsed.flatMap((item) => {
      const object = obj(item as Json);
      if (!object || typeof object.hostId !== "string" || !Array.isArray(object.machines)) return [];
      const machines = object.machines.filter((id): id is string => typeof id === "string");
      return [{ hostId: object.hostId, name: String(object.name ?? object.hostId), machines }];
    });
  } catch {
    return [];
  }
}

export const usePendingRevocations = create<{ list: PendingRevocation[] }>(() => ({ list: readPending() }));

function savePending(list: PendingRevocation[]) {
  usePendingRevocations.setState({ list });
  writeStorage(PENDING_KEY, list.length > 0 ? JSON.stringify(list) : null);
}

async function ask(connection: AcpConnection, hostId: string): Promise<RevokeOutcome> {
  try {
    return answerOutcome(
      await connection.request<Json>(REVOKE_MACHINE_METHOD, { hostId }, REVOKE_TIMEOUT_MS),
    );
  } catch {
    return "failed";
  }
}

/** The notice after Remove: what the machine said about this computer's access there, then this side. */
export function removedBothWays(name: string, there: string | null, here: RevokeOutcome): string {
  const first = there || `Removed ${name}.`;
  if (here === "revoked") return `${first} ${name} can no longer reach this computer.`;
  if (here === "failed")
    return `${first} ${name} may still reach this computer: revoke it under Remote access › Paired devices.`;
  return first;
}

/** End `hostId`'s own pairings with this computer, so removing a machine ends both directions. */
export async function revokeHere(hostId: string): Promise<RevokeOutcome> {
  const local = connectionFor(LOCAL_HOST);
  return local ? ask(local, hostId) : "failed";
}

/** Revoke `target` on this computer and every connected machine; remember the offline ones. */
export async function revokeEverywhere(target: Machine): Promise<RevokeResult[]> {
  const results: RevokeResult[] = [];
  const local = connectionFor(LOCAL_HOST);
  if (local) results.push({ name: "this computer", outcome: await ask(local, target.id) });
  const waiting: string[] = [];
  for (const link of machineLinks()) {
    if (link.machine.id === target.id) continue;
    const name = machineName(link.machine);
    if (!link.connection) {
      waiting.push(link.machine.id);
      results.push({ name, outcome: "waiting" });
      continue;
    }
    results.push({ name, outcome: await ask(link.connection, target.id) });
  }
  if (waiting.length > 0) {
    savePending(addPending(usePendingRevocations.getState().list, target.id, machineName(target), waiting));
  }
  return results;
}

/** Ask each machine that has come back for the revocations it missed. Run by every fleet sync. */
export async function deliverPendingRevocations(): Promise<void> {
  const pending = usePendingRevocations.getState().list;
  if (pending.length === 0) return;
  const links = machineLinks();
  const next: PendingRevocation[] = [];
  for (const item of pending) {
    const still: string[] = [];
    for (const id of item.machines) {
      const link = links.find((candidate) => candidate.machine.id === id);
      if (!link) continue;
      if (!link.connection) {
        still.push(id);
        continue;
      }
      if ((await ask(link.connection, item.hostId)) === "failed") {
        toast(`Could not revoke ${item.name} on ${machineName(link.machine)}: revoke it there.`, "error");
      }
    }
    if (still.length > 0) next.push({ ...item, machines: still });
  }
  savePending(next);
}

/** Settings › Machines › ‹machine› › Revoke everywhere, after asking. */
export function revokeEverywhereAsk(machine: Machine, onRemoved?: () => void): void {
  const name = machineName(machine);
  askConfirm({
    title: `Revoke ${name} everywhere?`,
    message:
      `For a lost or stolen computer. ${name} loses its access to this computer and to every machine this one is ` +
      `connected to, and is removed here. Machines that are offline are asked when they connect.`,
    action: "Revoke everywhere",
    destructive: true,
    run: () => {
      void (async () => {
        try {
          const results = await revokeEverywhere(machine);
          await forgetMachine(machine);
          const failed = results.some((result) => result.outcome === "failed");
          toast(revokeSummary(name, results), failed ? "error" : "success");
          onRemoved?.();
        } catch (error) {
          toast(errorText(error), "error");
        }
      })();
    },
  });
}
