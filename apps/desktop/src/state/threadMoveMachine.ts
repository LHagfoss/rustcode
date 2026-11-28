// Moving an existing thread between machines (This computer <-> paired machine, or paired <-> paired).
//
// Smart about both sides:
// - Project folder: remembers where a thread came from so moving back returns to the exact folder
//   (also persisted by the engine in `metadata.json` `machine_origins`), otherwise matches the
//   same project on the destination by exact path, git remote, or folder name (`state/projects.ts`),
//   and falls back to the destination's home folder.
// - Harness & account: keeps the same harness/model/effort when available on the destination,
//   restores the destination's remembered login/native session on bounce-back, or falls back to
//   the destination's default harness cleanly.
// - Safe order: exports in chunks under the 1 MiB gateway frame cap, imports and confirms durable
//   persistence on the destination FIRST, migrates unsent drafts/pins/selection/settings, and only
//   then removes the source copy.

import { create } from "zustand";
import { bool, type JsonObject, num, obj, RpcError, str } from "../acp/jsonrpc";
import { type HarnessSnapshot, Method, projectName, type SessionSummary } from "../acp/types";
import { unavailableReason } from "./harness";
import { lastAccountFor } from "./harnessActions";
import { newThreadAccountOn, type SignedOutLogins } from "./logins";
import { forgetTranscript } from "./machineCache";
import { clearMachineOrigins, rememberedMachineOrigin, rememberMachineOrigin } from "./machineOrigins";
import { forgetThreadOutbox } from "./machineOutbox";
import { type Machine, machineName, selfName, useMachines } from "./machines";
import { registeredProjects } from "./projectBook";
import { matchProject, type Project, projectAt, projectsOn } from "./projects";
import {
  type AppState,
  connectionFor,
  errorText,
  HOME_FOLDER,
  harnessesOn,
  keepUnlisted,
  loadMachineHarnesses,
  openThread,
  refreshSessions,
  STORAGE,
  selectThread,
  store,
  toast,
  tombstoneThread,
  untombstoneThread,
  useApp,
} from "./store";
import { isRunning, type ThreadState } from "./thread";
import { LOCAL_HOST, splitThreadKey, threadKey } from "./threadKeys";

const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

/** Threads currently in flight between machines: `threadKey -> destinationHost`. */
interface MovingMachineStore {
  moving: Record<string, string>;
}

const useMovingMachineStore = create<MovingMachineStore>(() => ({ moving: {} }));

/** Whether `threadId` is currently moving to another machine (and which host it is moving to). */
export function useMovingMachine(threadId: string | undefined): string | undefined {
  return useMovingMachineStore((s) => (threadId ? s.moving[threadId] : undefined));
}

function setMoving(threadId: string, toHost: string | null): void {
  useMovingMachineStore.setState((s) => {
    if (toHost === null) {
      const { [threadId]: _done, ...moving } = s.moving;
      return { moving };
    }
    return { moving: { ...s.moving, [threadId]: toHost } };
  });
}

export { rememberedMachineOrigin, rememberMachineOrigin };

export function resetMoveMachineForTests(): void {
  clearMachineOrigins();
  useMovingMachineStore.setState({ moving: {} });
}

// ---------------------------------------------------------------- planning

export interface MachineMoveDestination {
  /** Suggested absolute path on `toHost`, or undefined to let the destination use its home folder. */
  suggestedCwd?: string;
  /** Short human label shown in the picker row (project name or `"Home folder"`). */
  displayFolder: string;
  /** True when the thread previously ran on `toHost` and will return to its remembered folder. */
  returning: boolean;
}

/**
 * Predicts where a thread will land on `toHost`:
 * 1. Its remembered folder on `toHost` (when moving back).
 * 2. The matching checkout on `toHost` (exact path, then git remote, then folder name).
 * 3. `toHost`'s home folder.
 */
