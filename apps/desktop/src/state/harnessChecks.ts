// Running a Check: the app's one way to look at a harness. It detects the
// install again, verifies each account's sign-in and lists the models, and
// ends in a final state (`harnessCheck.ts`). Settings reads the state from
// `useHarnessChecks`; the engine's snapshot stays the source of everything else.

import { create } from "zustand";
import type { Harness } from "../acp/types";
import {
  type CheckEntry,
  type CheckResult,
  type CheckStep,
  failedResult,
  readyResult,
  settledEarly,
} from "./harnessCheck";
import { checkHarnessAccounts, fetchHarnessModels, useApp } from "./store";

interface ChecksState {
  byHarness: Record<string, CheckEntry>;
}

export const useHarnessChecks = create<ChecksState>(() => ({ byHarness: {} }));

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
  const joined = running.get(id);
  if (joined) return joined;
  const run = check(id).finally(() => running.delete(id));
  running.set(id, run);
  return run;
}

async function check(id: string): Promise<CheckResult> {
  const step = (s: CheckStep) => put(id, { running: s });
  let result: CheckResult;
  try {
    // One request re-detects the install and reads every account's own credentials.
    step("detect");
    await checkHarnessAccounts(true, [id], false);
    const harness = find(id);
    if (!harness) throw new Error("This harness is no longer listed.");
    result = settledEarly(harness) ?? (await listModels(id, step));
  } catch (error) {
    result = failedResult(error);
  }
  put(id, { result });
  return result;
}

async function listModels(id: string, step: (s: CheckStep) => void): Promise<CheckResult> {
  step("models");
  const count = await fetchHarnessModels(id, true);
  return readyResult(count, find(id)?.version);
}

/** Checks every harness but the built-in one, side by side. */
export function runAllHarnessChecks(): Promise<CheckResult[]> {
  const ids = (useApp.getState().harnesses?.harnesses ?? []).filter((h) => !h.builtin).map((h) => h.id);
  return Promise.all(ids.map(runHarnessCheck));
}

/** Forgets what a Check said, once the harness changed under it or is gone. */
export function forgetHarnessCheck(id: string) {
  put(id, null);
}
