import { type AcpConnection, type Inbound, type JsonObject, str } from "../acp/jsonrpc";
import { type HarnessSnapshot, Method } from "../acp/types";
import type { AppState } from "./appState";
import { resumeLostTurns } from "./lostTurns";
import { saveTranscript } from "./machineCache";
import { mirrorTranscript, setMirrorRequest } from "./machineMirror";
import { mergeMachineLists } from "./machineSync";
import {
  machineLinks,
  machineName,
  onMachineLinks,
  onMachineThreadsReady,
  resetMachinesForTests as resetMachineStoreForTests,
  useMachines,
} from "./machines";
import { MACHINE_LOST_TURN } from "./orchestration";
import { subscribePullWatches } from "./pullWatch";
import { connection, handleInbound, useApp } from "./store";
import { endTurn, finishTurn, isLoaded, isRunning } from "./thread";
import { isRemoteThread, LOCAL_HOST, splitThreadKey, threadKey } from "./threadKeys";
import { loadMachineHarnesses, openThread } from "./threadOpen";
import { subscribeWorkflows } from "./workflows";

// Threads on paired computers: each machine's connection, routing a thread's calls to it, and
// keeping a remote thread's transcript live. Split out of store.ts, which re-exports it.

// ---------------------------------------------------------------- machines
//
// A thread on a paired computer is driven over that computer's own connection
// (state/machines.ts holds one per machine, the shell's supervisor keeps it).
// Local threads keep their bare id and this module's `connection`; a remote
// thread's key carries its machine (`state/threadKeys.ts`), and every call
// that names it on the machine goes through `connectionForThread` with the
// key split back into the machine's own session id. Responses to a machine's
// own requests go back on the same connection they arrived on.

const machineConnections = new Map<string, AcpConnection>();
const machineOff = new Map<string, () => void>();
let machineRouting: Promise<void> | null = null;
/** The machine store's own subscription, kept so a test reset can let it go. */
let machineMirrorOff: (() => void) | null = null;
let transcriptKeepOff: (() => void) | null = null;
/** Threads whose transcript is waiting to be kept on this computer, by key. */
const transcriptTimers = new Map<string, ReturnType<typeof setTimeout>>();
/** A remote thread's transcript is kept at most this often while it changes. */
const KEEP_TRANSCRIPT_MS = 2_000;

/**
 * Keep the transcript of a thread opened on a machine (`state/machineCache.ts`), so it can be read
 * while the machine is away. Only a copy the machine just gave is kept: one drawn from this
 * computer's own copy is already what is kept.
 */
function keepRemoteTranscript(key: string): void {
  transcriptTimers.delete(key);
  const thread = useApp.getState().threads[key];
  if (!isLoaded(thread) || !machineConnections.has(splitThreadKey(key).host)) return;
  saveTranscript(thread, key);
  // The same read also goes to this computer's disk, where nothing ages out (docs/data-store.md ›
  // Thread mirror).
  void mirrorTranscript(thread, key);
}

function watchRemoteTranscripts(next: AppState, prev: AppState): void {
  if (next.threads === prev.threads) return;
  for (const [key, thread] of Object.entries(next.threads)) {
    if (thread === prev.threads[key] || !isRemoteThread(key) || transcriptTimers.has(key)) continue;
    transcriptTimers.set(
      key,
      setTimeout(() => keepRemoteTranscript(key), KEEP_TRANSCRIPT_MS),
    );
  }
}
/** `state/machines.ts` `resetMachinesForTests`, once its module is loaded. */

/** The connection a thread's operations go over: this computer's engine, or that machine's own. */
export function connectionFor(host: string): AcpConnection | null {
  return host ? (machineConnections.get(host) ?? null) : connection;
}

// The thread mirror writes through this computer's own engine (`state/machineMirror.ts`).
setMirrorRequest(async (params) => {
  const engine = connectionFor("");
  if (!engine) throw new Error("This computer's engine is not connected.");
  return engine.request(Method.mirror, params as JsonObject, 15_000);
});

