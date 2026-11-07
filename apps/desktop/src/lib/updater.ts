// The shell's updater (src-tauri/src/updater.rs): the release the update
// channel offers, and on macOS an in-place install that quits and reopens the
// app.

import { isTauri } from "../acp/transport";
import { shellCall } from "./shell";

/** Stable releases, or the betas of the next one too (Settings › About). */
export type UpdateChannel = "stable" | "beta";

export interface UpdateInfo {
  current: string;
  latest: string | null;
  available: boolean;
  canInstall: boolean;
  development: boolean;
  page: string | null;
  /** The channels this build offers: `beta` only where a beta has an app. */
  channels: UpdateChannel[];
}

/** Only the app updates; the browser UI has nothing to replace. */
export function canUpdate(): boolean {
  return isTauri();
}

/** One check on `channel`, hiding the beta named `skipped` (a skipped beta only). */
export function checkForUpdate(channel: UpdateChannel, skipped: string | null): Promise<UpdateInfo> {
  return shellCall<UpdateInfo>("update_check", { channel, skipped });
}

/** Where an install is (updater.rs `InstallProgress`). */
export type InstallProgress =
  | { phase: "downloading"; received: number; total: number | null }
  | { phase: "verifying" }
  | { phase: "installing" };

/** Resolves only on failure: on success the app quits and reopens updated. */
export async function installUpdate(
  channel: UpdateChannel,
  skipped: string | null,
  onProgress: (progress: InstallProgress) => void,
): Promise<void> {
  const { Channel } = await import("@tauri-apps/api/core");
  const progress = new Channel<InstallProgress>();
  progress.onmessage = onProgress;
  return shellCall<void>("update_install", { channel, skipped, onProgress: progress });
}

/** This app's own version, without the network. */
export async function appVersion(): Promise<string | null> {
  if (!isTauri()) return null;
  const { getVersion } = await import("@tauri-apps/api/app");
  return getVersion().catch(() => null);
}

/** Whether `0.0.0`-style dev builds: `tauri dev` is a debug build. */
export const isDevBuild = () => import.meta.env.DEV;
