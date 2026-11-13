// The environment fleet: every computer running RustCode that this one's pairings reach, and
// what each is called (docs/remote.md › Environment names and the fleet).
//
// Pairing is trust between two computers; this adds none. The app asks its own engine and every
// connected environment for `_rustcode/environment/sync`, which carries only names, platforms,
// versions and when a computer was last seen, and merges what comes back (the engine does the
// merge, `rustcode/engine/src/fleet.rs`): the newest rename of a computer wins, so a name chosen on
// one computer shows on all of them, and one that was offline picks it up the next time it syncs.
// A computer that is only *known* through another is listed read-only, with a Pair button.
//
// This module only orchestrates: it never decides which name is newer for a paired machine (the
// registry and the engine do, with the same rule) and never touches a token.

import { create } from "zustand";
import { type AcpConnection, arr, type Json, type JsonObject, obj, str } from "../acp/jsonrpc";
import { hasShell, shellCallWithin } from "../lib/shell";
import {
  cleanMachineName,
  type Machine,
  type MachineIdentity,
  machineLinks,
  machineName,
  refreshMachines,
  setLocalIdentity,
  useMachines,
} from "./machines";
import { connectionFor } from "./store";
import { LOCAL_HOST } from "./threadKeys";

export const FLEET_SYNC_METHOD = "_rustcode/environment/sync";

/** One computer, as a sync reports it. Times are ISO strings. */
export interface FleetEntry {
  hostId: string;
  name: string;
  /** When `name` was chosen by a person; null for the name the system gave. */
  nameUpdatedAt: string | null;
  platform: string;
  version: string | null;
  /** When some computer last had word from it. */
  lastSeenAt: string | null;
  /** The computer this one was learned through, when it did not tell us itself. */
  via: string | null;
}

/** What a sync answers: the computer itself and everything it knows of the others. */
export interface FleetView {
  host: FleetEntry;
  /** The name the system gives that computer, which clearing a rename returns to. */
  osName: string | null;
  entries: FleetEntry[];
}

export function parseFleetEntry(value: Json | undefined): FleetEntry | null {
  const entry = obj(value);
  const hostId = str(entry?.hostId);
  const name = str(entry?.name);
  if (!entry || !hostId || !name) return null;
  return {
    hostId,
    name,
    nameUpdatedAt: str(entry.nameUpdatedAt) ?? null,
    platform: str(entry.platform) ?? "unknown",
    version: str(entry.version) ?? null,
    lastSeenAt: str(entry.lastSeenAt) ?? null,
    via: str(entry.via) ?? null,
  };
}

/** The host entry may have an empty id (a computer no gateway ever ran on), so it is read apart. */
export function parseFleetView(value: Json | undefined): FleetView | null {
  const root = obj(value);
  const host = obj(root?.host);
  if (!root || !host) return null;
  const hostId = str(host.hostId) ?? "";
  return {
    host: {
      hostId,
      name: str(host.name) ?? "",
      nameUpdatedAt: str(host.nameUpdatedAt) ?? null,
      platform: str(host.platform) ?? "unknown",
      version: str(host.version) ?? null,
      lastSeenAt: str(host.lastSeenAt) ?? null,
      via: null,
    },
    osName: str(root.osName) ?? null,
    entries: arr(root.entries)
      .map((entry) => parseFleetEntry(entry))
      .filter((entry): entry is FleetEntry => entry !== null),
  };
}

/** A fleet entry as the wire carries it: absent fields left out, no `via` (the receiver derives it). */
export function wireEntry(entry: FleetEntry): JsonObject {
  const wire: JsonObject = { hostId: entry.hostId, name: entry.name, platform: entry.platform };
  if (entry.nameUpdatedAt) wire.nameUpdatedAt = entry.nameUpdatedAt;
  if (entry.version) wire.version = entry.version;
  if (entry.lastSeenAt) wire.lastSeenAt = entry.lastSeenAt;
  return wire;
}

