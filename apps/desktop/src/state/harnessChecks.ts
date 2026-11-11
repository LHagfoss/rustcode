// Running a Check: the app's one way to look at a harness. It detects the
// install again, verifies each account's sign-in and lists the models, and
// ends in a final state (`harnessCheck.ts`). Settings reads the state from
// `useHarnessChecks`; the engine's snapshot stays the source of everything else.

import { create } from "zustand";
import type { Harness } from "../acp/types";
import { type CheckEntry, type CheckResult, failedResult, readyResult, settledEarly } from "./harnessCheck";
import { checkHarnessAccounts, fetchHarnessModels, useApp } from "./store";

interface ChecksState {
  byHarness: Record<string, CheckEntry>;
  /** A Check all on its way: how many of its harnesses have ended. */
  all: { done: number; total: number } | null;
}

export const useHarnessChecks = create<ChecksState>(() => ({ byHarness: {}, all: null }));

function put(id: string, entry: CheckEntry | null) {
  useHarnessChecks.setState((s) => {
    const byHarness = { ...s.byHarness };
    if (entry) byHarness[id] = entry;
    else delete byHarness[id];
    return { byHarness };
  });
}

const find = (id: string): Harness | undefined =>
  useApp.getState().harnesses?.harnesses.find((h) => h.id === id);

const running = new Map<string, Promise<CheckResult>>();

/**
 * Checks one harness end to end and answers where it ended. Pressing Check
 * while one runs joins it. Nothing here throws: a failure is the result.
 */
export function runHarnessCheck(id: string): Promise<CheckResult> {
  return running.get(id) ?? track(id, check(id));
}

function track(id: string, run: Promise<CheckResult>): Promise<CheckResult> {
  const tracked = run.finally(() => running.delete(id));
  running.set(id, tracked);
  return tracked;
}

async function check(id: string): Promise<CheckResult> {
  let result: CheckResult;
  try {
    // One request re-detects the install and reads every account's own credentials.
    put(id, { running: "detect" });
    await checkHarnessAccounts(true, [id], false);
    result = await settle(id);
  } catch (error) {
    result = failedResult(error);
  }
  put(id, { result });
  return result;
}

/** Where a detected harness ends: an early answer, else it is started to list its models. */
async function settle(id: string): Promise<CheckResult> {
  const harness = find(id);
  if (!harness) throw new Error("This harness is no longer listed.");
  return settledEarly(harness) ?? (await listModels(id));
}

async function listModels(id: string): Promise<CheckResult> {
  put(id, { running: "models" });
  const count = await fetchHarnessModels(id, true);
  return readyResult(count, find(id)?.version);
}

/** Harnesses a Check all starts at once: each is a process and a login-shell probe. */
const ALL_CONCURRENCY = 2;

/**
 * Checks every harness but the built-in one, ending where pressing Check on
 * each would. One request detects them all (one plan read, one Keychain
 * read); then they are started two at a time, so none times out for the load
 * of the others. A harness already being checked is joined, not checked twice.
 */
export function runAllHarnessChecks(): Promise<CheckResult[]> {
  const ids = (useApp.getState().harnesses?.harnesses ?? []).filter((h) => !h.builtin).map((h) => h.id);
  const fresh = ids.filter((id) => !running.has(id));
  const total = ids.length;
  let done = total - fresh.length;
  const progress = () => useHarnessChecks.setState({ all: done < total ? { done, total } : null });
  progress();

  const detected = detectAll(fresh);
  let active = 0;
  const waiting: Array<() => void> = [];
  const slot = async () => {
    if (active < ALL_CONCURRENCY) active++;
    else await new Promise<void>((resume) => waiting.push(resume));
  };
  // A finished harness hands its place straight to the next one waiting.
  const release = () => {
    const next = waiting.shift();
    if (next) next();
    else active--;
  };

  for (const id of fresh) {
    track(
      id,
      (async () => {
        let result: CheckResult;
        try {
          await detected;
          await slot();
          try {
            result = await settle(id);
          } finally {
            release();
          }
        } catch (error) {
          result = failedResult(error);
        }
        put(id, { result });
        done++;
        progress();
        return result;
      })(),
    );
  }
  return Promise.all(ids.map((id) => running.get(id) as Promise<CheckResult>));
}

/** One request detects every harness; each shows "Checking…" until its turn to start. */
async function detectAll(ids: string[]): Promise<void> {
  for (const id of ids) put(id, { running: "detect" });
  if (ids.length) await checkHarnessAccounts(true, ids, false);
}

/** Forgets what a Check said, once the harness changed under it or is gone. */
export function forgetHarnessCheck(id: string) {
  put(id, null);
}