/** The same, from a thread key (`state/threadKeys.ts`). */
export function connectionForThread(key: string): AcpConnection | null {
  return connectionFor(splitThreadKey(key).host);
}

/** Whether the thread runs on another computer. */
export function threadIsRemote(key: string): boolean {
  return isRemoteThread(key);
}

/** The name of the machine a remote thread runs on; undefined for this computer. */
export function machineOfThread(key: string): string | undefined {
  return useApp.getState().machineNames[splitThreadKey(key).host];
}

/** Whether a machine's threads are listed but its connection is not open (read-only). */
export function threadOffline(key: string | null | undefined): boolean {
  if (!key) return false;
  const host = splitThreadKey(key).host;
  return !!host && !machineConnections.has(host);
}

/**
 * The harnesses, models and accounts one computer lists: this one's own, or a paired
 * machine's read over its connection (`loadMachineHarnesses`). Nothing for a machine that has
 * not answered yet, which the pickers draw as loading rather than as an empty engine.
 */
export function harnessesOn(
  s: { harnesses: HarnessSnapshot | null; remoteHarnesses?: Record<string, HarnessSnapshot> },
  host: string | null | undefined,
): HarnessSnapshot | null {
  return host ? (s.remoteHarnesses?.[host] ?? null) : s.harnesses;
}

/** The machine a thread runs on, as the chat's header reads it, and whether it is away now. */
export function useMachineOfThread(key: string | null | undefined): string | undefined {
  const names = useApp((s) => s.machineNames);
  return key ? names[splitThreadKey(key).host] : undefined;
}

/** A remote thread's machine is away: the row stays, the thread opens read-only (ChatView, Composer). */
export function useThreadOffline(key: string | null | undefined): boolean {
  const hosts = useApp((s) => s.connectedHosts);
  if (!key) return false;
  const host = splitThreadKey(key).host;
  return !!host && !hosts.includes(host);
}

/**
 * Follow the shell's machines: one inbound subscription per connection (so a
 * machine's notifications reach the transcript tagged with its key), and its
 * thread list mirrored into `sessions` whenever it is re-read.
 */
export async function startMachineRouting(): Promise<void> {
  if (machineRouting) return machineRouting;
  machineRouting = Promise.resolve().then(() => {
    onMachineLinks((id, conn) => watchMachineConnection(id, conn));
    onMachineThreadsReady((id, conn) => {
      void subscribePullWatches(conn, id).catch(() => {});
    });
    const mirror = () => {
      const state = useMachines.getState();
      const names = Object.fromEntries(state.machines.map((machine) => [machine.id, machineName(machine)]));
      useApp.setState((s) => {
        // A machine that was removed takes its kept harnesses with it, and **Run on** naming it
        // goes back here rather than to a machine that is no longer saved.
        const snapshots = Object.fromEntries(
          Object.entries(s.remoteHarnesses).filter(([host]) => !!names[host]),
        );
        const named = s.newThreadHost && !names[s.newThreadHost];
        return {
          machineNames: names,
          remoteHarnesses: snapshots,
          ...(named
            ? {
                newThreadHost: LOCAL_HOST,
                projectCwd: "",
                newThread: s.newThreadDrafts[LOCAL_HOST] ?? null,
                newThreadAccount: s.newThreadAccounts[LOCAL_HOST] ?? null,
              }
            : {}),
        };
      });
      mergeMachineLists(state.sessions);
    };
    machineMirrorOff = useMachines.subscribe((state, prev) => {
      if (state.sessions !== prev.sessions || state.machines !== prev.machines) mirror();
    });
    transcriptKeepOff = useApp.subscribe(watchRemoteTranscripts);
    for (const link of machineLinks()) watchMachineConnection(link.machine.id, link.connection);
    mirror();
  });
  return machineRouting;
}

