// A paired machine that follows this computer's settings (docs/desktop.md ›
// Settings on one machine or all). Off by default, one switch per machine on
// its Settings page. While it is on, this computer's shared server settings are
// written to that machine whenever its connection opens, and each change made
// here is written there too. A value changed on that machine itself is put back
// to this computer's on the next connection: this computer is the main one.
//
// The shared rows move (`SERVER_SETTING_KEYS`) with skills, themes and commands
// (state/configFilesFollow.ts); credentials, accounts and local paths stay put.

import { create } from "zustand";
import { type JsonObject, obj } from "../acp/jsonrpc";
import { readStorage, writeStorage } from "../lib/storage";
import { errorDetail, recordAutoSync } from "./autoSyncStatus";
import { copyFiles, followedKinds } from "./configFilesFollow";
import { SERVER_SETTING_KEYS } from "./settingsRegistry";
import { syncStorageKey } from "./storageSync";
import { LOCAL_HOST } from "./threadKeys";

const STORAGE = "rustcode.settingsFollow";
const REQUEST_MS = 10_000;
/** How often this computer's skills, themes and commands are listed to notice an edit made on disk. */
const FILES_CHECK_MS = 30_000;

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
syncStorageKey(STORAGE, () => useSettingsFollow.setState({ ids: readIds() }));

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
  const task = copyOnce(host)
    .catch((error: unknown) => {
      recordAutoSync(host, "follow", "failed", errorDetail(error));
      throw error;
    })
    .finally(() => {
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
  } catch (error) {
    recordAutoSync(host, "follow", "skipped", `Its settings could not be read (${errorDetail(error)}).`);
    return 0;
  }
  const values = followValues(local, remote);
  const count = Object.keys(values).length;
  if (count) {
    await scope.machineRequest(host, "_rustcode/settings/set", { values }, REQUEST_MS);
    await scope.loadServerSettings(host, true).catch(() => undefined);
  }
  const files = await copyFilesTo(host, scope.machineRequest);
  if (files) recordAutoSync(host, "follow", files.outcome, files.detail);
  else
    recordAutoSync(
      host,
      "follow",
      "synced",
      count ? `${count} ${count === 1 ? "setting" : "settings"} changed.` : undefined,
    );
  return count;
}

/** The follower also takes this computer's skills, themes and commands, recorded as this computer's. */
async function copyFilesTo(
  host: string,
  request: (host: string, method: string, params: JsonObject, timeoutMs: number) => Promise<unknown>,
): Promise<{ outcome: "skipped" | "failed"; detail: string } | null> {
  if (!followsThisComputer(host)) return null;
  const { ownHostId } = await import("./fleet");
  const self = await ownHostId();
  if (!self)
    return { outcome: "skipped", detail: "This computer has no id yet, so its files were not copied." };
  try {
    const copied = await copyFiles(request, LOCAL_HOST, host, self, followedKinds(host));
    return copied === null
      ? { outcome: "skipped", detail: "Its server is too old to take skills, themes and commands." }
      : null;
  } catch (error) {
    return { outcome: "failed", detail: `Files: ${errorDetail(error)}` };
  }
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
let lastListing: string | null = null;

/** A fresh listing against the last one: one that could not be read keeps the last as the baseline. */
export function compareListing(
  previous: string | null,
  listing: string | null,
): { baseline: string | null; changed: boolean } {
  if (listing === null) return { baseline: previous, changed: false };
  return { baseline: listing, changed: previous !== null && listing !== previous };
}

/** List this computer's skills, themes and commands; a change since the last look reaches every follower. */
async function checkLocalFiles(seed = false): Promise<void> {
  if (!seed && !useSettingsFollow.getState().ids.length) return;
  const scope = await import("./settingsScope");
  const listing = await scope.machineRequest(LOCAL_HOST, "_rustcode/config/files/list", {}, REQUEST_MS).then(
    (answer) => JSON.stringify(answer),
    () => null,
  );
  const next = compareListing(lastListing, listing);
  lastListing = next.baseline;
  if (next.changed) followLocalChange();
}

/** Copy to a follower each time its connection opens. */
export function startSettingsFollow(): void {
  if (started) return;
  started = true;
  void import("./machines").then((machines) => {
    machines.onMachineLinks((id, connection) => {
      if (connection && id !== LOCAL_HOST && followsThisComputer(id)) void copySettingsTo(id).catch(() => 0);
    });
  });
  void checkLocalFiles(true).catch(() => undefined);
  setInterval(() => void checkLocalFiles().catch(() => undefined), FILES_CHECK_MS);
}
