// The shell's updater (src-tauri/src/updater.rs): the release the update
// channel offers, and an in-place install that quits and reopens the app: a
// macOS bundle, a Linux AppImage or .deb, or a Windows NSIS install.

import { isTauri } from "../acp/transport";
import { shellCall } from "./shell";

/** Stable releases, or the betas of the next one too (Settings › About). */
export type UpdateChannel = "stable" | "beta";

export interface UpdateInfo {
  current: string;
  latest: string | null;
  available: boolean;
  /** The app can replace itself here; otherwise Update opens the release page. */
  canInstall: boolean;
  /** How this copy updates when the default wording is not the whole story (the .deb asks for a password). */
  installNote: string | null;
  development: boolean;
  page: string | null;
  /** The channels this build offers: `beta` only where a beta has an app. */
  channels: UpdateChannel[];
  /** The channel this answer is for, so an answer to the old setting is not shown. */
  channel: UpdateChannel;
}

/** Only the app updates; the browser UI has nothing to replace. */
export function canUpdate(): boolean {
  return isTauri();
}

/**
 * The channels this build offers, with no release check: Settings can offer the
 * picker before a check answers, and when one fails for being offline.
 */
export function updateChannels(): Promise<UpdateChannel[]> {
  return shellCall<UpdateChannel[]>("update_channels");
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
