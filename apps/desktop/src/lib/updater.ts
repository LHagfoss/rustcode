// The shell's updater (src-tauri/src/updater.rs): the latest GitHub release,
// and on macOS an in-place install that quits and reopens the app.

import { isTauri } from "../acp/transport";
import { shellCall } from "./shell";

export interface UpdateInfo {
  current: string;
  latest: string | null;
  available: boolean;
  canInstall: boolean;
  development: boolean;
  page: string | null;
}

/** Only the app updates; the browser UI has nothing to replace. */
export function canUpdate(): boolean {
  return isTauri();
}

export function checkForUpdate(): Promise<UpdateInfo> {
  return shellCall<UpdateInfo>("update_check");
}

/** Resolves only on failure: on success the app quits and reopens updated. */
export function installUpdate(): Promise<void> {
  return shellCall<void>("update_install");
}