export function planMachineMoveDestination(args: {
  sessionId: string;
  fromHost: string;
  fromCwd: string;
  toHost: string;
  sessions: readonly SessionSummary[];
  localProjects?: readonly { path: string }[];
  rememberedCwd?: string;
}): MachineMoveDestination {
  const { sessionId, fromHost, fromCwd, toHost, sessions, localProjects = [] } = args;
  const remembered = args.rememberedCwd ?? rememberedMachineOrigin(sessionId, toHost);
  if (remembered) {
    return {
      suggestedCwd: remembered,
      displayFolder: projectName(remembered),
      returning: true,
    };
  }
  const source: Project | undefined =
    projectAt(sessions, fromCwd, fromHost) ??
    (fromCwd && fromCwd !== HOME_FOLDER ? { path: fromCwd, name: projectName(fromCwd) } : undefined);

  const candidates: Project[] = [...projectsOn(sessions, toHost)];
  if (toHost === LOCAL_HOST) {
    const known = new Set(candidates.map((p) => p.path));
    for (const proj of localProjects) {
      if (proj.path && !known.has(proj.path)) {
        candidates.push({ path: proj.path, name: projectName(proj.path) });
        known.add(proj.path);
      }
    }
  }

  const matched = matchProject(source, candidates);
  if (matched) {
    return {
      suggestedCwd: matched.path,
      displayFolder: matched.name,
      returning: false,
    };
  }
  return {
    suggestedCwd: undefined,
    displayFolder: "Home folder",
    returning: false,
  };
}

export interface TargetHarnessPlan {
  harness: string;
  account?: string;
  model?: string;
  effort?: string;
  harnessChanged: boolean;
}

/**
 * Chooses the harness, account, model and effort on the destination machine:
 * keeps the thread's current harness when installed & enabled there, otherwise falls back
 * to the destination's default harness (or the built-in RustCode harness).
 */
export function resolveTargetHarness(args: {
  sourceHarness?: string;
  sourceAccount?: string | null;
  sourceModel?: string | null;
  sourceEffort?: string | null;
  targetSnapshot: HarnessSnapshot | null;
  signedOutLogins?: SignedOutLogins;
}): TargetHarnessPlan {
  const from = args.sourceHarness || "rustcode";
  const snapshot = args.targetSnapshot;
  if (!snapshot) {
    return {
      harness: from,
      account: args.sourceAccount ?? undefined,
      model: args.sourceModel ?? undefined,
      effort: args.sourceEffort ?? undefined,
      harnessChanged: false,
    };
  }

  const usable = (id: string | undefined) => {
    if (!id) return undefined;
    const h = snapshot.harnesses.find((entry) => entry.id === id);
    if (!h) return undefined;
    if (h.builtin) return h;
    return unavailableReason(h) ? undefined : h;
  };

  const same = usable(from);
  if (same) {
    const accountMatches =
      args.sourceAccount && same.accounts?.some((a) => a.id === args.sourceAccount)
        ? args.sourceAccount
        : undefined;
    return {
      harness: same.id,
      // When undefined, the destination engine checks its remembered origin account first,
      // then falls back to its own default login.
      account: accountMatches,
      model: args.sourceModel ?? undefined,
      effort: args.sourceEffort ?? undefined,
      harnessChanged: false,
    };
  }

  const fallback =
    usable(snapshot.default.harness) ??
    snapshot.harnesses.find((h) => !h.builtin && !unavailableReason(h)) ??
    snapshot.harnesses.find((h) => h.builtin);
  const chosenId = fallback?.id ?? "rustcode";
  const account = fallback
    ? (newThreadAccountOn(fallback, lastAccountFor(chosenId), args.signedOutLogins ?? {}) ?? undefined)
    : undefined;
  return {
    harness: chosenId,
    account,
    model:
      fallback?.last?.model ??
      (chosenId === snapshot.default.harness ? (snapshot.default.model ?? undefined) : undefined),
    effort:
      fallback?.last?.effort ??
      (chosenId === snapshot.default.harness ? (snapshot.default.effort ?? undefined) : undefined),
    harnessChanged: chosenId !== from,
  };
}

