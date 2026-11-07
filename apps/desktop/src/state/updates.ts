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
  return version.includes("-");
}

/** The version and its prerelease fields: `1.21.0-beta.10` is `[[1,21,0],["beta",10]]`. */
function versionParts(version: string): [number[], string[]] {
  const text = version.trim().replace(/^v/, "");
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
  if (preA.length !== preB.length) return preA.length ? -1 : 1;
  for (let i = 0; i < preA.length; i++) {
    const order = compareIdentifier(preA[i]!, preB[i]!);
    if (order !== 0) return order;
  }
  return 0;
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
  let info: UpdateInfo;
  try {
    // The store's own channel and skipped beta: a change of either re-checks.
    const { channel, skipped } = useUpdates.getState();
    info = await checkForUpdate(channel, skipped);
  } catch (error) {
    // Offline, GitHub busy, or the shell refused: say so in Settings and try again soon.
    useUpdates.setState({ error: error instanceof Error ? error.message : String(error) });
    retry = setTimeout(() => void checkNow(), retryDelay);
    retryDelay = Math.min(retryDelay * 2, RETRY_MAX_MS);
    return;
  }
  retryDelay = RETRY_FIRST_MS;
  useUpdates.setState({ info, error: null, checkedAt: Date.now() });
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
      },
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
  if (!info?.available) return null;
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
 * offers the betas of the next one. Checking again at once shows what it finds,
 * and drops the previous channel's offer so a stable setting never lures anyone
 * into a beta. Switching to Stable never offers a downgrade: the shell only
 * offers a version newer than this one.
 */
export function setUpdateChannel(channel: UpdateChannel) {
  if (channel === useUpdates.getState().channel) return;
  write(CHANNEL_KEY, channel);
  useUpdates.setState({ channel, info: null, checkedAt: null });
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
