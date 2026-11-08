// Updates, after the SwiftUI app's Updater: a check on launch, every 30
// seconds while the window is in front and every 30 minutes behind it, an "Update to X" row at the foot of the sidebar
// when a newer release is out, and What's New with the release's notes the
// first time a new version starts. The shell does the work
// (src-tauri/src/updater.rs); this keeps what the UI shows, the update channel
// and the beta the user skipped.

import { create } from "zustand";
import { openExternal } from "../lib/native";
import { shellCall } from "../lib/shell";
import {
  appVersion,
  canUpdate,
  checkForUpdate,
  type InstallProgress,
  installUpdate,
  isDevBuild,
  type UpdateChannel,
  type UpdateInfo,
  updateChannels,
} from "../lib/updater";

/** How often the app checks while its window is in front: a HEAD request to the release page. */
export const UPDATE_CHECK_MS = 30_000;
/** How often it checks while hidden or in the background. */
export const UPDATE_CHECK_HIDDEN_MS = 30 * 60_000;
const SEEN_KEY = "rustcode.seenVersion";
/** Written just before an in-app install: the next launch shows What's New whatever else it knows. */
const UPDATED_FROM_KEY = "rustcode.updatedFrom";
/** Stable or Beta (Settings › About). */
const CHANNEL_KEY = "rustcode.updateChannel";
/** The one beta the user skipped; the next beta is offered as usual. */
const SKIPPED_KEY = "rustcode.skippedBeta";

interface UpdatesState {
  /** This app's version from the shell, known before any check answers. */
  version: string | null;
  info: UpdateInfo | null;
  /** What checks look for: every release, or the betas of the next one too. */
  channel: UpdateChannel;
  /** The channels this build offers, from the shell (no network): `beta` only where a beta has an app. */
  channels: UpdateChannel[];
  /** The beta hidden from the row and from Settings, when one was skipped. */
  skipped: string | null;
  /** Why the last check failed, for Settings › Advanced; null after a good check. */
  error: string | null;
  checkedAt: number | null;
  installing: boolean;
  /** The install's progress, for the sidebar row's bar. */
  progress: InstallProgress | null;
  /** The release notes to show once, after the app was updated. */
  whatsNew: { version: string; notes: string | null } | null;
}

export const useUpdates = create<UpdatesState>(() => ({
  version: null,
  info: null,
  channel: readChannel(read(CHANNEL_KEY)),
  channels: ["stable"],
  skipped: read(SKIPPED_KEY),
  error: null,
  checkedAt: null,
  installing: false,
  progress: null,
  whatsNew: null,
}));

/** `channel` as stored, with anything unknown read as Stable: never a surprise beta. */
export function readChannel(value: string | null): UpdateChannel {
  return value === "beta" ? "beta" : "stable";
}

/** Whether a version is a prerelease: a beta, as `1.21.0-beta.2`. */
export function isPrerelease(version: string): boolean {
  return versionParts(version)[1].length > 0;
}

/**
 * The version and its prerelease fields: `1.21.0-beta.10` is
 * `[[1,21,0],["beta",10]]`. Build metadata (`1.21.0+build.7`) takes no part in
 * ordering, so it is dropped.
 */
function versionParts(version: string): [number[], string[]] {
  const text = version.trim().replace(/^v/, "").split("+")[0] ?? "";
  const dash = text.indexOf("-");
  const core = (dash === -1 ? text : text.slice(0, dash)).split(".");
  const pre =
    dash === -1
      ? []
      : text
          .slice(dash + 1)
          .split(".")
          .filter(Boolean);
  return [core.map((n) => Number.parseInt(n, 10) || 0), pre];
}

/** Semver's order for one prerelease field: numbers count as numbers, before text. */
function compareIdentifier(a: string, b: string): number {
  const [na, nb] = [/^\d+$/.test(a), /^\d+$/.test(b)];
  if (na && nb) return Math.sign(Number(a) - Number(b));
  if (na) return -1;
  if (nb) return 1;
  return a === b ? 0 : a < b ? -1 : 1;
}

/** Semver's order: `1.21.0-beta.2` is older than `1.21.0`, and `.10` newer than `.9`. */
export function compareVersions(a: string, b: string): number {
  const [coreA, preA] = versionParts(a);
  const [coreB, preB] = versionParts(b);
  for (let i = 0; i < Math.max(coreA.length, coreB.length); i++) {
    const left = coreA[i] ?? 0;
    const right = coreB[i] ?? 0;
    if (left !== right) return left > right ? 1 : -1;
  }
  // A release is newer than any prerelease of it.
  if (preA.length === 0 || preB.length === 0) {
    return preA.length === preB.length ? 0 : preA.length ? -1 : 1;
  }
  // Field by field up to the shorter suffix, then the longer one is newer:
  // `1.0.0-alpha` is older than `1.0.0-alpha.1`, and the order holds both ways.
  for (let i = 0; i < Math.min(preA.length, preB.length); i++) {
    const order = compareIdentifier(preA[i]!, preB[i]!);
    if (order !== 0) return order;
  }
  return Math.sign(preA.length - preB.length);
}