/**
 * Tests only: forget every machine's channel, the threads it listed and the names it had, so one
 * test file's machines cannot be another's (machinesThreads.test.ts, `resetForTests`).
 */
export function resetMachinesForTests(): void {
  for (const off of machineOff.values()) off();
  machineOff.clear();
  machineConnections.clear();
  machineRouting = null;
  machineMirrorOff?.();
  machineMirrorOff = null;
  transcriptKeepOff?.();
  transcriptKeepOff = null;
  for (const timer of transcriptTimers.values()) clearTimeout(timer);
  transcriptTimers.clear();
  resetMachineStoreForTests();
  useApp.setState({ connectedHosts: [], machineNames: {}, sessions: [] });
}

/**
 * Point the store at a machine's live connection (or at none, when it drops).
 * `startMachineRouting` calls it for every change `state/machines.ts` announces;
 * exported so the routing is testable without the app's shell (machinesThreads.test.ts).
 */
export function watchMachineConnection(id: string, next: AcpConnection | null): void {
  const current = machineConnections.get(id) ?? null;
  if (current === next) return;
  machineOff.get(id)?.();
  machineOff.delete(id);
  if (!next) {
    machineConnections.delete(id);
    // Its threads stay listed and keep the copy this window drew, read-only: the last replay is
    // still the thread. A turn that was running there ended with the connection, and the request
    // it waited on died with it (the engine keeps no pending-permission replay): its card goes
    // with the turn's own failure rather than holding an answer no connection can carry. A request
    // on a thread that was not running goes too, for the same reason (`reply` refuses an answer
    // made in the moment before this runs).
    useApp.setState((s) => ({
      connectedHosts: [...machineConnections.keys()],
      threads: Object.fromEntries(
        Object.entries(s.threads).map(([key, t]) => {
          if (splitThreadKey(key).host !== id) return [key, t];
          if (isRunning(t))
            return [
              key,
              { ...finishTurn(t, "failed", Date.now(), MACHINE_LOST_TURN), enginePhase: undefined },
            ];
          return [
            key,
            t.permissions.length || t.enginePhase ? { ...t, permissions: [], enginePhase: undefined } : t,
          ];
        }),
      ),
    }));
    return;
  }
  machineConnections.set(id, next);
  void subscribeWorkflows(next).catch(() => {});
  machineOff.set(
    id,
    next.onInbound((message) => void handleInbound(scopeInbound(message, id), next, id)),
  );
  // Back: a harness or login added there while it was away is in the list a second read gives.
  if (useApp.getState().remoteHarnesses[id]) void loadMachineHarnesses(id, { refresh: true });
  // Back: a turn whose request died with this machine's channel is asked about again now.
  resumeLostTurns(id);
  // Back: the copy on screen is from before it went away, so the thread it shows is read again.
  // Requests of the old connection did not survive the drop (they died with it), so there is
  // nothing to keep here: the reload is the machine's whole answer about the thread.
  useApp.setState((s) => ({
    connectedHosts: [...machineConnections.keys()],
    threads: Object.fromEntries(
      Object.entries(s.threads).map(([key, t]) =>
        splitThreadKey(key).host === id
          ? [key, { ...endTurn(t), enginePhase: undefined, load: { phase: "idle" as const } }]
          : [key, t],
      ),
    ),
  }));
  // Reload background transcripts too: they may have finished while disconnected.
  for (const key of Object.keys(useApp.getState().threads)) {
    if (splitThreadKey(key).host === id) void openThread(key);
  }
  for (const row of useApp.getState().sessions) {
    if (row.host === id && (row.status === "running" || row.status === "needs_input"))
      void openThread(row.id);
  }
}

/** A machine's ids reach the store as keys: `‹host›/‹sessionId›`. */
function scopeInbound(message: Inbound, host: string): Inbound {
  if (message.type === "closed") return message;
  const sessionId = str(message.params.sessionId);
  if (!sessionId) return message;
  return { ...message, params: { ...message.params, sessionId: threadKey(host, sessionId) } };
}