// ---------------------------------------------------------------- what a sync decides

/** The environments this computer is not paired with, known through another: read-only rows. */
export function knownOnly(
  entries: readonly FleetEntry[],
  machines: readonly Pick<Machine, "id">[],
): FleetEntry[] {
  const paired = new Set(machines.map((machine) => machine.id));
  return entries
    .filter((entry) => !paired.has(entry.hostId))
    .sort((left, right) => left.name.localeCompare(right.name) || left.hostId.localeCompare(right.hostId));
}

/** The name of the computer an entry was learned through: a paired machine's, else its own entry's. */
export function viaName(
  entry: Pick<FleetEntry, "via">,
  machines: readonly Pick<Machine, "id" | "name" | "displayName">[],
  entries: readonly FleetEntry[],
): string | null {
  if (!entry.via) return null;
  const machine = machines.find((saved) => saved.id === entry.via);
  if (machine) return machineName(machine);
  return entries.find((known) => known.hostId === entry.via)?.name ?? null;
}

/**
 * The names to hand the shell: every saved machine the fleet has a rename for whose name differs
 * from the one shown. The registry applies one only when it is newer than what it has saved, so
 * an old rename arriving again changes nothing.
 */
export function fleetNameUpdates(
  entries: readonly FleetEntry[],
  machines: readonly Pick<Machine, "id" | "name" | "displayName">[],
): { id: string; name: string; updatedAt: string }[] {
  const updates: { id: string; name: string; updatedAt: string }[] = [];
  for (const machine of machines) {
    const entry = entries.find((known) => known.hostId === machine.id);
    if (!entry?.nameUpdatedAt) continue;
    if (entry.name !== machineName(machine)) {
      updates.push({ id: machine.id, name: entry.name, updatedAt: entry.nameUpdatedAt });
    }
  }
  return updates;
}

/**
 * What to do about this computer's own name, given what its engine holds: push ours when it is the
 * newer rename, adopt the engine's when another computer renamed this one more recently.
 */
export type LocalNameStep =
  | { kind: "same" }
  | { kind: "push"; entry: FleetEntry }
  | { kind: "adopt"; name: string | null; updatedAt: string };

export function reconcileLocalName(
  identity: Pick<MachineIdentity, "displayName" | "nameUpdatedAt">,
  view: Pick<FleetView, "host" | "osName">,
): LocalNameStep {
  const ours = identity.nameUpdatedAt ?? null;
  const theirs = view.host.nameUpdatedAt;
  if (ours && (!theirs || ours > theirs) && view.host.hostId) {
    const name = cleanMachineName(identity.displayName) ?? view.osName;
    // A cleared name with no system name to return to has nothing to say.
    if (!name) return { kind: "same" };
    if (name === view.host.name && !theirs) return { kind: "same" };
    return { kind: "push", entry: { ...view.host, name, nameUpdatedAt: ours } };
  }
  if (theirs && (!ours || theirs > ours)) {
    // The system's own name is the default, so it shows as no name.
    const name = view.osName && view.host.name === view.osName ? null : view.host.name;
    if (name === (identity.displayName ?? null) && ours === theirs) return { kind: "same" };
    return { kind: "adopt", name, updatedAt: theirs };
  }
  return { kind: "same" };
}

// ---------------------------------------------------------------- the store

interface FleetStore {
  /** This computer, as its engine reports itself. */
  host: FleetEntry | null;
  /** Every other computer its engine knows of, paired or not. */
  entries: FleetEntry[];
  syncedAt: number | null;
  syncing: boolean;
}

export const useFleet = create<FleetStore>(() => ({
  host: null,
  entries: [],
  syncedAt: null,
  syncing: false,
}));

/** How long one sync request may take; a computer that is slow is tried again at the next sync. */
const SYNC_TIMEOUT_MS = 10_000;

let running = false;
let again = false;

