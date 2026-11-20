// What in Settings needs the person's attention, and on which page: the gear's
// dot says something does, and the nav puts the same dot on the page that says
// what and offers the fix. A paired machine that is not connected points at
// Machines; a problem `rc server status` reports (an older `rustcode` on
// PATH, a server on another version) points at Remote access, where the server
// row lists it with its fix.
//
// A server problem can be dismissed: it stays hidden, and drops its dot, until
// its words change (a new version, another path), so a dismissed warning comes
// back when it is about something else.

import { useMemo } from "react";
import { create } from "zustand";
import { useMachines } from "./machines";
import type { SettingsSection } from "./store";

export interface ServerProblem {
  code: string;
  message: string;
}

/** Problems that are a state, not a fault: they show on the page but put no dot on the gear. */
const QUIET_CODES = new Set(["not-installed", "restart-pending"]);

const STORAGE_KEY = "rustcode.settings.dismissedProblems";

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function readDismissed(): string[] {
  try {
    const parsed: unknown = JSON.parse(storage()?.getItem(STORAGE_KEY) ?? "[]");
    return Array.isArray(parsed) ? parsed.filter((key): key is string => typeof key === "string") : [];
  } catch {
    return [];
  }
}

/** One problem's identity: its code and its words, so a reworded one shows again. */
export function problemKey(problem: ServerProblem): string {
  return `${problem.code}\n${problem.message}`;
}

export const useDismissedProblems = create<{ keys: string[] }>(() => ({ keys: readDismissed() }));

export function dismissProblem(problem: ServerProblem): void {
  const key = problemKey(problem);
  const keys = [...useDismissedProblems.getState().keys.filter((k) => k !== key), key].slice(-50);
  useDismissedProblems.setState({ keys });
  try {
    storage()?.setItem(STORAGE_KEY, JSON.stringify(keys));
  } catch {
    // A private window keeps it for this session only.
  }
}

/** The problems still shown: every one the person has not dismissed in these words. */
export function visibleProblems(
  problems: readonly ServerProblem[],
  dismissed: readonly string[],
): ServerProblem[] {
  const hidden = new Set(dismissed);
  return problems.filter((problem) => !hidden.has(problemKey(problem)));
}

/** Each page that needs attention, with the reason in a few words, in nav order. */
export function settingsAttention(
  machinesAway: number,
  problems: readonly ServerProblem[],
  dismissed: readonly string[],
): Partial<Record<SettingsSection, string>> {
  const attention: Partial<Record<SettingsSection, string>> = {};
  if (machinesAway > 0) {
    attention.machines =
      machinesAway === 1 ? "a machine is not connected" : `${machinesAway} machines are not connected`;
  }
  const loud = visibleProblems(problems, dismissed).filter((problem) => !QUIET_CODES.has(problem.code));
  if (loud.length > 0) {
    attention.remote =
      loud.length === 1
        ? "the RustCode server has a warning"
        : `the RustCode server has ${loud.length} warnings`;
  }
  return attention;
}

/** The commands a problem names in backticks (`rc server restart`), for a copy action. */
export function problemCommand(message: string): string | null {
  for (const match of message.matchAll(/`([^`]+)`/g)) {
    const text = match[1]?.trim() ?? "";
    if (text.startsWith("rc ") || text.startsWith("rustcode ") || text.startsWith("brew ")) return text;
  }
  return null;
}

/** The pages that need attention now, for the gear and the nav. */
export function useSettingsAttention(): Partial<Record<SettingsSection, string>> {
  const away = useMachines((s) => s.machines.filter((m) => m.state !== "connected").length);
  const problems = useMachines((s) => s.server?.problems);
  const dismissed = useDismissedProblems((s) => s.keys);
  return useMemo(() => settingsAttention(away, problems ?? [], dismissed), [away, problems, dismissed]);
}