/**
 * Why an open thread cannot move to another machine right now, or undefined when ready.
 */
export function moveMachineBlockedReason(args: {
  running: boolean;
  sourceHost: string;
  sourceConnected: boolean;
  sourceMachineName?: string;
  moving?: boolean;
}): string | undefined {
  if (args.moving) return "Moving to another machine…";
  if (args.running) return "Stop the running turn before moving this thread.";
  if (args.sourceHost && !args.sourceConnected) {
    return `${args.sourceMachineName ?? "That machine"} is offline. Reconnect it to move this thread.`;
  }
  return undefined;
}

// ---------------------------------------------------------------- execution

function hostDisplayName(host: string, machines: readonly Machine[]): string {
  if (!host) return selfName();
  const m = machines.find((entry) => entry.id === host);
  return m ? machineName(m) : (get().machineNames[host] ?? "That machine");
}

function hostStableId(host: string): string {
  // Paired machines are keyed by their persistent gateway `host_id`.
  // Local machine passes undefined so the engine fills its own `fleet::own_id`,
  // falling back to `"local"` if no gateway id has been created yet.
  return host || "local";
}

function harnessDisplayName(host: string, harnessId: string): string {
  return harnessesOn(get(), host)?.harnesses.find((h) => h.id === harnessId)?.name ?? harnessId;
}

/**
 * Move an idle thread from its current machine to `toHost` (`LOCAL_HOST` for this computer,
 * or a paired machine's id). Returns the thread's new key (`threadKey(toHost, sessionId)`)
 * on success, or `null` if refused or failed.
 */
