// A paired machine that follows this computer's settings (docs/desktop.md ›
// Settings on one machine or all). Off by default, one switch per machine on
// its Settings page. While it is on, this computer's shared server settings are
// written to that machine whenever its connection opens, and each change made
// here is written there too. A value changed on that machine itself is put back
// to this computer's on the next connection: this computer is the main one.
//
// Only the shared rows move (`SERVER_SETTING_KEYS`). Credentials, accounts,
// local paths and app preferences stay where they are, as under All machines.

import { create } from "zustand";
import { type JsonObject, obj } from "../acp/jsonrpc";
import { readStorage, writeStorage } from "../lib/storage";
import { SERVER_SETTING_KEYS } from "./settingsRegistry";
import { LOCAL_HOST } from "./threadKeys";

const STORAGE = "rustcode.settingsFollow";
const REQUEST_MS = 10_000;

function readIds(): string[] {
  try {
    const raw = JSON.parse(readStorage(STORAGE) ?? "[]") as unknown;
    return Array.isArray(raw) ? raw.filter((id): id is string => typeof id === "string") : [];
  } catch {
    return [];
  }
}

/** The machines that follow this computer's settings, by id. */
export const useSettingsFollow = create<{ ids: string[] }>(() => ({ ids: readIds() }));

export function followsThisComputer(id: string): boolean {
  return useSettingsFollow.getState().ids.includes(id);
}

/** Turn following on or off for one machine; turning it on copies the settings at once. */
export function setFollowsThisComputer(id: string, on: boolean): Promise<number> {
  const ids = useSettingsFollow.getState().ids.filter((entry) => entry !== id);
  if (on) ids.push(id);
  writeStorage(STORAGE, JSON.stringify(ids));
  useSettingsFollow.setState({ ids });
  return on ? copySettingsTo(id) : Promise.resolve(0);
}

/** Forget a machine that was removed. */
export function forgetFollower(id: string): void {
  if (followsThisComputer(id)) void setFollowsThisComputer(id, false);
}

/**
 * The shared values a follower needs so it matches this computer: every shared
 * key this computer holds whose value there differs (or is missing).
 */
export function followValues(local: JsonObject, remote: JsonObject | null | undefined): JsonObject {
  const out: JsonObject = {};
  for (const key of SERVER_SETTING_KEYS) {
    const value = local[key];
    if (value === undefined) continue;
    if (remote && JSON.stringify(remote[key]) === JSON.stringify(value)) continue;
    out[key] = value;
  }
  return out;
}

const copies = new Map<string, Promise<number>>();
const again = new Set<string>();

/**
 * Write this computer's shared settings to one machine, where they differ. Answers how many values
 * changed there; a machine that is not connected is skipped and gets them when it connects. A call
 * while a copy to that machine runs makes it copy once more when it ends, so a change made
 * meanwhile is not lost.
 */
export function copySettingsTo(host: string): Promise<number> {
  const running = copies.get(host);
  if (running) {
    again.add(host);
    return running;
  }
  const task = copyOnce(host).finally(() => {
    copies.delete(host);
    if (again.delete(host)) void copySettingsTo(host).catch(() => 0);
  });
  copies.set(host, task);
  return task;
}

async function copyOnce(host: string): Promise<number> {
  const scope = await import("./settingsScope");
  let local: JsonObject | undefined;
  try {
    local = obj(
      obj(await scope.machineRequest(LOCAL_HOST, "_rustcode/settings/get", {}, REQUEST_MS))?.values,
    );
  } catch {
    // An older local server: the app's own values are this computer's settings.
  }
  local ??= scope.localServerSettingValues() as unknown as JsonObject;
  let remote: JsonObject | undefined;
  try {
    remote = obj(obj(await scope.machineRequest(host, "_rustcode/settings/get", {}, REQUEST_MS))?.values);
  } catch {
    return 0;
  }
  const values = followValues(local, remote);
  const count = Object.keys(values).length;
  if (count) {
    await scope.machineRequest(host, "_rustcode/settings/set", { values }, REQUEST_MS);
    await scope.loadServerSettings(host, true).catch(() => undefined);
  }
  return count;
}

/**
 * This computer's `settings.json` changed (this app, the CLI, an agent's `manage_settings`): bring
 * every connected follower up to date. One that is away catches up when it connects.
 */
export function followLocalChange(): void {
  const ids = useSettingsFollow.getState().ids;
  if (!ids.length) return;
  void import("./machines").then((machines) => {
    for (const id of ids) if (machines.machineConnection(id)) void copySettingsTo(id).catch(() => 0);
  });
}

let started = false;

/** Copy to a follower each time its connection opens. */
export function startSettingsFollow(): void {
  if (started) return;
  started = true;
  void import("./machines").then((machines) => {
    machines.onMachineLinks((id, connection) => {
      if (connection && id !== LOCAL_HOST && followsThisComputer(id)) void copySettingsTo(id).catch(() => 0);
    });
  });
}