/** `0.116.0` is newer than `0.115.2`; a beta is older than the release it leads to. */
export function newerVersion(a: string, b: string): boolean {
  return compareVersions(a, b) > 0;
}

/**
 * Whether What's New shows for `current`, given the version last seen and the
 * version an in-app install started from: only after an update, never on a
 * first install and never for a dev build.
 */
export function shouldShowWhatsNew(
  current: string,
  seen: string | null,
  development: boolean,
  updatedFrom: string | null = null,
): boolean {
  if (development) return false;
  if (updatedFrom && newerVersion(current, updatedFrom)) return true;
  return !!seen && newerVersion(current, seen);
}

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string | null) {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // Not kept in a private window.
  }
}

/** On launch, from the app's own version (no network): What's New after an update. */
async function whatsNewOnLaunch() {
  const current = await appVersion();
  if (!current) return;
  const seen = read(SEEN_KEY);
  const updatedFrom = read(UPDATED_FROM_KEY);
  write(SEEN_KEY, current);
  write(UPDATED_FROM_KEY, null);
  if (!shouldShowWhatsNew(current, seen, isDevBuild(), updatedFrom)) return;
  useUpdates.setState({ whatsNew: { version: current, notes: null } });
  // The notes follow when GitHub answers; offline the dialog links the release page.
  const notes = await shellCall<string | null>("release_notes", { version: current }).catch(() => null);
  if (useUpdates.getState().whatsNew?.version === current)
    useUpdates.setState({ whatsNew: { version: current, notes } });
}

/**
 * A failed check tries again sooner than the full interval — 30s, then doubling up to 5 minutes,
 * so a check that ran while the network was still coming up on launch does not wait half an hour.
 */
const RETRY_FIRST_MS = 30_000;
const RETRY_MAX_MS = 5 * 60_000;
let retry: ReturnType<typeof setTimeout> | undefined;
let retryDelay = RETRY_FIRST_MS;

let checking: Promise<void> | null = null;

/** Check now: on launch, on the interval, when the app comes back, and from Settings › Advanced. */
export function checkNow(): Promise<void> {
  // One check at a time: a second caller waits for the one already running.
  checking ??= runCheck().finally(() => {
    checking = null;
  });
  return checking;
}

/** Opening Settings checks again when the last good check is missing or over 10 minutes old. */
export function checkIfStale() {
  const last = useUpdates.getState().checkedAt;
  if (canUpdate() && (!last || Date.now() - last > 10 * 60_000)) void checkNow();
}

async function runCheck(): Promise<void> {
  clearTimeout(retry);
  // The channel and the skipped beta are what the request was made with. A
  // change while it is in flight makes its answer stale, so it is dropped and
  // the check runs again with what the store says now: a Beta answer can never
  // land after the switch back to Stable and offer a beta.
  for (;;) {
    const { channel, skipped } = useUpdates.getState();
    let info: UpdateInfo;
    try {
      info = await checkForUpdate(channel, skipped);
    } catch (error) {
      // Offline, GitHub busy, or the shell refused: say so in Settings and try again soon.
      useUpdates.setState({ error: error instanceof Error ? error.message : String(error) });
      retry = setTimeout(() => void checkNow(), retryDelay);
      retryDelay = Math.min(retryDelay * 2, RETRY_MAX_MS);
      return;
    }
    const now = useUpdates.getState();
    if (now.channel !== channel || now.skipped !== skipped) continue;
    retryDelay = RETRY_FIRST_MS;
    useUpdates.setState({
      info,
      error: null,
      checkedAt: Date.now(),
      channels: info.channels,
    });
    return;
  }
}

/** How far an install is, for the sidebar row and Settings: a fraction when the size is known. */
export function installProgress(progress: InstallProgress | null): {
  fraction?: number;
  step: string;
} {
  const mb = (bytes: number) => `${(bytes / 1_048_576).toFixed(1)} MB`;
  if (!progress) return { fraction: 0, step: "Starting the download…" };
  if (progress.phase === "verifying") return { fraction: 1, step: "Verifying the download…" };
  if (progress.phase === "installing") return { fraction: 1, step: "Installing · RustCode reopens" };
  const fraction = progress.total ? progress.received / progress.total : undefined;
  return {
    fraction,
    step: progress.total
      ? `Downloading · ${Math.round((fraction ?? 0) * 100)}% of ${mb(progress.total)}`
      : `Downloading · ${mb(progress.received)}`,
  };
}