export async function moveThreadToMachine(id: string, toHost: string): Promise<string | null> {
  const { host: fromHost, id: sessionId } = splitThreadKey(id);
  if (fromHost === toHost) return null;
  if (useMovingMachineStore.getState().moving[id]) return null;

  const s = get();
  const thread: ThreadState | undefined = s.threads[id];
  const summary = s.sessions.find((x) => x.id === id);
  if (isRunning(thread) || summary?.status === "running") {
    toast("Stop the running turn before moving this thread to another machine.");
    return null;
  }

  const machines = useMachines.getState().machines;
  const fromName = hostDisplayName(fromHost, machines);
  const toName = hostDisplayName(toHost, machines);

  const fromConn = connectionFor(fromHost);
  if (!fromConn) {
    toast(`${fromName} is not connected.`, "error");
    return null;
  }
  const toConn = connectionFor(toHost);
  if (!toConn) {
    toast(`${toName} is not connected.`, "error");
    return null;
  }

  if ((fromHost === LOCAL_HOST || toHost === LOCAL_HOST) && s.agent && !s.agent.sessionTransfer) {
    toast("This server cannot move threads between machines yet. Update RustCode.", "error");
    return null;
  }

  setMoving(id, toHost);
  try {
    // Ensure we know the destination machine's harnesses for smart fallback.
    const targetSnapshot = toHost ? await loadMachineHarnesses(toHost, { report: false }) : get().harnesses;

    const fromCwd = thread?.cwd || summary?.cwd || "";
    const dest = planMachineMoveDestination({
      sessionId,
      fromHost,
      fromCwd,
      toHost,
      sessions: get().sessions,
      localProjects: registeredProjects(),
    });

    const currentModel = thread?.configOptions.find((o) => o.id === "model")?.current ?? summary?.model;
    const currentEffort = thread?.configOptions.find((o) => o.id === "effort")?.current;
    const harnessPlan = resolveTargetHarness({
      sourceHarness: thread?.harness?.id ?? summary?.harness,
      sourceAccount: thread?.harness?.account ?? summary?.account,
      sourceModel: currentModel,
      sourceEffort: currentEffort,
      targetSnapshot,
      signedOutLogins: get().signedOutLogins,
    });

    // 1. Export the thread in chunks from the source machine (source stays untouched!).
    const chunks: { offset: number; data: string }[] = [];
    let exportId: string | undefined;
    let offset = 0;
    while (true) {
      let page: JsonObject | undefined;
      try {
        page = obj(
          await fromConn.request(
            Method.sessionExport,
            {
              sessionId,
              offset,
              ...(exportId ? { exportId } : {}),
            },
            30_000,
          ),
        );
      } catch (error) {
        if (error instanceof RpcError && error.isMethodNotFound) {
          toast(`${fromName} needs a newer RustCode to move threads.`, "error");
          return null;
        }
        toast(`Could not read the thread from ${fromName}: ${errorText(error)}`, "error");
        return null;
      }
      const data = str(page?.data) ?? "";
      exportId = str(page?.exportId) ?? exportId;
      chunks.push({ offset, data });
      if (bool(page?.done) === true) break;
      const next = num(page?.nextOffset);
      if (next === undefined || next <= offset) {
        toast(`Could not read the thread from ${fromName}: incomplete transfer.`, "error");
        return null;
      }
      offset = next;
    }

    // 2. Import the chunks onto the destination machine.
    let uploadId: string | undefined;
    let importResult: JsonObject | undefined;
    for (let i = 0; i < chunks.length; i++) {
      const chunk = chunks[i]!;
      const done = i === chunks.length - 1;
      const params: JsonObject = {
        offset: chunk.offset,
        data: chunk.data,
        done,
        ...(uploadId ? { uploadId } : {}),
        ...(done
          ? {
              ...(dest.suggestedCwd ? { suggestedCwd: dest.suggestedCwd } : {}),
              harness: harnessPlan.harness,
              ...(harnessPlan.account ? { account: harnessPlan.account } : {}),
              ...(harnessPlan.model ? { model: harnessPlan.model } : {}),
              ...(harnessPlan.effort ? { effort: harnessPlan.effort } : {}),
              fromMachineId: hostStableId(fromHost),
              fromMachineName: fromName,
              toMachineId: hostStableId(toHost),
              toMachineName: toName,
            }
          : {}),
      };
      try {
        const answer = obj(await toConn.request(Method.sessionImport, params, 30_000));
        if (!done) {
          uploadId = str(answer?.uploadId) ?? uploadId;
        } else {
          importResult = answer;
        }
      } catch (error) {
        if (error instanceof RpcError && error.isMethodNotFound) {
          toast(`${toName} needs a newer RustCode to move threads.`, "error");
          return null;
        }
        toast(`Could not move the thread to ${toName}: ${errorText(error)}`, "error");
        return null;
      }
    }

    const importedId = str(importResult?.sessionId) || sessionId;
    const importedCwd = str(importResult?.cwd) || dest.suggestedCwd || HOME_FOLDER;
    const importedTitle = str(importResult?.title) ?? summary?.title ?? thread?.title;
    const importedHarness = str(importResult?.harness) ?? harnessPlan.harness;
    const importedAccount = str(importResult?.account);
    const importedModel = str(importResult?.model) ?? harnessPlan.model;
    const harnessChanged = bool(importResult?.harnessChanged) ?? harnessPlan.harnessChanged;
    const resumedNative = bool(importResult?.resumedNative) ?? false;

    const newKey = threadKey(toHost, importedId);
    const wasSelected = get().selectedId === id;
    const wasPeeking = get().peekId === id;

    // 3. Remember where the thread came from so moving back previews & restores immediately.
    rememberMachineOrigin(importedId, fromHost, fromCwd);
    rememberMachineOrigin(importedId, toHost, importedCwd);

    // Clear any recent tombstone on the destination key (e.g. moving A -> B -> A within 2 minutes).
    untombstoneThread(newKey);

    // Build the destination summary row so the sidebar reflects the move immediately.
    const newSummary: SessionSummary = {
      id: newKey,
      cwd: importedCwd,
      title: importedTitle,
      updatedAt: Date.now(),
      harness: importedHarness,
      account: importedAccount,
      model: importedModel,
      status: summary?.status,
      pinned: summary?.pinned,
      snoozedUntil: summary?.snoozedUntil,
      ...(toHost ? { host: toHost } : {}),
    };
    keepUnlisted(newSummary);

    // Migrate client-side state (unsent draft, extras, pin, auto-compact, permissions, sidebar order).
    const before = get();
    const drafts = { ...before.drafts };
    if (id in drafts) {
      drafts[newKey] = drafts[id]!;
      delete drafts[id];
    }
    const extras = { ...before.extras };
    if (id in extras) {
      extras[newKey] = extras[id]!;
      delete extras[id];
    }
    const pinned = before.pinned.includes(id)
      ? [newKey, ...before.pinned.filter((x) => x !== id && x !== newKey)]
      : before.pinned;
    const autoCompactOff = { ...before.autoCompactOff };
    if (id in autoCompactOff) {
      autoCompactOff[newKey] = autoCompactOff[id]!;
      delete autoCompactOff[id];
    }
    const permissionModes = { ...before.permissionModes };
    if (id in permissionModes) {
      permissionModes[newKey] = permissionModes[id]!;
      delete permissionModes[id];
    }
    const { [id]: _oldThread, ...remainingThreads } = before.threads;
    set((curr) => ({
      threads: remainingThreads,
      drafts,
      extras,
      pinned,
      autoCompactOff,
      permissionModes,
      sessions: [newSummary, ...curr.sessions.filter((x) => x.id !== id && x.id !== newKey)],
      ...(wasSelected ? { selectedId: newKey } : {}),
      ...(wasPeeking ? { peekId: newKey } : {}),
    }));
    store(STORAGE.pinned, JSON.stringify(pinned));
    store(STORAGE.autoCompactOff, JSON.stringify(autoCompactOff));

    // Mirror into remote machine list cache when moving to a remote machine.
    if (toHost) {
      useMachines.setState((ms) => {
        const listed = (ms.sessions[toHost] ?? []).filter((x) => x.id !== importedId);
        const rawSummary: SessionSummary = { ...newSummary, id: importedId, host: undefined };
        return {
          sessions: { ...ms.sessions, [toHost]: [rawSummary, ...listed] },
          counts: { ...ms.counts, [toHost]: listed.length + 1 },
        };
      });
    }

    // 4. Destination has durably persisted the thread — now remove the source copy.
    tombstoneThread(id);
    forgetTranscript(id);
    forgetThreadOutbox(id);
    if (fromHost) {
      useMachines.setState((ms) => {
        const listed = ms.sessions[fromHost];
        if (!listed) return {};
        const next = listed.filter((x) => x.id !== sessionId);
        return {
          sessions: { ...ms.sessions, [fromHost]: next },
          counts: { ...ms.counts, [fromHost]: next.length },
        };
      });
    }
    let removeWarning = false;
    try {
      await fromConn.request(Method.sessionRemove, { sessionId }, 15_000);
    } catch {
      removeWarning = true;
    }

    // 5. Open the moved thread on its new machine so the transcript & move divider show immediately.
    if (wasSelected) {
      selectThread(newKey);
      await openThread(newKey);
    }
    if (fromHost === LOCAL_HOST || toHost === LOCAL_HOST) {
      void refreshSessions({ quiet: true });
    }

    const folder = importedCwd === HOME_FOLDER ? "Home folder" : projectName(importedCwd);
    if (removeWarning) {
      toast(`Moved to ${toName} · ${folder}, but the old copy on ${fromName} could not be removed.`, "info");
    } else if (resumedNative) {
      toast(`Back on ${toName} · ${folder}`, "success");
    } else if (harnessChanged) {
      toast(
        `Moved to ${toName} · ${folder} (using ${harnessDisplayName(toHost, importedHarness)})`,
        "success",
      );
    } else {
      toast(`Moved to ${toName} · ${folder}`, "success");
    }
    return newKey;
  } finally {
    setMoving(id, null);
  }
}