/**
 * Exchange names with this computer's engine and every connected environment, then apply what the
 * fleet agreed on: the saved names of paired machines and this computer's own. Away or older
 * environments are skipped; the next sync (a reconnect, a rename, a minute later) reaches them.
 */
export async function syncFleet(): Promise<void> {
  if (running) {
    again = true;
    return;
  }
  running = true;
  useFleet.setState({ syncing: true });
  try {
    do {
      again = false;
      await syncOnce();
    } while (again);
  } catch {
    // The engine did not answer; the next sync tries again.
  } finally {
    running = false;
    useFleet.setState({ syncing: false });
  }
}

async function ask(connection: AcpConnection | null, params: JsonObject): Promise<FleetView | null> {
  if (!connection) return null;
  return parseFleetView(await connection.request<Json>(FLEET_SYNC_METHOD, params, SYNC_TIMEOUT_MS));
}

async function syncOnce(): Promise<void> {
  const local = connectionFor(LOCAL_HOST);
  let view = await ask(local, {});
  if (!view) return;

  // This computer's own rename goes to its engine first, so every peer below hears it.
  const step = reconcileLocalName(useMachines.getState().local, view);
  if (step.kind === "push") {
    view = (await ask(local, { entries: [wireEntry(step.entry)] })) ?? view;
  }

  for (const link of machineLinks()) {
    if (!link.connection) continue;
    try {
      const remote = await ask(link.connection, {
        from: wireEntry(view.host),
        entries: view.entries.map(wireEntry),
      });
      // An answer from a computer that is not the one saved under this id is not merged.
      if (!remote || (remote.host.hostId && remote.host.hostId !== link.machine.id)) continue;
      view =
        (await ask(local, { from: wireEntry(remote.host), entries: remote.entries.map(wireEntry) })) ?? view;
    } catch {
      // Away, older than this protocol, or slow: caught up at the next sync.
    }
  }

  const machines = useMachines.getState().machines;
  if (hasShell()) {
    const names = fleetNameUpdates(view.entries, machines);
    if (names.length > 0) {
      try {
        const changed = await shellCallWithin<string[]>(SYNC_TIMEOUT_MS, "machines_apply_fleet", { names });
        if (changed.length > 0) await refreshMachines();
      } catch {
        // The registry could not be written; the engine still has the names.
      }
    }
  }
  const adopt = reconcileLocalName(useMachines.getState().local, view);
  if (adopt.kind === "adopt") setLocalIdentity({ displayName: adopt.name, nameUpdatedAt: adopt.updatedAt });

  useFleet.setState({ host: view.host, entries: view.entries, syncedAt: Date.now() });
}

// ---------------------------------------------------------------- when it runs

/** How often a sync repeats while the app is open: names, versions and last-seen times drift. */
const SYNC_EVERY_MS = 60_000;
/** A burst of changes (several machines connecting at once) becomes one sync. */
const SETTLE_MS = 800;

/** What changes should start a sync: a rename here, a machine's name or its link coming up or down. */
export function fleetSignature(
  local: Pick<MachineIdentity, "displayName" | "nameUpdatedAt">,
  machines: readonly Pick<Machine, "id" | "displayName" | "state">[],
): string {
  return JSON.stringify([
    local.displayName ?? null,
    local.nameUpdatedAt ?? null,
    machines.map((machine) => [machine.id, machine.displayName ?? null, machine.state === "connected"]),
  ]);
}

let started = false;

/** Start syncing the fleet: once the app has its engine, then on every change and every minute. */
export function startFleetSync(): void {
  if (started) return;
  started = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const soon = () => {
    clearTimeout(timer);
    timer = setTimeout(() => void syncFleet(), SETTLE_MS);
  };
  let signature = fleetSignature(useMachines.getState().local, useMachines.getState().machines);
  useMachines.subscribe((state) => {
    const next = fleetSignature(state.local, state.machines);
    if (next === signature) return;
    signature = next;
    soon();
  });
  setInterval(() => void syncFleet(), SYNC_EVERY_MS);
  soon();
}