let started = false;

/**
 * The channels this build offers, asked of the shell once: the picker in
 * Settings needs no release check, so it is there before one answers and when
 * one fails offline. The browser has no shell and no updates.
 */
export function loadChannels(): Promise<UpdateChannel[]> {
  return updateChannels()
    .then((channels) => {
      useUpdates.setState({ channels });
      return channels;
    })
    .catch(() => useUpdates.getState().channels);
}

/** Whether `info` offers an update on the channel now selected, not the one before it. */
export function offersUpdate(info: UpdateInfo | null, channel: UpdateChannel): boolean {
  return !!info?.available && info.latest !== null && info.channel === channel;
}

/** Starts the checks once per app run; the browser UI has nothing to update. */
export function startUpdateChecks() {
  // `?updatePreview` in a dev browser: the row and What's New with sample data, for screenshots.
  if (import.meta.env.DEV && !canUpdate() && new URLSearchParams(location.search).has("updatePreview")) {
    useUpdates.setState({
      info: {
        current: "0.115.0",
        latest: "0.116.0",
        available: true,
        canInstall: true,
        development: false,
        page: null,
        channels: ["stable", "beta"],
        channel: "stable",
      },
      channels: ["stable", "beta"],
      whatsNew: {
        version: "0.116.0",
        notes:
          "## Desktop\n- The sidebar shows a live clock for running threads and Pin/Settle on hover.\n- ⌘O changes the folder of a thread; ⌘⇧O the default.\n- Reports like `/status` open in a panel above the composer.\n\n## Engine\n- The agent edits in place and no longer branches or commits unless asked.",
      },
    });
    return;
  }
  if (started || !canUpdate()) return;
  started = true;
  void loadChannels();
  void appVersion().then((version) => version && useUpdates.setState({ version }));
  void whatsNewOnLaunch();
  setTimeout(() => void checkNow(), 5_000);
  setInterval(() => {
    const last = useUpdates.getState().checkedAt;
    const hidden = document.visibilityState !== "visible" || !document.hasFocus();
    if (!hidden || !last || Date.now() - last >= UPDATE_CHECK_HIDDEN_MS) void checkNow();
  }, UPDATE_CHECK_MS);
  // Back in front after a while away (a laptop that slept), or the window revealed again: check.
  const relaunched = () => {
    const last = useUpdates.getState().checkedAt;
    if (!last || Date.now() - last > UPDATE_CHECK_MS) void checkNow();
  };
  window.addEventListener("focus", relaunched);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") relaunched();
  });
}

/** The sidebar row's action: install in place where the app can, else the release page. */
export async function applyUpdate(): Promise<string | null> {
  const { info, channel, skipped } = useUpdates.getState();
  // Only an offer from the channel now selected, never the one before it.
  if (!info || !offersUpdate(info, channel)) return null;
  if (!info.canInstall) {
    if (info.page) openExternal(info.page);
    return null;
  }
  useUpdates.setState({ installing: true, progress: null });
  write(UPDATED_FROM_KEY, info.current);
  try {
    await installUpdate(channel, skipped, (progress) => useUpdates.setState({ progress }));
    return null;
  } catch (error) {
    write(UPDATED_FROM_KEY, null);
    useUpdates.setState({ installing: false, progress: null });
    return error instanceof Error ? error.message : String(error);
  }
}

/**
 * Pick the update channel (Settings › About): Stable is every release, Beta also
 * offers the betas of the next one. The offer on screen belongs to the channel
 * it came from, so it stops showing as one (the row compares `info.channel`)
 * until the check with the new channel answers. Switching to Stable never
 * offers a downgrade: the shell only offers a version newer than this one.
 */
export function setUpdateChannel(channel: UpdateChannel) {
  if (channel === useUpdates.getState().channel) return;
  write(CHANNEL_KEY, channel);
  useUpdates.setState({ channel, checkedAt: null });
  void checkNow();
}

/**
 * Hide the beta the row offers; the next one shows as usual. Kept in
 * localStorage, so it outlasts a restart, and passed to the shell so a check
 * does not name it again.
 */
export function skipBeta(): string | null {
  const info = useUpdates.getState().info;
  if (!info?.latest || !isPrerelease(info.latest)) return null;
  write(SKIPPED_KEY, info.latest);
  useUpdates.setState({ skipped: info.latest, info: { ...info, available: false } });
  return info.latest;
}

export function closeWhatsNew() {
  useUpdates.setState({ whatsNew: null });
}

/** What's New for the running version on demand (Settings › Advanced). */
export async function showWhatsNew(version: string) {
  const notes = await shellCall<string | null>("release_notes", { version }).catch(() => null);
  useUpdates.setState({ whatsNew: { version, notes } });
}
